//! `--gc-sections` (`spec/image.md` §3.2, I-6 to I-10, I-109): the least
//! set of sections closed under the edges of I-7 that contains the roots
//! of I-6, marked in parallel rounds (I-8).
//!
//! The shape of the computation:
//!
//! 1. **Facts** (one parallel pass over the files): per section whether it
//!    is allocated and whether it is a root by I-6's section rules; the
//!    `SHF_LINK_ORDER` pairs (I-109); the sections whose names are C
//!    identifiers (`__start_`/`__stop_`, `spec/layout.md` L-96).
//! 2. **Definitions** (one parallel pass over the keys): where each key's
//!    definition is, as a mark target; and whether the key is a root
//!    (rooted by resolution, exported, or `--gc-keep-exported`).
//! 3. **Frames** (one parallel pass over the `.eh_frame` sections): every
//!    relocation of every record resolved to a target, per record, so that
//!    a code section's FDEs (and their CIEs) contribute their edges (the
//!    LSDA, the personality) when the code section is marked (I-7).
//! 4. **Rounds**: round 0 is the roots; each later round walks the
//!    relocation records of the sections first marked by the round before,
//!    partitioned by section and by `SCAN_BLOCK` records. A section is
//!    marked with `swap(true)` on an atomic boolean; the worker whose swap
//!    returned `false` collects it for the next round (`spec/model.md` §6
//!    rules 2 and 3). The set is the least fixed point whatever the
//!    schedule; the order of a round's list is never observed.
//!
//! Edges come from relocation records only, through the tables
//! (`sym_map`, `redirect`, `def`): never from instruction bytes, so the
//! collector is the same for every architecture. Every record is an edge,
//! `R_*_NONE` included (a `.reloc ., R_*_NONE, sym` is how assembly asks
//! for a dependency). For a `SHT_REL` target the addend is in the relocated
//! bytes; the mark does not read them, so a section-symbol reference into
//! a split mergeable section marks every piece of that section (more than
//! needed, never less).

#![allow(
    clippy::result_large_err,
    reason = "the phases' diagnostic pair, made once per failed link"
)]

use std::sync::atomic::{AtomicBool, Ordering};

use mortar_core::assignment::{Assignment, OutOf};
use mortar_core::inputs::{Inputs, Reader};
use mortar_core::par::{Pool, SCAN_BLOCK};
use mortar_core::resolution::{Def, InternalDef, RefFlags, Resolution, SectionKind};
use mortar_core::split::Split;
use mortar_core::{Column, FileId, Liveness, SectionId, SymbolId};
use mortar_diag::{Diagnostic, Key, Kind, Place};
use mortar_elf::consts::{
    SHF_ALLOC, SHF_LINK_ORDER, SHT_GROUP, SHT_NOTE, STB_LOCAL, STT_SECTION, STV_DEFAULT,
    STV_PROTECTED,
};
use mortar_elf::{Object, SectionIndex, SymbolIndex, SymbolSection, SymbolTable};

use crate::read::{self, ReadFail};
use crate::{LiveInputs, LiveOptions, PHASE};

/// A diagnostic with its key.
pub(crate) type Keyed = (Key, Diagnostic);

/// "Every piece of the section": a target offset no piece lookup takes.
const WHOLE: u32 = u32::MAX;

/// What a reference makes live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// Nothing in a section: undefined, absolute, shared, a common (whose
    /// output is not collected), a linker-defined symbol.
    None,
    /// `section`, and in a split mergeable section the piece containing
    /// byte `at` (or every piece, [`WHOLE`]).
    At(SectionId, u32),
    /// `__start_NAME` / `__stop_NAME`: the sections `named[lo..hi]`.
    Named(u32, u32),
}

/// Section fact bits.
const ALLOC: u8 = 1;
/// A root by a section rule of I-6.
const ROOT: u8 = 2;
/// Its relocations are edges when it is live (I-7): every section but
/// `.eh_frame` (whose edges are its FDEs') and the debug sections (whose
/// references to collected code get the tombstone, I-47). A non-allocated
/// section's references are edges too: GNU ld marks through notes
/// (`.note.stapsdt` keeps `.stapsdt.base`), and a kept section's
/// reference to a collected one would be refused (I-12) or wrong.
const EDGES: u8 = 4;

