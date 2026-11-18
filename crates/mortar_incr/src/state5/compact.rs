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

use mortar_core::incr::{IncrReason, OptionKeys};
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
#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // one block per class
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
    // Key-keyed edits (FRAG_USE names FRAGS by key: no row numbers).
    let s_pool = m.strings.clone();
    let s: StrFn<'_> = &|x: Str| pool_string(&s_pool, x);
    keyed(&mut m.key_refs, &p.key_refs, s, 0, |_| {})?;
    keyed(&mut m.groups, &p.groups, s, 0, |_| {})?;
    keyed(&mut m.frags, &p.frags, s, frags_aux, |_| {})?;
    keyed(&mut m.holes, &p.holes, s, 0, |_| {})?;
    keyed(&mut m.commons, &p.commons, s, 0, |_| {})?;
    keyed(&mut m.copies, &p.copies, s, 0, |_| {})?;
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
        idx.first_global = fr.first_global;
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
        range!(frag_uses, FileTable::FragUse);
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
            m.frag_uses.retain(|(g, _)| *g != fr.file);
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
    rows.sort_by_key(key);
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
#[allow(clippy::too_many_lines, clippy::many_single_char_names, clippy::items_after_statements)] // one block per referencing table
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
    // SYMBOLS in the stated order; GONE keys, dead keys (IN-71b "Live
    // keys", by [`live_keys`]; no option set is stored: see there) and the
    // locals of gone files dropped.
    let alive = live_keys(m, &OptionKeys::default())?;
    let mut globals = Vec::new();
    let mut unmentioned = Vec::new();
    let mut locals = Vec::new();
    for (i, r) in m.symbols.iter().enumerate() {
        match r.kind {
            sym_kind::GONE => {}
            sym_kind::GLOBAL | sym_kind::VERSIONED if !alive[i] => {}
            sym_kind::LOCAL => {
                if let Ok(f) = file(r.file) {
                    locals.push(((f, r.at_index), i));
                }
            }
            _ if r.at_file == NONE => unmentioned.push(((s(r.name)?, s(r.version)?), i)),
            _ => globals.push(((file(r.at_file)?, r.at_index), i)),
        }
    }
    globals.sort_unstable();
    unmentioned.sort();
    locals.sort_unstable();
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
            properties: intern(r.properties)?,
            rank: u64::from(new) << 20,
            ..r.clone()
        });
    }
    n.file_index = live
        .iter()
        .map(|&f| FileIndexRow {
            first_global: m.file_index.get(f).map_or(0, |i| i.first_global),
            ..FileIndexRow::default()
        })
        .collect();
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
        // a key-parallel table shorter than SYMBOLS (a format-4 capture
        // has no SYMTAB_KEYS) reads as its empty rows, as the writer pads
        n.resolved.push(
            m.resolved.get(i).copied().unwrap_or(crate::update::rows::ResolvedRow {
                section: NONE,
                ..Default::default()
            }),
        );
        n.symtab_keys.push(
            m.symtab_keys
                .get(i)
                .copied()
                .unwrap_or(crate::update::rows::SymtabKeyRow { index: NONE, strtab: 0 }),
        );
    }
    // FILE_INDEX.locals (seam 6): the locals are the sorted tail, each
    // file's one run (by new file, then index).
    for (row, r) in n.symbols.iter().enumerate() {
        if r.kind != sym_kind::LOCAL {
            continue;
        }
        let row = u32::try_from(row).map_err(|_| state_corrupt("SYMBOLS over 2^32"))?;
        let Some(idx) = n.file_index.get_mut(r.file as usize) else {
            return Err(bad(&format!("a local's file {}", r.file)));
        };
        if idx.locals.count == 0 {
            idx.locals.first = row;
        }
        idx.locals.count += 1;
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
    n.order.clone_from(&m.order);
    n.segments.clone_from(&m.segments);
    for r in &m.slots {
        n.slots.push(crate::update::rows::SlotRow { symbol: sym(r.symbol)?, ..*r });
    }
    n.sizes.clone_from(&m.sizes);
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
    n.sized.clone_from(&m.sized);
    n.layout = m.layout.clone();
    n.leaves.clone_from(&m.leaves);
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
    // FRAGS and FRAG_USE (WP-239): keyed by (out, class, hash), no row of
    // a `.debug_*` output section (R-1), tombstones dropped; `refs` is
    // the number of live files with a `FRAG_USE` row of the key (a row
    // with none stays: a dead range, IN-72 item 6). A `FRAG_USE` key with
    // no `FRAGS` row, or two live `FRAGS` rows of one key, is a defect
    // (the capture's refusal is `state-collision`, [`check_capture`]).
    let mut debug = Vec::with_capacity(m.outsecs.len());
    for o in &m.outsecs {
        debug.push(s(o.name)?.starts_with(b".debug_"));
    }
    let is_debug = |out: u32| debug.get(out as usize).copied().unwrap_or(false);
    let uses = per_file(&m.frag_uses, &fid, &gone, |r| Ok(*r))?;
    let mut refs: std::collections::BTreeMap<(u32, u32, crate::format::Hash16), u32> =
        std::collections::BTreeMap::new();
    let mut kept: std::collections::BTreeMap<_, u64> = std::collections::BTreeMap::new();
    for r in m.frags.iter().filter(|r| !r.tomb && !is_debug(r.out)) {
        let key = (r.out, r.class, r.hash);
        match kept.insert(key, r.offset) {
            Some(o) if o == r.offset => continue,
            Some(_) => {
                return Err(state_corrupt(format!(
                    "FRAGS: two live rows of the key (out {}, class {:#x})",
                    r.out, r.class
                )));
            }
            None => {}
        }
        refs.insert(key, 0);
        n.frags.push(crate::update::rows::FragRow { refs: 0, ..*r });
    }
    let mut last: Option<(FileId, (u32, u32, crate::format::Hash16))> = None;
    let mut sorted_uses = uses;
    sorted_uses.sort_by_key(|(f, r)| (f.0, *r));
    for (f, r) in sorted_uses {
        if is_debug(r.out) {
            continue;
        }
        let key = (r.out, r.class, r.hash);
        if last == Some((f, key)) {
            continue;
        }
        last = Some((f, key));
        *refs.get_mut(&key).ok_or_else(|| bad("a FRAG_USE key's FRAGS row"))? += 1;
        n.frag_uses.push((f, crate::update::rows::FragUseRow { file: f.0, ..r }));
    }
    for r in &mut n.frags {
        r.refs = refs.get(&(r.out, r.class, r.hash)).copied().unwrap_or(0);
    }
    n.symtab = per_file(&m.symtab, &fid, &gone, |r| Ok(*r))?;
    n.regions = per_file(&m.regions, &fid, &gone, |r| Ok(*r))?;
    // Key-keyed: tombstones dropped, references mapped (encode sorts).
    for r in m.groups.iter().filter(|r| !r.tomb) {
        n.groups.push(crate::update::rows::GroupRow {
            signature: intern(r.signature)?,
            file: file(r.file)?,
            ..*r
        });
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
    for r in m.copies.iter().filter(|r| !r.tomb) {
        n.copies.push(crate::update::rows::CopyRow { key: sym(r.key)?, file: file(r.file)?, ..*r });
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
                other => *other,
            });
        }
        n.diags.push(DiagRow { subjects, ..d.clone() });
    }
    let _ = &mut intern;
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

