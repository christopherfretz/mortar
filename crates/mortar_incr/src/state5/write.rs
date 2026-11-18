//! The state written whole ([`encode`]) and patched in place
//! (`spec/incr.md` IN-46b, IN-18e; IN-77's W5/C4), in two calls (WP-213
//! follow-up, Q-1): [`plan_patch`] before W1 (every read, every check,
//! every decision) and [`patch`] at W5 (writes only). WP-213 (U-1): the
//! entry points and [`PatchPlan`]'s one public accessor; filled by U-2
//! (WP-214).

use std::collections::BTreeMap;
use std::sync::Arc;

use mortar_core::FileId;
use mortar_core::incr::IncrReason;

use super::read::{Core, core_of, decode_core};
use super::{
    BLOCK, Class, DiagKey, DiagRaw, ENTRY, Entry, HEADER, Header, Keyed, NO_ROW, Rec, StrFn,
    TABLES, corrupt, diag_raw, encode_rows, fresh_capacity, name, range_capacity, state_corrupt,
    table_index,
};
use crate::format::{Hash16, NONE, StatRow, StateError, Str, W, kind, stat_tag};
use crate::format::{index_kind, patch_kind};
use crate::ops::{WriteFailure, WriteOps};
use crate::update::rows::edge_kind;
use crate::update::rows::{FileIndexRow, FileTable, RowRange, file_flags};
use crate::update::testing::MemoryState;
use crate::update::view::{KeyEdits, Rows, StatePatch, StateView};

// ---- the whole state written -------------------------------------------------------

/// One table's body as the writer lays it out.
struct Body {
    rows: u64,
    capacity: u64,
    aux: u64,
    dead: u64,
    bytes: Vec<u8>,
}

fn body<T: Rec>(rows: &[T], class: Class) -> Body {
    let n = rows.len() as u64;
    Body { rows: n, capacity: fresh_capacity(class, n), aux: 0, dead: 0, bytes: encode_rows(rows) }
}

fn keyed_body<T: Keyed>(rows: &[T]) -> Body {
    let mut b = body(rows, Class::KeyKeyed);
    b.aux = b.rows;
    b.dead = rows.iter().filter(|r| r.tomb()).count() as u64;
    b
}

/// A key-keyed table sorted by key (stable), and the old position of
/// each sorted row.
fn sorted<T: Keyed>(rows: &[T], s: StrFn<'_>) -> Result<(Vec<T>, Vec<usize>), StateError> {
    let mut keyed = Vec::with_capacity(rows.len());
    for (i, r) in rows.iter().enumerate() {
        keyed.push((r.key(s).map_err(super::read::to_state_error)?, i));
    }
    keyed.sort();
    let order: Vec<usize> = keyed.iter().map(|(_, i)| *i).collect();
    Ok((order.iter().map(|&i| rows[i].clone()).collect(), order))
}

/// One file-keyed table laid out by file (`FileId` order, each file's rows
/// in their order), with IN-18e's slack: `count + max(4, count/8)` for a
/// file with rows, nothing for a file with none.
fn by_file<T: Rec>(
    files: usize,
    rows: impl Iterator<Item = (FileId, T)>,
    ranges: &mut [FileIndexRow],
    t: FileTable,
) -> Result<(Body, Vec<Option<u64>>), StateError> {
    let mut per: Vec<Vec<(usize, T)>> = (0..files).map(|_| Vec::new()).collect();
    for (i, (f, r)) in rows.enumerate() {
        per.get_mut(f.0 as usize)
            .ok_or_else(|| state_corrupt(format!("{}: a row of file {}", name(t.kind()), f.0)))?
            .push((i, r));
    }
    let mut slots: Vec<Option<T>> = Vec::new();
    let mut moved = Vec::new();
    for (f, rows) in per.into_iter().enumerate() {
        let count = u32::try_from(rows.len()).map_err(|_| state_corrupt("a range over 2^32"))?;
        let capacity = if count == 0 { 0 } else { range_capacity(count) };
        let first = u32::try_from(slots.len()).map_err(|_| state_corrupt("a table over 2^32"))?;
        ranges[f].ranges[t as usize] = RowRange { first, count, capacity };
        for (i, r) in rows {
            if moved.len() <= i {
                moved.resize(i + 1, None);
            }
            moved[i] = Some(slots.len() as u64);
            slots.push(Some(r));
        }
        for _ in count..capacity {
            slots.push(None);
        }
    }
    let extent = slots.len() as u64;
    let mut bytes = Vec::with_capacity(slots.len() * T::WIDTH);
    for s in slots {
        let mut w = W(&mut bytes);
        match s {
            Some(r) => r.put(&mut w),
            None => {
                w.pad(T::WIDTH);
            }
        }
    }
    let b = Body {
        rows: extent,
        capacity: fresh_capacity(Class::FileKeyed, extent),
        aux: 0,
        dead: 0,
        bytes,
    };
    Ok((b, moved))
}