/// What phase 50 knows of one file.
struct FileFacts<'s> {
    obj: Option<Object<'s>>,
    table: Option<SymbolTable<'s>>,
}

/// The shared state of the mark.
struct Marks<'t> {
    resolution: &'t Resolution,
    split: &'t Split,
    live: Vec<AtomicBool>,
    frag: Vec<AtomicBool>,
    /// Per `split.merge_of` row: every piece already marked.
    whole: Vec<AtomicBool>,
    named: &'t [(&'t [u8], SectionId)],
}

impl Marks<'_> {
    /// Marks `s` (a kept section only: `Liveness::check`'s rule); pushes it
    /// when this call marked it.
    fn section(&self, s: SectionId, out: &mut Vec<SectionId>) {
        let Some(cell) = self.live.get(s.0 as usize) else { return };
        if !self
            .resolution
            .sections
            .get(s)
            .is_some_and(mortar_core::resolution::SectionRow::is_kept)
        {
            return;
        }
        if !cell.swap(true, Ordering::Relaxed) {
            out.push(s);
        }
    }

    /// Marks `s` and, in a split mergeable section, the piece holding byte
    /// `at` (I-7: by offset through `Split`), or every piece.
    fn at(&self, s: SectionId, at: u32, out: &mut Vec<SectionId>) {
        self.section(s, out);
        if !self
            .resolution
            .sections
            .get(s)
            .is_some_and(mortar_core::resolution::SectionRow::is_kept)
        {
            return;
        }
        let rows = &self.split.merge_of;
        let Ok(row) = rows.binary_search_by_key(&s, |r| r.section) else { return };
        if at == WHOLE {
            if self.whole[row].swap(true, Ordering::Relaxed) {
                return;
            }
            for &f in self.split.piece_frag.get(rows[row].pieces.slots()).unwrap_or_default() {
                if let Some(m) = self.frag.get(f.0 as usize) {
                    m.store(true, Ordering::Relaxed);
                }
            }
            return;
        }
        if let Some(p) = self.split.merge_piece_at(s, at)
            && let Some(&f) = self.split.piece_frag.get(p.piece as usize)
            && let Some(m) = self.frag.get(f.0 as usize)
        {
            m.store(true, Ordering::Relaxed);
        }
    }

    fn target(&self, t: Target, out: &mut Vec<SectionId>) {
        match t {
            Target::None => {}
            Target::At(s, at) => self.at(s, at, out),
            Target::Named(lo, hi) => {
                for &(_, s) in self.named.get(lo as usize..hi as usize).unwrap_or_default() {
                    self.at(s, WHOLE, out);
                }
            }
        }
    }
}

/// Whether an input name is a start-up table GNU ld's script places by
/// name into `.init_array`/`.fini_array` (`.ctors`, `.ctors.NNNNN`, …):
/// all three references keep them (E14 Q5).
fn startup_input(name: &[u8]) -> bool {
    [&b".ctors"[..], b".dtors", b".init_array", b".fini_array", b".preinit_array"].iter().any(|p| {
        name == *p || name.strip_prefix(*p).is_some_and(|rest| rest.first() == Some(&b'.'))
    })
}

