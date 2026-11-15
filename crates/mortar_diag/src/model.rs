//! The diagnostic model (`spec/diag.md` §2 to §4): what a diagnostic says,
//! about which construct, and where. No text is formed here; `render.rs`
//! owns every character mortar prints.

use std::path::Path;

/// How serious a diagnostic is (`spec/diag.md` §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// The link goes on and its output is valid.
    Warning,
    /// The link fails and leaves no output file.
    Error,
}

impl Severity {
    /// The word of the head line: `warning` or `error`.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// What kind of construct a refusal names (`spec/diag.md` §4).
///
/// The class picks the noun of the message and tells a test what was
/// refused; the name of the construct is free, so refusing one more
/// relocation type, option or section flag adds nothing to this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum RefusalClass {
    /// A command-line option mortar knows and does not implement.
    Option,
    /// A value of an implemented option (`--hash-style=sysv`, `-z` keywords).
    OptionValue,
    /// A `-m` emulation.
    Emulation,
    /// A file format (`ELF32`, `big-endian ELF`, `LLVM bitcode`).
    FileFormat,
    /// An `e_machine` value.
    Machine,
    /// An `e_type` value in this role (`ET_EXEC` as an input).
    ObjectType,
    /// An `sh_type` value.
    SectionType,
    /// An `sh_flags` bit.
    SectionFlag,
    /// A relocation type.
    RelocationType,
    /// An `st_info` type.
    SymbolType,
    /// An `st_info` binding.
    SymbolBinding,
    /// A `ch_type` value of a compressed section.
    CompressionType,
    /// A linker-script, version-script or dynamic-list command.
    ScriptCommand,
    /// Anything else: a capability named in words.
    Feature,
}

impl RefusalClass {
    /// Every class, in the order of `spec/diag.md` §4.1.
    pub const ALL: [Self; 14] = [
        Self::Option,
        Self::OptionValue,
        Self::Emulation,
        Self::FileFormat,
        Self::Machine,
        Self::ObjectType,
        Self::SectionType,
        Self::SectionFlag,
        Self::RelocationType,
        Self::SymbolType,
        Self::SymbolBinding,
        Self::CompressionType,
        Self::ScriptCommand,
        Self::Feature,
    ];

    /// The stable kebab-case name of the class, for tests and machine
    /// formats. Never printed in the text form.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Option => "option",
            Self::OptionValue => "option-value",
            Self::Emulation => "emulation",
            Self::FileFormat => "file-format",
            Self::Machine => "machine",
            Self::ObjectType => "object-type",
            Self::SectionType => "section-type",
            Self::SectionFlag => "section-flag",
            Self::RelocationType => "relocation-type",
            Self::SymbolType => "symbol-type",
            Self::SymbolBinding => "symbol-binding",
            Self::CompressionType => "compression-type",
            Self::ScriptCommand => "script-command",
            Self::Feature => "feature",
        }
    }

    /// The noun of the message: `unsupported <noun>: <name>`.
    #[must_use]
    pub const fn noun(self) -> &'static str {
        match self {
            Self::Option => "option",
            Self::OptionValue => "option value",
            Self::Emulation => "emulation",
            Self::FileFormat => "file format",
            Self::Machine => "machine",
            Self::ObjectType => "object type",
            Self::SectionType => "section type",
            Self::SectionFlag => "section flag",
            Self::RelocationType => "relocation type",
            Self::SymbolType => "symbol type",
            Self::SymbolBinding => "symbol binding",
            Self::CompressionType => "compression type",
            Self::ScriptCommand => "linker script command",
            Self::Feature => "feature",
        }
    }
}

/// Why a relocation cannot be used against its target in this output
/// (`spec/diag.md` §3.2, `relocation-incompatible`; `spec/arch.md` §4.3,
/// §7). The reasons are a closed list: a test compares the reason, not the
/// text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum IncompatibleReason {
    /// An absolute or PC-relative use against a target that a shared
    /// object needs to reach position-independently.
    RecompileFpic,
    /// The same, in a position-independent executable.
    RecompileFpie,
    /// The relocation would need a dynamic relocation in a read-only
    /// section (a text relocation, never written).
    ReadOnlySection,
    /// A type whose addend must be 0 has another.
    AddendMustBeZero,
    /// A TLS relocation against a symbol that is not thread-local.
    NotThreadLocal,
    /// A relocation that is not TLS against a thread-local symbol.
    ThreadLocal,
    /// A local-exec TLS relocation in a shared object.
    LocalExecInShared,
    /// The instruction at the place is not the one the field form requires.
    InstructionMismatch,
    /// A relocation other than a data relocation in a section without
    /// `SHF_ALLOC`.
    NonAlloc,
}

impl IncompatibleReason {
    /// Every reason, in the order of `spec/diag.md` §3.2.
    pub const ALL: [Self; 9] = [
        Self::RecompileFpic,
        Self::RecompileFpie,
        Self::ReadOnlySection,
        Self::AddendMustBeZero,
        Self::NotThreadLocal,
        Self::ThreadLocal,
        Self::LocalExecInShared,
        Self::InstructionMismatch,
        Self::NonAlloc,
    ];

    /// The words after `; ` in the message.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::RecompileFpic => "recompile with -fPIC",
            Self::RecompileFpie => "recompile with -fPIE",
            Self::ReadOnlySection => "it is in a read-only section",
            Self::AddendMustBeZero => "the addend must be 0",
            Self::NotThreadLocal => "it is not thread-local",
            Self::ThreadLocal => "it is thread-local",
            Self::LocalExecInShared => "local-exec TLS in a shared object",
            Self::InstructionMismatch => "the instruction does not match the relocation",
            Self::NonAlloc => "not allowed in a non-allocated section",
        }
    }
}

