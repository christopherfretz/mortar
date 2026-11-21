//! WP-255e2 (rulings "After 255e's hand-back" item 3): [`Rows`] of an
//! UPDATE's output in state ids: [`StoredRows`] over the delta's
//! placement (`LayoutDelta::placement`: chunks, segments, `DEFINED`,
//! `SYM_PLACE`, patches, redirects, TLS), every section's place by
//! `LayoutDelta::place_of` (the one reader), the changed files' own
//! sections from their new `SECTIONS` rows (`ChangedFile::sections`), and
//! the affected keys' definitions from the resolve delta's `RESOLVED` rows.
//! One implementation of every formula (`rows::value`); a datum it cannot
//! give is `None` (or the trait's "absent") with the first reason recorded,
//! as [`StoredRows`] does, never a default.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, PoisonError};

use mortar_arch::{DemandFlags, SlotKind, SymFacts, SymValue, TlsBlock};
use mortar_core::incr::{IncrReason, LayoutDelta, PlaceNow, StoredLayout, Unimplemented};
use mortar_core::layout::{AllocKey, Chunk};
use mortar_core::resolution::SectionKind;
use mortar_core::{
    ChunkId, Column, FileId, FragId, SectionId, SymEntry, SymKey, SymKind, SymbolId,
};
use mortar_diag::Diagnostic;
use mortar_elf::SymbolIndex;
use mortar_elf::consts::STB_WEAK;
use mortar_image::clip_plan::{Member, Members, is_clip_member};
use mortar_image::rows::{EntryDef, OutSection, RowDef, Rows, Valued};
use mortar_incr::format::{SectionRow, def_kind, kind as table, section_facts};
use mortar_incr::update::rows::ResolvedRow;
use mortar_merge::What;
use mortar_merge::refs::PieceRef;
use mortar_synth::got::{GotAddrs, GotPlan};
use mortar_synth::{EhValue, EhWho, SynthError};

use super::stored_rows::StoredRows;

/// What [`UpdateRows`] reads of the update beyond the state, in state ids
/// (made by `shadow_set` from `placement`'s own values).
#[derive(Debug, Clone)]
pub(crate) struct UpdateData {
    /// The state-id layout delta `d`.
    pub(crate) d: LayoutDelta,
    /// The affected keys' new `RESOLVED` rows, sorted by key.
    pub(crate) resolved: Vec<(SymbolId, ResolvedRow)>,
    /// Per changed file, its keys' ids (position for position).
    pub(crate) key_ids: Vec<(FileId, Vec<SymbolId>)>,
    /// Per changed file: its first new section id and its new rows.
    pub(crate) files: Vec<(FileId, SectionId, Vec<SectionRow>)>,
    /// The new keys' names, in the order their rows are appended.
    pub(crate) new_keys: Vec<Vec<u8>>,
}

/// [`Rows`] of the update's output, in state ids.
pub(crate) struct UpdateRows<'a, 's> {
    base: StoredRows<'a, 's>,
    stored: &'a StoredLayout<'a>,
    data: &'a UpdateData,
    sections: u64,
    symbols: u64,
    outsecs: u64,
    failed: Mutex<Option<IncrReason>>,
}

fn front_only() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Image))
}

fn corrupt(what: &str) -> IncrReason {
    IncrReason::StateCorrupt(format!("WP-255e2 update rows: {what}"))
}

impl<'a, 's> UpdateRows<'a, 's> {
    /// The rows of `base` (made over `data.d.placement`) with `data`.
    pub(crate) fn new(
        base: StoredRows<'a, 's>,
        view: &mortar_incr::update::view::StateView<'_>,
        stored: &'a StoredLayout<'a>,
        data: &'a UpdateData,
    ) -> Self {
        Self {
            base,
            stored,
            data,
            sections: view.count(table::SECTIONS),
            symbols: view.count(table::SYMBOLS),
            outsecs: view.count(table::OUTSECS),
            failed: Mutex::new(None),
        }
    }

