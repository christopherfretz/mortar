//! P-P1, the composition of a `-r` output and its final link
//! (`spec/plumb.md` §14.5 P-P1; V-R1 of record
//! `spec/landed/layout/WP-154.md`). Owner: **RL-6** (WP-186; §14.8 Q-R2's
//! ruling (a)). WP-181 (RL-1) wrote it empty and added its `mod` line.
//!
//! Two final links of one line `L` by one linker are compared: the direct
//! link `D`, and `V`, the same line with the component's inputs replaced
//! by their `-r` output. `φ` maps a place of `D` to one of `V`: an input
//! piece by the piece, through the `-r` map for a component input
//! (`V`'s place of the `-r` output's section `out(S)`, plus `off(S)`,
//! which stage R's model gives: [`crate::relocatable::model`]); a
//! linker-made section to the section of its name. Items 1-4 are
//! compared under the tolerances (t1)-(t4); the exclusions (a)-(d) make
//! the report say `not judged: relocatable composition ({letter})`.
//!
//! Written from §14.5's text and from measured outputs; no linker crate
//! was read. Each side is also verified by stages 1-3 by the caller;
//! P-P1 reports only its own comparisons (its findings are `P-P1`).
//! Never panics.

// d, v, r: the direct link, the link through -r, the -r output, as §14.5
// names them; f, s, o: file, section, output section.
#![allow(clippy::many_single_char_names)]
#![allow(clippy::too_many_lines)] // each item reads as one clause of P-P1

use std::collections::{BTreeMap, BTreeSet};

use mortar_elf::consts::{
    SHF_ALLOC, SHF_MERGE, SHF_STRINGS, SHN_ABS, SHN_COMMON, SHN_UNDEF, SHT_DYNSYM, SHT_GROUP,
    SHT_NOBITS, STB_GLOBAL, STB_GNU_UNIQUE, STB_LOCAL, STB_WEAK, STT_TLS,
};
use mortar_elf::{Machine, SectionTable};

use crate::a64::{Field, Op};
use crate::equiv::{Form, Trace};
use crate::image::Image;
use crate::locate::{Obj, Place};
use crate::map::{Format, Map};
use crate::reloc::Engine;
use crate::relocatable as rel;
use crate::report::{CompCoverage, Finding, Report, Rule};
use crate::store::Store;
use crate::{Source, opts};

/// One final link, or the `-r` link: the output's bytes, its inputs, its
/// map, and a reference's line (mortar's is its map's `arg` records).
#[allow(missing_debug_implementations)]
pub struct Link<'a> {
    pub output: &'a [u8],
    pub src: &'a dyn Source,
    pub map: &'a Map,
    /// The linker's words, for a reference's map (§14.1 "The line").
    pub line: Option<Vec<String>>,
}

/// The `-r` link of the component: its output, inputs, map and line, and
/// the path by which `V`'s map names the `-r` output.
#[allow(missing_debug_implementations)]
pub struct Partial<'a> {
    pub link: Link<'a>,
    pub path: &'a str,
}

fn p1(msg: impl Into<String>) -> Finding {
    Finding::new(Rule::Rel(1), msg)
}

/// The words of a link's line: mortar's `arg` records, else the caller's.
fn words(l: &Link<'_>) -> Option<Vec<String>> {
    if l.map.format == Format::Mortar {
        (!l.map.args.is_empty())
            .then(|| l.map.args.iter().map(|w| String::from_utf8_lossy(w).into_owned()).collect())
    } else {
        l.line.clone()
    }
}

// ---- the -r model (stage R's) ----

/// An input across the links: path, member, and how many earlier inputs
/// of one link had both.
type Key = (String, Option<String>, u32);

/// What P-P1 reads of the `-r` link.
#[derive(Debug, Default)]
struct RModel {
    /// The `-r`'s inputs in `FileId` order.
    ins: Vec<Key>,
    places: Vec<Vec<rel::Place>>,
    frames: BTreeMap<(usize, usize), BTreeMap<u64, Option<u64>>>,
    out_eh: Option<u32>,
    /// An input record `(file, relocation section, index)` to its output
    /// record: (target output section, offset) (P-P3's matching).
    recs: BTreeMap<(usize, usize, usize), (u32, u64)>,
    /// The pieces of each output section, `(file, section, offset)`.
    pieces: BTreeMap<u32, Vec<(usize, usize, u64)>>,
    /// Each input's global definitions and the names its records name.
    defines: BTreeSet<String>,
    refs: BTreeSet<String>,
    /// Inputs that are archive members.
    members: BTreeSet<(String, String)>,
    /// Groups of the inputs: (file, member section) to its COMDAT signature.
    comdat: BTreeMap<(usize, usize), String>,
}

fn keys_of<'p>(it: impl Iterator<Item = (&'p str, Option<&'p str>)>) -> Vec<Key> {
    let mut seen: BTreeMap<(String, Option<String>), u32> = BTreeMap::new();
    it.map(|(p, m)| {
        let n = seen.entry((p.to_string(), m.map(str::to_string))).or_default();
        *n += 1;
        (p.to_string(), m.map(str::to_string), *n - 1)
    })
    .collect()
}

fn read_model(rv: &rel::Rv<'_>) -> RModel {
    let mut m = RModel {
        ins: keys_of(rv.ins.iter().map(|i| (i.path.as_str(), i.member.as_deref()))),
        places: rv.places.clone(),
        frames: rv.frames.clone(),
        out_eh: rv.out_eh,
        ..RModel::default()
    };
    for (&(ors, oi), &(f, rs, i)) in &rv.matched {
        let t = rv.out.secs.get(ors).map_or(0, |s| s.info);
        if let Some(r) = rv.out_rels.get(&ors).and_then(|v| v.get(oi)) {
            m.recs.insert((f, rs, i), (t, r.offset));
        }
    }
    for (f, ps) in rv.places.iter().enumerate() {
        for (s, p) in ps.iter().enumerate() {
            if let rel::Place::Whole { out, off } = p {
                m.pieces.entry(*out).or_default().push((f, s, *off));
            }
        }
    }
    for (f, i) in rv.ins.iter().enumerate() {
        if let Some(mem) = &i.member {
            m.members.insert((i.path.clone(), mem.clone()));
        }
        let e = &i.elf;
        for y in &e.syms {
            if !y.local() && y.in_section() {
                m.defines.insert(y.name.clone());
            }
        }
        for (rs, sec) in e.secs.iter().enumerate() {
            if sec.is_reloc() {
                for r in e.rels(rs) {
                    if let Some(y) = e.syms.get(r.sym as usize)
                        && !y.name.is_empty()
                    {
                        m.refs.insert(y.name.clone());
                    }
                }
            }
            if sec.ty == SHT_GROUP && sec.data.len() >= 4 {
                let flag = u32::from_le_bytes([sec.data[0], sec.data[1], sec.data[2], sec.data[3]]);
                let sig = e.syms.get(sec.info as usize).map(|y| y.name.clone()).unwrap_or_default();
                if flag & rel::GRP_COMDAT != 0 {
                    for w in sec.data[4..].chunks_exact(4) {
                        let k = u32::from_le_bytes([w[0], w[1], w[2], w[3]]) as usize;
                        m.comdat.insert((f, k), sig.clone());
                    }
                }
            }
        }
    }
    m
}

