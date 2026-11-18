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
    MentionRow, MetaRow, NameRow, OdrClassRow, OdrRefRow, RegionRow, ResolvedRow, SymbolRow,
    SymtabKeyRow, SymtabRow,
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
    pub regions: Vec<(FileId, RegionRow)>,
    pub key_refs: Vec<KeyRefRow>,
    pub groups: Vec<GroupRow>,
    pub frags: Vec<FragRow>,
    pub holes: Vec<HoleRow>,
    pub commons: Vec<CommonRow>,
    pub odr_classes: Vec<OdrClassRow>,
    pub odr_refs: Vec<OdrRefRow>,
    pub diags: Vec<DiagRow>,
    /// `NAMES`: [`MemoryState::find_key`] goes through these rows only
    /// (as U-2's reader), so a test that looks a key up adds its row
    /// ([`MemoryState::name_rows`] makes them all).
    pub names: Vec<NameRow>,
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

    /// The rows a format-4 state holds, as format 5 shapes them (WP-207
    /// §C: U-1's "`StateView` for format 4"), so that a package can test
    /// against the states of real full links before U-2 lands: `FILES`
    /// with `rank = FileId << 20` and round 0; `MENTIONS` and the other
    /// file-keyed rows by the file of their section; `KEY_REFS` as
    /// `MENTIONS` transposed; `RESOLVED` one row per `SYMBOLS` row (a key
    /// format 4 did not resolve: the default row); `GROUPS` from
    /// `RESOLVED`'s kind-6 rows; `FRAGS` with `refs` 1; `META`'s build id
    /// inline. **Not derivable from format 4**, left empty: `EDGES`,
    /// `SYMTAB`, `SYMTAB_KEYS`, `COMMONS`, `ODR_*`, `DIAGS`, `LAZY` members
    /// (format 4 stores none), and the boot id. `FILE_INDEX` gives each
    /// file's position and count in these vectors.
    #[must_use]
    pub fn from_format4(s: &crate::format::State) -> Self {
        let mut m = Self {
            plan_hash: s.plan_hash,
            probes: s.probes.clone(),
            outsecs: s.outsecs.clone(),
            chunks: s.chunks.clone(),
            order: s.order.clone(),
            segments: s.segments.clone(),
            slots: s.slots.clone(),
            sizes: s.sizes.clone(),
            sym_place: s.sym_place.clone(),
            defined: s.defined.clone(),
            patches: s.patches.clone(),
            redirects: s.redirects.clone(),
            sized: s.sized.clone(),
            layout: s.layout.clone(),
            leaves: s.leaves.clone(),
            stat: s.stat.clone(),
            strings: s.strings.clone(),
            sections: s.sections.clone(),
            symbols: s.symbols.clone(),
            ..Self::default()
        };
        let id = s.str(s.meta.build_id);
        let mut build_id = [0u8; 64];
        let n = id.len().min(64);
        build_id[..n].copy_from_slice(&id[..n]);
        m.meta = MetaRow {
            version: s.meta.version,
            machine: s.meta.machine,
            class: s.meta.class,
            output_kind: s.meta.output_kind,
            leaf_fn: s.meta.leaf_fn,
            file_size: s.meta.file_size,
            sealed_start: s.meta.sealed_start,
            sealed_end: s.meta.sealed_end,
            build_id_len: u8::try_from(n).unwrap_or(0),
            build_id,
            ..MetaRow::default()
        };
        m.files = s
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| FileRow {
                kind: f.kind,
                member: f.member,
                flags: f.flags,
                path: f.path,
                mention: f.mention,
                member_name: f.member_name,
                member_ordinal: f.member_ordinal,
                base: f.base,
                len: f.len,
                hash: f.hash,
                rank: (i as u64) << 20,
                round: 0,
            })
            .collect();
        m.keyed_from_format4(s);
        m.name_rows();
        m.file_index = (0..m.files.len()).map(|f| m.index_row(FileId(f as u32))).collect();
        m
    }

    /// [`Self::from_format4`]'s file-keyed, key-keyed and key-parallel
    /// rows.
    fn keyed_from_format4(&mut self, s: &crate::format::State) {
        use crate::format::{NONE, def_kind};
        let m = self;
        let file_of =
            |section: u32| FileId(s.sections.get(section as usize).map_or(NONE, |r| r.file));
        for r in &s.mentions {
            let row = MentionRow {
                symbol: r.symbol,
                kind: r.kind,
                bind: r.bind,
                typ: r.typ,
                other: r.other,
                section: r.section,
            };
            m.mentions.push((FileId(r.file), row));
            let mut flags = r.other & super::rows::key_ref_flags::VISIBILITY;
            if r.section != NONE {
                flags |= super::rows::key_ref_flags::IN_SECTION;
            }
            m.key_refs.push(KeyRefRow {
                key: r.symbol,
                file: r.file,
                def: r.kind,
                bind: r.bind,
                typ: r.typ,
                flags,
                tomb: false,
            });
        }
        m.key_refs.sort_by_key(|r| (r.key, r.file));
        m.resolved = vec![ResolvedRow::default(); s.symbols.len()];
        for r in &s.resolved {
            if r.kind == def_kind::COMDAT {
                m.groups.push(GroupRow { signature: r.name, keeper: r.file, tomb: false });
            } else if let Some(slot) = m.resolved.get_mut(r.symbol as usize) {
                *slot = ResolvedRow {
                    kind: r.kind,
                    typ: r.typ,
                    bind: r.bind,
                    other: r.other,
                    section: r.section,
                    value: r.value,
                    size: r.size,
                };
            }
        }
        m.index = s.index.iter().map(|r| (file_of(r.unit), *r)).collect();
        m.frag_at = s.frag_at.iter().map(|r| (file_of(r.section), *r)).collect();
        m.eh_records = s.eh_records.iter().map(|r| (file_of(r.section), r.clone())).collect();
        m.frags = s
            .frags
            .iter()
            .map(|f| FragRow {
                out: f.out,
                ordinal: f.ordinal,
                hash: f.hash,
                offset: f.offset,
                refs: 1,
                tomb: false,
            })
            .collect();
        m.holes = s
            .holes
            .iter()
            .map(|h| HoleRow {
                chunk: h.chunk,
                kind: h.kind,
                offset: h.offset,
                size: h.size,
                tomb: false,
            })
            .collect();
    }

    /// `FILE_INDEX`'s row of `file` over these vectors (first position and
    /// count of the file's rows; capacity = count).
    fn index_row(&self, file: FileId) -> FileIndexRow {
        use super::rows::{FileTable, RowRange};
        fn range(at: impl Iterator<Item = bool>) -> RowRange {
            let mut r = RowRange::default();
            for (i, mine) in at.enumerate() {
                if mine {
                    if r.count == 0 {
                        r.first = u32::try_from(i).unwrap_or(u32::MAX);
                    }
                    r.count += 1;
                }
            }
            r.capacity = r.count;
            r
        }
        let mut row = FileIndexRow::default();
        row.ranges[FileTable::Sections as usize] =
            range(self.sections.iter().map(|s| s.file == file.0));
        row.ranges[FileTable::Mentions as usize] = range(self.mentions.iter().map(|r| r.0 == file));
        row.ranges[FileTable::Index as usize] = range(self.index.iter().map(|r| r.0 == file));
        row.ranges[FileTable::Edges as usize] = range(self.edges.iter().map(|r| r.0 == file));
        row.ranges[FileTable::FragAt as usize] = range(self.frag_at.iter().map(|r| r.0 == file));
        row.ranges[FileTable::EhRecords as usize] =
            range(self.eh_records.iter().map(|r| r.0 == file));
        row.ranges[FileTable::Symtab as usize] = range(self.symtab.iter().map(|r| r.0 == file));
        row.ranges[FileTable::Regions as usize] = range(self.regions.iter().map(|r| r.0 == file));
        row
    }

    /// `NAMES` made afresh: one row per global (kinds 0 and 1) and `GONE`
    /// `SYMBOLS` row, by [`super::rows::name_hash`].
    pub fn name_rows(&mut self) {
        use super::rows::{name_hash, sym_kind};
        let mut names = Vec::new();
        for (i, s) in self.symbols.iter().enumerate() {
            if matches!(s.kind, sym_kind::GLOBAL | sym_kind::VERSIONED | sym_kind::GONE) {
                let (Ok(name), Ok(version)) = ((&*self).string(s.name), (&*self).string(s.version))
                else {
                    continue;
                };
                let row = u32::try_from(i).unwrap_or(u32::MAX);
                names.push(NameRow { hash: name_hash(&name, &version), row });
            }
        }
        names.sort_unstable();
        self.names = names;
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
            kind::NAMES => self.names.len(),
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
    fn regions_of(&self, file: FileId) -> Rows<Vec<RegionRow>> {
        let mut v = of(&self.regions, file);
        v.sort_by_key(|r| (r.out, r.category, r.ordinal));
        Ok(v)
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
        let hash = super::rows::name_hash(name, version);
        for n in self.names.iter().filter(|n| n.hash == hash) {
            let s = row(&self.symbols, n.row, "NAMES")?;
            if self.string(s.name)? == name && self.string(s.version)? == version {
                return Ok(Some(SymbolId(n.row)));
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
        m.name_rows();
        let v = m.view();
        assert_eq!(v.find_key(b"main", b"").unwrap(), Some(SymbolId(0)));
        assert_eq!(v.find_key(b"other", b"").unwrap(), None);
        let refs = v.key_refs(SymbolId(0)).unwrap();
        assert_eq!(refs.iter().map(|r| r.file).collect::<Vec<_>>(), [1, 2]);
        assert!(matches!(v.symbol(SymbolId(1)), Err(IncrReason::StateCorrupt(_))));
        assert_eq!(v.count(kind::SYMBOLS), 1);
    }

    // spec: WP-207 §C, U-1's "StateView for format 4"
    #[test]
    fn format4_rows_as_format5() {
        use crate::format;
        let mut s = format::State::default();
        s.files.push(format::FileRow::default());
        s.files.push(format::FileRow::default());
        s.sections.push(SectionRow { file: 1, ..SectionRow::default() });
        s.symbols.push(format::SymbolRow::default());
        s.mentions.push(format::MentionRow {
            file: 1,
            symbol: 0,
            section: 0,
            other: 2,
            ..format::MentionRow::default()
        });
        s.resolved.push(format::ResolvedRow {
            symbol: 0,
            section: 0,
            value: 8,
            ..format::ResolvedRow::default()
        });
        s.index.push(format::IndexRow { unit: 0, kind: 0, target: 0 });
        let m = MemoryState::from_format4(&s);
        let v = m.view();
        assert_eq!(v.file(FileId(1)).unwrap().rank, 1 << 20);
        assert_eq!(v.mentions_of(FileId(1)).unwrap().len(), 1);
        assert_eq!(v.index_of(FileId(1)).unwrap().len(), 1);
        assert_eq!(v.resolved(SymbolId(0)).unwrap().value, 8);
        let refs = v.key_refs(SymbolId(0)).unwrap();
        assert_eq!(
            (refs[0].file, refs[0].flags),
            (1, 2 | super::super::rows::key_ref_flags::IN_SECTION)
        );
        assert_eq!(v.file_index(FileId(1)).unwrap().ranges[0].count, 1);
    }

    // spec: incr.md IN-18e (`NAMES`: a hash never identifies a key alone;
    // a `GONE` key keeps its row)
    #[test]
    fn find_key_compares_bytes_and_finds_gone_keys() {
        use super::super::rows::{name_hash, sym_kind};
        let mut m = MemoryState::default();
        let first = m.intern(b"a");
        let gone = m.intern(b"b");
        m.symbols.push(SymbolRow { name: first, ..SymbolRow::default() });
        m.symbols.push(SymbolRow { name: gone, kind: sym_kind::GONE, ..SymbolRow::default() });
        let hash = name_hash(b"b", b"");
        // Row 0 filed under b's hash (a collision, made by hand): skipped.
        m.names = vec![NameRow { hash, row: 0 }, NameRow { hash, row: 1 }];
        let view = m.view();
        assert_eq!(view.find_key(b"b", b"").unwrap(), Some(SymbolId(1)));
        assert_eq!(view.find_key(b"a", b"").unwrap(), None);
        drop(view);
        m.name_rows();
        assert_eq!(m.view().find_key(b"a", b"").unwrap(), Some(SymbolId(0)));
    }
}