/// Whether `name` is a C identifier (`spec/layout.md` L-96: only such
/// sections have `__start_`/`__stop_` symbols).
fn c_identifier(name: &[u8]) -> bool {
    matches!(name.first(), Some(c) if c.is_ascii_alphabetic() || *c == b'_')
        && name.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

/// The output sections whose input sections are run by name at start-up
/// rather than referenced (I-6). `spec/layout.md` §5; the `Assignment`'s
/// `keep` has `.init*`, `.fini*`, `.preinit_array` already.
fn startup_output(name: &[u8]) -> bool {
    matches!(
        name,
        b".init"
            | b".fini"
            | b".init_array"
            | b".fini_array"
            | b".preinit_array"
            | b".ctors"
            | b".dtors"
    )
}

fn malformed(what: String) -> Diagnostic {
    Diagnostic::error(Kind::Malformed { what })
}

/// The mark (I-6 to I-8, I-109). `Err`: the least-keyed diagnostic of a
/// section whose relocations could not be read.
#[allow(clippy::too_many_lines, reason = "the four steps in order, each a few lines")]
pub(crate) fn collect(
    tables_in: &LiveInputs<'_, '_>,
    options: LiveOptions,
    pool: &Pool,
) -> Result<Liveness, Keyed> {
    let inputs = tables_in.inputs;
    let r = tables_in.resolution;
    let a = tables_in.assignment;
    let split = tables_in.split;
    let sections = r.sections.len() as usize;

    // 1. Facts per file.
    let files = inputs.files.len() as usize;
    let per_file: Vec<Result<FileOut<'_>, Keyed>> =
        pool.map(files, |f| file_facts(inputs, r, a, FileId(f as u32)));
    // A section no file's facts describe (not of an object, or a header
    // `mortar_elf` cannot read) is a root: the mark never drops what it
    // has not looked at.
    let mut facts = vec![ROOT | EDGES; sections];
    let mut link_order: Vec<(SectionId, SectionId)> = Vec::new();
    let mut named: Vec<(&[u8], SectionId)> = Vec::new();
    let mut group_members: Vec<SectionId> = Vec::new();
    // (member, its group's span in `group_members`).
    let mut group_of: Vec<(SectionId, u32, u32)> = Vec::new();
    let mut tables: Vec<FileFacts<'_>> = Vec::with_capacity(files);
    let mut errors: Vec<Keyed> = Vec::new();
    for (f, out) in per_file.into_iter().enumerate() {
        match out {
            Ok(o) => {
                let base = r.section_base.get(FileId(f as u32)).map_or(0, |b| b.start.0 as usize);
                for (k, bits) in o.bits.into_iter().enumerate() {
                    if let Some(cell) = facts.get_mut(base + k) {
                        *cell = bits;
                    }
                }
                link_order.extend(o.link_order);
                named.extend(o.named);
                for g in o.groups {
                    let start = group_members.len() as u32;
                    for &m in &g {
                        group_of.push((m, start, g.len() as u32));
                    }
                    group_members.extend(g);
                }
                tables.push(FileFacts { obj: o.obj, table: o.table });
            }
            Err(e) => {
                errors.push(e);
                tables.push(FileFacts { obj: None, table: None });
            }
        }
    }
    if let Some(e) = least(&mut errors) {
        return Err(e);
    }
    link_order.sort_unstable();
    group_of.sort_unstable();
    // By name, then section: a name's sections are one run.
    named.sort_unstable();

    // 2. Definitions.
    let keys = inputs.symbols.len() as usize;
    let defs: Vec<(Target, bool)> = pool.map(keys, |k| {
        let id = SymbolId(k as u32);
        let target = def_target(inputs, r, a, &tables, &named, id);
        let props = r.props.get(id).copied().unwrap_or_default();
        let exported = props.refs.contains(RefFlags::EXPORTED);
        let keep_exported = options.keep_exported
            && matches!(r.def.get(id), Some(Def::Defined(_)))
            && props.binding != STB_LOCAL
            && matches!(props.visibility, STV_DEFAULT | STV_PROTECTED);
        (target, exported || keep_exported)
    });

    // 3. Frames.
    let eh_rows = &split.eh_of;
    let eh: Vec<Result<Vec<(u32, Target)>, Keyed>> =
        pool.map(eh_rows.len(), |row| eh_edges(inputs, r, split, &tables, &defs, row));
    let mut eh_edges_of: Vec<Vec<(u32, Target)>> = Vec::with_capacity(eh.len());
    for e in eh {
        match e {
            Ok(v) => eh_edges_of.push(v),
            Err(e) => {
                errors.push(e);
                eh_edges_of.push(Vec::new());
            }
        }
    }
    if let Some(e) = least(&mut errors) {
        return Err(e);
    }
    // (code section, eh row, FDE ordinal), sorted by code section.
    let mut by_code: Vec<(SectionId, u32, u32)> = Vec::new();
    for (row, sr) in eh_rows.iter().enumerate() {
        for (ord, p) in split.eh_piece.get(sr.pieces.slots()).unwrap_or_default().iter().enumerate()
        {
            if let Some(code) = p.code.get()
                && !p.is_cie()
            {
                by_code.push((code, row as u32, ord as u32));
            }
        }
    }
    by_code.sort_unstable();

    let marks = Marks {
        resolution: r,
        split,
        live: (0..sections).map(|_| AtomicBool::new(false)).collect(),
        frag: (0..split.frags.len() as usize).map(|_| AtomicBool::new(false)).collect(),
        whole: (0..split.merge_of.len()).map(|_| AtomicBool::new(false)).collect(),
        named: &named,
    };

    // 4. Round 0: the roots, in section then key order.
    let mut round: Vec<SectionId> = Vec::new();
    for (s, &bits) in facts.iter().enumerate() {
        if bits & ROOT != 0 {
            marks.at(SectionId(s as u32), WHOLE, &mut round);
        }
    }
    for &root in tables_in.roots {
        if let Some(&(target, _)) = defs.get(root.0 as usize) {
            marks.target(target, &mut round);
        }
    }
    for &(target, rooted) in &defs {
        if rooted {
            marks.target(target, &mut round);
        }
    }

    let cx = Walk {
        inputs,
        r,
        split,
        tables: &tables,
        defs: &defs,
        facts: &facts,
        link_order: &link_order,
        group_of: &group_of,
        group_members: &group_members,
        by_code: &by_code,
        eh_edges: &eh_edges_of,
        marks: &marks,
    };
    while !round.is_empty() {
        let units = cx.units(&round);
        let got = pool.map_collect(units.len(), |u, out| cx.walk(units[u], out));
        let mut next = Vec::new();
        for item in got.items {
            match item {
                Out::Marked(s) => next.push(s),
                Out::Failed(e) => errors.push(*e),
            }
        }
        if let Some(e) = least(&mut errors) {
            return Err(e);
        }
        round = next;
    }

    let live: Vec<bool> = marks.live.into_iter().map(AtomicBool::into_inner).collect();
    let frag_live: Vec<bool> = marks.frag.into_iter().map(AtomicBool::into_inner).collect();
    Ok(Liveness {
        live: Column::from_vec(live).unwrap_or_default(),
        frag_live: Column::from_vec(frag_live).unwrap_or_default(),
    })
}

/// The least-keyed diagnostic of a list (the list is a set: the sections
/// whose records are read are the live ones, whatever the schedule).
fn least(errors: &mut Vec<Keyed>) -> Option<Keyed> {
    errors.sort_by_key(|e| e.0);
    errors.drain(..).next()
}

/// One file's facts (step 1).
struct FileOut<'s> {
    obj: Option<Object<'s>>,
    table: Option<SymbolTable<'s>>,
    /// Per section of the file, from its first id.
    bits: Vec<u8>,
    link_order: Vec<(SectionId, SectionId)>,
    named: Vec<(&'s [u8], SectionId)>,
    /// The section groups of more than one kept member, members in file
    /// order.
    groups: Vec<Vec<SectionId>>,
}

fn file_facts<'s>(
    inputs: &Inputs<'s>,
    r: &Resolution,
    assignment: &Assignment<'_>,
    file: FileId,
) -> Result<FileOut<'s>, Keyed> {
    let empty = FileOut {
        obj: None,
        table: None,
        bits: Vec::new(),
        link_order: Vec::new(),
        named: Vec::new(),
        groups: Vec::new(),
    };
    let Some(range) = r.section_base.get(file).copied() else { return Ok(empty) };
    let Reader::Object(obj) = inputs.files[file].reader else { return Ok(empty) };
    if range.is_empty() {
        return Ok(FileOut { obj: Some(obj), ..empty });
    }
    let path = || inputs.files[file].display();
    let table = obj.symbols().map_err(|e| {
        (Key::new(PHASE, file.0, 0), malformed(e.to_string()).at(Place::File(path())))
    })?;
    let len = range.len() as usize;
    let (groups, grouped) = section_groups(inputs, r, obj, file, range.start.0, len)?;
    let mut bits = vec![ROOT | EDGES; len];
    let mut link_order = Vec::new();
    let mut named = Vec::new();
    for (k, sid) in range.iter().enumerate() {
        let row = r.sections[sid];
        if !row.is_kept() || k == 0 {
            continue;
        }
        let Ok(s) = obj.section(SectionIndex(k as u32)) else { continue };
        let h = s.header();
        let alloc = h.sh_flags & SHF_ALLOC != 0;
        let ordered = h.sh_flags & SHF_LINK_ORDER != 0 && h.sh_link != 0;
        let name = s.name().unwrap_or_default();
        let out = assignment.out_of.get(sid).copied().unwrap_or(OutOf::None);
        let out_name = out.out().and_then(|o| assignment.outsecs.get(o)).map(|o| o.name);
        // I-6, section rules (I-109 first: a `SHF_LINK_ORDER` section
        // follows its target, alloc or not).
        let by_type =
            !grouped[k] && (!alloc || h.sh_type == SHT_NOTE || row.kind == SectionKind::Note);
        let root = if ordered {
            false
        } else {
            assignment.keep.get(sid).copied().unwrap_or(false)
                || by_type
                // `.eh_frame` is kept whole; its FDEs follow their code
                // (I-3) and its relocations are edges only through them.
                || row.kind == SectionKind::EhFrame
                // Kept but without a place (`OutOf::Consumed`, a kind
                // without output): as without `--gc-sections`.
                || out.out().is_none()
                || out_name.is_some_and(startup_output)
                || startup_input(name)
        };
        let mut b = 0;
        if alloc {
            b |= ALLOC;
        }
        if root {
            b |= ROOT;
        }
        if row.kind != SectionKind::EhFrame && !mortar_arch::is_debug_name(name) {
            b |= EDGES;
        }
        bits[k] = b;
        if ordered && let Some(target) = r.section_id(file, SectionIndex(h.sh_link)) {
            link_order.push((target, sid));
        }
        if c_identifier(name) {
            named.push((name, sid));
        }
    }
    Ok(FileOut { obj: Some(obj), table, bits, link_order, named, groups })
}