// ---- one final link ----

/// An address space: memory, or one non-allocated output section by name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Space {
    Mem,
    Non(String),
}

/// `(start, end, object, section, input offset)`, sorted by start, and
/// the running largest end.
#[derive(Debug, Default)]
struct Spans {
    v: Vec<(u64, u64, usize, usize, u64)>,
    max_end: Vec<u64>,
}

impl Spans {
    fn seal(&mut self) {
        self.v.sort_unstable();
        let mut m = 0;
        self.max_end = self
            .v
            .iter()
            .map(|s| {
                m = m.max(s.1);
                m
            })
            .collect();
    }

    /// The pieces holding `t`: (object, section, offset); those that end
    /// at `t` when none holds it (an end symbol).
    fn ids(&self, t: u64) -> Vec<(usize, usize, u64)> {
        let n = self.v.partition_point(|s| s.0 <= t);
        let mut inside = Vec::new();
        let mut at_end = Vec::new();
        for i in (0..n).rev() {
            if self.max_end[i] < t {
                break;
            }
            let (a, z, o, s, b) = self.v[i];
            if t < z {
                inside.push((o, s, b + (t - a)));
            } else if t == z {
                at_end.push((o, s, b + (t - a)));
            }
        }
        if inside.is_empty() { at_end } else { inside }
    }

    /// Whether a piece holds `t` (not only ends at it).
    fn holds(&self, t: u64) -> bool {
        let n = self.v.partition_point(|s| s.0 <= t);
        (0..n).rev().take_while(|&i| self.max_end[i] >= t).any(|i| t < self.v[i].1)
    }
}

/// One final link, read, with its engine's verdict on each record.
struct Side<'a> {
    tag: &'static str,
    image: Image<'a>,
    objs: Vec<Obj<'a>>,
    keys: Vec<Key>,
    by_key: BTreeMap<Key, usize>,
    /// (object, section) to (offset to the engine's form).
    forms: BTreeMap<(usize, usize), BTreeMap<u64, Form>>,
    spans: BTreeMap<Space, Spans>,
    machine: Machine,
    class64: bool,
}

fn side<'a>(
    tag: &'static str,
    output: &'a [u8],
    store: &'a Store<'a>,
    map: &Map,
    line: Option<&[String]>,
) -> Result<Side<'a>, String> {
    let image = Image::read(output).map_err(|e| format!("the {tag} cannot be read: {e}"))?;
    let located = crate::locate::locate(map, store, 0, &image);
    let mut objs = located.objs;
    let o = opts::of(map, line).or_else(|| {
        line.map(|w| {
            opts::Options::read(&w.iter().map(|x| x.as_bytes().to_vec()).collect::<Vec<_>>())
        })
    });
    let exports = o.as_ref().and_then(|o| crate::fate::static_exports(map, o));
    crate::fate::collect_empty(&mut objs, o.as_ref(), exports.as_ref());
    let mut forms: BTreeMap<(usize, usize), BTreeMap<u64, Form>> = BTreeMap::new();
    {
        let mut engine = Engine::new(&image, &objs, map.format);
        engine.set_wrap(crate::wrap::Wrap::of(o.as_ref()));
        engine.set_defsym(crate::defsym::Defsym::of(o.as_ref()));
        engine.set_keeper_line(line);
        engine.trace = Some(Trace::with_capacity(0));
        crate::run_engine(&mut engine, &image, map, None);
        let recs = engine.trace.take().unwrap_or_default().records;
        for t in recs {
            forms.entry((t.oi as usize, t.si as usize)).or_default().insert(t.off, t.form);
        }
    }
    let keys = keys_of(objs.iter().map(|o| (o.path.as_str(), o.member.as_deref())));
    let by_key = keys.iter().enumerate().map(|(i, k)| (k.clone(), i)).collect();
    let mut spans: BTreeMap<Space, Spans> = BTreeMap::new();
    for (oi, ob) in objs.iter().enumerate() {
        for (si, s) in ob.secs.iter().enumerate() {
            let space = if s.alloc() { Space::Mem } else { Space::Non(s.name.clone()) };
            let sp = spans.entry(space).or_default();
            match &s.place {
                Place::Whole(a) => sp.v.push((*a, a.saturating_add(s.shdr.sh_size), oi, si, 0)),
                Place::Pieces(ps) => {
                    for p in ps {
                        sp.v.push((p.addr, p.addr.saturating_add(p.size), oi, si, p.input_offset));
                    }
                }
                _ => {}
            }
        }
    }
    for sp in spans.values_mut() {
        sp.seal();
    }
    let machine = objs.first().map_or(Machine::AArch64, |o| o.elf.machine());
    let class64 = image.class() == mortar_elf::Class::Elf64;
    Ok(Side { tag, image, objs, keys, by_key, forms, spans, machine, class64 })
}

impl Side<'_> {
    fn space_of(&self, o: usize, s: usize) -> Space {
        let sec = &self.objs[o].secs[s];
        if sec.alloc() { Space::Mem } else { Space::Non(sec.name.clone()) }
    }

    /// The place of offset `x` of input section `(o, s)`.
    fn addr(&self, o: usize, s: usize, x: u64) -> Option<u64> {
        let sec = self.objs.get(o)?.secs.get(s)?;
        match &sec.place {
            Place::Whole(a) => Some(a.wrapping_add(x)),
            Place::Pieces(_) => crate::reloc::place_at(&sec.place, x),
            _ => None,
        }
    }

    fn placed(&self, o: usize, s: usize) -> Option<bool> {
        match self.objs.get(o)?.secs.get(s)?.place {
            Place::Whole(_) | Place::Pieces(_) | Place::Merged | Place::Rewritten => Some(true),
            Place::Absent | Place::Discarded => Some(false),
            Place::Ambiguous => None,
        }
    }

    /// `n` bytes at `a` of a space.
    fn bytes(&self, space: &Space, a: u64, n: u64) -> Option<&[u8]> {
        match space {
            Space::Mem => self.image.bytes(a, n),
            Space::Non(name) => {
                let i = self.image.nonalloc_section(name)?;
                self.image.nonalloc_bytes(i, a, n)
            }
        }
    }

    /// The output section holding a place: its name.
    fn out_name(&self, space: &Space, a: u64, n: u64) -> String {
        match space {
            Space::Mem => self
                .image
                .alloc_section_of(a, n)
                .map_or_else(|| "(no section)".to_string(), |(_, s)| s.name.clone()),
            Space::Non(name) => name.clone(),
        }
    }

    fn label(&self, o: usize, s: usize) -> String {
        let ob = &self.objs[o];
        format!("{}:({})", ob.label(), ob.secs.get(s).map_or("?", |x| x.name.as_str()))
    }

    fn mask(&self, v: u64) -> u64 {
        if self.class64 { v } else { v & 0xffff_ffff }
    }
}

