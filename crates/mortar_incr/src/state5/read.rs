//! The lazy per-block reader of a format-5 state (`spec/incr.md` IN-18c,
//! IN-18d; IN-70 step U1). WP-213 (U-1): the entry point [`open`] and the
//! adapter [`LayoutOf`]; filled by U-2 (WP-214).
//!
//! [`open`] reads and checks the header and the directory (two positioned
//! reads); every other read is of the blocks a row, a range or a key's
//! rows lie in, each hashed against its block-list entry (the list hashed
//! against the directory on its first use) before a byte of it is used,
//! and kept for the view's life. Nothing here reads a table whole but the
//! "whole" class and [`decode`] (the compaction, `incr_state`, tests).

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use mortar_core::demands::DemandFlags;
use mortar_core::incr::{
    IncrReason, LayoutRows, StoredEhRange, StoredFrag, StoredFragUse, StoredKeyRef, StoredRegion,
    StoredSection, StoredSlot, SymtabBlock, SymtabTables,
};
use mortar_core::layout::{
    AllocKey, Chunk, ChunkKind, DefValue, Defined, Erratum, Hole, Part, Patch, PatchKind, Place,
    Redirect, Segment, SymPlace, Synth, ThunkTarget, Tls,
};
use mortar_core::{ChunkId, FileId, OutSecId, SectionId, SymEntry, SymKey, SymbolId};
use mortar_elf::SymbolIndex;

use super::{
    BLOCK, DiagRaw, ENTRY, Entry, FILE_TABLES, HEADER, Header, Keyed, Rec, StrFn, TABLES, corrupt,
    decode_rows, diag_row, name, parse_directory, table_index,
};
use crate::format::{
    ChunkRow, DefinedRow, Hash16, LayoutRow, NONE, OutSecRow, PatchRow, ProbeRow, RedirectRow,
    SectionRow, SegmentRow, SizeRow, StatRow, StateError, Str, SymPlaceRow, chunk_bits, kind,
    layout_bits, patch_kind,
};
use crate::update::diags::DiagRow;
use crate::update::rows::{
    CommonRow, CopyRow, EdgeRow, FileIndexRow, FileRow, FileTable, FragRow, FragUseRow, GroupRow,
    HoleRow, IndexRow, KeyRefRow, MentionRow, MetaRow, NameRow, OdrClassRow, OdrRefRow, RegionRow,
    ResolvedRow, SlotRow, SymbolRow, SymtabKeyRow, SymtabRow, file_flags, sym_kind,
};
use crate::update::testing::MemoryState;
use crate::update::view::{Rows, StateRows, StateView};

/// Where a state's bytes are read from.
#[derive(Debug)]
pub(crate) enum Src {
    /// U1's descriptor (a duplicate of it: the same open file).
    File(File),
    /// Bytes in memory (`incr_state`, tests).
    Bytes(Vec<u8>),
}

impl Src {
    fn read_at(&self, buf: &mut [u8], off: u64) -> io::Result<()> {
        match self {
            Self::File(f) => f.read_exact_at(buf, off),
            Self::Bytes(b) => {
                let start = usize::try_from(off).map_err(|_| io::ErrorKind::UnexpectedEof)?;
                let src = start
                    .checked_add(buf.len())
                    .and_then(|end| b.get(start..end))
                    .ok_or(io::ErrorKind::UnexpectedEof)?;
                buf.copy_from_slice(src);
                Ok(())
            }
        }
    }
    fn len(&self) -> io::Result<u64> {
        match self {
            Self::File(f) => Ok(f.metadata()?.len()),
            Self::Bytes(b) => Ok(b.len() as u64),
        }
    }
    /// The descriptor W5 writes through.
    pub(crate) fn file(&self) -> Option<&File> {
        match self {
            Self::File(f) => Some(f),
            Self::Bytes(_) => None,
        }
    }
}

/// Verified blocks by (table, block).
type Blocks = HashMap<(usize, u64), Arc<Vec<u8>>>;

/// An open state: the checked header and directory, the verified lists
/// and blocks read so far, and the bytes read (IN-18c's test counts them).
#[derive(Debug)]
pub(crate) struct Core {
    pub(crate) src: Src,
    pub(crate) header: Header,
    pub(crate) dir: Vec<Entry>,
    lists: Mutex<HashMap<usize, Arc<Vec<Hash16>>>>,
    blocks: Mutex<Blocks>,
    read: AtomicU64,
}

fn io_corrupt(what: &str, e: &io::Error) -> IncrReason {
    corrupt(format!("{what}: {e}"))
}

impl Core {
    /// Read and check the header and the directory; `plan_hash` (when
    /// given) against the header's.
    pub(crate) fn open(src: Src, plan_hash: Option<[u8; 16]>) -> Result<Self, IncrReason> {
        let core = Self {
            src,
            header: Header::default(),
            dir: Vec::new(),
            lists: Mutex::new(HashMap::new()),
            blocks: Mutex::new(HashMap::new()),
            read: AtomicU64::new(0),
        };
        let mut head = [0u8; HEADER];
        core.pread(&mut head, 0).map_err(|e| io_corrupt("the header", &e))?;
        let header = Header::parse(&head).map_err(IncrReason::from)?;
        let len = core.src.len().map_err(|e| io_corrupt("the state's length", &e))?;
        if len != header.len {
            return Err(corrupt(format!("{len} bytes, the header says {}", header.len)));
        }
        let mut dir = vec![0u8; TABLES.len() * ENTRY];
        core.pread(&mut dir, HEADER as u64).map_err(|e| io_corrupt("the directory", &e))?;
        let dir = parse_directory(&header, &dir).map_err(IncrReason::from)?;
        let rows = |k: u32| table_index(k).map_or(0, |i| dir[i].rows);
        if rows(kind::FILES) != rows(kind::FILE_INDEX)
            || rows(kind::SYMBOLS) != rows(kind::RESOLVED)
            || rows(kind::SYMBOLS) != rows(kind::SYMTAB_KEYS)
            || rows(kind::META) != 1
            || rows(kind::LAYOUT) != 1
        {
            return Err(corrupt("parallel tables of different lengths"));
        }
        if let Some(want) = plan_hash
            && want != header.plan_hash
        {
            return Err(IncrReason::OptionsChanged("the plan hash differs".to_owned()));
        }
        Ok(Self { header, dir, ..core })
    }

    fn pread(&self, buf: &mut [u8], off: u64) -> io::Result<()> {
        self.read.fetch_add(buf.len() as u64, Ordering::Relaxed);
        self.src.read_at(buf, off)
    }

    /// Bytes read so far.
    pub(crate) fn bytes_read(&self) -> u64 {
        self.read.load(Ordering::Relaxed)
    }

