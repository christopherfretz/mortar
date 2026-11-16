//! Mutation coverage as a number (`spec/plumb.md` §9.5, WP-028d): a
//! correct link is broken one field at a time, in each class of mistake a
//! linker can make, and every mutant must give a finding the clean link
//! does not have. The result is, per class, the mutants applied, the
//! ones caught and the ones missed (each named: a missed mutant is a hole
//! in the verifier, or an equivalent mutant that §9.5 explains).
//!
//! The mutants are chosen from the output, the inputs and the map with a
//! fixed seed: the same link gives the same mutants and the same table
//! (§7.1). Only the output is changed, as a linker's mistake changes it;
//! a mortar build id is re-sealed after each change (B1 would otherwise
//! catch every mutant, §6.7).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use mortar_elf::Machine;
use mortar_elf::consts::{
    PT_LOAD, PT_TLS, SHF_TLS, SHT_NOBITS, SHT_REL, SHT_RELA, SHT_SYMTAB, STT_FILE, STT_SECTION,
};

use crate::a64::{self, Def, Field, Form, Op};
use crate::buildid::{self, Style, Tree};
use crate::image::Image;
use crate::locate::{self, Place};
use crate::{Input, Map, verify};

/// The classes, in the order of the table (§9.5).
pub const CLASSES: &[&str] = &[
    "field-off-by-one",
    "field-wrong-symbol",
    "field-wrong-addend",
    "stale-relaxation",
    "got-slot-swap",
    "irelative-swap",
    "eh-hdr-order",
    "eh-hdr-entry",
    "fde-pc-begin",
    "symtab-value",
    "symtab-size",
    "e-entry",
    "header-ident",
    "phdr-filesz",
    "phdr-memsz",
    "phdr-align",
    "phdr-flags",
    "tls-segment",
    "property-bit",
    "linker-symbol",
    "section-overlap",
    "thunk-target",
    "iplt-entry",
    "erratum-fix",
    // Stage 3 (`crate::dynmutate::CLASSES`, same order).
    "dyn-needed",
    "dyn-tag",
    "dyn-rel-offset",
    "dyn-rel-drop",
    "dyn-rel-addend",
    "dyn-rel-symbol",
    "dyn-rel-order",
    "dynsym-value",
    "dynsym-visibility",
    "dynsym-unique",
    "gnu-hash",
    "versym",
    "version-need",
    "interp",
    "relro",
    "lazy-slot",
    "copy-size",
    "copy-relro",
    "copy-section",
    "plt0-disp1",
    "plt0-disp2",
    "plt-index",
    "plt-back-jump",
    "spie-rel-drop",
    "spie-rel-extra",
    "spie-irel-twice",
    "spie-interp",
    "spie-needed",
    "spie-relacount",
    "spie-dynamic-word",
];

/// How many mutants of each class to run, from which seed, on how many
/// threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub seed: u64,
    /// At most this many mutants per class (chosen by the seed); 0: all.
    pub per_class: usize,
    pub threads: usize,
    /// The byte sweep of the synthetic sections instead of the classes
    /// (`crate::sweep`, WP-082).
    pub sweep: bool,
    /// The sweep's window: in a section larger than twice this many
    /// bytes, only the first and last `window` bytes are flipped (a
    /// static-pie's `.rela.dyn` holds a thousand alike records; WP-089);
    /// 0: every byte.
    pub window: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config { seed: 0x6d6f_7274_6172_0028, per_class: 16, threads: 1, sweep: false, window: 0 }
    }
}

/// One class's result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClassResult {
    pub class: &'static str,
    /// Mutants that change the output (equivalent edits are not made).
    pub applied: usize,
    pub caught: usize,
    /// The mutants no new finding caught, by description.
    pub missed: Vec<String>,
    /// Candidates of the class in this link, before sampling.
    pub candidates: usize,
}

/// The result over one link.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// Findings of the unmutated link (a mortar link should have none).
    pub baseline: usize,
    pub classes: Vec<ClassResult>,
}

impl Outcome {
    /// Adds `other` class by class (several links in one table).
    pub fn add(&mut self, other: &Outcome, label: &str) {
        self.baseline += other.baseline;
        for c in &other.classes {
            let mine = if let Some(i) = self.classes.iter().position(|m| m.class == c.class) {
                &mut self.classes[i]
            } else {
                self.classes.push(ClassResult { class: c.class, ..ClassResult::default() });
                self.classes.last_mut().expect("pushed")
            };
            mine.applied += c.applied;
            mine.caught += c.caught;
            mine.candidates += c.candidates;
            mine.missed.extend(c.missed.iter().map(|m| format!("{label}: {m}")));
        }
    }

    /// The table: class, candidates, applied, caught, missed; then every
    /// missed mutant.
    pub fn render(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "{:22} {:>10} {:>8} {:>7} {:>7}",
            "class", "candidates", "applied", "caught", "missed"
        );
        let (mut a, mut c) = (0, 0);
        for r in &self.classes {
            let _ = writeln!(
                s,
                "{:22} {:>10} {:>8} {:>7} {:>7}",
                r.class,
                r.candidates,
                r.applied,
                r.caught,
                r.missed.len()
            );
            a += r.applied;
            c += r.caught;
        }
        let _ = writeln!(s, "{:22} {:>10} {:>8} {:>7} {:>7}", "total", "", a, c, a - c);
        for r in &self.classes {
            for m in &r.missed {
                let _ = writeln!(s, "missed {}: {m}", r.class);
            }
        }
        s
    }
}

/// One mutant: byte edits of the output file.
#[derive(Debug, Clone)]
struct Mutant {
    class: usize,
    what: String,
    edits: Vec<(usize, Vec<u8>)>,
}

/// `SplitMix64`: the fixed-seed choice of mutants.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn le(b: &[u8]) -> u64 {
    b.iter().rev().fold(0u64, |a, &x| a << 8 | u64::from(x))
}

fn bytes_of(v: u64, w: usize) -> Vec<u8> {
    v.to_le_bytes()[..w].to_vec()
}

/// The bits of `v` under `mask`, packed to the low end.
fn pext(v: u64, mask: u64) -> u64 {
    let (mut out, mut k) = (0u64, 0);
    for i in 0..64 {
        if mask >> i & 1 == 1 {
            out |= (v >> i & 1) << k;
            k += 1;
        }
    }
    out
}