    /// `rows::sym` of `key`, or the first reason a read could not answer.
    pub(crate) fn value_of(&self, key: SymKey) -> Result<SymValue, IncrReason> {
        *self.failed.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.base.clear_failure();
        let v = mortar_image::rows::sym(self, key);
        let mine = self.failed.lock().unwrap_or_else(PoisonError::into_inner).clone();
        mine.or_else(|| self.base.failure()).map_or(Ok(v), Err)
    }

    fn fail(&self, why: IncrReason) {
        let mut f = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        if f.is_none() {
            *f = Some(why);
        }
    }

    /// A changed file's new section row of id `s` (an id past the stored
    /// `SECTIONS`), with its file.
    fn changed(&self, s: SectionId) -> Option<(FileId, &SectionRow)> {
        if u64::from(s.0) < self.sections {
            return None;
        }
        self.data.files.iter().find_map(|(file, first, rows)| {
            let k = s.0.checked_sub(first.0)? as usize;
            rows.get(k).map(|r| (*file, r))
        })
    }

    /// The file of section `s`, and whether the update places its sections.
    fn file_of(&self, s: SectionId) -> Option<(FileId, bool)> {
        if let Some((f, _)) = self.changed(s) {
            return Some((f, true));
        }
        let f = FileId(self.base.section_row(s)?.file);
        Some((f, self.data.d.files.binary_search(&f).is_ok()))
    }

    fn resolved(&self, id: SymbolId) -> Option<&ResolvedRow> {
        let k = self.data.resolved.binary_search_by_key(&id, |r| r.0).ok()?;
        Some(&self.data.resolved[k].1)
    }

    fn key_ids(&self, file: FileId) -> Option<&[SymbolId]> {
        self.data.key_ids.iter().find(|k| k.0 == file).map(|k| k.1.as_slice())
    }
}

impl Rows for UpdateRows<'_, '_> {
    fn chunks(&self) -> &Column<ChunkId, Chunk> {
        self.base.chunks()
    }

    fn out_section(&self, shndx: u32) -> Option<OutSection> {
        self.base.out_section(shndx)
    }

    fn place(&self, s: SectionId) -> Option<(ChunkId, u64)> {
        let p = match self.data.d.place_of(self.stored, s) {
            Ok(PlaceNow::Here(p) | PlaceNow::Stored(p)) => p,
            Ok(PlaceNow::NotPlaced) => return None,
            Err(e) => {
                self.fail(e);
                return None;
            }
        };
        let (c, off) = p.get()?;
        if self.chunks().get(c).is_none() {
            self.fail(corrupt("a place in no chunk of the delta"));
            return None;
        }
        Some((c, off))
    }

    fn section_size(&self, s: SectionId) -> Option<u64> {
        match self.changed(s) {
            Some((_, r)) => Some(r.size),
            None => self.base.section_size(s),
        }
    }

    fn section_flags(&self, s: SectionId) -> u64 {
        match self.changed(s) {
            Some((_, r)) => r.sh_flags,
            None => self.base.section_flags(s),
        }
    }

    fn section_kind(&self, s: SectionId) -> Option<SectionKind> {
        match self.changed(s) {
            Some((_, r)) => match section_facts(r, self.outsecs) {
                Ok(f) => Some(f.kind),
                Err(why) => {
                    self.fail(corrupt(&format!("a changed file's SECTIONS row: {why}")));
                    None
                }
            },
            None => self.base.section_kind(s),
        }
    }

    fn section_place(&self, s: SectionId) -> bool {
        // P-1: a section of a file the update places has a place exactly
        // when the delta placed it (`place_of`'s `Here`); any other, its
        // stored facts.
        match self.file_of(s) {
            Some((_, true)) => self.place(s).is_some(),
            Some((_, false)) => self.base.section_place(s),
            None => false,
        }
    }

    fn out_flags(&self, s: SectionId) -> Option<u64> {
        let (c, _) = self.place(s)?;
        Some(self.chunks().get(c)?.flags)
    }

