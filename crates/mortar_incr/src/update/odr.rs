//! ODR checks in an update (`spec/incr.md` IN-71d; odr.md §8.1): W1 and
//! W2 recomputed for the affected keys from `COPIES`, `RESOLVED` and the
//! changed files' copies, by the full link's comparator
//! (`mortar_odr::foreign::compare_copies`); every other check of odr.md
//! falls back from the switches alone
//! (`mortar_odr::register::update_fallback`). WP-213 (U-1): the entry
//! point [`odr`]; WP-220 seam: its signature takes the switches; **filled
//! by U-8** (the function only).
//!
//! What the comparator reads of a key is built from rows alone by
//! [`stored_key`] ([`KeyInput`]): each copy's `COPIES` row (size, type,
//! index), its group (the copy's `MENTIONS` row by position, that row's
//! `SECTIONS.anchor`), its file's current rank, the key's name
//! (`SYMBOLS`), each group's keeper (`GROUPS`, by current rank), the
//! resolved definition (`RESOLVED`, its section's or `KEY_REFS`' file,
//! that file's `MENTIONS` for the index) and each file's display
//! ([`stored_display`], from `FILES`).

use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_core::inputs::Reader;
use mortar_core::plan::Odr;
use mortar_core::{FileId, Inputs, Resolution, SectionId, SymbolId};
use mortar_diag::FileRef;
use mortar_elf::SymbolIndex;
use mortar_elf::consts::STB_WEAK;
use mortar_odr::foreign::{At, CopyDef, Definition, Finding, Names, Resolved, compare_copies};

use super::diags::DiagRow;
use super::resolve::ResolveDelta;
use super::rows::{self, CopyRow, MentionRow};
use super::view::{ChangedFiles, StateView};
use crate::format::{NONE, def_kind};

/// The ODR findings of the ODR-affected keys (WP-211 §C; IN-71d), as
/// `DIAGS` rows for U-9's union. Called by the join after `resolve`.
///
/// # Errors
/// `state-corrupt`; `update-not-implemented (meta)` when the switches
/// turn on anything but W1 and W2 (IN-71d), or under `--keep-mason-meta`
/// with units in a changed file. The switches are passed in, never
/// stored. Until U-8 lands: `update-not-implemented (meta)` when any
/// changed file carries `.mason.meta` units, else no findings.
pub fn odr(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    resolved: &ResolveDelta,
    odr: &Odr,
) -> Result<Vec<DiagRow>, IncrReason> {
    let _ = (view, resolved, odr);
    if changed.files.iter().any(|f| f.meta_units > 0) {
        return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Meta)));
    }
    Ok(Vec::new())
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

/// The signature of a stored copy's group: the copy's `MENTIONS` row (at
/// position `index - FILE_INDEX.first_global` of its file's range, WP-215
/// D7), that row's section, the section's `SECTIONS.anchor` (`anchor_kind`
/// 1: the signature).
///
/// # Errors
/// `state-corrupt`: no such row, or the section is not in a group.
pub fn stored_signature(view: &StateView<'_>, row: &CopyRow) -> Result<Vec<u8>, IncrReason> {
    let file = FileId(row.file);
    let first = view.file_index(file)?.first_global;
    let at = row.index.checked_sub(first).ok_or_else(|| corrupt("a copy before first_global"))?;
    let mentions = view.mentions_of(file)?;
    let m = mentions.get(at as usize).ok_or_else(|| corrupt("a copy without its mention"))?;
    if m.section == NONE {
        return Err(corrupt("a copy's mention without a section"));
    }
    let section = view.section(SectionId(m.section))?;
    if section.anchor_kind != 1 {
        return Err(corrupt("a copy's section outside any group"));
    }
    view.string(section.anchor)
}

/// The key's name as a diagnostic writes it, from its `SYMBOLS` row.
///
/// # Errors
/// `state-corrupt`.
pub fn stored_name(view: &StateView<'_>, key: SymbolId) -> Result<String, IncrReason> {
    let s = view.symbol(key)?;
    let mut name = String::from_utf8_lossy(&view.string(s.name)?).into_owned();
    let version = view.string(s.version)?;
    if !version.is_empty() {
        name.push('@');
        name.push_str(&String::from_utf8_lossy(&version));
    }
    Ok(name)
}

