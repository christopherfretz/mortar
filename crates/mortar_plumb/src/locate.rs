//! Resolving a map against the input files (`spec/plumb.md` §3.3): which
//! input section each record means, and where each section is.

use crate::hasher::HashMap;

use mortar_elf::archive::{Archive, Content};
use mortar_elf::consts::{
    SHF_ALLOC, SHF_EXECINSTR, SHF_MERGE, SHF_WRITE, SHT_GROUP, SHT_NOBITS, SHT_NULL, SHT_REL,
    SHT_RELA, SHT_STRTAB, SHT_SYMTAB, SHT_SYMTAB_SHNDX, SHT_X86_64_UNWIND,
};
use mortar_elf::{Error, Machine, Object, Rel, Rela, SectionIndex, Shdr, SymbolTable};

use crate::image::Image;
use crate::map::{Format, Map, MapFile, Placed, SectionRef};
use crate::report::{Finding, Rule, Site};
use crate::store::{At, Bytes, Resolved, Store};

/// A piece of an input section placed on its own (§3.1).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Piece {
    pub input_offset: u64,
    pub addr: u64,
    pub size: u64,
}

/// Where an input section is (§3.3).
#[derive(Debug, Clone)]
pub(crate) enum Place {
    /// The map does not name it.
    Absent,
    /// Placed whole at an address.
    Whole(u64),
    /// Placed as pieces (lld's `.eh_frame`; mortar's pieces).
    Pieces(Vec<Piece>),
    /// A mergeable section whose pieces a foreign map does not place.
    Merged,
    /// `.eh_frame` placed with a size other than its input's: the linker
    /// rewrote its records.
    Rewritten,
    /// Discarded, by the map's word.
    Discarded,
    /// The map's records cannot say which of several sections is where.
    Ambiguous,
}

#[derive(Debug)]
pub(crate) struct Sec<'a> {
    pub name: String,
    pub shdr: Shdr,
    /// The contents ([`Obj::data`]); empty for `SHT_NOBITS` and for a
    /// section out of bounds.
    body: Body<'a>,
    pub place: Place,
    /// The relocation section that applies to this one.
    pub relocs: Option<u32>,
}

/// Where a section's contents are (§8.1): resident (the skeleton, or a
/// file held whole), or `size` bytes at `offset` of the object, read when
/// a rule asks for them.
#[derive(Debug, Clone, Copy)]
enum Body<'a> {
    Resident(&'a [u8]),
    Read { offset: u64, size: u64 },
}

impl Sec<'_> {
    pub fn alloc(&self) -> bool {
        self.shdr.sh_flags & SHF_ALLOC != 0
    }
    pub fn tbss(&self) -> bool {
        self.shdr.sh_flags & mortar_elf::consts::SHF_TLS != 0 && self.shdr.sh_type == SHT_NOBITS
    }
    pub fn merge(&self) -> bool {
        self.shdr.sh_flags & SHF_MERGE != 0
    }
    pub fn is_eh_frame(&self, machine: Machine) -> bool {
        self.name == ".eh_frame"
            || (machine == Machine::X86_64 && self.shdr.sh_type == SHT_X86_64_UNWIND)
    }
    /// Sections a linker consumes rather than places.
    pub fn consumed(&self) -> bool {
        matches!(
            self.shdr.sh_type,
            SHT_NULL | SHT_REL | SHT_RELA | SHT_SYMTAB | SHT_STRTAB | SHT_GROUP | SHT_SYMTAB_SHNDX
        )
    }
}

/// An input object the map names, read: its skeleton resident, its
/// sections' contents read by position when asked for (§8.1).
#[derive(Debug)]
pub(crate) struct Obj<'a> {
    pub path: String,
    pub member: Option<String>,
    pub elf: Object<'a>,
    pub symtab: Option<SymbolTable<'a>>,
    pub secs: Vec<Sec<'a>>,
    store: &'a Store<'a>,
    at: At,
    /// Whether a section's contents are not resident.
    lazy: bool,
}

