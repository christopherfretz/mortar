//! ODR checks in an update (`spec/incr.md` IN-71d; odr.md §8.1): W1 and
//! W2 recomputed for the ODR-affected keys from `COPIES`, `RESOLVED` and
//! the changed files' copies, by the full link's comparator
//! (`mortar_odr::foreign::compare_copies`); every other check of odr.md
//! falls back from the switches alone
//! (`mortar_odr::register::update_fallback`). WP-213 (U-1): the entry
//! point [`odr`]; WP-220: filled by U-8.
//!
//! What the comparator reads of a key is built from rows alone ([`Now`],
//! [`KeyInput`]): each copy's `COPIES` row (size, type, index), its group
//! (the copy's `MENTIONS` row by position, that row's `SECTIONS.anchor`),
//! its file's current rank, the key's name (`SYMBOLS`), each group's
//! keeper (`GROUPS`, by current rank), the resolved definition
//! (`RESOLVED`, its section's or `KEY_REFS`' file, that file's `MENTIONS`
//! for the index) and each file's display (`FILES`). [`Now`] composes the
//! stored rows with the update's (`ChangedFiles`, `ResolveDelta`): a
//! changed file's rows replace its stored ones, its strings are read with
//! `ChangedFiles::string` and compared by bytes.

use std::collections::{BTreeMap, BTreeSet};

use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_core::inputs::Reader;
use mortar_core::plan::{Odr, OdrMode};
use mortar_core::{FileId, Inputs, Resolution, SectionId, SymbolId};
use mortar_diag::FileRef;
use mortar_elf::SymbolIndex;
use mortar_elf::consts::STB_WEAK;
use mortar_odr::foreign::{
    At, CopyDef, Definition, Finding, Names, Resolved, compare_copies, diagnostic,
};
use mortar_odr::register::{Check, update_fallback};

use super::diags::{DiagRow, Subject};
use super::resolve::ResolveDelta;
use super::rows::{
    self, CommonRow, CopyRow, FileRow, GroupRow, KeyRefRow, MentionRow, ResolvedRow,
};
use super::view::{Change, ChangedFile, ChangedFiles, StateView};
use crate::format::{NONE, def_kind, kind};

/// What an update's ODR checks give U-9 (IN-71d, IN-75a; WP-220
/// rulings-2 §1). The contract: every stored ODR `DIAGS` row of a key in
/// `keys` is replaced by the rows of `rows` for it (possibly none); every
/// other stored ODR row stays. U-9 derives that set from nothing else
/// (`ResolveDelta::affected` is U-3's and means something else).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OdrUpdate {
    /// The keys recomputed, sorted and unique.
    pub keys: Vec<SymbolId>,
    /// Their W1/W2 findings, in key order then the comparator's.
    pub rows: Vec<DiagRow>,
}

/// The ODR findings of the ODR-affected keys (WP-211 §C; IN-71d): W1 and
/// W2, exact, with the set of keys recomputed ([`OdrUpdate`]). Called by
/// the join after `resolve`.
///
/// In order, before any row is read: `--odr=off` recomputes nothing; a
/// switch that turns on anything of odr.md but W1 and W2
/// ([`update_fallback`]) gives `update-not-implemented (meta)`; so does
/// `--keep-mason-meta` with a changed file that carries units, or a gone
/// or no longer extracted file (whose units the update does not know;
/// the conservative fall-back stays in v1, rulings-2 §3). Then each
/// ODR-affected key ([`Now::affected`]) is compared afresh; the rows'
/// `key.unit` is the `FILES` row (U-9 maps it), their subjects the key
/// and the files the text names.
///
/// # Errors
/// `state-corrupt`; `update-not-implemented (meta)` as above.
pub fn odr(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    resolved: &ResolveDelta,
    odr: &Odr,
) -> Result<OdrUpdate, IncrReason> {
    if odr.mode == OdrMode::Off {
        return Ok(OdrUpdate::default());
    }
    if update_fallback(odr).is_some() {
        return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Meta)));
    }
    if odr.keep_meta
        && changed.files.iter().any(|f| {
            f.meta_units > 0 || matches!(f.change, Change::Gone | Change::NoLongerExtracted)
        })
    {
        return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Meta)));
    }
    let (w1, w2) = (Check::W1.runs(odr), Check::W2.runs(odr));
    if !w1 && !w2 {
        return Ok(OdrUpdate::default());
    }
    let now = Now::new(view, changed, resolved);
    let rank = |f: u32| now.rank(f);
    let mut out = OdrUpdate::default();
    for key in now.affected()? {
        let input = now.key_input(key, &rank)?;
        for (_, finding) in input.compare(w1, w2) {
            out.rows.push(now.diag_row(finding)?);
        }
        out.keys.push(key);
    }
    Ok(out)
}