    fn kept_copy(&self, d: SectionId) -> Option<SectionId> {
        self.base.kept_copy(d)
    }

    fn named_piece(&self, s: SectionId, v: u64) -> Option<PieceRef> {
        self.base.named_piece(s, v)
    }

    fn split_row(&self, s: SectionId) -> Option<u32> {
        self.base.split_row(s)
    }

    fn section_piece(&self, s: SectionId, row: u32, r: i64) -> Option<Result<PieceRef, What>> {
        self.base.section_piece(s, row, r)
    }

    fn frag_live(&self, f: FragId) -> bool {
        self.base.frag_live(f)
    }

    fn frag_place(&self, f: FragId) -> Option<(ChunkId, u64)> {
        self.base.frag_place(f)
    }

    fn frag_offset(&self, f: FragId) -> Option<u64> {
        self.base.frag_offset(f)
    }

    fn merged_chunk(&self, s: SectionId) -> Option<ChunkId> {
        self.base.merged_chunk(s)
    }

    fn outside_merge(&self, s: SectionId, r: i64, w: &What) -> Diagnostic {
        self.base.outside_merge(s, r, w)
    }

    fn eh_value(&self, s: SectionId, r: i64, who: EhWho) -> Option<Result<EhValue, SynthError>> {
        self.base.eh_value(s, r, who)
    }

    fn eh_frame(&self) -> Option<(u32, u64)> {
        self.base.eh_frame()
    }

    fn def(&self, id: SymbolId) -> Option<RowDef> {
        let Some(r) = self.resolved(id) else {
            if u64::from(id.0) >= self.symbols {
                self.fail(corrupt("a new key without a RESOLVED row of the delta"));
                return None;
            }
            return self.base.def(id);
        };
        Some(match r.kind {
            def_kind::UNDEFINED => RowDef::Undefined { weak: r.bind == STB_WEAK },
            def_kind::SECTION => {
                let (file, update) = self.file_of(SectionId(r.section))?;
                if update {
                    // A changed file's entry: its position among the
                    // file's keys (UpdateRows' own numbering, read back
                    // only by `entry`).
                    let ids = self.key_ids(file)?;
                    let mut at = ids.iter().enumerate().filter(|(_, k)| **k == id);
                    let (Some((k, _)), None) = (at.next(), at.next()) else {
                        self.fail(front_only());
                        return None;
                    };
                    RowDef::Defined(SymEntry { file, index: SymbolIndex(u32::try_from(k).ok()?) })
                } else {
                    RowDef::Defined(self.base.definer_entry(file, id, r.kind, r.section)?)
                }
            }
            def_kind::INTERNAL => RowDef::Internal,
            _ => {
                // An affected absolute, common or shared key: its
                // definer's entry is not built here.
                self.fail(front_only());
                return None;
            }
        })
    }

    fn entry(&self, e: SymEntry) -> Option<EntryDef> {
        let id = if let Some(ids) = self.key_ids(e.file) {
            *ids.get(e.index.0 as usize)?
        } else {
            // An unchanged file's entry: the stored one, unless its key
            // is affected (the stored row is then the old winner's).
            let Some(id) = self.unchanged_key(e) else { return self.base.entry(e) };
            id
        };
        let Some(r) = self.resolved(id) else {
            self.fail(front_only());
            return None;
        };
        if r.kind != def_kind::SECTION {
            self.fail(front_only());
            return None;
        }
        Some(EntryDef {
            value: r.value,
            size: r.size,
            sym_type: r.typ,
            place: Some(Ok(SectionId(r.section))),
        })
    }

    fn global(&self, id: SymbolId) -> Valued {
        mortar_image::rows::def_value(self, id)
    }

    fn alloc_place(&self, key: AllocKey) -> Option<(ChunkId, u64)> {
        self.base.alloc_place(key)
    }

    fn copy_of(&self, id: SymbolId) -> Option<u32> {
        self.base.copy_of(id)
    }

