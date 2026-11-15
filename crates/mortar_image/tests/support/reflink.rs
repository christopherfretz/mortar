//! Glue for end-to-end tests against a reference link (WP-026; reusable by
//! WP-027): a GNU ld map-file reader, a small reader for the linked
//! executable, and a plan that places the user objects' input sections
//! where the reference put them, with every symbol value read from the
//! reference.
//!
//! What a [`RefWorld`] compares: each placed input section of the given
//! objects (`SHT_PROGBITS`/`SHT_INIT_ARRAY`/... with file bytes, not merge
//! sections, not `.eh_frame`, which the reference rewrites) is a plan row
//! at the reference's file offset; its records are applied by the image
//! with `S` from the reference (globals from the executable's symbol table,
//! locals and section symbols from the map's placement, discarded sections
//! as dead targets). The filled bytes of every row are then compared with
//! the executable's bytes at the same offset, except the fields of the
//! records left out ([`Left`]), which are masked.
#![allow(dead_code, clippy::missing_panics_doc, clippy::too_many_lines)]

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

use mortar_arch::{Proof, ProofRun, Reloc, SymValue, TargetKind};
use mortar_core::{FileId, SectionId};
use mortar_elf::{Object, SectionIndex, SymbolSection};
use mortar_image::plan::{Builder, InputUnit, Plan, RecOrder, RelocSection};

use super::{FakeTables, MemSource, a64, key};

const SHF_ALLOC: u64 = 2;
const SHF_EXECINSTR: u64 = 4;
const SHF_MERGE: u64 = 0x10;
const SHT_NOBITS: u32 = 8;
const SHT_RELA: u32 = 4;
const STT_SECTION: u8 = 3;
const STT_TLS: u8 = 6;
const STT_GNU_IFUNC: u8 = 10;

/// One input-section line of a GNU ld map file ("Linker script and memory
/// map"): the output section it is in, its name, address, size and file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapLine {
    pub out: String,
    pub input: String,
    pub addr: u64,
    pub size: u64,
    pub file: String,
}

fn hex(s: &str) -> Option<u64> {
    u64::from_str_radix(s.strip_prefix("0x")?, 16).ok()
}