    pub(crate) fn entry(&self, k: u32) -> &Entry {
        &self.dir[table_index(k).unwrap_or(0)]
    }

    /// Table `t`'s block list, checked against its directory entry.
    pub(crate) fn list(&self, t: usize) -> Rows<Arc<Vec<Hash16>>> {
        if let Some(l) = self.lists.lock().unwrap_or_else(PoisonError::into_inner).get(&t) {
            return Ok(Arc::clone(l));
        }
        let e = &self.dir[t];
        let mut bytes = vec![0u8; usize::try_from(e.list_len()).unwrap_or(usize::MAX)];
        self.pread(&mut bytes, e.list).map_err(|err| io_corrupt(name(e.kind), &err))?;
        if mortar_digest::l128(&bytes) != e.list_hash {
            return Err(corrupt(format!("{}: block list hash differs", name(e.kind))));
        }
        let list: Vec<Hash16> = bytes
            .chunks_exact(16)
            .map(|c| {
                let mut h = [0u8; 16];
                h.copy_from_slice(c);
                h
            })
            .collect();
        let list = Arc::new(list);
        self.lists.lock().unwrap_or_else(PoisonError::into_inner).insert(t, Arc::clone(&list));
        Ok(list)
    }

    /// Block `k` of table `t`'s body, checked against its list entry;
    /// kept when `keep`.
    pub(crate) fn block(&self, t: usize, k: u64, keep: bool) -> Rows<Arc<Vec<u8>>> {
        if let Some(b) = self.blocks.lock().unwrap_or_else(PoisonError::into_inner).get(&(t, k)) {
            return Ok(Arc::clone(b));
        }
        let e = &self.dir[t];
        let list = self.list(t)?;
        let want = list
            .get(usize::try_from(k).unwrap_or(usize::MAX))
            .ok_or_else(|| corrupt(format!("{}: block {k} out of range", name(e.kind))))?;
        let start = k * BLOCK as u64;
        let len = (e.body_len() - start).min(BLOCK as u64);
        let mut bytes = vec![0u8; usize::try_from(len).unwrap_or(0)];
        self.pread(&mut bytes, e.body + start).map_err(|err| io_corrupt(name(e.kind), &err))?;
        if mortar_digest::l128(&bytes) != *want {
            return Err(corrupt(format!("{} block {k}: hash differs", name(e.kind))));
        }
        let bytes = Arc::new(bytes);
        if keep {
            self.blocks
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert((t, k), Arc::clone(&bytes));
        }
        Ok(bytes)
    }

    /// Bytes `start .. start + len` of table `t`'s body, every block they
    /// lie in verified.
    pub(crate) fn body(&self, t: usize, start: u64, len: u64, keep: bool) -> Rows<Vec<u8>> {
        let e = &self.dir[t];
        let end = start.checked_add(len).filter(|&end| end <= e.body_len());
        let end = end.ok_or_else(|| corrupt(format!("{}: bytes out of range", name(e.kind))))?;
        let mut out = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
        let mut at = start;
        while at < end {
            let k = at / BLOCK as u64;
            let b = self.block(t, k, keep)?;
            let from = usize::try_from(at - k * BLOCK as u64).unwrap_or(0);
            let to = usize::try_from((end - k * BLOCK as u64).min(BLOCK as u64)).unwrap_or(0);
            out.extend_from_slice(b.get(from..to).unwrap_or_default());
            at = (k + 1) * BLOCK as u64;
        }
        Ok(out)
    }

    /// Rows `first .. first + count` of table `k` (inside its `rows`).
    pub(crate) fn rows<T: Rec>(&self, k: u32, first: u64, count: u64) -> Rows<Vec<T>> {
        self.rows_kept::<T>(k, first, count, true)
    }

    fn rows_kept<T: Rec>(&self, k: u32, first: u64, count: u64, keep: bool) -> Rows<Vec<T>> {
        let t = table_index(k).ok_or_else(|| corrupt("an unknown table"))?;
        let e = &self.dir[t];
        if first.checked_add(count).is_none_or(|end| end > e.rows) {
            return Err(corrupt(format!(
                "{}: rows {first}..+{count} out of range ({} rows)",
                name(k),
                e.rows
            )));
        }
        let w = u64::from(e.width);
        let bytes = self.body(t, first * w, count * w, keep)?;
        decode_rows(&bytes, k, first)
    }

    pub(crate) fn row<T: Rec>(&self, k: u32, at: u64) -> Rows<T> {
        self.rows::<T>(k, at, 1)?.pop().ok_or_else(|| corrupt(name(k)))
    }

    /// Every row of table `k` (a whole read: not kept).
    pub(crate) fn all<T: Rec>(&self, k: u32) -> Rows<Vec<T>> {
        let rows = self.entry(k).rows;
        self.rows_kept::<T>(k, 0, rows, false)
    }

    /// Bytes of `STRINGS`.
    pub(crate) fn string(&self, at: Str) -> Rows<Vec<u8>> {
        if at.len == 0 {
            return Ok(Vec::new());
        }
        let t = table_index(kind::STRINGS).unwrap_or(0);
        let e = &self.dir[t];
        let end = u64::from(at.off) + u64::from(at.len);
        if end > e.rows {
            return Err(corrupt("STRINGS: a string out of range"));
        }
        self.body(t, u64::from(at.off), u64::from(at.len), true)
    }

    /// A file's `FILE_INDEX` row, its ranges checked against the tables'
    /// extents.
    pub(crate) fn file_index(&self, file: FileId) -> Rows<FileIndexRow> {
        let row: FileIndexRow = self.row(kind::FILE_INDEX, u64::from(file.0))?;
        for t in FILE_TABLES {
            let r = row.ranges[t as usize];
            let extent = self.entry(t.kind()).rows;
            if u64::from(r.first) + u64::from(r.capacity) > extent && r.capacity != 0 {
                return Err(corrupt(format!(
                    "FILE_INDEX row {}: {} range out of range",
                    file.0,
                    name(t.kind())
                )));
            }
        }
        if u64::from(row.locals.first) + u64::from(row.locals.count)
            > self.entry(kind::SYMBOLS).rows
            && row.locals.count != 0
        {
            return Err(corrupt(format!("FILE_INDEX row {}: locals out of SYMBOLS", file.0)));
        }
        Ok(row)
    }

    /// A file's rows of a file-keyed table (none for a gone file).
    pub(crate) fn range<T: Rec>(&self, file: FileId, t: FileTable) -> Rows<Vec<T>> {
        let idx = self.file_index(file)?;
        if idx.gone {
            return Ok(Vec::new());
        }
        let r = idx.ranges[t as usize];
        self.rows(t.kind(), u64::from(r.first), u64::from(r.count))
    }

