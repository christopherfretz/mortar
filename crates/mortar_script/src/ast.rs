//! The checked AST of stage 1 (`spec/script.md` §2): plain data, every
//! node with the byte range it was read from.

/// A byte range `start..end` of the script's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    /// `start..end`.
    #[must_use]
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// The bytes of `source` the span covers (empty if it is out of range).
    #[must_use]
    pub fn of<'a>(&self, source: &'a [u8]) -> &'a [u8] {
        source.get(self.start..self.end).unwrap_or_default()
    }
}

/// A name, path or format name as the script spells it (§2.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Word {
    /// The bytes: an unquoted name as written, a quoted string without its
    /// quotes. Never empty. Not necessarily UTF-8 (a quoted string may hold
    /// any byte but `"` and NUL).
    pub text: Vec<u8>,
    /// Whether it was quoted.
    pub quoted: bool,
    /// The token, quotes included.
    pub span: Span,
}

impl Word {
    /// The text, if it is UTF-8.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.text).ok()
    }

    /// The text, bytes that are not UTF-8 replaced by U+FFFD.
    #[must_use]
    pub fn to_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.text).into_owned()
    }
}

/// A parsed script: its commands in order (§2). Empty statements (`;`)
/// are not kept.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Script {
    pub commands: Vec<Command>,
}

/// One command and the bytes it was read from, keyword to closing
/// parenthesis.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Command {
    pub kind: CommandKind,
    pub span: Span,
}

/// The commands of stage 1 (§2.2 to §2.9).
///
/// Deliberately not `#[non_exhaustive]`: a consumer matches every command
/// and must refuse, by name, what it does not do; a command added by a
/// later stage then breaks the build of every consumer instead of being
/// skipped by a `_` arm.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CommandKind {
    /// `INPUT ( list )` (§2.3).
    Input(Vec<InputItem>),
    /// `GROUP ( list )` (§2.3).
    Group(Vec<InputItem>),
    /// `OUTPUT_FORMAT ( default )` or `OUTPUT_FORMAT ( default , big ,
    /// little )` (§2.5).
    OutputFormat { default: Word, big: Option<Word>, little: Option<Word> },
    /// `OUTPUT_ARCH ( name )` (§2.5).
    OutputArch(Word),
    /// `TARGET ( bfdname )` (§2.5).
    Target(Word),
    /// `SEARCH_DIR ( path )` (§2.6).
    SearchDir(Word),
    /// `OUTPUT ( file )` (§2.7).
    Output(Word),
    /// `ENTRY ( symbol )` (§2.7).
    Entry(Word),
    /// `EXTERN ( symbol ... )`: one or more (§2.7).
    Extern(Vec<Word>),
    /// `STARTUP ( file )` (§2.3).
    Startup(Word),
    /// `INCLUDE file` (§2.8): not expanded here.
    Include(Word),
}

impl CommandKind {
    /// The command's keyword, as the manual spells it.
    #[must_use]
    pub const fn keyword(&self) -> &'static str {
        match self {
            Self::Input(_) => "INPUT",
            Self::Group(_) => "GROUP",
            Self::OutputFormat { .. } => "OUTPUT_FORMAT",
            Self::OutputArch(_) => "OUTPUT_ARCH",
            Self::Target(_) => "TARGET",
            Self::SearchDir(_) => "SEARCH_DIR",
            Self::Output(_) => "OUTPUT",
            Self::Entry(_) => "ENTRY",
            Self::Extern(_) => "EXTERN",
            Self::Startup(_) => "STARTUP",
            Self::Include(_) => "INCLUDE",
        }
    }
}

/// An entry of an `INPUT`, `GROUP` or `AS_NEEDED` list (§2.3).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InputItem {
    /// A file or library.
    File(InputFile),
    /// `AS_NEEDED ( list )`; `span` is keyword to closing parenthesis.
    AsNeeded { items: Vec<InputItem>, span: Span },
}

