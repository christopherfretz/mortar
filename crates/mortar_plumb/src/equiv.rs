//! The equivalence of an incremental update and a full link
//! (`spec/plumb.md` §6.9; `spec/incr.md` IN-2, IN-62): `equiv(X, F)`.
//!
//! Each side is verified as any link; the P-E rules then compare the two
//! through identities (what an item is, not where it went). Never panics.

use std::collections::{BTreeMap, BTreeSet};

use mortar_elf::Machine;
use mortar_elf::consts::{
    ET_DYN, PT_TLS, SHF_ALLOC, SHF_MERGE, SHF_STRINGS, SHN_ABS, SHN_COMMON, SHN_UNDEF,
    SHT_NOBITS, SHT_NOTE, STB_GLOBAL, STB_GNU_UNIQUE, STB_WEAK, STT_FILE, STT_SECTION, STT_TLS,
};
use mortar_elf::{SectionIndex, SymbolIndex, SymbolSection};

use crate::image::Image;
use crate::locate::{Obj, Place};
use crate::map::{ChunkKind, Map, SectionRef};
use crate::reloc::Engine;
use crate::report::{Finding, Reason, Report, Rewrite, Rule};
use crate::{Input, a64};

/// What `equiv` found (§6.9).
#[derive(Debug, Default)]
pub struct Equiv {
    /// `X`'s and `F`'s own reports (every stage).
    pub x: Report,
    pub f: Report,
    /// The P-E findings, sorted.
    pub findings: Vec<Finding>,
    /// What was compared: relocation records traced on both sides, unit
    /// bytes compared, symbols matched, table entries matched.
    pub records: u64,
    pub bytes: u64,
    pub symbols: u64,
    pub entries: u64,
}

impl Equiv {
    /// Both sides clean and no P-E finding (exit status 0, §8).
    pub fn clean(&self) -> bool {
        self.x.clean() && self.f.clean() && self.findings.is_empty()
    }

    /// The P-E findings with rule `P-E{n}`.
    pub fn with(&self, n: u8) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(move |f| f.rule == Rule::Equiv(n))
    }

    /// One summary line for the pair.
    pub fn summary(&self) -> String {
        format!(
            "equiv: {} findings; X {} findings, F {} findings; {} records, {} bytes, {} symbols, {} table entries compared",
            self.findings.len(),
            self.x.findings.len(),
            self.f.findings.len(),
            self.records,
            self.bytes,
            self.symbols,
            self.entries
        )
    }
}

/// A record's outcome in one side's engine (P-E4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Form {
    /// Verified now, or left for a later pass (GOT pairs, pieces).
    Plain,
    Rewrite(Rewrite),
    NotChecked(Reason),
    Failed,
}

/// One traced record.
#[derive(Debug, Clone)]
pub(crate) struct Traced {
    pub oi: usize,
    pub si: usize,
    pub off: u64,
    pub form: Form,
}

/// The engine's counts before a record (compared after it).
pub(crate) struct Tally {
    verified: u64,
    failed: u64,
    findings: usize,
    accepted: Vec<(Rewrite, u64)>,
    not_checked: Vec<(Reason, u64)>,
}

pub(crate) fn tally(e: &Engine<'_, '_>) -> Tally {
    Tally {
        verified: e.cov.verified.values().sum(),
        failed: e.cov.failed,
        findings: e.findings.len(),
        accepted: e.cov.accepted.iter().map(|(k, v)| (*k, *v)).collect(),
        not_checked: e.cov.not_checked.iter().map(|(k, v)| (*k, *v)).collect(),
    }
}

/// The first key whose count grew.
fn grew<K: Copy + Ord>(now: &BTreeMap<K, u64>, was: &[(K, u64)]) -> Option<K> {
    now.iter()
        .find(|(k, v)| **v > was.iter().find(|(j, _)| j == *k).map_or(0, |(_, w)| *w))
        .map(|(k, _)| *k)
}

pub(crate) fn traced(e: &mut Engine<'_, '_>, oi: usize, si: usize, off: u64, b: &Tally) {
    let form = if e.findings.len() > b.findings || e.cov.failed > b.failed {
        Form::Failed
    } else if let Some(r) = grew(&e.cov.accepted, &b.accepted) {
        Form::Rewrite(r)
    } else if let Some(r) = grew(&e.cov.not_checked, &b.not_checked) {
        Form::NotChecked(r)
    } else {
        let _ = b.verified;
        Form::Plain
    };
    if let Some(t) = e.trace.as_mut() {
        t.push(Traced { oi, si, off, form });
    }
}

/// An input file across the two sides: path, member, and how many earlier
/// objects of one side had both.
type ObjKey = (String, Option<String>, u32);
/// An input section across the two sides.
type UKey = (ObjKey, u32);

/// Where one side put an input section, and what its records cover.
#[derive(Debug, Clone)]
struct Unit {
    alloc: bool,
    nobits: bool,
    tbss: bool,
    eh: bool,
    merge: bool,
    /// The non-alloc output section's index.
    out: Option<usize>,
    /// `(input offset, address, size)`: one for a whole section.
    pieces: Vec<(u64, u64, u64)>,
    size: u64,
    /// The relocated ranges `(offset, width)`.
    relocs: Vec<(u64, u64)>,
    /// The input record lengths of `.eh_frame` pieces, by input offset.
    eh_len: BTreeMap<u64, u64>,
}

/// One side, read and verified.
struct Side<'a> {
    tag: &'static str,
    image: Image<'a>,
    map: &'a Map,
    report: Report,
    forms: BTreeMap<(UKey, u64), Form>,
    units: BTreeMap<UKey, Unit>,
    /// Global definitions of the inputs: `(unit, value)`, or `None` for an
    /// absolute one with its value.
    defs: BTreeMap<String, Vec<(Option<UKey>, u64)>>,
    rewritten: BTreeSet<u64>,
    /// `(start, end, id, base)` sorted by start, with the running maximum
    /// of `end` (§6.9 identities).
    spans: Vec<(u64, u64, String, u64)>,
    pmax: Vec<u64>,
    tls: Option<u64>,
    machine: Option<Machine>,
}

fn file_key(map: &Map, file: u32) -> String {
    map.files.get(file as usize).map_or_else(
        || format!("#{file}"),
        |f| format!("{}({})@{:?}", f.path, f.member.as_deref().unwrap_or(""), f.member_offset),
    )
}

fn sec_key(s: &SectionRef) -> String {
    match s {
        SectionRef::Index(i) => format!("#{i}"),
        SectionRef::Name(n) => n.clone(),
    }
}

impl<'a> Side<'a> {
    /// Every identity of address `v` (§6.9): `(id, offset)`.
    fn ids(&self, v: u64) -> BTreeSet<(&str, u64)> {
        self.ids_open(v).into_iter().map(|(i, o, _)| (i, o)).collect()
    }