    /// The rows of a key-keyed table whose key is at least `lo` and while
    /// `more` holds, from the sorted body and the sorted append area
    /// (two binary searches; IN-18e), tombstones left out, in key order.
    #[allow(clippy::many_single_char_names)]
    pub(crate) fn keyed<T: Keyed>(
        &self,
        k: u32,
        lo: &T::K,
        more: &dyn Fn(&T::K) -> bool,
    ) -> Rows<Vec<T>> {
        let e = *self.entry(k);
        let s: StrFn<'_> = &|x| self.string(x);
        let mut out: Vec<(T::K, T)> = Vec::new();
        for (from, to) in [(0, e.aux), (e.aux, e.rows)] {
            let (mut a, mut b) = (from, to);
            while a < b {
                let mid = a + (b - a) / 2;
                let row: T = self.row(k, mid)?;
                if row.key(s)? < *lo {
                    a = mid + 1;
                } else {
                    b = mid;
                }
            }
            let mut last: Option<T::K> = None;
            let mut at = a;
            while at < to {
                let row: T = self.row(k, at)?;
                let key = row.key(s)?;
                if !more(&key) {
                    break;
                }
                if last.as_ref().is_some_and(|l| *l > key) {
                    return Err(corrupt(format!("{} row {at}: rows not sorted", name(k))));
                }
                last = Some(key.clone());
                if !row.tomb() {
                    out.push((key, row));
                }
                at += 1;
            }
        }
        out.sort_by(|x, y| x.0.cmp(&y.0));
        Ok(out.into_iter().map(|(_, r)| r).collect())
    }
}

// ---- the view -------------------------------------------------------------------

/// The views [`open`] made, by the address of their reader: how
/// `plan_patch` and `patch`, given a `StateView`, find its descriptor and
/// verified blocks (the `StateRows` trait has no accessor for them).
static VIEWS: Mutex<Vec<(usize, Arc<Core>)>> = Mutex::new(Vec::new());

/// The reader behind a view [`open`] made.
struct Reader {
    core: Arc<Core>,
}

impl Reader {
    fn address(&self) -> usize {
        std::ptr::from_ref(self).cast::<()>() as usize
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        let me = self.address();
        VIEWS.lock().unwrap_or_else(PoisonError::into_inner).retain(|(a, _)| *a != me);
    }
}

/// The open state behind `view`, when [`open`] (or [`open_bytes`]) made it.
pub(crate) fn core_of(view: &StateView<'_>) -> Option<Arc<Core>> {
    let rows: &dyn StateRows = &**view;
    let at = std::ptr::from_ref(rows).cast::<()>() as usize;
    VIEWS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|(a, _)| *a == at)
        .map(|(_, c)| Arc::clone(c))
}

fn view_of(core: Core) -> StateView<'static> {
    let reader = Box::new(Reader { core: Arc::new(core) });
    let entry = (reader.address(), Arc::clone(&reader.core));
    VIEWS.lock().unwrap_or_else(PoisonError::into_inner).push(entry);
    StateView::new(reader)
}

/// Open the state on U1's descriptor (WP-211 §C; IN-18c): check the header
/// (magic, version 5, plan hash against `plan_hash`) and the directory in
/// full, and return a view that reads every other block on demand, each
/// hashed before use. The descriptor is read with positioned reads only
/// (through a duplicate of it, the same open file: `plan_patch` and
/// `patch` find it from the view). Called by the join (IN-70 U1) after the
/// lock and the flag (IN-77b).
///
/// # Errors
/// `state-corrupt`, `state-version` (a format-4 state), `options-changed`
/// (the plan hash).
pub fn open(file: &File, plan_hash: [u8; 16]) -> Result<StateView<'_>, IncrReason> {
    let dup = file.try_clone().map_err(|e| io_corrupt("the state's descriptor", &e))?;
    Ok(view_of(Core::open(Src::File(dup), Some(plan_hash))?))
}

/// A view over a state's bytes in memory, the plan hash not checked
/// (`incr_state`, tests): the same reader as [`open`].
///
/// # Errors
/// As [`open`].
pub fn open_bytes(bytes: Vec<u8>) -> Result<StateView<'static>, IncrReason> {
    Ok(view_of(Core::open(Src::Bytes(bytes), None)?))
}

/// The bytes the view's reader has read so far (header and directory
/// included), for IN-18c's test; `None` for a view [`open`] did not make.
#[must_use]
pub fn bytes_read(view: &StateView<'_>) -> Option<u64> {
    core_of(view).map(|c| c.bytes_read())
}