/// A file of an input list as spelled (§2.4). Resolving it to a file is
/// `mortar_input`'s, by the rules of §2.4.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InputFile {
    /// What the spelling says to look for.
    pub form: FileForm,
    /// The name: for [`FileForm::Path`] the whole spelling, for
    /// [`FileForm::Sysroot`] the spelling *after* the `=` or `$SYSROOT`
    /// prefix, for the library forms the text after `-l` or `-l:`.
    pub name: Vec<u8>,
    /// The token as written: quoted or not, its span.
    pub spelling: Word,
}

/// How an input-list name is looked up (§2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileForm {
    /// A path: absolute, or relative to the script's directory, the
    /// current directory, then the library search path.
    Path,
    /// `=path` or `$SYSROOT/path`: inside the sysroot.
    Sysroot,
    /// `-lname`: as `-l name` on the command line.
    Library,
    /// `-l:file`: as `-l :file` on the command line.
    LibraryVerbatim,
}

// ---- version scripts and dynamic lists (`spec/script.md` §9, M4) -------
//
// The AST of §9.1 (WP-116); the parser that builds it is WP-125's
// (`version.rs`). Plain data: what an entry means (exact name or glob,
// its escapes) is `spec/resolve.md` §9.7 "Entries", decided from the raw
// text kept here, never by the parser.

/// A version script (§9.1 `version-script`): every `--version-script`
/// file of the line read as one script, in order (or the body of a
/// `VERSION` command). Either named nodes, or one anonymous node alone.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct VersionScript {
    /// The nodes in the order the files and the text give them.
    pub nodes: Vec<VersionNode>,
}

impl VersionScript {
    /// Whether the script is one anonymous node (`{ global: a; local: *;
    /// };`): its keys are exported unversioned (`spec/dyn.md` D-114).
    #[must_use]
    pub fn is_anonymous(&self) -> bool {
        matches!(self.nodes.as_slice(), [node] if node.name.is_none())
    }
}

/// One node: `NAME { body } parents ;`, or the anonymous `{ body } ;`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VersionNode {
    /// The version name; `None` for the anonymous node.
    pub name: Option<Word>,
    /// The parent nodes, in the order written (GNU ld accepts several,
    /// §9.1), each an earlier node of the script.
    pub parents: Vec<VersionParent>,
    /// The entries of the body, in order, each with its scope.
    pub entries: Vec<VersionEntry>,
    /// Which file of the line the node is in: the index among the
    /// `--version-script` files, in command-line order (0 for a `VERSION`
    /// command). Spans are offsets into that file.
    pub file: u32,
    /// The node, name (or `{`) to its `;`.
    pub span: Span,
}

/// A parent named after a node's closing `}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VersionParent {
    /// The name as written.
    pub name: Word,
    /// The index in [`VersionScript::nodes`] of the node it names, which
    /// comes earlier (a forward or missing parent is refused, §9.1).
    pub node: usize,
}

/// One entry of a node's body or of a dynamic list.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VersionEntry {
    /// `global:` or `local:` in force where it stands (`Global` before the
    /// first label, §9.1). Always `Global` in a dynamic list.
    pub scope: Scope,
    /// The language of the `extern` block it is in, if any.
    pub language: Language,
    /// The pattern: for an unquoted name the **raw text** as written,
    /// backslashes and glob bytes kept; for a quoted string the bytes
    /// between the quotes (`Word::quoted` set: always an exact name).
    pub pattern: Word,
}

/// The scope label an entry is under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    /// `global:` (or no label yet).
    Global,
    /// `local:`.
    Local,
}

/// The language of an entry (§9.1 `extern STRING { ... }`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Language {
    /// Outside any `extern` block: a symbol-table name.
    Plain,
    /// `extern "C"`: plain names, as [`Language::Plain`] (`spec/dyn.md`
    /// D-114).
    C,
    /// `extern "C++"`: matched against demangled names (`spec/resolve.md`
    /// §9.7 rule 6); refused by name until the demangler exists.
    Cxx,
}

/// A dynamic list (§9.1 `dynamic-list`): every `--dynamic-list` file of
/// the line read as one list, in order.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct DynamicList {
    /// The entries, in order; each `scope` is [`Scope::Global`].
    pub entries: Vec<VersionEntry>,
}
