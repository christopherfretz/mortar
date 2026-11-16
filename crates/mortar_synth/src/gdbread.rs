//! The reader of `.gdb_index`'s inputs (`spec/synth.md` §17.2 to §17.4,
//! S-G2 to S-G5a): the units of the live `.debug_info` sections, their
//! root DIEs and address ranges, and the GNU name sets, from the input
//! objects with their relocations. Phase 80 runs it, only with
//! `--gdb-index` (S-G1); [`crate::gdbindex`] builds the plan from what it
//! returns.
//!
//! Every read is bounded by its section; anything S-G2 to S-G5 does not
//! list is a refusal by name ([`ReadError::Refused`], §17.6), never a
//! guess.

use std::borrow::Cow;
use std::collections::HashMap;

use mortar_core::inputs::RelocForm;
use mortar_core::{FileId, SectionId};
use mortar_elf::consts::{SHT_REL, SHT_RELA};
use mortar_elf::raw::{Rel, Rela, Table};
use mortar_elf::{Object, SectionIndex, SymbolIndex, SymbolTable};
use mortar_merge::What;
use mortar_merge::eh::{RelocClass, Target};
use mortar_merge::phase::{Link, Relocs, target};

use crate::eh::CodeRef;

/// S-G3: a debug-bearing unit without a name set.
pub const INCOMPLETE: &str = "gdb-index-incomplete";
/// S-G2: a type unit.
pub const TYPE_UNIT: &str = "gdb-index-type-unit";
/// S-G2, S-G5: a unit, DIE, abbreviation, range list or address that
/// cannot be read.
pub const MALFORMED_UNIT: &str = "gdb-index-malformed-unit";
/// S-G4a: a set that does not parse.
pub const MALFORMED_SET: &str = "gdb-index-malformed-name-set";
/// S-G4a, S-G5a: a set naming a unit not in the output.
pub const ORPHAN_SET: &str = "gdb-index-orphan-name-set";
/// S-G4, S-G7: past the format's limits.
pub const TOO_LARGE: &str = "gdb-index-too-large";

/// What the reader reads the link through.
pub struct Source<'t, 's> {
    pub link: &'t dyn Link<'s>,
    pub relocs: &'t dyn Relocs,
    /// Whether a section is in the output: kept, assigned and live.
    pub in_output: &'t (dyn Fn(SectionId) -> bool + Sync),
    /// Whether a section is consumed by the index (S-G6:
    /// `Dropped(Consumed)`; a COMDAT copy is not).
    pub consumed: &'t (dyn Fn(SectionId) -> bool + Sync),
    /// The output's address size: 8, or 4 (ELFCLASS32).
    pub word: u8,
}

impl std::fmt::Debug for Source<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source").field("word", &self.word).finish_non_exhaustive()
    }
}

/// A refusal of §17.6, or an input that could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    Refused {
        /// One of the names of §17.6.
        name: &'static str,
        file: FileId,
        /// The section and the offset in it, when one is known.
        at: Option<(SectionIndex, u64)>,
        note: String,
    },
    /// The store could not give a section's bytes (`input-changed`,
    /// `cannot-open-file`), or the object's tables are malformed.
    Input { file: FileId, section: SectionIndex, what: What },
}

impl ReadError {
    /// The file the error is about.
    #[must_use]
    pub fn file(&self) -> FileId {
        match self {
            Self::Refused { file, .. } | Self::Input { file, .. } => *file,
        }
    }
}

/// One unit of S-G2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub section: SectionId,
    /// Its offset in its input section.
    pub offset: u64,
    /// Its length, with the 4-byte length field.
    pub length: u64,
    /// S-G3a.
    pub bearing: bool,
    /// Its kept ranges (S-G5): `ranges[first..first + count]` of the file.
    pub first: u32,
    pub count: u32,
}

/// A kept range of S-G5: its start in an input section and its length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitRange {
    pub start: CodeRef,
    pub len: u64,
}

/// One entry of a name set: the name's bytes in its section's bytes and
/// the attribute byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub start: u32,
    pub len: u32,
    pub attributes: u8,
}

/// One set of S-G4a.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Set {
    /// `.debug_gnu_pubtypes` (else `.debug_gnu_pubnames`).
    pub types: bool,
    /// Its section and offset (diagnostics).
    pub section: SectionIndex,
    pub at: u64,
    /// The unit it names: a `.debug_info` section and the offset in it.
    pub unit: (SectionId, u64),
    /// Its section's bytes: `File::bytes[bytes]`.
    pub bytes: u32,
    pub entries: Vec<Entry>,
}

/// What one file holds.
#[derive(Debug, Default)]
pub struct File<'s> {
    pub units: Vec<Unit>,
    pub ranges: Vec<UnitRange>,
    pub sets: Vec<Set>,
    /// The sets' sections' bytes.
    pub bytes: Vec<Cow<'s, [u8]>>,
    /// The file has a `.debug_names` (Clang `-gpubnames`; not read).
    pub debug_names: bool,
    /// The file has a `.debug_pubnames` or `.debug_pubtypes` (not read).
    pub old_pubnames: bool,
}

/// Reads file `file`: its units, ranges and sets. A file that is not a
/// relocatable object has none.
///
/// # Errors
///
/// The first problem found in the file (§17.6).
pub fn read_file<'s>(src: &Source<'_, 's>, file: FileId) -> Result<File<'s>, ReadError> {
    let Some(object) = src.link.object(file) else { return Ok(File::default()) };
    let mut cx = FileCx::new(src, file, object)?;
    if cx.infos.is_empty() && cx.set_secs.is_empty() {
        return Ok(cx.out);
    }
    for i in 0..cx.infos.len() {
        cx.units_of(cx.infos[i])?;
    }
    for i in 0..cx.set_secs.len() {
        cx.sets_of(cx.set_secs[i])?;
    }
    Ok(cx.out)
}