/// The section groups of `file` (I-7: a group's members are live
/// together), and per section of the file whether it is in a group with
/// an allocated member: such a member is not a root by its type (a note,
/// a non-allocated section) but lives and dies with its group (E14 Q1,
/// Q3).
fn section_groups(
    inputs: &Inputs<'_>,
    r: &Resolution,
    obj: Object<'_>,
    file: FileId,
    start: u32,
    len: usize,
) -> Result<(Vec<Vec<SectionId>>, Vec<bool>), Keyed> {
    let path = || inputs.files[file].display();
    let mut groups: Vec<Vec<SectionId>> = Vec::new();
    let mut grouped = vec![false; len];
    for k in 1..len {
        let sid = start + k as u32;
        let row = r.sections[SectionId(sid)];
        if row.kind != SectionKind::Group {
            continue;
        }
        let Ok(s) = obj.section(SectionIndex(k as u32)) else { continue };
        if s.sh_type() != SHT_GROUP {
            continue;
        }
        let key = Key::new(PHASE, file.0, k as u64);
        let place =
            || Place::section(path(), String::from_utf8_lossy(s.name().unwrap_or_default()), 0);
        let g = s.group().map_err(|e| (key, malformed(e.to_string()).at(place())))?;
        let mut members = Vec::with_capacity(g.len());
        let mut alloc = false;
        for m in g.members() {
            let m = m.map_err(|e| (key, malformed(e.to_string()).at(place())))?;
            let Some(id) = r.section_id(file, m) else { continue };
            if !r.sections[id].is_kept() {
                continue;
            }
            if obj.section(m).is_ok_and(|ms| ms.flags() & SHF_ALLOC != 0) {
                alloc = true;
            }
            members.push(id);
        }
        if alloc {
            for &m in &members {
                if let Some(slot) = grouped.get_mut((m.0 - start) as usize) {
                    *slot = true;
                }
            }
        }
        if members.len() > 1 {
            groups.push(members);
        }
    }
    Ok((groups, grouped))
}