/// What a diagnostic says (`spec/diag.md` §3). Each variant has a stable
/// id ([`Kind::id`]) and one wording, fixed by the specification.
///
/// Variants are grouped by area and sorted by id inside an area; a new one
/// is inserted at its place, never appended (`spec/diag.md` §3.3).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Kind {
    /// `conflicting-options`: two options that cannot be used together,
    /// spelled as the user wrote them (`-r` and `-shared`).
    ConflictingOptions { first: String, second: String },
    // -- command line ---------------------------------------------------
    /// `invalid-option-value`: the value given to an option is not one the
    /// option's grammar allows. `option` is spelled as the user wrote it.
    InvalidOptionValue { option: String, value: String },
    /// `missing-argument`: an option that takes a value ended the command
    /// line.
    MissingArgument { option: String },
    /// `no-input-files`: the command line names nothing to link.
    NoInputFiles,
    /// `response-file-limit`: more response files (`@file`) were expanded
    /// than the limit allows, which a response file that names itself
    /// reaches.
    ResponseFileLimit { limit: u32 },
    /// `unbalanced-option`: an option that opens or closes a region of the
    /// command line (`--push-state`, `--start-group`, `--start-lib`) has
    /// no counterpart. Both are spelled as the user wrote them, or would.
    UnbalancedOption { option: String, counterpart: String },
    /// `unknown-option`: not an option of GNU ld's command line as mortar
    /// knows it. (An option mortar knows and does not implement is a
    /// [`Kind::Refused`].)
    UnknownOption { option: String },

    // -- files ----------------------------------------------------------
    /// `ambiguous-sysroot-path`: a `=` (or `$SYSROOT`) path with a relative
    /// rest while a sysroot is set, which GNU ld and lld read differently
    /// (`spec/input.md` §4.3). `spelling` is the path as written.
    AmbiguousSysrootPath { spelling: String },
    /// `cannot-open-file`: an input could not be opened or its bytes could
    /// not be read. `reason` is the system's ([`crate::os_reason`]).
    CannotOpenFile { path: String, reason: String },
    /// `cannot-open-output`: the output file could not be created.
    CannotOpenOutput { path: String, reason: String },
    /// `cannot-write-output`: writing, closing or renaming the output file
    /// failed after it was created (`spec/image.md` I-60, I-64). `reason`
    /// is the system's ([`crate::os_reason`]).
    CannotWriteOutput { path: String, reason: String },
    /// `duplicate-soname`: a shared object whose `DT_SONAME` is already
    /// loaded from other bytes; this one is not used (`spec/input.md` §8.3).
    DuplicateSoname { soname: String },
    /// `ignored-member`: an archive member that is not a relocatable
    /// object. `what` says what it is (`spec/input.md` §7.2).
    IgnoredMember { what: String },
    /// `incompatible-file`: an ELF file of another target than the link's
    /// (`spec/input.md` §3.3). Both are target names (`aarch64`, `x86_64`).
    IncompatibleFile { expected: String, found: String },
    /// `input-changed`: a file changed while it was read
    /// (`spec/input.md` §11.5).
    InputChanged,
    /// `library-not-found`: no file for `-l<name>` on the search path.
    /// `name` is the text after `-l`.
    LibraryNotFound { name: String },
    /// `needed-not-found`: the `DT_NEEDED` entry `name` of the shared
    /// object `needed_by` was not found (`spec/input.md` §8.6).
    NeededNotFound { name: String, needed_by: String },
    /// `no-target`: neither `-m` nor any ELF input decides the target
    /// (`spec/input.md` §3.1).
    NoTarget,
    /// `script-cycle`: a linker script names itself, directly or through
    /// other scripts (`spec/input.md` §5.3).
    ScriptCycle,
    /// `skipped-incompatible`: a candidate of a search is of another
    /// target and the search goes on (`spec/input.md` §4.4). `search` is
    /// what was searched for: `-lname`, or the name as written.
    SkippedIncompatible { path: String, search: String },
    /// `static-link-of-shared-object`: a shared object in a static link
    /// (`spec/input.md` §8.4).
    StaticLinkOfSharedObject { path: String },

    // -- file formats ---------------------------------------------------
    /// `cet-report` (x86-64 `-z cet-report=warning|error`, `spec/x64.md`
    /// X-43): the live input object `path` lacks `property` (`IBT` or
    /// `SHSTK`) in its `GNU_PROPERTY_X86_FEATURE_1_AND`. One per (object,
    /// property); the severity is the option's.
    CetReport { path: String, property: String },
    /// `malformed-input`: a reader rejected the bytes of a file. `what` is
    /// the reader's statement of what is wrong.
    Malformed { what: String },
    /// `unknown-file-type`: the file is no format mortar can name.
    UnknownFileType,
    /// `unknown-property`: a GNU program property of a type whose merge
    /// rule mortar does not know, dropped from the output
    /// (`spec/synth.md` S-95). `path` names the input. A warning.
    UnknownProperty { path: String, pr_type: u32 },

    // -- symbols --------------------------------------------------------
    /// `archive-backref`: under `--warn-backrefs`, GNU ld's walk would
    /// leave the key undefined (`spec/resolve.md` §4.6).
    ArchiveBackref { name: String },
    /// `archive-choice`: under `--warn-backrefs`, GNU ld's walk would
    /// define the key from another file (`spec/resolve.md` §4.6).
    ArchiveChoice { name: String },
    /// `comdat-mismatch`: a reference to a symbol that only a discarded
    /// copy of the COMDAT group `signature` defines (`spec/resolve.md`
    /// §6.3).
    ComdatMismatch { name: String, signature: String },
    /// `common-alignment`: a definition less aligned than the common
    /// symbols of its key (`spec/resolve.md` §3.3).
    CommonAlignment { name: String },
    /// `common-larger-than-definition`: a common symbol larger than the
    /// definition that wins (`spec/resolve.md` §3.3).
    CommonLargerThanDefinition { name: String },
    /// `common-overridden`: under `--warn-common`, a definition overrides
    /// a common symbol (`spec/resolve.md` §3.3).
    CommonOverridden { name: String },
    /// `discarded-section-reference`: a relocation refers to a symbol of
    /// a section of a discarded COMDAT copy (`spec/resolve.md` §6.3).
    DiscardedSectionReference { name: String },
    /// `duplicate-symbol`: two definitions that may not coexist. `name` is
    /// the name as the symbol table has it (mangled).
    DuplicateSymbol { name: String },
    /// `entry-not-found`: the entry symbol is not defined
    /// (`spec/resolve.md` §5.5).
    EntryNotFound { name: String },
    /// `gnu-warning`: the text of a `.gnu.warning` section
    /// (`spec/resolve.md` §10).
    GnuWarning { text: String },
    /// `linker-symbol-redefined`: an object defines a symbol the linker
    /// defines and does not let a file override (`spec/resolve.md` §8.2).
    LinkerSymbolRedefined { name: String },
    /// `multiple-common`: under `--warn-common`, a second common symbol of
    /// one key (`spec/resolve.md` §3.3).
    MultipleCommon { name: String },
    /// `non-exported-symbol`: a hidden key referenced by a shared object
    /// (`spec/resolve.md` §3.5).
    NonExportedSymbol { name: String },
    /// `required-symbol`: a `--require-defined` symbol is not defined.
    RequiredSymbol { name: String },
    /// `tls-mismatch`: a TLS and a non-TLS mention of one key
    /// (`spec/resolve.md` §3.4).
    TlsMismatch { name: String },
    /// `undefined-symbol`: a key with surviving references and no
    /// definition, one diagnostic per key (`spec/resolve.md` §5.3).
    /// `name` is the name as the symbol table has it (mangled).
    UndefinedSymbol { name: String },
    /// `undefined-version`: `name@version` names a version no node defines
    /// (`spec/resolve.md` §9.4).
    UndefinedVersion { name: String, version: String },
    /// `version-reassigned`: a version script assigns `name` to two
    /// versions; `first` is kept (`spec/resolve.md` §9.7).
    VersionReassigned { name: String, first: String, second: String },
    /// `versioned-export` (`spec/dyn.md` D-27): an exported definition
    /// `name@@version` of an executable, which would need a version
    /// definition (`.gnu.version_d`, M4).
    VersionedExport { name: String, version: String },

    // -- one-definition rule (M6, `spec/odr.md` §9.1) --------------------
    /// `meta-malformed` (`spec/meta.md` MT-14): a Mason metadata container
    /// that breaks its format; `reason` says how.
    MetaMalformed { reason: String },
    /// `meta-version` (`spec/meta.md` MT-35): a unit of a major version
    /// mortar does not read; its records are ignored.
    MetaVersion { major: u16 },
    /// `odr-abi-mismatch` (`spec/odr.md` O-30).
    OdrAbiMismatch { option: String, first: String, second: String },
    /// `odr-archive-shadow` (`spec/odr.md` §9.1).
    OdrArchiveShadow { name: String },
    /// `odr-declaration-mismatch` (`spec/odr.md` §9.1).
    OdrDeclarationMismatch { name: String },
    /// `odr-internal-reference` (`spec/odr.md` §9.1).
    OdrInternalReference { name: String },
    /// `odr-isa-mismatch` (`spec/odr.md` §9.1).
    OdrIsaMismatch { name: String },
    /// `odr-size-mismatch` (`spec/odr.md` §9.1).
    OdrSizeMismatch { name: String },
    /// `odr-strong-and-comdat` (`spec/odr.md` §9.1).
    OdrStrongAndComdat { name: String },
    /// `odr-symbol-type-mismatch` (`spec/odr.md` §9.1).
    OdrSymbolTypeMismatch { name: String },
    /// `odr-type-mismatch` (`spec/odr.md` §9.1).
    OdrTypeMismatch { name: String },
    /// `odr-violation` (`spec/odr.md` §9.1).
    OdrViolation { name: String },
    /// `odr-vtable-rtti-mismatch` (`spec/odr.md` §9.1).
    OdrVtableRttiMismatch { name: String },

    // -- relocations ----------------------------------------------------
    /// `canonical-plt-in-pie` (`spec/dyn.md` D-44): the relocation
    /// `r_type` takes the address of the shared function `name` in a way
    /// only a canonical PLT entry fills, which a `Pie` cannot have. GNU
    /// ld's words.
    CanonicalPltInPie { r_type: String, name: String },
    /// `copy-relocation-against-protected` (`spec/dyn.md` D-18): a `COPY`
    /// or canonical PLT demand on `name`, which `file` defines
    /// `STV_PROTECTED`. GNU ld's words.
    CopyRelocationAgainstProtected { name: String, file: String },
    /// `copy-relocation-of-zero-size` (`spec/dyn.md` D-42): a `COPY`
    /// demand on `name`, whose shared definition has `st_size` 0. lld's
    /// words.
    CopyRelocationOfZeroSize { name: String },
    /// `relocation-incompatible`: the relocation `r_type` (its ABI name)
    /// cannot be used against `name` in this output, for `reason`. `name`
    /// is the symbol-table name, or the section and offset of a section
    /// symbol's target (`.text+0x40`).
    RelocationIncompatible { r_type: String, name: String, reason: IncompatibleReason },
    /// `relocation-misaligned`: the value `value` of the relocation
    /// `r_type` is not a multiple of `align` bytes.
    RelocationMisaligned { r_type: String, value: i64, align: u64 },
    /// `relocation-out-of-range`: the value `value` of the relocation
    /// `r_type` is outside its field's check, `[min, max]`; `name` is the
    /// target as for [`Kind::RelocationIncompatible`].
    RelocationOutOfRange { r_type: String, value: i64, min: i64, max: i64, name: String },
    /// `text-relocation` (`spec/dyn.md` D-47): the relocation `r_type`
    /// against `name` would need a dynamic record in a section without
    /// `SHF_WRITE`. GNU ld's words.
    TextRelocation { r_type: String, name: String },
    /// `weak-undefined-pc-in-pie` (`spec/dyn.md` D-25): in a `Pie`, the
    /// relocation `r_type` is an unfillable use of the undefined weak
    /// `name`; once per key. GNU ld's words.
    WeakUndefinedPcInPie { r_type: String, name: String },

    // -- layout ---------------------------------------------------------
    /// `executable-stack` (a warning, `spec/layout.md` L-84): `file` has a
    /// `.note.GNU-stack` with `SHF_EXECINSTR`, so the stack is made
    /// executable. GNU ld's words.
    ExecutableStack { file: String },
    /// `output-too-large` (`spec/layout.md` L-73): `what` (a chunk, the
    /// file) ends beyond `limit`, the end of the class's address space.
    OutputTooLarge { what: String, limit: String },
    /// `section-conflict` (`spec/layout.md` L-47, L-49, L-61): two live
    /// inputs of output section `name` cannot be combined; `first` and
    /// `second` are class words (`TLS`, `non-TLS`, `allocated`,
    /// `non-allocated`) or `SHT_` names.
    SectionConflict { name: String, first: String, second: String },

    // -- refusals -------------------------------------------------------
    /// `refused`: a construct mortar does not implement, by name
    /// (`docs/ARCHITECTURE.md`, principle 1). Always an error.
    /// `not_recognized` is set for a file format GNU ld does not
    /// recognize either: the message then carries GNU ld's phrase
    /// (`spec/diag.md` §8.3).
    Refused { class: RefusalClass, name: String, not_recognized: bool },

    // -- internal -------------------------------------------------------
    /// `internal-error`: mortar found its own data in a state its
    /// specification forbids: a table that violates its own validity rules
    /// (`spec/model.md` §4.4). A bug of mortar's, not of the input. `what`
    /// names the rule and what is wrong. Always an error.
    InternalError { what: String },
}