/// The bytes of a format-5 state holding `m`'s rows, each table with
/// IN-18e's fresh capacity: the full link's writer (over the normalized
/// capture, `super::compact::normalize`), the compaction's, and
/// `incr_state convert`'s. `m`'s numbering is kept but for two tables the
/// format numbers itself: `SECTIONS` (a file's rows contiguous, with its
/// slack: every section reference is renumbered; `file` [`NONE`] rows are
/// dropped) and `FRAGS` (sorted by its key, which `FRAG_USE` names).
/// Key-keyed tables are sorted by key; tombstones are kept.
///
/// # Errors
/// `state-corrupt` for rows the format cannot hold (a row of a file past
/// `FILES`, a key-parallel table longer than `SYMBOLS`, a reference to a
/// dropped section).
#[allow(clippy::too_many_lines)] // one block per table, in kind order
pub fn encode(m: &MemoryState) -> Result<Vec<u8>, StateError> {
    let files = m.files.len();
    if !m.odr_classes.is_empty() || !m.odr_refs.is_empty() {
        return Err(state_corrupt("ODR_CLASSES or ODR_REFS rows: format 5 holds none (WP-236)"));
    }
    let strings_of = |x: Str| -> Result<Vec<u8>, IncrReason> {
        let start = x.off as usize;
        start
            .checked_add(x.len as usize)
            .and_then(|end| m.strings.get(start..end))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| corrupt("STRINGS: a string out of range"))
    };
    let s: StrFn<'_> = &strings_of;
    let mut ranges = vec![FileIndexRow::default(); files];
    for (f, r) in m.files.iter().enumerate() {
        ranges[f].gone =
            r.flags & file_flags::GONE != 0 || m.file_index.get(f).is_some_and(|i| i.gone);
        ranges[f].first_global = m.file_index.get(f).map_or(0, |i| i.first_global);
    }
    // SECTIONS first: every other table's section references follow it.
    let live = m.sections.iter().enumerate().filter(|(_, r)| r.file != NONE);
    let (sec_body, moved) = by_file(
        files,
        live.map(|(i, r)| (FileId(r.file), (i, r.clone()))).map(|(f, (_, r))| (f, r)),
        &mut ranges,
        FileTable::Sections,
    )?;
    // `by_file` numbers rows by their position among the live ones; map
    // the old positions.
    let mut sec_map = vec![NONE; m.sections.len()];
    let mut k = 0;
    for (i, r) in m.sections.iter().enumerate() {
        if r.file != NONE {
            sec_map[i] = moved.get(k).copied().flatten().map_or(NONE, |v| v as u32);
            k += 1;
        }
    }
    let sec = |x: u32| -> Result<u32, StateError> {
        if x == NONE {
            return Ok(NONE);
        }
        sec_map.get(x as usize).copied().filter(|&v| v != NONE).ok_or_else(|| {
            state_corrupt(format!("a reference to SECTIONS row {x}, which has none"))
        })
    };
    let (frags, _) = sorted(&m.frags, s)?;
    let live_frags: Vec<_> = frags.iter().filter(|r| !r.tomb).collect();
    if live_frags
        .windows(2)
        .any(|w| (w[0].out, w[0].class, w[0].hash) == (w[1].out, w[1].class, w[1].hash))
    {
        return Err(state_corrupt("FRAGS: two live rows of one key"));
    }
    let mut mentions = Vec::with_capacity(m.mentions.len());
    for (f, r) in &m.mentions {
        mentions.push((*f, crate::update::rows::MentionRow { section: sec(r.section)?, ..*r }));
    }
    let mut index = Vec::with_capacity(m.index.len());
    for (f, r) in &m.index {
        let target = match r.kind {
            index_kind::SECTION
            | index_kind::EH_FRAME
            | index_kind::LINK_ORDER
            | index_kind::LOCAL_SLOT => sec(r.target)?,
            _ => r.target,
        };
        index.push((*f, crate::format::IndexRow { unit: sec(r.unit)?, kind: r.kind, target }));
    }
    let mut edges = Vec::with_capacity(m.edges.len());
    for (f, r) in &m.edges {
        let target = match r.kind {
            edge_kind::LOCAL | edge_kind::FDE_SECTION | edge_kind::NONALLOC_SECTION => {
                sec(r.target)?
            }
            _ => r.target,
        };
        edges.push((*f, crate::update::rows::EdgeRow { source: sec(r.source)?, target, ..*r }));
    }
    if m.frag_uses.iter().any(|(f, r)| r.file != f.0) {
        return Err(state_corrupt("FRAG_USE: a row in another file's range"));
    }
    let (men_body, _) = by_file(files, mentions.into_iter(), &mut ranges, FileTable::Mentions)?;
    let (idx_body, _) = by_file(files, index.into_iter(), &mut ranges, FileTable::Index)?;
    let (edge_body, _) = by_file(files, edges.into_iter(), &mut ranges, FileTable::Edges)?;
    let (fu_body, _) =
        by_file(files, m.frag_uses.iter().copied(), &mut ranges, FileTable::FragUse)?;
    let (st_body, _) = by_file(files, m.symtab.iter().copied(), &mut ranges, FileTable::Symtab)?;
    let (reg_body, _) = by_file(files, m.regions.iter().copied(), &mut ranges, FileTable::Regions)?;
    for (f, idx) in ranges.iter().enumerate() {
        if idx.ranges[FileTable::Symtab as usize].count > 1 {
            return Err(state_corrupt(format!("SYMTAB: file {f} has more than one row")));
        }
    }
    let mut patches = m.patches.clone();
    for p in &mut patches {
        if p.kind == patch_kind::E843419 {
            p.section = sec(p.section)?;
        }
    }
    let mut redirects = m.redirects.clone();
    for r in &mut redirects {
        r.section = sec(r.section)?;
    }
    let mut resolved = m.resolved.clone();
    for r in &mut resolved {
        r.section = sec(r.section)?;
    }
    let mut symtab_keys = m.symtab_keys.clone();
    let n = m.symbols.len();
    if resolved.len() > n || symtab_keys.len() > n {
        return Err(state_corrupt("a key-parallel table longer than SYMBOLS"));
    }
    resolved.resize(n, crate::update::rows::ResolvedRow { section: NONE, ..Default::default() });
    symtab_keys.resize(n, crate::update::rows::SymtabKeyRow { index: NONE, strtab: 0 });
    // DIAGS' strings are appended to a copy of the pool.
    let mut strings = m.strings.clone();
    let mut add = |b: &[u8]| -> Str {
        if b.is_empty() {
            return Str::default();
        }
        let off = u32::try_from(strings.len()).unwrap_or(u32::MAX);
        strings.extend_from_slice(b);
        Str { off, len: u32::try_from(b.len()).unwrap_or(u32::MAX) }
    };
    let diags: Vec<DiagRaw> = m.diags.iter().map(|d| diag_raw(d, &mut add)).collect();
    if u32::try_from(strings.len()).is_err() {
        return Err(state_corrupt("the string pool exceeds 4 GiB"));
    }
    let strings_of2 = |x: Str| -> Result<Vec<u8>, IncrReason> {
        let start = x.off as usize;
        start
            .checked_add(x.len as usize)
            .and_then(|end| strings.get(start..end))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| corrupt("STRINGS: a string out of range"))
    };
    let s2: StrFn<'_> = &strings_of2;
    let mut stat = body(&m.stat, Class::AppendOnly);
    stat.aux = m.stat.iter().rposition(|r| r.tag == stat_tag::OUTPUT).map_or(NO_ROW, |i| i as u64);
    let mut bodies: Vec<(u32, Body)> = vec![
        (kind::META, body(std::slice::from_ref(&m.meta), Class::Whole)),
        (kind::STAT, stat),
        (kind::FILES, body(&m.files, Class::AppendOnly)),
        (kind::PROBES, body(&m.probes, Class::Whole)),
        (kind::OUTSECS, body(&m.outsecs, Class::Whole)),
        (kind::SYMBOLS, body(&m.symbols, Class::KeyParallel)),
        (kind::SECTIONS, sec_body),
        (kind::CHUNKS, body(&m.chunks, Class::Whole)),
        (kind::ORDER, body(&m.order, Class::Whole)),
        (kind::SEGMENTS, body(&m.segments, Class::Whole)),
        (kind::FRAGS, keyed_body(&frags)),
        (kind::SLOTS, body(&m.slots, Class::Whole)),
        (kind::SIZES, body(&m.sizes, Class::Whole)),
        (kind::SYM_PLACE, body(&m.sym_place, Class::Whole)),
        (kind::DEFINED, body(&m.defined, Class::Whole)),
        (kind::PATCHES, body(&patches, Class::Whole)),
        (kind::REDIRECTS, body(&redirects, Class::Whole)),
        (kind::SIZED, body(&m.sized, Class::Whole)),
        (kind::LAYOUT, body(std::slice::from_ref(&m.layout), Class::Whole)),
        (kind::LEAVES, body(&m.leaves, Class::Whole)),
        (kind::STRINGS, body(&strings, Class::AppendOnly)),
        (kind::MENTIONS, men_body),
        (kind::RESOLVED, body(&resolved, Class::KeyParallel)),
        (kind::INDEX, idx_body),
        (kind::HOLES, keyed_body(&sorted(&m.holes, s)?.0)),
        (kind::FILE_INDEX, body(&ranges, Class::AppendOnly)),
        (kind::EDGES, edge_body),
        (kind::SYMTAB, st_body),
        (kind::GROUPS, keyed_body(&sorted(&m.groups, s)?.0)),
        (kind::KEY_REFS, keyed_body(&sorted(&m.key_refs, s)?.0)),
        (kind::COMMONS, keyed_body(&sorted(&m.commons, s)?.0)),
        (kind::DIAGS, keyed_body(&sorted(&diags, s2)?.0)),
        (kind::SYMTAB_KEYS, body(&symtab_keys, Class::KeyParallel)),
        (kind::REGIONS, reg_body),
        (kind::NAMES, keyed_body(&sorted(&m.names, s)?.0)),
        (kind::COPIES, keyed_body(&sorted(&m.copies, s)?.0)),
        (kind::FRAG_USE, fu_body),
    ];
    debug_assert!(bodies.iter().zip(TABLES).all(|((k, _), t)| *k == t.0));
    Ok(assemble(&mut bodies, m.plan_hash, 0))
}