fn corrupt(what: &str) -> IncrReason {
    IncrReason::StateCorrupt(format!("{what} (WP-220: ODR rows)"))
}

/// The `COPIES` row of a full link's [`Definition`] (`spec/incr.md`
/// IN-18e, WP-220): the capture's one producer, for a full link's state and
/// a changed file's new rows alike. `key` is the key's `SYMBOLS` row (or,
/// for a changed file, its position in `ChangedFile::keys`).
#[must_use]
pub fn copy_row(d: &Definition, key: u32) -> CopyRow {
    CopyRow { key, file: d.file.0, index: d.index.0, size: d.size, typ: d.sym_type, tomb: false }
}

/// A live object's `MENTIONS` rows as format 5 stores them (WP-215 D7: one
/// per non-local entry, in symbol-table order, none skipped), each key as
/// its `SymbolId` and each section as its `SectionId` (not yet numbered
/// as state rows), with the file's `first_global` (`sh_info`). The
/// producer format 4's capture calls before it sorts (`tables::
/// file_mentions`). `None`: not an object, or no symbol table.
#[must_use]
pub fn mentions_by_position(
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    file: FileId,
) -> Option<(u32, Vec<MentionRow>)> {
    let Reader::Object(obj) = inputs.files.get(file)?.reader else { return None };
    let table = obj.symbols().ok()??;
    let rows = crate::tables::file_mentions(inputs, resolution, file, &table)
        .into_iter()
        .map(|m| MentionRow {
            symbol: m.symbol,
            kind: m.kind,
            bind: m.bind,
            typ: m.typ,
            other: m.other,
            section: m.section,
        })
        .collect();
    Some((table.first_global().0, rows))
}

/// A stored copy as the comparator reads it: its `COPIES` row, its group
/// (a number within one [`KeyInput`]) and its file's current rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowCopy {
    pub row: CopyRow,
    pub group: u32,
    pub rank: u64,
}

impl CopyDef for RowCopy {
    type Group = u32;
    fn group(&self) -> u32 {
        self.group
    }
    fn at(&self) -> At {
        At { file: FileId(self.row.file), index: SymbolIndex(self.row.index) }
    }
    fn rank(&self) -> u64 {
        self.rank
    }
    fn sym_type(&self) -> u8 {
        self.row.typ
    }
    fn size(&self) -> u64 {
        self.row.size
    }
}

/// One group of a key's copies: its signature and its keeper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInput {
    pub signature: Vec<u8>,
    pub keeper: Option<FileId>,
}

/// Everything [`compare_copies`] reads of one key, from rows alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyInput {
    pub key: SymbolId,
    /// As a diagnostic writes it (`name@version`).
    pub name: String,
    /// Contiguous by group (groups by signature bytes), then by (current
    /// rank, index): the comparator's precondition.
    pub copies: Vec<RowCopy>,
    /// By `RowCopy::group`.
    pub groups: Vec<GroupInput>,
    pub outside: bool,
    pub resolved: Option<Resolved>,
}

impl Names<u32> for KeyInput {
    fn symbol(&self, _key: SymbolId) -> String {
        self.name.clone()
    }
    fn group(&self, group: u32) -> String {
        self.groups
            .get(group as usize)
            .map_or_else(String::new, |g| String::from_utf8_lossy(&g.signature).into_owned())
    }
    fn kept(&self, group: u32, file: FileId) -> bool {
        self.groups.get(group as usize).is_some_and(|g| g.keeper == Some(file))
    }
}