/// A loaded section: its bytes and its relocations, sorted by offset.
struct Sec<'s> {
    index: SectionIndex,
    id: SectionId,
    name: &'static str,
    bytes: Cow<'s, [u8]>,
    relocs: Vec<Raw>,
}

/// A relocation record as read: offset, symbol, type, the RELA addend.
#[derive(Debug, Clone, Copy)]
struct Raw {
    offset: u64,
    sym: u32,
    r_type: u32,
    addend: Option<i64>,
}

/// An address of S-G5: a section (or none) and an offset in it (or the
/// absolute value).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Addr {
    sec: Option<SectionId>,
    off: i128,
}

impl Addr {
    fn plus(self, n: u64) -> Self {
        Self { sec: self.sec, off: self.off + i128::from(n) }
    }
}

/// The names of the sections the reader loads.
const NAMES: [&str; 8] = [
    ".debug_info",
    ".debug_abbrev",
    ".debug_addr",
    ".debug_rnglists",
    ".debug_ranges",
    ".debug_gnu_pubnames",
    ".debug_gnu_pubtypes",
    ".debug_types",
];

struct FileCx<'t, 'u, 's> {
    src: &'t Source<'u, 's>,
    file: FileId,
    object: Object<'s>,
    symbols: Option<SymbolTable<'s>>,
    secs: Vec<Sec<'s>>,
    /// `secs` positions by section id.
    by_id: HashMap<SectionId, usize>,
    /// The `.debug_info` sections in the output, in section order.
    infos: Vec<usize>,
    /// The consumed set sections, in section order.
    set_secs: Vec<usize>,
    out: File<'s>,
}

/// The fields of a root DIE that S-G5 reads.
#[derive(Debug, Default)]
struct Root {
    low: Option<Pc>,
    high: Option<High>,
    ranges: Option<RangesAt>,
    addr_base: Option<(usize, u64)>,
    rnglists_base: Option<(usize, u64)>,
    dwo: bool,
}

#[derive(Debug, Clone, Copy)]
enum Pc {
    Addr(Addr),
    Index(u64),
}

#[derive(Debug, Clone, Copy)]
enum High {
    Pc(Pc),
    Len(u64),
}

#[derive(Debug, Clone, Copy)]
enum RangesAt {
    Offset(usize, u64),
    Index(u64),
}

/// DWARF constants (DWARF 5 §7; GNU's from `include/dwarf2.def`).
mod dw {
    pub const TAG_COMPILE_UNIT: u64 = 0x11;
    pub const TAG_PARTIAL_UNIT: u64 = 0x3c;
    pub const TAG_TYPE_UNIT: u64 = 0x41;
    pub const TAG_SKELETON_UNIT: u64 = 0x4a;
    pub const AT_LOW_PC: u64 = 0x11;
    pub const AT_HIGH_PC: u64 = 0x12;
    pub const AT_RANGES: u64 = 0x55;
    pub const AT_ADDR_BASE: u64 = 0x73;
    pub const AT_RNGLISTS_BASE: u64 = 0x74;
    pub const AT_DWO_NAME: u64 = 0x76;
    pub const AT_GNU_DWO_NAME: u64 = 0x2130;
    pub const AT_GNU_ADDR_BASE: u64 = 0x2133;
    pub const UT_COMPILE: u8 = 1;
    pub const UT_TYPE: u8 = 2;
    pub const UT_SKELETON: u8 = 4;
    pub const UT_SPLIT_TYPE: u8 = 6;
}