/// The low bits of `v` spread over `mask`.
fn pdep(v: u64, mask: u64) -> u64 {
    let (mut out, mut k) = (0u64, 0);
    for i in 0..64 {
        if mask >> i & 1 == 1 {
            out |= (v >> k & 1) << i;
            k += 1;
        }
    }
    out
}

/// A relocated place of the output.
struct Place_ {
    p: u64,
    off: usize,
    def: &'static Def,
    /// The input's bytes of the record's window `[r_offset - 4, + 16)`
    /// (clipped), and where `r_offset` is in it.
    input: Vec<u8>,
    at: usize,
    /// The record's symbol: its name, or object and index for a local.
    key: String,
}

fn def_of(m: Machine, ty: u32) -> Option<&'static Def> {
    match m {
        Machine::AArch64 => a64::def(ty),
        Machine::X86_64 => crate::x64::def(ty),
        Machine::I386 => crate::x86::def(ty),
    }
}

/// The ELF header-table layout of the output's class: entry sizes and
/// field offsets (gABI "ELF Header", "Program Header", "Symbol Table").
struct Lay {
    /// An address's bytes.
    w: usize,
    sym: usize,
    sym_value: usize,
    sym_size: usize,
    ph: usize,
    ph_flags: usize,
    ph_filesz: usize,
    ph_memsz: usize,
    ph_align: usize,
    sh: usize,
    sh_size: usize,
}

impl Lay {
    const fn of(elf64: bool) -> Lay {
        if elf64 {
            Lay {
                w: 8,
                sym: 24,
                sym_value: 8,
                sym_size: 16,
                ph: 56,
                ph_flags: 4,
                ph_filesz: 32,
                ph_memsz: 40,
                ph_align: 48,
                sh: 64,
                sh_size: 32,
            }
        } else {
            Lay {
                w: 4,
                sym: 16,
                sym_value: 4,
                sym_size: 8,
                ph: 32,
                ph_flags: 24,
                ph_filesz: 16,
                ph_memsz: 20,
                ph_align: 28,
                sh: 40,
                sh_size: 20,
            }
        }
    }
}

fn pc_relative(op: Op) -> bool {
    matches!(op, Op::Prel | Op::GotPc | Op::Slot(_, Form::Prel | Form::PrelA | Form::Page))
}

struct Gen<'i, 'a> {
    image: &'i Image<'a>,
    out: Vec<Vec<Mutant>>,
}

impl Gen<'_, '_> {
    fn off(&self, addr: u64, len: u64) -> Option<usize> {
        self.image
            .phdrs
            .iter()
            .find(|p| {
                p.p_type == PT_LOAD
                    && addr >= p.p_vaddr
                    && addr.checked_add(len).is_some_and(|e| e <= p.p_vaddr + p.p_filesz)
            })
            .and_then(|p| usize::try_from(p.p_offset + (addr - p.p_vaddr)).ok())
    }

    fn push(&mut self, class: &str, what: String, edits: Vec<(usize, Vec<u8>)>) {
        let data = self.image.data;
        let changes = edits.iter().any(|(o, b)| data.get(*o..*o + b.len()).is_some_and(|d| d != b));
        let fits =
            edits.iter().all(|(o, b)| o.checked_add(b.len()).is_some_and(|e| e <= data.len()));
        if !changes || !fits {
            return;
        }
        let ci = CLASSES.iter().position(|c| *c == class).expect("a class of CLASSES");
        self.out[ci].push(Mutant { class: ci, what, edits });
    }

    fn word(&self, off: usize, w: usize) -> u64 {
        le(&self.image.data[off..off + w])
    }
}

/// The relocated places of the placed input sections.
#[allow(clippy::many_single_char_names)] // the spec's names
fn places(image: &Image<'_>, map: &Map, inputs: &[Input<'_>]) -> Vec<Place_> {
    let located = locate::locate(map, inputs, image);
    let g = Gen { image, out: Vec::new() };
    let mut out = Vec::new();
    for (oi, o) in located.objs.iter().enumerate() {
        let machine = o.elf.machine();
        for s in &o.secs {
            let Some(rs) = s.relocs else { continue };
            if !s.alloc() || s.shdr.sh_type == SHT_NOBITS || s.shdr.sh_flags & SHF_TLS != 0 {
                continue;
            }
            let Some(recs) = o.records(rs) else { continue };
            for (r_offset, sym, ty, _) in recs {
                let Some(def) = def_of(machine, ty) else { continue };
                // A marker (x86-64 TLSDESC_CALL, i386 TLS_DESC_CALL) has no
                // field: only its instruction's 2 bytes, for stale-relaxation.
                let marker = def.field == Field::Insn
                    || (machine == Machine::I386 && ty == crate::x86::TLS_DESC_CALL);
                let w = if marker { 2 } else { u64::from(def.field.width()) };
                if !marker && (def.field.mask() == 0 || w == 0) {
                    continue;
                }
                let p = match &s.place {
                    Place::Whole(a) => a.wrapping_add(r_offset),
                    Place::Pieces(ps) => {
                        let Some(pc) = ps.iter().find(|pc| {
                            r_offset >= pc.input_offset && r_offset + w <= pc.input_offset + pc.size
                        }) else {
                            continue;
                        };
                        pc.addr + (r_offset - pc.input_offset)
                    }
                    _ => continue,
                };
                let Some(off) = g.off(p, w) else { continue };
                let lo = r_offset.saturating_sub(4) as usize;
                let hi = (r_offset as usize + 16).min(s.data.len());
                if hi <= lo || r_offset as usize + w as usize > s.data.len() {
                    continue;
                }
                let key = o
                    .symtab
                    .and_then(|t| {
                        let d = t.get(mortar_elf::SymbolIndex(sym)).ok()?;
                        let local = d.binding() == mortar_elf::consts::STB_LOCAL;
                        let n = t.name(&d).ok().filter(|n| !n.is_empty() && !local)?;
                        Some(String::from_utf8_lossy(n).into_owned())
                    })
                    .unwrap_or_else(|| format!("{oi}:{sym}"));
                out.push(Place_ {
                    p,
                    off,
                    def,
                    input: s.data[lo..hi].to_vec(),
                    at: r_offset as usize - lo,
                    key,
                });
            }
        }
    }
    out
}