    /// [`Side::ids`] with whether the span holds `v` before its end.
    fn ids_open(&self, v: u64) -> BTreeSet<(&str, u64, bool)> {
        let mut out = BTreeSet::new();
        let mut i = self.spans.partition_point(|s| s.0 <= v);
        while i > 0 {
            i -= 1;
            if self.pmax[i] < v {
                break;
            }
            let (a, z, id, base) = &self.spans[i];
            if *a <= v && v <= *z {
                out.insert((id.as_str(), base + (v - a), v < *z));
            }
        }
        out
    }

    /// The first identity of `v` (the least of those whose span holds it
    /// before its end, else the least): two values with the same one
    /// correspond, and matching by it first keeps a multiset linear.
    fn canon(&self, v: u64) -> Option<(String, u64)> {
        let ids = self.ids_open(v);
        let mut open = ids.iter().filter(|i| i.2);
        open.next().or_else(|| ids.iter().next()).map(|(i, o, _)| ((*i).to_string(), *o))
    }

    /// The address of input definition `(unit, value)` on this side.
    fn def_addr(&self, u: &UKey, value: u64) -> Option<u64> {
        let unit = self.units.get(u)?;
        unit.pieces
            .iter()
            .find(|(o, _, z)| *o <= value && value <= o + z)
            .map(|(o, a, _)| a + (value - o))
    }

    fn word(&self) -> u64 {
        if self.image.class() == mortar_elf::Class::Elf32 { 4 } else { 8 }
    }

    fn sec(&self, name: &str) -> Option<usize> {
        self.image.sections.iter().position(|s| s.name == name)
    }

    fn sec_bytes(&self, name: &str) -> Option<&'a [u8]> {
        self.image.section_data(self.sec(name)?)
    }

    /// X's holes in output section `name`: `(offset, size)`.
    fn holes(&self, name: &str) -> Vec<(u64, u64)> {
        self.map
            .dead
            .iter()
            .filter(|d| d.section == name && d.kind.is_none())
            .map(|d| (d.offset, d.size))
            .collect()
    }
}

fn le(b: &[u8], o: usize, w: usize) -> Option<u64> {
    let s = b.get(o..o.checked_add(w)?)?;
    Some(s.iter().rev().fold(0u64, |a, &x| a << 8 | u64::from(x)))
}

/// Runs one side: its report and, from a stage-2 engine with the trace on,
/// its units, forms and definitions. `None` when the output cannot be read.
fn run_side<'a>(
    tag: &'static str,
    output: &'a [u8],
    inputs: &[Input<'a>],
    map: &'a Map,
) -> Result<Side<'a>, Report> {
    let image = match Image::read(output) {
        Ok(i) => i,
        Err(_) => return Err(crate::verify(output, inputs, map)),
    };
    let located = crate::locate::locate(map, inputs, &image);
    let objs: Vec<Obj<'a>> = located.objs;
    let mut engine = Engine::new(&image, &objs, map.format);
    engine.trace = Some(Vec::new());
    crate::run_engine(&mut engine, &image, map, None);
    let dynamic = crate::is_dynamic(output);
    let report = if dynamic {
        let ctx = crate::Ctx { line: None, files: &crate::HostFiles };
        crate::verify_with(output, inputs, map, &ctx)
    } else {
        let mut r = Report::default();
        r.findings.extend(crate::structure::check(&image));
        r.findings.extend(crate::structure::check_layout(&image, map.format));
        r.findings.extend(located.findings);
        r.findings.append(&mut engine.findings);
        r.coverage = std::mem::take(&mut engine.cov);
        r.sort();
        r
    };
    let mut seen: BTreeMap<(String, Option<String>), u32> = BTreeMap::new();
    let keys: Vec<ObjKey> = objs
        .iter()
        .map(|o| {
            let n = seen.entry((o.path.clone(), o.member.clone())).or_default();
            *n += 1;
            (o.path.clone(), o.member.clone(), *n - 1)
        })
        .collect();
    let mut forms = BTreeMap::new();
    for t in engine.trace.take().unwrap_or_default() {
        forms.insert(((keys[t.oi].clone(), t.si as u32), t.off), t.form);
    }
    let mut units = BTreeMap::new();
    for (oi, o) in objs.iter().enumerate() {
        let machine = o.elf.machine();
        for (si, s) in o.secs.iter().enumerate() {
            let pieces = match &s.place {
                Place::Whole(a) => vec![(0, *a, s.shdr.sh_size)],
                Place::Pieces(ps) => ps.iter().map(|p| (p.input_offset, p.addr, p.size)).collect(),
                _ => continue,
            };
            let mut relocs = Vec::new();
            if let Some(rs) = s.relocs
                && let Some(raw) = o.records(rs)
            {
                for (off, _, ty, _) in raw {
                    let w = crate::reloc::width(machine, ty);
                    if w > 0 {
                        relocs.push((off, u64::from(w)));
                    }
                }
            }
            relocs.sort_unstable();
            let eh = s.alloc() && s.is_eh_frame(machine);
            let mut eh_len = BTreeMap::new();
            if eh {
                for (o_in, _, _) in &pieces {
                    if let Some(l) = le(s.data, *o_in as usize, 4) {
                        eh_len.insert(*o_in, l + 4);
                    }
                }
            }
            let out = (!s.alloc()).then(|| image.nonalloc_section(&s.name)).flatten();
            units.insert(
                (keys[oi].clone(), si as u32),
                Unit {
                    alloc: s.alloc(),
                    nobits: s.shdr.sh_type == SHT_NOBITS,
                    tbss: s.tbss(),
                    eh,
                    merge: s.merge(),
                    out,
                    pieces,
                    size: s.shdr.sh_size,
                    relocs,
                    eh_len,
                },
            );
        }
    }
    let mut defs: BTreeMap<String, Vec<(Option<UKey>, u64)>> = BTreeMap::new();
    for (name, list) in &engine.defs {
        for &(oi, idx) in list {
            let Some(t) = objs[oi].symtab else { continue };
            let Ok(sym) = t.get(SymbolIndex(idx)) else { continue };
            let d = match t.section_of(SymbolIndex(idx), &sym) {
                Ok(SymbolSection::Section(k)) => (Some((keys[oi].clone(), k.0)), sym.st_value),
                Ok(SymbolSection::Absolute) => (None, sym.st_value),
                _ => continue,
            };
            defs.entry(name.clone()).or_default().push(d);
        }
    }
    let rewritten = std::mem::take(&mut engine.rewritten_words);
    let machine = objs.first().map(|o| o.elf.machine());
    drop(engine);
    let mut spans: Vec<(u64, u64, String, u64)> = Vec::new();
    for p in &map.placed {
        let id = format!("u:{}:{}", file_key(map, p.file), sec_key(&p.section));
        spans.push((p.addr, p.addr.saturating_add(p.size), id, p.input_offset.unwrap_or(0)));
    }
    let mut nth: BTreeMap<(ChunkKind, &str), u32> = BTreeMap::new();
    for c in &map.chunks {
        let k = nth.entry((c.kind, c.name.as_str())).or_default();
        *k += 1;
        if let Some(a) = c.addr {
            let id = format!("c:{}:{}:{}", c.kind.name(), c.name, *k - 1);
            spans.push((a, a.saturating_add(c.size), id, 0));
        }
    }
    for s in &image.sections {
        if s.alloc() {
            let a = s.shdr.sh_addr;
            spans.push((a, a, format!("s:{}", s.name), 0));
            let z = a.saturating_add(s.shdr.sh_size);
            spans.push((z, z, format!("e:{}", s.name), 0));
        }
    }
    spans.sort();
    let mut pmax = Vec::with_capacity(spans.len());
    let mut m = 0;
    for s in &spans {
        m = m.max(s.1);
        pmax.push(m);
    }
    let tls = image.phdrs.iter().find(|p| p.p_type == PT_TLS).map(|p| p.p_vaddr);
    Ok(Side {
        tag,
        image,
        map,
        report,
        forms,
        units,
        defs,
        rewritten,
        spans,
        pmax,
        tls,
        machine,
    })
}