/// The input-section lines of a GNU ld `-Map` file. A long name is alone on
/// its line, its address, size and file on the next; `*fill*`, patterns
/// (`*(...)`), symbol lines (no file) and the discarded list are skipped.
#[must_use]
pub fn parse_map(text: &str) -> Vec<MapLine> {
    let mut out = Vec::new();
    let Some(start) = text.find("Linker script and memory map") else { return out };
    let mut cur_out = String::new();
    let mut pending: Option<String> = None;
    for line in text[start..].lines() {
        if line.is_empty() {
            pending = None;
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        if !line.starts_with(' ') {
            // An output section (or a script statement).
            if let Some(name) = toks.first()
                && name.starts_with('.')
            {
                (*name).clone_into(&mut cur_out);
            }
            pending = None;
            continue;
        }
        if line.starts_with("  ") {
            // A continuation: address, size, file of the pending name.
            if let Some(name) = pending.take()
                && toks.len() >= 3
                && let (Some(addr), Some(size)) = (hex(toks[0]), hex(toks[1]))
            {
                out.push(MapLine {
                    out: cur_out.clone(),
                    input: name,
                    addr,
                    size,
                    file: toks[2..].join(" "),
                });
            }
            continue;
        }
        let name = toks[0];
        if name.starts_with('*') {
            pending = None;
            continue;
        }
        if toks.len() == 1 {
            pending = Some(name.to_owned());
            continue;
        }
        if toks.len() >= 4
            && let (Some(addr), Some(size)) = (hex(toks[1]), hex(toks[2]))
        {
            out.push(MapLine {
                out: cur_out.clone(),
                input: name.to_owned(),
                addr,
                size,
                file: toks[3..].join(" "),
            });
        }
        pending = None;
    }
    out
}

/// A section header of the linked executable.
#[derive(Clone, Debug)]
pub struct ExeSection {
    pub name: String,
    pub sh_type: u32,
    pub flags: u64,
    pub addr: u64,
    pub offset: u64,
    pub size: u64,
}

/// A defined symbol of the executable's `.symtab`.
#[derive(Clone, Copy, Debug)]
pub struct ExeSym {
    pub value: u64,
    pub st_type: u8,
    pub global: bool,
}

/// The linked executable (ELF64 little-endian): section headers and the
/// defined symbols by name (a global wins over a local of the same name).
pub struct RefExe {
    pub bytes: Vec<u8>,
    pub sections: Vec<ExeSection>,
    pub syms: HashMap<Vec<u8>, ExeSym>,
}

fn rd(b: &[u8], o: usize, n: usize) -> u64 {
    let mut v = [0u8; 8];
    v[..n].copy_from_slice(&b[o..o + n]);
    u64::from_le_bytes(v)
}

fn cstr(b: &[u8], o: usize) -> &[u8] {
    let e = b[o..].iter().position(|&c| c == 0).map_or(b.len(), |p| o + p);
    &b[o..e]
}

impl RefExe {
    #[must_use]
    pub fn parse(bytes: Vec<u8>) -> Self {
        let b = &bytes;
        assert!(b.starts_with(b"\x7fELF\x02\x01"), "not ELF64 LE");
        let shoff = rd(b, 0x28, 8) as usize;
        let shnum = rd(b, 0x3c, 2) as usize;
        let shstrndx = rd(b, 0x3e, 2) as usize;
        let hdr = |i: usize| shoff + 64 * i;
        let names_off = rd(b, hdr(shstrndx) + 24, 8) as usize;
        let mut sections = Vec::new();
        let mut symtab = None;
        for i in 0..shnum {
            let h = hdr(i);
            let s = ExeSection {
                name: String::from_utf8_lossy(cstr(b, names_off + rd(b, h, 4) as usize))
                    .into_owned(),
                sh_type: rd(b, h + 4, 4) as u32,
                flags: rd(b, h + 8, 8),
                addr: rd(b, h + 16, 8),
                offset: rd(b, h + 24, 8),
                size: rd(b, h + 32, 8),
            };
            if s.sh_type == 2 {
                symtab = Some((s.offset as usize, s.size as usize, rd(b, h + 40, 4) as usize));
            }
            sections.push(s);
        }
        let mut syms = HashMap::new();
        if let Some((off, size, link)) = symtab {
            let str_off = sections[link].offset as usize;
            for k in 1..size / 24 {
                let e = off + 24 * k;
                let name = cstr(b, str_off + rd(b, e, 4) as usize).to_vec();
                let info = b[e + 4];
                let shndx = rd(b, e + 6, 2);
                if shndx == 0 || name.is_empty() {
                    continue;
                }
                let sym =
                    ExeSym { value: rd(b, e + 8, 8), st_type: info & 0xf, global: info >> 4 != 0 };
                match syms.get(&name) {
                    Some(ExeSym { global: true, .. }) if !sym.global => {}
                    _ => {
                        syms.insert(name, sym);
                    }
                }
            }
        }
        Self { bytes, sections, syms }
    }

    /// The file offset of address (or, for a non-alloc section, offset)
    /// `addr` in output section `out`.
    #[must_use]
    pub fn offset_in(&self, out: &str, addr: u64) -> Option<u64> {
        let s = self.sections.iter().find(|s| s.name == out && s.sh_type != SHT_NOBITS)?;
        (s.addr..s.addr + s.size).contains(&addr).then(|| s.offset + (addr - s.addr))
    }

    /// The `PT_TLS` segment as a TLS block, if any.
    #[must_use]
    pub fn tls(&self) -> Option<mortar_arch::TlsBlock> {
        let b = &self.bytes;
        let phoff = rd(b, 0x20, 8) as usize;
        let phnum = rd(b, 0x38, 2) as usize;
        (0..phnum).map(|i| phoff + 56 * i).find(|&p| rd(b, p, 4) == 7).map(|p| {
            mortar_arch::TlsBlock {
                start: rd(b, p + 16, 8),
                size: rd(b, p + 40, 8),
                align: rd(b, p + 48, 8),
            }
        })
    }
}

/// Why a record was left out of the plan (its field is masked).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Left {
    /// A GOT-relative or GOT-entry type: the slot is the reference's.
    Got,
    /// A TLS type (the reference relaxes them in a static executable).
    Tls,
    /// The target is an `STT_GNU_IFUNC`: the reference points it at an
    /// IPLT entry.
    Ifunc,
    /// The target is in a merge section, which the reference rewrote.
    Merge,
    /// The target is undefined in the executable (a weak reference).
    Undefined,
}

