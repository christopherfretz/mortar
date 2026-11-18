//! `.symtab` and `.strtab` per file in an update (`spec/incr.md` IN-49a):
//! each changed or drawn-in file's block rewritten in place or moved to the
//! tail reserve, the global area kept dense. Not `crate::symtab::SymtabPlan`
//! (the full link's). WP-213 (U-1): the types [`FileSymbols`],
//! [`LocalSymbol`], [`GlobalChange`] and the entry point [`plan`];
//! **filled by U-5** (the function only).

use mortar_core::incr::{IncrReason, LayoutDelta, StoredLayout, SymtabPlan, Unimplemented};
use mortar_core::{FileId, SectionId, SymbolId};

/// One local entry of a file's block (S-74), as read from the file's
/// symbol table by the join. Bytes and indices as ELF gives them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalSymbol<'a> {
    /// The name (no terminator); empty for an unnamed entry.
    pub name: &'a [u8],
    /// `st_info`, `st_other`.
    pub info: u8,
    pub other: u8,
    /// Its section in the update's id space, or `None` (absolute or
    /// `STT_FILE`).
    pub section: Option<SectionId>,
    /// `st_value`: the offset in `section` (bytes), or the absolute value.
    pub value: u64,
    /// `st_size` (bytes).
    pub size: u64,
}

/// A file whose block is rewritten (IN-49a: changed, or drawn-in by IN-72
/// item 2a), with its local entries in S-74's order. Written by the join;
/// read by U-5. Key: `file`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSymbols<'a> {
    /// Its `FILES` row (the update's id space).
    pub file: FileId,
    pub locals: Vec<LocalSymbol<'a>>,
}

/// What happens to a global's entry (IN-49a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalKind {
    /// New: appended at the area's end.
    Added,
    /// Removed: the area's last entry takes its index.
    Removed,
    /// Demoted to a local of its definer's block (S-74).
    Demoted,
    /// Promoted back to the area (appended).
    Promoted,
    /// Same index, new value, section, size, binding or name.
    Changed,
}

/// A global whose `.symtab` entry the update changes (IN-49a; WP-213 added
/// it to `plan`'s inputs: WP-211 §C gave U-5 no globals). Written by the
/// join from U-3's `ResolveDelta`; read by U-5. Key: `key`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalChange<'a> {
    /// The `SYMBOLS` row.
    pub key: SymbolId,
    pub kind: GlobalKind,
    pub name: &'a [u8],
    pub info: u8,
    pub other: u8,
    /// Its definition's section in the update's id space, or `None`.
    pub section: Option<SectionId>,
    pub value: u64,
    pub size: u64,
}

/// Plan the update's `.symtab` and `.strtab` writes (WP-211 §C; IN-49a).
/// Called by the join after U-4 (IN-70 U7).
///
/// # Errors
/// `symtab-exhausted`, `state-corrupt`. Until U-5 lands:
/// `update-not-implemented (symtab)`.
pub fn plan(
    stored: &StoredLayout<'_>,
    layout: &LayoutDelta,
    files: &[FileSymbols<'_>],
    globals: &[GlobalChange<'_>],
) -> Result<SymtabPlan, IncrReason> {
    let _ = (stored, layout, files, globals);
    Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Symtab)))
}