/// Every candidate mutant of the link, by class.
#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // one block per class, in CLASSES' order
fn candidates(
    image: &Image<'_>,
    map: &Map,
    inputs: &[Input<'_>],
    rng: &mut Rng,
    failed: &BTreeSet<u64>,
) -> Vec<Vec<Mutant>> {
    let mut g = Gen { image, out: vec![Vec::new(); CLASSES.len()] };
    let data = image.data;
    let elf64 = image.class() == mortar_elf::Class::Elf64;
    let lay = Lay::of(elf64);
    let i386 = image.header.e_machine == mortar_elf::consts::EM_386;
    // A place the clean link already has a finding at is not mutated: its
    // mutants would repeat that finding (an anomaly's records, A6).
    let mut places = places(image, map, inputs);
    places.retain(|pl| !failed.contains(&pl.p));
    let mut by_type: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, pl) in places.iter().enumerate() {
        by_type.entry(pl.def.r_type).or_default().push(i);
    }
    for (i, pl) in places.iter().enumerate() {
        let name = pl.def.name;
        let at = format!("{name} at {:#x} ({})", pl.p, pl.key);
        if pl.def.field == Field::Insn || (i386 && pl.def.r_type == crate::x86::TLS_DESC_CALL) {
            let inp = &pl.input;
            let desc = if i386 { crate::x86::TLS_DESC_CALL } else { crate::x64::TLSDESC_CALL };
            if matches!(
                image.header.e_machine,
                mortar_elf::consts::EM_X86_64 | mortar_elf::consts::EM_386
            ) && pl.def.r_type == desc
                && pl.at + 2 <= inp.len()
            {
                g.push(
                    "stale-relaxation",
                    format!("{at}: the input's call kept"),
                    vec![(pl.off, inp[pl.at..pl.at + 2].to_vec())],
                );
            }
            continue;
        }
        let w = usize::from(pl.def.field.width());
        let mask = pl.def.field.mask();
        let v = g.word(pl.off, w);
        // field-off-by-one: one unit of the field's lowest bit.
        let f = pext(v, mask).wrapping_add(1);
        g.push(
            "field-off-by-one",
            format!("{at}: field + 1"),
            vec![(pl.off, bytes_of((v & !mask) | pdep(f, mask), w))],
        );
        // field-wrong-symbol: the value another record of the type gives
        // its own symbol, made this place's.
        if let Some(others) = by_type.get(&pl.def.r_type) {
            for _ in 0..8 {
                let o = &places[others[rng.below(others.len())]];
                if o.key == pl.key {
                    continue;
                }
                let v2 = g.word(o.off, w);
                let field = pl.def.field;
                let new = if !pc_relative(pl.def.op) {
                    (v & !mask) | (v2 & mask)
                } else if let Field::Data(_) = field {
                    v2.wrapping_add(o.p).wrapping_sub(pl.p) & mask
                } else if field == Field::AdrPage {
                    let t = a64::page(o.p).wrapping_add(a64::decode(field, v2, o.p) as u64);
                    field.encode(v, t.wrapping_sub(a64::page(pl.p)) as i64)
                } else {
                    let t = o.p.wrapping_add(a64::decode(field, v2, o.p) as u64);
                    field.encode(v, t.wrapping_sub(pl.p) as i64)
                };
                if new != v {
                    g.push(
                        "field-wrong-symbol",
                        format!("{at}: {}'s value", o.key),
                        vec![(pl.off, bytes_of(new, w))],
                    );
                    break;
                }
            }
        }
        // field-wrong-addend: 16 bytes further (a page for the page
        // fields, 16 units of a MOVW group).
        let new = match pl.def.field {
            Field::Data(_) => v.wrapping_add(16) & mask,
            f @ (Field::AdrPage | Field::Hi12) => f.encode(v, a64::decode(f, v, pl.p) + 0x1000),
            f @ (Field::Movw(s) | Field::MovwSigned(s)) => {
                f.encode(v, a64::decode(f, v, pl.p) + (16 << s))
            }
            f => f.encode(v, a64::decode(f, v, pl.p) + 16),
        };
        g.push("field-wrong-addend", format!("{at}: + 16"), vec![(pl.off, bytes_of(new, w))]);
        // stale-relaxation: the input's instruction bytes around a
        // rewritten record, with the output's field.
        let inp = &pl.input;
        match image.header.e_machine {
            mortar_elf::consts::EM_AARCH64 if w == 4 => {
                let iw = le(&inp[pl.at..pl.at + 4]);
                if iw & !mask != v & !mask {
                    g.push(
                        "stale-relaxation",
                        format!("{at}: the input's instruction kept"),
                        vec![(pl.off, bytes_of((iw & !mask) | (v & mask), 4))],
                    );
                }
            }
            mortar_elf::consts::EM_X86_64 => {
                use crate::x64 as x;
                let (lo, hi): (usize, usize) = match pl.def.r_type {
                    x::GOTPCREL
                    | x::GOTPCRELX
                    | x::REX_GOTPCRELX
                    | x::GOTTPOFF
                    | x::GOTPC32_TLSDESC => (3, 0),
                    x::TLSGD | x::TLSLD => (4, 12),
                    _ => (0, 0),
                };
                if lo == 0 || pl.at < lo || pl.off < lo {
                    continue;
                }
                let mut edits = vec![(pl.off - lo, inp[pl.at - lo..pl.at].to_vec())];
                if hi > 0 && pl.at + 4 + hi <= inp.len() {
                    edits.push((pl.off + 4, inp[pl.at + 4..pl.at + 4 + hi].to_vec()));
                }
                g.push("stale-relaxation", format!("{at}: the input's bytes kept"), edits);
            }
            mortar_elf::consts::EM_386 => {
                use crate::x86 as x;
                // spec/x86.md §8, §9: the bytes before the field, and a
                // GD/LD sequence's call after it.
                let (lo, hi): (usize, usize) = match pl.def.r_type {
                    x::GOT32X | x::TLS_IE | x::TLS_GOTIE | x::TLS_GOTDESC => (2, 0),
                    x::TLS_GD | x::TLS_LDM => (3, 6),
                    _ => (0, 0),
                };
                if lo == 0 || pl.at < lo || pl.off < lo {
                    continue;
                }
                let mut edits = vec![(pl.off - lo, inp[pl.at - lo..pl.at].to_vec())];
                if hi > 0 && pl.at + 4 + hi <= inp.len() {
                    edits.push((pl.off + 4, inp[pl.at + 4..pl.at + 4 + hi].to_vec()));
                }
                g.push("stale-relaxation", format!("{at}: the input's bytes kept"), edits);
            }
            _ => {}
        }
        let _ = i;
    }
    // Sections by name.
    let sec = |n: &str| image.sections.iter().find(|s| s.name == n && s.shdr.sh_type != SHT_NOBITS);
    // got-slot-swap: two words of the GOT with different contents.
    for n in [".got", ".got.plt"] {
        let Some(s) = sec(n) else { continue };
        let Ok(o) = usize::try_from(s.shdr.sh_offset) else { continue };
        let ww = lay.w;
        let words = s.shdr.sh_size as usize / ww;
        if o + words * ww > data.len() {
            continue;
        }
        for k in 0..words.saturating_sub(1) {
            let (a, b) = (o + ww * k, o + ww * (k + 1));
            let (wa, wb) = (data[a..a + ww].to_vec(), data[b..b + ww].to_vec());
            g.push(
                "got-slot-swap",
                format!("{n} words {k} and {} swapped", k + 1),
                vec![(a, wb), (b, wa)],
            );
        }
    }
    // irelative-swap: two IRELATIVE records' resolvers swapped (one
    // record: its resolver moved by 4).
    let irelative: u32 = match image.header.e_machine {
        mortar_elf::consts::EM_AARCH64 => 1032,
        mortar_elf::consts::EM_386 => 42,
        _ => 37,
    };
    if i386 {
        // REL: the resolver is the word at r_offset (X86-50).
        let mut words = Vec::new();
        for s in image.sections.iter().filter(|s| s.alloc() && s.shdr.sh_type == SHT_REL) {
            let Ok(o) = usize::try_from(s.shdr.sh_offset) else { continue };
            for k in 0..(s.shdr.sh_size / 8) as usize {
                let e = o + 8 * k;
                if e + 8 <= data.len()
                    && le(&data[e + 4..e + 5]) == u64::from(irelative)
                    && let Some(w) = g.off(le(&data[e..e + 4]), 4)
                {
                    words.push(w);
                }
            }
        }
        for (k, &e) in words.iter().enumerate() {
            let a = data[e..e + 4].to_vec();
            if let Some(&f) = words.get(k + 1) {
                g.push(
                    "irelative-swap",
                    format!("IRELATIVE records {k} and {} swap resolvers", k + 1),
                    vec![(e, data[f..f + 4].to_vec()), (f, a.clone())],
                );
            }
            g.push(
                "irelative-swap",
                format!("IRELATIVE record {k}: resolver + 4"),
                vec![(e, bytes_of(le(&a).wrapping_add(4), 4))],
            );
        }
    }
    let mut irel = Vec::new();
    for s in image.sections.iter().filter(|s| s.alloc() && s.shdr.sh_type == SHT_RELA) {
        let Ok(o) = usize::try_from(s.shdr.sh_offset) else { continue };
        for k in 0..(s.shdr.sh_size / 24) as usize {
            let e = o + 24 * k;
            if e + 24 <= data.len() && le(&data[e + 8..e + 12]) == u64::from(irelative) {
                irel.push(e);
            }
        }
    }
    for (k, &e) in irel.iter().enumerate() {
        let a = &data[e + 16..e + 24];
        if let Some(&f) = irel.get(k + 1) {
            let b = &data[f + 16..f + 24];
            g.push(
                "irelative-swap",
                format!("IRELATIVE records {k} and {} swap resolvers", k + 1),
                vec![(e + 16, b.to_vec()), (f + 16, a.to_vec())],
            );
        }
        g.push(
            "irelative-swap",
            format!("IRELATIVE record {k}: resolver + 4"),
            vec![(e + 16, bytes_of(le(a).wrapping_add(4), 8))],
        );
    }
    // .eh_frame_hdr: the search table in its usual encodings (pcrel
    // sdata4, udata4, datarel sdata4).
    if let Some(h) = sec(".eh_frame_hdr")
        && let Ok(o) = usize::try_from(h.shdr.sh_offset)
        && data.get(o..o + 4) == Some(&[1, 0x1b, 0x03, 0x3b][..])
        && o + 12 <= data.len()
    {
        let n = le(&data[o + 8..o + 12]) as usize;
        let t = o + 12;
        if t + 8 * n <= data.len() {
            for k in 0..n {
                let e = t + 8 * k;
                if k + 1 < n {
                    g.push(
                        "eh-hdr-order",
                        format!(".eh_frame_hdr entries {k} and {} swapped", k + 1),
                        vec![(e, data[e + 8..e + 16].to_vec()), (e + 8, data[e..e + 8].to_vec())],
                    );
                }
                let loc = le(&data[e..e + 4]);
                g.push(
                    "eh-hdr-entry",
                    format!(".eh_frame_hdr entry {k}: initial location + 4"),
                    vec![(e, bytes_of(loc.wrapping_add(4), 4))],
                );
                let fde = le(&data[e + 4..e + 8]);
                g.push(
                    "eh-hdr-entry",
                    format!(".eh_frame_hdr entry {k}: FDE address + 4"),
                    vec![(e + 4, bytes_of(fde.wrapping_add(4), 4))],
                );
            }
        }
    }
    // fde-pc-begin: each FDE's initial location + 4.
    if let Some(s) = sec(".eh_frame")
        && let Ok(o) = usize::try_from(s.shdr.sh_offset)
        && let Some(b) = data.get(o..o + s.shdr.sh_size as usize)
    {
        for (at, w) in fde_pc_begins(b, lay.w) {
            let v = le(&b[at..at + w]);
            g.push(
                "fde-pc-begin",
                format!(".eh_frame FDE at {:#x}: initial location + 4", s.shdr.sh_addr + at as u64),
                vec![(o + at, bytes_of(v.wrapping_add(4), w))],
            );
        }
    }
    // .symtab: a value + 4, a size + 1.
    if let Some(s) = image.sections.iter().find(|s| s.shdr.sh_type == SHT_SYMTAB)
        && let Ok(o) = usize::try_from(s.shdr.sh_offset)
    {
        for (k, sym) in image.symbols.iter().enumerate().skip(1) {
            let ty = sym.sym.sym_type();
            if sym.sym.st_shndx == 0 || ty == STT_SECTION || ty == STT_FILE {
                continue;
            }
            let e = o + lay.sym * k;
            if e + lay.sym > data.len() {
                break;
            }
            let label = format!("{} ({:#x})", sym.name, sym.sym.st_value);
            g.push(
                "symtab-value",
                format!("{label}: value + 4"),
                vec![(e + lay.sym_value, bytes_of(sym.sym.st_value.wrapping_add(4), lay.w))],
            );
            g.push(
                "symtab-size",
                format!("{label}: size {} + 1", sym.sym.st_size),
                vec![(e + lay.sym_size, bytes_of(sym.sym.st_size.wrapping_add(1), lay.w))],
            );
        }
        // linker-symbol: a symbol the linker defines (Y5's names) + 8.
        for (k, sym) in image.symbols.iter().enumerate().skip(1) {
            let n = sym.name.as_str();
            let y5 = matches!(
                n,
                "__ehdr_start"
                    | "__executable_start"
                    | "__init_array_start"
                    | "__init_array_end"
                    | "__fini_array_start"
                    | "__fini_array_end"
                    | "__preinit_array_start"
                    | "__preinit_array_end"
                    | "_GLOBAL_OFFSET_TABLE_"
                    | "__GNU_EH_FRAME_HDR"
                    | "_edata"
                    | "edata"
                    | "__bss_start"
                    | "__bss_start__"
                    | "_end"
                    | "end"
                    | "__end__"
                    | "_bss_end__"
                    | "__bss_end__"
                    | "_etext"
                    | "etext"
                    | "__etext"
                    | "__rela_iplt_start"
                    | "__rela_iplt_end"
                    | "__rel_iplt_start"
                    | "__rel_iplt_end"
                    | "_DYNAMIC"
                    | "_TLS_MODULE_BASE_"
            ) || n.starts_with("__start_")
                || n.starts_with("__stop_");
            if !y5 || sym.sym.st_shndx == 0 {
                continue;
            }
            let e = o + lay.sym * k;
            if e + lay.sym <= data.len() {
                g.push(
                    "linker-symbol",
                    format!("{n} ({:#x}) + 8", sym.sym.st_value),
                    vec![(e + lay.sym_value, bytes_of(sym.sym.st_value.wrapping_add(8), lay.w))],
                );
            }
        }
    }
    // e_entry.
    let entry = image.header.e_entry;
    g.push(
        "e-entry",
        format!("e_entry {entry:#x} + 4"),
        vec![(24, bytes_of(entry.wrapping_add(4), lay.w))],
    );
    g.push(
        "e-entry",
        format!("e_entry {entry:#x} + 0x10000000"),
        vec![(24, bytes_of(entry.wrapping_add(0x1000_0000), lay.w))],
    );
    // EI_OSABI and e_flags (S1).
    let osabi = image.header.ei_osabi;
    g.push(
        "header-ident",
        format!("EI_OSABI {osabi} made {}", osabi ^ 3),
        vec![(7, vec![osabi ^ 3])],
    );
    g.push("header-ident", format!("EI_OSABI {osabi} made 9"), vec![(7, vec![9])]);
    if matches!(image.header.e_machine, mortar_elf::consts::EM_386 | mortar_elf::consts::EM_X86_64)
    {
        let at = if elf64 { 48 } else { 36 };
        let f = image.header.e_flags;
        g.push(
            "header-ident",
            format!("e_flags {f:#x} | 1"),
            vec![(at, bytes_of(u64::from(f | 1), 4))],
        );
    }
    // Program headers.
    let phoff = image.header.e_phoff as usize;
    for (k, p) in image.phdrs.iter().enumerate() {
        let e = phoff + lay.ph * k;
        let (fl, fs, ms, al, w) = (lay.ph_flags, lay.ph_filesz, lay.ph_memsz, lay.ph_align, lay.w);
        let ty = match p.p_type {
            PT_LOAD => "PT_LOAD".to_owned(),
            PT_TLS => "PT_TLS".to_owned(),
            t => format!("{t:#x}"),
        };
        let at = format!("phdr {k} ({ty} at {:#x})", p.p_vaddr);
        if p.p_type == PT_TLS {
            g.push(
                "tls-segment",
                format!("{at}: p_memsz + p_align"),
                vec![(e + ms, bytes_of(p.p_memsz.wrapping_add(p.p_align.max(1)), w))],
            );
            g.push(
                "tls-segment",
                format!("{at}: p_align * 2"),
                vec![(e + al, bytes_of(p.p_align.max(1) * 2, w))],
            );
            if p.p_filesz >= 8 {
                g.push(
                    "tls-segment",
                    format!("{at}: p_filesz - 8"),
                    vec![(e + fs, bytes_of(p.p_filesz - 8, w))],
                );
            }
            continue;
        }
        if p.p_filesz >= 8 {
            g.push(
                "phdr-filesz",
                format!("{at}: p_filesz - 8"),
                vec![(e + fs, bytes_of(p.p_filesz - 8, w))],
            );
        }
        g.push(
            "phdr-filesz",
            format!("{at}: p_filesz + 8"),
            vec![(e + fs, bytes_of(p.p_filesz + 8, w))],
        );
        if p.p_memsz >= 8 {
            g.push(
                "phdr-memsz",
                format!("{at}: p_memsz - 8"),
                vec![(e + ms, bytes_of(p.p_memsz - 8, w))],
            );
        }
        g.push(
            "phdr-memsz",
            format!("{at}: p_memsz + 0x10"),
            vec![(e + ms, bytes_of(p.p_memsz + 0x10, w))],
        );
        g.push(
            "phdr-align",
            format!("{at}: p_align * 2"),
            vec![(e + al, bytes_of(p.p_align.max(1) * 2, w))],
        );
        if p.p_align > 1 {
            g.push(
                "phdr-align",
                format!("{at}: p_align / 2"),
                vec![(e + al, bytes_of(p.p_align / 2, w))],
            );
        }
        for (bit, n) in [(1u32, "PF_X"), (2, "PF_W"), (4, "PF_R")] {
            g.push(
                "phdr-flags",
                format!("{at}: {n} toggled"),
                vec![(e + fl, (p.p_flags ^ bit).to_le_bytes().to_vec())],
            );
        }
    }
    // property-bit: each of the low 4 bits of the feature words.
    if let Some(s) = sec(".note.gnu.property")
        && let Ok(o) = usize::try_from(s.shdr.sh_offset)
        && let Some(b) = data.get(o..o + s.shdr.sh_size as usize)
        && b.len() >= 16
    {
        let descsz = le(&b[4..8]) as usize;
        let mut p = 16;
        while p + 8 <= (16 + descsz).min(b.len()) {
            let ty = le(&b[p..p + 4]) as u32;
            let sz = le(&b[p + 4..p + 8]) as usize;
            if sz == 4 && p + 12 <= b.len() && matches!(ty, 0xc000_0000 | 0xc000_0002 | 0xc000_8002)
            {
                let v = le(&b[p + 8..p + 12]) as u32;
                for bit in 0..4 {
                    g.push(
                        "property-bit",
                        format!("property {ty:#x} = {v:#x}: bit {bit} toggled"),
                        vec![(o + p + 8, (v ^ 1 << bit).to_le_bytes().to_vec())],
                    );
                }
            }
            p += (8 + sz).next_multiple_of(lay.w);
        }
    }
    // section-overlap: an allocated section grown 8 bytes into the next.
    let mut alloc: Vec<(usize, u64, u64)> = image
        .sections
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            s.alloc()
                && s.shdr.sh_size > 0
                && !(s.shdr.sh_flags & SHF_TLS != 0 && s.shdr.sh_type == SHT_NOBITS)
        })
        .map(|(i, s)| (i, s.shdr.sh_addr, s.shdr.sh_size))
        .collect();
    alloc.sort_by_key(|x| (x.1, x.0));
    let shoff = image.header.e_shoff as usize;
    for w in alloc.windows(2) {
        let ((i, a, sz), (j, b, _)) = (w[0], w[1]);
        if a + sz <= b {
            let name = &image.sections[i].name;
            g.push(
                "section-overlap",
                format!("{name} grown into {}", image.sections[j].name),
                vec![(shoff + lay.sh * i + lay.sh_size, bytes_of(b + 8 - a, lay.w))],
            );
        }
    }
    // thunk-target: a range-extension thunk (the map's chunks) reaching 4
    // bytes past its target.
    // A thunk chunk may hold several veneers, and padding (GNU ld's
    // `.text.stub`): each veneer symbol in it is a start; failing one, the
    // chunk's start.
    let mut starts: Vec<(String, u64)> = Vec::new();
    for c in map.chunks.iter().filter(|c| c.kind == crate::map::ChunkKind::Thunk) {
        let Some(addr) = c.addr else { continue };
        let mut named: Vec<(String, u64)> = image
            .symbols
            .iter()
            .filter(|x| {
                let v = x.sym.st_value;
                v >= addr
                    && v < addr + c.size
                    && !x.name.starts_with('$')
                    && !x.name.is_empty()
                    && x.sym.sym_type() != STT_SECTION
            })
            .map(|x| (format!("{} in {}", x.name, c.name), x.sym.st_value))
            .collect();
        if named.is_empty() {
            named.push((c.name.clone(), addr));
        }
        starts.extend(named);
    }
    starts.sort_by_key(|x| x.1);
    starts.dedup_by_key(|x| x.1);
    for (cname, addr) in starts {
        let Some(o) = g.off(addr, 12) else { continue };
        let w0 = g.word(o, 4) as u32;
        let w1 = g.word(o + 4, 4) as u32;
        let (at, new) = if a64::add_imm(w1).is_some() {
            let imm = (w1 >> 10) & 0xfff;
            (o + 4, (w1 & !(0xfff << 10)) | ((imm + 4) & 0xfff) << 10)
        } else {
            (o, w0 ^ 1 << 5)
        };
        g.push(
            "thunk-target",
            format!("thunk {cname} at {addr:#x}: target + 4"),
            vec![(at, new.to_le_bytes().to_vec())],
        );
    }
    // iplt-entry: an IPLT entry loading the next slot, branching through
    // another register, or (x86-64 IBT) without its endbr64.
    for s in image.sections.iter().filter(|s| matches!(s.name.as_str(), ".iplt" | ".plt")) {
        if i386 {
            // spec/x86.md X86-48/49: `ff 25 <word>`, after `endbr32` with IBT.
            let (Ok(o), Ok(n)) =
                (usize::try_from(s.shdr.sh_offset), usize::try_from(s.shdr.sh_size))
            else {
                continue;
            };
            let Some(b) = data.get(o..o + n) else { continue };
            let mut k = 0;
            for j in 0..n.saturating_sub(5) {
                if b[j] != 0xff || b[j + 1] != 0x25 || k >= 64 {
                    continue;
                }
                let at = format!("{} entry {k} at {:#x}", s.name, s.shdr.sh_addr + j as u64);
                if j >= 4 && b[j - 4..j] == crate::x86::ENDBR32 {
                    g.push(
                        "iplt-entry",
                        format!("{at}: endbr32 made a NOP"),
                        vec![(o + j - 4, vec![0x0f, 0x1f, 0x40, 0x00])],
                    );
                }
                let d = le(&b[j + 2..j + 6]).wrapping_add(4);
                g.push(
                    "iplt-entry",
                    format!("{at}: the next word"),
                    vec![(o + j + 2, bytes_of(d, 4))],
                );
                k += 1;
            }
            continue;
        }
        let (Ok(o), Ok(n)) = (usize::try_from(s.shdr.sh_offset), usize::try_from(s.shdr.sh_size))
        else {
            continue;
        };
        if o + n > data.len() {
            continue;
        }
        for k in (0..n / 16).take(64) {
            let e = o + 16 * k;
            let at = format!("{} entry {k} at {:#x}", s.name, s.shdr.sh_addr + 16 * k as u64);
            match image.header.e_machine {
                mortar_elf::consts::EM_AARCH64 => {
                    for (w, what) in
                        [(1usize, "the slot's low 12 bits + 8"), (2, "another register")]
                    {
                        let v = g.word(e + 4 * w, 4) as u32;
                        let new = if w == 1 { v.wrapping_add(1 << 10) } else { v ^ 1 };
                        g.push(
                            "iplt-entry",
                            format!("{at}: {what}"),
                            vec![(e + 4 * w, new.to_le_bytes().to_vec())],
                        );
                    }
                }
                mortar_elf::consts::EM_X86_64 => {
                    let b = &data[e..e + 16];
                    if b[..4] == [0xf3, 0x0f, 0x1e, 0xfa] {
                        g.push(
                            "iplt-entry",
                            format!("{at}: endbr64 made a NOP"),
                            vec![(e, vec![0x0f, 0x1f, 0x40, 0x00])],
                        );
                    }
                    // `ff 25 <rel32>` whole inside the 16 bytes (a mold
                    // entry's `jmp` at +6 overran with a scan to +13).
                    if let Some(j) = (0..=10).find(|&j| b[j] == 0xff && b[j + 1] == 0x25) {
                        let d = le(&b[j + 2..j + 6]).wrapping_add(8);
                        g.push(
                            "iplt-entry",
                            format!("{at}: the next slot"),
                            vec![(e + j + 2, bytes_of(d, 4))],
                        );
                    }
                }
                _ => {}
            }
        }
    }
    // erratum-fix (a64.md §11.2): an ADR fix to the next page, or undone
    // (the ADRP of the same page put back: a missing fix); a patch whose
    // load has its offset changed (the relocation not applied, as WP-032e
    // found), or undone (the load put back at F).
    if image.header.e_machine == mortar_elf::consts::EM_AARCH64 {
        let exec = image.sections.iter().filter(|s| {
            s.shdr.sh_flags & mortar_elf::consts::SHF_EXECINSTR != 0 && s.shdr.sh_addr != 0
        });
        let mut ranges: Vec<(u64, u64)> =
            exec.map(|s| (s.shdr.sh_addr, s.shdr.sh_addr + s.shdr.sh_size)).collect();
        ranges.sort_unstable();
        for (lo, hi) in ranges {
            let mut page = lo & !0xFFF;
            while page < hi {
                for q in [page + 0xFF8, page + 0xFFC] {
                    if q < lo || q + 4 > hi {
                        continue;
                    }
                    let Some(o) = g.off(q, 4) else { continue };
                    let w = g.word(o, 4) as u32;
                    if let Some((d, t)) = a64::adr(w, q)
                        && t.trailing_zeros() >= 12
                    {
                        let next = adr_word(d, q, t.wrapping_add(0x1000));
                        let back = adrp_word(d, q, t);
                        if let Some(n) = next {
                            g.push(
                                "erratum-fix",
                                format!("ADR fix at {q:#x}: the next page"),
                                vec![(o, n.to_le_bytes().to_vec())],
                            );
                        }
                        if let Some(b) = back {
                            g.push(
                                "erratum-fix",
                                format!("ADR fix at {q:#x}: undone"),
                                vec![(o, b.to_le_bytes().to_vec())],
                            );
                        }
                    }
                }
                for f in [page, page + 4, page + 8] {
                    if f < lo || f + 4 > hi {
                        continue;
                    }
                    let Some(o) = g.off(f, 4) else { continue };
                    let w = g.word(o, 4) as u32;
                    let Some((false, t)) = a64::branch(w, f) else { continue };
                    let (Some(to), Some(bo)) = (g.off(t, 4), g.off(t.wrapping_add(4), 4)) else {
                        continue;
                    };
                    let moved = g.word(to, 4) as u32;
                    let back = g.word(bo, 4) as u32;
                    if !a64::is_ldst_unsigned_imm(moved)
                        || a64::branch(back, t.wrapping_add(4)) != Some((false, f.wrapping_add(4)))
                    {
                        continue;
                    }
                    let imm = (moved >> 10) & 0xFFF;
                    let stale = if imm == 0 { moved | 1 << 10 } else { moved & !(0xFFF << 10) };
                    g.push(
                        "erratum-fix",
                        format!("patch for {f:#x} at {t:#x}: offset not relocated"),
                        vec![(to, stale.to_le_bytes().to_vec())],
                    );
                    g.push(
                        "erratum-fix",
                        format!("patch for {f:#x}: undone"),
                        vec![(o, moved.to_le_bytes().to_vec())],
                    );
                }
                page += 0x1000;
            }
        }
    }
    for (class, what, edits) in crate::dynmutate::candidates(image) {
        g.push(class, what, edits);
    }
    g.out
}