/// A file's display name (`spec/diag.md` §2.3) from its `FILES` row:
/// `path`, or `archive(member)` for a member; equal to the full link's
/// `File::display` (WP-220's proof).
///
/// # Errors
/// `state-corrupt`.
pub fn stored_display(view: &StateView<'_>, file: FileId) -> Result<FileRef, IncrReason> {
    let f = view.file(file)?;
    let path = String::from_utf8_lossy(&view.string(f.path)?).into_owned();
    if f.member == 1 {
        let member = view.string(f.member_name)?;
        Ok(FileRef::in_archive(path, String::from_utf8_lossy(&member)))
    } else {
        Ok(FileRef::new(path))
    }
}

/// `file`'s entry for `key` of mention kind `kind` (in section `section`
/// when not [`NONE`]), from its `MENTIONS` by position: its symbol-table
/// index and its row.
fn stored_entry(
    view: &StateView<'_>,
    file: FileId,
    key: SymbolId,
    kind: u8,
    section: u32,
) -> Result<(SymbolIndex, MentionRow), IncrReason> {
    let first = view.file_index(file)?.first_global;
    let mentions = view.mentions_of(file)?;
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

/// The key's resolved definition as W2 reads it, from rows: whether it is
/// outside the key's copies (an object's definition or common, not in a
/// grouped section) and, when it is, its place, binding, type and
/// commonness. The file: the section's (`SECTIONS.file`); a common's, the
/// winning `COMMONS` row; an absolute's, the `KEY_REFS` definer (the
/// strong one, else the least current rank: resolve.md §3). The index:
/// that file's `MENTIONS` entry of the key by position, whose binding and
/// type are the entry's.
///
/// # Errors
/// `state-corrupt`.
pub fn stored_resolved(
    view: &StateView<'_>,
    key: SymbolId,
    rank: &dyn Fn(u32) -> Result<u64, IncrReason>,
) -> Result<(bool, Option<Resolved>), IncrReason> {
    let r = view.resolved(key)?;
    let (file, common) = match r.kind {
        def_kind::SECTION => {
            let section = view.section(SectionId(r.section))?;
            if section.anchor_kind == 1 {
                return Ok((false, None));
            }
            (FileId(section.file), false)
        }
        def_kind::COMMON => {
            let rows = view.commons(key)?;
            let w = rows::winning_common(&rows, rank)?
                .ok_or_else(|| corrupt("a common key without COMMONS rows"))?;
            (FileId(w.file), true)
        }
        def_kind::ABSOLUTE => {
            let mut best: Option<(bool, u64, u32)> = None;
            for k in view.key_refs(key)? {
                if k.tomb || k.def != def_kind::ABSOLUTE {
                    continue;
                }
                let at = (k.bind == STB_WEAK, rank(k.file)?, k.file);
                if best.is_none_or(|b| at < b) {
                    best = Some(at);
                }
            }
            let (_, _, f) = best.ok_or_else(|| corrupt("an absolute key without its definer"))?;
            (FileId(f), false)
        }
        _ => return Ok((false, None)),
    };
    let kind = if common { def_kind::COMMON } else { r.kind };
    let section = if r.kind == def_kind::SECTION { r.section } else { NONE };
    let (index, m) = stored_entry(view, file, key, kind, section)?;
    let at = At { file, index };
    Ok((true, Some(Resolved { at, binding: m.bind, sym_type: m.typ, common })))
}

/// [`KeyInput`] of a stored key: its live `COPIES` rows, their groups
/// and keepers, its name and resolved definition. `rank` gives a `FILES`
/// row's CURRENT rank.
///
/// # Errors
/// `state-corrupt`.
pub fn stored_key(
    view: &StateView<'_>,
    key: SymbolId,
    rank: &dyn Fn(u32) -> Result<u64, IncrReason>,
) -> Result<KeyInput, IncrReason> {
    let mut found: Vec<(Vec<u8>, u64, CopyRow)> = Vec::new();
    for row in view.copies(key)? {
        if row.tomb {
            continue;
        }
        found.push((stored_signature(view, &row)?, rank(row.file)?, row));
    }
    found.sort_by(|a, b| (&a.0, a.1, a.2.index).cmp(&(&b.0, b.1, b.2.index)));
    let mut groups: Vec<GroupInput> = Vec::new();
    let mut copies = Vec::with_capacity(found.len());
    for (signature, r, row) in found {
        if groups.last().is_none_or(|g| g.signature != signature) {
            let keeper = rows::keeper(&view.group(&signature)?, rank)?.map(|g| FileId(g.file));
            groups.push(GroupInput { signature, keeper });
        }
        let group = u32::try_from(groups.len() - 1).map_err(|_| corrupt("groups"))?;
        copies.push(RowCopy { row, group, rank: r });
    }
    let (outside, resolved) = stored_resolved(view, key, rank)?;
    Ok(KeyInput { key, name: stored_name(view, key)?, copies, groups, outside, resolved })
}