impl Kind {
    /// Every stable id, in the order of `spec/diag.md` §3.1.
    pub const IDS: [&'static str; 70] = [
        "conflicting-options",
        "invalid-option-value",
        "missing-argument",
        "no-input-files",
        "response-file-limit",
        "unbalanced-option",
        "unknown-option",
        "ambiguous-sysroot-path",
        "cannot-open-file",
        "cannot-open-output",
        "cannot-write-output",
        "duplicate-soname",
        "ignored-member",
        "incompatible-file",
        "input-changed",
        "library-not-found",
        "needed-not-found",
        "no-target",
        "script-cycle",
        "skipped-incompatible",
        "static-link-of-shared-object",
        "cet-report",
        "malformed-input",
        "unknown-file-type",
        "unknown-property",
        "archive-backref",
        "archive-choice",
        "comdat-mismatch",
        "common-alignment",
        "common-larger-than-definition",
        "common-overridden",
        "discarded-section-reference",
        "duplicate-symbol",
        "entry-not-found",
        "gnu-warning",
        "linker-symbol-redefined",
        "multiple-common",
        "non-exported-symbol",
        "required-symbol",
        "tls-mismatch",
        "undefined-symbol",
        "undefined-version",
        "version-reassigned",
        "versioned-export",
        "meta-malformed",
        "meta-version",
        "odr-abi-mismatch",
        "odr-archive-shadow",
        "odr-declaration-mismatch",
        "odr-internal-reference",
        "odr-isa-mismatch",
        "odr-size-mismatch",
        "odr-strong-and-comdat",
        "odr-symbol-type-mismatch",
        "odr-type-mismatch",
        "odr-violation",
        "odr-vtable-rtti-mismatch",
        "canonical-plt-in-pie",
        "copy-relocation-against-protected",
        "copy-relocation-of-zero-size",
        "relocation-incompatible",
        "relocation-misaligned",
        "relocation-out-of-range",
        "text-relocation",
        "weak-undefined-pc-in-pie",
        "executable-stack",
        "output-too-large",
        "section-conflict",
        "refused",
        "internal-error",
    ];