/// The state's bytes from its bodies (in kind order): directory, lists,
/// header.
fn assemble(bodies: &mut [(u32, Body)], plan_hash: Hash16, flags: u32) -> Vec<u8> {
    let mut at = (HEADER + TABLES.len() * ENTRY) as u64;
    let mut dir = Vec::with_capacity(bodies.len());
    let mut total = at;
    for ((k, b), (_, _, width)) in bodies.iter().zip(TABLES) {
        let e = Entry {
            kind: *k,
            width: u32::try_from(width).unwrap_or(0),
            rows: b.rows,
            capacity: b.capacity,
            aux: b.aux,
            dead: b.dead,
            body: 0,
            list: at,
            list_hash: [0; 16],
        };
        let e = Entry { body: at + e.list_len(), ..e };
        at = e.body + e.body_len();
        total = at;
        dir.push(e);
    }
    let mut out = Vec::with_capacity(usize::try_from(total).unwrap_or(0));
    out.resize(HEADER + TABLES.len() * ENTRY, 0);
    for ((_, b), e) in bodies.iter_mut().zip(dir.iter_mut()) {
        b.bytes.resize(usize::try_from(e.body_len()).unwrap_or(0), 0);
        let mut list = Vec::with_capacity(usize::try_from(e.list_len()).unwrap_or(0));
        for block in b.bytes.chunks(BLOCK) {
            list.extend_from_slice(&mortar_digest::l128(block));
        }
        e.list_hash = mortar_digest::l128(&list);
        out.extend_from_slice(&list);
        out.extend_from_slice(&b.bytes);
        b.bytes = Vec::new();
    }
    let dir_bytes = super::directory_bytes(&dir);
    out[HEADER..HEADER + dir_bytes.len()].copy_from_slice(&dir_bytes);
    let h = Header {
        flags,
        count: u32::try_from(TABLES.len()).unwrap_or(0),
        block: BLOCK as u32,
        len: out.len() as u64,
        plan_hash,
        dir_hash: mortar_digest::l128(&dir_bytes),
    };
    out[..HEADER].copy_from_slice(&h.bytes());
    out
}

// ---- the plan ------------------------------------------------------------------------

/// What W5 (or the compaction) will write, decided before W1 by
/// [`plan_patch`]: the verified blocks with the patch's rows in them, or
/// the whole compacted state. **Opaque** to every package but U-2; the one
/// public question is [`PatchPlan::compacts`].
///
/// `Default` is the empty plan (writes nothing, patches in place).
#[derive(Debug, Clone, Default)]
pub struct PatchPlan {
    /// W5 is the compaction (IN-18f): the rows do not fit, or `S` is
    /// hard-linked (IN-77b).
    compacts: bool,
    /// Why (the report's `delta state compacted <reason>`).
    reason: String,
    pub(crate) inner: Option<Arc<Planned>>,
}

impl PartialEq for PatchPlan {
    fn eq(&self, other: &Self) -> bool {
        self.compacts == other.compacts
            && self.reason == other.reason
            && match (&self.inner, &other.inner) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }
}

impl Eq for PatchPlan {}

impl PatchPlan {
    /// W5/C4 is the compaction ([`super::compact`]), not [`patch`]; the
    /// report's `delta state compacted` (IN-46b).
    #[must_use]
    pub const fn compacts(&self) -> bool {
        self.compacts
    }

    /// Why the plan compacts (empty when it does not).
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Blocks the in-place plan writes (tests: IN-46b's O(change)).
    #[must_use]
    pub fn blocks(&self) -> usize {
        match self.inner.as_deref() {
            Some(Planned::InPlace(p)) => p.blocks.len(),
            _ => 0,
        }
    }
}

/// A plan's contents.
#[derive(Debug)]
pub(crate) enum Planned {
    InPlace(InPlace),
    Compact { core: Arc<Core>, state: Box<MemoryState>, strs: Strs },
}

/// How the plan placed the patch's strings (F-11): each appended `Str`
/// of the patch (by its offset and length in the patch's id space) to
/// the stored `Str` of equal bytes it reuses, or to its place in the
/// bytes the plan appends. `old` is the state's `STRINGS` length.
#[derive(Debug, Clone, Default)]
pub(crate) struct Strs {
    pub(crate) old: u64,
    pub(crate) map: BTreeMap<(u32, u32), Str>,
}

impl Strs {
    /// `x` in the plan's string space: a stored `Str` unchanged, a
    /// patch's `Str` mapped; one the patch's rows never named is
    /// `state-corrupt` (its bytes were not kept).
    pub(crate) fn of(&self, x: Str) -> Rows<Str> {
        if x.len == 0 || u64::from(x.off) + u64::from(x.len) <= self.old {
            return Ok(x);
        }
        self.map.get(&(x.off, x.len)).copied().ok_or_else(|| {
            corrupt(format!(
                "STRINGS: the patch's string at {} ({} bytes) is named by none of its rows",
                x.off, x.len
            ))
        })
    }

    /// W4's output row in the plan's string space.
    pub(crate) fn output(&self, row: &StatRow) -> Rows<StatRow> {
        Ok(StatRow { path: self.of(row.path)?, fstype: self.of(row.fstype)?, ..row.clone() })
    }
}

/// The in-place patch: every block W5 writes, with its new bytes (each
/// read and verified first), the old lists of their tables, the new
/// directory (list hashes left for [`patch`]), and where the output's
/// `STAT` row goes.
#[derive(Debug)]
pub(crate) struct InPlace {
    pub(crate) core: Arc<Core>,
    pub(crate) blocks: BTreeMap<(usize, u64), Vec<u8>>,
    pub(crate) lists: BTreeMap<usize, Arc<Vec<Hash16>>>,
    pub(crate) dir: Vec<Entry>,
    pub(crate) stat_out: u64,
    pub(crate) strs: Strs,
}

/// Why the patch does not fit (IN-18e): the compaction.
struct Overflow(String);

enum PlanError {
    Fit(Overflow),
    Fail(IncrReason),
}

impl From<IncrReason> for PlanError {
    fn from(r: IncrReason) -> Self {
        Self::Fail(r)
    }
}

type Plan<T> = Result<T, PlanError>;

fn overflow<T>(why: impl Into<String>) -> Plan<T> {
    Err(PlanError::Fit(Overflow(why.into())))
}

/// The in-place editor: blocks read and verified once, then changed.
struct Ed<'c> {
    core: &'c Core,
    dir: Vec<Entry>,
    blocks: BTreeMap<(usize, u64), Vec<u8>>,
    /// The `STRINGS` bytes the patch appends (the join's, then U-2's own).
    appended: Vec<u8>,
    old_strings: u64,
}