impl<'a> Obj<'a> {
    /// The contents of section `si`: empty for `SHT_NOBITS`, for a
    /// section out of bounds, for an index past the table, and for one
    /// whose read failed (an I2 finding, [`Store::take_errors`]).
    pub fn data(&self, si: usize) -> Bytes<'a> {
        match self.secs.get(si).map(|s| s.body) {
            Some(Body::Resident(b)) => Bytes::Ref(b),
            Some(Body::Read { offset, size }) => self.store.body(self.at, offset, size),
            None => Bytes::Ref(&[]),
        }
    }

    /// The contents of section `si` when they are resident for the
    /// object's life (the skeleton's sections, an object read whole).
    pub fn resident(&self, si: usize) -> Option<&'a [u8]> {
        match self.secs.get(si).map(|s| s.body) {
            Some(Body::Resident(b)) => Some(b),
            _ => None,
        }
    }

    /// The length of [`Obj::data`] of section `si`, without reading it
    /// (the read can still fail and give no bytes).
    pub fn data_len(&self, si: usize) -> u64 {
        match self.secs.get(si).map(|s| s.body) {
            Some(Body::Resident(b)) => b.len() as u64,
            Some(Body::Read { size, .. }) => size,
            None => 0,
        }
    }

    /// Reads the whole object for a phase that reads most of its sections:
    /// one read, kept while the next object is judged (§8.1).
    pub fn load(&self) {
        if self.lazy {
            self.store.load(self.at);
        }
    }

    pub fn site(&self, sec: usize, offset: u64) -> Site {
        Site {
            file: self.path.clone(),
            member: self.member.clone(),
            section: self.secs.get(sec).map_or_else(String::new, |s| s.name.clone()),
            offset,
        }
    }

    pub fn label(&self) -> String {
        match &self.member {
            Some(m) => format!("{}({m})", self.path),
            None => self.path.clone(),
        }
    }

    /// The relocation records of a relocation section, with addends for
    /// RELA (`None` addend for REL), or `None` if it cannot be read.
    pub fn records(&self, reloc_sec: u32) -> Option<Vec<RawRecord>> {
        let s = self.elf.section(SectionIndex(reloc_sec)).ok()?;
        // Contents not resident are read and checked as resident ones
        // (`Section::rela_in`: the same checks in the same order).
        let body: Bytes<'_>;
        if s.sh_type() == SHT_RELA {
            let r = match s.rela() {
                Err(Error::NotResident { .. }) => {
                    body = self.data(reloc_sec as usize);
                    s.rela_in(&body).ok()?
                }
                r => r.ok()?,
            };
            Some(
                r.iter().map(|x: Rela| (x.r_offset, x.r_sym, x.r_type, Some(x.r_addend))).collect(),
            )
        } else {
            let r = match s.rel() {
                Err(Error::NotResident { .. }) => {
                    body = self.data(reloc_sec as usize);
                    s.rel_in(&body).ok()?
                }
                r => r.ok()?,
            };
            Some(r.iter().map(|x: Rel| (x.r_offset, x.r_sym, x.r_type, None)).collect())
        }
    }
}

/// The relocated byte ranges of a section, sorted (§6.3).
pub(crate) fn covered(obj: &Obj<'_>, sec: usize) -> Vec<(u64, u64)> {
    let Some(rs) = obj.secs[sec].relocs else { return Vec::new() };
    let Some(recs) = obj.records(rs) else { return Vec::new() };
    let machine = obj.elf.machine();
    let mut v: Vec<(u64, u64)> = recs
        .iter()
        .map(|&(off, _, ty, _)| {
            (off, off.saturating_add(u64::from(crate::reloc::width(machine, ty))))
        })
        .collect();
    v.sort_unstable();
    v
}