    /// The stable kebab-case id of the kind. Tests assert on it; the text
    /// form does not print it.
    #[must_use]
    pub const fn id(&self) -> &'static str {
        match self {
            Self::ConflictingOptions { .. } => "conflicting-options",
            Self::InvalidOptionValue { .. } => "invalid-option-value",
            Self::MissingArgument { .. } => "missing-argument",
            Self::NoInputFiles => "no-input-files",
            Self::ResponseFileLimit { .. } => "response-file-limit",
            Self::UnbalancedOption { .. } => "unbalanced-option",
            Self::UnknownOption { .. } => "unknown-option",
            Self::AmbiguousSysrootPath { .. } => "ambiguous-sysroot-path",
            Self::CannotOpenFile { .. } => "cannot-open-file",
            Self::CannotOpenOutput { .. } => "cannot-open-output",
            Self::CannotWriteOutput { .. } => "cannot-write-output",
            Self::DuplicateSoname { .. } => "duplicate-soname",
            Self::IgnoredMember { .. } => "ignored-member",
            Self::IncompatibleFile { .. } => "incompatible-file",
            Self::InputChanged => "input-changed",
            Self::LibraryNotFound { .. } => "library-not-found",
            Self::NeededNotFound { .. } => "needed-not-found",
            Self::NoTarget => "no-target",
            Self::ScriptCycle => "script-cycle",
            Self::SkippedIncompatible { .. } => "skipped-incompatible",
            Self::StaticLinkOfSharedObject { .. } => "static-link-of-shared-object",
            Self::CetReport { .. } => "cet-report",
            Self::Malformed { .. } => "malformed-input",
            Self::UnknownFileType => "unknown-file-type",
            Self::UnknownProperty { .. } => "unknown-property",
            Self::ArchiveBackref { .. } => "archive-backref",
            Self::ArchiveChoice { .. } => "archive-choice",
            Self::ComdatMismatch { .. } => "comdat-mismatch",
            Self::CommonAlignment { .. } => "common-alignment",
            Self::CommonLargerThanDefinition { .. } => "common-larger-than-definition",
            Self::CommonOverridden { .. } => "common-overridden",
            Self::DiscardedSectionReference { .. } => "discarded-section-reference",
            Self::DuplicateSymbol { .. } => "duplicate-symbol",
            Self::EntryNotFound { .. } => "entry-not-found",
            Self::GnuWarning { .. } => "gnu-warning",
            Self::LinkerSymbolRedefined { .. } => "linker-symbol-redefined",
            Self::MultipleCommon { .. } => "multiple-common",
            Self::NonExportedSymbol { .. } => "non-exported-symbol",
            Self::RequiredSymbol { .. } => "required-symbol",
            Self::TlsMismatch { .. } => "tls-mismatch",
            Self::UndefinedSymbol { .. } => "undefined-symbol",
            Self::UndefinedVersion { .. } => "undefined-version",
            Self::VersionReassigned { .. } => "version-reassigned",
            Self::VersionedExport { .. } => "versioned-export",
            Self::MetaMalformed { .. } => "meta-malformed",
            Self::MetaVersion { .. } => "meta-version",
            Self::OdrAbiMismatch { .. } => "odr-abi-mismatch",
            Self::OdrArchiveShadow { .. } => "odr-archive-shadow",
            Self::OdrDeclarationMismatch { .. } => "odr-declaration-mismatch",
            Self::OdrInternalReference { .. } => "odr-internal-reference",
            Self::OdrIsaMismatch { .. } => "odr-isa-mismatch",
            Self::OdrSizeMismatch { .. } => "odr-size-mismatch",
            Self::OdrStrongAndComdat { .. } => "odr-strong-and-comdat",
            Self::OdrSymbolTypeMismatch { .. } => "odr-symbol-type-mismatch",
            Self::OdrTypeMismatch { .. } => "odr-type-mismatch",
            Self::OdrViolation { .. } => "odr-violation",
            Self::OdrVtableRttiMismatch { .. } => "odr-vtable-rtti-mismatch",
            Self::CanonicalPltInPie { .. } => "canonical-plt-in-pie",
            Self::CopyRelocationAgainstProtected { .. } => "copy-relocation-against-protected",
            Self::CopyRelocationOfZeroSize { .. } => "copy-relocation-of-zero-size",
            Self::RelocationIncompatible { .. } => "relocation-incompatible",
            Self::RelocationMisaligned { .. } => "relocation-misaligned",
            Self::RelocationOutOfRange { .. } => "relocation-out-of-range",
            Self::TextRelocation { .. } => "text-relocation",
            Self::WeakUndefinedPcInPie { .. } => "weak-undefined-pc-in-pie",
            Self::ExecutableStack { .. } => "executable-stack",
            Self::OutputTooLarge { .. } => "output-too-large",
            Self::SectionConflict { .. } => "section-conflict",
            Self::Refused { .. } => "refused",
            Self::InternalError { .. } => "internal-error",
        }
    }

    /// The symbol the diagnostic is about, as the symbol table has it:
    /// the name the demangling hook is asked about (`spec/diag.md` §5.4).
    #[must_use]
    pub fn symbol(&self) -> Option<&str> {
        match self {
            Self::ArchiveBackref { name }
            | Self::ArchiveChoice { name }
            | Self::ComdatMismatch { name, .. }
            | Self::CommonAlignment { name }
            | Self::CommonLargerThanDefinition { name }
            | Self::CommonOverridden { name }
            | Self::DiscardedSectionReference { name }
            | Self::DuplicateSymbol { name }
            | Self::EntryNotFound { name }
            | Self::LinkerSymbolRedefined { name }
            | Self::MultipleCommon { name }
            | Self::NonExportedSymbol { name }
            | Self::RequiredSymbol { name }
            | Self::TlsMismatch { name }
            | Self::UndefinedSymbol { name }
            | Self::UndefinedVersion { name, .. }
            | Self::VersionReassigned { name, .. }
            | Self::VersionedExport { name, .. }
            | Self::OdrArchiveShadow { name }
            | Self::OdrDeclarationMismatch { name }
            | Self::OdrInternalReference { name }
            | Self::OdrIsaMismatch { name }
            | Self::OdrSizeMismatch { name }
            | Self::OdrStrongAndComdat { name }
            | Self::OdrSymbolTypeMismatch { name }
            | Self::OdrTypeMismatch { name }
            | Self::OdrViolation { name }
            | Self::OdrVtableRttiMismatch { name }
            | Self::CanonicalPltInPie { name, .. }
            | Self::CopyRelocationAgainstProtected { name, .. }
            | Self::CopyRelocationOfZeroSize { name }
            | Self::RelocationIncompatible { name, .. }
            | Self::RelocationOutOfRange { name, .. }
            | Self::TextRelocation { name, .. }
            | Self::WeakUndefinedPcInPie { name, .. } => Some(name),
            _ => None,
        }
    }
}