/// IN-71b "Live keys" (WP-215 R.6 3, `briefs/wp-215-rulings-3.md` item
/// 3), per `SYMBOLS` row of `m`: a global key is LIVE when a stored row
/// gives it a mention by a live file (a `KEY_REFS` row, a definition: its
/// `RESOLVED` section in a live file, a `COMMONS` row), a `LAZY` member's
/// definition (a `KEY_REFS` row of a `LAZY` file: its undefined mentions
/// are not stored), or an option names it (`options`, matched by name as
/// `InputsView::key_of` matches: split at the first `@`, `foo@@V` is
/// `foo`); any other global is DEAD. A local is live while its file is,
/// a `GONE` key never. The one rule of the canon and the compaction
/// ([`normalize`] drops the dead keys); public for other packages' tests
/// (U-3's series replaces its stand-in by it).
///
/// `options` is never stored (`OptionKeys`): [`normalize`], so the
/// compaction and the canon, pass the empty set; a key that only an
/// option names is kept there by its `SYMBOLS.option` bit (seam 6, set by
/// the capture from `Plan::option_keys`: `--wrap` and `--defsym` names;
/// WP-214 §10's finding, closed by WP-244 for those).
///
/// # Errors
/// `state-corrupt` for a string out of the pool.
pub fn live_keys(m: &MemoryState, options: &OptionKeys) -> Result<Vec<bool>, StateError> {
    let gone = |f: u32| {
        let f = f as usize;
        m.files.get(f).is_none_or(|r| r.flags & file_flags::GONE != 0)
            || m.file_index.get(f).is_some_and(|i| i.gone)
    };
    let mut live = vec![false; m.symbols.len()];
    let mut mark = |k: u32| {
        if let Some(x) = live.get_mut(k as usize) {
            *x = true;
        }
    };
    for r in m.key_refs.iter().filter(|r| !r.tomb && (r.file == 0 || !gone(r.file))) {
        mark(r.key);
    }
    for r in m.commons.iter().filter(|r| !r.tomb && !gone(r.file)) {
        mark(r.key);
    }
    for (k, r) in m.resolved.iter().enumerate() {
        if r.section != NONE
            && let Some(sec) = m.sections.get(r.section as usize)
            && sec.file != NONE
            && !gone(sec.file)
        {
            mark(u32::try_from(k).unwrap_or(NONE));
        }
    }
    // The option names, as (name, version).
    let mut named: std::collections::HashSet<(Vec<u8>, Vec<u8>)> = std::collections::HashSet::new();
    let mut name = |b: &[u8]| {
        let key = match b.iter().position(|&c| c == b'@') {
            None => (b.to_vec(), Vec::new()),
            Some(i) if b.get(i + 1) == Some(&b'@') => (b[..i].to_vec(), Vec::new()),
            Some(i) => (b[..i].to_vec(), b[i + 1..].to_vec()),
        };
        named.insert(key);
    };
    for k in options.entry.iter().chain(&options.undefined).chain(&options.require_defined) {
        name(&k.name);
    }
    for w in &options.wrap {
        name(w);
        name(&[b"__wrap_".as_slice(), w].concat());
        name(&[b"__real_".as_slice(), w].concat());
    }
    for d in &options.defsym {
        name(&d.name);
        if let Some(b) = d.value.base() {
            name(b);
        }
    }
    let s = |x: Str| pool_string(&m.strings, x).map_err(super::read::to_state_error);
    for (k, r) in m.symbols.iter().enumerate() {
        match r.kind {
            sym_kind::GONE => live[k] = false,
            sym_kind::LOCAL => live[k] = !gone(r.file),
            // `SYMBOLS.option` (seam 6): the command line names it, so the
            // state does without `options` (never GONE, never dead).
            _ if r.option => live[k] = true,
            _ if live[k] || named.is_empty() => {}
            _ => live[k] = named.contains(&(s(r.name)?, s(r.version)?)),
        }
    }
    Ok(live)
}