    fn defined(&self, id: SymbolId) -> Option<u64> {
        self.base.defined(id)
    }

    fn facts(&self, key: SymKey) -> SymFacts {
        if let SymKind::Global(id) = key.kind()
            && let Some(r) = self.resolved(id)
        {
            return SymFacts::unpack(r.facts).unwrap_or_else(|| {
                self.fail(corrupt("the delta's RESOLVED facts"));
                self.base.facts(key)
            });
        }
        self.base.facts(key)
    }

    fn facts_at(&self, key: SymKey, mention: Option<SymEntry>) -> SymFacts {
        if mention.is_some() {
            self.fail(front_only());
        }
        self.facts(key)
    }

    fn name(&self, key: SymKey) -> &[u8] {
        if let SymKind::Global(id) = key.kind()
            && let Some(k) = u64::from(id.0).checked_sub(self.symbols)
        {
            return self.data.new_keys.get(k as usize).map_or_else(
                || {
                    self.fail(corrupt("a new key out of range"));
                    &b""[..]
                },
                Vec::as_slice,
            );
        }
        self.base.name(key)
    }

    fn demand(&self, key: SymKey) -> DemandFlags {
        self.base.demand(key)
    }

    fn got(&self) -> &GotPlan {
        self.base.got()
    }

    fn got_addrs(&self) -> GotAddrs {
        self.base.got_addrs()
    }

    fn slot_index(&self, key: SymKey, kind: SlotKind) -> Option<u32> {
        self.base.slot_index(key, kind)
    }

    fn redirect(&self, s: SectionId, off: u64) -> Option<(ChunkId, u64)> {
        self.base.redirect(s, off)
    }

    fn tls(&self) -> Option<TlsBlock> {
        self.base.tls()
    }
}

impl UpdateRows<'_, '_> {
    /// The affected key an unchanged file's entry `e` defines, if any (an
    /// affected key whose `RESOLVED` row of the delta names a section of
    /// `e`'s file and `e` is that file's definer entry).
    fn unchanged_key(&self, e: SymEntry) -> Option<SymbolId> {
        self.data.resolved.iter().find_map(|(id, r)| {
            if r.kind != def_kind::SECTION {
                return None;
            }
            let (file, update) = self.file_of(SectionId(r.section))?;
            if update || file != e.file {
                return None;
            }
            (self.base.definer_entry(file, *id, r.kind, r.section)? == e).then_some(*id)
        })
    }
}