/// The placed sections of one object.
#[derive(Debug)]
pub struct RefUnit {
    pub file: usize,
    pub name: String,
    pub offset: u64,
    pub size: u64,
    pub records: usize,
}

/// A plan built from a reference link, with what to compare.
pub struct RefWorld {
    pub plan: Plan,
    pub source: MemSource,
    pub tables: FakeTables,
    pub units: Vec<RefUnit>,
    /// File ranges not compared (fields of left-out records).
    pub masks: Vec<Range<u64>>,
    pub left: BTreeMap<Left, usize>,
    /// Records applied against a discarded (dead) target.
    pub dead: usize,
    /// The fields of those records (file offset, width, referring section):
    /// masked in [`compare`] and checked against the I-47 table by
    /// [`check_tombstones`] instead, since GNU ld redirects a reference into
    /// a dropped COMDAT copy to the kept copy (spec/image.md §16 table).
    pub tombs: Vec<(u64, u64, String)>,
    /// Global definitions of the objects whose map-derived value disagrees
    /// with the executable's symbol table (must be empty: checks the glue).
    pub disagree: Vec<String>,
}

fn field_width(r_type: u32) -> u64 {
    match r_type {
        257 | 260 => 8,
        259 | 262 => 2,
        t if t >= 1024 => 8,
        _ => 4,
    }
}

fn left_for(r_type: u32) -> Option<Left> {
    match r_type {
        300..=313 => Some(Left::Got),
        t if t >= 512 => Some(Left::Tls),
        _ => None,
    }
}