/// Whether `out` equals the section's bytes outside its relocated ranges.
fn equal_outside(data: &[u8], covered: &[(u64, u64)], out: &[u8]) -> bool {
    if data.len() != out.len() {
        return false;
    }
    let mut at = 0usize;
    for &(s, e) in covered {
        let s = (s as usize).min(data.len());
        if s > at && data[at..s] != out[at..s] {
            return false;
        }
        at = at.max((e as usize).min(data.len()));
    }
    data[at..] == out[at..]
}

/// The result of resolving a map.
#[derive(Debug, Default)]
pub(crate) struct Located<'a> {
    pub objs: Vec<Obj<'a>>,
    pub findings: Vec<Finding>,
}

/// The path of a thin archive's member: the stored path, from the
/// archive's directory when it is relative (GNU ar manual, "--thin").
pub(crate) fn thin_member_path(archive: &str, stored: &[u8]) -> String {
    let stored = String::from_utf8_lossy(stored);
    let dir = std::path::Path::new(archive).parent().unwrap_or(std::path::Path::new(""));
    dir.join(stored.as_ref()).to_string_lossy().into_owned()
}

/// The files a thin archive's members are, as [`Input::path`]s
/// (§2.2): what a caller reads besides the paths the map names. Empty for
/// anything that is not a thin archive.
pub fn thin_members(archive_path: &str, bytes: &[u8]) -> Vec<String> {
    let Ok(archive) = Archive::parse(bytes) else { return Vec::new() };
    archive
        .members()
        .filter_map(Result::ok)
        .filter_map(|m| match m.content() {
            Content::External { path } => Some(thin_member_path(archive_path, path)),
            _ => None,
        })
        .collect()
}

/// Object `k` of the store, as the map's file `file` names it.
fn read_obj<'a>(store: &'a Store<'a>, k: usize, file: &MapFile) -> Result<Obj<'a>, String> {
    let (at, view) = store.object(k);
    let elf = Object::parse_view(view).map_err(|e| e.to_string())?;
    let symtab = elf.symbols().map_err(|e| e.to_string())?;
    let mut secs: Vec<Sec<'a>> = Vec::with_capacity(elf.section_count() as usize);
    let mut lazy = false;
    for s in elf.sections() {
        let name = String::from_utf8_lossy(s.name().unwrap_or(b"")).into_owned();
        let body = if s.sh_type() == SHT_NOBITS {
            Body::Resident(&[])
        } else {
            match s.data() {
                Ok(d) => Body::Resident(d),
                Err(Error::NotResident { .. }) => {
                    lazy = true;
                    Body::Read { offset: s.header().sh_offset, size: s.header().sh_size }
                }
                Err(_) => Body::Resident(&[]),
            }
        };
        secs.push(Sec { name, shdr: *s.header(), body, place: Place::Absent, relocs: None });
    }
    for i in 0..secs.len() {
        if matches!(secs[i].shdr.sh_type, SHT_RELA | SHT_REL) {
            let target = secs[i].shdr.sh_info as usize;
            if target < secs.len() && target != i {
                secs[target].relocs = Some(i as u32);
            }
        }
    }
    Ok(Obj {
        path: file.path.clone(),
        member: file.member.clone(),
        elf,
        symtab,
        secs,
        store,
        at,
        lazy,
    })
}