impl KeyInput {
    /// W1 and W2 of the key, by the full link's comparator.
    #[must_use]
    pub fn compare(&self, w1: bool, w2: bool) -> Vec<(usize, Finding)> {
        compare_copies(self.key, &self.copies, self.outside, || self.resolved, self, w1, w2)
    }
}

/// The state as the update makes it, for W1 and W2: the stored rows with
/// each changed file's rows in place of its stored ones (a `DrawnIn`
/// file keeps its stored rows: they are equal) and `ResolveDelta`'s
/// `RESOLVED`, `GROUPS`, `COMMONS` and `KEY_REFS` edits applied. Ids
/// are the update's (`mortar_core::incr`): a new key, section or file
/// past the state's count.
#[derive(Debug)]
pub struct Now<'a, 'v> {
    view: &'a StateView<'v>,
    changed: &'a ChangedFiles,
    delta: &'a ResolveDelta,
    files: BTreeMap<FileId, &'a ChangedFile>,
    key_ids: BTreeMap<FileId, &'a [SymbolId]>,
    /// The changed files' new copies by key (`CopyRow.key` the key's id,
    /// `file` the changed file's).
    new_copies: BTreeMap<SymbolId, Vec<CopyRow>>,
    symbols: u64,
    sections: u64,
    file_rows: u64,
}

/// Whether `e` names the same `(key, file)` row as `r`.
fn same_key_file(e: (u32, u32), r: (u32, u32)) -> bool {
    e == r
}

impl<'a, 'v> Now<'a, 'v> {
    /// The composition of `view` with the update's rows.
    #[must_use]
    pub fn new(
        view: &'a StateView<'v>,
        changed: &'a ChangedFiles,
        delta: &'a ResolveDelta,
    ) -> Self {
        let files: BTreeMap<FileId, &ChangedFile> =
            changed.files.iter().map(|f| (f.file, f)).collect();
        let key_ids: BTreeMap<FileId, &[SymbolId]> =
            delta.key_ids.iter().map(|(f, ids)| (*f, ids.as_slice())).collect();
        let mut new_copies: BTreeMap<SymbolId, Vec<CopyRow>> = BTreeMap::new();
        for cf in &changed.files {
            if !replaces(cf) || matches!(cf.change, Change::Gone | Change::NoLongerExtracted) {
                continue;
            }
            let ids = key_ids.get(&cf.file).copied().unwrap_or_default();
            for c in &cf.odr.copies {
                // A copy whose key has no id is reported by `copies`.
                let key = ids.get(c.key as usize).copied().unwrap_or(SymbolId(u32::MAX));
                let row = CopyRow { key: key.0, file: cf.file.0, ..*c };
                new_copies.entry(key).or_default().push(row);
            }
        }
        Self {
            view,
            changed,
            delta,
            files,
            key_ids,
            new_copies,
            symbols: view.count(kind::SYMBOLS),
            sections: view.count(kind::SECTIONS),
            file_rows: view.count(kind::FILES),
        }
    }