impl Ed<'_> {
    fn t(k: u32) -> usize {
        table_index(k).unwrap_or(0)
    }

    /// Write `bytes` at `off` of table `t`'s body (inside its capacity).
    fn write(&mut self, t: usize, off: u64, bytes: &[u8]) -> Plan<()> {
        let e = self.dir[t];
        if off + bytes.len() as u64 > e.body_len() {
            return overflow(format!("{}: past its capacity", name(e.kind)));
        }
        let mut at = 0usize;
        while at < bytes.len() {
            let pos = off + at as u64;
            let k = pos / BLOCK as u64;
            if !self.blocks.contains_key(&(t, k)) {
                let b = self.core.block(t, k, true)?;
                self.blocks.insert((t, k), b.to_vec());
            }
            let block = self.blocks.get_mut(&(t, k)).ok_or_else(|| corrupt("a block"))?;
            let from = usize::try_from(pos - k * BLOCK as u64).unwrap_or(0);
            let n = (block.len() - from).min(bytes.len() - at);
            block[from..from + n].copy_from_slice(&bytes[at..at + n]);
            at += n;
        }
        Ok(())
    }

    fn put_rows<T: Rec>(&mut self, k: u32, first: u64, rows: &[T]) -> Plan<()> {
        let t = Self::t(k);
        if first + rows.len() as u64 > self.dir[t].capacity {
            return overflow(format!("{}: over its capacity", name(k)));
        }
        self.write(t, first * T::WIDTH as u64, &encode_rows(rows))
    }

    fn zero_rows(&mut self, k: u32, first: u64, count: u64) -> Plan<()> {
        if count == 0 {
            return Ok(());
        }
        let t = Self::t(k);
        let w = u64::from(self.dir[t].width);
        self.write(t, first * w, &vec![0u8; usize::try_from(count * w).unwrap_or(0)])
    }

    /// A whole table written again (IN-18e "whole").
    fn whole<T: Rec>(&mut self, k: u32, rows: Option<&[T]>) -> Plan<()> {
        let Some(rows) = rows else { return Ok(()) };
        let t = Self::t(k);
        let old = self.dir[t].rows;
        let n = rows.len() as u64;
        if n > self.dir[t].capacity {
            return overflow(format!("{} over its capacity", name(k)));
        }
        self.put_rows(k, 0, rows)?;
        if old > n {
            self.zero_rows(k, n, old - n)?;
        }
        self.dir[t].rows = n;
        Ok(())
    }

    /// A string: old (through the reader) or appended by the patch.
    fn string(&self, x: Str) -> Rows<Vec<u8>> {
        if u64::from(x.off) >= self.old_strings && x.len != 0 {
            let start = usize::try_from(u64::from(x.off) - self.old_strings).unwrap_or(usize::MAX);
            return start
                .checked_add(x.len as usize)
                .and_then(|end| self.appended.get(start..end))
                .map(<[u8]>::to_vec)
                .ok_or_else(|| corrupt("STRINGS: an appended string out of range"));
        }
        self.core.string(x)
    }

    fn add_string(&mut self, b: &[u8]) -> Str {
        if b.is_empty() {
            return Str::default();
        }
        let off = self.old_strings + self.appended.len() as u64;
        self.appended.extend_from_slice(b);
        Str {
            off: u32::try_from(off).unwrap_or(u32::MAX),
            len: u32::try_from(b.len()).unwrap_or(u32::MAX),
        }
    }

    /// A key-keyed table's edits (IN-18e): deletes are tombstones; a put
    /// of a present key (live or dead) changes the row in place; another
    /// put is inserted into the sorted append area. `stable_rows`: rows
    /// are named by number elsewhere, so an insert that would move an area
    /// row is the compaction (no table since WP-239: `FRAG_USE` names
    /// `FRAGS` by key).
    #[allow(clippy::too_many_lines, clippy::many_single_char_names)] // one arm per edit
    fn keyed<T: Keyed>(&mut self, k: u32, edits: &KeyEdits<T>, stable_rows: bool) -> Plan<()> {
        if edits.put.is_empty() && edits.delete.is_empty() {
            return Ok(());
        }
        let t = Self::t(k);
        let e = self.dir[t];
        let mut area: Vec<T> = self.core.rows(k, e.aux, e.rows - e.aux)?;
        let area_len0 = area.len();
        let mut dead = e.dead;
        let mut body_writes: Vec<(u64, T)> = Vec::new();
        let mut area_changed = false;
        let this = &*self;
        let s: StrFn<'_> = &|x| this.string(x);
        let mut area_keys: Vec<T::K> = area.iter().map(|r| r.key(s)).collect::<Rows<_>>()?;
        for (del, row) in
            edits.delete.iter().map(|r| (true, r)).chain(edits.put.iter().map(|r| (false, r)))
        {
            let key = row.key(s)?;
            // the sorted body
            let (mut a, mut b) = (0, e.aux);
            while a < b {
                let mid = a + (b - a) / 2;
                let r: T = match body_writes.iter().rev().find(|(i, _)| *i == mid) {
                    Some((_, r)) => r.clone(),
                    None => self.core.row(k, mid)?,
                };
                if r.key(s)? < key {
                    a = mid + 1;
                } else {
                    b = mid;
                }
            }
            let found = if a < e.aux {
                let r: T = match body_writes.iter().rev().find(|(i, _)| *i == a) {
                    Some((_, r)) => r.clone(),
                    None => self.core.row(k, a)?,
                };
                (r.key(s)? == key).then_some((true, a, r))
            } else {
                None
            };
            let found = match found {
                Some(f) => Some(f),
                None => {
                    area_keys.binary_search(&key).ok().map(|i| (false, i as u64, area[i].clone()))
                }
            };
            match (del, found) {
                (true, None) => {
                    return Err(PlanError::Fail(corrupt(format!(
                        "{}: the patch deletes a key the state does not hold ({key:?})",
                        name(k)
                    ))));
                }
                (true, Some((_, _, r))) if r.tomb() => {
                    return Err(PlanError::Fail(corrupt(format!(
                        "{}: the patch deletes a dead key ({key:?})",
                        name(k)
                    ))));
                }
                (true, Some((in_body, i, mut r))) => {
                    r.set_tomb(true);
                    if !r.tomb() {
                        return Err(PlanError::Fail(corrupt(format!(
                            "{}: its rows are not deleted",
                            name(k)
                        ))));
                    }
                    dead += 1;
                    if in_body {
                        body_writes.push((i, r));
                    } else {
                        area[i as usize] = r;
                        area_changed = true;
                    }
                }
                (false, Some((in_body, i, old))) => {
                    if old.tomb() {
                        dead -= 1;
                    }
                    let mut r = row.clone();
                    r.set_tomb(false);
                    if in_body {
                        body_writes.push((i, r));
                    } else {
                        area[i as usize] = r;
                        area_changed = true;
                    }
                }
                (false, None) => {
                    let at = area_keys.partition_point(|x| *x < key);
                    if stable_rows && at < area_len0 {
                        return overflow(format!(
                            "{}: an insert would move a row others name",
                            name(k)
                        ));
                    }
                    let mut r = row.clone();
                    r.set_tomb(false);
                    area.insert(at, r);
                    area_keys.insert(at, key);
                    area_changed = true;
                }
            }
        }
        let rows = e.aux + area.len() as u64;
        if rows > e.capacity {
            return overflow(format!("{}: its append area is full", name(k)));
        }
        if dead * 4 > rows {
            return overflow(format!("{}: tombstones pass 25%", name(k)));
        }
        for (i, r) in body_writes {
            self.put_rows(k, i, std::slice::from_ref(&r))?;
        }
        if area_changed {
            self.put_rows(k, e.aux, &area)?;
        }
        self.dir[t].rows = rows;
        self.dir[t].dead = dead;
        Ok(())
    }
}