/// A place of `V`: an address of a space, or the content of a merged entry.
#[derive(Debug, Clone, PartialEq, Eq)]
enum VPlace {
    At(Space, u64),
    Content(Vec<u8>),
}

/// What the comparison reads: `D`, `V`, the `-r` model, the input maps.
struct Cx<'a, 'b> {
    d: &'b Side<'a>,
    v: &'b Side<'a>,
    r: &'b RModel,
    /// `D`'s objects that are `-r` inputs: object to `FileId`.
    comp_of: BTreeMap<usize, usize>,
    /// `V`'s object that is the `-r` output.
    rv: Option<usize>,
    /// (t4): `D`'s COMDAT member to `V`'s kept copy, when `V` keeps the
    /// other copy: `(object, section)` of `D` to `(object, section)` of `D`.
    t4: BTreeMap<(usize, usize), (usize, usize)>,
    /// The commons, by key (`φ`: an allocation of a key to the one of the
    /// same key): `D`'s value, size, and `V`'s value.
    commons: Vec<(u64, u64, u64)>,
    cov: CompCoverage,
    out: Vec<Finding>,
}

impl Cx<'_, '_> {
    fn tol(&mut self, t: &str) {
        *self.cov.tolerances.entry(t.to_string()).or_default() += 1;
    }