/// A file as mortar names it: a path, or a member of an archive
/// (`spec/diag.md` §2.3).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileRef {
    path: String,
    member: Option<String>,
}

impl FileRef {
    /// A file by the path the user or a script gave.
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into(), member: None }
    }

    /// A member of an archive: `libx.a(y.o)`.
    pub fn in_archive(archive: impl Into<String>, member: impl Into<String>) -> Self {
        Self { path: archive.into(), member: Some(member.into()) }
    }

    /// A file by a [`Path`]. Bytes that are not UTF-8 become U+FFFD.
    #[must_use]
    pub fn from_path(path: &Path) -> Self {
        Self::new(path.to_string_lossy())
    }

    /// The path of the file, or of the archive for a member.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The member's name, for a member of an archive.
    #[must_use]
    pub fn member(&self) -> Option<&str> {
        self.member.as_deref()
    }
}

/// Where something is (`spec/diag.md` §2.3). The renderer formats each form
/// the same way wherever it appears.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Place {
    /// A file: `foo.o`, `libx.a(y.o)`.
    File(FileRef),
    /// A byte of a section of a file: `foo.o:(.text+0x10)`.
    Section { file: FileRef, section: String, offset: u64 },
    /// A symbol of a file: `foo.o:(main)`. The name is the symbol table's
    /// (mangled); the renderer asks the demangling hook.
    Symbol { file: FileRef, name: String },
    /// A command-line argument, by its text: `argument '--entry=main'`.
    Argument(String),
    /// A position in a linker script, version script or dynamic list:
    /// `libc.so:5:9`. Lines and columns start at 1; a column counts bytes.
    Script { path: String, line: u32, column: u32 },
}