/// Reads the inputs a map names and decides the place of every section
/// (§3.3).
#[allow(clippy::too_many_lines)] // the map's records, kind by kind
///
/// `store` holds the inputs (§8.1), `m` the map's index among the maps it
/// was read for.
pub(crate) fn locate<'a>(
    map: &Map,
    store: &'a Store<'a>,
    m: usize,
    image: &Image<'_>,
) -> Located<'a> {
    let mut out = Located::default();
    // Map file index -> object index.
    let mut obj_of: Vec<Option<usize>> = vec![None; map.files.len()];
    let out_machine = image.header.e_machine;
    let mut missing_said: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for (i, f) in map.files.iter().enumerate() {
        let read = match store.resolved(m, i) {
            Resolved::Missing => {
                if missing_said.insert(&f.path) {
                    out.findings.push(Finding::new(
                        Rule::InputMissing,
                        format!("the map names '{}', which is not among the inputs", f.path),
                    ));
                }
                continue;
            }
            Resolved::Error(e) => Err(e.clone()),
            Resolved::Object(k) => read_obj(store, *k, f),
        };
        match read {
            Ok(obj) if obj.elf.header().e_machine == out_machine => {
                obj_of[i] = Some(out.objs.len());
                out.objs.push(obj);
            }
            Ok(_) => out.findings.push(Finding::new(
                Rule::InputUnreadable,
                format!("{f}: not an object of the output's machine"),
            )),
            Err(e) => out.findings.push(Finding::new(Rule::InputUnreadable, format!("{f}: {e}"))),
        }
    }
    let names: Vec<HashMap<String, Vec<usize>>> = out
        .objs
        .iter()
        .map(|o| {
            let mut m: HashMap<String, Vec<usize>> = HashMap::default();
            for (i, s) in o.secs.iter().enumerate() {
                if !s.consumed() {
                    m.entry(s.name.clone()).or_default().push(i);
                }
            }
            m
        })
        .collect();
    let gnu = map.format == Format::Gnu;
    // mold's map gives every non-alloc row the address 0: the offsets are
    // the rows' sizes laid out in order at their alignments (§3.6).
    let mut cursor: Option<HashMap<String, u64>> =
        (map.format == Format::Mold).then(HashMap::default);
    for p in &map.placed {
        let Some(oi) = obj_of.get(p.file as usize).copied().flatten() else { continue };
        place_one(&mut out, &names[oi], oi, p, image, gnu, cursor.as_mut());
    }
    // mortar's non-alloc records (§3.2): a place in a named output
    // section, or merged into it.
    // R-9: M2, M3 and M5 apply within one non-alloc output section.
    let mut extents: Vec<(usize, u64, u64, u32)> = Vec::new();
    for r in &map.nonalloc {
        let Some(oi) = obj_of.get(r.file as usize).copied().flatten() else { continue };
        let by_name = candidates(&out.objs[oi], &names[oi], &r.section);
        let free: Vec<usize> = by_name
            .iter()
            .copied()
            .filter(|&i| {
                let s = &out.objs[oi].secs[i];
                !s.alloc() && matches!(s.place, Place::Absent)
            })
            .collect();
        let target = image.nonalloc_section(&r.out);
        if free.is_empty() && by_name.len() == 1 && !out.objs[oi].secs[by_name[0]].alloc() {
            out.findings.push(Finding::new(
                Rule::Map(2),
                format!("line {}: the non-alloc section is placed twice", r.line),
            ));
            continue;
        }
        let ([si], Some(t)) = (free.as_slice(), target) else {
            out.findings.push(Finding::new(
                Rule::Map(1),
                format!(
                    "line {}: the non-alloc record names no single free section, or output section '{}' is not a non-alloc section of the output",
                    r.line, r.out
                ),
            ));
            continue;
        };
        let s = &mut out.objs[oi].secs[*si];
        s.place = match r.offset {
            None => Place::Merged,
            Some(off) => {
                if s.shdr.sh_size != r.size {
                    out.findings.push(Finding::new(
                        Rule::Map(3),
                        format!(
                            "line {}: the size {:#x} is not the input's {:#x}",
                            r.line, r.size, s.shdr.sh_size
                        ),
                    ));
                }
                extents.push((t, off, r.size, r.line));
                Place::Whole(image.sections[t].shdr.sh_addr.wrapping_add(off))
            }
        };
    }
    extents.sort_unstable();
    for w in extents.windows(2) {
        if w[0].0 == w[1].0 && w[0].1.saturating_add(w[0].2) > w[1].1 && w[1].2 > 0 && w[0].2 > 0 {
            out.findings.push(Finding::new(
                Rule::Map(5),
                format!("lines {} and {}: two non-alloc sections overlap", w[0].3, w[1].3),
            ));
        }
    }
    for d in &map.discarded {
        let Some(oi) = obj_of.get(d.file as usize).copied().flatten() else { continue };
        for si in candidates(&out.objs[oi], &names[oi], &d.section) {
            if matches!(out.objs[oi].secs[si].place, Place::Absent) {
                out.objs[oi].secs[si].place = Place::Discarded;
            }
        }
    }
    for f in &map.folds {
        let (Some(oi), Some(ti)) = (
            obj_of.get(f.file as usize).copied().flatten(),
            obj_of.get(f.into_file as usize).copied().flatten(),
        ) else {
            continue;
        };
        let from = candidates(&out.objs[oi], &names[oi], &f.section);
        let into = candidates(&out.objs[ti], &names[ti], &f.into_section);
        if let ([a], [b]) = (from.as_slice(), into.as_slice()) {
            let place = out.objs[ti].secs[*b].place.clone();
            out.objs[oi].secs[*a].place = place;
        } else {
            out.findings.push(Finding::new(
                Rule::Map(1),
                format!("line {}: the fold names no single section", f.line),
            ));
        }
    }
    // A foreign map's record of a mergeable section is not a place: the
    // linker merged its pieces (GNU lists overlapping entries, lld an
    // `<internal>` row, mold nothing). Its pieces are found by content
    // (§3.3, §5.5). mortar's own map places pieces exactly.
    if map.format != Format::Mortar {
        for o in &mut out.objs {
            for s in &mut o.secs {
                if s.merge()
                    && matches!(s.place, Place::Whole(_) | Place::Absent | Place::Ambiguous)
                {
                    s.place = Place::Merged;
                }
            }
        }
    }
    check_map(&mut out, image);
    out
}

