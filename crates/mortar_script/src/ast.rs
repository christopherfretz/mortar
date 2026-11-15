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