/// Places the input sections of `objects` (name as the map shows it, and
/// bytes) where `map` says, and builds the plan of the whole executable.
#[must_use]
pub fn ref_world(
    exe: &RefExe,
    map: &[MapLine],
    objects: &[(&str, &[u8])],
    window: u64,
) -> RefWorld {
    let pool = mortar_core::par::Pool::new(2);
    let mut source = MemSource::default();
    let mut tables = FakeTables::default();
    let mut b = Builder::new(a64(), exe.bytes.len() as u64);
    let mut units = Vec::new();
    let mut masks = Vec::new();
    let mut left: BTreeMap<Left, usize> = BTreeMap::new();
    let mut dead = 0usize;
    let mut tombs = Vec::new();
    let mut disagree = Vec::new();
    let mut next_key = 1u32;
    let mut next_section = 0u32;
    for (fi, &(map_name, bytes)) in objects.iter().enumerate() {
        let file = FileId(fi as u32);
        source.files.insert(file, (bytes.to_vec(), true));
        let obj = Object::parse(bytes).expect("object");
        // Map lines of this object, in order, matched to its sections by
        // name (sections of one name are listed in section order).
        let mut lines: BTreeMap<&str, Vec<&MapLine>> = BTreeMap::new();
        for l in map.iter().filter(|l| l.file == map_name) {
            lines.entry(l.input.as_str()).or_default().push(l);
        }
        let mut placed: HashMap<u32, (u64, Option<u64>)> = HashMap::new(); // index -> (addr, file offset)
        let mut merge: Vec<u32> = Vec::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        for s in obj.sections() {
            let name = String::from_utf8_lossy(s.name().unwrap()).into_owned();
            let flags = s.flags();
            if s.merge().unwrap().is_some() || flags & SHF_MERGE != 0 {
                merge.push(s.index().0);
                continue;
            }
            let Some(ls) = lines.get(name.as_str()) else { continue };
            let k = seen.entry(name.clone()).or_default();
            // A same-name section of size 0 is still listed; match by order
            // among the sections listed with that name and size.
            let Some(pos) =
                ls.iter().enumerate().skip(*k).find(|(_, l)| l.size == s.size()).map(|(i, _)| i)
            else {
                continue;
            };
            *k = pos + 1;
            let l = ls[pos];
            let off = (s.sh_type() != SHT_NOBITS).then(|| exe.offset_in(&l.out, l.addr)).flatten();
            placed.insert(s.index().0, (l.addr, off));
        }
        let symtab = obj.symbols().unwrap().expect("symtab");
        // Keys: one per symbol index.
        let base = next_key;
        next_key += symtab.len();
        for (si, sym) in symtab.iter() {
            let k = key(base + si.0);
            let name = symtab.name(&sym).unwrap();
            let sect = symtab.section_of(si, &sym).unwrap();
            let local = sym.binding() == 0;
            match sect {
                SymbolSection::Section(SectionIndex(x))
                    if local || sym.st_info & 0xf == STT_SECTION =>
                {
                    if let Some(&(addr, _)) = placed.get(&x) {
                        tables.define(k, addr + sym.st_value, true);
                    } else if merge.contains(&x) {
                        tables.names.insert(k, b"<merge>".to_vec());
                    } else {
                        tables.syms.insert(
                            k,
                            SymValue { s: 0, kind: TargetKind::Local, size: 0, live: false },
                        );
                    }
                }
                SymbolSection::Absolute => {
                    tables.define(k, sym.st_value, true);
                }
                _ if !local => {
                    if let Some(e) = exe.syms.get(name) {
                        if e.st_type == STT_GNU_IFUNC {
                            tables.names.insert(k, b"<ifunc>".to_vec());
                        } else {
                            tables.define(k, e.value, true);
                        }
                        if let SymbolSection::Section(SectionIndex(x)) = sect
                            && let Some(&(addr, _)) = placed.get(&x)
                            && addr + sym.st_value != e.value
                            && e.st_type != STT_GNU_IFUNC
                            && e.st_type != STT_TLS
                        {
                            disagree.push(String::from_utf8_lossy(name).into_owned());
                        }
                    }
                }
                _ => {}
            }
        }
        // Relocation sections by target.
        let mut rela_of: HashMap<u32, Vec<Reloc>> = HashMap::new();
        let mut left_of: HashMap<u32, Vec<(u64, u64, Left)>> = HashMap::new();
        for s in obj.sections() {
            if s.sh_type() != SHT_RELA {
                continue;
            }
            let rs = s.rela().unwrap();
            let target = rs.target().0;
            if !placed.contains_key(&target) {
                continue;
            }
            for r in rs {
                let k = key(base + r.r_sym);
                let why = left_for(r.r_type).or_else(|| {
                    if r.r_sym == 0 {
                        return None;
                    }
                    match tables.names.get(&k).map(Vec::as_slice) {
                        Some(b"<merge>") => Some(Left::Merge),
                        Some(b"<ifunc>") => Some(Left::Ifunc),
                        _ if !tables.syms.contains_key(&k) => Some(Left::Undefined),
                        _ => None,
                    }
                });
                if let Some(w) = why {
                    left_of.entry(target).or_default().push((r.r_offset, field_width(r.r_type), w));
                    continue;
                }
                if r.r_sym != 0 && !tables.syms[&k].live {
                    dead += 1;
                    let name = String::from_utf8_lossy(
                        obj.section(SectionIndex(target)).unwrap().name().unwrap(),
                    )
                    .into_owned();
                    if let Some(&(_, Some(off))) = placed.get(&target) {
                        let w = field_width(r.r_type);
                        if name == ".debug_aranges" {
                            // I-112: the tuple's length too.
                            tombs.push((off + r.r_offset + w, w, LENGTH.to_owned()));
                        }
                        tombs.push((off + r.r_offset, w, name));
                    }
                }
                if r.r_sym == 0 {
                    tables.define(k, 0, true);
                }
                rela_of.entry(target).or_default().push(Reloc {
                    r_offset: r.r_offset,
                    key: k,
                    r_type: r.r_type,
                    addend: r.r_addend,
                });
            }
        }
        for s in obj.sections() {
            let x = s.index().0;
            let Some(&(addr, Some(off))) = placed.get(&x) else { continue };
            let name = String::from_utf8_lossy(s.name().unwrap()).into_owned();
            if name == ".eh_frame" || s.size() == 0 {
                continue;
            }
            let records = rela_of.remove(&x).unwrap_or_default();
            for (o, w, why) in left_of.remove(&x).unwrap_or_default() {
                *left.entry(why).or_default() += 1;
                masks.push(off + o..off + (o + w).min(s.size()));
            }
            let exec = s.flags() & SHF_EXECINSTR != 0;
            let nrec = records.len();
            let relocs = (!records.is_empty()).then(|| {
                let order = RecOrder::of(&records);
                let proof = if exec {
                    vec![ProofRun {
                        start: 0,
                        end: s.size(),
                        proof: Proof::Insn,
                        unit: 0,
                        unit_offset: 0,
                    }]
                } else {
                    Vec::new()
                };
                b.relocs(RelocSection {
                    records: records.into_boxed_slice(),
                    order,
                    proof: proof.into_boxed_slice(),
                })
            });
            let name_id = b.name(name.as_bytes());
            let src = s.file_range().unwrap().expect("bytes").start as u64;
            let alloc = s.flags() & SHF_ALLOC != 0;
            b.input(
                off,
                s.size(),
                InputUnit {
                    file,
                    index: x,
                    section: SectionId(next_section),
                    src,
                    start: 0,
                    section_size: s.size(),
                    addr,
                    alloc,
                    name: name_id,
                    relocs,
                },
            );
            next_section += 1;
            units.push(RefUnit { file: fi, name, offset: off, size: s.size(), records: nrec });
        }
    }
    let plan = b.finish(window, &pool).expect("plan");
    RefWorld { plan, source, tables, units, masks, left, dead, tombs, disagree }
}