/// `ADR Xd, target` at `pc`, if it reaches.
fn adr_word(d: u32, pc: u64, target: u64) -> Option<u32> {
    let delta = target.wrapping_sub(pc) as i64;
    (-(1 << 20)..(1 << 20)).contains(&delta).then(|| {
        let v = delta as u32 & 0x1F_FFFF;
        0x1000_0000 | (v & 3) << 29 | (v >> 2) << 5 | d
    })
}

/// `ADRP Xd, page` at `pc`, if it reaches.
fn adrp_word(d: u32, pc: u64, page: u64) -> Option<u32> {
    let delta = (page.wrapping_sub(pc & !0xFFF) as i64) >> 12;
    (-(1 << 20)..(1 << 20)).contains(&delta).then(|| {
        let v = delta as u32 & 0x1F_FFFF;
        0x9000_0000 | (v & 3) << 29 | (v >> 2) << 5 | d
    })
}

/// The `(offset, width)` of each FDE's initial location in `.eh_frame`'s
/// bytes `b`, from its CIE's `R` encoding.
fn fde_pc_begins(b: &[u8], addr: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut cies: BTreeMap<usize, usize> = BTreeMap::new();
    let mut at = 0usize;
    while at + 8 <= b.len() {
        let len = le(&b[at..at + 4]) as usize;
        if len == 0 || len == 0xffff_ffff || at + 4 + len > b.len() {
            break;
        }
        let id = le(&b[at + 4..at + 8]) as usize;
        if id == 0 {
            if let Some(w) = cie_width(&b[at + 8..at + 4 + len], addr) {
                cies.insert(at, w);
            }
        } else if let Some(cie) = (at + 4).checked_sub(id)
            && let Some(&w) = cies.get(&cie)
            && at + 8 + w <= at + 4 + len
        {
            out.push((at + 8, w));
        }
        at += 4 + len;
    }
    out
}