/// Where the definition of key `id` is, as a mark target (I-7).
fn def_target(
    inputs: &Inputs<'_>,
    r: &Resolution,
    assignment: &Assignment<'_>,
    tables: &[FileFacts<'_>],
    named: &[(&[u8], SectionId)],
    id: SymbolId,
) -> Target {
    match assignment.def(r, id) {
        Some(Def::Defined(e)) => {
            let Some(table) = tables.get(e.file.0 as usize).and_then(|f| f.table.as_ref()) else {
                return Target::None;
            };
            let Ok(sym) = table.get(e.index) else { return Target::None };
            match table.section_of(e.index, &sym) {
                Ok(SymbolSection::Section(i)) => match r.section_id(e.file, i) {
                    Some(s) => Target::At(s, u32::try_from(sym.st_value).unwrap_or(WHOLE)),
                    None => Target::None,
                },
                _ => Target::None,
            }
        }
        Some(Def::Internal(InternalDef::Provide { conditional: true })) => {
            let name = inputs.symbols.get(id).map_or(&[][..], |k| k.name);
            let sec = name.strip_prefix(b"__start_").or_else(|| name.strip_prefix(b"__stop_"));
            match sec {
                Some(sec) if c_identifier(sec) => {
                    let lo = named.partition_point(|&(n, _)| n < sec);
                    let hi = named.partition_point(|&(n, _)| n <= sec);
                    if lo < hi { Target::Named(lo as u32, hi as u32) } else { Target::None }
                }
                _ => Target::None,
            }
        }
        _ => Target::None,
    }
}

/// What record `raw` of `file` refers to (I-7): a local or section
/// symbol's section, or the definition of a global's key (`sym_map`,
/// `redirect` for an undefined mention, `def`). `None`: a record whose
/// symbol cannot be read, which the scan reports in a live section.
fn record_target(
    inputs: &Inputs<'_>,
    r: &Resolution,
    table: &SymbolTable<'_>,
    defs: &[(Target, bool)],
    file: FileId,
    raw: read::Raw,
) -> Target {
    if raw.r_sym == 0 {
        return Target::None;
    }
    let index = SymbolIndex(raw.r_sym);
    let Ok(sym) = table.get(index) else { return Target::None };
    let first = table.first_global().0;
    if raw.r_sym < first || sym.sym_type() == STT_SECTION {
        let Ok(SymbolSection::Section(i)) = table.section_of(index, &sym) else {
            return Target::None;
        };
        let Some(s) = r.section_id(file, i) else { return Target::None };
        let at = if sym.sym_type() == STT_SECTION {
            raw.addend
                .and_then(|a| i64::try_from(sym.st_value).ok()?.checked_add(a))
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(WHOLE)
        } else {
            u32::try_from(sym.st_value).unwrap_or(WHOLE)
        };
        return Target::At(s, at);
    }
    let Some(&id) = inputs.sym_map_of(file).get((raw.r_sym - first) as usize) else {
        return Target::None;
    };
    let to = r.redirect.get(id).copied().unwrap_or(id);
    let id =
        if to != id && table.section_of(index, &sym).is_ok_and(|s| s == SymbolSection::Undefined) {
            to
        } else {
            id
        };
    defs.get(id.0 as usize).map_or(Target::None, |d| d.0)
}

/// The relocations of `.eh_frame` row `row`, resolved, with the ordinal of
/// the record each lies in, sorted by ordinal (step 3). A relocation in
/// no record is S-39's to refuse; it is no edge here.
fn eh_edges(
    inputs: &Inputs<'_>,
    r: &Resolution,
    split: &Split,
    tables: &[FileFacts<'_>],
    defs: &[(Target, bool)],
    row: usize,
) -> Result<Vec<(u32, Target)>, Keyed> {
    let sr = split.eh_of[row];
    let s = sr.section;
    let sec = r.sections[s];
    if sec.reloc.0 == 0 || !sec.is_kept() {
        return Ok(Vec::new());
    }
    let Some((file, index)) = r.sh_index(s) else { return Ok(Vec::new()) };
    let Some(f) = tables.get(file.0 as usize) else { return Ok(Vec::new()) };
    let (Some(obj), Some(table)) = (f.obj.as_ref(), f.table.as_ref()) else {
        return Ok(Vec::new());
    };
    let mut buf = Vec::new();
    let recs = read::records(inputs, file, obj, sec.reloc, &mut buf)
        .map_err(|e| fail(inputs, obj, file, index, sec.reloc, &e))?;
    let mut out = Vec::with_capacity(recs.len());
    for i in 0..recs.len() {
        let Some(raw) = recs.get(i) else { continue };
        let Ok(off) = u32::try_from(raw.r_offset) else { continue };
        let Some(p) = split.eh_piece_at(s, off) else { continue };
        let target = record_target(inputs, r, table, defs, file, raw);
        if target != Target::None {
            out.push((p.ordinal, target));
        }
    }
    out.sort_by_key(|e| e.0);
    Ok(out)
}

/// The diagnostic of a relocation section of section `index` that could
/// not be read, at key `(50, file, index)` (I-4).
fn fail(
    inputs: &Inputs<'_>,
    obj: &Object<'_>,
    file: FileId,
    index: SectionIndex,
    rel: SectionIndex,
    e: &ReadFail,
) -> Keyed {
    let path = inputs.files[file].display();
    let name = obj
        .section(rel)
        .ok()
        .and_then(|s| s.name().ok())
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_default();
    let d = e.diagnostic(path.path()).at(Place::section(path, name, 0));
    (Key::new(PHASE, file.0, u64::from(index.0)), d)
}

/// One unit of a round: records `lo..hi` of section `s`'s relocations;
/// the first unit of a section also takes its FDEs' and `SHF_LINK_ORDER`
/// dependents' edges.
#[derive(Debug, Clone, Copy)]
struct Unit {
    s: SectionId,
    lo: usize,
    hi: usize,
}

/// What a unit collects.
enum Out {
    /// A section this unit's swap marked: the next round walks it.
    Marked(SectionId),
    Failed(Box<Keyed>),
}

struct Walk<'w, 't, 's> {
    inputs: &'w Inputs<'s>,
    r: &'w Resolution,
    split: &'w Split,
    tables: &'w [FileFacts<'s>],
    defs: &'w [(Target, bool)],
    facts: &'w [u8],
    link_order: &'w [(SectionId, SectionId)],
    group_of: &'w [(SectionId, u32, u32)],
    group_members: &'w [SectionId],
    by_code: &'w [(SectionId, u32, u32)],
    eh_edges: &'w [Vec<(u32, Target)>],
    marks: &'w Marks<'t>,
}