impl<'t, 'u, 's> FileCx<'t, 'u, 's> {
    fn new(src: &'t Source<'u, 's>, file: FileId, object: Object<'s>) -> Result<Self, ReadError> {
        let mut cx = FileCx {
            src,
            file,
            object,
            symbols: None,
            secs: Vec::new(),
            by_id: HashMap::new(),
            infos: Vec::new(),
            set_secs: Vec::new(),
            out: File::default(),
        };
        let input = |section: SectionIndex, e: mortar_elf::Error| ReadError::Input {
            file,
            section,
            what: What::Malformed(e.to_string()),
        };
        // The relocation section of each section, and the sections to load.
        let mut rel_of: HashMap<u32, SectionIndex> = HashMap::new();
        let mut wanted: Vec<(SectionIndex, &'static str)> = Vec::new();
        for s in object.sections() {
            let t = s.sh_type();
            if t == SHT_RELA || t == SHT_REL {
                rel_of.insert(s.header().sh_info, s.index());
                continue;
            }
            let name = s.name().map_err(|e| input(s.index(), e))?;
            match name {
                b".debug_names" => cx.out.debug_names = true,
                b".debug_pubnames" | b".debug_pubtypes" => cx.out.old_pubnames = true,
                _ => {}
            }
            if let Some(n) = NAMES.iter().find(|n| n.as_bytes() == name) {
                wanted.push((s.index(), n));
            }
        }
        for (index, name) in wanted {
            let Some(id) = src.link.section_id(file, index) else { continue };
            let sets = matches!(name, ".debug_gnu_pubnames" | ".debug_gnu_pubtypes");
            let load = if sets { (src.consumed)(id) } else { (src.in_output)(id) };
            if !load {
                continue;
            }
            if name == ".debug_types" {
                return Err(cx.refuse(
                    TYPE_UNIT,
                    Some((index, 0)),
                    "a .debug_types section (DWARF 4 type units): the index has no type-unit \
                     list (spec/synth.md S-G2)"
                        .into(),
                ));
            }
            let s = object.section(index).map_err(|e| input(index, e))?;
            let bytes = src
                .link
                .bytes(file, &s)
                .map_err(|what| ReadError::Input { file, section: index, what })?;
            let relocs = match rel_of.get(&index.0) {
                Some(&r) if name != ".debug_abbrev" => cx.raw_relocs(r)?,
                _ => Vec::new(),
            };
            let at = cx.secs.len();
            cx.by_id.insert(id, at);
            if name == ".debug_info" {
                cx.infos.push(at);
            } else if sets {
                cx.set_secs.push(at);
            }
            cx.secs.push(Sec { index, id, name, bytes, relocs });
        }
        if cx.secs.iter().any(|s| !s.relocs.is_empty()) {
            cx.symbols = object.symbols().map_err(|e| input(SectionIndex(0), e))?;
        }
        Ok(cx)
    }

    fn refuse(
        &self,
        name: &'static str,
        at: Option<(SectionIndex, u64)>,
        note: String,
    ) -> ReadError {
        ReadError::Refused { name, file: self.file, at, note }
    }

    /// The relocation records of relocation section `r`, sorted by
    /// offset; two at one offset are refused when read ([`Self::reloc`]).
    fn raw_relocs(&self, r: SectionIndex) -> Result<Vec<Raw>, ReadError> {
        let file = self.file;
        let s = self.object.section(r).map_err(|e| ReadError::Input {
            file,
            section: r,
            what: What::Malformed(e.to_string()),
        })?;
        let records = self
            .src
            .link
            .bytes(file, &s)
            .map_err(|what| ReadError::Input { file, section: r, what })?;
        let class = self.object.class();
        let mut out: Vec<Raw> = match self.src.link.form() {
            RelocForm::Rela => Table::<Rela>::new(&records, class)
                .iter()
                .map(|r| Raw {
                    offset: r.r_offset,
                    sym: r.r_sym,
                    r_type: r.r_type,
                    addend: Some(r.r_addend),
                })
                .collect(),
            RelocForm::Rel => Table::<Rel>::new(&records, class)
                .iter()
                .map(|r| Raw { offset: r.r_offset, sym: r.r_sym, r_type: r.r_type, addend: None })
                .collect(),
        };
        out.sort_by_key(|r| r.offset);
        Ok(out)
    }

    /// The relocation at `at` of `sec`, classified and resolved; `None`
    /// without one (or with `R_*_NONE`).
    fn reloc(&self, sec: usize, at: u64) -> Result<Option<(Target, i64, RelocClass, u8)>, ReadError> {
        let s = &self.secs[sec];
        let i = s.relocs.partition_point(|r| r.offset < at);
        let Some(r) = s.relocs.get(i).filter(|r| r.offset == at) else { return Ok(None) };
        let bad = |note: String| self.refuse(MALFORMED_UNIT, Some((s.index, at)), note);
        if s.relocs.get(i + 1).is_some_and(|n| n.offset == at) {
            return Err(bad(format!("two relocations at one field of {}", s.name)));
        }
        let info = self
            .src
            .relocs
            .classify(r.r_type, &s.bytes, at)
            .map_err(|w| bad(format!("{} in {}", w.message(), s.name)))?;
        if info.class == RelocClass::None {
            return Ok(None);
        }
        let addend = match r.addend {
            Some(a) => a,
            None => self.src.relocs.addend(r.r_type, &s.bytes, at),
        };
        let t = if r.sym == 0 {
            Target::Absolute(0)
        } else {
            let Some(symbols) = &self.symbols else {
                return Err(bad("a relocation without a symbol table".into()));
            };
            target(self.src.link, self.file, symbols, SymbolIndex(r.sym))
                .map_err(|w| bad(format!("{} in {}", w.message(), s.name)))?
        };
        Ok(Some((t, addend, info.class, info.width)))
    }

    /// An address field of `size` bytes at `at` of `sec` (S-G5).
    fn addr(&self, sec: usize, at: u64, size: u8) -> Result<Addr, ReadError> {
        let s = &self.secs[sec];
        let raw = uint(&s.bytes, at, size).ok_or_else(|| self.past_end(sec, at))?;
        match self.reloc(sec, at)? {
            None => Ok(Addr { sec: None, off: i128::from(raw) }),
            Some((t, addend, class, width)) => {
                if class != RelocClass::Abs || width != size {
                    return Err(self.refuse(
                        MALFORMED_UNIT,
                        Some((s.index, at)),
                        format!(
                            "a relocation of another class or width at an address field of {}",
                            s.name
                        ),
                    ));
                }
                match t {
                    Target::Section { section, value } => Ok(Addr {
                        sec: Some(section),
                        off: i128::from(value) + i128::from(addend),
                    }),
                    Target::Absolute(v) => {
                        Ok(Addr { sec: None, off: i128::from(v) + i128::from(addend) })
                    }
                    Target::Undefined { .. } | Target::Elsewhere(_) => Err(self.refuse(
                        MALFORMED_UNIT,
                        Some((s.index, at)),
                        format!(
                            "an address field of {} relocated against a symbol not defined in \
                             a section of the link",
                            s.name
                        ),
                    )),
                }
            }
        }
    }

    /// A 4-byte section offset at `at` of `sec`, through its relocation,
    /// into a section named `want`: that section's position and the
    /// offset.
    fn offset_into(&self, sec: usize, at: u64, want: &str) -> Result<(usize, u64), ReadError> {
        let s = &self.secs[sec];
        let bad = |note: String| self.refuse(MALFORMED_UNIT, Some((s.index, at)), note);
        if uint(&s.bytes, at, 4).is_none() {
            return Err(self.past_end(sec, at));
        }
        let Some((t, addend, class, width)) = self.reloc(sec, at)? else {
            return Err(bad(format!("an offset into {want} without a relocation in {}", s.name)));
        };
        let Target::Section { section, value } = t else {
            return Err(bad(format!("an offset into {want} relocated against no section")));
        };
        if class != RelocClass::Abs || width != 4 {
            return Err(bad(format!("a relocation of another class at an offset into {want}")));
        }
        let Some(&pos) = self.by_id.get(&section).filter(|&&p| self.secs[p].name == want) else {
            return Err(bad(format!("an offset into {want} relocated against another section")));
        };
        let off = i128::from(value) + i128::from(addend);
        let off = u64::try_from(off).map_err(|_| bad(format!("a negative offset into {want}")))?;
        Ok((pos, off))
    }

    fn past_end(&self, sec: usize, at: u64) -> ReadError {
        let s = &self.secs[sec];
        self.refuse(MALFORMED_UNIT, Some((s.index, at)), format!("a read past the end of {}", s.name))
    }

    /// The units of `.debug_info` section `sec` (S-G2), with their
    /// ranges (S-G5).
    fn units_of(&mut self, sec: usize) -> Result<(), ReadError> {
        let len = self.secs[sec].bytes.len() as u64;
        let mut at = 0u64;
        while at < len {
            let end = self.unit(sec, at)?;
            at = end;
        }
        Ok(())
    }

    /// Reads the unit at `start` of `sec`; returns its end.
    #[allow(clippy::too_many_lines, reason = "the header, the root DIE and its ranges in order")]
    fn unit(&mut self, sec: usize, start: u64) -> Result<u64, ReadError> {
        let word = self.src.word;
        let (index, id) = (self.secs[sec].index, self.secs[sec].id);
        let bad = |cx: &Self, note: &str| {
            cx.refuse(
                MALFORMED_UNIT,
                Some((index, start)),
                format!("{note} (the unit at {start:#x} of .debug_info)"),
            )
        };
        let bytes: &[u8] = &self.secs[sec].bytes;
        let mut c = Cur { b: bytes, pos: start };
        let unit_length = c.u32().ok_or_else(|| bad(self, "a truncated unit header"))?;
        if unit_length == 0xffff_ffff {
            return Err(bad(self, "64-bit DWARF"));
        }
        let end = start + 4 + u64::from(unit_length);
        if end > bytes.len() as u64 {
            return Err(bad(self, "a unit longer than its section"));
        }
        let version = c.u16().ok_or_else(|| bad(self, "a truncated unit header"))?;
        if !(2..=5).contains(&version) {
            return Err(bad(self, &format!("DWARF version {version}")));
        }
        let (unit_type, abbrev_at, addr_size) = if version >= 5 {
            let ut = c.u8().ok_or_else(|| bad(self, "a truncated unit header"))?;
            let size = c.u8().ok_or_else(|| bad(self, "a truncated unit header"))?;
            let at = c.pos;
            c.pos += 4;
            (Some(ut), at, size)
        } else {
            let at = c.pos;
            c.pos += 4;
            let size = c.u8().ok_or_else(|| bad(self, "a truncated unit header"))?;
            (None, at, size)
        };
        match unit_type {
            None | Some(dw::UT_COMPILE) => {}
            Some(dw::UT_SKELETON) => c.pos += 8,
            Some(dw::UT_TYPE | dw::UT_SPLIT_TYPE) => {
                return Err(self.refuse(
                    TYPE_UNIT,
                    Some((index, start)),
                    format!(
                        "a type unit at {start:#x} of .debug_info: the index has no type-unit \
                         list (spec/synth.md S-G2)"
                    ),
                ));
            }
            Some(t) => return Err(bad(self, &format!("unit type {t:#x}"))),
        }
        if addr_size != word {
            return Err(bad(self, &format!("address size {addr_size} in an output of {word}")));
        }
        let (abbrev_sec, abbrev_off) = self.offset_into(sec, abbrev_at, ".debug_abbrev")?;
        // The root DIE.
        let bytes: &[u8] = &self.secs[sec].bytes;
        let mut c = Cur { b: &bytes[..end as usize], pos: c.pos };
        let code = c.uleb().ok_or_else(|| bad(self, "a truncated root DIE"))?;
        if code == 0 {
            return Err(bad(self, "a unit without a root DIE"));
        }
        let abbrev = abbrev_of(&self.secs[abbrev_sec].bytes, abbrev_off, code)
            .ok_or_else(|| bad(self, &format!("abbreviation {code} not found or malformed")))?;
        match abbrev.tag {
            dw::TAG_COMPILE_UNIT | dw::TAG_SKELETON_UNIT => {}
            dw::TAG_TYPE_UNIT => {
                return Err(self.refuse(
                    TYPE_UNIT,
                    Some((index, start)),
                    format!("a type unit at {start:#x} of .debug_info (spec/synth.md S-G2)"),
                ));
            }
            dw::TAG_PARTIAL_UNIT => return Err(bad(self, "a partial unit")),
            t => return Err(bad(self, &format!("a root DIE of tag {t:#x}"))),
        }
        let mut root = Root::default();
        for &(name, form, implicit) in &abbrev.attrs {
            let mut form = form;
            if form == 0x16 {
                form = c.uleb().ok_or_else(|| bad(self, "a truncated root DIE"))?;
            }
            let at = c.pos;
            match (name, form) {
                (dw::AT_LOW_PC, _) => root.low = Some(self.pc(sec, &mut c, form, version, &bad)?),
                (dw::AT_HIGH_PC, 0x0b | 0x05 | 0x06 | 0x07 | 0x0f) => {
                    root.high = Some(High::Len(c.data(form).ok_or_else(|| bad(self, "a truncated root DIE"))?));
                }
                (dw::AT_HIGH_PC, 0x21) => {
                    root.high = Some(High::Len(
                        u64::try_from(implicit).map_err(|_| bad(self, "a negative high_pc"))?,
                    ));
                }
                (dw::AT_HIGH_PC, _) => {
                    root.high = Some(High::Pc(self.pc(sec, &mut c, form, version, &bad)?));
                }
                (dw::AT_RANGES, 0x23) => {
                    root.ranges = Some(RangesAt::Index(
                        c.uleb().ok_or_else(|| bad(self, "a truncated root DIE"))?,
                    ));
                }
                (dw::AT_RANGES, 0x17 | 0x06) => {
                    let want = if version >= 5 { ".debug_rnglists" } else { ".debug_ranges" };
                    let (s, o) = self.offset_into(sec, at, want)?;
                    root.ranges = Some(RangesAt::Offset(s, o));
                    c.pos += 4;
                }
                (dw::AT_RANGES, f) => {
                    return Err(bad(self, &format!("DW_AT_ranges of form {f:#x}")));
                }
                (dw::AT_ADDR_BASE | dw::AT_GNU_ADDR_BASE, 0x17 | 0x06) => {
                    root.addr_base = Some(self.offset_into(sec, at, ".debug_addr")?);
                    c.pos += 4;
                }
                (dw::AT_RNGLISTS_BASE, 0x17 | 0x06) => {
                    root.rnglists_base = Some(self.offset_into(sec, at, ".debug_rnglists")?);
                    c.pos += 4;
                }
                (dw::AT_ADDR_BASE | dw::AT_GNU_ADDR_BASE | dw::AT_RNGLISTS_BASE, f) => {
                    return Err(bad(self, &format!("attribute {name:#x} of form {f:#x}")));
                }
                _ => {
                    if name == dw::AT_DWO_NAME || name == dw::AT_GNU_DWO_NAME {
                        root.dwo = true;
                    }
                    if !c.skip(form, addr_size, version) {
                        return Err(bad(self, &format!("attribute {name:#x} of form {form:#x}")));
                    }
                }
            }
            if c.pos > end {
                return Err(bad(self, "a root DIE past the end of its unit"));
            }
        }
        let bearing = unit_type == Some(dw::UT_SKELETON)
            || abbrev.tag == dw::TAG_SKELETON_UNIT
            || root.dwo
            || abbrev.children;
        let first = self.out.ranges.len();
        self.ranges(&root, word, &bad)?;
        let count = self.out.ranges.len() - first;
        let too_large = |cx: &Self| cx.refuse(TOO_LARGE, Some((index, start)), "2^32 ranges".into());
        self.out.units.push(Unit {
            section: id,
            offset: start,
            length: end - start,
            bearing,
            first: u32::try_from(first).map_err(|_| too_large(self))?,
            count: u32::try_from(count).map_err(|_| too_large(self))?,
        });
        Ok(end)
    }

    /// An address attribute of `form` at the cursor (S-G5).
    fn pc(
        &self,
        sec: usize,
        c: &mut Cur<'_>,
        form: u64,
        version: u16,
        bad: &dyn Fn(&Self, &str) -> ReadError,
    ) -> Result<Pc, ReadError> {
        let t = |cx: &Self| bad(cx, "a truncated root DIE");
        Ok(match form {
            0x01 => {
                let at = c.pos;
                c.pos += u64::from(self.src.word);
                Pc::Addr(self.addr(sec, at, self.src.word)?)
            }
            0x1b | 0x1f01 => Pc::Index(c.uleb().ok_or_else(|| t(self))?),
            0x29 => Pc::Index(c.uint(1).ok_or_else(|| t(self))?),
            0x2a => Pc::Index(c.uint(2).ok_or_else(|| t(self))?),
            0x2b => Pc::Index(c.uint(3).ok_or_else(|| t(self))?),
            0x2c => Pc::Index(c.uint(4).ok_or_else(|| t(self))?),
            f => return Err(bad(self, &format!("an address of form {f:#x} (DWARF {version})"))),
        })
    }

    /// Entry `index` of the unit's `.debug_addr` (at its base).
    fn addr_index(
        &self,
        root: &Root,
        index: u64,
        bad: &dyn Fn(&Self, &str) -> ReadError,
    ) -> Result<Addr, ReadError> {
        let Some((sec, base)) = root.addr_base else {
            return Err(bad(self, "an address index without DW_AT_addr_base"));
        };
        let w = u64::from(self.src.word);
        let at = index
            .checked_mul(w)
            .and_then(|o| o.checked_add(base))
            .ok_or_else(|| self.past_end(sec, base))?;
        self.addr(sec, at, self.src.word)
    }

    fn resolve_pc(
        &self,
        root: &Root,
        pc: Pc,
        bad: &dyn Fn(&Self, &str) -> ReadError,
    ) -> Result<Addr, ReadError> {
        match pc {
            Pc::Addr(a) => Ok(a),
            Pc::Index(i) => self.addr_index(root, i, bad),
        }
    }

    /// The unit's ranges (S-G5), pushed onto `out.ranges`.
    fn ranges(
        &mut self,
        root: &Root,
        word: u8,
        bad: &dyn Fn(&Self, &str) -> ReadError,
    ) -> Result<(), ReadError> {
        let base = match root.low {
            Some(pc) => self.resolve_pc(root, pc, bad)?,
            None => Addr { sec: None, off: 0 },
        };
        let mut found: Vec<(Addr, Addr)> = Vec::new();
        match root.ranges {
            Some(RangesAt::Offset(sec, off)) if self.secs[sec].name == ".debug_ranges" => {
                self.debug_ranges(sec, off, base, word, &mut found)?;
            }
            Some(RangesAt::Offset(sec, off)) => {
                self.rnglist(root, sec, off, base, word, bad, &mut found)?;
            }
            Some(RangesAt::Index(i)) => {
                let Some((sec, rbase)) = root.rnglists_base else {
                    return Err(bad(self, "DW_FORM_rnglistx without DW_AT_rnglists_base"));
                };
                let at = i.checked_mul(4).and_then(|o| o.checked_add(rbase));
                let entry = at
                    .and_then(|at| uint(&self.secs[sec].bytes, at, 4))
                    .ok_or_else(|| self.past_end(sec, rbase))?;
                self.rnglist(root, sec, rbase + entry, base, word, bad, &mut found)?;
            }
            None => {
                if let (Some(_), Some(high)) = (root.low, root.high) {
                    let end = match high {
                        High::Len(n) => base.plus(n),
                        High::Pc(pc) => self.resolve_pc(root, pc, bad)?,
                    };
                    found.push((base, end));
                }
            }
        }
        for (start, end) in found {
            if start.sec != end.sec {
                return Err(bad(self, "a range whose ends are in two sections"));
            }
            let len = end.off - start.off;
            if len < 0 {
                return Err(bad(self, "a range that ends before it starts"));
            }
            if len == 0 {
                continue;
            }
            let Some(section) = start.sec else {
                return Err(bad(self, "a range at an absolute address"));
            };
            if !(self.src.in_output)(section) {
                continue;
            }
            let (Ok(offset), Ok(len)) = (i64::try_from(start.off), u64::try_from(len)) else {
                return Err(bad(self, "a range outside its section"));
            };
            if offset < 0 {
                return Err(bad(self, "a range before the start of its section"));
            }
            self.out.ranges.push(UnitRange { start: CodeRef { section, offset }, len });
        }
        Ok(())
    }

    /// A DWARF 2-4 range list at `off` of `.debug_ranges` section `sec`.
    fn debug_ranges(
        &self,
        sec: usize,
        mut off: u64,
        mut base: Addr,
        word: u8,
        found: &mut Vec<(Addr, Addr)>,
    ) -> Result<(), ReadError> {
        let w = u64::from(word);
        let ones = if word == 8 { u64::MAX } else { u64::from(u32::MAX) };
        loop {
            let s = &self.secs[sec];
            let begin_raw = uint(&s.bytes, off, word).ok_or_else(|| self.past_end(sec, off))?;
            let end_raw = uint(&s.bytes, off + w, word).ok_or_else(|| self.past_end(sec, off + w))?;
            let begin = self.addr(sec, off, word)?;
            let end = self.addr(sec, off + w, word)?;
            let plain = begin.sec.is_none() && end.sec.is_none();
            off += 2 * w;
            if plain && begin_raw == 0 && end_raw == 0 && begin.off == 0 && end.off == 0 {
                return Ok(());
            }
            if begin.sec.is_none() && begin.off == i128::from(ones) {
                base = end;
                continue;
            }
            let (Some(b), Some(e)) = (add(base, begin), add(base, end)) else {
                return Err(self.refuse(
                    MALFORMED_UNIT,
                    Some((self.secs[sec].index, off - 2 * w)),
                    "a .debug_ranges pair relocated against a relocated base".into(),
                ));
            };
            found.push((b, e));
        }
    }

    /// A DWARF 5 range list at `off` of `.debug_rnglists` section `sec`.
    #[allow(clippy::too_many_arguments, reason = "the list's context")]
    fn rnglist(
        &self,
        root: &Root,
        sec: usize,
        mut off: u64,
        mut base: Addr,
        word: u8,
        bad: &dyn Fn(&Self, &str) -> ReadError,
        found: &mut Vec<(Addr, Addr)>,
    ) -> Result<(), ReadError> {
        let w = u64::from(word);
        loop {
            let bytes: &[u8] = &self.secs[sec].bytes;
            let mut c = Cur { b: bytes, pos: off };
            let short = |cx: &Self, at| cx.past_end(sec, at);
            let code = c.u8().ok_or_else(|| short(self, off))?;
            match code {
                0 => return Ok(()),
                1 => {
                    let i = c.uleb().ok_or_else(|| short(self, off))?;
                    base = self.addr_index(root, i, bad)?;
                }
                2 => {
                    let first = c.uleb().ok_or_else(|| short(self, off))?;
                    let second = c.uleb().ok_or_else(|| short(self, off))?;
                    found.push((self.addr_index(root, first, bad)?, self.addr_index(root, second, bad)?));
                }
                3 => {
                    let first = c.uleb().ok_or_else(|| short(self, off))?;
                    let len = c.uleb().ok_or_else(|| short(self, off))?;
                    let start = self.addr_index(root, first, bad)?;
                    found.push((start, start.plus(len)));
                }
                4 => {
                    let first = c.uleb().ok_or_else(|| short(self, off))?;
                    let second = c.uleb().ok_or_else(|| short(self, off))?;
                    found.push((base.plus(first), base.plus(second)));
                }
                5 => {
                    let at = c.pos;
                    c.pos += w;
                    base = self.addr(sec, at, word)?;
                }
                6 => {
                    let at = c.pos;
                    c.pos += 2 * w;
                    found.push((self.addr(sec, at, word)?, self.addr(sec, at + w, word)?));
                }
                7 => {
                    let at = c.pos;
                    c.pos += w;
                    let start = self.addr(sec, at, word)?;
                    let len = c.uleb().ok_or_else(|| short(self, off))?;
                    found.push((start, start.plus(len)));
                }
                k => {
                    return Err(bad(self, &format!("range list entry kind {k:#x}")));
                }
            }
            off = c.pos;
        }
    }

    /// The sets of consumed section `sec` (S-G4a).
    fn sets_of(&mut self, sec: usize) -> Result<(), ReadError> {
        let s = &self.secs[sec];
        let (index, types) = (s.index, s.name == ".debug_gnu_pubtypes");
        let bytes: &[u8] = &s.bytes;
        let len = bytes.len() as u64;
        let mut at = 0u64;
        let mut sets = Vec::new();
        while at < len {
            let bad = |note: &str| {
                self.refuse(MALFORMED_SET, Some((index, at)), format!("{note} ({} set at {at:#x})", s.name))
            };
            let mut c = Cur { b: bytes, pos: at };
            let unit_length = c.u32().ok_or_else(|| bad("a truncated header"))?;
            if unit_length == 0xffff_ffff {
                return Err(bad("64-bit DWARF"));
            }
            let end = at + 4 + u64::from(unit_length);
            if end > len {
                return Err(bad("a set longer than its section"));
            }
            let mut c = Cur { b: &bytes[..end as usize], pos: c.pos };
            let version = c.u16().ok_or_else(|| bad("a truncated header"))?;
            if version != 2 {
                return Err(bad(&format!("version {version}")));
            }
            let info_at = c.pos;
            c.pos += 4;
            if c.u32().is_none() {
                return Err(bad("a truncated header"));
            }
            let unit = self.set_unit(sec, info_at, &bad)?;
            let mut entries = Vec::new();
            loop {
                let die = c.u32().ok_or_else(|| bad("no terminator"))?;
                if die == 0 {
                    break;
                }
                let attributes = c.u8().ok_or_else(|| bad("a truncated entry"))?;
                if attributes & 0x0f != 0 || (attributes >> 4) & 7 > 4 {
                    return Err(bad(&format!("attribute byte {attributes:#04x}")));
                }
                let start = c.pos;
                let name = c.cstr().ok_or_else(|| bad("a name that runs past the set"))?;
                let (Ok(start), Ok(n)) = (u32::try_from(start), u32::try_from(name.len())) else {
                    return Err(self.refuse(TOO_LARGE, Some((index, at)), "a set of 4 GiB".into()));
                };
                entries.push(Entry { start, len: n, attributes });
            }
            if c.pos != end {
                return Err(bad("bytes after the terminator"));
            }
            sets.push(Set { types, section: index, at, unit, bytes: 0, entries });
            at = end;
        }
        let b = u32::try_from(self.out.bytes.len()).unwrap_or(u32::MAX);
        for set in &mut sets {
            set.bytes = b;
        }
        let s = &mut self.secs[sec];
        self.out.bytes.push(std::mem::take(&mut s.bytes));
        self.out.sets.extend(sets);
        Ok(())
    }

    /// The unit a set's `debug_info_offset` at `at` of `sec` names.
    fn set_unit(
        &self,
        sec: usize,
        at: u64,
        bad: &dyn Fn(&str) -> ReadError,
    ) -> Result<(SectionId, u64), ReadError> {
        let Some((t, addend, class, width)) = self.reloc(sec, at)? else {
            return Err(bad("debug_info_offset without a relocation"));
        };
        let Target::Section { section, value } = t else {
            return Err(bad("debug_info_offset relocated against no section"));
        };
        if class != RelocClass::Abs || width != 4 {
            return Err(bad("debug_info_offset with a relocation of another class"));
        }
        let off = u64::try_from(i128::from(value) + i128::from(addend))
            .map_err(|_| bad("a negative debug_info_offset"))?;
        if let Some(&pos) = self.by_id.get(&section) {
            if self.secs[pos].name == ".debug_info" {
                return Ok((section, off));
            }
            return Err(bad("debug_info_offset relocated against another section"));
        }
        // Not loaded: a `.debug_info` of this file that is not in the
        // output (S-G5a), or another section.
        let base = self.src.link.section_id(self.file, SectionIndex(0));
        let index = base.map(|b| SectionIndex(section.0.wrapping_sub(b.0)));
        let orphan = index.is_some_and(|i| {
            self.src.link.section_id(self.file, i) == Some(section)
                && self.object.section(i).ok().and_then(|s| s.name().ok()) == Some(b".debug_info")
        });
        if orphan {
            return Err(self.refuse(
                ORPHAN_SET,
                Some((self.secs[sec].index, at)),
                format!(
                    "a name set naming a unit of a .debug_info section that is not in the output \
                     (offset {off:#x})"
                ),
            ));
        }
        Err(bad("debug_info_offset relocated against another section"))
    }
}

/// `base + a` (DWARF 2-4 pairs are offsets from the unit's base): the
/// section of whichever term has one; `None` when both have one.
fn add(base: Addr, a: Addr) -> Option<Addr> {
    match (base.sec, a.sec) {
        (Some(_), Some(_)) => None,
        (sec, None) | (None, sec) => Some(Addr { sec, off: base.off + a.off }),
    }
}

/// The little-endian unsigned integer of `size` bytes at `at`.
fn uint(b: &[u8], at: u64, size: u8) -> Option<u64> {
    let at = usize::try_from(at).ok()?;
    let f = b.get(at..at.checked_add(usize::from(size))?)?;
    Some(f.iter().rev().fold(0u64, |v, &x| v << 8 | u64::from(x)))
}

/// A read cursor; every read is bounded.
struct Cur<'b> {
    b: &'b [u8],
    pos: u64,
}