    /// `φ` of offset `x` of `D`'s input section `(o, s)`: its places in `V`.
    fn phi(&self, o: usize, s: usize, x: u64) -> Vec<VPlace> {
        let (o, s) = self.t4.get(&(o, s)).copied().unwrap_or((o, s));
        let space = self.d.space_of(o, s);
        let Some(&f) = self.comp_of.get(&o) else {
            let Some(&vo) = self.v.by_key.get(&self.d.keys[o]) else { return Vec::new() };
            return self.v.addr(vo, s, x).map(|a| VPlace::At(space, a)).into_iter().collect();
        };
        let Some(vo) = self.rv else { return Vec::new() };
        let vspace = |vs: usize| self.v.space_of(vo, vs);
        match self.r.places.get(f).and_then(|p| p.get(s)) {
            Some(rel::Place::Whole { out, off }) => {
                let vs = *out as usize;
                self.v
                    .addr(vo, vs, off.wrapping_add(x))
                    .map(|a| VPlace::At(vspace(vs), a))
                    .into_iter()
                    .collect()
            }
            Some(rel::Place::Merged { .. }) => {
                // The entry's content: lld's merged strings (§14.4).
                let sec = &self.d.objs[o].secs[s];
                let Some(a) = self.d.addr(o, s, x) else { return Vec::new() };
                let n = if sec.shdr.sh_flags & SHF_STRINGS != 0 {
                    let mut k = 0u64;
                    while self.d.bytes(&space, a + k, 1).is_some_and(|b| b[0] != 0) {
                        k += 1;
                    }
                    k + 1
                } else {
                    sec.shdr.sh_entsize.max(1)
                };
                self.d
                    .bytes(&space, a, n)
                    .map(|b| VPlace::Content(b.to_vec()))
                    .into_iter()
                    .collect()
            }
            Some(rel::Place::Framed) => {
                let (Some(eh), Some(rm)) = (self.r.out_eh, self.r.frames.get(&(f, s))) else {
                    return Vec::new();
                };
                match rm.range(..=x).next_back() {
                    Some((&start, &Some(to))) => {
                        let vs = eh as usize;
                        self.v
                            .addr(vo, vs, to + (x - start))
                            .map(|a| VPlace::At(vspace(vs), a))
                            .into_iter()
                            .collect()
                    }
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    /// `φ` of a place of `D` in a space: every input piece that holds it.
    fn phi_addr(&self, space: &Space, t: u64) -> (bool, Vec<VPlace>) {
        if *space == Space::Mem
            && let Some(&(a, _, b)) = self.commons.iter().find(|c| c.0 <= t && t < c.0 + c.1.max(1))
        {
            return (true, vec![VPlace::At(Space::Mem, b + (t - a))]);
        }
        let Some(sp) = self.d.spans.get(space) else { return (false, Vec::new()) };
        // A place no input piece holds, inside an allocated output
        // section: a linker-made section (an IPLT entry, a slot): the
        // section of its name, at the same offset. Only a place outside
        // every section is an end (`_end`, `__init_array_end`).
        if *space == Space::Mem
            && !sp.holds(t)
            && let Some((_, ds)) = self.d.image.alloc_section_of(t, 1)
            && let Some(vs) = self.v.image.sections.iter().find(|x| x.name == ds.name && x.alloc())
        {
            return (true, vec![VPlace::At(Space::Mem, vs.shdr.sh_addr + (t - ds.shdr.sh_addr))]);
        }
        let ids = sp.ids(t);
        let found = !ids.is_empty();
        let mut out = Vec::new();
        for (o, s, x) in ids {
            for p in self.phi(o, s, x) {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
        (found, out)
    }

    /// Whether `V`'s place `t` of `space` is one of `want`.
    fn hits(&self, want: &[VPlace], space: &Space, t: u64) -> bool {
        want.iter().any(|w| match w {
            VPlace::At(s, a) => {
                (s == space || matches!((s, space), (Space::Non(_), Space::Non(_))))
                    && self.v.mask(*a) == self.v.mask(t)
            }
            VPlace::Content(b) => {
                self.v.bytes(space, t, b.len() as u64).is_some_and(|x| x == b.as_slice())
            }
        })
    }
}

/// `D`'s and `V`'s output sections, item 1.
fn item1(cx: &mut Cx<'_, '_>) {
    let id = |s: &crate::image::OutSection| (s.name.clone(), s.shdr.sh_type, s.shdr.sh_flags);
    let a: Vec<_> = cx.d.image.sections.iter().map(id).collect();
    let b: Vec<_> = cx.v.image.sections.iter().map(id).collect();
    for i in 0..a.len().max(b.len()) {
        let show = |x: Option<&(String, u32, u64)>| {
            x.map_or_else(
                || "no section".to_string(),
                |(n, t, f)| format!("{n} (type {t:#x}, flags {f:#x})"),
            )
        };
        if a.get(i) != b.get(i) {
            cx.out.push(p1(format!(
                "P-P1 the output sections differ at {i}: {} in the direct link, {} through -r",
                show(a.get(i)),
                show(b.get(i))
            )));
            return;
        }
    }
}

/// The relocation records of `D`'s input section `(o, s)` with their
/// windows: `(index, offset, type, width)`.
fn records_of(d: &Side<'_>, o: usize, s: usize) -> Vec<(usize, u64, u32, u8, u32, Option<i64>)> {
    let ob = &d.objs[o];
    let Some(rs) = ob.secs[s].relocs else { return Vec::new() };
    let Some(raw) = ob.records(rs) else { return Vec::new() };
    raw.into_iter()
        .enumerate()
        .map(|(i, (off, sym, ty, a))| {
            (i, off, ty, crate::reloc::width(ob.elf.machine(), ty), sym, a)
        })
        .collect()
}

/// The `V` counterpart of `D`'s record `i` at `off` of `(o, s)`: object,
/// section, offset.
fn v_record(
    cx: &Cx<'_, '_>,
    o: usize,
    s: usize,
    i: usize,
    off: u64,
) -> Option<(usize, usize, u64)> {
    let (o, s) = cx.t4.get(&(o, s)).copied().unwrap_or((o, s));
    match cx.comp_of.get(&o) {
        None => {
            let vo = *cx.v.by_key.get(&cx.d.keys[o])?;
            Some((vo, s, off))
        }
        Some(&f) => {
            let rs = cx.d.objs[o].secs[s].relocs? as usize;
            let (t, at) = cx.r.recs.get(&(f, rs, i))?;
            Some((cx.rv?, *t as usize, *at))
        }
    }
}

/// The relocation definition of a type on the machine.
fn def_of(m: Machine, ty: u32) -> Option<&'static crate::a64::Def> {
    match m {
        Machine::AArch64 => crate::a64::def(ty),
        Machine::X86_64 => crate::x64::def(ty),
        Machine::I386 => crate::x86::def(ty),
    }
}

/// The full place a field names, when its type's field holds all of it:
/// `S + A` (absolute) or `S + A - P` (relative, branch and data).
fn full_target(s: &Side<'_>, space: &Space, p: u64, ty: u32) -> Option<u64> {
    let def = def_of(s.machine, ty)?;
    let full = matches!(
        def.field,
        Field::Data(4 | 8) | Field::Imm26 | Field::Imm19 | Field::Imm14 | Field::Adr
    );
    if !full || !matches!(def.op, Op::Abs | Op::Prel) {
        return None;
    }
    let w = u64::from(def.field.width());
    let b = s.bytes(space, p, w)?;
    let word = b.iter().rev().fold(0u64, |a, &x| a << 8 | u64::from(x));
    let x = crate::a64::decode(def.field, word, p);
    let t = match def.op {
        Op::Prel => p.wrapping_add(x as u64),
        _ => x as u64,
    };
    Some(s.mask(t))
}

/// A record's addend: `r_addend` (RELA), else the input field's (REL;
/// i386's data fields).
fn addend_of(d: &Side<'_>, o: usize, s: usize, off: u64, w: u8, a: Option<i64>) -> i64 {
    a.unwrap_or_else(|| {
        let data = d.objs[o].data(s);
        let b: &[u8] = &data;
        let (Ok(at), w) = (usize::try_from(off), usize::from(w)) else { return 0 };
        match b.get(at..at.saturating_add(w)) {
            Some(x) if w == 4 => i64::from(i32::from_le_bytes([x[0], x[1], x[2], x[3]])),
            Some(x) if w == 2 => i64::from(i16::from_le_bytes([x[0], x[1]])),
            _ => 0,
        }
    })
}

/// The space a record's target lies in: its symbol's section's.
fn target_space(d: &Side<'_>, o: usize, sym: u32) -> Space {
    let ob = &d.objs[o];
    let Some(t) = ob.symtab else { return Space::Mem };
    let Ok(y) = t.get(mortar_elf::SymbolIndex(sym)) else { return Space::Mem };
    match t.section_of(mortar_elf::SymbolIndex(sym), &y) {
        Ok(mortar_elf::SymbolSection::Section(k)) => d.space_of(o, k.0 as usize),
        _ => Space::Mem,
    }
}

fn form_name(f: Option<Form>) -> &'static str {
    match f {
        None => "not judged",
        Some(Form::Failed) => "wrong",
        Some(_) => "verified",
    }
}

/// Items 2 and 3: the pieces, the bytes and the records.
fn items23(cx: &mut Cx<'_, '_>) {
    let d = cx.d;
    let v = cx.v;
    for o in 0..d.objs.len() {
        for s in 0..d.objs[o].secs.len() {
            let sec = &d.objs[o].secs[s];
            if sec.consumed() || s == 0 {
                continue;
            }
            let Some(dp) = d.placed(o, s) else { continue };
            // Item 2: placed in D exactly when in V.
            let (vo, vs) = match cx.comp_of.get(&o) {
                None => match v.by_key.get(&d.keys[o]) {
                    Some(&vo) => (Some(vo), s),
                    None => (None, s),
                },
                Some(&f) => match cx.r.places.get(f).and_then(|p| p.get(s)) {
                    // The -r makes its own section of it (a note, P-P10):
                    // the -r output's section is V's piece.
                    Some(rel::Place::Consumed) => continue,
                    Some(rel::Place::Whole { out, .. } | rel::Place::Merged { out }) => {
                        (cx.rv, *out as usize)
                    }
                    Some(rel::Place::Framed) => (cx.rv, cx.r.out_eh.map_or(0, |e| e as usize)),
                    _ => (None, s),
                },
            };
            let mut vp = match vo {
                Some(vo) if vs != 0 => v.placed(vo, vs),
                _ => Some(false),
            };
            if !dp && vp == Some(true) && cx.comp_of.contains_key(&o) {
                // A component piece kept by the -r but dropped by D (a
                // COMDAT copy elected outside): (t4) when V keeps the
                // other copy, below.
                if cx.t4.values().any(|&(a, b)| (a, b) == (o, s)) {
                    vp = Some(false);
                }
            }
            if dp && vp == Some(false) && cx.t4.contains_key(&(o, s)) {
                vp = Some(true);
                cx.tol("t4");
            }
            if let Some(vp) = vp
                && dp != vp
            {
                // A section V does not keep because the -r output's piece
                // holds it among others: judged by the piece below.
                let word = |b: bool| if b { "placed" } else { "not placed" };
                cx.out.push(
                    p1(format!(
                        "P-P1 {} is {} in the direct link and {} through -r",
                        d.label(o, s),
                        word(dp),
                        word(vp)
                    ))
                    .site(d.objs[o].site(s, 0)),
                );
                continue;
            }
            if !dp {
                continue;
            }
            cx.cov.pieces += 1;
            let space = d.space_of(o, s);
            let Some(da) = d.addr(o, s, 0) else { continue };
            if sec.is_eh_frame(d.machine) {
                *cx.cov.not_compared.entry(".eh_frame pieces".into()).or_default() += 1;
                continue;
            }
            let size = sec.shdr.sh_size;
            let vplaces = cx.phi(o, s, 0);
            let Some(VPlace::At(vspace, va)) = vplaces.first().cloned() else {
                if !matches!(vplaces.first(), Some(VPlace::Content(_))) {
                    cx.out.push(
                        p1(format!("P-P1 {} has no place through -r", d.label(o, s)))
                            .site(d.objs[o].site(s, 0)),
                    );
                }
                continue;
            };
            // Item 2: the output section of the same name.
            let (nd, nv) = (d.out_name(&space, da, size), v.out_name(&vspace, va, size));
            if size > 0 && nd != nv {
                cx.out.push(
                    p1(format!(
                        "P-P1 {} is in output section {nd} in the direct link and {nv} through -r",
                        d.label(o, s)
                    ))
                    .site(d.objs[o].site(s, 0)),
                );
                continue;
            }
            if d.addr(o, s, 0) != Some(da) || da != va || space != vspace {
                cx.tol("t1");
            }
            // Item 3: the bytes outside the relocated fields.
            let recs = records_of(d, o, s);
            let t4_bytes = cx.t4.contains_key(&(o, s));
            if sec.shdr.sh_type != SHT_NOBITS && !t4_bytes && sec.shdr.sh_flags & SHF_MERGE == 0 {
                let mut masked = vec![false; usize::try_from(size).unwrap_or(0)];
                for &(_, off, _, w, _, _) in &recs {
                    for k in off..off.saturating_add(u64::from(w)) {
                        if let Some(m) = masked.get_mut(k as usize) {
                            *m = true;
                        }
                    }
                }
                let (bd, bv) = (d.bytes(&space, da, size), v.bytes(&vspace, va, size));
                if let (Some(bd), Some(bv)) = (bd, bv) {
                    cx.cov.bytes += size;
                    if let Some(k) = (0..bd.len())
                        .find(|&k| bd[k] != bv[k] && !masked.get(k).copied().unwrap_or(true))
                    {
                        let mut g = p1(format!(
                            "P-P1 {}+{k:#x}: a byte no relocation covers is {:#04x} in the direct link and {:#04x} through -r",
                            d.label(o, s),
                            bd[k],
                            bv[k]
                        ))
                        .site(d.objs[o].site(s, k as u64))
                        .at(va + k as u64);
                        g.expected = Some(u64::from(bd[k]));
                        g.found = Some(u64::from(bv[k]));
                        cx.out.push(g);
                    }
                } else if size > 0 {
                    cx.out.push(
                        p1(format!("P-P1 {}: its bytes cannot be read on one side", d.label(o, s)))
                            .site(d.objs[o].site(s, 0)),
                    );
                }
            }
            if t4_bytes {
                cx.tol("t4");
            }
            // Item 3: every relocated field resolves to φ of D's target.
            for (i, off, ty, w, sym, a) in recs {
                if w == 0 {
                    continue;
                }
                cx.cov.records += 1;
                let at_site = d.objs[o].site(s, off);
                let tname = rel::tname(machine_code(d.machine), ty);
                let fd = d.forms.get(&(o, s)).and_then(|m| m.get(&off)).copied();
                let Some((wo, ws, woff)) = v_record(cx, o, s, i, off) else {
                    cx.out.push(
                        p1(format!(
                            "P-P1 {}+{off:#x} {tname} has no record through -r",
                            d.label(o, s)
                        ))
                        .site(at_site),
                    );
                    continue;
                };
                let fv = v.forms.get(&(wo, ws)).and_then(|m| m.get(&woff)).copied();
                let Some(pd) = d.addr(o, s, off) else { continue };
                let Some(pv) = v.addr(wo, ws, woff) else {
                    cx.out.push(
                        p1(format!(
                            "P-P1 {}+{off:#x} {tname}: its record through -r has no place",
                            d.label(o, s)
                        ))
                        .site(at_site),
                    );
                    continue;
                };
                let tspace = target_space(d, o, sym);
                // S = (S + A) - A with D's addend on both sides: x86's
                // `S - 4` may lie in the piece before S.
                let ad = addend_of(d, o, s, off, w, a);
                let td = full_target(d, &space, pd, ty).map(|t| d.mask(t.wrapping_sub(ad as u64)));
                let tv = full_target(v, &v.space_of(wo, ws), pv, ty)
                    .map(|t| v.mask(t.wrapping_sub(ad as u64)));
                if let (Some(td), Some(tv)) = (td, tv) {
                    let (found, want) = cx.phi_addr(&tspace, td);
                    if found && !want.is_empty() {
                        if !cx.hits(&want, &tspace, tv) {
                            cx.out.push(
                                    p1(format!(
                                        "P-P1 {}+{off:#x} {tname} resolves to {td:#x} ({}) in the direct link and to {tv:#x} through -r, not {}",
                                        d.label(o, s),
                                        describe(d, &tspace, td),
                                        show_places(&want)
                                    ))
                                    .site(at_site)
                                    .at(pv),
                                );
                        }
                        continue;
                    }
                }
                // By form: a target no input piece holds (a slot, an IPLT
                // entry, a TLS offset, a partial field): the two links'
                // verdicts must agree.
                cx.cov.records_by_form += 1;
                if (fd == Some(Form::Failed)) != (fv == Some(Form::Failed)) {
                    cx.out.push(
                        p1(format!(
                            "P-P1 {}+{off:#x} {tname} resolves to its target in the direct link ({}) and to another through -r ({})",
                            d.label(o, s),
                            form_name(fd),
                            form_name(fv)
                        ))
                        .site(at_site)
                        .at(pv),
                    );
                }
            }
        }
    }
}

fn machine_code(m: Machine) -> u16 {
    match m {
        Machine::AArch64 => rel::EM_AARCH64,
        Machine::X86_64 => rel::EM_X86_64,
        Machine::I386 => rel::EM_386,
    }
}

fn describe(d: &Side<'_>, space: &Space, t: u64) -> String {
    let ids = d.spans.get(space).map(|s| s.ids(t)).unwrap_or_default();
    ids.first().map_or_else(|| format!("{t:#x}"), |(o, s, x)| format!("{}+{x:#x}", d.label(*o, *s)))
}

fn show_places(w: &[VPlace]) -> String {
    let v: Vec<String> = w
        .iter()
        .map(|p| match p {
            VPlace::At(_, a) => format!("{a:#x}"),
            VPlace::Content(b) => format!("the bytes {:?}", String::from_utf8_lossy(b)),
        })
        .collect();
    v.join(" or ")
}

/// A unit of item 2's order: an input piece of `D` outside the component,
/// or one output section of the `-r` output, which `V` places as one
/// piece and `D` as its pieces (L-R1's merge by name).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Piece(usize, usize),
    R(usize),
}

/// Item 2's order: the units of each allocated output section, by
/// address, each where its first piece is (§14.8: the order a `-r`
/// output section's pieces have among themselves is P-P2's; the final
/// link places the section whole).
fn item2_order(cx: &mut Cx<'_, '_>) {
    let canon = |cx: &Cx<'_, '_>, o: usize, s: usize| -> Unit {
        // (t4): the other copy of a member stands for D's.
        let (o, s) = cx.t4.iter().find(|(_, b)| **b == (o, s)).map_or((o, s), |(a, _)| *a);
        match cx.comp_of.get(&o).and_then(|&f| cx.r.places.get(f)?.get(s)) {
            Some(rel::Place::Whole { out, .. }) => Unit::R(*out as usize),
            _ => Unit::Piece(o, s),
        }
    };
    let mut dl: BTreeMap<String, Vec<(u64, Unit)>> = BTreeMap::new();
    for (o, ob) in cx.d.objs.iter().enumerate() {
        for (s, sec) in ob.secs.iter().enumerate() {
            if !sec.alloc() || sec.shdr.sh_size == 0 || sec.shdr.sh_flags & SHF_MERGE != 0 {
                continue;
            }
            if let Place::Whole(a) = sec.place {
                let name = cx.d.out_name(&Space::Mem, a, sec.shdr.sh_size);
                dl.entry(name).or_default().push((a, canon(cx, o, s)));
            }
        }
    }
    let mut vl: BTreeMap<String, Vec<(u64, Unit)>> = BTreeMap::new();
    for (vo, ob) in cx.v.objs.iter().enumerate() {
        for (vs, sec) in ob.secs.iter().enumerate() {
            if !sec.alloc() || sec.shdr.sh_size == 0 || sec.shdr.sh_flags & SHF_MERGE != 0 {
                continue;
            }
            let Place::Whole(a) = sec.place else { continue };
            let name = cx.v.out_name(&Space::Mem, a, sec.shdr.sh_size);
            if Some(vo) == cx.rv {
                vl.entry(name).or_default().push((a, Unit::R(vs)));
            } else if let Some(&o) = cx.d.by_key.get(&cx.v.keys[vo]) {
                vl.entry(name).or_default().push((a, canon(cx, o, vs)));
            }
        }
    }
    let first = |mut l: Vec<(u64, Unit)>| -> Vec<Unit> {
        l.sort_by_key(|x| x.0);
        let mut out: Vec<Unit> = Vec::new();
        for (_, u) in l {
            if !out.contains(&u) {
                out.push(u);
            }
        }
        out
    };
    let show = |cx: &Cx<'_, '_>, u: Unit| match u {
        Unit::Piece(o, s) => cx.d.label(o, s),
        Unit::R(vs) => cx.rv.map_or_else(|| "?".into(), |vo| cx.v.label(vo, vs)),
    };
    for (name, a) in dl {
        let Some(b) = vl.remove(&name) else { continue };
        let (a, b) = (first(a), first(b));
        if let Some(k) = (0..a.len().min(b.len())).find(|&k| a[k] != b[k]) {
            cx.out.push(p1(format!(
                "P-P1 the pieces of {name} are in another order through -r: {} is piece {k} in the direct link, {} through -r",
                show(cx, a[k]),
                show(cx, b[k])
            )));
        }
    }
}

/// A global of a symbol table: value, size, type, binding, visibility,
/// section index.
type Glob = (u64, u64, u8, u8, u8, u16);

fn globals(image: &Image<'_>, dynamic: bool) -> BTreeMap<String, Glob> {
    let mut m = BTreeMap::new();
    let mut add = |name: String, y: &mortar_elf::Sym| {
        if !name.is_empty() && matches!(y.binding(), STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE) {
            m.entry(name).or_insert((
                y.st_value,
                y.st_size,
                y.st_info & 0xf,
                y.binding(),
                y.st_other & 3,
                y.st_shndx,
            ));
        }
    };
    if dynamic {
        let Ok(t) = SectionTable::parse(image.data, &image.header) else { return m };
        let Ok(Some(d)) = t.find_symbol_table(SHT_DYNSYM) else { return m };
        for (_, y) in d.iter() {
            let name =
                d.name(&y).map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
            add(name, &y);
        }
    } else {
        for s in &image.symbols {
            add(s.name.clone(), &s.sym);
        }
    }
    m
}

/// Item 4: the globals of `.symtab` and `.dynsym`.
fn item4<'a>(cx: &mut Cx<'a, '_>) {
    let tls = |i: &Image<'_>| {
        i.phdrs.iter().find(|p| p.p_type == mortar_elf::consts::PT_TLS).map(|p| p.p_vaddr)
    };
    let (tls_d, tls_v) = (tls(&cx.d.image), tls(&cx.v.image));
    // (t2): the locals, and so `.strtab`, are not compared; counted when
    // they differ.
    let locals = |i: &Image<'_>| -> Vec<(String, u8)> {
        i.symbols
            .iter()
            .filter(|y| y.sym.binding() == STB_LOCAL)
            .map(|y| (y.name.clone(), y.sym.st_info))
            .collect()
    };
    let table = |i: &Image<'a>, n: &str| {
        i.sections.iter().position(|y| y.name == n).and_then(|k| i.section_data(k))
    };
    if locals(&cx.d.image) != locals(&cx.v.image)
        || table(&cx.d.image, ".strtab") != table(&cx.v.image, ".strtab")
        || table(&cx.d.image, ".shstrtab") != table(&cx.v.image, ".shstrtab")
    {
        cx.tol("t2");
    }
    // (t3): the build-id descriptor is not compared; counted when it differs.
    let note = |i: &Image<'a>| {
        i.sections
            .iter()
            .position(|y| y.name == ".note.gnu.build-id")
            .and_then(|k| i.section_data(k))
    };
    if let (Some(a), Some(b)) = (note(&cx.d.image), note(&cx.v.image))
        && a != b
    {
        cx.tol("t3");
    }
    for (dynamic, table) in [(false, ".symtab"), (true, ".dynsym")] {
        let a = globals(&cx.d.image, dynamic);
        let b = globals(&cx.v.image, dynamic);
        for name in a.keys().chain(b.keys()).collect::<BTreeSet<_>>() {
            let (Some(x), Some(y)) = (a.get(name), b.get(name)) else {
                let (w, z) = if a.contains_key(name) { ("in", "not") } else { ("not", "in") };
                cx.out.push(p1(format!(
                    "P-P1 symbol {name}: {w} the direct link's {table}, {z} through -r"
                )));
                continue;
            };
            cx.cov.symbols += 1;
            if (x.1, x.2, x.3, x.4) != (y.1, y.2, y.3, y.4) {
                cx.out.push(p1(format!(
                    "P-P1 symbol {name}: size {:#x}, type {}, binding {}, visibility {} in the direct link; size {:#x}, type {}, binding {}, visibility {} through -r ({table})",
                    x.1, x.2, x.3, x.4, y.1, y.2, y.3, y.4
                )));
                continue;
            }
            if !value_corresponds(cx, *x, *y, tls_d, tls_v) {
                cx.out.push(p1(format!(
                    "P-P1 symbol {name}: {:#x} in the direct link, {:#x} through -r ({table})",
                    x.0, y.0
                )));
            }
        }
    }
}