fn uleb(b: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for s in (0..64).step_by(7) {
        let x = *b.get(*at)?;
        *at += 1;
        v |= u64::from(x & 0x7f) << s;
        if x & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// The width of the FDE pointers a CIE's body (after the id) gives.
fn cie_width(b: &[u8], addr: usize) -> Option<usize> {
    let version = *b.first()?;
    let nul = b[1..].iter().position(|&c| c == 0)? + 1;
    let aug = &b[1..nul];
    let mut at = nul + 1;
    uleb(b, &mut at)?;
    uleb(b, &mut at)?; // the data alignment's bytes, as an unsigned LEB
    if version == 1 {
        at += 1;
    } else {
        uleb(b, &mut at)?;
    }
    let mut enc = 0u8; // absptr
    if aug.first() == Some(&b'z') {
        uleb(b, &mut at)?;
        for &c in &aug[1..] {
            match c {
                b'R' => {
                    enc = *b.get(at)?;
                    at += 1;
                }
                b'L' => at += 1,
                b'P' => {
                    let pe = *b.get(at)?;
                    at += 1 + width_of(pe, addr)?;
                }
                _ => {}
            }
        }
    }
    width_of(enc, addr)
}

/// The width of a pointer encoding; `absptr` is an address (`addr`).
fn width_of(enc: u8, addr: usize) -> Option<usize> {
    match enc & 0x0f {
        0x00 => Some(addr),
        0x04 | 0x0c => Some(8),
        0x03 | 0x0b => Some(4),
        0x02 | 0x0a => Some(2),
        _ => None,
    }
}

/// The findings of a report as comparable keys.
fn keys(r: &crate::Report) -> BTreeSet<(String, Option<u64>, String)> {
    r.findings.iter().map(|f| (f.rule.id(), f.addr, f.message.clone())).collect()
}

/// Runs the mutants of every class on the link (`output`, `inputs`,
/// `map`): see the module's documentation.
///
/// # Errors
///
/// When the output cannot be read (O1): there is nothing to mutate.
pub fn run(
    output: &[u8],
    inputs: &[Input<'_>],
    map: &Map,
    cfg: &Config,
) -> Result<Outcome, String> {
    run_with(output, inputs, map, cfg, None)
}

/// [`run`], judging each mutant with stage 3 when `ctx` is given and the
/// output is dynamic (`spec/plumb.md` §12).
///
/// # Errors
///
/// As [`run`].
pub fn run_with(
    output: &[u8],
    inputs: &[Input<'_>],
    map: &Map,
    cfg: &Config,
    ctx: Option<&crate::Ctx<'_>>,
) -> Result<Outcome, String> {
    let verify = |out: &[u8], inputs: &[Input<'_>], map: &Map| match ctx {
        Some(c) if crate::is_dynamic(out) => crate::verify_with(out, inputs, map, c),
        _ => verify(out, inputs, map),
    };
    let image = Image::read(output)?;
    let mut rng = Rng(cfg.seed);
    let mut base = verify(output, inputs, map);
    // A link whose fixes show the erratum option keeps it when a mutant
    // undoes them (§5.10: otherwise the option would be inferred off).
    let mut given = map.clone();
    if given.fix_843419.is_none() && base.coverage.erratum_sites > 0 {
        given.fix_843419 = Some(true);
        base = verify(output, inputs, &given);
    }
    let map = &given;
    let failed: BTreeSet<u64> = base.findings.iter().filter_map(|f| f.addr).collect();
    let all = candidates(&image, map, inputs, &mut rng, &failed);
    let base_keys = keys(&base);
    let style = Style::of(map);
    let mut chosen: Vec<Mutant> = Vec::new();
    let mut classes: Vec<ClassResult> =
        CLASSES.iter().map(|c| ClassResult { class: c, ..ClassResult::default() }).collect();
    for (ci, mut ms) in all.into_iter().enumerate() {
        classes[ci].candidates = ms.len();
        if cfg.per_class > 0 && ms.len() > cfg.per_class {
            // A partial Fisher-Yates shuffle, then the chosen in their
            // original order.
            let mut idx: Vec<usize> = (0..ms.len()).collect();
            for k in 0..cfg.per_class {
                let j = k + rng.below(idx.len() - k);
                idx.swap(k, j);
            }
            let mut pick: Vec<usize> = idx[..cfg.per_class].to_vec();
            pick.sort_unstable();
            ms = pick.into_iter().map(|k| ms[k].clone()).collect();
        }
        chosen.extend(ms);
    }
    let judge = |m: &Mutant| -> bool {
        let mut out = output.to_vec();
        for (o, b) in &m.edits {
            out[*o..*o + b.len()].copy_from_slice(b);
        }
        match style {
            Style::Tree(t, _) => {
                buildid::seal(&mut out, t);
            }
            Style::Unstated => {
                buildid::seal(&mut out, Tree::L128);
            }
            Style::Given | Style::Unknown => {}
        }
        let r = verify(&out, inputs, map);
        keys(&r).iter().any(|k| !base_keys.contains(k))
    };
    let threads = cfg.threads.max(1).min(chosen.len().max(1));
    let caught: Vec<bool> = if threads == 1 {
        chosen.iter().map(judge).collect()
    } else {
        let mut res = vec![false; chosen.len()];
        std::thread::scope(|s| {
            let chunk = chosen.len().div_ceil(threads);
            let handles: Vec<_> = chosen
                .chunks(chunk)
                .map(|c| s.spawn(move || c.iter().map(judge).collect::<Vec<bool>>()))
                .collect();
            let mut k = 0;
            for h in handles {
                for v in h.join().expect("a mutation thread") {
                    res[k] = v;
                    k += 1;
                }
            }
        });
        res
    };
    for (m, c) in chosen.iter().zip(caught) {
        let r = &mut classes[m.class];
        r.applied += 1;
        if c {
            r.caught += 1;
        } else {
            r.missed.push(m.what.clone());
        }
    }
    Ok(Outcome { baseline: base.findings.len(), classes })
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: plumb.md §12 — stage 3's classes close the table, in order.
    #[test]
    fn s12_dynamic_classes_close_the_table() {
        let tail = &CLASSES[CLASSES.len() - crate::dynmutate::CLASSES.len()..];
        let names: Vec<&str> = crate::dynmutate::CLASSES.iter().map(|c| c.0).collect();
        assert_eq!(tail, names.as_slice());
    }

    // spec: Arm A64 ADR/ADRP encodings (a64.md §11.2's `adr` and W1).
    #[test]
    fn s9_5_adr_and_adrp_words() {
        assert_eq!(adr_word(1, 0x41_2ffc, 0x41_3000), Some(0x1000_0021));
        let w = adr_word(1, 0x41_2ffc, 0x41_3000).unwrap();
        assert_eq!(a64::adr(w, 0x41_2ffc), Some((1, 0x41_3000)));
        let p = adrp_word(0, 0x41_2ffc, 0x62_3000).unwrap();
        assert_eq!(a64::adrp(p, 0x41_2ffc), Some((0, 0x62_3000)));
        assert_eq!(adr_word(0, 0, 1 << 21), None);
    }

    #[test]
    fn s9_5_bit_packing() {
        let m = 0x6000_0000 | 0x7ffff << 5;
        for v in [0u64, 1, 0x1234, 0x1f_ffff] {
            assert_eq!(pext(pdep(v, m), m), v & 0x1f_ffff);
        }
        assert_eq!(pdep(1, 0xff0), 0x10);
    }

    #[test]
    fn s9_5_cie_pointer_width() {
        // version 1, "zR", code 4, data -8 (0x78), ra 30, aug len 1, R = 0x1b.
        let cie = [1u8, b'z', b'R', 0, 4, 0x78, 30, 1, 0x1b];
        assert_eq!(cie_width(&cie, 8), Some(4));
        let plain = [1u8, 0, 4, 0x78, 30];
        assert_eq!(cie_width(&plain, 8), Some(8));
    }
}
