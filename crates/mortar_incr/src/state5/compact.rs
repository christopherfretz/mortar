//! The compaction (`spec/incr.md` IN-18f; IN-77's W5/C4 when the patch does
//! not fit, or `S` is hard-linked or a symbolic link). WP-213 (U-1): the
//! entry point [`compact`]; filled by U-2 (WP-214). Its read of every
//! block (IN-18f step 1) and steps 2-4 are `super::plan_patch`'s, before
//! W1 (WP-213 follow-up, Q-1): [`apply`] (step 2) and [`normalize`]
//! (steps 3-4, also the full link's writer: a compacted state equals the
//! capture's by construction). [`compact`] is step 5: writes only.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use mortar_core::incr::IncrReason;
use mortar_core::{FileId, SymbolId};

use super::write::{PatchPlan, Planned};
use super::{DiagRaw, Keyed, StrFn, corrupt, diag_raw, state_corrupt};
use crate::format::{NONE, StatRow, StateError, Str, index_kind, patch_kind, stat_tag};
use crate::ops::{LockMode, WriteFailure, WriteOps};
use crate::update::diags::{DiagRow, Subject};
use crate::update::rows::{
    FileIndexRow, FileTable, KeyRefRow, RowRange, edge_kind, file_flags, sym_kind,
};
use crate::update::testing::MemoryState;
use crate::update::view::{KeyEdits, StatePatch, StateView};

type Rows<T> = Result<T, IncrReason>;

// ---- step 2: the patch in memory ----------------------------------------------------

