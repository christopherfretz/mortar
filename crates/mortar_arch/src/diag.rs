//! The diagnostics of scan and fill (§3.4, §7): each [`ScanError`] and
//! [`FieldError`] as its `mortar_diag` kind, its place and its key.
//!
//! The message texts are `mortar_diag`'s (diag.md §3.2, from arch.md §10);
//! this module chooses the kind, the place and the key.

use mortar_core::{ChunkId, FileId};
use mortar_diag::{Diagnostic, FileRef, IncompatibleReason, Key, Kind, Place, RefusalClass};

use crate::cx::{ScanError, fill_key, scan_key};
use crate::error::{FieldError, Incompatible, RefName, Refusal};
use crate::shapes::Erratum;

/// The feature name of §3.4's overlap refusal.
pub const OVERLAP: Refusal = Refusal::named(RefusalClass::Feature, "overlapping relocations");

/// Where a relocation is, for its diagnostic: the key's parts and the
/// place `file:(section+0xoff)`.
#[derive(Debug, Clone, Copy)]
pub struct RelocPlace<'a> {
    pub file: FileId,
    pub path: &'a FileRef,
    /// The section's index in its file.
    pub section_index: u32,
    pub section: &'a str,
    /// The record's index in its relocation section.
    pub record: u32,
    pub r_offset: u64,
}

impl RelocPlace<'_> {
    fn place(&self) -> Place {
        Place::section(self.path.clone(), self.section, self.r_offset)
    }
}

impl Incompatible {
    /// The `mortar_diag` reason (diag.md §3.2), in the same order.
    #[must_use]
    pub const fn reason(self) -> IncompatibleReason {
        match self {
            Self::RecompilePic => IncompatibleReason::RecompileFpic,
            Self::RecompilePie => IncompatibleReason::RecompileFpie,
            Self::ReadOnly => IncompatibleReason::ReadOnlySection,
            Self::AddendMustBeZero => IncompatibleReason::AddendMustBeZero,
            Self::NotThreadLocal => IncompatibleReason::NotThreadLocal,
            Self::TlsSectionSymbol => IncompatibleReason::TlsSectionSymbol,
            Self::ThreadLocal => IncompatibleReason::ThreadLocal,
            Self::LocalExecInShared => IncompatibleReason::LocalExecInShared,
            Self::InstructionMismatch => IncompatibleReason::InstructionMismatch,
            Self::NotInNonAlloc => IncompatibleReason::NonAlloc,
        }
    }
}

fn saturate(v: i128) -> i64 {
    i64::try_from(v).unwrap_or(if v < 0 { i64::MIN } else { i64::MAX })
}

fn outside(r_type: RefName, off: u64) -> Kind {
    Kind::Malformed {
        what: format!("relocation {r_type} at offset {off:#x} lies outside its section"),
    }
}

fn internal(r_type: RefName, what: &str) -> Kind {
    Kind::InternalError { what: format!("relocation {r_type}: {what}") }
}

/// The key of the `undefined-symbol` diagnostic of one undefined key
/// (§3.4): the scan key of its first surviving reference in `(FileId,
/// section index, record index)` order, which the driver chooses among
/// the sink's `undefined` events.
#[must_use]
pub fn undefined_key(file: FileId, section_index: u32, record: u32) -> Key {
    scan_key(file, section_index, record)
}

/// A scan error (§3.4) against the target named `target` (its
/// symbol-table name, or `section+0xoff` for a section symbol): the
/// diagnostic and its key `(70, FileId, section index × 2^32 + record)`.
#[must_use]
pub fn scan(
    err: ScanError,
    at: &RelocPlace<'_>,
    r_type: RefName,
    target: &str,
) -> (Key, Diagnostic) {
    let name = || target.to_owned();
    let d = match err {
        ScanError::Refused(r) => r.diagnostic(),
        ScanError::Incompatible(i) => Diagnostic::error(Kind::RelocationIncompatible {
            r_type: r_type.to_string(),
            name: name(),
            reason: i.reason(),
        }),
        ScanError::Dynamic(r) => Diagnostic::error(r.kind(r_type.to_string(), name())),
        ScanError::Outside => Diagnostic::error(outside(r_type, at.r_offset)),
        ScanError::Overlap => OVERLAP.diagnostic(),
        ScanError::Internal => Diagnostic::error(internal(
            r_type,
            "the target kind has no cell for this output kind (spec/arch.md §4.3)",
        )),
    };
    (scan_key(at.file, at.section_index, at.record), d.at(at.place()))
}