/// Whether `vx` of `X` and `vf` of `F` correspond (§6.9): their identity
/// sets meet, or both are empty and the values equal.
fn corr(x: &Side<'_>, vx: u64, f: &Side<'_>, vf: u64) -> bool {
    let a = x.ids(vx);
    let b = f.ids(vf);
    if a.is_empty() && b.is_empty() {
        return vx == vf;
    }
    a.iter().any(|i| b.contains(i))
}

fn finding(n: u8, msg: impl Into<String>) -> Finding {
    Finding::new(Rule::Equiv(n), msg)
}

/// `equiv(X, F)` (§6.9): `X` an update (its map claims coverage), `F` a full
/// or plain link of the same inputs. `inputs` holds the files both maps
/// name. Never panics.
pub fn check(
    x_out: &[u8],
    x_map: &Map,
    f_out: &[u8],
    f_map: &Map,
    inputs: &[Input<'_>],
) -> Equiv {
    let mut eq = Equiv::default();
    let (x, f) = match (
        run_side("X", x_out, inputs, x_map),
        run_side("F", f_out, inputs, f_map),
    ) {
        (Ok(x), Ok(f)) => (x, f),
        (x, f) => {
            eq.x = x.map_or_else(|r| r, |s| s.report);
            eq.f = f.map_or_else(|r| r, |s| s.report);
            eq.findings.push(finding(1, "a side's output cannot be read: nothing to compare"));
            return eq;
        }
    };
    let mut out = Vec::new();
    headers(&x, &f, &mut out);
    units(&x, &f, &mut out, &mut eq);
    symbols(&x, &f, &mut out, &mut eq);
    records(&x, &f, &mut out, &mut eq);
    tables(&x, &f, &mut out, &mut eq);
    packed(&x, &f, &mut out);
    rest(&x, &f, &mut out);
    out.sort_by(|a, b| {
        (a.addr.is_none(), a.addr, a.rule, &a.site, &a.message).cmp(&(
            b.addr.is_none(),
            b.addr,
            b.rule,
            &b.site,
            &b.message,
        ))
    });
    out.dedup();
    eq.findings = out;
    eq.x = x.report;
    eq.f = f.report;
    eq
}

/// P-E1: sections and segments.
fn headers(x: &Side<'_>, f: &Side<'_>, out: &mut Vec<Finding>) {
    let (hx, hf) = (&x.image.header, &f.image.header);
    if hx.class != hf.class
        || hx.ei_osabi != hf.ei_osabi
        || hx.e_type != hf.e_type
        || hx.e_machine != hf.e_machine
        || hx.e_flags != hf.e_flags
    {
        out.push(finding(1, "the file headers differ (class, OS ABI, e_type, e_machine or e_flags)"));
    }
    if !corr(x, hx.e_entry, f, hf.e_entry) {
        let mut g = finding(1, "e_entry names another place").at(hx.e_entry);
        g.expected = Some(hf.e_entry);
        g.found = Some(hx.e_entry);
        out.push(g);
    }
    let (sx, sf) = (&x.image.sections, &f.image.sections);
    if sx.len() != sf.len() {
        out.push(finding(1, format!("{} sections, the full link has {}", sx.len(), sf.len())));
    }
    let name = |s: &[crate::image::OutSection], i: u32| {
        s.get(i as usize).map_or_else(|| format!("#{i}"), |s| s.name.clone())
    };
    for (i, (a, b)) in sx.iter().zip(sf.iter()).enumerate() {
        let (p, q) = (&a.shdr, &b.shdr);
        if a.name != b.name
            || p.sh_type != q.sh_type
            || p.sh_flags != q.sh_flags
            || p.sh_entsize != q.sh_entsize
            || name(sx, p.sh_link) != name(sf, q.sh_link)
            || (p.sh_flags & mortar_elf::consts::SHF_INFO_LINK != 0
                && name(sx, p.sh_info) != name(sf, q.sh_info))
        {
            out.push(finding(
                1,
                format!("section {i} is '{}' here and '{}' in the full link, or its type, flags, entsize or links differ", a.name, b.name),
            ));
        }
    }
    let (px, pf) = (&x.image.phdrs, &f.image.phdrs);
    if px.len() != pf.len() {
        out.push(finding(1, format!("{} program headers, the full link has {}", px.len(), pf.len())));
    }
    let inside = |s: &Side<'_>, p: &crate::image::Phdr| -> Vec<String> {
        s.image
            .sections
            .iter()
            .filter(|c| {
                c.alloc()
                    && c.shdr.sh_size > 0
                    && (c.shdr.sh_flags & mortar_elf::consts::SHF_TLS != 0) == (p.p_type == PT_TLS)
                    && p.p_memsz > 0
                    && p.p_vaddr <= c.shdr.sh_addr
                    && c.shdr.sh_addr < p.p_vaddr.saturating_add(p.p_memsz)
            })
            .map(|c| c.name.clone())
            .collect()
    };
    for (i, (a, b)) in px.iter().zip(pf.iter()).enumerate() {
        if a.p_type != b.p_type || a.p_flags != b.p_flags || a.p_align != b.p_align {
            out.push(finding(1, format!("program header {i}'s type, flags or alignment differ")));
        } else if inside(x, a) != inside(f, b) {
            out.push(finding(1, format!("program header {i} covers other sections")));
        }
    }
}