/// Apply `p` to `m` (a whole decode, `super::read::decode_core`: the
/// state's own id space) the way [`super::plan_patch`]'s in-place editor
/// would (IN-18f step 2): the same row numbers for every row another
/// table names (`SECTIONS`: a file's new rows in its old range when they
/// fit its capacity, else at the table's extent; `FRAGS`: a new row at its
/// sorted place in the append area). `frags_aux`: `FRAGS`' sorted body.
///
/// Public for the tests (patch-then-read equals rewrite).
///
/// # Errors
/// `state-corrupt`: a row the patch names that `m` does not hold.
pub fn apply(m: &mut MemoryState, p: &StatePatch, frags_aux: usize) -> Rows<()> {
    macro_rules! whole {
        ($($f:ident),*) => {$(
            if let Some(v) = &p.$f {
                m.$f = v.clone();
            }
        )*};
    }
    whole!(
        probes, outsecs, chunks, order, segments, slots, sizes, sym_place, defined, patches,
        redirects, sized, leaves
    );
    if let Some(l) = &p.layout {
        m.layout = l.clone();
    }
    if let Some(meta) = &p.meta {
        m.meta = meta.clone();
    }
    m.meta.output_synced = true;
    for (f, row) in &p.files_set {
        *m.files
            .get_mut(f.0 as usize)
            .ok_or_else(|| corrupt(format!("FILES: the patch sets row {}", f.0)))? = row.clone();
    }
    for row in &p.files_append {
        m.files.push(row.clone());
        m.file_index.push(FileIndexRow::default());
    }
    for (i, row) in &p.stat_set {
        *m.stat
            .get_mut(*i as usize)
            .ok_or_else(|| corrupt(format!("STAT: the patch sets row {i}")))? = row.clone();
    }
    // Appended STAT rows go after the extent, the output's row stays.
    m.stat.extend(p.stat_append.iter().cloned());
    m.strings.extend_from_slice(&p.strings);
    // Key-keyed edits first: a FRAGS insert shifts the old FRAG_AT rows'
    // references; the patch's own FRAG_AT rows are in the new numbering.
    let s_pool = m.strings.clone();
    let s: StrFn<'_> = &|x: Str| pool_string(&s_pool, x);
    keyed(&mut m.key_refs, &p.key_refs, s, 0, |_| {})?;
    keyed(&mut m.groups, &p.groups, s, 0, |_| {})?;
    let mut inserted = Vec::new();
    keyed(&mut m.frags, &p.frags, s, frags_aux, |at| inserted.push(at))?;
    for at in inserted {
        for (_, r) in &mut m.frag_at {
            if r.frag >= at as u64 {
                r.frag += 1;
            }
        }
    }
    keyed(&mut m.holes, &p.holes, s, 0, |_| {})?;
    keyed(&mut m.commons, &p.commons, s, 0, |_| {})?;
    keyed(&mut m.names, &p.names, s, 0, |_| {})?;
    if !(p.odr_classes.put.is_empty()
        && p.odr_classes.delete.is_empty()
        && p.odr_refs.put.is_empty()
        && p.odr_refs.delete.is_empty())
    {
        return Err(corrupt(
            "the patch has ODR_CLASSES or ODR_REFS rows: format 5 holds none (WP-236)",
        ));
    }
    diags(&mut m.diags, &p.diags)?;
    // File-keyed.
    for fr in &p.ranges {
        let f = fr.file.0 as usize;
        if f >= m.files.len() {
            return Err(corrupt(format!("FILE_INDEX: the patch's file {f} is past FILES")));
        }
        if fr.gone {
            m.files[f].flags |= file_flags::GONE;
        }
        let idx = &mut m.file_index[f];
        let was_gone = idx.gone;
        if let Some(rows) = &fr.sections {
            let old = idx.ranges[FileTable::Sections as usize];
            let n = u32::try_from(rows.len()).map_err(|_| corrupt("a range over 2^32"))?;
            let filler = crate::format::SectionRow { file: NONE, ..Default::default() };
            if !was_gone && n <= old.capacity && old.capacity != 0 {
                for i in 0..old.count {
                    m.sections[(old.first + i) as usize] = filler.clone();
                }
                for (i, r) in rows.iter().enumerate() {
                    m.sections[old.first as usize + i] = r.clone();
                }
                idx.ranges[FileTable::Sections as usize].count = n;
            } else {
                if !was_gone {
                    for i in 0..old.count {
                        m.sections[(old.first + i) as usize] = filler.clone();
                    }
                }
                let capacity = if n == 0 { 0 } else { super::range_capacity(n) };
                let first =
                    u32::try_from(m.sections.len()).map_err(|_| corrupt("a table over 2^32"))?;
                m.sections.extend(rows.iter().cloned());
                m.sections.resize(m.sections.len() + (capacity - n) as usize, filler);
                idx.ranges[FileTable::Sections as usize] = RowRange { first, count: n, capacity };
            }
        }
        macro_rules! range {
            ($field:ident, $t:expr) => {
                if let Some(rows) = &fr.$field {
                    m.$field.retain(|(g, _)| *g != fr.file);
                    m.$field.extend(rows.iter().map(|r| (fr.file, r.clone())));
                    idx.ranges[$t as usize].count = u32::try_from(rows.len()).unwrap_or(u32::MAX);
                }
            };
        }
        range!(mentions, FileTable::Mentions);
        range!(index, FileTable::Index);
        range!(edges, FileTable::Edges);
        range!(frag_at, FileTable::FragAt);
        range!(eh_records, FileTable::EhRecords);
        range!(regions, FileTable::Regions);
        if let Some(st) = &fr.symtab {
            m.symtab.retain(|(g, _)| *g != fr.file);
            m.symtab.extend(st.iter().map(|r| (fr.file, *r)));
        }
        if fr.gone {
            idx.gone = true;
            for i in 0..idx.ranges[FileTable::Sections as usize].count {
                let at = (idx.ranges[FileTable::Sections as usize].first + i) as usize;
                m.sections[at] = crate::format::SectionRow { file: NONE, ..Default::default() };
            }
            m.mentions.retain(|(g, _)| *g != fr.file);
            m.index.retain(|(g, _)| *g != fr.file);
            m.edges.retain(|(g, _)| *g != fr.file);
            m.frag_at.retain(|(g, _)| *g != fr.file);
            m.eh_records.retain(|(g, _)| *g != fr.file);
            m.symtab.retain(|(g, _)| *g != fr.file);
            m.regions.retain(|(g, _)| *g != fr.file);
        }
    }
    // Key-parallel.
    let n = m.symbols.len();
    for (id, r) in &p.symbols.set {
        *m.symbols
            .get_mut(id.0 as usize)
            .ok_or_else(|| corrupt("SYMBOLS: the patch sets a row past the table"))? = *r;
    }
    for (id, r) in &p.resolved.set {
        *m.resolved
            .get_mut(id.0 as usize)
            .ok_or_else(|| corrupt("RESOLVED: the patch sets a row past the table"))? = *r;
    }
    for (id, r) in &p.symtab_keys.set {
        *m.symtab_keys
            .get_mut(id.0 as usize)
            .ok_or_else(|| corrupt("SYMTAB_KEYS: the patch sets a row past the table"))? = *r;
    }
    let add = p.symbols.append.len();
    if p.resolved.append.len() > add || p.symtab_keys.append.len() > add {
        return Err(corrupt("a key-parallel table appended past SYMBOLS"));
    }
    m.symbols.extend(p.symbols.append.iter().copied());
    m.resolved.extend(p.resolved.append.iter().copied());
    m.resolved
        .resize(n + add, crate::update::rows::ResolvedRow { section: NONE, ..Default::default() });
    m.symtab_keys.extend(p.symtab_keys.append.iter().copied());
    m.symtab_keys.resize(n + add, crate::update::rows::SymtabKeyRow { index: NONE, strtab: 0 });
    Ok(())
}