fn candidates(
    obj: &Obj<'_>,
    names: &HashMap<String, Vec<usize>>,
    section: &SectionRef,
) -> Vec<usize> {
    match section {
        SectionRef::Index(i) => {
            let i = *i as usize;
            if i < obj.secs.len() && !obj.secs[i].consumed() { vec![i] } else { Vec::new() }
        }
        SectionRef::Name(n) => names.get(n).cloned().unwrap_or_default(),
    }
}

/// Whether a section placed whole may have a size other than its input's.
fn size_may_change(s: &Sec<'_>, machine: Machine) -> bool {
    // A linker writes the merge of the inputs' property notes (§6.6) where
    // the first one goes: GNU ld's map gives it the merged size.
    s.merge() || s.is_eh_frame(machine) || s.name == ".note.gnu.property"
}

#[allow(clippy::too_many_lines)]
fn place_one(
    out: &mut Located<'_>,
    names: &HashMap<String, Vec<usize>>,
    oi: usize,
    p: &Placed,
    image: &Image<'_>,
    gnu: bool,
    cursor: Option<&mut HashMap<String, u64>>,
) {
    let obj = &out.objs[oi];
    let machine = obj.elf.machine();
    let cands = candidates(obj, names, &p.section);
    let what = match &p.section {
        SectionRef::Index(i) => format!("section {i}"),
        SectionRef::Name(n) => format!("'{n}'"),
    };
    if cands.is_empty() {
        // GNU ld names the first input file as the origin of the sections
        // it makes (.iplt, .rela.iplt, .got, ...): those are its own.
        if !gnu {
            out.findings.push(Finding::new(
                Rule::Map(1),
                format!("line {}: {} has no section {what}", p.line, obj.label()),
            ));
        }
        return;
    }
    if let Some(off) = p.input_offset {
        if let [si] = cands.as_slice() {
            let piece = Piece { input_offset: off, addr: p.addr, size: p.size };
            let s = &mut out.objs[oi].secs[*si];
            match &mut s.place {
                Place::Pieces(v) => v.push(piece),
                Place::Absent => s.place = Place::Pieces(vec![piece]),
                _ => {
                    let label = out.objs[oi].label();
                    out.findings.push(Finding::new(
                        Rule::Map(2),
                        format!("line {}: {label} {what} is placed whole and as pieces", p.line),
                    ));
                }
            }
        } else {
            for &si in &cands {
                out.objs[oi].secs[si].place = Place::Ambiguous;
            }
        }
        return;
    }
    let free: Vec<usize> =
        cands.iter().copied().filter(|&i| matches!(obj.secs[i].place, Place::Absent)).collect();
    let fits: Vec<usize> = free
        .iter()
        .copied()
        .filter(|&i| size_may_change(&obj.secs[i], machine) || obj.secs[i].shdr.sh_size == p.size)
        .collect();
    let chosen = match fits.len() {
        // GNU ld names the first input as the origin of the `.eh_frame`
        // it writes for its PLT (x86): a further `.eh_frame` row of that
        // file is the linker's (the FDEs are judged by §6.4, which knows
        // a reference's PLT FDE).
        0 if free.is_empty() && gnu && cands.iter().all(|&i| obj.secs[i].is_eh_frame(machine)) => {
            return;
        }
        0 if free.is_empty() => {
            out.findings.push(Finding::new(
                Rule::Map(2),
                format!("line {}: {} {what} is placed more than once", p.line, obj.label()),
            ));
            return;
        }
        0 => {
            out.findings.push(
                Finding::new(
                    Rule::Map(3),
                    format!(
                        "line {}: {} {what} is placed with size {:#x}, its input size is {:#x}",
                        p.line,
                        obj.label(),
                        p.size,
                        obj.secs[free[0]].shdr.sh_size
                    ),
                )
                .at(p.addr),
            );
            if free.len() == 1 {
                // The size rule fails; the section still has this place.
                out.objs[oi].secs[free[0]].place = Place::Whole(p.addr);
            }
            return;
        }
        1 => fits[0],
        _ => {
            if let Some(i) = disambiguate(obj, &fits, p, image) {
                i
            } else {
                for &si in &fits {
                    out.objs[oi].secs[si].place = Place::Ambiguous;
                }
                return;
            }
        }
    };
    let s = &mut out.objs[oi].secs[chosen];
    let mut addr = p.addr;
    // A non-alloc section's place is its offset in the output section of
    // its name, whose address is 0 (§3.3): GNU ld and lld write it.
    if !s.alloc()
        && !s.merge()
        && let Some(c) = cursor
    {
        let at = c.entry(s.name.clone()).or_default();
        let off = at.checked_next_multiple_of(s.shdr.sh_addralign.max(1)).unwrap_or(u64::MAX);
        *at = off.saturating_add(p.size);
        addr = image
            .sections
            .iter()
            .find(|o| o.name == s.name && !o.alloc())
            .map_or(off, |o| o.shdr.sh_addr.wrapping_add(off));
    }
    s.place = if s.is_eh_frame(machine) && s.shdr.sh_size != p.size {
        Place::Rewritten
    } else {
        Place::Whole(addr)
    };
}