/// Whether `V`'s value `y` is `φ` of `D`'s `x`.
fn value_corresponds(
    cx: &Cx<'_, '_>,
    x: Glob,
    y: Glob,
    tls_d: Option<u64>,
    tls_v: Option<u64>,
) -> bool {
    if matches!(x.5, SHN_UNDEF | SHN_ABS | SHN_COMMON)
        || matches!(y.5, SHN_UNDEF | SHN_ABS | SHN_COMMON)
    {
        return x.5 == y.5 && x.0 == y.0;
    }
    // A TLS symbol's value is its offset in the TLS block.
    let (xa, ya) = if x.2 == STT_TLS {
        (x.0.wrapping_add(tls_d.unwrap_or(0)), y.0.wrapping_add(tls_v.unwrap_or(0)))
    } else {
        (x.0, y.0)
    };
    let sec_of = |i: &Image<'_>, k: u16| i.sections.get(usize::from(k)).cloned();
    let (Some(sd), Some(sv)) = (sec_of(&cx.d.image, x.5), sec_of(&cx.v.image, y.5)) else {
        return false;
    };
    let space =
        if sd.shdr.sh_flags & SHF_ALLOC != 0 { Space::Mem } else { Space::Non(sd.name.clone()) };
    let (found, want) = cx.phi_addr(&space, xa);
    if found && !want.is_empty() {
        return cx.hits(&want, &space, ya);
    }
    // A linker-made place: the section of its name, at the same distance
    // from its start, or both at its end.
    sd.name == sv.name
        && (xa.wrapping_sub(sd.shdr.sh_addr) == ya.wrapping_sub(sv.shdr.sh_addr)
            || (xa == sd.shdr.sh_addr + sd.shdr.sh_size && ya == sv.shdr.sh_addr + sv.shdr.sh_size))
}

