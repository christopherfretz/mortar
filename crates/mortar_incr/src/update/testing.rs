//! The in-memory backing of [`StateView`] (WP-213, U-1): hand-made rows
//! for the packages' own tests (each U-package is tested ALONE against
//! rows it builds here, `spec/landed/incr/WP-207.md` §C). Compiled always
//! (the packages' integration tests use it) and called by no link: a plain
//! or incremental link never builds one. Every accessor filters or copies
//! public fields: no hashing, no failure but a row out of range
//! (`state-corrupt`, as the real reader).

use mortar_core::incr::IncrReason;
use mortar_core::{ChunkId, FileId, OutSecId, SectionId, SymbolId};

use super::diags::DiagRow;
use super::rows::{
    CommonRow, EdgeRow, FileIndexRow, FileRow, FragRow, GroupRow, HoleRow, IndexRow, KeyRefRow,
    MentionRow, MetaRow, OdrClassRow, OdrRefRow, ResolvedRow, SymbolRow, SymtabKeyRow, SymtabRow,
};
use super::view::{Rows, StateRows, StateView};
use crate::format::{
    ChunkRow, DefinedRow, EhRow, FragAtRow, LayoutRow, OutSecRow, PatchRow, ProbeRow, RedirectRow,
    SectionRow, SegmentRow, SizeRow, SlotRow, StatRow, Str, SymPlaceRow, kind,
};

/// A state in memory: the tables as plain vectors. File-keyed rows are
/// kept with their file (`(FileId, row)`; `sections` use their own `file`
/// column); key-parallel tables are indexed by `SYMBOLS` row.
#[derive(Debug, Clone, Default)]
pub struct MemoryState {
    pub plan_hash: [u8; 16],
    pub meta: MetaRow,
    pub probes: Vec<ProbeRow>,
    pub outsecs: Vec<OutSecRow>,
    pub chunks: Vec<ChunkRow>,
    pub order: Vec<u32>,
    pub segments: Vec<SegmentRow>,
    pub slots: Vec<SlotRow>,
    pub sizes: Vec<SizeRow>,
    pub sym_place: Vec<SymPlaceRow>,
    pub defined: Vec<DefinedRow>,
    pub patches: Vec<PatchRow>,
    pub redirects: Vec<RedirectRow>,
    pub sized: Vec<SizeRow>,
    pub layout: LayoutRow,
    pub leaves: Vec<[u8; 32]>,
    pub files: Vec<FileRow>,
    pub file_index: Vec<FileIndexRow>,
    pub stat: Vec<StatRow>,
    pub strings: Vec<u8>,
    /// Sorted by file, then section index: a file's rows are contiguous.
    pub sections: Vec<SectionRow>,
    pub mentions: Vec<(FileId, MentionRow)>,
    pub index: Vec<(FileId, IndexRow)>,
    pub edges: Vec<(FileId, EdgeRow)>,
    pub frag_at: Vec<(FileId, FragAtRow)>,
    pub eh_records: Vec<(FileId, EhRow)>,
    pub symtab: Vec<(FileId, SymtabRow)>,
    pub key_refs: Vec<KeyRefRow>,
    pub groups: Vec<GroupRow>,
    pub frags: Vec<FragRow>,
    pub holes: Vec<HoleRow>,
    pub commons: Vec<CommonRow>,
    pub odr_classes: Vec<OdrClassRow>,
    pub odr_refs: Vec<OdrRefRow>,
    pub diags: Vec<DiagRow>,
    pub symbols: Vec<SymbolRow>,
    pub resolved: Vec<ResolvedRow>,
    pub symtab_keys: Vec<SymtabKeyRow>,
}

impl MemoryState {
    /// A [`StateView`] over these rows (borrowed).
    #[must_use]
    pub fn view(&self) -> StateView<'_> {
        StateView::new(Box::new(self))
    }

    /// Append `s` to `STRINGS` and give its `Str`.
    pub fn intern(&mut self, s: &[u8]) -> Str {
        let off = u32::try_from(self.strings.len()).unwrap_or(u32::MAX);
        self.strings.extend_from_slice(s);
        Str { off, len: u32::try_from(s.len()).unwrap_or(u32::MAX) }
    }
}

fn corrupt(what: &str) -> IncrReason {
    IncrReason::StateCorrupt(format!("{what}: row out of range"))
}

fn row<T: Clone>(rows: &[T], at: u32, what: &str) -> Rows<T> {
    rows.get(at as usize).cloned().ok_or_else(|| corrupt(what))
}