/// Chooses among same-named, same-sized sections of one file (§3.3): the
/// one whose bytes outside its relocated ranges are the output's at the
/// record's address; failing that, the first in index order when all of
/// them are indistinguishable (equal bytes and equal relocation records);
/// otherwise none.
fn disambiguate(obj: &Obj<'_>, fits: &[usize], p: &Placed, image: &Image<'_>) -> Option<usize> {
    let first = &obj.secs[fits[0]];
    if first.shdr.sh_type != SHT_NOBITS
        && let Some(outb) = image.bytes(p.addr, p.size)
    {
        let matching: Vec<usize> = fits
            .iter()
            .copied()
            .filter(|&i| equal_outside(&obj.data(i), &covered(obj, i), outb))
            .collect();
        if let [one] = matching.as_slice() {
            return Some(*one);
        }
    }
    let recs = |i: usize| obj.secs[i].relocs.and_then(|r| obj.records(r)).unwrap_or_default();
    let r0 = recs(fits[0]);
    let d0 = obj.data(fits[0]);
    fits.iter()
        .all(|&i| {
            *obj.data(i) == *d0 && obj.secs[i].shdr.sh_type == first.shdr.sh_type && recs(i) == r0
        })
        .then_some(fits[0])
}

/// The map rules about places (§3.3): M4 (inside an output section with
/// compatible flags, aligned) and M5 (no overlap).
fn check_map(out: &mut Located<'_>, image: &Image<'_>) {
    let mut placed: Vec<(u64, u64, usize, usize)> = Vec::new();
    for (oi, o) in out.objs.iter().enumerate() {
        for (si, s) in o.secs.iter().enumerate() {
            let Place::Whole(addr) = s.place else { continue };
            // A section without bytes has an address only for its symbols;
            // a .tbss section occupies no memory of its segment.
            if !s.alloc() || s.shdr.sh_size == 0 || s.tbss() {
                continue;
            }
            // A section whose output size may differ from its input's (M3)
            // is checked at its first byte.
            let len = if size_may_change(s, o.elf.machine()) { 1 } else { s.shdr.sh_size };
            let align = s.shdr.sh_addralign.max(1);
            let site = o.site(si, 0);
            if align.is_power_of_two() && addr % align != 0 {
                out.findings.push(
                    Finding::new(
                        Rule::Map(4),
                        format!("placed at {addr:#x}, not a multiple of its alignment {align:#x}"),
                    )
                    .at(addr)
                    .site(site.clone()),
                );
            }
            match image.alloc_section_of(addr, len) {
                None => out.findings.push(
                    Finding::new(
                        Rule::Map(4),
                        format!("placed at [{addr:#x}, +{len:#x}), in no allocated output section"),
                    )
                    .at(addr)
                    .site(site),
                ),
                Some((_, os)) => {
                    let need = s.shdr.sh_flags & (SHF_EXECINSTR | SHF_WRITE);
                    if os.shdr.sh_flags & need != need {
                        out.findings.push(
                            Finding::new(
                                Rule::Map(4),
                                format!("placed in output section '{}', which lacks its flags {need:#x}", os.name),
                            )
                            .at(addr)
                            .site(site),
                        );
                    }
                }
            }
            if len > 0 {
                placed.push((addr, len, oi, si));
            }
        }
    }
    placed.sort_unstable();
    for w in placed.windows(2) {
        let (a, asz, ao, asi) = w[0];
        let (b, _, bo, bsi) = w[1];
        if a + asz > b {
            let other = out.objs[ao].site(asi, 0);
            out.findings.push(
                Finding::new(
                    Rule::Map(5),
                    format!(
                        "placed at {b:#x}, overlapping {}:({}) placed at [{a:#x}, +{asz:#x})",
                        out.objs[ao].label(),
                        other.section
                    ),
                )
                .at(b)
                .site(out.objs[bo].site(bsi, 0)),
            );
        }
    }
}