/// P-E2: the same live units, each placed once.
fn units(x: &Side<'_>, f: &Side<'_>, out: &mut Vec<Finding>, _eq: &mut Equiv) {
    // (unit identity, input offset) -> (size, is an FDE piece, line)
    type Set = BTreeMap<(String, String, Option<u64>), (u64, bool, bool)>;
    let set = |s: &Side<'_>, out: &mut Vec<Finding>| -> Set {
        let mut m = Set::new();
        for p in &s.map.placed {
            let eh = s.image.alloc_section_of(p.addr, 0).is_some_and(|(_, o)| o.name == ".eh_frame");
            let cie = eh
                && p.input_offset.is_some()
                && s.image.value(p.addr.saturating_add(4), 4) == Some(0);
            if cie {
                continue;
            }
            let k = (file_key(s.map, p.file), sec_key(&p.section), p.input_offset);
            if m.insert(k.clone(), (p.size, eh, false)).is_some() && p.input_offset.is_some() {
                out.push(finding(2, format!("{}: the piece {}:{} at {:#x} is placed twice", s.tag, k.0, k.1, p.input_offset.unwrap_or(0))).at(p.addr));
            }
        }
        for n in &s.map.nonalloc {
            let k = (file_key(s.map, n.file), format!("{}>{}", sec_key(&n.section), n.out), n.offset.map(|_| 0));
            m.insert(k, (n.size, false, true));
        }
        m
    };
    let (mx, mf) = (set(x, out), set(f, out));
    for (k, (zx, eh, _)) in &mx {
        match mf.get(k) {
            None => out.push(finding(2, format!("{}:{} is placed in the update, not in the full link", k.0, k.1))),
            Some((zf, _, _)) if zx != zf && !(*eh && zx > zf && matches!(zx - zf, 4 | 8 | 12)) => {
                out.push(finding(2, format!("{}:{} has size {zx:#x}, {zf:#x} in the full link", k.0, k.1)));
            }
            Some(_) => {}
        }
    }
    for k in mf.keys() {
        if !mx.contains_key(k) {
            out.push(finding(2, format!("{}:{} is placed in the full link, not in the update", k.0, k.1)));
        }
    }
    let chunks = |s: &Side<'_>| -> BTreeSet<(String, String, u32)> {
        let mut nth: BTreeMap<(ChunkKind, String), u32> = BTreeMap::new();
        let mut set = BTreeSet::new();
        for c in &s.map.chunks {
            if matches!(c.kind, ChunkKind::Thunk | ChunkKind::Patch) {
                continue;
            }
            let k = nth.entry((c.kind, c.name.clone())).or_default();
            *k += 1;
            set.insert((c.kind.name().to_string(), c.name.clone(), *k - 1));
        }
        set
    };
    for (a, b, which) in [(chunks(x), chunks(f), "the update"), (chunks(f), chunks(x), "the full link")] {
        for c in a.difference(&b) {
            out.push(finding(2, format!("the {} chunk '{}' ({}) is only in {which}", c.0, c.1, c.2)));
        }
    }
    let folds = |s: &Side<'_>| -> BTreeSet<String> {
        s.map
            .folds
            .iter()
            .map(|d| {
                format!(
                    "{}:{}>{}:{}",
                    file_key(s.map, d.file),
                    sec_key(&d.section),
                    file_key(s.map, d.into_file),
                    sec_key(&d.into_section)
                )
            })
            .collect()
    };
    for d in folds(x).symmetric_difference(&folds(f)) {
        out.push(finding(2, format!("the fold {d} is in one link only")));
    }
}

fn bind_global(b: u8) -> bool {
    matches!(b, STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE)
}

/// `st_shndx`'s kind: undefined, absolute, common, or a section by name.
fn shndx_kind(s: &Side<'_>, n: u16) -> String {
    match n {
        SHN_UNDEF => "undef".into(),
        SHN_ABS => "abs".into(),
        SHN_COMMON => "common".into(),
        k if k >= 0xff00 => format!("reserved {k:#x}"),
        k => s.image.sections.get(usize::from(k)).map_or_else(|| format!("#{k}"), |c| c.name.clone()),
    }
}

/// The globals of `.symtab` by name; a name twice is P-E3.
fn globals<'s>(s: &'s Side<'_>, out: &mut Vec<Finding>) -> BTreeMap<&'s str, &'s crate::image::OutSym> {
    let mut m: BTreeMap<&str, &crate::image::OutSym> = BTreeMap::new();
    for y in s.image.symbols.iter().skip(1) {
        if bind_global(y.sym.binding()) && m.insert(y.name.as_str(), y).is_some() {
            out.push(finding(3, format!("{}: the global '{}' is defined twice in .symtab", s.tag, y.name)));
        }
    }
    m
}

/// P-E3: symbols.
fn symbols(x: &Side<'_>, f: &Side<'_>, out: &mut Vec<Finding>, eq: &mut Equiv) {
    let (gx, gf) = (globals(x, out), globals(f, out));
    for (name, a) in &gx {
        let Some(b) = gf.get(name) else {
            out.push(finding(3, format!("the global '{name}' is only in the update")));
            continue;
        };
        eq.symbols += 1;
        let (p, q) = (&a.sym, &b.sym);
        if p.st_info != q.st_info
            || p.st_other != q.st_other
            || p.st_size != q.st_size
            || shndx_kind(x, p.st_shndx) != shndx_kind(f, q.st_shndx)
        {
            out.push(finding(3, format!("the global '{name}' differs in type, binding, visibility, size or section")));
            continue;
        }
        if !value_same(x, f, name, p, q) {
            let mut g = finding(3, format!("the global '{name}' names another definition or place"));
            g.symbol = Some((*name).to_string());
            g.expected = Some(q.st_value);
            g.found = Some(p.st_value);
            out.push(g);
        }
    }
    for name in gf.keys() {
        if !gx.contains_key(name) {
            out.push(finding(3, format!("the global '{name}' is only in the full link")));
        }
    }
    // Locals: a multiset with corresponding values.
    type Key = (String, u8, u8, u64, String);
    let locals = |s: &Side<'_>| {
        let mut m: BTreeMap<Key, Vec<u64>> = BTreeMap::new();
        for y in s.image.symbols.iter().skip(1) {
            // A per-file CIE's mapping symbol (IN-34a) is in one link only.
            if bind_global(y.sym.binding()) || shndx_kind(s, y.sym.st_shndx) == ".eh_frame" {
                continue;
            }
            let k = (y.name.clone(), y.sym.st_info, y.sym.st_other, y.sym.st_size, shndx_kind(s, y.sym.st_shndx));
            m.entry(k).or_default().push(y.sym.st_value);
        }
        m
    };
    let (lx, lf) = (locals(x), locals(f));
    for (k, vx) in &lx {
        let empty = Vec::new();
        let vf = lf.get(k).unwrap_or(&empty);
        if vx.len() != vf.len() {
            out.push(finding(3, format!("{} local symbols '{}', {} in the full link", vx.len(), k.0, vf.len())));
            continue;
        }
        eq.symbols += vx.len() as u64;
        let ty = k.1 & 0xf;
        if ty == STT_SECTION || ty == STT_FILE {
            continue;
        }
        let tls = |s: &Side<'_>, v: u64| if ty == STT_TLS { s.tls.map_or(v, |t| t + v) } else { v };
        let mut used = vec![false; vf.len()];
        // By first identity, then the rest by correspondence.
        let mut by: BTreeMap<Option<(String, u64)>, Vec<usize>> = BTreeMap::new();
        for (j, &b) in vf.iter().enumerate() {
            by.entry(f.canon(tls(f, b)).or(Some((String::new(), tls(f, b))))).or_default().push(j);
        }
        let mut rest = Vec::new();
        for &a in vx {
            let c = x.canon(tls(x, a)).or(Some((String::new(), tls(x, a))));
            match by.get_mut(&c).and_then(Vec::pop) {
                Some(j) => used[j] = true,
                None => rest.push(a),
            }
        }
        for a in rest {
            let hit = (0..vf.len()).find(|&j| !used[j] && corr(x, tls(x, a), f, tls(f, vf[j])));
            match hit {
                Some(j) => used[j] = true,
                None => {
                    let mut g = finding(3, format!("the local '{}' has a value no local of the full link corresponds to", k.0)).at(a);
                    g.symbol = Some(k.0.clone());
                    out.push(g);
                }
            }
        }
    }
    for (k, v) in &lf {
        if !lx.contains_key(k) {
            out.push(finding(3, format!("{} local symbols '{}' only in the full link", v.len(), k.0)));
        }
    }
    // .dynsym: the same entries in the same order.
    let (dx, df) = (dynsyms(x), dynsyms(f));
    if dx.len() != df.len() {
        out.push(finding(3, format!(".dynsym has {} entries, {} in the full link", dx.len(), df.len())));
    }
    for (i, (a, b)) in dx.iter().zip(df.iter()).enumerate() {
        eq.symbols += 1;
        if a.0 != b.0 || a.1 != b.1 || a.2 != b.2 || a.4 != b.4 || shndx_kind(x, a.5) != shndx_kind(f, b.5) || !corr(x, a.3, f, b.3) {
            out.push(finding(3, format!(".dynsym entry {i} ('{}') differs from the full link's ('{}')", a.0, b.0)));
        }
    }
}

