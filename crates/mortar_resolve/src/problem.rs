//! The diagnostics of stage A (`spec/resolve.md` §14), as data: each
//! carries what its message needs (files, keys, the two definers, ...)
//! and its key (`spec/model.md` §8). Stage B maps them to `mortar_diag`
//! kinds with GNU ld's phrases.
//!
//! [`Problem`]'s order is its key, then its content: sorting a list of
//! problems gathered in any order gives the order of the diagnostics.

use mortar_core::{ComdatId, FileId, OptId, SymEntry, SymbolId};
use mortar_elf::{SectionIndex, SymbolIndex};

use crate::sections::SectionRefusal;

/// The phase ordinal of resolution's own diagnostics (§12.3).
pub const PHASE: u8 = 30;

/// A diagnostic key: phase, unit, sequence (`spec/model.md` §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DiagKey {
    pub phase: u8,
    pub unit: u32,
    pub seq: u64,
}

impl DiagKey {
    /// Phase 30 at the symbol-table entry `entry`.
    #[must_use]
    pub fn at(entry: SymEntry) -> Self {
        Self { phase: PHASE, unit: entry.file.0, seq: u64::from(entry.index.0) }
    }

    /// Phase 30, unit `file`, sequence `seq`.
    #[must_use]
    pub fn of(file: FileId, seq: u64) -> Self {
        Self { phase: PHASE, unit: file.0, seq }
    }
}

/// How bad a problem is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Warning,
    Error,
}

/// Why a symbol-table entry is malformed (`malformed-input`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MalformedSymbol {
    /// A non-local `STT_SECTION` or `STT_FILE` (§3.4).
    NonLocalSectionOrFile,
    /// An undefined `STB_GNU_UNIQUE` (§3.6).
    UndefinedUnique,
    /// A common whose alignment is not a power of two (§3.3).
    CommonAlignment(u64),
}

/// A definition's place in a `duplicate-symbol` diagnostic (§3.8): the
/// defining section and `st_value`, or an absolute value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DefPlace {
    pub entry: SymEntry,
    /// The defining section; `None` for an absolute definition.
    pub section: Option<SectionIndex>,
    pub value: u64,
}

/// What went wrong, with the data its message needs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum What {
    /// `undefined-symbol` for a judged reference of a needed or used
    /// shared object (`spec/dyn.md` D-13): entry `index` of `file`'s
    /// `.dynsym` refers to `key`, which nothing satisfies. A warning under
    /// `--warn-unresolved-symbols`.
    ShlibUndefined { file: FileId, index: SymbolIndex, key: SymbolId, warn: bool },
    /// `non-exported-symbol` (§3.5): a needed or used shared object
    /// (`entry`) refers to `key`, whose program definition `def` is hidden
    /// and which no live shared object defines.
    NonExported { key: SymbolId, def: SymEntry, entry: SymEntry },
    /// `Refused{Feature, "archive member needed by an --as-needed shared
    /// object"}` (`spec/dyn.md` D-12, Q1): `entry`, a strong reference of
    /// an as-needed shared object found needed, names `key`, which only
    /// the unextracted lazy file `member` defines.
    AsNeededExtraction { key: SymbolId, entry: SymEntry, member: FileId },
    /// `Refused{Feature, ..}` (`spec/dyn.md` D-19a): the keeping mention of
    /// the needed object `file` depends on a mention whose `AS_NEEDED` or
    /// group state the input tables do not record (a second mention of
    /// one soname inside a linker script).
    NeededOrderUnknown { file: FileId },
    /// `Refused{FileFormat, "GCC LTO object"}` (§10).
    LtoSlim { file: FileId },
    /// `Refused{SymbolBinding, <0x..>}` (§1.2).
    SymbolBinding { entry: SymEntry, binding: u8 },
    /// `Refused{SymbolBinding, "STB_WEAK common"}` (§1.2).
    WeakCommon { entry: SymEntry },
    /// `Refused{SymbolType, ..}` (§1.2).
    SymbolType { entry: SymEntry, sym_type: u8 },
    /// `Refused{Feature, "SHN_X86_64_LCOMMON"}` or `"SHN 0x.."` (§2).
    ReservedSection { entry: SymEntry, shndx: u16 },
    /// `malformed-input` for a symbol (§3.3, §3.4, §3.6).
    Malformed { entry: SymEntry, why: MalformedSymbol },
    /// A section refused by §10.
    Section { file: FileId, section: SectionIndex, why: SectionRefusal },
    /// `Refused{Feature, "group flags 0x.."}` (§6.4).
    GroupFlags { file: FileId, section: SectionIndex, flags: u32 },
    /// `malformed-input`: a section in two groups (§6.5).
    SectionInTwoGroups { file: FileId, section: SectionIndex },
    /// `duplicate-symbol` (§3.8): the winning definition first, then the
    /// other clashing ones in `≺`.
    Duplicate { key: SymbolId, defs: Box<[DefPlace]> },
    /// `tls-mismatch` (§3.4): the definition (or the first mention), and
    /// the first mention of the other kind.
    TlsMismatch { key: SymbolId, first: SymEntry, first_tls: bool, other: SymEntry },
    /// `linker-symbol-redefined` (§8.2).
    LinkerSymbolRedefined { key: SymbolId, entry: SymEntry },
    /// `required-symbol` (§4.2).
    RequiredSymbol { key: OptId<SymbolId>, name: Box<[u8]>, arg: u32 },
    /// `entry-not-found` (§5.5), a warning.
    EntryNotFound { key: OptId<SymbolId>, name: Box<[u8]>, arg: u32 },
    /// `common-larger-than-definition` (§3.3), a warning.
    CommonLarger { key: SymbolId, def: SymEntry, def_size: u64, common: SymEntry, common_size: u64 },
    /// `common-alignment` (§3.3), a warning.
    CommonAlignment {
        key: SymbolId,
        def: SymEntry,
        def_align: u64,
        common: SymEntry,
        common_align: u64,
    },
    /// `multiple-common` (`--warn-common`), a warning.
    MultipleCommon { key: SymbolId, first: SymEntry, second: SymEntry },
    /// `common-overridden` (`--warn-common`), a warning.
    CommonOverridden { key: SymbolId, common: SymEntry, def: SymEntry },
    /// `archive-backref` (§4.6), a warning: GNU ld would leave `key`
    /// undefined; `member` is mortar's definer, `referrer` the first
    /// strong reference.
    ArchiveBackref { key: SymbolId, member: FileId, referrer: SymEntry },
    /// `archive-choice` (§4.6), a warning: GNU ld would take another
    /// definition.
    ArchiveChoice { key: SymbolId, mortar: SymEntry, gnu: SymEntry, referrer: SymEntry },
    /// `gnu-warning` for a `.gnu.warning` section of a live file (§10),
    /// a warning; the text is the section's.
    GnuWarning { file: FileId, section: SectionIndex },
    /// `meta-malformed` (`spec/meta.md` MT-6), a warning, once per file:
    /// a section named `.mason.meta` that is not a container (its type or
    /// flags are not MT-1's); it took the other rows of §10.
    MetaNotContainer { file: FileId, section: SectionIndex },
}