impl Walk<'_, '_, '_> {
    /// The units of a round (I-8): per section, and per `SCAN_BLOCK`
    /// records of a section with more.
    fn units(&self, round: &[SectionId]) -> Vec<Unit> {
        let mut units = Vec::with_capacity(round.len());
        for &s in round {
            let mut n = 0;
            let row = self.r.sections[s];
            // A section without facts (not read) has its records walked.
            let edges = self.facts.get(s.0 as usize).is_none_or(|b| b & (EDGES | ALLOC) != 0);
            if edges && row.reloc.0 != 0 && row.kind != SectionKind::EhFrame {
                n = self.count(s).unwrap_or(1);
            }
            if n <= SCAN_BLOCK {
                units.push(Unit { s, lo: 0, hi: n });
                continue;
            }
            let mut lo = 0;
            while lo < n {
                let hi = (lo + SCAN_BLOCK).min(n);
                units.push(Unit { s, lo, hi });
                lo = hi;
            }
        }
        units
    }

    /// The record count of `s`'s relocation section; `None` when its
    /// header cannot be read (the walk then reports it).
    fn count(&self, s: SectionId) -> Option<usize> {
        let (file, _) = self.r.sh_index(s)?;
        let obj = self.tables.get(file.0 as usize)?.obj.as_ref()?;
        read::count(obj, self.r.sections[s].reloc).ok()
    }

    fn walk(&self, u: Unit, out: &mut Vec<Out>) {
        let mut marked = Vec::new();
        if let Err(e) = self.records(u, &mut marked) {
            out.push(Out::Failed(Box::new(e)));
        }
        if u.lo == 0 {
            self.frames(u.s, &mut marked);
            let from = self.link_order.partition_point(|&(t, _)| t < u.s);
            for &(t, dep) in &self.link_order[from..] {
                if t != u.s {
                    break;
                }
                self.marks.at(dep, WHOLE, &mut marked);
            }
            // I-7: the members of a section group live together.
            let at = self.group_of.partition_point(|&(m, _, _)| m < u.s);
            if let Some(&(m, start, len)) = self.group_of.get(at)
                && m == u.s
            {
                let span = start as usize..(start + len) as usize;
                for &mate in self.group_members.get(span).unwrap_or_default() {
                    self.marks.at(mate, WHOLE, &mut marked);
                }
            }
        }
        out.extend(marked.into_iter().map(Out::Marked));
    }

    fn records(&self, u: Unit, marked: &mut Vec<SectionId>) -> Result<(), Keyed> {
        if u.hi == 0 {
            return Ok(());
        }
        let row = self.r.sections[u.s];
        let internal = |what: &str| {
            let d = Diagnostic::error(Kind::InternalError {
                what: format!("liveness: section {} {what}", u.s.0),
            });
            (Key::new(PHASE, 0, u64::from(u.s.0)), d)
        };
        let Some((file, index)) = self.r.sh_index(u.s) else {
            return Err(internal("has relocations but no file"));
        };
        let Some(obj) = self.tables.get(file.0 as usize).and_then(|f| f.obj.as_ref()) else {
            return Err(internal("has relocations but its file is not an object"));
        };
        let f = &self.tables[file.0 as usize];
        let mut buf = Vec::new();
        let recs = read::records(self.inputs, file, obj, row.reloc, &mut buf)
            .map_err(|e| fail(self.inputs, obj, file, index, row.reloc, &e))?;
        if recs.len() < u.hi {
            let e = ReadFail::Malformed(format!(
                "relocation section {} has {} records, its header says {}",
                row.reloc,
                recs.len(),
                u.hi
            ));
            return Err(fail(self.inputs, obj, file, index, row.reloc, &e));
        }
        let Some(table) = f.table.as_ref() else {
            // No symbol table: every record must name symbol 0 (the scan
            // checks it); none is an edge.
            return Ok(());
        };
        for i in u.lo..u.hi {
            let Some(raw) = recs.get(i) else { continue };
            let t = record_target(self.inputs, self.r, table, self.defs, file, raw);
            self.marks.target(t, marked);
        }
        Ok(())
    }

    /// I-7: the edges of `code`'s live FDEs and of their CIEs.
    fn frames(&self, code: SectionId, marked: &mut Vec<SectionId>) {
        let from = self.by_code.partition_point(|&(c, _, _)| c < code);
        for &(c, row, ord) in &self.by_code[from..] {
            if c != code {
                break;
            }
            let Some(sr) = self.split.eh_of.get(row as usize) else { continue };
            let cie =
                self.split.eh_piece.get(sr.pieces.start as usize + ord as usize).map(|p| p.cie);
            let edges = &self.eh_edges[row as usize];
            for want in [Some(ord), cie] {
                let Some(want) = want else { continue };
                let lo = edges.partition_point(|e| e.0 < want);
                for &(o, t) in &edges[lo..] {
                    if o != want {
                        break;
                    }
                    self.marks.target(t, marked);
                }
            }
        }
    }
}