/// Whether a global's two values name the same definition (P-E3).
fn value_same(x: &Side<'_>, f: &Side<'_>, name: &str, p: &mortar_elf::Sym, q: &mortar_elf::Sym) -> bool {
    if p.st_shndx == SHN_ABS || p.st_shndx == SHN_UNDEF {
        return p.st_value == q.st_value;
    }
    let tls = p.sym_type() == STT_TLS;
    let at = |s: &Side<'_>, v: u64| if tls { s.tls.map_or(v, |t| t + v) } else { v };
    let (vx, vf) = (at(x, p.st_value), at(f, q.st_value));
    let placed: Vec<&UKey> = x
        .defs
        .get(name)
        .into_iter()
        .flatten()
        .filter_map(|(u, _)| u.as_ref())
        .filter(|u| x.units.contains_key(*u))
        .collect();
    if placed.is_empty() {
        return corr(x, vx, f, vf);
    }
    x.defs.get(name).into_iter().flatten().any(|(u, val)| {
        u.as_ref().is_some_and(|u| {
            x.def_addr(u, *val) == Some(vx)
                && f.defs.get(name).into_iter().flatten().any(|(w, wv)| {
                    w.as_ref() == Some(u) && *wv == *val && f.def_addr(u, *val) == Some(vf)
                })
        })
    })
}

/// `.dynsym`'s entries: (name, info, other, value, size, shndx).
fn dynsyms(s: &Side<'_>) -> Vec<(String, u8, u8, u64, u64, u16)> {
    let (Some(d), Some(st)) = (s.sec_bytes(".dynsym"), s.sec_bytes(".dynstr")) else {
        return Vec::new();
    };
    let w64 = s.word() == 8;
    let ent = if w64 { 24 } else { 16 };
    let mut v = Vec::new();
    for i in 0..d.len() / ent {
        let o = i * ent;
        let (name, info, other, shndx, value, size) = if w64 {
            (le(d, o, 4), d.get(o + 4), d.get(o + 5), le(d, o + 6, 2), le(d, o + 8, 8), le(d, o + 16, 8))
        } else {
            (le(d, o, 4), d.get(o + 12), d.get(o + 13), le(d, o + 14, 2), le(d, o + 4, 4), le(d, o + 8, 4))
        };
        let n = name.and_then(|n| cstr(st, n as usize)).unwrap_or_default();
        v.push((n, *info.unwrap_or(&0), *other.unwrap_or(&0), value.unwrap_or(0), size.unwrap_or(0), shndx.unwrap_or(0) as u16));
    }
    v
}

fn cstr(b: &[u8], o: usize) -> Option<String> {
    let s = b.get(o..)?;
    let n = s.iter().position(|&c| c == 0)?;
    Some(String::from_utf8_lossy(&s[..n]).into_owned())
}

/// Whether two forms of one record may differ (P-E4 2).
fn may_differ(a: Form, b: Form, dyn_out: bool) -> bool {
    let ok = |f: Form| match f {
        Form::Plain => true,
        Form::Rewrite(r) => match r {
            Rewrite::AdrpAddToAdr
            | Rewrite::GotToAdrpAdd
            | Rewrite::GotToAdr
            | Rewrite::Veneer
            | Rewrite::Erratum843419Adr
            | Rewrite::Erratum843419Patch => true,
            Rewrite::GotToLea | Rewrite::GotToDirect | Rewrite::GotToImm | Rewrite::GotToMovImm => {
                dyn_out
            }
            _ => false,
        },
        _ => false,
    };
    ok(a) && ok(b)
}

fn form_name(f: Form) -> String {
    match f {
        Form::Plain => "verified".into(),
        Form::Rewrite(r) => format!("accepted as {}", r.id()),
        Form::NotChecked(r) => format!("not checked ({})", r.id()),
        Form::Failed => "failed".into(),
    }
}