/// A diagnostic of resolution.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Problem {
    pub key: DiagKey,
    pub what: What,
}

impl Problem {
    /// A problem keyed at the entry it names.
    pub(crate) fn at(entry: SymEntry, what: What) -> Self {
        Self { key: DiagKey::at(entry), what }
    }

    /// The default severity (§14).
    #[must_use]
    pub fn severity(&self) -> Severity {
        match self.what {
            What::EntryNotFound { .. }
            | What::CommonLarger { .. }
            | What::CommonAlignment { .. }
            | What::MultipleCommon { .. }
            | What::CommonOverridden { .. }
            | What::ArchiveBackref { .. }
            | What::ArchiveChoice { .. }
            | What::GnuWarning { .. }
            | What::MetaNotContainer { .. }
            | What::ShlibUndefined { warn: true, .. } => Severity::Warning,
            _ => Severity::Error,
        }
    }

    /// The `mortar_diag` kind id stage B maps the problem to (§14).
    #[must_use]
    pub fn id(&self) -> &'static str {
        match self.what {
            What::AsNeededExtraction { .. }
            | What::NeededOrderUnknown { .. }
            | What::LtoSlim { .. }
            | What::SymbolBinding { .. }
            | What::WeakCommon { .. }
            | What::SymbolType { .. }
            | What::ReservedSection { .. }
            | What::GroupFlags { .. }
            | What::Section { .. } => "refused",
            What::Malformed { .. } | What::SectionInTwoGroups { .. } => "malformed-input",
            What::Duplicate { .. } => "duplicate-symbol",
            What::TlsMismatch { .. } => "tls-mismatch",
            What::LinkerSymbolRedefined { .. } => "linker-symbol-redefined",
            What::RequiredSymbol { .. } => "required-symbol",
            What::EntryNotFound { .. } => "entry-not-found",
            What::CommonLarger { .. } => "common-larger-than-definition",
            What::CommonAlignment { .. } => "common-alignment",
            What::MultipleCommon { .. } => "multiple-common",
            What::CommonOverridden { .. } => "common-overridden",
            What::ArchiveBackref { .. } => "archive-backref",
            What::ArchiveChoice { .. } => "archive-choice",
            What::GnuWarning { .. } => "gnu-warning",
            What::MetaNotContainer { .. } => "meta-malformed",
            What::ShlibUndefined { .. } => "undefined-symbol",
            What::NonExported { .. } => "non-exported-symbol",
        }
    }
}

/// The data the scan needs for `comdat-mismatch` (§6.3 (b)): a key that
/// only dropped copies define.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ComdatMismatch {
    pub key: SymbolId,
    /// The group of the first dropped definition by `≺`.
    pub group: ComdatId,
    /// The file whose copy of `group` is kept.
    pub keeper: FileId,
    /// The first dropped definition by `≺`.
    pub dropped: SymEntry,
}