impl StateRows for Reader {
    fn count(&self, k: u32) -> u64 {
        table_index(k).map_or(0, |t| self.core.dir[t].rows)
    }
    fn plan_hash(&self) -> [u8; 16] {
        self.core.header.plan_hash
    }
    fn meta(&self) -> Rows<MetaRow> {
        self.core.row(kind::META, 0)
    }
    fn probes(&self) -> Rows<Vec<ProbeRow>> {
        self.core.all_kept(kind::PROBES)
    }
    fn outsecs(&self) -> Rows<Vec<OutSecRow>> {
        self.core.all_kept(kind::OUTSECS)
    }
    fn chunks(&self) -> Rows<Vec<ChunkRow>> {
        self.core.all_kept(kind::CHUNKS)
    }
    fn order(&self) -> Rows<Vec<u32>> {
        self.core.all_kept(kind::ORDER)
    }
    fn segments(&self) -> Rows<Vec<SegmentRow>> {
        self.core.all_kept(kind::SEGMENTS)
    }
    fn slots(&self) -> Rows<Vec<SlotRow>> {
        self.core.all_kept(kind::SLOTS)
    }
    fn sizes(&self) -> Rows<Vec<SizeRow>> {
        self.core.all_kept(kind::SIZES)
    }
    fn sym_place(&self) -> Rows<Vec<SymPlaceRow>> {
        self.core.all_kept(kind::SYM_PLACE)
    }
    fn defined(&self) -> Rows<Vec<DefinedRow>> {
        self.core.all_kept(kind::DEFINED)
    }
    fn patches(&self) -> Rows<Vec<PatchRow>> {
        self.core.all_kept(kind::PATCHES)
    }
    fn redirects(&self) -> Rows<Vec<RedirectRow>> {
        self.core.all_kept(kind::REDIRECTS)
    }
    fn sized(&self) -> Rows<Vec<SizeRow>> {
        self.core.all_kept(kind::SIZED)
    }
    fn layout(&self) -> Rows<LayoutRow> {
        self.core.row(kind::LAYOUT, 0)
    }
    fn leaves(&self) -> Rows<Vec<[u8; 32]>> {
        self.core.all_kept(kind::LEAVES)
    }
    fn file(&self, file: FileId) -> Rows<FileRow> {
        self.core.row(kind::FILES, u64::from(file.0))
    }
    fn file_index(&self, file: FileId) -> Rows<FileIndexRow> {
        self.core.file_index(file)
    }
    fn stat(&self, row: u32) -> Rows<StatRow> {
        self.core.row(kind::STAT, u64::from(row))
    }
    fn string(&self, at: Str) -> Rows<Vec<u8>> {
        self.core.string(at)
    }
    fn sections_of(&self, file: FileId) -> Rows<Vec<SectionRow>> {
        let rows: Vec<SectionRow> = self.core.range(file, FileTable::Sections)?;
        if rows.iter().any(|r| r.file != file.0) {
            return Err(corrupt(format!("SECTIONS: file {}'s range names another file", file.0)));
        }
        Ok(rows)
    }
    fn section(&self, section: SectionId) -> Rows<SectionRow> {
        self.core.row(kind::SECTIONS, u64::from(section.0))
    }
    fn mentions_of(&self, file: FileId) -> Rows<Vec<MentionRow>> {
        self.core.range(file, FileTable::Mentions)
    }
    fn index_of(&self, file: FileId) -> Rows<Vec<IndexRow>> {
        self.core.range(file, FileTable::Index)
    }
    fn edges_of(&self, file: FileId) -> Rows<Vec<EdgeRow>> {
        self.core.range(file, FileTable::Edges)
    }
    fn frag_uses_of(&self, file: FileId) -> Rows<Vec<FragUseRow>> {
        let mut rows: Vec<FragUseRow> = self.core.range(file, FileTable::FragUse)?;
        if rows.iter().any(|r| r.file != file.0) {
            return Err(corrupt(format!("FRAG_USE: file {}'s range names another file", file.0)));
        }
        rows.sort();
        Ok(rows)
    }
    fn symtab_of(&self, file: FileId) -> Rows<Option<SymtabRow>> {
        let mut rows: Vec<SymtabRow> = self.core.range(file, FileTable::Symtab)?;
        if rows.len() > 1 {
            return Err(corrupt(format!("SYMTAB: file {} has {} rows", file.0, rows.len())));
        }
        Ok(rows.pop())
    }
    fn regions_of(&self, file: FileId) -> Rows<Vec<RegionRow>> {
        let mut v: Vec<RegionRow> = self.core.range(file, FileTable::Regions)?;
        v.sort_by_key(|r| (r.out, r.category, r.ordinal));
        Ok(v)
    }
    fn key_refs(&self, key: SymbolId) -> Rows<Vec<KeyRefRow>> {
        self.core.keyed::<KeyRefRow>(kind::KEY_REFS, &(key.0, 0), &|k| k.0 == key.0)
    }
    fn group(&self, signature: &[u8]) -> Rows<Vec<GroupRow>> {
        let lo = (signature.to_vec(), 0);
        self.core.keyed::<GroupRow>(kind::GROUPS, &lo, &|k| k.0.as_slice() == signature)
    }
    fn frags_of(&self, out: OutSecId, class: u32, hash: [u8; 16]) -> Rows<Option<FragRow>> {
        let key = (out.0, class, hash);
        let mut rows = self.core.keyed::<FragRow>(kind::FRAGS, &key, &|k| *k == key)?;
        if rows.len() > 1 {
            return Err(corrupt(format!("FRAGS: {} live rows of one key", rows.len())));
        }
        Ok(rows.pop())
    }
    fn holes_in(&self, chunk: ChunkId) -> Rows<Vec<HoleRow>> {
        self.core.keyed::<HoleRow>(kind::HOLES, &(chunk.0, 0), &|k| k.0 == chunk.0)
    }
    fn commons(&self, key: SymbolId) -> Rows<Vec<CommonRow>> {
        self.core.keyed::<CommonRow>(kind::COMMONS, &(key.0, 0), &|k| k.0 == key.0)
    }
    fn copies(&self, key: SymbolId) -> Rows<Vec<CopyRow>> {
        self.core.keyed::<CopyRow>(kind::COPIES, &(key.0, 0, 0), &|k| k.0 == key.0)
    }
    fn odr_classes(&self, odr_key: u32) -> Rows<Vec<OdrClassRow>> {
        // Format 5 holds no ODR record table (WP-220 R-1, WP-236).
        let _ = odr_key;
        Ok(Vec::new())
    }
    fn odr_refs(&self, odr_key: u32) -> Rows<Vec<OdrRefRow>> {
        let _ = odr_key;
        Ok(Vec::new())
    }
    fn diags(&self) -> Rows<Vec<DiagRow>> {
        let s: StrFn<'_> = &|x| self.core.string(x);
        let raw: Vec<DiagRaw> = self.core.all_kept(kind::DIAGS)?;
        raw.iter().filter(|r| !r.tomb).map(|r| diag_row(r, s)).collect()
    }
    fn symbol(&self, key: SymbolId) -> Rows<SymbolRow> {
        self.core.row(kind::SYMBOLS, u64::from(key.0))
    }
    fn resolved(&self, key: SymbolId) -> Rows<ResolvedRow> {
        self.core.row(kind::RESOLVED, u64::from(key.0))
    }
    fn symtab_key(&self, key: SymbolId) -> Rows<SymtabKeyRow> {
        self.core.row(kind::SYMTAB_KEYS, u64::from(key.0))
    }
    fn find_key(&self, name: &[u8], version: &[u8]) -> Rows<Option<SymbolId>> {
        let hash = crate::update::rows::name_hash(name, version);
        let rows = self.core.keyed::<NameRow>(kind::NAMES, &(hash, 0), &|k| k.0 == hash)?;
        for n in rows {
            let s: SymbolRow = self.core.row(kind::SYMBOLS, u64::from(n.row))?;
            if s.kind == sym_kind::LOCAL {
                return Err(corrupt(format!("NAMES: row {} is a local", n.row)));
            }
            if self.core.string(s.name)? == name && self.core.string(s.version)? == version {
                return Ok(Some(SymbolId(n.row)));
            }
        }
        Ok(None)
    }
}

impl Core {
    /// A "whole" table's rows, kept (small, read again by the update).
    fn all_kept<T: Rec>(&self, k: u32) -> Rows<Vec<T>> {
        let rows = self.entry(k).rows;
        self.rows::<T>(k, 0, rows)
    }
}

// ---- the whole read ---------------------------------------------------------------