/// Plan the state's patch (IN-46b "verified before re-hashed"; IN-70 U9):
/// read every block W5 will write and check it against its block-list
/// entry (IN-18c), put `patch`'s rows into the blocks, and decide in place
/// or compaction (a range, append area or tombstone bound that does not
/// fit, IN-18e; `S` hard-linked, IN-77b). For a compaction, IN-18f step
/// 1's read of every block happens here too, and the compacted state is
/// built here. Called by the join in U9, **before W1**: a failure here
/// leaves the old pair untouched.
///
/// The patch's ids are the update's (`mortar_core::incr`): an appended
/// row's id is its table's count plus its position. Ranges (U-10 must
/// number new `SECTIONS` rows the same way): a file of `ranges` whose new
/// rows fit its old range's capacity keeps the range's rows (`first ..`);
/// any other range is appended at the table's extent, with capacity
/// `count + max(4, count/8)`, in `ranges` order.
///
/// # Errors
/// `state-corrupt` (a block that fails its hash, a patch row naming a row
/// the state does not hold); `update-not-implemented (state5)` for a view
/// `state5::open` did not make.
pub fn plan_patch(view: &StateView<'_>, patch: &StatePatch) -> Result<PatchPlan, IncrReason> {
    let Some(core) = core_of(view) else {
        return Err(IncrReason::UpdateNotImplemented(Some(
            mortar_core::incr::Unimplemented::State5,
        )));
    };
    let (owned, strs) = intern_patch(&core, patch)?;
    let patch = &owned;
    let linked = core.src.file().and_then(|f| f.metadata().ok()).is_some_and(|m| {
        use std::os::unix::fs::MetadataExt as _;
        m.nlink() > 1
    });
    let why = if linked {
        "the state is hard-linked".to_owned()
    } else {
        match plan_in_place(&core, patch) {
            Ok(mut p) => {
                p.strs = strs;
                return Ok(PatchPlan {
                    compacts: false,
                    reason: String::new(),
                    inner: Some(Arc::new(Planned::InPlace(p))),
                });
            }
            Err(PlanError::Fail(r)) => return Err(r),
            Err(PlanError::Fit(Overflow(why))) => why,
        }
    };
    let mut state = decode_core(&core)?;
    let frags_aux = usize::try_from(core.entry(kind::FRAGS).aux).unwrap_or(0);
    super::compact::apply(&mut state, patch, frags_aux)?;
    // Every check of the compaction happens here (a reference with no
    // image is a defect): `compact` normalizes again with the binding.
    super::compact::normalize(&state).map_err(|e| corrupt(e.to_string()))?;
    Ok(PatchPlan {
        compacts: true,
        reason: why,
        inner: Some(Arc::new(Planned::Compact { core, state: Box::new(state), strs })),
    })
}

