//! What an architecture function returns instead of a value: a refusal
//! by name (§3.2), a field error (§7), or the reason a relocation is
//! incompatible with its target (§7, §10).

use std::fmt;

use mortar_diag::{Diagnostic, RefusalClass};

/// The name a refusal carries (diag.md §4.2): a spelling from the defining
/// document, or a number in hexadecimal when it has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefName {
    Named(&'static str),
    /// Shown as `0x…` (lowercase).
    Hex(u64),
    /// A named field and its value, shown as `<what> 0x…` (`e_flags
    /// 0x5`: x86.md X86-1, "the `e_flags` value shown").
    Value {
        what: &'static str,
        value: u64,
    },
}

impl fmt::Display for RefName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Named(name) => f.write_str(name),
            Self::Hex(n) => write!(f, "{n:#x}"),
            Self::Value { what, value } => write!(f, "{what} {value:#x}"),
        }
    }
}

/// A construct refused by name (`docs/ARCHITECTURE.md` principle 1).
/// `Copy` and allocation-free: the text is formed when it is reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Refusal {
    pub class: RefusalClass,
    pub name: RefName,
}

impl Refusal {
    #[must_use]
    pub const fn named(class: RefusalClass, name: &'static str) -> Self {
        Self { class, name: RefName::Named(name) }
    }

    #[must_use]
    pub const fn hex(class: RefusalClass, value: u64) -> Self {
        Self { class, name: RefName::Hex(value) }
    }

    /// The `refused` diagnostic (diag.md §4), without a place.
    #[must_use]
    pub fn diagnostic(&self) -> Diagnostic {
        Diagnostic::refused(self.class, self.name.to_string())
    }
}

/// A value outside the check of its field: `value` as the check saw it
/// (for ELFCLASS32 the 32-bit-reduced value, §2.3) and the inclusive
/// bounds, all in decimal in the message (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Range {
    pub value: i128,
    pub min: i128,
    pub max: i128,
}

/// Why `apply`, `write_*` or `write_addend` could not write a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldError {
    /// `relocation-out-of-range` (§7).
    Range(Range),
    /// `relocation-misaligned` (§7): `value mod align ≠ 0`.
    Misaligned { value: i64, align: u8 },
    /// The field (or its window) does not lie in the section: input that
    /// scan reports as `malformed-input` (§3.2). `apply` returns it rather
    /// than panic if called on such a record.
    Outside { offset: u64, width: u8 },
    /// The instruction at the place does not match the field form (a64.md
    /// §3.2). Scan refuses it first; `apply` returns it rather than encode.
    Instruction { word: u32 },
    /// A slot `apply` needs is not in `Slots`: §5.2 was violated. An
    /// internal error, never a user diagnostic.
    MissingSlot,
    /// A table term `apply` needs is absent (no TLS template, no GOT
    /// base, no PLT entry): an internal error like `MissingSlot`.
    MissingTerm(&'static str),
    /// A form this build does not write; scan refuses it with a place.
    Refused(Refusal),
}

impl FieldError {
    /// An internal error (a violated invariant of the tables), as opposed
    /// to a property of the input.
    #[must_use]
    pub const fn is_internal(&self) -> bool {
        matches!(self, Self::MissingSlot | Self::MissingTerm(_))
    }
}

/// The `{reason}` of `relocation-incompatible` (§10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Incompatible {
    RecompilePic,
    RecompilePie,
    ReadOnly,
    AddendMustBeZero,
    NotThreadLocal,
    ThreadLocal,
    LocalExecInShared,
    InstructionMismatch,
    NotInNonAlloc,
}

impl Incompatible {
    /// Every reason, in §10's order.
    pub const ALL: [Self; 9] = [
        Self::RecompilePic,
        Self::RecompilePie,
        Self::ReadOnly,
        Self::AddendMustBeZero,
        Self::NotThreadLocal,
        Self::ThreadLocal,
        Self::LocalExecInShared,
        Self::InstructionMismatch,
        Self::NotInNonAlloc,
    ];

    /// The text of the reason, exactly as §10 lists it.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::RecompilePic => "recompile with -fPIC",
            Self::RecompilePie => "recompile with -fPIE",
            Self::ReadOnly => "it is in a read-only section",
            Self::AddendMustBeZero => "the addend must be 0",
            Self::NotThreadLocal => "it is not thread-local",
            Self::ThreadLocal => "it is thread-local",
            Self::LocalExecInShared => "local-exec TLS in a shared object",
            Self::InstructionMismatch => "the instruction does not match the relocation",
            Self::NotInNonAlloc => "not allowed in a non-allocated section",
        }
    }
}