    /// The changed file whose rows replace `file`'s stored ones.
    fn replaced(&self, file: FileId) -> Option<&'a ChangedFile> {
        self.files.get(&file).copied().filter(|cf| replaces(cf))
    }

    fn is_old_key(&self, key: SymbolId) -> bool {
        u64::from(key.0) < self.symbols
    }

    /// A file's CURRENT rank: an appeared or newly extracted file's from
    /// its `ChangedFile::row` (IN-18e: the midpoint of its neighbours'),
    /// every other file's stored `FILES.rank`.
    ///
    /// # Errors
    /// `state-corrupt`.
    pub fn rank(&self, file: u32) -> Result<u64, IncrReason> {
        if let Some(cf) = self.files.get(&FileId(file))
            && matches!(cf.change, Change::Appeared | Change::Extracted)
        {
            return Ok(cf.row.rank);
        }
        Ok(self.view.file(FileId(file))?.rank)
    }

    /// A file's `MENTIONS` rows by position (keys as ids) and its
    /// `first_global`.
    fn mentions(&self, file: FileId) -> Result<(u32, Vec<MentionRow>), IncrReason> {
        let Some(cf) = self.replaced(file) else {
            return Ok((self.view.file_index(file)?.first_global, self.view.mentions_of(file)?));
        };
        if matches!(cf.change, Change::Gone | Change::NoLongerExtracted) {
            return Ok((0, Vec::new()));
        }
        let ids = self.key_ids.get(&file).copied().unwrap_or_default();
        let rows = cf
            .mentions
            .iter()
            .map(|m| {
                let id =
                    ids.get(m.symbol as usize).ok_or_else(|| corrupt("a key without its id"))?;
                Ok(MentionRow { symbol: id.0, ..*m })
            })
            .collect::<Result<Vec<_>, IncrReason>>()?;
        Ok((cf.first_global, rows))
    }

    /// A section's file, `anchor_kind` and (for kind 1) its signature's
    /// bytes.
    fn section(&self, id: u32) -> Result<(FileId, u8, Vec<u8>), IncrReason> {
        if u64::from(id) < self.sections {
            let s = self.view.section(SectionId(id))?;
            let sig = if s.anchor_kind == 1 { self.view.string(s.anchor)? } else { Vec::new() };
            return Ok((FileId(s.file), s.anchor_kind, sig));
        }
        for cf in &self.changed.files {
            let Some(at) = id.checked_sub(cf.first_section.0) else { continue };
            if let Some(s) = cf.sections.get(at as usize) {
                let sig = if s.anchor_kind == 1 {
                    self.changed.string(self.view, s.anchor)?
                } else {
                    Vec::new()
                };
                return Ok((cf.file, s.anchor_kind, sig));
            }
        }
        Err(corrupt("a section past every file's"))
    }

    /// The signature of a copy's group (its mention by position, its
    /// section's anchor).
    fn signature(&self, row: &CopyRow) -> Result<Vec<u8>, IncrReason> {
        let (first, mentions) = self.mentions(FileId(row.file))?;
        let at =
            row.index.checked_sub(first).ok_or_else(|| corrupt("a copy before first_global"))?;
        let m = mentions.get(at as usize).ok_or_else(|| corrupt("a copy without its mention"))?;
        if m.section == NONE {
            return Err(corrupt("a copy's mention without a section"));
        }
        let (_, anchor_kind, sig) = self.section(m.section)?;
        if anchor_kind != 1 {
            return Err(corrupt("a copy's section outside any group"));
        }
        Ok(sig)
    }

    /// A key's live copies (no promised order).
    fn copies(&self, key: SymbolId) -> Result<Vec<CopyRow>, IncrReason> {
        let mut out = Vec::new();
        if self.is_old_key(key) {
            for row in self.view.copies(key)? {
                if !row.tomb && self.replaced(FileId(row.file)).is_none() {
                    out.push(row);
                }
            }
        }
        if let Some(new) = self.new_copies.get(&key) {
            out.extend(new.iter().copied());
        }
        Ok(out)
    }

    fn resolved_row(&self, key: SymbolId) -> Result<ResolvedRow, IncrReason> {
        if let Ok(i) = self.delta.resolved.binary_search_by_key(&key, |r| r.0) {
            return Ok(self.delta.resolved[i].1);
        }
        if self.is_old_key(key) { self.view.resolved(key) } else { Ok(ResolvedRow::default()) }
    }

    fn commons(&self, key: SymbolId) -> Result<Vec<CommonRow>, IncrReason> {
        let edits = &self.delta.commons;
        let mut rows = if self.is_old_key(key) { self.view.commons(key)? } else { Vec::new() };
        rows.retain(|r| {
            !edits
                .delete
                .iter()
                .chain(&edits.put)
                .any(|e| same_key_file((e.key, e.file), (r.key, r.file)))
        });
        rows.extend(edits.put.iter().filter(|e| e.key == key.0 && !e.tomb).copied());
        Ok(rows)
    }

    fn key_refs(&self, key: SymbolId) -> Result<Vec<KeyRefRow>, IncrReason> {
        let edits = &self.delta.key_refs;
        let mut rows = if self.is_old_key(key) { self.view.key_refs(key)? } else { Vec::new() };
        rows.retain(|r| {
            !edits
                .delete
                .iter()
                .chain(&edits.put)
                .any(|e| same_key_file((e.key, e.file), (r.key, r.file)))
        });
        rows.extend(edits.put.iter().filter(|e| e.key == key.0 && !e.tomb).copied());
        Ok(rows)
    }

    /// A group's live `GROUPS` rows, by signature BYTES.
    fn group(&self, signature: &[u8]) -> Result<Vec<GroupRow>, IncrReason> {
        let edits = &self.delta.groups;
        let mine = |rows: &[GroupRow]| -> Result<Vec<u32>, IncrReason> {
            let mut files = Vec::new();
            for e in rows {
                if self.changed.string(self.view, e.signature)? == signature {
                    files.push(e.file);
                }
            }
            Ok(files)
        };
        let gone: Vec<u32> = [mine(&edits.delete)?, mine(&edits.put)?].concat();
        let mut rows = self.view.group(signature)?;
        rows.retain(|r| !r.tomb && !gone.contains(&r.file));
        for e in &edits.put {
            if !e.tomb && self.changed.string(self.view, e.signature)? == signature {
                rows.push(*e);
            }
        }
        Ok(rows)
    }

    /// The key's name as a diagnostic writes it.
    fn name(&self, key: SymbolId) -> Result<String, IncrReason> {
        let (name, version) = if self.is_old_key(key) {
            let s = self.view.symbol(key)?;
            (self.view.string(s.name)?, self.view.string(s.version)?)
        } else {
            let at = u64::from(key.0) - self.symbols;
            let k = usize::try_from(at)
                .ok()
                .and_then(|at| self.delta.new_keys.get(at))
                .ok_or_else(|| corrupt("a new key past ResolveDelta's"))?;
            (k.name.clone(), k.version.clone())
        };
        let mut out = String::from_utf8_lossy(&name).into_owned();
        if !version.is_empty() {
            out.push('@');
            out.push_str(&String::from_utf8_lossy(&version));
        }
        Ok(out)
    }

    /// A file's display name (`spec/diag.md` §2.3): `path`, or
    /// `archive(member)`, from its `FILES` row (an appeared file's from
    /// its `ChangedFile::row`).
    ///
    /// # Errors
    /// `state-corrupt`.
    pub fn display(&self, file: FileId) -> Result<FileRef, IncrReason> {
        let row: FileRow = match self.files.get(&file) {
            Some(cf) if cf.change == Change::Appeared => cf.row.clone(),
            _ => self.view.file(file)?,
        };
        let path = String::from_utf8_lossy(&self.changed.string(self.view, row.path)?).into_owned();
        if row.member == 1 {
            let member = self.changed.string(self.view, row.member_name)?;
            Ok(FileRef::in_archive(path, String::from_utf8_lossy(&member)))
        } else {
            Ok(FileRef::new(path))
        }
    }

    /// `file`'s entry for `key` of mention kind `kind` (in section
    /// `section` when not [`NONE`]): its index and row.
    fn entry(
        &self,
        file: FileId,
        key: SymbolId,
        kind: u8,
        section: u32,
    ) -> Result<(SymbolIndex, MentionRow), IncrReason> {
        let (first, mentions) = self.mentions(file)?;
        let (at, m) = mentions
            .iter()
            .enumerate()
            .find(|(_, m)| {
                m.symbol == key.0 && m.kind == kind && (section == NONE || m.section == section)
            })
            .ok_or_else(|| corrupt("a resolved definition without its mention"))?;
        let at = u32::try_from(at).map_err(|_| corrupt("a mention position"))?;
        Ok((SymbolIndex(first + at), *m))
    }

    /// The key's resolved definition as W2 reads it: whether it is outside
    /// the key's copies (an object's definition or common, not in a
    /// grouped section) and, when it is, its place, binding, type and
    /// commonness. The file: the section's; a common's, the winning
    /// `COMMONS` row; an absolute's, the `KEY_REFS` definer (the strong
    /// one, else the least current rank: resolve.md §3). Index, binding
    /// and type: that file's `MENTIONS` entry of the key by position.
    ///
    /// # Errors
    /// `state-corrupt`.
    pub fn resolved(
        &self,
        key: SymbolId,
        rank: &dyn Fn(u32) -> Result<u64, IncrReason>,
    ) -> Result<(bool, Option<Resolved>), IncrReason> {
        let r = self.resolved_row(key)?;
        let (file, common) = match r.kind {
            def_kind::SECTION => {
                let (file, anchor_kind, _) = self.section(r.section)?;
                if anchor_kind == 1 {
                    return Ok((false, None));
                }
                (file, false)
            }
            def_kind::COMMON => {
                let rows = self.commons(key)?;
                let w = rows::winning_common(&rows, rank)?
                    .ok_or_else(|| corrupt("a common key without COMMONS rows"))?;
                (FileId(w.file), true)
            }
            def_kind::ABSOLUTE => {
                let mut best: Option<(bool, u64, u32)> = None;
                for k in self.key_refs(key)? {
                    if k.tomb || k.def != def_kind::ABSOLUTE {
                        continue;
                    }
                    let at = (k.bind == STB_WEAK, rank(k.file)?, k.file);
                    if best.is_none_or(|b| at < b) {
                        best = Some(at);
                    }
                }
                let (_, _, f) =
                    best.ok_or_else(|| corrupt("an absolute key without its definer"))?;
                (FileId(f), false)
            }
            _ => return Ok((false, None)),
        };
        let section = if r.kind == def_kind::SECTION { r.section } else { NONE };
        let kind = if common { def_kind::COMMON } else { r.kind };
        let (index, m) = self.entry(file, key, kind, section)?;
        let at = At { file, index };
        Ok((true, Some(Resolved { at, binding: m.bind, sym_type: m.typ, common })))
    }

    /// [`KeyInput`] of a key: its live copies, their groups (by signature
    /// bytes) and keepers, its name and resolved definition. `rank` gives
    /// a `FILES` row's CURRENT rank. A key without copies gives an input
    /// without copies and reads nothing more.
    ///
    /// # Errors
    /// `state-corrupt`.
    pub fn key_input(
        &self,
        key: SymbolId,
        rank: &dyn Fn(u32) -> Result<u64, IncrReason>,
    ) -> Result<KeyInput, IncrReason> {
        let mut found: Vec<(Vec<u8>, u64, CopyRow)> = Vec::new();
        for row in self.copies(key)? {
            if u64::from(row.key) != u64::from(key.0) {
                return Err(corrupt("a changed file's copy without its key id"));
            }
            found.push((self.signature(&row)?, rank(row.file)?, row));
        }
        if found.is_empty() {
            return Ok(KeyInput {
                key,
                name: String::new(),
                copies: Vec::new(),
                groups: Vec::new(),
                outside: false,
                resolved: None,
            });
        }
        found.sort_by(|a, b| (&a.0, a.1, a.2.index).cmp(&(&b.0, b.1, b.2.index)));
        let mut groups: Vec<GroupInput> = Vec::new();
        let mut copies = Vec::with_capacity(found.len());
        for (signature, r, row) in found {
            if groups.last().is_none_or(|g| g.signature != signature) {
                let keeper = rows::keeper(&self.group(&signature)?, rank)?.map(|g| FileId(g.file));
                groups.push(GroupInput { signature, keeper });
            }
            let group = u32::try_from(groups.len() - 1).map_err(|_| corrupt("groups"))?;
            copies.push(RowCopy { row, group, rank: r });
        }
        let (outside, resolved) = self.resolved(key, rank)?;
        Ok(KeyInput { key, name: self.name(key)?, copies, groups, outside, resolved })
    }

    /// The ODR-affected keys (IN-71d): every key of a COMDAT copy in a
    /// changed, appeared, gone, drawn-in, extracted or no longer extracted
    /// file (its stored version's `MENTIONS` in grouped sections, its new
    /// version's copies), every key whose `RESOLVED` row the update
    /// rewrites, and every key with a copy in a group whose keeper changed
    /// (`ResolveDelta::reelected`: "the first is kept" can change with no
    /// copy changing).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn affected(&self) -> Result<BTreeSet<SymbolId>, IncrReason> {
        let mut keys = BTreeSet::new();
        for cf in &self.changed.files {
            if cf.change == Change::Appeared || u64::from(cf.file.0) >= self.file_rows {
                continue;
            }
            for m in self.view.mentions_of(cf.file)? {
                if m.kind == def_kind::SECTION
                    && m.section != NONE
                    && self.view.section(SectionId(m.section))?.anchor_kind == 1
                {
                    keys.insert(SymbolId(m.symbol));
                }
            }
        }
        keys.extend(self.new_copies.keys().copied());
        keys.extend(self.delta.resolved.iter().map(|r| r.0));
        for (signature, old, new) in &self.delta.reelected {
            let mut files: BTreeSet<FileId> = [*old, *new].into_iter().collect();
            files.extend(self.group(signature)?.iter().map(|g| FileId(g.file)));
            for file in files {
                let (_, mentions) = self.mentions(file)?;
                for m in mentions {
                    if m.kind != def_kind::SECTION || m.section == NONE {
                        continue;
                    }
                    let (_, anchor_kind, sig) = self.section(m.section)?;
                    if anchor_kind == 1 && sig == *signature {
                        keys.insert(SymbolId(m.symbol));
                    }
                }
            }
        }
        Ok(keys)
    }

    /// A finding as a `DIAGS` row: §9.1's key with the unit as the `FILES`
    /// row, the subjects the key and each file named (IN-75a, WP-220),
    /// made by [`super::diags::row`], capture's own function (WP-221 seam
    /// 7, F-8).
    ///
    /// # Errors
    /// `state-corrupt`: a file named without its row.
    pub fn diag_row(&self, finding: Finding) -> Result<DiagRow, IncrReason> {
        let mut shown: BTreeMap<FileId, FileRef> = BTreeMap::new();
        for at in finding.classes.iter().flatten() {
            if let std::collections::btree_map::Entry::Vacant(e) = shown.entry(at.file) {
                e.insert(self.display(at.file)?);
            }
        }
        let display = |f: FileId| shown.get(&f).cloned();
        let (key, d, s) = diagnostic(&display, finding);
        let mut subjects = vec![Subject::Key(s.symbol)];
        subjects.extend(s.files.into_iter().map(Subject::File));
        // WP-221 seam 7 (F-8): the one row function capture uses too.
        super::diags::row(key, &d, subjects, Vec::new())
            .ok_or_else(|| corrupt("an ODR finding that is not a warning"))
    }
}