// ---- the exclusions ----

/// The last of a pair of options, and the values of an option `--o=X` or
/// `--o X` (one or two dashes).
fn opt_values(w: &[String], name: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut i = 0;
    while i < w.len() {
        let a = w[i].trim_start_matches('-');
        if w[i].starts_with('-') {
            if let Some(x) = a.strip_prefix(name).and_then(|r| r.strip_prefix('=')) {
                v.push(x.to_string());
            } else if a == name && i + 1 < w.len() {
                v.push(w[i + 1].clone());
                i += 1;
            }
        }
        i += 1;
    }
    v
}

fn last_of(w: &[String], yes: &str, no: &str) -> bool {
    let mut on = false;
    for x in w {
        let a = x.trim_start_matches('-');
        if a == yes {
            on = true;
        } else if a == no {
            on = false;
        }
    }
    on
}

/// P-P1's exclusions (a)-(d), decided from the two lines and the links.
fn exclusions(l: &[String], rl: &[String], r: &RModel, d: &Side<'_>) -> Vec<(char, String)> {
    let mut x = Vec::new();
    let archive = !r.members.is_empty() || rl.iter().any(|w| w == "--start-lib");
    if archive {
        // §3.8's closures, as the two links' maps show them (each map's
        // input set is judged by §3.8 on its own link).
        let paths: BTreeSet<&str> = r.members.iter().map(|(p, _)| p.as_str()).collect();
        let in_d: BTreeSet<(String, String)> = d
            .objs
            .iter()
            .filter(|o| paths.contains(o.path.as_str()))
            .filter_map(|o| o.member.clone().map(|m| (o.path.clone(), m)))
            .collect();
        if in_d != r.members {
            x.push((
                'a',
                "the -r line's closure extracts other archive members than the direct link's"
                    .into(),
            ));
        }
    }
    let gc = last_of(l, "gc-sections", "no-gc-sections");
    let icf = opt_values(l, "icf").last().is_some_and(|v| v != "none");
    if gc || icf {
        let mixed =
            r.pieces.values().any(|ps| ps.iter().map(|p| p.0).collect::<BTreeSet<_>>().len() > 1);
        if mixed {
            x.push((
                'b',
                format!(
                    "the line has {} and an output section of the -r output holds pieces of two input files",
                    if gc { "--gc-sections" } else { "--icf" }
                ),
            ));
        }
    }
    for sym in opt_values(l, "wrap") {
        if r.defines.contains(&sym) && r.refs.contains(&sym) {
            x.push(('c', format!("--wrap={sym}, which the component both defines and references")));
        }
    }
    if !opt_values(l, "exclude-libs").is_empty() && !r.members.is_empty() {
        x.push(('d', "--exclude-libs, and the -r line extracts an archive member".into()));
    }
    x
}