/// The capture's checks (WP-239 R-2), run where the capture's rows are
/// built and before any byte of a state is written: two different live
/// `FRAGS` rows with one key `(out, class, hash)` at the capture's sort
/// are `state-collision` (the caller writes no state and removes the
/// previous one). The same row twice is one.
///
/// # Errors
/// `state-collision`.
pub fn check_capture(m: &MemoryState) -> Result<(), IncrReason> {
    let mut keys: Vec<_> = m.frags.iter().filter(|r| !r.tomb).collect();
    keys.sort_by_key(|r| (r.out, r.class, r.hash, r.offset));
    for w in keys.windows(2) {
        let (a, b) = (w[0], w[1]);
        if (a.out, a.class, a.hash) == (b.out, b.class, b.hash) && a.offset != b.offset {
            return Err(IncrReason::StateCollision(format!(
                "two merged fragments of output section {} (class {:#x}) have one 128-bit \
                 hash (offsets {} and {})",
                a.out, a.class, a.offset, b.offset
            )));
        }
    }
    Ok(())
}

/// A file's `.eh_frame` range as `FILES`' three `u32` columns (WP-239
/// S4-3): bytes from the chunk's start, bytes held, kept records. A value
/// past `u32` is `state-limit` (no state is written, as
/// `state-collision`): the capture builds every `FILES` row through this.
///
/// # Errors
/// `state-limit`.
pub fn eh_columns(offset: u64, capacity: u64, records: u64) -> Result<[u32; 3], IncrReason> {
    let col = |v: u64, what: &str| {
        u32::try_from(v).map_err(|_| {
            IncrReason::StateLimit(format!(
                "a file's .eh_frame {what} ({v}) is past the state's 32-bit column"
            ))
        })
    };
    let end = offset.saturating_add(capacity);
    col(end, "range end")?;
    Ok([col(offset, "offset")?, col(capacity, "capacity")?, col(records, "record count")?])
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
    let Some(Planned::Compact { core, state, strs }) = plan.inner.as_deref() else {
        return Err(WriteFailure::Fallback(corrupt("an in-place plan given to compact")));
    };
    let output = &strs.output(output).map_err(WriteFailure::Fallback)?;
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
    let Some(Planned::Compact { state, strs, .. }) = plan.inner.as_deref() else { return None };
    compacted_bytes(state, &strs.output(output).ok()?).ok()
}