/// The in-place plan, or why the patch does not fit.
#[allow(clippy::too_many_lines)] // one block per class
fn plan_in_place(core: &Arc<Core>, p: &StatePatch) -> Plan<InPlace> {
    let mut ed = Ed {
        core,
        dir: core.dir.clone(),
        blocks: BTreeMap::new(),
        appended: p.strings.clone(),
        old_strings: core.entry(kind::STRINGS).rows,
    };
    // whole
    let mut meta = match &p.meta {
        Some(m) => m.clone(),
        None => core.row(kind::META, 0)?,
    };
    meta.output_synced = true;
    ed.put_rows(kind::META, 0, std::slice::from_ref(&meta))?;
    ed.whole(kind::PROBES, p.probes.as_deref())?;
    ed.whole(kind::OUTSECS, p.outsecs.as_deref())?;
    ed.whole(kind::CHUNKS, p.chunks.as_deref())?;
    ed.whole(kind::ORDER, p.order.as_deref())?;
    ed.whole(kind::SEGMENTS, p.segments.as_deref())?;
    ed.whole(kind::SLOTS, p.slots.as_deref())?;
    ed.whole(kind::SIZES, p.sizes.as_deref())?;
    ed.whole(kind::SYM_PLACE, p.sym_place.as_deref())?;
    ed.whole(kind::DEFINED, p.defined.as_deref())?;
    ed.whole(kind::PATCHES, p.patches.as_deref())?;
    ed.whole(kind::REDIRECTS, p.redirects.as_deref())?;
    ed.whole(kind::SIZED, p.sized.as_deref())?;
    ed.whole(kind::LAYOUT, p.layout.as_ref().map(std::slice::from_ref))?;
    ed.whole(kind::LEAVES, p.leaves.as_deref())?;
    // append-only: FILES and FILE_INDEX
    let old_files = core.entry(kind::FILES).rows;
    for (f, row) in &p.files_set {
        if u64::from(f.0) >= old_files {
            return Err(PlanError::Fail(corrupt(format!(
                "FILES: the patch sets row {}, past the table",
                f.0
            ))));
        }
        ed.put_rows(kind::FILES, u64::from(f.0), std::slice::from_ref(row))?;
    }
    if !p.files_append.is_empty() {
        let t = Ed::t(kind::FILES);
        let n = p.files_append.len() as u64;
        if old_files + n > ed.dir[t].capacity
            || old_files + n > ed.dir[Ed::t(kind::FILE_INDEX)].capacity
        {
            return overflow("FILES: its append area is full");
        }
        ed.put_rows(kind::FILES, old_files, &p.files_append)?;
        ed.dir[t].rows = old_files + n;
        ed.dir[Ed::t(kind::FILE_INDEX)].rows = old_files + n;
        let fresh = vec![FileIndexRow::default(); p.files_append.len()];
        ed.put_rows(kind::FILE_INDEX, old_files, &fresh)?;
    }
    // A new rank (an appended file, a moved one) that ties a live file's:
    // U-10 found no midpoint between two ranks (IN-18e), the compaction
    // renumbers them. FILES is read whole only then.
    let mut ranks: Vec<(u64, u64)> = Vec::new();
    for (f, row) in &p.files_set {
        let old: crate::update::rows::FileRow = core.row(kind::FILES, u64::from(f.0))?;
        if old.rank != row.rank {
            ranks.push((row.rank, u64::from(f.0)));
        }
    }
    for (i, row) in p.files_append.iter().enumerate() {
        ranks.push((row.rank, old_files + i as u64));
    }
    if !ranks.is_empty() {
        let mut all: Vec<crate::update::rows::FileRow> = core.all(kind::FILES)?;
        for (f, row) in &p.files_set {
            all[f.0 as usize] = row.clone();
        }
        all.extend(p.files_append.iter().cloned());
        let mut live: Vec<u64> =
            all.iter().filter(|r| r.flags & file_flags::GONE == 0).map(|r| r.rank).collect();
        live.sort_unstable();
        if live.windows(2).any(|w| w[0] == w[1]) {
            return overflow("FILES: no rank midpoint left");
        }
    }
    // STAT: rows set, rows appended; the output's row kept (or appended).
    let t = Ed::t(kind::STAT);
    let old_stat = ed.dir[t].rows;
    for (i, row) in &p.stat_set {
        if u64::from(*i) >= old_stat {
            return Err(PlanError::Fail(corrupt(format!(
                "STAT: the patch sets row {i}, past the table"
            ))));
        }
        ed.put_rows(kind::STAT, u64::from(*i), std::slice::from_ref(row))?;
    }
    let mut rows = old_stat;
    ed.put_rows(kind::STAT, rows, &p.stat_append)?;
    rows += p.stat_append.len() as u64;
    let stat_out = if ed.dir[t].aux == NO_ROW {
        rows += 1;
        rows - 1
    } else {
        ed.dir[t].aux
    };
    // Load the output row's block now: W5 changes it.
    let out_row: StatRow =
        if stat_out < old_stat { core.row(kind::STAT, stat_out)? } else { StatRow::default() };
    ed.put_rows(kind::STAT, stat_out, std::slice::from_ref(&out_row))?;
    if rows > ed.dir[t].capacity {
        return overflow("STAT: its append area is full");
    }
    ed.dir[t].rows = rows;
    ed.dir[t].aux = stat_out;
    // file-keyed
    let total_files = ed.dir[Ed::t(kind::FILE_INDEX)].rows;
    for fr in &p.ranges {
        let f = u64::from(fr.file.0);
        if f >= total_files {
            return Err(PlanError::Fail(corrupt(format!(
                "FILE_INDEX: the patch's file {f} is past FILES"
            ))));
        }
        let mut idx: FileIndexRow =
            if f < old_files { core.file_index(fr.file)? } else { FileIndexRow::default() };
        let was_gone = idx.gone;
        // F-10: ALWAYS given (never "keep the stored value").
        idx.first_global = fr.first_global;
        macro_rules! range {
            ($field:expr, $t:expr) => {
                if let Some(rows) = $field {
                    place_range(&mut ed, &mut idx, $t, rows, was_gone)?;
                }
            };
        }
        range!(fr.sections.as_deref(), FileTable::Sections);
        range!(fr.mentions.as_deref(), FileTable::Mentions);
        range!(fr.index.as_deref(), FileTable::Index);
        range!(fr.edges.as_deref(), FileTable::Edges);
        range!(fr.frag_uses.as_deref(), FileTable::FragUse);
        range!(fr.symtab.as_ref().map(Option::as_slice), FileTable::Symtab);
        range!(fr.regions.as_deref(), FileTable::Regions);
        idx.gone = idx.gone || fr.gone;
        ed.put_rows(kind::FILE_INDEX, f, std::slice::from_ref(&idx))?;
    }
    // key-keyed
    ed.keyed(kind::KEY_REFS, &p.key_refs, false)?;
    ed.keyed(kind::GROUPS, &p.groups, false)?;
    ed.keyed(kind::FRAGS, &p.frags, false)?;
    ed.keyed(kind::HOLES, &p.holes, false)?;
    ed.keyed(kind::COMMONS, &p.commons, false)?;
    ed.keyed(kind::COPIES, &p.copies, false)?;
    if !(p.odr_classes.put.is_empty()
        && p.odr_classes.delete.is_empty()
        && p.odr_refs.put.is_empty()
        && p.odr_refs.delete.is_empty())
    {
        return Err(PlanError::Fail(corrupt(
            "the patch has ODR_CLASSES or ODR_REFS rows: format 5 holds none (WP-236)",
        )));
    }
    ed.keyed(kind::NAMES, &p.names, false)?;
    if !p.diags.put.is_empty() || !p.diags.delete.is_empty() {
        let mut raw = KeyEdits::<DiagRaw>::default();
        for d in &p.diags.put {
            let mut add = |b: &[u8]| ed.add_string(b);
            raw.put.push(diag_raw(d, &mut add));
        }
        for d in &p.diags.delete {
            // a delete names the row by its key: strings are compared,
            // not added (an absent string is a key the state lacks).
            let key: DiagKey = (
                d.key.phase,
                d.key.unit,
                d.key.seq,
                d.code.clone().into_bytes(),
                d.text.clone().into_bytes(),
            );
            let mut add = |b: &[u8]| ed.add_string(b);
            let r = diag_raw(d, &mut add);
            let _ = key;
            raw.delete.push(r);
        }
        ed.keyed(kind::DIAGS, &raw, false)?;
    }
    // key-parallel
    let syms = ed.dir[Ed::t(kind::SYMBOLS)].rows;
    let n = p.symbols.append.len();
    let mut resolved_append = p.resolved.append.clone();
    let mut keys_append = p.symtab_keys.append.clone();
    if resolved_append.len() > n || keys_append.len() > n {
        return Err(PlanError::Fail(corrupt("a key-parallel table appended past SYMBOLS")));
    }
    resolved_append
        .resize(n, crate::update::rows::ResolvedRow { section: NONE, ..Default::default() });
    keys_append.resize(n, crate::update::rows::SymtabKeyRow { index: NONE, strtab: 0 });
    parallel(&mut ed, kind::SYMBOLS, &p.symbols.set, &p.symbols.append, syms)?;
    parallel(&mut ed, kind::RESOLVED, &p.resolved.set, &resolved_append, syms)?;
    parallel(&mut ed, kind::SYMTAB_KEYS, &p.symtab_keys.set, &keys_append, syms)?;
    // STRINGS last: DIAGS may have appended.
    if !ed.appended.is_empty() {
        let t = Ed::t(kind::STRINGS);
        let old = ed.old_strings;
        let bytes = std::mem::take(&mut ed.appended);
        if old + bytes.len() as u64 > ed.dir[t].capacity
            || u32::try_from(old + bytes.len() as u64).is_err()
        {
            return overflow("STRINGS: its append area is full");
        }
        ed.write(t, old, &bytes)?;
        ed.dir[t].rows = old + bytes.len() as u64;
    }
    // Blocks no row changed are not written (the output's and META's are).
    let must = [
        (Ed::t(kind::STAT), stat_out * u64::from(ed.dir[t].width) / BLOCK as u64),
        (Ed::t(kind::META), 0),
    ];
    let mut blocks = BTreeMap::new();
    let mut lists = BTreeMap::new();
    for ((t, k), b) in ed.blocks {
        let old = core.block(t, k, true)?;
        if *old == b && !must.contains(&(t, k)) {
            continue;
        }
        if let std::collections::btree_map::Entry::Vacant(v) = lists.entry(t) {
            v.insert(core.list(t)?);
        }
        blocks.insert((t, k), b);
    }
    Ok(InPlace {
        core: Arc::clone(core),
        blocks,
        lists,
        dir: ed.dir,
        stat_out,
        strs: Strs::default(),
    })
}

/// One file's new rows of one file-keyed table (IN-18e): in place when they
/// fit the range's capacity, else a new range at the table's extent.
fn place_range<T: Rec>(
    ed: &mut Ed<'_>,
    idx: &mut FileIndexRow,
    t: FileTable,
    rows: &[T],
    gone: bool,
) -> Plan<()> {
    let k = t.kind();
    let ti = Ed::t(k);
    let old = idx.ranges[t as usize];
    let n = u32::try_from(rows.len()).map_err(|_| PlanError::Fail(corrupt("a range over 2^32")))?;
    if !gone && n <= old.capacity && old.capacity != 0 {
        ed.put_rows(k, u64::from(old.first), rows)?;
        if old.count > n {
            ed.zero_rows(k, u64::from(old.first + n), u64::from(old.count - n))?;
        }
        idx.ranges[t as usize].count = n;
        return Ok(());
    }
    // The old range's rows are dead; the new range takes the extent.
    if old.count > 0 && !gone {
        ed.zero_rows(k, u64::from(old.first), u64::from(old.count))?;
    }
    let extent = ed.dir[ti].rows;
    let capacity = if n == 0 { 0 } else { range_capacity(n) };
    if extent + u64::from(capacity) > ed.dir[ti].capacity {
        return overflow(format!("{}: its append area is full", name(k)));
    }
    let first = u32::try_from(extent).map_err(|_| PlanError::Fail(corrupt("a table over 2^32")))?;
    ed.put_rows(k, extent, rows)?;
    ed.dir[ti].rows = extent + u64::from(capacity);
    idx.ranges[t as usize] = RowRange { first, count: n, capacity };
    Ok(())
}