/// Whether a changed file's rows replace its stored ones (a drawn-in
/// file's are equal to them: the stored ones are read).
fn replaces(cf: &ChangedFile) -> bool {
    cf.change != Change::DrawnIn
}

/// [`KeyInput`] of a stored key, the state alone (WP-220's proof:
/// `tests/incr_odr.rs`). `rank` gives a `FILES` row's current rank.
///
/// # Errors
/// `state-corrupt`.
pub fn stored_key(
    view: &StateView<'_>,
    key: SymbolId,
    rank: &dyn Fn(u32) -> Result<u64, IncrReason>,
) -> Result<KeyInput, IncrReason> {
    let (changed, delta) = (ChangedFiles::default(), ResolveDelta::default());
    Now::new(view, &changed, &delta).key_input(key, rank)
}

/// [`Now::resolved`] of a stored key, the state alone.
///
/// # Errors
/// `state-corrupt`.
pub fn stored_resolved(
    view: &StateView<'_>,
    key: SymbolId,
    rank: &dyn Fn(u32) -> Result<u64, IncrReason>,
) -> Result<(bool, Option<Resolved>), IncrReason> {
    let (changed, delta) = (ChangedFiles::default(), ResolveDelta::default());
    Now::new(view, &changed, &delta).resolved(key, rank)
}

/// [`Now::display`] of a stored file, the state alone.
///
/// # Errors
/// `state-corrupt`.
pub fn stored_display(view: &StateView<'_>, file: FileId) -> Result<FileRef, IncrReason> {
    let changed = ChangedFiles::default();
    let delta = ResolveDelta::default();
    Now::new(view, &changed, &delta).display(file)
}