impl Place {
    /// A file by path.
    pub fn file(path: impl Into<String>) -> Self {
        Self::File(FileRef::new(path))
    }

    /// A byte of a section of a file.
    pub fn section(file: FileRef, section: impl Into<String>, offset: u64) -> Self {
        Self::Section { file, section: section.into(), offset }
    }

    /// A symbol of a file, by its symbol-table name.
    pub fn symbol(file: FileRef, name: impl Into<String>) -> Self {
        Self::Symbol { file, name: name.into() }
    }

    /// A command-line argument, by its text.
    pub fn argument(text: impl Into<String>) -> Self {
        Self::Argument(text.into())
    }

    /// A position in a script.
    pub fn script(path: impl Into<String>, line: u32, column: u32) -> Self {
        Self::Script { path: path.into(), line, column }
    }
}

impl From<FileRef> for Place {
    fn from(file: FileRef) -> Self {
        Self::File(file)
    }
}

/// How a context line's place relates to the diagnostic
/// (`spec/diag.md` §2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Relation {
    /// `defined in`: where a definition of the symbol is.
    DefinedIn,
    /// `in`: what contains the construct, when the head line has no place.
    In,
    /// `named by`: the argument or script position that brought a file in.
    NamedBy,
    /// `referenced by`: where a reference to the symbol is.
    ReferencedBy,
}

impl Relation {
    /// Every relation, in the order of `spec/diag.md` §2.4.
    pub const ALL: [Self; 4] = [Self::DefinedIn, Self::In, Self::NamedBy, Self::ReferencedBy];

    /// The words printed between `>>> ` and the place.
    #[must_use]
    pub const fn phrase(self) -> &'static str {
        match self {
            Self::DefinedIn => "defined in",
            Self::In => "in",
            Self::NamedBy => "named by",
            Self::ReferencedBy => "referenced by",
        }
    }
}

/// One `>>> ` line under the head line (`spec/diag.md` §2.4).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Context {
    /// A place and how it relates: `>>> referenced by foo.o:(.text+0x10)`.
    At(Relation, Place),
    /// A fact that is not a place: `>>> expected one of: sysv, gnu, both`.
    Note(String),
}

/// One diagnostic: a severity, a kind, an optional place for the head line
/// and context lines in the order given (`spec/diag.md` §2).
///
/// Built by value on the path that found the problem and handed to a
/// [`crate::Collector`]:
///
/// ```
/// use mortar_diag::{Diagnostic, FileRef, Kind, Place};
///
/// let file = FileRef::in_archive("libx.a", "y.o");
/// let diag = Diagnostic::error(Kind::UndefinedSymbol { name: "missing".to_owned() })
///     .at(Place::section(file, ".text", 0x10));
/// assert_eq!(diag.id(), "undefined-symbol");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Diagnostic {
    severity: Severity,
    kind: Kind,
    place: Option<Place>,
    context: Vec<Context>,
}

impl Diagnostic {
    /// A diagnostic of the given severity. A [`Kind::Refused`] is an error
    /// whatever is asked for: a refusal that lets the link go on is the
    /// silent acceptance principle 1 forbids. So is a
    /// [`Kind::InternalError`]: output from data mortar knows to be wrong
    /// is never written.
    #[must_use]
    pub fn new(severity: Severity, kind: Kind) -> Self {
        let always_error = matches!(kind, Kind::Refused { .. } | Kind::InternalError { .. });
        let severity = if always_error { Severity::Error } else { severity };
        Self { severity, kind, place: None, context: Vec::new() }
    }

    /// An error: the link fails.
    #[must_use]
    pub fn error(kind: Kind) -> Self {
        Self::new(Severity::Error, kind)
    }

    /// A warning: the link goes on.
    #[must_use]
    pub fn warning(kind: Kind) -> Self {
        Self::new(Severity::Warning, kind)
    }