fn pool_string(pool: &[u8], x: Str) -> Rows<Vec<u8>> {
    let start = x.off as usize;
    start
        .checked_add(x.len as usize)
        .and_then(|end| pool.get(start..end))
        .map(<[u8]>::to_vec)
        .ok_or_else(|| corrupt("STRINGS: a string out of range"))
}

/// A key-keyed table's edits in memory, with the in-place editor's rule:
/// deletes set the tomb, a put of a present key replaces its row, another
/// put is inserted at its sorted place among the rows past `aux`
/// (`inserted` is told each position).
fn keyed<T: Keyed>(
    rows: &mut Vec<T>,
    e: &KeyEdits<T>,
    s: StrFn<'_>,
    aux: usize,
    mut inserted: impl FnMut(usize),
) -> Rows<()> {
    if e.put.is_empty() && e.delete.is_empty() {
        return Ok(());
    }
    let mut keys: Vec<T::K> = rows.iter().map(|r| r.key(s)).collect::<Rows<_>>()?;
    let aux = aux.min(rows.len());
    for (del, row) in e.delete.iter().map(|r| (true, r)).chain(e.put.iter().map(|r| (false, r))) {
        let key = row.key(s)?;
        let found = keys.iter().position(|k| *k == key);
        match (del, found) {
            (true, None) => {
                return Err(corrupt(format!(
                    "a key-keyed delete of a key the state lacks ({key:?})"
                )));
            }
            (true, Some(i)) if rows[i].tomb() => {
                return Err(corrupt(format!("a key-keyed delete of a dead key ({key:?})")));
            }
            (true, Some(i)) => {
                rows[i].set_tomb(true);
                if !rows[i].tomb() {
                    return Err(corrupt(
                        "a delete from a table whose rows are not deleted (NAMES)",
                    ));
                }
            }
            (false, Some(i)) => {
                let mut r = row.clone();
                r.set_tomb(false);
                rows[i] = r;
            }
            (false, None) => {
                let at = aux + keys[aux..].partition_point(|k| *k < key);
                let mut r = row.clone();
                r.set_tomb(false);
                rows.insert(at, r);
                keys.insert(at, key);
                inserted(at);
            }
        }
    }
    Ok(())
}