impl<'b> Cur<'b> {
    fn uint(&mut self, n: u8) -> Option<u64> {
        let v = uint(self.b, self.pos, n)?;
        self.pos += u64::from(n);
        Some(v)
    }
    fn u8(&mut self) -> Option<u8> {
        self.uint(1).map(|v| v as u8)
    }
    fn u16(&mut self) -> Option<u16> {
        self.uint(2).map(|v| v as u16)
    }
    fn u32(&mut self) -> Option<u32> {
        self.uint(4).map(|v| v as u32)
    }
    fn uleb(&mut self) -> Option<u64> {
        let mut v = 0u64;
        let mut shift = 0u32;
        loop {
            let byte = self.u8()?;
            if shift < 64 {
                v |= u64::from(byte & 0x7f) << shift;
            } else if byte & 0x7f != 0 {
                return None;
            }
            shift += 7;
            if byte & 0x80 == 0 {
                return Some(v);
            }
        }
    }
    fn sleb(&mut self) -> Option<i64> {
        let mut v = 0i64;
        let mut shift = 0u32;
        loop {
            let byte = self.u8()?;
            if shift < 64 {
                v |= i64::from(byte & 0x7f) << shift;
            }
            shift += 7;
            if byte & 0x80 == 0 {
                if shift < 64 && byte & 0x40 != 0 {
                    v |= -1i64 << shift;
                }
                return Some(v);
            }
        }
    }
    /// A NUL-terminated string, without the NUL.
    fn cstr(&mut self) -> Option<&'b [u8]> {
        let at = usize::try_from(self.pos).ok()?;
        let rest = self.b.get(at..)?;
        let n = rest.iter().position(|&c| c == 0)?;
        self.pos += n as u64 + 1;
        Some(&rest[..n])
    }
    fn advance(&mut self, n: u64) -> bool {
        match self.pos.checked_add(n) {
            Some(p) if p <= self.b.len() as u64 => {
                self.pos = p;
                true
            }
            _ => false,
        }
    }
    /// A constant of a `data` or `udata` form.
    fn data(&mut self, form: u64) -> Option<u64> {
        match form {
            0x0b => self.uint(1),
            0x05 => self.uint(2),
            0x06 => self.uint(4),
            0x07 => self.uint(8),
            0x0f => self.uleb(),
            _ => None,
        }
    }
    /// Skips a value of `form` (32-bit DWARF); `false` for a form it does
    /// not know or a value past the end.
    fn skip(&mut self, form: u64, addr_size: u8, version: u16) -> bool {
        let fixed = |c: &mut Self, n: u64| c.advance(n);
        match form {
            0x01 => fixed(self, u64::from(addr_size)),
            0x03 => self.uint(2).is_some_and(|n| fixed(self, n)),
            0x04 => self.uint(4).is_some_and(|n| fixed(self, n)),
            0x09 | 0x18 => self.uleb().is_some_and(|n| fixed(self, n)),
            0x0a => self.uint(1).is_some_and(|n| fixed(self, n)),
            0x0b | 0x0c | 0x11 | 0x25 | 0x29 => fixed(self, 1),
            0x05 | 0x12 | 0x26 | 0x2a => fixed(self, 2),
            0x27 | 0x2b => fixed(self, 3),
            0x06 | 0x0e | 0x13 | 0x17 | 0x1c | 0x1d | 0x1f | 0x28 | 0x2c | 0x1f20 | 0x1f21 => {
                fixed(self, 4)
            }
            0x07 | 0x14 | 0x20 | 0x24 => fixed(self, 8),
            0x1e => fixed(self, 16),
            0x10 => fixed(self, if version <= 2 { u64::from(addr_size) } else { 4 }),
            0x08 => self.cstr().is_some(),
            0x0d => self.sleb().is_some(),
            0x0f | 0x15 | 0x1a | 0x1b | 0x22 | 0x23 | 0x1f01 | 0x1f02 => self.uleb().is_some(),
            0x19 | 0x21 => true,
            _ => false,
        }
    }
}