/// P-E4: relocated fields by value, and every other byte.
fn records(x: &Side<'_>, f: &Side<'_>, out: &mut Vec<Finding>, eq: &mut Equiv) {
    let dyn_out = x.image.header.e_type == ET_DYN && f.image.header.e_type == ET_DYN;
    let x86 = !matches!(x.machine, Some(Machine::AArch64) | None);
    let site = |u: &UKey, off: u64| crate::report::Site {
        file: u.0.0.clone(),
        member: u.0.1.clone(),
        section: format!("#{}", u.1),
        offset: off,
    };
    // The forms of each record of a unit placed in both.
    let mut windows: BTreeMap<&UKey, Vec<u64>> = BTreeMap::new();
    for ((u, off), a) in &x.forms {
        let Some(b) = f.forms.get(&(u.clone(), *off)) else { continue };
        eq.records += 1;
        if *a == Form::Failed || *b == Form::Failed {
            continue; // the side's own finding says it
        }
        if matches!(a, Form::Rewrite(_)) || matches!(b, Form::Rewrite(_)) {
            windows.entry(u).or_default().push(*off);
        }
        let bad = match (a, b) {
            (Form::NotChecked(_), _) | (_, Form::NotChecked(_)) => true,
            _ => a != b && !may_differ(*a, *b, dyn_out),
        };
        if bad {
            out.push(
                finding(4, format!("the record is {} in the update and {} in the full link", form_name(*a), form_name(*b)))
                    .site(site(u, *off)),
            );
        }
    }
    // Bytes outside relocated ranges.
    for (u, ux) in &x.units {
        let Some(uf) = f.units.get(u) else { continue };
        if ux.nobits || ux.tbss || uf.nobits {
            continue;
        }
        let bytes = |s: &Side<'_>, unit: &Unit, a: u64, z: u64| -> Option<Vec<u8>> {
            if unit.alloc {
                s.image.bytes(a, z).map(<[u8]>::to_vec)
            } else {
                s.image.nonalloc_bytes(unit.out?, a, z).map(<[u8]>::to_vec)
            }
        };
        let win = windows.get(u);
        for (io, ax, zx) in &ux.pieces {
            let Some((_, af, zf)) = uf.pieces.iter().find(|p| p.0 == *io) else { continue };
            let mut len = (*zx).min(*zf);
            let mut skip: Vec<(u64, u64)> = Vec::new();
            if ux.eh {
                // The input record's length; the length word and CIE pointer apart.
                len = len.min(ux.eh_len.get(io).copied().unwrap_or(len));
                skip.push((0, 8));
            }
            let (Some(bx), Some(bf)) = (bytes(x, ux, *ax, len), bytes(f, uf, *af, len)) else {
                if len > 0 {
                    out.push(finding(4, "a unit's bytes cannot be read on one side").site(site(u, *io)));
                }
                continue;
            };
            eq.bytes += len;
            let masked = |k: u64| -> bool {
                let at = io + k;
                if skip.iter().any(|(o, n)| *o <= k && k < o + n) {
                    return true;
                }
                // The relocated ranges (sorted by offset).
                let i = ux.relocs.partition_point(|r| r.0 <= at);
                if ux.relocs[..i].iter().rev().take(4).any(|(o, w)| at < o + w) {
                    return true;
                }
                if x86 && win.is_some_and(|w| w.iter().any(|o| *o <= at + 3 && at < o + 4)) {
                    return true;
                }
                if !x86 {
                    let wx = (ax + k) & !3;
                    let wf = (af + k) & !3;
                    return x.rewritten.contains(&wx)
                        || f.rewritten.contains(&wf)
                        || a64::erratum_843419_site(wx)
                        || a64::erratum_843419_site(wf)
                        || a64::erratum_843419_adrp(wx)
                        || a64::erratum_843419_adrp(wf);
                }
                x.rewritten.contains(&(ax + k)) || f.rewritten.contains(&(af + k))
            };
            if let Some(k) = (0..len).find(|&k| bx[k as usize] != bf[k as usize] && !masked(k)) {
                let mut g = finding(4, format!("a byte no relocation covers differs at offset {:#x}", io + k))
                    .at(ax + k)
                    .site(site(u, io + k));
                g.expected = Some(u64::from(bf[k as usize]));
                g.found = Some(u64::from(bx[k as usize]));
                out.push(g);
            }
            if ux.eh && zx > &len {
                // Absorbed bytes: DW_CFA_nop.
                if bytes(x, ux, ax + len, zx - len).is_some_and(|b| b.iter().any(|&c| c != 0)) {
                    out.push(finding(4, "an FDE's absorbed bytes are not DW_CFA_nop").at(ax + len).site(site(u, *io)));
                }
            }
        }
        let _ = (ux.merge, ux.size);
    }
}

/// An item of a table: its exact part and its address parts.
type Item = (String, Vec<u64>);

/// Matches two multisets of items whose address parts correspond
/// (P-E5): returns the unmatched items of each side.
fn match_items(x: &Side<'_>, xs: Vec<Item>, f: &Side<'_>, fs: Vec<Item>) -> (Vec<Item>, Vec<Item>) {
    let mut by: BTreeMap<String, Vec<Vec<u64>>> = BTreeMap::new();
    for (k, a) in fs {
        by.entry(k).or_default().push(a);
    }
    let mut left = Vec::new();
    for (k, a) in xs {
        let cands = by.get_mut(&k);
        let hit = cands.as_ref().and_then(|c| {
            c.iter().position(|b| b.len() == a.len() && a.iter().zip(b).all(|(p, q)| corr(x, *p, f, *q)))
        });
        match (cands, hit) {
            (Some(c), Some(j)) => {
                c.swap_remove(j);
            }
            _ => left.push((k, a)),
        }
    }
    let rest = by.into_iter().flat_map(|(k, v)| v.into_iter().map(move |a| (k.clone(), a))).collect();
    (left, rest)
}

fn in_holes(holes: &[(u64, u64)], o: u64, n: u64) -> bool {
    holes.iter().any(|(h, z)| *h <= o && o.saturating_add(n) <= h + z)
}

/// Whether `v` lies in an allocated section (an address word).
fn is_addr(s: &Side<'_>, v: u64) -> bool {
    v != 0 && s.image.alloc_section_of(v, 0).is_some()
}

fn report_left(n: u8, what: &str, left: (Vec<Item>, Vec<Item>), out: &mut Vec<Finding>) {
    for (k, a) in left.0 {
        out.push(finding(n, format!("{what} {k} {a:x?} of the update has no match in the full link")));
    }
    for (k, a) in left.1 {
        out.push(finding(n, format!("{what} {k} {a:x?} of the full link has no match in the update")));
    }
}