/// `DIAGS`' edits (keyed by the diagnostic's key, code and text).
fn diags(rows: &mut Vec<DiagRow>, e: &KeyEdits<DiagRow>) -> Rows<()> {
    let key = |d: &DiagRow| (d.key.phase, d.key.unit, d.key.seq, d.code.clone(), d.text.clone());
    for d in &e.delete {
        let k = key(d);
        let r = rows
            .iter_mut()
            .find(|r| key(r) == k && !r.tomb)
            .ok_or_else(|| corrupt(format!("DIAGS: a delete of a key the state lacks ({k:?})")))?;
        r.tomb = true;
    }
    for d in &e.put {
        let k = key(d);
        let mut d = d.clone();
        d.tomb = false;
        match rows.iter_mut().find(|r| key(r) == k) {
            Some(r) => *r = d,
            None => rows.push(d),
        }
    }
    rows.sort_by(|a, b| key(a).cmp(&key(b)));
    Ok(())
}

// ---- steps 3 and 4: drop and renumber ------------------------------------------------

/// IN-18f steps 3-4 over `m` (any id space: a capture's or a whole
/// decode's): drop `GONE` files and their rows, tombstones, `GONE` keys
/// and unnamed strings; renumber `FILES` by `rank` (new `FileId` =
/// position, `rank` = `FileId << 20`), `SYMBOLS` in IN-18e's one row
/// order, `STRINGS` afresh (interned in table order, each distinct string
/// once); `SECTIONS` (by file, then position) and `FRAGS` (by identity)
/// are numbered by [`super::write::encode`]. `STAT`'s output row moves
/// last. The full link's writer is `encode(normalize(capture))`, the
/// compaction's `encode(normalize(apply(decode(S))))`: equal by
/// construction for equal rows (IN-18f's "byte for byte").
///
/// # Errors
/// `state-corrupt` for a reference with no image in a map (IN-18f step 4:
/// a defect).
#[allow(clippy::too_many_lines)] // one block per referencing table
pub fn normalize(m: &MemoryState) -> Result<MemoryState, StateError> {
    let bad = |what: &str| state_corrupt(format!("compaction: {what} has no image"));
    let old_pool = &m.strings;
    let s = |x: Str| pool_string(old_pool, x).map_err(super::read::to_state_error);
    // FILES by rank; GONE files dropped.
    let gone = |f: usize| {
        m.files[f].flags & file_flags::GONE != 0 || m.file_index.get(f).is_some_and(|i| i.gone)
    };
    let mut live: Vec<usize> = (0..m.files.len()).filter(|&f| !gone(f)).collect();
    live.sort_by_key(|&f| (m.files[f].rank, f));
    let mut file_map = vec![NONE; m.files.len()];
    for (new, &old) in live.iter().enumerate() {
        file_map[old] = u32::try_from(new).map_err(|_| state_corrupt("FILES over 2^32"))?;
    }
    let file = |f: u32| -> Result<u32, StateError> {
        file_map
            .get(f as usize)
            .copied()
            .filter(|&v| v != NONE)
            .ok_or_else(|| bad(&format!("FILES row {f}")))
    };
    let fid = |f: FileId| file(f.0).map(FileId);
    // SYMBOLS in the stated order; GONE keys and the locals of gone files
    // dropped.
    let mut globals = Vec::new();
    let mut unmentioned = Vec::new();
    let mut locals = Vec::new();
    for (i, r) in m.symbols.iter().enumerate() {
        match r.kind {
            sym_kind::GONE => {}
            sym_kind::LOCAL => {
                if let Ok(f) = file(r.file) {
                    locals.push(((f, r.at_index), i));
                }
            }
            _ if r.at_file == NONE => unmentioned.push(((s(r.name)?, s(r.version)?), i)),
            _ => globals.push(((file(r.at_file)?, r.at_index), i)),
        }
    }
    globals.sort();
    unmentioned.sort();
    locals.sort();
    let order: Vec<usize> = globals
        .iter()
        .map(|x| x.1)
        .chain(unmentioned.iter().map(|x| x.1))
        .chain(locals.iter().map(|x| x.1))
        .collect();
    let mut sym_map = vec![NONE; m.symbols.len()];
    for (new, &old) in order.iter().enumerate() {
        sym_map[old] = u32::try_from(new).map_err(|_| state_corrupt("SYMBOLS over 2^32"))?;
    }
    let sym = |x: u32| -> Result<u32, StateError> {
        sym_map
            .get(x as usize)
            .copied()
            .filter(|&v| v != NONE)
            .ok_or_else(|| bad(&format!("SYMBOLS row {x}")))
    };
    // STRINGS afresh.
    let mut pool: Vec<u8> = Vec::new();
    let mut seen: HashMap<Vec<u8>, Str> = HashMap::new();
    let mut intern = |x: Str| -> Result<Str, StateError> {
        let b = s(x)?;
        if b.is_empty() {
            return Ok(Str::default());
        }
        if let Some(v) = seen.get(&b) {
            return Ok(*v);
        }
        let off = u32::try_from(pool.len()).map_err(|_| state_corrupt("STRINGS over 4 GiB"))?;
        let v = Str {
            off,
            len: u32::try_from(b.len()).map_err(|_| state_corrupt("a string over 4 GiB"))?,
        };
        pool.extend_from_slice(&b);
        seen.insert(b, v);
        Ok(v)
    };
    let mut n = MemoryState { plan_hash: m.plan_hash, ..MemoryState::default() };
    // In kind order (the pool's order).
    n.meta = m.meta.clone();
    n.meta.version = intern(m.meta.version)?;
    // STAT: matched by path; a gone file's row dropped unless a live
    // file has its path; the output's row last.
    let out = m.stat.iter().rposition(|r| r.tag == stat_tag::OUTPUT);
    let mut live_paths = std::collections::HashSet::new();
    let mut gone_paths = std::collections::HashSet::new();
    for (f, r) in m.files.iter().enumerate() {
        if gone(f) { &mut gone_paths } else { &mut live_paths }.insert(s(r.path)?);
    }
    for (i, r) in m.stat.iter().enumerate() {
        if r.tag == stat_tag::INPUT {
            let p = s(r.path)?;
            if gone_paths.contains(&p) && !live_paths.contains(&p) {
                continue;
            }
        }
        if Some(i) != out {
            n.stat.push(StatRow { path: intern(r.path)?, fstype: intern(r.fstype)?, ..r.clone() });
        }
    }
    if let Some(i) = out {
        let r = &m.stat[i];
        n.stat.push(StatRow { path: intern(r.path)?, fstype: intern(r.fstype)?, ..r.clone() });
    }
    for &f in &live {
        let r = &m.files[f];
        let new = file(u32::try_from(f).unwrap_or(NONE))?;
        n.files.push(crate::update::rows::FileRow {
            path: intern(r.path)?,
            member_name: intern(r.member_name)?,
            rank: u64::from(new) << 20,
            ..r.clone()
        });
    }
    n.file_index = vec![FileIndexRow::default(); live.len()];
    for r in &m.probes {
        n.probes.push(crate::format::ProbeRow { path: intern(r.path)?, ..r.clone() });
    }
    for r in &m.outsecs {
        n.outsecs.push(crate::format::OutSecRow { name: intern(r.name)?, ..r.clone() });
    }
    for &i in &order {
        let r = &m.symbols[i];
        let (f, at) = if r.kind == sym_kind::LOCAL {
            (file(r.file)?, file(r.at_file)?)
        } else {
            (r.file, if r.at_file == NONE { NONE } else { file(r.at_file)? })
        };
        n.symbols.push(crate::update::rows::SymbolRow {
            file: f,
            at_file: at,
            name: intern(r.name)?,
            version: intern(r.version)?,
            ..*r
        });
        n.resolved.push(*m.resolved.get(i).ok_or_else(|| bad("a RESOLVED row"))?);
        n.symtab_keys.push(*m.symtab_keys.get(i).ok_or_else(|| bad("a SYMTAB_KEYS row"))?);
    }
    // SECTIONS: live files' rows by (new file, position); fillers dropped.
    let mut secs: Vec<(u32, usize)> = Vec::new();
    for (i, r) in m.sections.iter().enumerate() {
        if r.file != NONE
            && let Ok(f) = file(r.file)
        {
            secs.push((f, i));
        }
    }
    secs.sort_unstable();
    let mut sec_map = vec![NONE; m.sections.len()];
    for (new, &(f, old)) in secs.iter().enumerate() {
        sec_map[old] = u32::try_from(new).map_err(|_| state_corrupt("SECTIONS over 2^32"))?;
        let r = &m.sections[old];
        n.sections.push(crate::format::SectionRow {
            file: f,
            anchor: intern(r.anchor)?,
            name: intern(r.name)?,
            ..r.clone()
        });
    }
    let sec = |x: u32| -> Result<u32, StateError> {
        if x == NONE {
            return Ok(NONE);
        }
        sec_map
            .get(x as usize)
            .copied()
            .filter(|&v| v != NONE)
            .ok_or_else(|| bad(&format!("SECTIONS row {x}")))
    };
    for r in &m.chunks {
        n.chunks.push(crate::format::ChunkRow { name: intern(r.name)?, ..r.clone() });
    }
    n.order = m.order.clone();
    n.segments = m.segments.clone();
    for r in &m.slots {
        n.slots.push(crate::format::SlotRow { symbol: sym(r.symbol)?, ..*r });
    }
    n.sizes = m.sizes.clone();
    for r in &m.sym_place {
        n.sym_place.push(crate::format::SymPlaceRow { symbol: sym(r.symbol)?, ..*r });
    }
    for r in &m.defined {
        n.defined.push(crate::format::DefinedRow { symbol: sym(r.symbol)?, ..*r });
    }
    for r in &m.patches {
        let (symbol, section) = if r.kind == patch_kind::E843419 {
            (r.symbol, sec(r.section)?)
        } else {
            (sym(r.symbol)?, r.section)
        };
        n.patches.push(crate::format::PatchRow { symbol, section, ..*r });
    }
    for r in &m.redirects {
        n.redirects.push(crate::format::RedirectRow { section: sec(r.section)?, ..*r });
    }
    n.sized = m.sized.clone();
    n.layout = m.layout.clone();
    n.leaves = m.leaves.clone();
    // File-keyed (sorted by new file, each file's rows in their order).
    fn per_file<T: Clone>(
        rows: &[(FileId, T)],
        fid: &dyn Fn(FileId) -> Result<FileId, StateError>,
        gone: &dyn Fn(usize) -> bool,
        mut map: impl FnMut(&T) -> Result<T, StateError>,
    ) -> Result<Vec<(FileId, T)>, StateError> {
        let mut v = Vec::with_capacity(rows.len());
        for (i, (f, r)) in rows.iter().enumerate() {
            if gone(f.0 as usize) {
                continue;
            }
            v.push((fid(*f)?, i, map(r)?));
        }
        v.sort_by_key(|x| (x.0.0, x.1));
        Ok(v.into_iter().map(|(f, _, r)| (f, r)).collect())
    }
    n.mentions = per_file(&m.mentions, &fid, &gone, |r| {
        Ok(crate::update::rows::MentionRow {
            symbol: sym(r.symbol)?,
            section: sec(r.section)?,
            ..*r
        })
    })?;
    n.index = per_file(&m.index, &fid, &gone, |r| {
        let target = match r.kind {
            index_kind::GLOBAL | index_kind::SLOT => sym(r.target)?,
            _ => sec(r.target)?,
        };
        Ok(crate::format::IndexRow { unit: sec(r.unit)?, target, ..*r })
    })?;
    n.edges = per_file(&m.edges, &fid, &gone, |r| {
        let target = match r.kind {
            edge_kind::LOCAL | edge_kind::FDE_SECTION | edge_kind::NONALLOC_SECTION => {
                sec(r.target)?
            }
            _ => sym(r.target)?,
        };
        Ok(crate::update::rows::EdgeRow { source: sec(r.source)?, target, ..*r })
    })?;
    // FRAG_AT keeps naming FRAGS rows of `m`; encode renumbers both
    // (tombstoned frags dropped below, so map them first).
    let mut frag_map = vec![u64::MAX; m.frags.len()];
    let mut k = 0u64;
    for (i, r) in m.frags.iter().enumerate() {
        if !r.tomb {
            frag_map[i] = k;
            k += 1;
            n.frags.push(*r);
        }
    }
    n.frag_at = per_file(&m.frag_at, &fid, &gone, |r| {
        let frag = frag_map
            .get(usize::try_from(r.frag).unwrap_or(usize::MAX))
            .copied()
            .filter(|&v| v != u64::MAX)
            .ok_or_else(|| bad(&format!("FRAGS row {}", r.frag)))?;
        Ok(crate::format::FragAtRow { section: sec(r.section)?, frag, ..*r })
    })?;
    n.eh_records = per_file(&m.eh_records, &fid, &gone, |r| {
        Ok(crate::format::EhRow { section: sec(r.section)?, ..r.clone() })
    })?;
    n.symtab = per_file(&m.symtab, &fid, &gone, |r| Ok(*r))?;
    n.regions = per_file(&m.regions, &fid, &gone, |r| Ok(*r))?;
    // Key-keyed: tombstones dropped, references mapped (encode sorts).
    for r in m.groups.iter().filter(|r| !r.tomb) {
        n.groups.push(crate::update::rows::GroupRow { signature: intern(r.signature)?, ..*r });
    }
    for r in m.key_refs.iter().filter(|r| !r.tomb()) {
        n.key_refs.push(KeyRefRow { key: sym(r.key)?, file: file(r.file)?, ..*r });
    }
    for r in m.commons.iter().filter(|r| !r.tomb) {
        n.commons.push(crate::update::rows::CommonRow {
            key: sym(r.key)?,
            file: file(r.file)?,
            ..*r
        });
    }
    n.holes = m.holes.iter().filter(|r| !r.tomb).copied().collect();
    for d in m.diags.iter().filter(|d| !d.tomb) {
        let mut subjects = Vec::with_capacity(d.subjects.len());
        for x in &d.subjects {
            subjects.push(match x {
                Subject::File(f) => Subject::File(fid(*f)?),
                Subject::Key(k) => Subject::Key(SymbolId(sym(k.0)?)),
                Subject::OdrKey { name, class } => {
                    Subject::OdrKey { name: intern(*name)?, class: *class }
                }
                other => other.clone(),
            });
        }
        n.diags.push(DiagRow { subjects, ..d.clone() });
    }
    drop(intern);
    // NAMES: the globals' index, rebuilt (it is derived from SYMBOLS).
    for (row, r) in n.symbols.iter().enumerate() {
        if matches!(r.kind, sym_kind::GLOBAL | sym_kind::VERSIONED) {
            let mut b = s_new(&pool, r.name)?;
            b.push(0);
            b.extend_from_slice(&s_new(&pool, r.version)?);
            n.names.push(crate::update::rows::NameRow {
                hash: mortar_core::hash::hash_bytes(&b),
                row: u32::try_from(row).unwrap_or(NONE),
            });
        }
    }
    n.strings = pool;
    Ok(n)
}