fn parallel<T: Rec>(
    ed: &mut Ed<'_>,
    k: u32,
    set: &[(mortar_core::SymbolId, T)],
    append: &[T],
    rows: u64,
) -> Plan<()> {
    for (id, r) in set {
        if u64::from(id.0) >= rows {
            return Err(PlanError::Fail(corrupt(format!(
                "{}: the patch sets row {}, past the table",
                name(k),
                id.0
            ))));
        }
        ed.put_rows(k, u64::from(id.0), std::slice::from_ref(r))?;
    }
    if !append.is_empty() {
        let t = Ed::t(k);
        if rows + append.len() as u64 > ed.dir[t].capacity {
            return overflow(format!("{}: its append area is full", name(k)));
        }
        ed.put_rows(k, rows, append)?;
        ed.dir[t].rows = rows + append.len() as u64;
    }
    Ok(())
}

/// The writes of an in-place plan with the binding `output`: blocks, list
/// entries, the directory, the header but its flags word (offset, bytes).
pub(crate) fn writes(p: &InPlace, output: &StatRow) -> Vec<(u64, Vec<u8>)> {
    let mut blocks = p.blocks.clone();
    let mut dir = p.dir.clone();
    let st = table_index(kind::STAT).unwrap_or(0);
    let w = u64::from(dir[st].width);
    let off = p.stat_out * w;
    let k = off / BLOCK as u64;
    if let Some(b) = blocks.get_mut(&(st, k)) {
        let row = encode_rows(std::slice::from_ref(output));
        let from = usize::try_from(off - k * BLOCK as u64).unwrap_or(0);
        // A row may straddle two blocks.
        let n = (b.len() - from).min(row.len());
        b[from..from + n].copy_from_slice(&row[..n]);
        if n < row.len()
            && let Some(b2) = blocks.get_mut(&(st, k + 1))
        {
            b2[..row.len() - n].copy_from_slice(&row[n..]);
        }
    }
    let mut out = Vec::new();
    let mut lists: BTreeMap<usize, Vec<Hash16>> = BTreeMap::new();
    for ((t, k), b) in &blocks {
        let list = lists
            .entry(*t)
            .or_insert_with(|| p.lists.get(t).map(|l| l.to_vec()).unwrap_or_default());
        if let Some(h) = list.get_mut(usize::try_from(*k).unwrap_or(usize::MAX)) {
            *h = mortar_digest::l128(b);
        }
        out.push((dir[*t].body + k * BLOCK as u64, b.clone()));
        out.push((
            dir[*t].list + k * 16,
            list.get(usize::try_from(*k).unwrap_or(0)).copied().unwrap_or_default().to_vec(),
        ));
    }
    for (t, list) in &lists {
        let mut bytes = Vec::with_capacity(list.len() * 16);
        for h in list {
            bytes.extend_from_slice(h);
        }
        dir[*t].list_hash = mortar_digest::l128(&bytes);
    }
    let dir_bytes = super::directory_bytes(&dir);
    let h = Header { dir_hash: mortar_digest::l128(&dir_bytes), ..p.core.header };
    out.push((HEADER as u64, dir_bytes));
    let head = h.bytes();
    out.push((0, head[..12].to_vec()));
    out.push((16, head[16..].to_vec()));
    out
}

/// Write the planned patch at W5/C4 (IN-46b, IN-77): the planned blocks,
/// their block-list entries, the directory entries, and the header but
/// its flags word, with the new binding: `output`, the `STAT` output row
/// W4 or C3 took (IN-77a), and `META`'s `output-synced` 1 (planned); then
/// `fdatasync`. Writes only: no read, no check. Every call through `ops`
/// on the view's descriptor. Called by U-7's `write` when
/// `!plan.compacts()`. The empty plan (`Default`) writes nothing.
///
/// # Errors
/// [`WriteFailure::Failed`] for an I/O error (IN-77e);
/// [`WriteFailure::Fallback`] (`state-corrupt`) for a plan of another view
/// or a compaction's plan.
pub fn patch(
    view: &StateView<'_>,
    plan: &PatchPlan,
    output: &StatRow,
    ops: &mut dyn WriteOps,
) -> Result<(), WriteFailure> {
    let Some(inner) = plan.inner.as_deref() else { return Ok(()) };
    let Planned::InPlace(p) = inner else {
        return Err(WriteFailure::Fallback(corrupt("a compaction's plan given to patch")));
    };
    let mine = core_of(view).is_some_and(|c| Arc::ptr_eq(&c, &p.core));
    let file = p.core.src.file().filter(|_| mine);
    let Some(file) = file else {
        return Err(WriteFailure::Fallback(corrupt("the plan is not this view's")));
    };
    let output = &p.strs.output(output).map_err(WriteFailure::Fallback)?;
    for (off, bytes) in writes(p, output) {
        ops.pwrite(file, off, &bytes)
            .map_err(|error| WriteFailure::Failed { step: "W5", error })?;
    }
    ops.fdatasync(file).map_err(|error| WriteFailure::Failed { step: "W5", error })
}

/// The bytes of [`patch`]'s result over `bytes` (tests, `incr_state`): the
/// writes applied to a copy.
#[must_use]
pub fn patched_bytes(bytes: &[u8], plan: &PatchPlan, output: &StatRow) -> Option<Vec<u8>> {
    let Some(Planned::InPlace(p)) = plan.inner.as_deref() else { return None };
    let output = &p.strs.output(output).ok()?;
    let mut out = bytes.to_vec();
    for (off, b) in writes(p, output) {
        let off = usize::try_from(off).ok()?;
        out.get_mut(off..off + b.len())?.copy_from_slice(&b);
    }
    Some(out)
}

/// Rows of a `StateError` as the update's reason.
impl From<Overflow> for IncrReason {
    fn from(o: Overflow) -> Self {
        corrupt(o.0)
    }
}

// ---- the patch's strings (F-11) ------------------------------------------

/// Every `Str` of a patch's rows, with whether its bytes may be a group
/// signature (a `GROUPS` row's, a section's anchor).
fn each_str(q: &mut StatePatch, f: &mut dyn FnMut(&mut Str, bool) -> Rows<()>) -> Rows<()> {
    use crate::update::diags::Subject;
    if let Some(m) = &mut q.meta {
        f(&mut m.version, false)?;
    }
    for r in q.probes.iter_mut().flatten() {
        f(&mut r.path, false)?;
    }
    for r in q.outsecs.iter_mut().flatten() {
        f(&mut r.name, false)?;
    }
    for r in q.chunks.iter_mut().flatten() {
        f(&mut r.name, false)?;
    }
    for r in q.files_set.iter_mut().map(|x| &mut x.1).chain(q.files_append.iter_mut()) {
        f(&mut r.path, false)?;
        f(&mut r.member_name, false)?;
    }
    for r in q.stat_set.iter_mut().map(|x| &mut x.1).chain(q.stat_append.iter_mut()) {
        f(&mut r.path, false)?;
        f(&mut r.fstype, false)?;
    }
    for r in q.ranges.iter_mut().flat_map(|x| x.sections.iter_mut().flatten()) {
        f(&mut r.name, false)?;
        f(&mut r.anchor, r.anchor_kind == 1)?;
    }
    for r in q.groups.put.iter_mut().chain(q.groups.delete.iter_mut()) {
        f(&mut r.signature, true)?;
    }
    for d in q.diags.put.iter_mut().chain(q.diags.delete.iter_mut()) {
        for x in &mut d.subjects {
            if let Subject::OdrKey { name, .. } = x {
                f(name, false)?;
            }
        }
    }
    for r in q.symbols.set.iter_mut().map(|x| &mut x.1).chain(q.symbols.append.iter_mut()) {
        f(&mut r.name, false)?;
        f(&mut r.version, false)?;
    }
    Ok(())
}