/// An abbreviation: its tag, children flag and attribute specifications
/// `(name, form, implicit_const)`.
#[derive(Debug)]
struct Abbrev {
    tag: u64,
    children: bool,
    attrs: Vec<(u64, u64, i64)>,
}

/// Abbreviation `code` of the table at `off` of `bytes`.
fn abbrev_of(bytes: &[u8], off: u64, code: u64) -> Option<Abbrev> {
    let mut c = Cur { b: bytes, pos: off };
    loop {
        let this = c.uleb()?;
        if this == 0 {
            return None;
        }
        let tag = c.uleb()?;
        let children = c.u8()? != 0;
        let mut attrs = Vec::new();
        loop {
            let name = c.uleb()?;
            let form = c.uleb()?;
            if name == 0 && form == 0 {
                break;
            }
            let implicit = if form == 0x21 { c.sleb()? } else { 0 };
            attrs.push((name, form, implicit));
        }
        if this == code {
            return Some(Abbrev { tag, children, attrs });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leb128_and_cstr() {
        let b = [0xe5, 0x8e, 0x26, 0x7f, b'a', b'b', 0, 0x80, 0x7f];
        let mut c = Cur { b: &b, pos: 0 };
        assert_eq!(c.uleb(), Some(624_485));
        assert_eq!(c.sleb(), Some(-1));
        assert_eq!(c.cstr(), Some(&b"ab"[..]));
        assert_eq!(c.sleb(), Some(-128));
        assert_eq!(c.u8(), None);
    }

    #[test]
    fn skip_knows_every_dwarf5_form_and_gnus() {
        let b = [0u8; 64];
        for form in (0x01..=0x2c).chain([0x1f01, 0x1f02, 0x1f20, 0x1f21]) {
            let mut c = Cur { b: &b, pos: 0 };
            let known = c.skip(form, 8, 5);
            assert_eq!(known, form != 0x02 && form != 0x16, "form {form:#x}");
        }
    }

    #[test]
    fn abbreviation_lookup() {
        // code 1: compile_unit, children, (low_pc, addr), (x, implicit_const -3);
        // code 2: skeleton, no children, no attributes.
        let b = [1, 0x11, 1, 0x11, 0x01, 0x05, 0x21, 0x7d, 0, 0, 2, 0x4a, 0, 0, 0, 0];
        let a = abbrev_of(&b, 0, 1).unwrap();
        assert_eq!((a.tag, a.children), (0x11, true));
        assert_eq!(a.attrs, vec![(0x11, 0x01, 0), (0x05, 0x21, -3)]);
        let a = abbrev_of(&b, 0, 2).unwrap();
        assert_eq!((a.tag, a.children, a.attrs.len()), (0x4a, false, 0));
        assert!(abbrev_of(&b, 0, 3).is_none());
    }

    #[test]
    fn dwarf4_pairs_are_offsets_from_the_base() {
        let s = Some(SectionId(7));
        let base = Addr { sec: s, off: 0x10 };
        assert_eq!(add(base, Addr { sec: None, off: 4 }), Some(Addr { sec: s, off: 0x14 }));
        let reloc = Addr { sec: Some(SectionId(9)), off: 8 };
        assert_eq!(add(Addr { sec: None, off: 0 }, reloc), Some(reloc));
        assert_eq!(add(base, reloc), None);
    }
}