impl UpdateRows<'_, '_> {
    /// WP-255e4 item 1 (c2b-1, rulings "After 255e2" item 3): the member
    /// index of the update's output read from the STATE, for the chunks of
    /// `want` (sorted): every section the update's placement puts in one of
    /// them that THE member rule ([`mortar_image::clip_plan::is_clip_member`])
    /// takes, by offset, with its `SECTIONS.align`. A file the delta places
    /// (`LayoutDelta::files`): its sections' P-1 is "the delta placed it"
    /// (as [`Rows::section_place`] here), its changed rows
    /// (`ChangedFile::sections`) in place of the stored ones; any other
    /// file: its stored `SECTIONS.bits` (KEPT, not COLLECTED) with the
    /// delta's `killed` and `revived`. `folded` is `false` by name: IN-12
    /// refuses `--icf` in incremental mode, so a state holds no fold.
    /// O(sections): every file's `SECTIONS` range is read (the shadow's
    /// cost; the replacement needs a per-chunk read, the record says).
    /// A merged chunk of `want` is refused: `FRAGS` holds no fragment's
    /// section, size or alignment.
    pub(crate) fn members(
        &self,
        view: &mortar_incr::update::view::StateView<'_>,
        want: &[ChunkId],
    ) -> Result<Members, String> {
        use mortar_core::layout::ChunkKind;
        for &c in want {
            if let Some(ch) = self.chunks().get(c)
                && matches!(ch.kind, ChunkKind::Merged(_))
            {
                return Err(format!(
                    "merged chunk {}: FRAGS gives no fragment's merge section, size or \
                     alignment (the member index of a merged chunk)",
                    c.0
                ));
            }
        }
        *self.failed.lock().unwrap_or_else(PoisonError::into_inner) = None;
        let d = &self.data.d;
        let mut by: BTreeMap<ChunkId, Vec<(u64, Member)>> = BTreeMap::new();
        let mut consider = |s: SectionId, row: &SectionRow, update: bool| -> Result<(), String> {
            let revived = d.revived.binary_search(&s).is_ok();
            let Some((c, off)) = self.place(s) else {
                if revived && !update {
                    return Err(format!(
                        "section {} (an unchanged file's) is revived and place_of gives it no \
                         place",
                        s.0
                    ));
                }
                return Ok(());
            };
            if want.binary_search(&c).is_err() {
                return Ok(());
            }
            let facts = section_facts(row, self.outsecs)
                .map_err(|w| format!("SECTIONS row {}: {w}", s.0))?;
            let (kept, live) = if update {
                (true, true)
            } else {
                let killed = d.killed.binary_search(&s).is_ok();
                (facts.kept || revived, (!facts.collected || revived) && !killed)
            };
            let Some(chunk) = self.chunks().get(c) else {
                return Err(format!("section {}: a place in no chunk {}", s.0, c.0));
            };
            if is_clip_member(
                kept,
                facts.kind.is_placed(),
                live,
                false,
                row.sh_type,
                row.size,
                chunk,
            ) {
                by.entry(c)
                    .or_default()
                    .push((off, Member { section: s, align: row.align.max(1) }));
            }
            Ok(())
        };
        let files = u32::try_from(view.count(table::FILES)).unwrap_or(u32::MAX);
        for file in (0..files).map(FileId) {
            if let Some((_, first, rows)) = self.data.files.iter().find(|x| x.0 == file) {
                for (k, row) in rows.iter().enumerate() {
                    consider(SectionId(first.0 + u32::try_from(k).unwrap_or(u32::MAX)), row, true)?;
                }
                continue;
            }
            let ids = self.stored.sections_of(file).map_err(|e| e.report())?;
            let rows = view.sections_of(file).map_err(|e| e.report())?;
            if ids.len() != rows.len() {
                return Err(format!(
                    "file {}: {} stored sections, {} SECTIONS rows",
                    file.0,
                    ids.len(),
                    rows.len()
                ));
            }
            let update = d.files.binary_search(&file).is_ok();
            for (sec, row) in ids.iter().zip(&rows) {
                consider(sec.id, row, update)?;
            }
        }
        if let Some(e) = self.failed.lock().unwrap_or_else(PoisonError::into_inner).take() {
            return Err(e.report());
        }
        let by_chunk = by
            .into_iter()
            .map(|(c, mut v)| {
                v.sort_by_key(|m| m.0);
                (c, v.into_iter().map(|m| m.1).collect())
            })
            .collect();
        Ok(Members { by_chunk, merged: BTreeMap::new() })
    }

    /// Section `s`'s `(file, index in its file)` (D3-1): a changed file's
    /// new row by its position, any other by its stored row.
    pub(crate) fn file_index(&self, s: SectionId) -> Option<(FileId, u32)> {
        if u64::from(s.0) >= self.sections {
            return self.data.files.iter().find_map(|(file, first, rows)| {
                let k = s.0.checked_sub(first.0)?;
                ((k as usize) < rows.len()).then_some((*file, k))
            });
        }
        let row = self.stored.section(s).ok()??;
        Some((row.file, row.index))
    }

    /// The section of `file` at `index` in the update's ids (D3-1): a
    /// changed file's new row, any other file's stored row.
    pub(crate) fn state_id(&self, file: FileId, index: u32) -> Option<SectionId> {
        if let Some((_, first, rows)) = self.data.files.iter().find(|x| x.0 == file) {
            return ((index as usize) < rows.len()).then_some(SectionId(first.0 + index));
        }
        let ids = self.stored.sections_of(file).ok()?;
        ids.iter().find(|x| x.index == index).map(|x| x.id)
    }