    /// The refusal of a construct by name (`spec/diag.md` §4). `name` is
    /// the construct's spelling in the document that defines it
    /// (`R_AARCH64_TLSDESC_CALL`, `--plugin`, `SHT_GNU_verdef`), or its
    /// number in hexadecimal when it has none (`0x999`).
    #[must_use]
    pub fn refused(class: RefusalClass, name: impl Into<String>) -> Self {
        Self::error(Kind::Refused { class, name: name.into(), not_recognized: false })
    }

    /// The refusal of a file format that GNU ld does not recognize either
    /// (`spec/diag.md` §8.3): class `file-format`, and the message ends in
    /// GNU ld's `file not recognized: file format not recognized`.
    #[must_use]
    pub fn refused_unrecognized_format(name: impl Into<String>) -> Self {
        Self::error(Kind::Refused {
            class: RefusalClass::FileFormat,
            name: name.into(),
            not_recognized: true,
        })
    }

    /// Sets the place the head line starts with.
    #[must_use]
    pub fn at(mut self, place: impl Into<Place>) -> Self {
        self.place = Some(place.into());
        self
    }

    /// Adds a context line: a place and how it relates.
    #[must_use]
    pub fn with(mut self, relation: Relation, place: impl Into<Place>) -> Self {
        self.context.push(Context::At(relation, place.into()));
        self
    }

    /// Adds a context line that states a fact.
    #[must_use]
    pub fn note(mut self, text: impl Into<String>) -> Self {
        self.context.push(Context::Note(text.into()));
        self
    }

    /// The severity. A warning a [`crate::Collector`] promoted
    /// (`--fatal-warnings`) reads as an error in its [`crate::Report`].
    #[must_use]
    pub fn severity(&self) -> Severity {
        self.severity
    }

    /// What the diagnostic says.
    #[must_use]
    pub fn kind(&self) -> &Kind {
        &self.kind
    }