fn field_kind(err: FieldError, r_type: RefName, target: &str, off: u64) -> Diagnostic {
    match err {
        FieldError::Range(r) => Diagnostic::error(Kind::RelocationOutOfRange {
            r_type: r_type.to_string(),
            value: saturate(r.value),
            min: saturate(r.min),
            max: saturate(r.max),
            name: target.to_owned(),
        }),
        FieldError::Misaligned { value, align } => Diagnostic::error(Kind::RelocationMisaligned {
            r_type: r_type.to_string(),
            value,
            align: u64::from(align),
        }),
        FieldError::Outside { offset, .. } => Diagnostic::error(outside(r_type, offset)),
        FieldError::Instruction { .. } => Diagnostic::error(Kind::RelocationIncompatible {
            r_type: r_type.to_string(),
            name: target.to_owned(),
            reason: IncompatibleReason::InstructionMismatch,
        }),
        FieldError::MissingSlot => Diagnostic::error(internal(
            r_type,
            &format!("no slot in Slots at offset {off:#x} (spec/arch.md §5.2)"),
        )),
        FieldError::MissingTerm(term) => Diagnostic::error(internal(
            r_type,
            &format!("no {term} in the tables at offset {off:#x}"),
        )),
        FieldError::Refused(r) => r.diagnostic(),
    }
}

/// A fill error (§7) of a relocation: the diagnostic and its key `(100,
/// FileId, section index × 2^32 + record)`.
#[must_use]
pub fn fill(
    err: FieldError,
    at: &RelocPlace<'_>,
    r_type: RefName,
    target: &str,
) -> (Key, Diagnostic) {
    let d = field_kind(err, r_type, target, at.r_offset);
    (fill_key(at.file, at.section_index, at.record), d.at(at.place()))
}

/// A fill error (§7) of a synthetic chunk (a PLT entry, a thunk, a patch):
/// the place is the output section and offset, the key `(100, 0,
/// ChunkId)`. `what` names the field (the thunk's `ADRP` type, say).
#[must_use]
pub fn chunk(
    err: FieldError,
    chunk: ChunkId,
    output: &FileRef,
    section: &str,
    offset: u64,
    what: RefName,
    target: &str,
) -> (Key, Diagnostic) {
    let d = field_kind(err, what, target, offset);
    (Key::new(100, 0, u64::from(chunk.0)), d.at(Place::section(output.clone(), section, offset)))
}

/// The refusal of an erratum candidate that is neither proved code nor
/// safely skipped (§3.7 `erratum_sites`, a64.md §11.1 condition 5):
/// `refused`, class `feature`, with a note naming the way to link without
/// the fix: leaving out the option that asks for it (mortar, like GNU ld,
/// lld and mold, has no option that turns it off), and for a GCC driver
/// that passes it, `-mno-fix-cortex-a53-843419`. The caller adds the place,
/// the candidate's input section and the offset of `W1` in it
/// (`Unproved.q_unit`, `q_unit_offset`).
#[must_use]
pub fn unproved(e: Erratum) -> Diagnostic {
    let (name, way_out) = match e {
        Erratum::E843419 => (
            "erratum 843419 scan without mapping symbols",
            "link without --fix-cortex-a53-843419 (a GCC driver that passes it \
             drops it with -mno-fix-cortex-a53-843419)",
        ),
        // `Erratum` is `non_exhaustive`; every variant has its own arm.
        _ => {
            ("erratum scan without mapping symbols", "link without the option of the erratum's fix")
        }
    };
    Diagnostic::refused(RefusalClass::Feature, name)
        .note(format!("the candidate cannot be proved code or data; {way_out}, for no erratum fix"))
}

/// The key of [`unproved`]'s diagnostic: `(90, 0, ChunkId)` of the
/// candidate's input section, because candidates depend on the layout's
/// addresses (model §8; a64.md §11.1).
#[must_use]
pub fn unproved_key(chunk: ChunkId) -> Key {
    Key::new(90, 0, u64::from(chunk.0))
}