    /// The update's state-id delta.
    pub(crate) fn delta(&self) -> &LayoutDelta {
        &self.data.d
    }
}

/// Step 2's comparison: `rows::sym` over `rows` against `front` (front 2's
/// `FrontRows`) for every global of front 2, keys matched by name (a
/// `SYMBOLS` row is not a `SymbolId`): `rows: N equal, M refused (by
/// reason), K new keys not compared, 0 differ` or the first difference.
pub(crate) fn compare_values<F: Rows>(
    rows: &UpdateRows<'_, '_>,
    view: &mortar_incr::update::view::StateView<'_>,
    front: &F,
    globals: u32,
) -> String {
    let mut by_name: HashMap<Vec<u8>, Option<u32>> = HashMap::new();
    for r in 0..u32::try_from(view.count(table::SYMBOLS)).unwrap_or(0) {
        let Ok(row) = view.symbol(SymbolId(r)) else { continue };
        if row.kind != mortar_incr::format::sym_kind::GLOBAL {
            continue;
        }
        let Ok(n) = view.string(row.name) else { continue };
        by_name.entry(n).and_modify(|v| *v = None).or_insert(Some(r));
    }
    let first_new = u32::try_from(rows.symbols).unwrap_or(u32::MAX);
    for (k, n) in rows.data.new_keys.iter().enumerate() {
        let id = first_new.saturating_add(u32::try_from(k).unwrap_or(u32::MAX));
        by_name.entry(n.clone()).and_modify(|v| *v = None).or_insert(Some(id));
    }
    let (mut equal, mut unmatched) = (0u64, 0u64);
    let mut refused: BTreeMap<String, (u64, String)> = BTreeMap::new();
    let mut affected = 0u64;
    let mut differ: Vec<String> = Vec::new();
    for i in 0..globals {
        let key = SymKey::global(SymbolId(i));
        let name = front.name(key).to_vec();
        let Some(&Some(sid)) = by_name.get(&name) else {
            unmatched += 1;
            continue;
        };
        let want = mortar_image::rows::sym(front, key);
        match rows.value_of(SymKey::global(SymbolId(sid))) {
            Ok(got) if got == want => {
                equal += 1;
                if rows.resolved(SymbolId(sid)).is_some() {
                    affected += 1;
                }
            }
            Ok(got) => {
                if differ.is_empty() {
                    differ.push(format!(
                        "{}: front 2 {want:x?}, update rows {got:x?}",
                        String::from_utf8_lossy(&name)
                    ));
                }
                differ.push(String::new());
            }
            Err(e) => {
                // WP-255e3: the class of a refused key: the read that
                // refused and the stored definition's kind.
                let site = rows.base.refused_at().unwrap_or("-");
                let kind = view
                    .resolved(SymbolId(sid))
                    .map_or_else(|_| "new".to_owned(), |r| format!("{}", r.kind));
                let at = format!("{} at {site}, RESOLVED.kind {kind}", e.report());
                let r = refused.entry(at).or_insert((0, String::new()));
                r.0 += 1;
                if r.1.len() < 160 {
                    if !r.1.is_empty() {
                        r.1.push(' ');
                    }
                    r.1.push_str(&String::from_utf8_lossy(&name));
                }
            }
        }
    }
    let total: u64 = refused.values().map(|r| r.0).sum();
    let refused: Vec<String> =
        refused.iter().map(|(k, (n, e))| format!("{n} {k}, e.g. {e}")).collect();
    if differ.is_empty() {
        format!(
            "update rows: {equal} equal ({affected} affected keys), {total} refused [{}], \
             {unmatched} not matched by name, 0 differ",
            refused.join("; ")
        )
    } else {
        format!(
            "finding: update rows: {} differ (first {}); {equal} equal, refused [{}]",
            differ.len(),
            differ[0],
            refused.join("; ")
        )
    }
}