/// The patch with its strings placed (F-11, IN-18e): a string whose bytes
/// the state already holds WHERE THE UPDATE LOOKS ANYWAY names the stored
/// `Str`: the old rows of the same file (`SECTIONS` names and anchors),
/// the old row a set replaces (`FILES`, `STAT`, `SYMBOLS`,
/// `META`, the whole tables `PROBES`, `OUTSECS`, `CHUNKS`), and a
/// signature's `GROUPS` rows (the key-keyed lookup by bytes). Every other
/// string is appended once (equal bytes within the patch share one
/// place). No global index, no whole read of `STRINGS`; a string no row
/// names is dropped. The same placed patch feeds the in-place plan and
/// the compaction, so both see one string space.
#[allow(clippy::too_many_lines)] // one source per table
fn intern_patch(core: &Core, p: &StatePatch) -> Rows<(StatePatch, Strs)> {
    use crate::format::{ChunkRow, OutSecRow, ProbeRow, SectionRow};
    use crate::update::rows::{FileRow, GroupRow, MetaRow, SymbolRow};
    use std::collections::HashMap;
    let old = core.entry(kind::STRINGS).rows;
    let mut strs = Strs { old, map: BTreeMap::new() };
    let mut q = p.clone();
    q.strings = Vec::new();
    let fresh = |x: Str| x.len != 0 && u64::from(x.off) + u64::from(x.len) > old;
    let bytes_of = |x: Str| -> Rows<&[u8]> {
        let start = u64::from(x.off)
            .checked_sub(old)
            .ok_or_else(|| corrupt("STRINGS: a patch string straddles the state's end"))?;
        usize::try_from(start)
            .ok()
            .and_then(|a| a.checked_add(x.len as usize).map(|b| (a, b)))
            .and_then(|(a, b)| p.strings.get(a..b))
            .ok_or_else(|| corrupt("STRINGS: a patch string past the patch's bytes"))
    };
    let mut known: HashMap<Vec<u8>, Str> = HashMap::new();
    let learn = |known: &mut HashMap<Vec<u8>, Str>, x: Str| -> Rows<()> {
        if x.len != 0 && !fresh(x) {
            let b = core.string(x)?;
            known.entry(b).or_insert(x);
        }
        Ok(())
    };
    // The old rows the patch replaces, read only when a new row has a
    // string of the patch.
    if let Some(m) = &p.meta
        && fresh(m.version)
    {
        let o: MetaRow = core.row(kind::META, 0)?;
        learn(&mut known, o.version)?;
    }
    if p.probes.iter().flatten().any(|r| fresh(r.path)) {
        for r in core.all::<ProbeRow>(kind::PROBES)? {
            learn(&mut known, r.path)?;
        }
    }
    if p.outsecs.iter().flatten().any(|r| fresh(r.name)) {
        for r in core.all::<OutSecRow>(kind::OUTSECS)? {
            learn(&mut known, r.name)?;
        }
    }
    if p.chunks.iter().flatten().any(|r| fresh(r.name)) {
        for r in core.all::<ChunkRow>(kind::CHUNKS)? {
            learn(&mut known, r.name)?;
        }
    }
    let files = core.entry(kind::FILES).rows;
    for (f, r) in &p.files_set {
        if (fresh(r.path) || fresh(r.member_name)) && u64::from(f.0) < files {
            let o: FileRow = core.row(kind::FILES, u64::from(f.0))?;
            learn(&mut known, o.path)?;
            learn(&mut known, o.member_name)?;
        }
    }
    let stats = core.entry(kind::STAT).rows;
    for (i, r) in &p.stat_set {
        if (fresh(r.path) || fresh(r.fstype)) && u64::from(*i) < stats {
            let o: StatRow = core.row(kind::STAT, u64::from(*i))?;
            learn(&mut known, o.path)?;
            learn(&mut known, o.fstype)?;
        }
    }
    let indexed = core.entry(kind::FILE_INDEX).rows;
    for fr in &p.ranges {
        let new = fr.sections.iter().flatten().any(|r| fresh(r.name) || fresh(r.anchor));
        if new && u64::from(fr.file.0) < indexed {
            for r in core.range::<SectionRow>(fr.file, FileTable::Sections)? {
                learn(&mut known, r.name)?;
                learn(&mut known, r.anchor)?;
            }
        }
    }
    let syms = core.entry(kind::SYMBOLS).rows;
    for (k, r) in &p.symbols.set {
        if (fresh(r.name) || fresh(r.version)) && u64::from(k.0) < syms {
            let o: SymbolRow = core.row(kind::SYMBOLS, u64::from(k.0))?;
            learn(&mut known, o.name)?;
            learn(&mut known, o.version)?;
        }
    }
    // Place every string of the rows.
    let mut placed: HashMap<Vec<u8>, Str> = HashMap::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut place = |x: &mut Str, signature: bool| -> Rows<()> {
        if !fresh(*x) {
            return Ok(());
        }
        if let Some(&y) = strs.map.get(&(x.off, x.len)) {
            *x = y;
            return Ok(());
        }
        let b = bytes_of(*x)?;
        let mut y = known.get(b).copied();
        if y.is_none() && signature {
            let lo = (b.to_vec(), 0);
            let rows = core.keyed::<GroupRow>(kind::GROUPS, &lo, &|k| k.0.as_slice() == b)?;
            if let Some(g) = rows.first() {
                known.insert(b.to_vec(), g.signature);
                y = Some(g.signature);
            }
        }
        let y = if let Some(y) = y.or_else(|| placed.get(b).copied()) {
            y
        } else {
            let off = old + buf.len() as u64;
            let y = Str {
                off: u32::try_from(off).map_err(|_| corrupt("STRINGS: past 4 GiB"))?,
                len: x.len,
            };
            buf.extend_from_slice(b);
            placed.insert(b.to_vec(), y);
            y
        };
        strs.map.insert((x.off, x.len), y);
        *x = y;
        Ok(())
    };
    each_str(&mut q, &mut place)?;
    q.strings = buf;
    local_ordinals(core, &mut q)?;
    Ok((q, strs))
}

/// U-3's §R.7 finding (WP-214 §10): a new local key's `SYMBOLS.ordinal`
/// (IN-18e: among its file's locals with the same name, by symbol-table
/// index, as the capture's `LocalIdentity::ordinal`) is written here, the
/// update having no local numbering: the appended locals of one file are
/// that file's every local (a changed file's locals are all new keys,
/// §R.7), so the ordinal is counted among them. `q`'s strings are placed.
fn local_ordinals(core: &Core, q: &mut StatePatch) -> Rows<()> {
    use crate::update::rows::sym_kind;
    let old = core.entry(kind::STRINGS).rows;
    let mut locals: Vec<(u32, Vec<u8>, u32, usize)> = Vec::new();
    for (i, r) in q.symbols.append.iter().enumerate() {
        if r.kind != sym_kind::LOCAL {
            continue;
        }
        let name = if u64::from(r.name.off) >= old && r.name.len != 0 {
            let a = usize::try_from(u64::from(r.name.off) - old).unwrap_or(usize::MAX);
            q.strings
                .get(a..a.saturating_add(r.name.len as usize))
                .ok_or_else(|| corrupt("STRINGS: a local's name past the patch's bytes"))?
                .to_vec()
        } else {
            core.string(r.name)?
        };
        locals.push((r.file, name, r.at_index, i));
    }
    locals.sort();
    let mut n = 0u32;
    for k in 0..locals.len() {
        n = if k > 0 && (locals[k - 1].0, &locals[k - 1].1) == (locals[k].0, &locals[k].1) {
            n + 1
        } else {
            0
        };
        q.symbols.append[locals[k].3].ordinal = n;
    }
    Ok(())
}