// ---- (t4) ----

/// The COMDAT signature of each group member of a side's inputs, and
/// which copy each side keeps.
fn comdat_of(s: &Side<'_>) -> BTreeMap<(usize, usize), String> {
    let mut m = BTreeMap::new();
    for (o, ob) in s.objs.iter().enumerate() {
        let Some(t) = ob.symtab else { continue };
        for (g, sec) in ob.secs.iter().enumerate() {
            if sec.shdr.sh_type != SHT_GROUP {
                continue;
            }
            let data = ob.data(g);
            let b: &[u8] = &data;
            if b.len() < 4 || u32::from_le_bytes([b[0], b[1], b[2], b[3]]) & 1 == 0 {
                continue;
            }
            let Ok(y) = t.get(mortar_elf::SymbolIndex(sec.shdr.sh_info)) else { continue };
            let sig =
                t.name(&y).map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
            for w in b[4..].chunks_exact(4) {
                m.insert((o, u32::from_le_bytes([w[0], w[1], w[2], w[3]]) as usize), sig.clone());
            }
        }
    }
    m
}

/// (t4): for each signature whose kept copy in `D` is another input than
/// the one `V` keeps (one inside the component, one outside), `D`'s
/// kept member to the other copy's member of the same name.
fn t4_map(cx: &Cx<'_, '_>) -> BTreeMap<(usize, usize), (usize, usize)> {
    let groups = comdat_of(cx.d);
    let mut by_sig: BTreeMap<&str, Vec<(usize, usize)>> = BTreeMap::new();
    for (k, sig) in &groups {
        by_sig.entry(sig.as_str()).or_default().push(*k);
    }
    let kept_v = |cx: &Cx<'_, '_>, o: usize, s: usize| -> bool {
        match cx.comp_of.get(&o) {
            None => {
                cx.v.by_key.get(&cx.d.keys[o]).is_some_and(|&vo| cx.v.placed(vo, s) == Some(true))
            }
            Some(&f) => match cx.r.places.get(f).and_then(|p| p.get(s)) {
                Some(rel::Place::Whole { out, .. }) => {
                    cx.rv.is_some_and(|vo| cx.v.placed(vo, *out as usize) == Some(true))
                }
                _ => false,
            },
        }
    };
    let mut m = BTreeMap::new();
    for members in by_sig.values() {
        let in_comp = |o: usize| cx.comp_of.contains_key(&o);
        let dk: Vec<_> =
            members.iter().filter(|&&(o, s)| cx.d.placed(o, s) == Some(true)).collect();
        let vk: Vec<_> = members.iter().filter(|&&(o, s)| kept_v(cx, o, s)).collect();
        for &&(o, s) in &dk {
            if vk.iter().any(|&&(p, _)| p == o) {
                continue;
            }
            let name = &cx.d.objs[o].secs[s].name;
            if let Some(&&(p, t)) = vk
                .iter()
                .find(|&&&(p, t)| in_comp(p) != in_comp(o) && &cx.d.objs[p].secs[t].name == name)
            {
                m.insert((o, s), (p, t));
            }
        }
    }
    m
}