fn s_new(pool: &[u8], x: Str) -> Result<Vec<u8>, StateError> {
    pool_string(pool, x).map_err(super::read::to_state_error)
}

/// `DIAGS`' rows as stored, for a test's comparison.
#[allow(dead_code)]
pub(crate) fn diags_raw(m: &MemoryState) -> Vec<DiagRaw> {
    let mut pool = Vec::new();
    let mut add = |b: &[u8]| {
        let off = u32::try_from(pool.len()).unwrap_or(NONE);
        pool.extend_from_slice(b);
        Str { off, len: u32::try_from(b.len()).unwrap_or(NONE) }
    };
    m.diags.iter().map(|d| diag_raw(d, &mut add)).collect()
}

// ---- step 5: the write ---------------------------------------------------------------

/// `dir(S)/.<base(S)>.mortar-compact`.
fn temporary(state_path: &Path) -> PathBuf {
    let mut base = OsString::from(".");
    base.push(state_path.file_name().unwrap_or_default());
    base.push(".mortar-compact");
    state_path.with_file_name(base)
}

/// Write the whole new state the plan holds (IN-18f steps 2-5, read and
/// renumbered by `plan_patch`), with the binding `output` (as
/// `super::patch`), to `dir(state_path)/.<base(state_path)>.mortar-compact`
/// (created and `flock`ed through `ops`), `fdatasync`, rename over
/// `state_path`, `fsync` the directory. Writes only. Called by U-7's
/// `write` at W5/C4 when `plan.compacts()`.
///
/// # Errors
/// [`WriteFailure::Failed`] for an I/O error (IN-77e: the temporary is
/// removed, the flag stays set); [`WriteFailure::Fallback`]
/// (`state-corrupt`) for a plan that is not a compaction's.
pub fn compact(
    view: &StateView<'_>,
    plan: &PatchPlan,
    output: &StatRow,
    ops: &mut dyn WriteOps,
    state_path: &Path,
) -> Result<(), WriteFailure> {
    let Some(Planned::Compact { core, state }) = plan.inner.as_deref() else {
        return Err(WriteFailure::Fallback(corrupt("an in-place plan given to compact")));
    };
    if !super::read::core_of(view).is_some_and(|c| std::sync::Arc::ptr_eq(&c, core)) {
        return Err(WriteFailure::Fallback(corrupt("the plan is not this view's")));
    }
    let bytes = compacted_bytes(state, output)
        .map_err(|e| WriteFailure::Fallback(corrupt(e.to_string())))?;
    let tmp = temporary(state_path);
    let failed = |step: &'static str| move |error| WriteFailure::Failed { step, error };
    let file = ops.create_new(&tmp).map_err(failed("compact"))?;
    let result = (|| {
        ops.flock(&file, LockMode::Exclusive)?;
        ops.pwrite(&file, 0, &bytes)?;
        ops.fdatasync(&file)?;
        ops.rename(&tmp, state_path)?;
        let dir =
            state_path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        ops.fsync_dir(dir)
    })();
    if let Err(error) = result {
        let _ = ops.unlink(&tmp);
        return Err(WriteFailure::Failed { step: "compact", error });
    }
    Ok(())
}

/// The compacted state's bytes with the binding `output` (tests,
/// `incr_state`): `state` is the patched state in the old id space
/// ([`apply`]'s), `output`'s strings name its pool (the old pool and the
/// patch's `strings`); the output's row replaces the old one, then
/// [`normalize`] (CPU only, no read) and the writer.
///
/// # Errors
/// The writer's.
pub(crate) fn compacted_bytes(
    state: &MemoryState,
    output: &StatRow,
) -> Result<Vec<u8>, StateError> {
    let mut m = state.clone();
    let mut row = output.clone();
    row.tag = stat_tag::OUTPUT;
    if let Some(i) = m.stat.iter().rposition(|r| r.tag == stat_tag::OUTPUT) {
        m.stat.remove(i);
    }
    m.stat.push(row);
    let mut n = normalize(&m)?;
    n.meta.compactions = n.meta.compactions.saturating_add(1);
    super::write::encode(&n)
}

/// The compacted bytes of a plan (tests, `incr_state`).
#[must_use]
pub fn plan_bytes(plan: &PatchPlan, output: &StatRow) -> Option<Vec<u8>> {
    let Some(Planned::Compact { state, .. }) = plan.inner.as_deref() else { return None };
    compacted_bytes(state, output).ok()
}