/// P-E5: synthetic tables.
fn tables(x: &Side<'_>, f: &Side<'_>, out: &mut Vec<Finding>, eq: &mut Equiv) {
    let w = x.word();
    // The GOT's words.
    for name in [".got", ".got.plt", ".igot", ".igot.plt"] {
        let words = |s: &Side<'_>, holes: &[(u64, u64)]| -> Vec<Item> {
            let Some(b) = s.sec_bytes(name) else { return Vec::new() };
            let mut v = Vec::new();
            for i in 0..(b.len() as u64) / w {
                let o = i * w;
                if in_holes(holes, o, w) {
                    continue;
                }
                let Some(val) = le(b, o as usize, w as usize) else { continue };
                let reserved = name == ".got.plt" && i < 3;
                if reserved {
                    v.push((format!("reserved {i}"), vec![val]));
                } else if is_addr(s, val) {
                    v.push(("address".into(), vec![val]));
                } else {
                    v.push((format!("value {val:#x}"), Vec::new()));
                }
            }
            v
        };
        let (xs, fs) = (words(x, &x.holes(name)), words(f, &[]));
        eq.entries += xs.len() as u64;
        report_left(5, &format!("the {name} word"), match_items(x, xs, f, fs), out);
    }
    // Relocation tables.
    let dynsym_x = dynsyms(x);
    let dynsym_f = dynsyms(f);
    for name in [".rela.dyn", ".rel.dyn", ".rela.plt", ".rel.plt", ".rela.iplt", ".rel.iplt"] {
        let rela = name.starts_with(".rela");
        let recs = |s: &Side<'_>, syms: &[(String, u8, u8, u64, u64, u16)], holes: &[(u64, u64)]| -> Vec<Item> {
            let Some(b) = s.sec_bytes(name) else { return Vec::new() };
            let ent = if rela { 3 * w } else { 2 * w };
            let mut v = Vec::new();
            for i in 0..(b.len() as u64) / ent {
                let o = i * ent;
                if in_holes(holes, o, ent) {
                    continue;
                }
                let r_off = le(b, o as usize, w as usize).unwrap_or(0);
                let info = le(b, (o + w) as usize, w as usize).unwrap_or(0);
                let (ty, sym) = if w == 8 { (info & 0xffff_ffff, info >> 32) } else { (info & 0xff, info >> 8) };
                let sname = syms.get(sym as usize).map_or("", |e| e.0.as_str());
                let osec = s.image.alloc_section_of(r_off, 0).map_or("", |(_, c)| c.name.as_str());
                let mut key = format!("type {ty} '{sname}' in {osec}");
                let mut addrs = vec![r_off];
                if rela {
                    let a = le(b, (o + 2 * w) as usize, w as usize).unwrap_or(0);
                    if is_addr(s, a) {
                        addrs.push(a);
                    } else {
                        key.push_str(&format!(" addend {a:#x}"));
                    }
                }
                v.push((key, addrs));
            }
            v
        };
        let (xs, fs) = (recs(x, &dynsym_x, &x.holes(name)), recs(f, &dynsym_f, &[]));
        eq.entries += xs.len() as u64;
        if name.contains("plt") {
            // In the same order (the IRELATIVEs).
            if xs.len() != fs.len()
                || xs.iter().zip(&fs).any(|(a, b)| a.0 != b.0 || a.1.iter().zip(&b.1).any(|(p, q)| !corr(x, *p, f, *q)))
            {
                out.push(finding(5, format!("{name}'s records differ from the full link's, or their order does")));
            }
        } else {
            report_left(5, &format!("the {name} record"), match_items(x, xs, f, fs), out);
        }
    }
    // .eh_frame_hdr: a set of (location, FDE).
    let hdr = |s: &Side<'_>| -> Option<Vec<Item>> {
        let i = s.sec(".eh_frame_hdr")?;
        let b = s.image.section_data(i)?;
        let base = s.image.sections[i].shdr.sh_addr;
        if b.len() < 12 || b[0] != 1 || b[2] != 0x03 || b[3] != 0x3b {
            return None;
        }
        let n = le(b, 8, 4)?;
        let mut v = Vec::new();
        for k in 0..n {
            let o = 12 + 8 * k as usize;
            let loc = le(b, o, 4)? as u32 as i32;
            let fde = le(b, o + 4, 4)? as u32 as i32;
            v.push((String::new(), vec![base.wrapping_add(loc as i64 as u64), base.wrapping_add(fde as i64 as u64)]));
        }
        Some(v)
    };
    match (hdr(x), hdr(f)) {
        (Some(a), Some(b)) => {
            eq.entries += a.len() as u64;
            report_left(5, "the .eh_frame_hdr entry", match_items(x, a, f, b), out);
        }
        (None, None) => {}
        _ => out.push(finding(5, ".eh_frame_hdr's table cannot be read on one side, or is on one side only")),
    }
    // Notes, in order.
    for (i, c) in x.image.sections.iter().enumerate() {
        // A non-allocated note section (`.note.stapsdt`: addresses in
        // its descriptors) is made of units, compared by P-E4.
        if c.shdr.sh_type != SHT_NOTE || !c.alloc() {
            continue;
        }
        let a = x.image.section_data(i).map(notes);
        let b = f.sec(&c.name).and_then(|j| f.image.section_data(j)).map(notes);
        if a != b {
            out.push(finding(5, format!("the notes of {} differ from the full link's", c.name)));
        }
    }
    dynamic(x, f, out, eq);
    for name in [".dynstr", ".hash", ".gnu.hash", ".gnu.version", ".gnu.version_r", ".gnu.version_d"] {
        if x.sec_bytes(name) != f.sec_bytes(name) {
            out.push(finding(5, format!("{name} differs from the full link's")));
        }
    }
}

/// A note section's notes: (name, type, descriptor), a build id's by its
/// length only.
fn notes(b: &[u8]) -> Vec<(Vec<u8>, u64, Vec<u8>)> {
    let mut v = Vec::new();
    let mut o = 0usize;
    let up = |n: usize| n.div_ceil(4) * 4;
    while o + 12 <= b.len() {
        let (Some(nz), Some(dz), Some(ty)) = (le(b, o, 4), le(b, o + 4, 4), le(b, o + 8, 4)) else { break };
        let (nz, dz) = (nz as usize, dz as usize);
        let name = b.get(o + 12..o + 12 + nz).map(<[u8]>::to_vec).unwrap_or_default();
        let d0 = o + 12 + up(nz);
        let mut desc = b.get(d0..d0 + dz).map(<[u8]>::to_vec).unwrap_or_default();
        if name.as_slice() == b"GNU\0" && ty == 3 {
            desc = vec![0; desc.len()];
        }
        v.push((name, ty, desc));
        o = d0 + up(dz);
    }
    v
}

/// P-E5: `.dynamic`, the same tags in the same order.
fn dynamic(x: &Side<'_>, f: &Side<'_>, out: &mut Vec<Finding>, eq: &mut Equiv) {
    let w = x.word();
    let read = |s: &Side<'_>| -> Vec<(u64, u64)> {
        let Some(b) = s.sec_bytes(".dynamic") else { return Vec::new() };
        (0..(b.len() as u64) / (2 * w))
            .map(|i| {
                let o = (i * 2 * w) as usize;
                (le(b, o, w as usize).unwrap_or(0), le(b, o + w as usize, w as usize).unwrap_or(0))
            })
            .collect()
    };
    let (dx, df) = (read(x), read(f));
    let ent = |rela: bool| if rela { 3 * w } else { 2 * w };
    let hole_bytes = |name: &str| x.holes(name).iter().map(|h| h.1).sum::<u64>();
    if dx.len() != df.len() {
        out.push(finding(5, format!(".dynamic has {} entries, {} in the full link", dx.len(), df.len())));
    }
    for (i, (a, b)) in dx.iter().zip(&df).enumerate() {
        eq.entries += 1;
        let ok = a.0 == b.0
            && match a.0 {
                // Address tags.
                3 | 4 | 5 | 6 | 7 | 12 | 13 | 17 | 23 | 25 | 26 | 32 | 0x6fff_fef5 | 0x6fff_fff0
                | 0x6fff_fffc | 0x6fff_fffe => corr(x, a.1, f, b.1),
                // DT_RELASZ, DT_RELSZ.
                8 => a.1 >= b.1 && a.1 - b.1 == hole_bytes(".rela.dyn"),
                18 => a.1 >= b.1 && a.1 - b.1 == hole_bytes(".rel.dyn"),
                // DT_RELACOUNT, DT_RELCOUNT.
                0x6fff_fff9 => a.1 >= b.1 && (a.1 - b.1) * ent(true) <= hole_bytes(".rela.dyn"),
                0x6fff_fffa => a.1 >= b.1 && (a.1 - b.1) * ent(false) <= hole_bytes(".rel.dyn"),
                _ => a.1 == b.1,
            };
        if !ok {
            let mut g = finding(5, format!(".dynamic entry {i} (tag {:#x}) differs from the full link's", a.0));
            g.expected = Some(b.1);
            g.found = Some(a.1);
            out.push(g);
        }
    }
}