/// The names some input of `D` or `V` defines `SHN_COMMON`, as each link
/// allocated them: `D`'s value and size, `V`'s value.
fn commons_of(d: &Side<'_>, v: &Side<'_>) -> Vec<(u64, u64, u64)> {
    let mut names = BTreeSet::new();
    for ob in d.objs.iter().chain(v.objs.iter()) {
        let Some(t) = ob.symtab else { continue };
        for (_, y) in t.iter() {
            if y.st_shndx == SHN_COMMON
                && let Ok(n) = t.name(&y)
            {
                names.insert(n.to_vec());
            }
        }
    }
    names
        .into_iter()
        .filter_map(|n| {
            let n = String::from_utf8_lossy(&n).into_owned();
            let (x, y) = (d.image.global(&n)?, v.image.global(&n)?);
            Some((d.mask(x.sym.st_value), x.sym.st_size, v.mask(y.sym.st_value)))
        })
        .collect()
}

// ---- the entry point ----

/// P-P1 (§14.5): the direct link `direct` and the link `via` through the
/// `-r` output `rmap` (its inputs, map and line, and the path `via`'s map
/// names it by), both by the same linker. The report has P-P1's findings
/// only, and `coverage.composition` (`composition: {x} judged, {y} not
/// judged ({letters})`). Never panics.
pub fn compose(direct: &Link<'_>, via: &Link<'_>, rmap: &Partial<'_>) -> Report {
    let mut report = Report::default();
    let mut cov = CompCoverage::default();
    let ds = Store::read(direct.src, &[direct.map]);
    let vs = Store::read(via.src, &[via.map]);
    let rs = Store::read(rmap.link.src, &[rmap.link.map]);
    let rwords = words(&rmap.link);
    let (_, model) =
        rel::model(rmap.link.output, &rs, 0, rmap.link.map, rwords.as_deref(), read_model);
    let Some(r) = model else {
        report.findings.push(p1("P-P1 the -r output cannot be read: nothing to compare"));
        report.coverage.composition = Some(cov);
        return report;
    };
    let lwords = words(direct);
    let d = side("direct link", direct.output, &ds, direct.map, lwords.as_deref());
    let vwords = words(via);
    let v = side("link through -r", via.output, &vs, via.map, vwords.as_deref());
    let (d, v) = match (d, v) {
        (Ok(d), Ok(v)) => (d, v),
        (d, v) => {
            for e in [d.err(), v.err()].into_iter().flatten() {
                report.findings.push(p1(format!("P-P1 {e}: nothing to compare")));
            }
            report.coverage.composition = Some(cov);
            return report;
        }
    };
    let excl =
        exclusions(lwords.as_deref().unwrap_or(&[]), rwords.as_deref().unwrap_or(&[]), &r, &d);
    if !excl.is_empty() {
        cov.not_judged = excl;
        report.coverage.composition = Some(cov);
        return report;
    }
    cov.judged = 1;
    let mut comp_of = BTreeMap::new();
    for (f, k) in r.ins.iter().enumerate() {
        if let Some(&o) = d.by_key.get(k) {
            comp_of.insert(o, f);
        }
    }
    let rv = v.objs.iter().position(|o| o.path == rmap.path && o.member.is_none());
    if rv.is_none() {
        report.findings.push(p1(format!(
            "P-P1 the link through -r does not name the -r output {}",
            rmap.path
        )));
    }
    let mut cx = Cx {
        d: &d,
        v: &v,
        r: &r,
        comp_of,
        rv,
        t4: BTreeMap::new(),
        commons: commons_of(&d, &v),
        cov,
        out: Vec::new(),
    };
    cx.t4 = t4_map(&cx);
    item1(&mut cx);
    items23(&mut cx);
    item2_order(&mut cx);
    item4(&mut cx);
    if !d.forms.is_empty() || !v.forms.is_empty() {
        let _ = (d.tag, v.tag);
    }
    report.findings.append(&mut cx.out);
    report.coverage.composition = Some(cx.cov);
    report.sort();
    report.findings.dedup();
    report
}

/// The tolerances P-P1 used and the exclusion letters, for a caller's
/// line (`composition not judged: (<letter>) <reason>`).
pub fn not_judged(report: &Report) -> Vec<String> {
    report
        .coverage
        .composition
        .as_ref()
        .map(|c| c.not_judged.iter().map(|(l, why)| format!("({l}) {why}")).collect())
        .unwrap_or_default()
}