/// The whole state as rows (IN-18f step 1; `incr_state`, tests, `canon`):
/// every block read and verified, every range and reference checked.
/// The rows keep the state's numbering: `sections` has one row per
/// `SECTIONS` row of the extent, the rows of no live range with `file`
/// [`NONE`]; key-keyed tables keep their tombstones and row order (body,
/// then area); `file_index` is the stored one.
///
/// # Errors
/// `state-corrupt`, `state-version`.
pub fn decode(bytes: &[u8]) -> Result<MemoryState, StateError> {
    let core = Core::open(Src::Bytes(bytes.to_vec()), None).map_err(to_state_error)?;
    decode_core(&core).map_err(to_state_error)
}

/// The state's size by table (U-2's gate, `state5_size_report`): per
/// table its rows, capacity and bytes (list and body) with its share;
/// the total; and the lazy archive members' share (their `FILES`,
/// `FILE_INDEX` and `STAT` rows and their file-keyed rows) on its own
/// line.
///
/// # Errors
/// The state's own.
pub fn size_report(bytes: &[u8]) -> Result<String, StateError> {
    use std::fmt::Write as _;
    let core = Core::open(Src::Bytes(bytes.to_vec()), None).map_err(to_state_error)?;
    let total = bytes.len() as u64;
    #[allow(clippy::cast_precision_loss)] // a share for people
    let pct = |b: u64| if total == 0 { 0.0 } else { b as f64 * 100.0 / total as f64 };
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<12} {:>12} {:>12} {:>14} {:>7}",
        "table", "rows", "capacity", "bytes", "share"
    );
    let head = (HEADER + core.dir.len() * ENTRY) as u64;
    let _ = writeln!(
        out,
        "{:<12} {:>12} {:>12} {:>14} {:>6.2}%",
        "header+dir",
        "",
        "",
        head,
        pct(head)
    );
    for e in &core.dir {
        let b = e.list_len() + e.body_len();
        let _ = writeln!(
            out,
            "{:<12} {:>12} {:>12} {:>14} {:>6.2}%",
            name(e.kind),
            e.rows,
            e.capacity,
            b,
            pct(b)
        );
    }
    let _ = writeln!(out, "{:<12} {:>12} {:>12} {:>14} {:>6.2}%", "total", "", "", total, 100.0);
    // lazy members
    let files: Vec<FileRow> = core.all(kind::FILES).map_err(to_state_error)?;
    let index: Vec<FileIndexRow> = core.all(kind::FILE_INDEX).map_err(to_state_error)?;
    let mut lazy = 0u64;
    let mut n = 0u64;
    for (f, idx) in files.iter().zip(&index) {
        if f.flags & file_flags::LAZY == 0 {
            continue;
        }
        n += 1;
        lazy += (FileRow::WIDTH + FileIndexRow::WIDTH + <StatRow as Rec>::WIDTH) as u64;
        for t in FILE_TABLES {
            lazy +=
                u64::from(idx.ranges[t as usize].capacity) * u64::from(core.entry(t.kind()).width);
        }
    }
    let _ = writeln!(out, "lazy members {n} files, {lazy} bytes, {:.2}%", pct(lazy));
    Ok(out)
}