/// A relocation record as read: `(r_offset, symbol, type, addend)`, the
/// addend `None` for REL.
pub type RawRecord = (u64, u32, u32, Option<i64>);

#[cfg(test)]
mod tests {
    use super::{thin_member_path, thin_members};

    /// A thin archive of `names` (each a short name, no data).
    fn thin(names: &[&str]) -> Vec<u8> {
        let mut b = b"!<thin>\n".to_vec();
        for n in names {
            let head =
                format!("{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n", format!("{n}/"), 0, 0, 0, 644, 10);
            b.extend_from_slice(head.as_bytes());
        }
        b
    }

    // spec/plumb.md §2.2: a thin archive's member is the file at its stored
    // path, from the archive's directory.
    #[test]
    fn s2_2_thin_archive_members() {
        assert_eq!(thin_member_path("libt.a", b"one.o"), "one.o");
        assert_eq!(thin_member_path("d/libt.a", b"lib/one.o"), "d/lib/one.o");
        assert_eq!(thin_member_path("d/libt.a", b"/abs/one.o"), "/abs/one.o");
        assert_eq!(thin_members("d/libt.a", &thin(&["one.o", "two.o"])), ["d/one.o", "d/two.o"]);
        assert!(thin_members("x.o", b"\x7fELF").is_empty());
    }
}