    /// The stable id of the kind.
    #[must_use]
    pub fn id(&self) -> &'static str {
        self.kind.id()
    }

    /// The class and name of the refused construct, for a refusal.
    #[must_use]
    pub fn refusal(&self) -> Option<(RefusalClass, &str)> {
        match &self.kind {
            Kind::Refused { class, name, .. } => Some((*class, name)),
            _ => None,
        }
    }

    /// The place the head line starts with.
    #[must_use]
    pub fn place(&self) -> Option<&Place> {
        self.place.as_ref()
    }

    /// The context lines, in the order they were added.
    #[must_use]
    pub fn context(&self) -> &[Context] {
        &self.context
    }

    /// Makes a warning an error (`--fatal-warnings`, `spec/diag.md` §6.3).
    pub(crate) fn promote(&mut self) {
        self.severity = Severity::Error;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The position of a kind in [`Kind::IDS`]. The match is exhaustive on
    /// purpose: a variant added without its id in the list does not
    /// compile here.
    fn index(kind: &Kind) -> usize {
        match kind {
            Kind::ConflictingOptions { .. } => 0,
            Kind::InvalidOptionValue { .. } => 1,
            Kind::MissingArgument { .. } => 2,
            Kind::NoInputFiles => 3,
            Kind::ResponseFileLimit { .. } => 4,
            Kind::UnbalancedOption { .. } => 5,
            Kind::UnknownOption { .. } => 6,
            Kind::AmbiguousSysrootPath { .. } => 7,
            Kind::CannotOpenFile { .. } => 8,
            Kind::CannotOpenOutput { .. } => 9,
            Kind::CannotWriteOutput { .. } => 10,
            Kind::DuplicateSoname { .. } => 11,
            Kind::IgnoredMember { .. } => 12,
            Kind::IncompatibleFile { .. } => 13,
            Kind::InputChanged => 14,
            Kind::LibraryNotFound { .. } => 15,
            Kind::NeededNotFound { .. } => 16,
            Kind::NoTarget => 17,
            Kind::ScriptCycle => 18,
            Kind::SkippedIncompatible { .. } => 19,
            Kind::StaticLinkOfSharedObject { .. } => 20,
            Kind::CetReport { .. } => 21,
            Kind::Malformed { .. } => 22,
            Kind::UnknownFileType => 23,
            Kind::UnknownProperty { .. } => 24,
            Kind::ArchiveBackref { .. } => 25,
            Kind::ArchiveChoice { .. } => 26,
            Kind::ComdatMismatch { .. } => 27,
            Kind::CommonAlignment { .. } => 28,
            Kind::CommonLargerThanDefinition { .. } => 29,
            Kind::CommonOverridden { .. } => 30,
            Kind::DiscardedSectionReference { .. } => 31,
            Kind::DuplicateSymbol { .. } => 32,
            Kind::EntryNotFound { .. } => 33,
            Kind::GnuWarning { .. } => 34,
            Kind::LinkerSymbolRedefined { .. } => 35,
            Kind::MultipleCommon { .. } => 36,
            Kind::NonExportedSymbol { .. } => 37,
            Kind::RequiredSymbol { .. } => 38,
            Kind::TlsMismatch { .. } => 39,
            Kind::UndefinedSymbol { .. } => 40,
            Kind::UndefinedVersion { .. } => 41,
            Kind::VersionReassigned { .. } => 42,
            Kind::VersionedExport { .. } => 43,
            Kind::MetaMalformed { .. } => 44,
            Kind::MetaVersion { .. } => 45,
            Kind::OdrAbiMismatch { .. } => 46,
            Kind::OdrArchiveShadow { .. } => 47,
            Kind::OdrDeclarationMismatch { .. } => 48,
            Kind::OdrInternalReference { .. } => 49,
            Kind::OdrIsaMismatch { .. } => 50,
            Kind::OdrSizeMismatch { .. } => 51,
            Kind::OdrStrongAndComdat { .. } => 52,
            Kind::OdrSymbolTypeMismatch { .. } => 53,
            Kind::OdrTypeMismatch { .. } => 54,
            Kind::OdrViolation { .. } => 55,
            Kind::OdrVtableRttiMismatch { .. } => 56,
            Kind::CanonicalPltInPie { .. } => 57,
            Kind::CopyRelocationAgainstProtected { .. } => 58,
            Kind::CopyRelocationOfZeroSize { .. } => 59,
            Kind::RelocationIncompatible { .. } => 60,
            Kind::RelocationMisaligned { .. } => 61,
            Kind::RelocationOutOfRange { .. } => 62,
            Kind::TextRelocation { .. } => 63,
            Kind::WeakUndefinedPcInPie { .. } => 64,
            Kind::ExecutableStack { .. } => 65,
            Kind::OutputTooLarge { .. } => 66,
            Kind::SectionConflict { .. } => 67,
            Kind::Refused { .. } => 68,
            Kind::InternalError { .. } => 69,
        }
    }

    // spec: diag.md §2.2, §3.1 (every kind has its id, and the list has
    // every kind)
    #[test]
    fn every_variant_is_in_ids() {
        let s = String::new;
        let all = [
            Kind::ConflictingOptions { first: s(), second: s() },
            Kind::InvalidOptionValue { option: s(), value: s() },
            Kind::MissingArgument { option: s() },
            Kind::NoInputFiles,
            Kind::ResponseFileLimit { limit: 0 },
            Kind::UnbalancedOption { option: s(), counterpart: s() },
            Kind::UnknownOption { option: s() },
            Kind::AmbiguousSysrootPath { spelling: s() },
            Kind::CannotOpenFile { path: s(), reason: s() },
            Kind::CannotOpenOutput { path: s(), reason: s() },
            Kind::CannotWriteOutput { path: s(), reason: s() },
            Kind::DuplicateSoname { soname: s() },
            Kind::IgnoredMember { what: s() },
            Kind::IncompatibleFile { expected: s(), found: s() },
            Kind::InputChanged,
            Kind::LibraryNotFound { name: s() },
            Kind::NeededNotFound { name: s(), needed_by: s() },
            Kind::NoTarget,
            Kind::ScriptCycle,
            Kind::SkippedIncompatible { path: s(), search: s() },
            Kind::StaticLinkOfSharedObject { path: s() },
            Kind::CetReport { path: s(), property: s() },
            Kind::Malformed { what: s() },
            Kind::UnknownFileType,
            Kind::UnknownProperty { path: s(), pr_type: 0 },
            Kind::ArchiveBackref { name: s() },
            Kind::ArchiveChoice { name: s() },
            Kind::ComdatMismatch { name: s(), signature: s() },
            Kind::CommonAlignment { name: s() },
            Kind::CommonLargerThanDefinition { name: s() },
            Kind::CommonOverridden { name: s() },
            Kind::DiscardedSectionReference { name: s() },
            Kind::DuplicateSymbol { name: s() },
            Kind::EntryNotFound { name: s() },
            Kind::GnuWarning { text: s() },
            Kind::LinkerSymbolRedefined { name: s() },
            Kind::MultipleCommon { name: s() },
            Kind::NonExportedSymbol { name: s() },
            Kind::RequiredSymbol { name: s() },
            Kind::TlsMismatch { name: s() },
            Kind::UndefinedSymbol { name: s() },
            Kind::UndefinedVersion { name: s(), version: s() },
            Kind::VersionReassigned { name: s(), first: s(), second: s() },
            Kind::VersionedExport { name: s(), version: s() },
            Kind::MetaMalformed { reason: s() },
            Kind::MetaVersion { major: 2 },
            Kind::OdrAbiMismatch { option: s(), first: s(), second: s() },
            Kind::OdrArchiveShadow { name: s() },
            Kind::OdrDeclarationMismatch { name: s() },
            Kind::OdrInternalReference { name: s() },
            Kind::OdrIsaMismatch { name: s() },
            Kind::OdrSizeMismatch { name: s() },
            Kind::OdrStrongAndComdat { name: s() },
            Kind::OdrSymbolTypeMismatch { name: s() },
            Kind::OdrTypeMismatch { name: s() },
            Kind::OdrViolation { name: s() },
            Kind::OdrVtableRttiMismatch { name: s() },
            Kind::CanonicalPltInPie { r_type: s(), name: s() },
            Kind::CopyRelocationAgainstProtected { name: s(), file: s() },
            Kind::CopyRelocationOfZeroSize { name: s() },
            Kind::RelocationIncompatible {
                r_type: s(),
                name: s(),
                reason: IncompatibleReason::NonAlloc,
            },
            Kind::RelocationMisaligned { r_type: s(), value: 0, align: 1 },
            Kind::RelocationOutOfRange { r_type: s(), value: 0, min: 0, max: 0, name: s() },
            Kind::TextRelocation { r_type: s(), name: s() },
            Kind::WeakUndefinedPcInPie { r_type: s(), name: s() },
            Kind::ExecutableStack { file: s() },
            Kind::OutputTooLarge { what: s(), limit: s() },
            Kind::SectionConflict { name: s(), first: s(), second: s() },
            Kind::Refused { class: RefusalClass::Feature, name: s(), not_recognized: false },
            Kind::InternalError { what: s() },
        ];
        assert_eq!(all.len(), Kind::IDS.len());
        for (position, kind) in all.iter().enumerate() {
            assert_eq!(index(kind), position);
            assert_eq!(kind.id(), Kind::IDS[position]);
        }
    }
}