/// The `incr_state` tool's guard (U-2 brief): a state whose "updating"
/// flag (bit 0 of the flags word, IN-18c) is set is mid-update, refused.
///
/// # Errors
/// `StateError::Corrupt` naming the flag; the header's own errors.
pub fn refuse_updating(bytes: &[u8]) -> Result<(), StateError> {
    let h = Header::parse(bytes.get(..HEADER).unwrap_or(bytes))?;
    if h.flags & 1 != 0 {
        return Err(StateError::Corrupt(
            "the state's updating flag is set (an update was interrupted): refused".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn to_state_error(r: IncrReason) -> StateError {
    match r {
        IncrReason::StateVersion(d) => StateError::Version(d),
        other => StateError::Corrupt(other.report()),
    }
}

/// [`decode`] over an open state.
#[allow(clippy::too_many_lines)] // one line per table, in kind order
pub(crate) fn decode_core(c: &Core) -> Rows<MemoryState> {
    // IN-18f step 1: every block, also those past every table's rows
    // (the rows' own blocks are verified as they are read below).
    for (t, e) in c.dir.iter().enumerate() {
        let used = (e.rows * u64::from(e.width)).div_ceil(BLOCK as u64);
        for k in used..e.blocks() {
            c.block(t, k, false)?;
        }
    }
    let mut m = MemoryState {
        plan_hash: c.header.plan_hash,
        meta: c.all::<MetaRow>(kind::META)?.pop().ok_or_else(|| corrupt("META"))?,
        probes: c.all(kind::PROBES)?,
        outsecs: c.all(kind::OUTSECS)?,
        chunks: c.all(kind::CHUNKS)?,
        order: c.all(kind::ORDER)?,
        segments: c.all(kind::SEGMENTS)?,
        slots: c.all(kind::SLOTS)?,
        sizes: c.all(kind::SIZES)?,
        sym_place: c.all(kind::SYM_PLACE)?,
        defined: c.all(kind::DEFINED)?,
        patches: c.all(kind::PATCHES)?,
        redirects: c.all(kind::REDIRECTS)?,
        sized: c.all(kind::SIZED)?,
        layout: c.all::<LayoutRow>(kind::LAYOUT)?.pop().ok_or_else(|| corrupt("LAYOUT"))?,
        leaves: c.all(kind::LEAVES)?,
        files: c.all(kind::FILES)?,
        file_index: c.all(kind::FILE_INDEX)?,
        stat: c.all(kind::STAT)?,
        strings: c.all::<u8>(kind::STRINGS)?,
        symbols: c.all(kind::SYMBOLS)?,
        resolved: c.all(kind::RESOLVED)?,
        symtab_keys: c.all(kind::SYMTAB_KEYS)?,
        key_refs: c.all(kind::KEY_REFS)?,
        groups: c.all(kind::GROUPS)?,
        frags: c.all(kind::FRAGS)?,
        holes: c.all(kind::HOLES)?,
        commons: c.all(kind::COMMONS)?,
        copies: c.all(kind::COPIES)?,
        names: c.all(kind::NAMES)?,
        ..MemoryState::default()
    };
    let strings = &m.strings;
    let s: StrFn<'_> = &|x: Str| {
        let start = x.off as usize;
        start
            .checked_add(x.len as usize)
            .and_then(|end| strings.get(start..end))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| corrupt("STRINGS: a string out of range"))
    };
    let raw: Vec<DiagRaw> = c.all(kind::DIAGS)?;
    m.diags = raw.iter().map(|r| diag_row(r, s)).collect::<Rows<_>>()?;
    check_sorted::<KeyRefRow>(c, kind::KEY_REFS, &m.key_refs, s)?;
    check_sorted::<GroupRow>(c, kind::GROUPS, &m.groups, s)?;
    check_sorted::<FragRow>(c, kind::FRAGS, &m.frags, s)?;
    check_sorted::<HoleRow>(c, kind::HOLES, &m.holes, s)?;
    check_sorted::<CommonRow>(c, kind::COMMONS, &m.commons, s)?;
    check_sorted::<CopyRow>(c, kind::COPIES, &m.copies, s)?;
    check_sorted::<DiagRaw>(c, kind::DIAGS, &raw, s)?;
    check_sorted::<NameRow>(c, kind::NAMES, &m.names, s)?;
    // The file-keyed tables: per live file its range; ranges in bounds and
    // not overlapping (IN-18d).
    let extent = |t: FileTable| c.entry(t.kind()).rows;
    let mut sections: Vec<SectionRow> = vec![
        SectionRow { file: NONE, ..SectionRow::default() };
        usize::try_from(extent(FileTable::Sections))
            .unwrap_or(0)
    ];
    let all_sections: Vec<SectionRow> = c.all(kind::SECTIONS)?;
    let mentions: Vec<MentionRow> = c.all(kind::MENTIONS)?;
    let index: Vec<IndexRow> = c.all(kind::INDEX)?;
    let edges: Vec<EdgeRow> = c.all(kind::EDGES)?;
    let frag_uses: Vec<FragUseRow> = c.all(kind::FRAG_USE)?;
    let symtab: Vec<SymtabRow> = c.all(kind::SYMTAB)?;
    let regions: Vec<RegionRow> = c.all(kind::REGIONS)?;
    let mut spans: Vec<Vec<(u64, u64)>> = vec![Vec::new(); FileTable::COUNT];
    for (f, idx) in m.file_index.iter().enumerate() {
        let file = FileId(u32::try_from(f).unwrap_or(NONE));
        for t in FILE_TABLES {
            let r = idx.ranges[t as usize];
            if u64::from(r.first) + u64::from(r.capacity) > extent(t) && r.capacity != 0 {
                return Err(corrupt(format!(
                    "FILE_INDEX row {f}: {} out of range",
                    name(t.kind())
                )));
            }
            if r.capacity != 0 && !idx.gone {
                spans[t as usize]
                    .push((u64::from(r.first), u64::from(r.first) + u64::from(r.capacity)));
            }
        }
        if idx.gone {
            continue;
        }
        // IN-18d: the file's locals, one run of its own LOCAL rows.
        // The end in `usize`, checked: a corrupt row (first near 2^32) is
        // `state-corrupt`, never an overflow of the `u32` sum.
        let start = idx.locals.first as usize;
        let run = start.checked_add(idx.locals.count as usize).map(|end| start..end);
        let ok = idx.locals.count == 0
            || run.and_then(|run| m.symbols.get(run)).is_some_and(|rows| {
                rows.iter().all(|r| r.kind == sym_kind::LOCAL && r.file == file.0)
            });
        if !ok {
            return Err(corrupt(format!("FILE_INDEX row {f}: locals not a run of its own")));
        }
        let at = |t: FileTable| {
            let r = idx.ranges[t as usize];
            r.first as usize..r.first as usize + r.count as usize
        };
        for i in at(FileTable::Sections) {
            let row = all_sections[i].clone();
            if row.file != file.0 {
                return Err(corrupt(format!("SECTIONS row {i}: in file {f}'s range")));
            }
            sections[i] = row;
        }
        m.mentions.extend(mentions[at(FileTable::Mentions)].iter().map(|r| (file, *r)));
        m.index.extend(index[at(FileTable::Index)].iter().map(|r| (file, *r)));
        m.edges.extend(edges[at(FileTable::Edges)].iter().map(|r| (file, *r)));
        for i in at(FileTable::FragUse) {
            if frag_uses[i].file != file.0 {
                return Err(corrupt(format!("FRAG_USE row {i}: in file {f}'s range")));
            }
            m.frag_uses.push((file, frag_uses[i]));
        }
        m.symtab.extend(symtab[at(FileTable::Symtab)].iter().map(|r| (file, *r)));
        m.regions.extend(regions[at(FileTable::Regions)].iter().map(|r| (file, *r)));
        if idx.ranges[FileTable::Symtab as usize].count > 1 {
            return Err(corrupt(format!("SYMTAB: file {f} has more than one row")));
        }
    }
    for t in FILE_TABLES {
        let mut v = std::mem::take(&mut spans[t as usize]);
        v.sort_unstable();
        if v.windows(2).any(|w| w[0].1 > w[1].0) {
            return Err(corrupt(format!("FILE_INDEX: two {} ranges overlap", name(t.kind()))));
        }
    }
    m.sections = sections;
    Ok(m)
}

/// A key-keyed table's body and its append area are each sorted.
fn check_sorted<T: Keyed>(c: &Core, k: u32, rows: &[T], s: StrFn<'_>) -> Rows<()> {
    let aux = usize::try_from(c.entry(k).aux).unwrap_or(0);
    let dead = rows.iter().filter(|r| r.tomb()).count() as u64;
    if dead != c.entry(k).dead {
        return Err(corrupt(format!(
            "{}: {dead} tombstones, the directory says {}",
            name(k),
            c.entry(k).dead
        )));
    }
    for part in [&rows[..aux.min(rows.len())], &rows[aux.min(rows.len())..]] {
        let keys = part.iter().map(|r| r.key(s)).collect::<Rows<Vec<_>>>()?;
        if keys.windows(2).any(|w| w[0] >= w[1]) {
            return Err(corrupt(format!("{}: rows not sorted", name(k))));
        }
    }
    Ok(())
}

// ---- the stored layout ---------------------------------------------------------------

/// The stored layout of a state (`mortar_core::incr::LayoutRows` over a
/// [`StateView`]): the `Placement` rows in the core's types, which U-4 and
/// U-5 read through `StoredLayout::new(&LayoutOf { view })`. The rows to
/// core types translation is the inverse of the capture (U-2's): ids are
/// the state's rows (the update's id space); a symbol named by a
/// `SYMBOLS` row is `SymKey::global(row)` for a global and
/// `SymKey::local((file, index))` for a local.
#[derive(Debug, Clone, Copy)]
pub struct LayoutOf<'v, 's> {
    pub view: &'v StateView<'s>,
}

/// The names of synthetic chunks, each made `'static` once (`Synth::name`
/// is `&'static [u8]`; the state holds the bytes).
static SYNTH_NAMES: Mutex<Vec<&'static [u8]>> = Mutex::new(Vec::new());

fn synth_name(n: &[u8]) -> &'static [u8] {
    let mut names = SYNTH_NAMES.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(&s) = names.iter().find(|s| **s == n) {
        return s;
    }
    let s: &'static [u8] = Box::leak(n.to_vec().into_boxed_slice());
    names.push(s);
    s
}

impl LayoutOf<'_, '_> {
    #[allow(clippy::trivially_copy_pass_by_ref)] // a view is borrowed
    fn sym_key(&self, row: u32) -> Rows<SymKey> {
        let s = self.view.symbol(SymbolId(row))?;
        Ok(if s.kind == sym_kind::LOCAL {
            SymKey::local(SymEntry { file: FileId(s.file), index: SymbolIndex(s.at_index) })
        } else {
            SymKey::global(SymbolId(row))
        })
    }

    /// A `SECTIONS` row as the core's stored section.
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn stored_section(
        &self,
        sid: SectionId,
        file: FileId,
        index: u32,
        r: &SectionRow,
    ) -> Rows<StoredSection> {
        Ok(StoredSection {
            id: sid,
            file,
            index,
            anchor_kind: r.anchor_kind,
            anchor: self.view.string(r.anchor)?,
            name: self.view.string(r.name)?,
            ordinal: r.ordinal,
            sh_type: r.sh_type,
            sh_flags: r.sh_flags,
            size: r.size,
            align: r.align,
            place: if r.place_chunk == NONE {
                Place::NONE
            } else {
                Place::at(ChunkId(r.place_chunk), r.place_offset)
            },
        })
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn global(&self, row: u32) -> Rows<SymbolId> {
        self.sym_key(row)?.as_global().ok_or_else(|| corrupt(format!("SYMBOLS row {row}: a local")))
    }
}

/// A `CHUNKS` row as a core chunk.
pub(crate) fn chunk_of(c: &ChunkRow, name: &[u8]) -> Rows<Chunk> {
    use mortar_core::identity::ChunkTag;
    let tag = crate::capture::TAGS.get(c.tag as usize).ok_or_else(|| corrupt("CHUNKS: tag"))?;
    let part = Part { out: OutSecId(c.out), part: c.part };
    let kind = match tag {
        ChunkTag::FileHeader => ChunkKind::FileHeader,
        ChunkTag::ProgramHeaders => ChunkKind::ProgramHeaders,
        ChunkTag::SectionHeaders => ChunkKind::SectionHeaders,
        ChunkTag::Body => ChunkKind::Body(part),
        ChunkTag::Patches => ChunkKind::Patches(part),
        ChunkTag::Merged => ChunkKind::Merged(OutSecId(c.out)),
        ChunkTag::Relocs => ChunkKind::Relocs(OutSecId(c.out)),
        ChunkTag::Synthetic => ChunkKind::Synthetic(Synth {
            name: synth_name(name),
            address_dependent: c.bits & chunk_bits::ADDRESS_DEPENDENT != 0,
        }),
        ChunkTag::Group => return Err(corrupt("CHUNKS: a group chunk (-r) in a link state")),
    };
    Ok(Chunk {
        kind,
        sh_type: c.sh_type,
        flags: c.sh_flags,
        align: c.align,
        size: c.size,
        capacity: c.capacity,
        addr: (c.bits & chunk_bits::HAS_ADDR != 0).then_some(c.addr),
        offset: c.offset,
        shndx: (c.bits & chunk_bits::HAS_SHNDX != 0).then_some(c.shndx),
        relro: c.bits & chunk_bits::RELRO != 0,
    })
}

fn id(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(NONE)
}

impl LayoutRows for LayoutOf<'_, '_> {
    fn chunks(&self) -> Result<Vec<(ChunkId, Chunk)>, IncrReason> {
        let rows = self.view.chunks()?;
        let mut out = Vec::with_capacity(rows.len());
        for (i, c) in rows.iter().enumerate() {
            out.push((ChunkId(id(i)), chunk_of(c, &self.view.string(c.name)?)?));
        }
        Ok(out)
    }
    fn order(&self) -> Result<Vec<ChunkId>, IncrReason> {
        Ok(self.view.order()?.into_iter().map(ChunkId).collect())
    }
    fn segments(&self) -> Result<Vec<Segment>, IncrReason> {
        self.view
            .segments()?
            .into_iter()
            .map(|g| {
                Ok(Segment {
                    p_type: g.p_type,
                    flags: g.p_flags,
                    offset: g.offset,
                    addr: g.addr,
                    filesz: g.filesz,
                    memsz: g.memsz,
                    align: u8::try_from(g.align).map_err(|_| corrupt("SEGMENTS: alignment"))?,
                })
            })
            .collect()
    }
    fn outsecs(&self) -> Result<Vec<(OutSecId, Vec<u8>)>, IncrReason> {
        let rows = self.view.outsecs()?;
        let mut out = Vec::with_capacity(rows.len());
        for (i, o) in rows.iter().enumerate() {
            out.push((OutSecId(id(i)), self.view.string(o.name)?));
        }
        Ok(out)
    }
    fn sections_of(&self, file: FileId) -> Result<Vec<StoredSection>, IncrReason> {
        let idx = self.view.file_index(file)?;
        let first = idx.ranges[FileTable::Sections as usize].first;
        let rows = self.view.sections_of(file)?;
        let mut out = Vec::with_capacity(rows.len());
        for (k, r) in rows.iter().enumerate() {
            out.push(self.stored_section(SectionId(first + id(k)), file, id(k), r)?);
        }
        Ok(out)
    }
    fn section(&self, sid: SectionId) -> Result<Option<StoredSection>, IncrReason> {
        // Past the extent: new to the update. A row of no file's live
        // range (slack; a gone file's) is no stored section either.
        if u64::from(sid.0) >= self.view.count(kind::SECTIONS) {
            return Ok(None);
        }
        // A row is its file's when it lies in that file's live range (a
        // slack row is zeros: "file 0", outside file 0's range).
        let r = self.view.section(sid)?;
        if r.file == NONE || u64::from(r.file) >= self.view.count(kind::FILE_INDEX) {
            return Ok(None);
        }
        let file = FileId(r.file);
        let idx = self.view.file_index(file)?;
        let range = idx.ranges[FileTable::Sections as usize];
        if idx.gone || sid.0 < range.first || sid.0 - range.first >= range.count {
            return Ok(None);
        }
        Ok(Some(self.stored_section(sid, file, sid.0 - range.first, &r)?))
    }
    fn regions_of(&self, file: FileId) -> Result<Vec<StoredRegion>, IncrReason> {
        Ok(self
            .view
            .regions_of(file)?
            .into_iter()
            .map(|r| StoredRegion {
                file,
                out: OutSecId(r.out),
                category: r.category,
                ordinal: r.ordinal,
                chunk: ChunkId(r.chunk),
                offset: r.offset,
                capacity: r.capacity,
            })
            .collect())
    }
    fn holes_in(&self, chunk: ChunkId) -> Result<Vec<Hole>, IncrReason> {
        self.view
            .holes_in(chunk)?
            .into_iter()
            .map(|h| {
                Ok(Hole {
                    chunk,
                    offset: h.offset,
                    size: h.size,
                    kind: crate::capture::hole_of(h.kind).map_err(corrupt)?,
                })
            })
            .collect()
    }
    fn frags_of(
        &self,
        out: OutSecId,
        class: u32,
        hash: [u8; 16],
    ) -> Result<Option<StoredFrag>, IncrReason> {
        Ok(self.view.frags_of(out, class, hash)?.map(|f| StoredFrag {
            out,
            class,
            hash,
            offset: f.offset,
            size: f.size,
            refs: f.refs,
        }))
    }
    fn frag_uses_of(&self, file: FileId) -> Result<Vec<StoredFragUse>, IncrReason> {
        Ok(self
            .view
            .frag_uses_of(file)?
            .into_iter()
            .map(|r| StoredFragUse { file, out: OutSecId(r.out), class: r.class, hash: r.hash })
            .collect())
    }
    fn eh_range_of(&self, file: FileId) -> Result<Option<StoredEhRange>, IncrReason> {
        let f = self.view.file(file)?;
        Ok((f.eh_records != 0).then_some(StoredEhRange {
            offset: f.eh_offset,
            capacity: f.eh_capacity,
            records: f.eh_records,
            fdes: f.eh_fdes,
        }))
    }
    /// `FILES` read whole (the named read of IN-18c, WP-243): every file
    /// not `GONE` with kept records, by its current rank.
    fn eh_ranges(&self) -> Result<Vec<(FileId, u64, StoredEhRange)>, IncrReason> {
        let n = u32::try_from(self.view.count(kind::FILES))
            .map_err(|_| corrupt("FILES over 2^32 rows"))?;
        let mut out = Vec::new();
        for i in 0..n {
            let f = self.view.file(FileId(i))?;
            if f.eh_records == 0 || f.flags & file_flags::GONE != 0 {
                continue;
            }
            let range = StoredEhRange {
                offset: f.eh_offset,
                capacity: f.eh_capacity,
                records: f.eh_records,
                fdes: f.eh_fdes,
            };
            out.push((FileId(i), f.rank, range));
        }
        out.sort_by_key(|&(f, rank, _)| (rank, f));
        Ok(out)
    }
    fn key_refs_of(&self, file: FileId) -> Result<Vec<StoredKeyRef>, IncrReason> {
        let mut keys: Vec<u32> = self.view.mentions_of(file)?.iter().map(|m| m.symbol).collect();
        keys.sort_unstable();
        keys.dedup();
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            let row = self
                .view
                .key_refs(SymbolId(key))?
                .into_iter()
                .find(|r| r.file == file.0)
                .ok_or_else(|| {
                    corrupt(format!("KEY_REFS: no row of key {key} for file {}", file.0))
                })?;
            let demand = DemandFlags::from_bits(row.demand)
                .ok_or_else(|| corrupt(format!("KEY_REFS: key {key}'s demand")))?;
            out.push(StoredKeyRef { key: SymbolId(key), file, demand });
        }
        Ok(out)
    }
    fn slots(&self) -> Result<Vec<StoredSlot>, IncrReason> {
        let mut out = Vec::new();
        let mut index = [0u32; 6];
        for s in self.view.slots()? {
            let k = crate::capture::SLOT_LISTS
                .get(s.kind as usize)
                .ok_or_else(|| corrupt("SLOTS: kind"))?;
            let i = &mut index[s.kind as usize];
            out.push(StoredSlot {
                kind: *k,
                key: self.sym_key(s.symbol)?,
                index: *i,
                refs: s.refs,
            });
            *i += 1;
        }
        Ok(out)
    }
    fn patches(&self) -> Result<Vec<Patch>, IncrReason> {
        let mut out = Vec::new();
        for r in self.view.patches()? {
            let kind = match r.kind {
                patch_kind::THUNK => PatchKind::Thunk(ThunkTarget::Sym {
                    key: self.sym_key(r.symbol)?,
                    addend: r.value.cast_signed(),
                }),
                patch_kind::THUNK_PLT => {
                    PatchKind::Thunk(ThunkTarget::Plt(self.sym_key(r.symbol)?))
                }
                patch_kind::E843419 => PatchKind::Erratum {
                    erratum: Erratum::E843419,
                    section: SectionId(r.section),
                    offset: r.value,
                },
                _ => return Err(corrupt("PATCHES: kind")),
            };
            out.push(Patch { chunk: ChunkId(r.chunk), offset: r.offset, kind });
        }
        Ok(out)
    }
    fn redirects(&self) -> Result<Vec<Redirect>, IncrReason> {
        Ok(self
            .view
            .redirects()?
            .into_iter()
            .map(|r| Redirect { section: SectionId(r.section), offset: r.offset, patch: r.patch })
            .collect())
    }
    fn sym_place(&self) -> Result<Vec<SymPlace>, IncrReason> {
        let mut out = Vec::new();
        for r in self.view.sym_place()? {
            out.push(SymPlace {
                key: AllocKey::Common(self.global(r.symbol)?),
                chunk: ChunkId(r.chunk),
                offset: r.offset,
                size: r.size,
                align: r.align,
            });
        }
        Ok(out)
    }
    fn defined(&self) -> Result<Vec<Defined>, IncrReason> {
        let mut out = Vec::new();
        for d in self.view.defined()? {
            let value = if d.kind == 0 {
                DefValue::Absolute(d.value)
            } else {
                DefValue::At { chunk: ChunkId(d.chunk), offset: d.value }
            };
            out.push(Defined { symbol: self.global(d.symbol)?, value });
        }
        Ok(out)
    }
    fn tls_and_entry(&self) -> Result<(Option<Tls>, Option<u64>), IncrReason> {
        let l = self.view.layout()?;
        let tls = (l.bits & layout_bits::TLS != 0).then_some(Tls {
            start: l.tls_start,
            size: l.tls_size,
            align: l.tls_align,
        });
        Ok((tls, (l.bits & layout_bits::ENTRY != 0).then_some(l.entry)))
    }
    fn symtab_block(&self, file: FileId) -> Result<Option<SymtabBlock>, IncrReason> {
        self.view.symtab_of(file)
    }
    fn symtab_tables(&self) -> Result<(SymtabBlock, SymtabTables), IncrReason> {
        let global = self.view.symtab_of(FileId(0))?.unwrap_or_default();
        Ok((global, self.view.meta()?.symtab))
    }
}