fn of<T: Copy>(rows: &[(FileId, T)], file: FileId) -> Vec<T> {
    rows.iter().filter(|(f, _)| *f == file).map(|(_, r)| *r).collect()
}

impl StateRows for &MemoryState {
    fn count(&self, table: u32) -> u64 {
        let n = match table {
            kind::FILES | kind::FILE_INDEX => self.files.len(),
            kind::SYMBOLS | kind::RESOLVED | kind::SYMTAB_KEYS => self.symbols.len(),
            kind::SECTIONS => self.sections.len(),
            kind::CHUNKS => self.chunks.len(),
            kind::OUTSECS => self.outsecs.len(),
            kind::STAT => self.stat.len(),
            kind::STRINGS => self.strings.len(),
            kind::MENTIONS => self.mentions.len(),
            kind::KEY_REFS => self.key_refs.len(),
            kind::DIAGS => self.diags.len(),
            _ => 0,
        };
        n as u64
    }
    fn plan_hash(&self) -> [u8; 16] {
        self.plan_hash
    }
    fn meta(&self) -> Rows<MetaRow> {
        Ok(self.meta.clone())
    }
    fn probes(&self) -> Rows<Vec<ProbeRow>> {
        Ok(self.probes.clone())
    }
    fn outsecs(&self) -> Rows<Vec<OutSecRow>> {
        Ok(self.outsecs.clone())
    }
    fn chunks(&self) -> Rows<Vec<ChunkRow>> {
        Ok(self.chunks.clone())
    }
    fn order(&self) -> Rows<Vec<u32>> {
        Ok(self.order.clone())
    }
    fn segments(&self) -> Rows<Vec<SegmentRow>> {
        Ok(self.segments.clone())
    }
    fn slots(&self) -> Rows<Vec<SlotRow>> {
        Ok(self.slots.clone())
    }
    fn sizes(&self) -> Rows<Vec<SizeRow>> {
        Ok(self.sizes.clone())
    }
    fn sym_place(&self) -> Rows<Vec<SymPlaceRow>> {
        Ok(self.sym_place.clone())
    }
    fn defined(&self) -> Rows<Vec<DefinedRow>> {
        Ok(self.defined.clone())
    }
    fn patches(&self) -> Rows<Vec<PatchRow>> {
        Ok(self.patches.clone())
    }
    fn redirects(&self) -> Rows<Vec<RedirectRow>> {
        Ok(self.redirects.clone())
    }
    fn sized(&self) -> Rows<Vec<SizeRow>> {
        Ok(self.sized.clone())
    }
    fn layout(&self) -> Rows<LayoutRow> {
        Ok(self.layout.clone())
    }
    fn leaves(&self) -> Rows<Vec<[u8; 32]>> {
        Ok(self.leaves.clone())
    }
    fn file(&self, file: FileId) -> Rows<FileRow> {
        row(&self.files, file.0, "FILES")
    }
    fn file_index(&self, file: FileId) -> Rows<FileIndexRow> {
        row(&self.file_index, file.0, "FILE_INDEX")
    }
    fn stat(&self, at: u32) -> Rows<StatRow> {
        row(&self.stat, at, "STAT")
    }
    fn string(&self, at: Str) -> Rows<Vec<u8>> {
        let start = at.off as usize;
        start
            .checked_add(at.len as usize)
            .and_then(|end| self.strings.get(start..end))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| corrupt("STRINGS"))
    }
    fn sections_of(&self, file: FileId) -> Rows<Vec<SectionRow>> {
        Ok(self.sections.iter().filter(|s| s.file == file.0).cloned().collect())
    }
    fn section(&self, section: SectionId) -> Rows<SectionRow> {
        row(&self.sections, section.0, "SECTIONS")
    }
    fn mentions_of(&self, file: FileId) -> Rows<Vec<MentionRow>> {
        Ok(of(&self.mentions, file))
    }
    fn index_of(&self, file: FileId) -> Rows<Vec<IndexRow>> {
        Ok(of(&self.index, file))
    }
    fn edges_of(&self, file: FileId) -> Rows<Vec<EdgeRow>> {
        Ok(of(&self.edges, file))
    }
    fn frag_at_of(&self, file: FileId) -> Rows<Vec<FragAtRow>> {
        Ok(of(&self.frag_at, file))
    }
    fn eh_records_of(&self, file: FileId) -> Rows<Vec<EhRow>> {
        Ok(self.eh_records.iter().filter(|(f, _)| *f == file).map(|(_, r)| r.clone()).collect())
    }
    fn symtab_of(&self, file: FileId) -> Rows<Option<SymtabRow>> {
        Ok(of(&self.symtab, file).first().copied())
    }
    fn key_refs(&self, key: SymbolId) -> Rows<Vec<KeyRefRow>> {
        let mut v: Vec<KeyRefRow> =
            self.key_refs.iter().filter(|r| r.key == key.0 && !r.tomb).copied().collect();
        v.sort_by_key(|r| r.file);
        Ok(v)
    }
    fn group(&self, signature: &[u8]) -> Rows<Option<GroupRow>> {
        for g in self.groups.iter().filter(|g| !g.tomb) {
            if self.string(g.signature)? == signature {
                return Ok(Some(*g));
            }
        }
        Ok(None)
    }
    fn frags_in(&self, out: OutSecId) -> Rows<Vec<FragRow>> {
        let mut v: Vec<FragRow> =
            self.frags.iter().filter(|f| f.out == out.0 && !f.tomb).copied().collect();
        v.sort_by_key(|f| f.offset);
        Ok(v)
    }
    fn holes_in(&self, chunk: ChunkId) -> Rows<Vec<HoleRow>> {
        let mut v: Vec<HoleRow> =
            self.holes.iter().filter(|h| h.chunk == chunk.0 && !h.tomb).copied().collect();
        v.sort_by_key(|h| h.offset);
        Ok(v)
    }
    fn common(&self, key: SymbolId) -> Rows<Option<CommonRow>> {
        Ok(self.commons.iter().find(|c| c.key == key.0 && !c.tomb).copied())
    }
    fn odr_classes(&self, odr_key: u32) -> Rows<Vec<OdrClassRow>> {
        Ok(self.odr_classes.iter().filter(|c| c.key == odr_key && !c.tomb).copied().collect())
    }
    fn odr_refs(&self, odr_key: u32) -> Rows<Vec<OdrRefRow>> {
        Ok(self.odr_refs.iter().filter(|c| c.key == odr_key && !c.tomb).copied().collect())
    }
    fn diags(&self) -> Rows<Vec<DiagRow>> {
        Ok(self.diags.iter().filter(|d| !d.tomb).cloned().collect())
    }
    fn symbol(&self, key: SymbolId) -> Rows<SymbolRow> {
        row(&self.symbols, key.0, "SYMBOLS")
    }
    fn resolved(&self, key: SymbolId) -> Rows<ResolvedRow> {
        row(&self.resolved, key.0, "RESOLVED")
    }
    fn symtab_key(&self, key: SymbolId) -> Rows<SymtabKeyRow> {
        row(&self.symtab_keys, key.0, "SYMTAB_KEYS")
    }
    fn find_key(&self, name: &[u8], version: &[u8]) -> Rows<Option<SymbolId>> {
        use super::rows::sym_kind;
        for (i, s) in self.symbols.iter().enumerate() {
            if matches!(s.kind, sym_kind::GLOBAL | sym_kind::VERSIONED)
                && self.string(s.name)? == name
                && self.string(s.version)? == version
            {
                return Ok(Some(SymbolId(u32::try_from(i).unwrap_or(u32::MAX))));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: incr.md IN-18e (the in-memory backing answers by key)
    #[test]
    fn memory_state_answers_by_key() {
        let mut m = MemoryState::default();
        let name = m.intern(b"main");
        m.symbols.push(SymbolRow { name, ..SymbolRow::default() });
        m.resolved.push(ResolvedRow::default());
        m.key_refs.push(KeyRefRow { key: 0, file: 2, ..KeyRefRow::default() });
        m.key_refs.push(KeyRefRow { key: 0, file: 1, ..KeyRefRow::default() });
        m.key_refs.push(KeyRefRow { key: 0, file: 3, tomb: true, ..KeyRefRow::default() });
        let v = m.view();
        assert_eq!(v.find_key(b"main", b"").unwrap(), Some(SymbolId(0)));
        assert_eq!(v.find_key(b"other", b"").unwrap(), None);
        let refs = v.key_refs(SymbolId(0)).unwrap();
        assert_eq!(refs.iter().map(|r| r.file).collect::<Vec<_>>(), [1, 2]);
        assert!(matches!(v.symbol(SymbolId(1)), Err(IncrReason::StateCorrupt(_))));
        assert_eq!(v.count(kind::SYMBOLS), 1);
    }
}