/// A difference between the fill and the reference: file offset, ours,
/// theirs, the unit's name.
#[derive(Debug)]
pub struct Diff {
    pub offset: u64,
    pub ours: u8,
    pub theirs: u8,
    pub unit: String,
}

/// Compares every unit's bytes outside the masks.
#[must_use]
pub fn compare(w: &RefWorld, ours: &[u8], theirs: &[u8]) -> (u64, Vec<Diff>) {
    let mut masked = vec![false; ours.len()];
    for m in w.masks.iter().cloned().chain(w.tombs.iter().map(|&(o, n, _)| o..o + n)) {
        for x in m {
            masked[x as usize] = true;
        }
    }
    let mut compared = 0u64;
    let mut diffs = Vec::new();
    for u in &w.units {
        for x in u.offset..u.offset + u.size {
            if masked[x as usize] {
                continue;
            }
            compared += 1;
            let (a, b) = (ours[x as usize], theirs[x as usize]);
            if a != b && diffs.len() < 64 {
                diffs.push(Diff { offset: x, ours: a, theirs: b, unit: u.name.clone() });
            }
        }
    }
    (compared, diffs)
}

/// The name [`RefWorld::tombs`] gives an `.debug_aranges` tuple's length.
pub const LENGTH: &str = "<aranges length>";

/// Checks every tombstoned field of `ours` against the I-47 table, and each
/// `.debug_aranges` tuple length after one against I-112's 0. Returns the
/// number checked and the fields that differ.
#[must_use]
pub fn check_tombstones(w: &RefWorld, ours: &[u8]) -> (usize, Vec<(u64, String, u64)>) {
    let mut bad = Vec::new();
    for (off, width, name) in &w.tombs {
        let want = if name == LENGTH {
            0
        } else {
            let Some(v) =
                mortar_image::tombstone::field(&super::ARCH, name.as_bytes(), false, *width as u8)
            else {
                continue;
            };
            v
        };
        let got = rd(ours, *off as usize, *width as usize);
        if got != want {
            bad.push((*off, name.clone(), got));
        }
    }
    (w.tombs.len(), bad)
}