/// P-E5: packed order.
fn packed(x: &Side<'_>, f: &Side<'_>, out: &mut Vec<Finding>) {
    let seq = |s: &Side<'_>, c: &crate::image::OutSection| -> Vec<String> {
        let (lo, hi) = (c.shdr.sh_addr, c.shdr.sh_addr.saturating_add(c.shdr.sh_size));
        let eh = c.name == ".eh_frame";
        let mut v: Vec<(u64, String)> = s
            .map
            .placed
            .iter()
            .filter(|p| p.size > 0 && lo <= p.addr && p.addr < hi)
            .filter(|p| !eh || (p.input_offset.is_some() && s.image.value(p.addr + 4, 4) != Some(0)))
            .map(|p| {
                (p.addr, format!("{}:{}@{:?}", file_key(s.map, p.file), sec_key(&p.section), p.input_offset))
            })
            .collect();
        v.sort();
        v.into_iter().map(|p| p.1).collect()
    };
    for c in &x.image.sections {
        if !c.alloc() || !(crate::dead::packed(&c.name, c.shdr.sh_type) || c.name == ".eh_frame") {
            continue;
        }
        let Some(d) = f.image.sections.iter().find(|d| d.name == c.name && d.alloc()) else { continue };
        let (a, b) = (seq(x, c), seq(f, d));
        if a != b {
            let k = a.iter().zip(&b).position(|(p, q)| p != q).unwrap_or(a.len().min(b.len()));
            out.push(
                finding(5, format!("the packed section {}'s units are in another order (first at unit {k})", c.name))
                    .at(c.shdr.sh_addr),
            );
        }
    }
}

/// P-E6: nothing else.
fn rest(x: &Side<'_>, f: &Side<'_>, out: &mut Vec<Finding>) {
    let img = &x.image;
    let len = img.data.len() as u64;
    let mut used: Vec<(u64, u64)> = vec![(0, u64::from(img.header.e_ehsize))];
    let h = &img.header;
    used.push((h.e_phoff, u64::from(h.e_phnum) * u64::from(h.e_phentsize)));
    used.push((h.e_shoff, u64::from(h.e_shnum) * u64::from(h.e_shentsize)));
    for c in &img.sections {
        if c.shdr.sh_type != SHT_NOBITS {
            used.push((c.shdr.sh_offset, c.shdr.sh_size));
        }
    }
    used.sort_unstable();
    let mut at = 0u64;
    for (o, z) in used {
        if o > at && !img.data[at as usize..o.min(len) as usize].iter().all(|&b| b == 0) {
            out.push(finding(6, format!("file bytes {at:#x}..{o:#x}, outside every section and header, are not zero")));
        }
        at = at.max(o.saturating_add(z)).min(len);
    }
    if at < len && !img.data[at as usize..].iter().all(|&b| b == 0) {
        out.push(finding(6, format!("file bytes from {at:#x}, after every section, are not zero")));
    }
    // Non-alloc sections.
    for (i, c) in img.sections.iter().enumerate().skip(1) {
        if c.alloc() || c.shdr.sh_flags & SHF_ALLOC != 0 || c.shdr.sh_type == SHT_NOBITS {
            continue;
        }
        if matches!(c.name.as_str(), ".symtab" | ".strtab" | ".shstrtab" | ".symtab_shndx") {
            continue;
        }
        let Some(b) = img.section_data(i) else { continue };
        let units: Vec<(u64, u64)> = x
            .map
            .nonalloc
            .iter()
            .filter(|n| n.out == c.name)
            .filter_map(|n| Some((n.offset?, n.size)))
            .chain(x.map.dead.iter().filter(|d| d.section == c.name).map(|d| (d.offset, d.size)))
            .collect();
        if units.is_empty() {
            if c.shdr.sh_flags & (SHF_MERGE | SHF_STRINGS) == (SHF_MERGE | SHF_STRINGS) {
                continue;
            }
            if x.map.nonalloc.iter().any(|n| n.out == c.name) {
                continue;
            }
            if f.sec_bytes(&c.name) != Some(b) {
                out.push(finding(6, format!("{}, which no unit holds, differs from the full link's", c.name)));
            }
            continue;
        }
        let mut u = units;
        u.sort_unstable();
        let mut at = 0u64;
        for (o, z) in u {
            if o > at && b.get(at as usize..o as usize).is_some_and(|s| s.iter().any(|&v| v != 0)) {
                out.push(finding(6, format!("{} bytes {at:#x}..{o:#x} are in no unit or dead range and not zero", c.name)));
            }
            at = at.max(o.saturating_add(z));
        }
        if (at as usize) < b.len() && b[at as usize..].iter().any(|&v| v != 0) {
            out.push(finding(6, format!("{} bytes from {at:#x} are in no unit or dead range and not zero", c.name)));
        }
    }
    let _ = SectionIndex(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    // §6.9 P-E4 2: the forms that may differ, per output type.
    #[test]
    fn p_e4_forms_that_may_differ() {
        let rw = Form::Rewrite;
        for (a, b, dyn_out, ok) in [
            (Form::Plain, rw(Rewrite::AdrpAddToAdr), false, true),
            (rw(Rewrite::GotToAdrpAdd), rw(Rewrite::GotToAdr), false, true),
            (Form::Plain, rw(Rewrite::Veneer), false, true),
            (Form::Plain, rw(Rewrite::Erratum843419Patch), false, true),
            (Form::Plain, rw(Rewrite::Erratum843419Adr), false, true),
            (rw(Rewrite::GotToLea), rw(Rewrite::GotToMovImm), true, true),
            (rw(Rewrite::GotToLea), rw(Rewrite::GotToMovImm), false, false),
            (Form::Plain, rw(Rewrite::GotToDirect), false, false),
            (Form::Plain, rw(Rewrite::GotToImm), true, true),
            (Form::Plain, rw(Rewrite::TlsIeToLe), false, false),
            (rw(Rewrite::TlsDescToIe), rw(Rewrite::TlsDescToLe), true, false),
            (Form::Plain, rw(Rewrite::TlsGdToIe), true, false),
            (Form::Plain, rw(Rewrite::WeakBranchNop), false, false),
            (Form::Plain, Form::NotChecked(Reason::Tls), false, false),
        ] {
            assert_eq!(may_differ(a, b, dyn_out), ok, "{a:?} {b:?} dyn {dyn_out}");
        }
    }
}
