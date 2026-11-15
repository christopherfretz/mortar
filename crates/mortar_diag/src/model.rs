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
    /// `cannot-open-file`: an input could not be opened or its bytes could
    /// not be read. `reason` is the system's ([`crate::os_reason`]).
    CannotOpenFile { path: String, reason: String },
    /// `cannot-open-output`: the output file could not be created.
    CannotOpenOutput { path: String, reason: String },
    /// `library-not-found`: no file for `-l<name>` on the search path.
    /// `name` is the text after `-l`.
    LibraryNotFound { name: String },

    // -- file formats ---------------------------------------------------
    /// `malformed-input`: a reader rejected the bytes of a file. `what` is
    /// the reader's statement of what is wrong.
    Malformed { what: String },
    /// `unknown-file-type`: the file is no format mortar can name.
    UnknownFileType,

    // -- symbols --------------------------------------------------------
    /// `duplicate-symbol`: two definitions that may not coexist. `name` is
    /// the name as the symbol table has it (mangled).
    DuplicateSymbol { name: String },
    /// `undefined-symbol`: a reference nothing defines. `name` is the name
    /// as the symbol table has it (mangled).
    UndefinedSymbol { name: String },

    // -- refusals -------------------------------------------------------
    /// `refused`: a construct mortar does not implement, by name
    /// (`docs/ARCHITECTURE.md`, principle 1). Always an error.
    Refused { class: RefusalClass, name: String },
}

impl Kind {
    /// Every stable id, in the order of `spec/diag.md` §3.1.
    pub const IDS: [&'static str; 15] = [
        "conflicting-options",
        "invalid-option-value",
        "missing-argument",
        "no-input-files",
        "response-file-limit",
        "unbalanced-option",
        "unknown-option",
        "cannot-open-file",
        "cannot-open-output",
        "library-not-found",
        "malformed-input",
        "unknown-file-type",
        "duplicate-symbol",
        "undefined-symbol",
        "refused",
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
            Self::CannotOpenFile { .. } => "cannot-open-file",
            Self::CannotOpenOutput { .. } => "cannot-open-output",
            Self::LibraryNotFound { .. } => "library-not-found",
            Self::Malformed { .. } => "malformed-input",
            Self::UnknownFileType => "unknown-file-type",
            Self::DuplicateSymbol { .. } => "duplicate-symbol",
            Self::UndefinedSymbol { .. } => "undefined-symbol",
            Self::Refused { .. } => "refused",
        }
    }

    /// The symbol the diagnostic is about, as the symbol table has it:
    /// the name the demangling hook is asked about (`spec/diag.md` §5.4).
    #[must_use]
    pub fn symbol(&self) -> Option<&str> {
        match self {
            Self::DuplicateSymbol { name } | Self::UndefinedSymbol { name } => Some(name),
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
/// use mortar_diag::{Diagnostic, FileRef, Kind, Place, Relation};
///
/// let file = FileRef::in_archive("libx.a", "y.o");
/// let diag = Diagnostic::error(Kind::UndefinedSymbol { name: "missing".to_owned() })
///     .with(Relation::ReferencedBy, Place::section(file, ".text", 0x10));
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
    /// silent acceptance principle 1 forbids.
    #[must_use]
    pub fn new(severity: Severity, kind: Kind) -> Self {
        let severity =
            if matches!(kind, Kind::Refused { .. }) { Severity::Error } else { severity };
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
        Self::error(Kind::Refused { class, name: name.into() })
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
            Kind::Refused { class, name } => Some((*class, name)),
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
            Kind::CannotOpenFile { .. } => 7,
            Kind::CannotOpenOutput { .. } => 8,
            Kind::LibraryNotFound { .. } => 9,
            Kind::Malformed { .. } => 10,
            Kind::UnknownFileType => 11,
            Kind::DuplicateSymbol { .. } => 12,
            Kind::UndefinedSymbol { .. } => 13,
            Kind::Refused { .. } => 14,
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
            Kind::CannotOpenFile { path: s(), reason: s() },
            Kind::CannotOpenOutput { path: s(), reason: s() },
            Kind::LibraryNotFound { name: s() },
            Kind::Malformed { what: s() },
            Kind::UnknownFileType,
            Kind::DuplicateSymbol { name: s() },
            Kind::UndefinedSymbol { name: s() },
            Kind::Refused { class: RefusalClass::Feature, name: s() },
        ];
        assert_eq!(all.len(), Kind::IDS.len());
        for (position, kind) in all.iter().enumerate() {
            assert_eq!(index(kind), position);
            assert_eq!(kind.id(), Kind::IDS[position]);
        }
    }
}
