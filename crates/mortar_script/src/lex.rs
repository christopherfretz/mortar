//! The lexer (`spec/script.md` §1).
//!
//! GNU ld's lexer is modal: whether `a-b`, `=x`, `,` or `ah` is one token,
//! several, or an error depends on the construct being read. The parser
//! says which mode each token is read in ([`Lexer::next`] takes the mode),
//! so a token never has to be re-lexed. Every token carries the byte range
//! it was read from; [`crate::position`] turns an offset into a line and a
//! column.
//!
//! Modes implemented: [`Mode::Script`], [`Mode::InputList`],
//! [`Mode::Expression`]. The modes of later stages (section-description
//! wildcards, version scripts) are specified in `spec/script.md` §1.2 and
//! are added here as variants when a stage needs them.

use crate::Span;

/// What the parser is reading, which decides how bytes become tokens
/// (`spec/script.md` §1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Statement level, and the arguments of `ENTRY`, `OUTPUT`,
    /// `SEARCH_DIR`, `OUTPUT_FORMAT`, `OUTPUT_ARCH`, `TARGET`, `STARTUP`
    /// and `INCLUDE` (GNU ld's SCRIPT state).
    Script,
    /// The lists of `INPUT`, `GROUP` and `AS_NEEDED` (GNU ld's INPUTLIST
    /// state).
    InputList,
    /// Expressions; in stage 1 only the list of `EXTERN` (GNU ld's
    /// EXPRESSION state).
    Expression,
}

/// An operator or separator (`spec/script.md` §1.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum Punct {
    LParen,
    RParen,
    LBrace,
    RBrace,
    Semicolon,
    Comma,
    Colon,
    Question,
    Bang,
    Tilde,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Amp,
    Pipe,
    Caret,
    Lt,
    Gt,
    Shl,
    Shr,
    Le,
    Ge,
    EqEq,
    Ne,
    AndAnd,
    OrOr,
    /// `=`
    Assign,
    PlusAssign,
    MinusAssign,
    StarAssign,
    SlashAssign,
    ShlAssign,
    ShrAssign,
    AmpAssign,
    PipeAssign,
    CaretAssign,
}

impl Punct {
    /// The spelling.
    #[must_use]
    pub const fn spelling(self) -> &'static str {
        match self {
            Self::LParen => "(",
            Self::RParen => ")",
            Self::LBrace => "{",
            Self::RBrace => "}",
            Self::Semicolon => ";",
            Self::Comma => ",",
            Self::Colon => ":",
            Self::Question => "?",
            Self::Bang => "!",
            Self::Tilde => "~",
            Self::Plus => "+",
            Self::Minus => "-",
            Self::Star => "*",
            Self::Slash => "/",
            Self::Percent => "%",
            Self::Amp => "&",
            Self::Pipe => "|",
            Self::Caret => "^",
            Self::Lt => "<",
            Self::Gt => ">",
            Self::Shl => "<<",
            Self::Shr => ">>",
            Self::Le => "<=",
            Self::Ge => ">=",
            Self::EqEq => "==",
            Self::Ne => "!=",
            Self::AndAnd => "&&",
            Self::OrOr => "||",
            Self::Assign => "=",
            Self::PlusAssign => "+=",
            Self::MinusAssign => "-=",
            Self::StarAssign => "*=",
            Self::SlashAssign => "/=",
            Self::ShlAssign => "<<=",
            Self::ShrAssign => ">>=",
            Self::AmpAssign => "&=",
            Self::PipeAssign => "|=",
            Self::CaretAssign => "^=",
        }
    }

    /// Whether this is one of the assignment operators of GNU ld's manual,
    /// "Simple Assignments".
    #[must_use]
    pub const fn is_assignment(self) -> bool {
        matches!(
            self,
            Self::Assign
                | Self::PlusAssign
                | Self::MinusAssign
                | Self::StarAssign
                | Self::SlashAssign
                | Self::ShlAssign
                | Self::ShrAssign
                | Self::AmpAssign
                | Self::PipeAssign
                | Self::CaretAssign
        )
    }
}

macro_rules! keywords {
    ($( $variant:ident = $spelling:literal [$script:literal, $expr:literal, $list:literal] ),* $(,)?) => {
        /// A keyword of GNU ld's script language (`spec/script.md` §1.7).
        /// Which keywords a mode recognizes is [`Keyword::in_mode`].
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[allow(missing_docs, non_camel_case_types)]
        pub enum Keyword { $( $variant ),* }

        impl Keyword {
            /// Every keyword, sorted by spelling (byte order).
            pub const ALL: &'static [Keyword] = &[ $( Keyword::$variant ),* ];

            /// The spelling (keywords are case-sensitive).
            #[must_use]
            pub const fn spelling(self) -> &'static str {
                match self { $( Self::$variant => $spelling ),* }
            }

            /// Whether `mode` reads this spelling as the keyword rather
            /// than as a name.
            #[must_use]
            pub const fn in_mode(self, mode: Mode) -> bool {
                match self { $( Self::$variant => match mode {
                    Mode::Script => $script,
                    Mode::Expression => $expr,
                    Mode::InputList => $list,
                } ),* }
            }
        }
    };
}

// [SCRIPT, EXPRESSION, INPUTLIST], from ld.bfd 2.44 (`spec/script.md` §1.7).
keywords! {
    ABSOLUTE = "ABSOLUTE" [true, true, false],
    ADDR = "ADDR" [true, true, false],
    AFTER = "AFTER" [true, false, false],
    ALIGN = "ALIGN" [true, true, false],
    ALIGNOF = "ALIGNOF" [true, true, false],
    ALIGN_WITH_INPUT = "ALIGN_WITH_INPUT" [true, true, false],
    ASSERT = "ASSERT" [true, true, false],
    AS_NEEDED = "AS_NEEDED" [false, false, true],
    AT = "AT" [true, true, false],
    BEFORE = "BEFORE" [true, false, false],
    BIND = "BIND" [true, true, false],
    BLOCK = "BLOCK" [true, true, false],
    CONSTANT = "CONSTANT" [true, true, false],
    COPY = "COPY" [false, true, false],
    DATA_SEGMENT_ALIGN = "DATA_SEGMENT_ALIGN" [true, true, false],
    DATA_SEGMENT_END = "DATA_SEGMENT_END" [true, true, false],
    DATA_SEGMENT_RELRO_END = "DATA_SEGMENT_RELRO_END" [true, true, false],
    DEFINED = "DEFINED" [true, true, false],
    DSECT = "DSECT" [false, true, false],
    ENTRY = "ENTRY" [true, false, false],
    EXTERN = "EXTERN" [true, false, false],
    FLOAT = "FLOAT" [true, false, false],
    FORCE_COMMON_ALLOCATION = "FORCE_COMMON_ALLOCATION" [true, false, false],
    FORCE_GROUP_ALLOCATION = "FORCE_GROUP_ALLOCATION" [true, false, false],
    GROUP = "GROUP" [true, false, false],
    HIDDEN = "HIDDEN" [true, true, false],
    HLL = "HLL" [true, false, false],
    INCLUDE = "INCLUDE" [true, true, false],
    INFO = "INFO" [false, true, false],
    INHIBIT_COMMON_ALLOCATION = "INHIBIT_COMMON_ALLOCATION" [true, false, false],
    INPUT = "INPUT" [true, false, false],
    INSERT = "INSERT" [true, false, false],
    LD_FEATURE = "LD_FEATURE" [true, false, false],
    LENGTH = "LENGTH" [true, true, false],
    LOADADDR = "LOADADDR" [true, true, false],
    LOG2CEIL = "LOG2CEIL" [true, true, false],
    MAP = "MAP" [true, false, false],
    MAX = "MAX" [true, true, false],
    MEMORY = "MEMORY" [true, false, false],
    MIN = "MIN" [true, true, false],
    NEXT = "NEXT" [true, true, false],
    NOCROSSREFS = "NOCROSSREFS" [true, true, false],
    NOCROSSREFS_TO = "NOCROSSREFS_TO" [true, true, false],
    NOFLOAT = "NOFLOAT" [true, false, false],
    NOLOAD = "NOLOAD" [false, true, false],
    ONLY_IF_RO = "ONLY_IF_RO" [true, true, false],
    ONLY_IF_RW = "ONLY_IF_RW" [true, true, false],
    ORIGIN = "ORIGIN" [true, true, false],
    OUTPUT = "OUTPUT" [true, false, false],
    OUTPUT_ARCH = "OUTPUT_ARCH" [true, false, false],
    OUTPUT_FORMAT = "OUTPUT_FORMAT" [true, false, false],
    OVERLAY = "OVERLAY" [true, true, false],
    PHDRS = "PHDRS" [true, false, false],
    PROVIDE = "PROVIDE" [true, true, false],
    PROVIDE_HIDDEN = "PROVIDE_HIDDEN" [true, true, false],
    READONLY = "READONLY" [false, true, false],
    REGION_ALIAS = "REGION_ALIAS" [true, false, false],
    SEARCH_DIR = "SEARCH_DIR" [true, false, false],
    SECTIONS = "SECTIONS" [true, false, false],
    SEGMENT_START = "SEGMENT_START" [true, true, false],
    SIZEOF = "SIZEOF" [true, true, false],
    SIZEOF_HEADERS = "SIZEOF_HEADERS" [true, true, false],
    SPECIAL = "SPECIAL" [true, true, false],
    STARTUP = "STARTUP" [true, false, false],
    SUBALIGN = "SUBALIGN" [true, true, false],
    SYSLIB = "SYSLIB" [true, false, false],
    TARGET = "TARGET" [true, false, false],
    TYPE = "TYPE" [false, true, false],
    VERSION = "VERSION" [true, false, false],
    l = "l" [true, false, false],
    len = "len" [true, false, false],
    o = "o" [true, false, false],
    org = "org" [true, false, false],
}

impl Keyword {
    /// The keyword `text` spells in `mode`, if any.
    #[must_use]
    pub fn lookup(mode: Mode, text: &[u8]) -> Option<Self> {
        let found = Self::ALL.binary_search_by(|k| k.spelling().as_bytes().cmp(text)).ok()?;
        let keyword = Self::ALL[found];
        keyword.in_mode(mode).then_some(keyword)
    }
}

/// What a token is (`spec/script.md` §1).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// An unquoted name: a symbol, a file name, a format name. Its text is
    /// the token's span.
    Name,
    /// A quoted string. Its text is the span without the two quotes.
    String,
    /// An integer, with its value (§1.5).
    Integer(u64),
    /// A keyword of the mode the token was read in.
    Keyword(Keyword),
    /// An operator or separator.
    Punct(Punct),
    /// In an input list, `-l` followed by name characters: a library. The
    /// name is the span after `-l` (`:file` included for `-l:file`).
    Library,
    /// The end of the bytes. Its span is empty, at the end.
    Eof,
}

/// A token and the bytes it was read from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// What stops the lexer: where, and the statement for the diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    /// The byte offset the error is reported at.
    pub offset: usize,
    /// `spec/diag.md` §3.2 "what": lower case, no final period.
    pub what: String,
    /// Context notes, in order.
    pub notes: Vec<String>,
}

impl LexError {
    fn new(offset: usize, what: impl Into<String>) -> Self {
        Self { offset, what: what.into(), notes: Vec::new() }
    }

    fn note(mut self, text: impl Into<String>) -> Self {
        self.notes.push(text.into());
        self
    }
}

/// Bytes that start a name in SCRIPT and INPUTLIST mode (§1.4).
const fn is_script_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || matches!(b, b'_' | b'/' | b'.' | b'\\' | b'$' | b'~')
}

/// Bytes that continue a name in SCRIPT and INPUTLIST mode (§1.4).
const fn is_script_name_char(b: u8) -> bool {
    is_script_name_start(b)
        || b.is_ascii_digit()
        || matches!(b, b'+' | b':' | b'[' | b']' | b',' | b'=' | b'-')
}

/// Bytes that start a name in EXPRESSION mode (§1.4).
const fn is_expr_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || matches!(b, b'_' | b'.' | b'\\' | b'$')
}

/// Bytes that continue a name in EXPRESSION mode (§1.4).
const fn is_expr_name_char(b: u8) -> bool {
    is_expr_name_start(b) || b.is_ascii_digit() || matches!(b, b'/' | b'~')
}

/// How a byte is shown in a diagnostic: `'x'` for printable ASCII, the
/// value in hexadecimal otherwise.
pub(crate) fn show_byte(b: u8) -> String {
    if b.is_ascii_graphic() { format!("'{}'", b as char) } else { format!("byte 0x{b:02x}") }
}

/// The lexer: a position in the bytes of one script.
#[derive(Debug, Clone)]
pub struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    /// A lexer at the start of `src`.
    #[must_use]
    pub fn new(src: &'a [u8]) -> Self {
        Self { src, pos: 0 }
    }

    /// The bytes being read.
    #[must_use]
    pub fn source(&self) -> &'a [u8] {
        self.src
    }

    /// The offset of the next byte to read.
    #[must_use]
    pub fn offset(&self) -> usize {
        self.pos
    }

    fn peek(&self, ahead: usize) -> Option<u8> {
        self.src.get(self.pos + ahead).copied()
    }

    /// Skips whitespace and comments (§1.3).
    fn skip_trivia(&mut self, mode: Mode) -> Result<(), LexError> {
        while let Some(b) = self.peek(0) {
            match b {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                b'/' if self.peek(1) == Some(b'*') => {
                    let start = self.pos;
                    let body = &self.src[start + 2..];
                    let Some(end) = body.windows(2).position(|w| w == b"*/") else {
                        return Err(LexError::new(start, "unterminated comment"));
                    };
                    if let Some(nul) = body[..end].iter().position(|&c| c == 0) {
                        return Err(LexError::new(start + 2 + nul, "NUL byte in a comment"));
                    }
                    self.pos = start + 2 + end + 2;
                }
                b'#' if mode != Mode::InputList => {
                    let rest = &self.src[self.pos..];
                    let len = rest.iter().position(|&c| c == b'\n').unwrap_or(rest.len());
                    if let Some(nul) = rest[..len].iter().position(|&c| c == 0) {
                        return Err(LexError::new(self.pos + nul, "NUL byte in a comment"));
                    }
                    self.pos += len;
                }
                _ => break,
            }
        }
        Ok(())
    }

    /// The length of the run of bytes from `at` satisfying `f`.
    fn run(&self, at: usize, f: impl Fn(u8) -> bool) -> usize {
        self.src[at.min(self.src.len())..].iter().take_while(|&&b| f(b)).count()
    }

    /// Reads the next token in `mode`.
    pub fn next(&mut self, mode: Mode) -> Result<Token, LexError> {
        self.skip_trivia(mode)?;
        let start = self.pos;
        let Some(b) = self.peek(0) else {
            return Ok(Token { kind: TokenKind::Eof, span: Span::new(start, start) });
        };
        let kind = match mode {
            Mode::Script => self.script_token(b)?,
            Mode::InputList => self.input_list_token(b)?,
            Mode::Expression => self.expression_token(b)?,
        };
        Ok(Token { kind, span: Span::new(start, self.pos) })
    }

    fn string(&mut self) -> Result<TokenKind, LexError> {
        let start = self.pos;
        let body = &self.src[start + 1..];
        let Some(len) = body.iter().position(|&c| c == b'"') else {
            return Err(LexError::new(start, "unterminated string"));
        };
        if let Some(nul) = body[..len].iter().position(|&c| c == 0) {
            return Err(LexError::new(start + 1 + nul, "NUL byte in a string"));
        }
        self.pos = start + 1 + len + 1;
        Ok(TokenKind::String)
    }

    fn invalid(&self, b: u8) -> LexError {
        LexError::new(self.pos, format!("invalid character {}", show_byte(b)))
    }

    /// An integer starting at the current position, `len` bytes long.
    fn integer(&mut self, len: usize) -> Result<TokenKind, LexError> {
        let start = self.pos;
        let text = &self.src[start..start + len];
        let value = parse_integer(text).map_err(|what| LexError::new(start, what))?;
        self.pos += len;
        Ok(TokenKind::Integer(value))
    }

    /// A token starting with a digit: the whole alphanumeric run (§1.5).
    fn digit_integer(&mut self) -> Result<TokenKind, LexError> {
        let len = self.run(self.pos, |c| c.is_ascii_alphanumeric());
        self.integer(len)
    }

    /// `$` + hex digits [+ `K`/`M`]: its length, 0 if there is none.
    fn dollar_integer_len(&self) -> usize {
        let digits = self.run(self.pos + 1, |c| c.is_ascii_hexdigit());
        if digits == 0 {
            return 0;
        }
        let suffix = usize::from(matches!(self.peek(1 + digits), Some(b'K' | b'k' | b'M' | b'm')));
        1 + digits + suffix
    }

    fn script_token(&mut self, b: u8) -> Result<TokenKind, LexError> {
        let next = self.peek(1);
        // `/=` is an operator although `/` starts a name: an operator wins a
        // tie, a longer name wins (`/=x` is a name).
        if b == b'/' && next == Some(b'=') && !self.peek(2).is_some_and(is_script_name_char) {
            self.pos += 2;
            return Ok(TokenKind::Punct(Punct::SlashAssign));
        }
        if b == b'$' {
            let int = self.dollar_integer_len();
            let name = 1 + self.run(self.pos + 1, is_script_name_char);
            if int > 0 && int >= name {
                return self.integer(int);
            }
        }
        if is_script_name_start(b) {
            let len = 1 + self.run(self.pos + 1, is_script_name_char);
            let text = &self.src[self.pos..self.pos + len];
            self.pos += len;
            return Ok(
                Keyword::lookup(Mode::Script, text).map_or(TokenKind::Name, TokenKind::Keyword)
            );
        }
        if b.is_ascii_digit() {
            return self.digit_integer();
        }
        let (punct, len) = match (b, next, self.peek(2)) {
            (b'"', ..) => return self.string(),
            (b'<', Some(b'<'), Some(b'=')) => (Punct::ShlAssign, 3),
            (b'>', Some(b'>'), Some(b'=')) => (Punct::ShrAssign, 3),
            (b'+', Some(b'='), _) => (Punct::PlusAssign, 2),
            (b'-', Some(b'='), _) => (Punct::MinusAssign, 2),
            (b'*', Some(b'='), _) => (Punct::StarAssign, 2),
            (b'&', Some(b'='), _) => (Punct::AmpAssign, 2),
            (b'|', Some(b'='), _) => (Punct::PipeAssign, 2),
            (b'^', Some(b'='), _) => (Punct::CaretAssign, 2),
            (b'=', Some(b'='), _) => (Punct::EqEq, 2),
            (b'!', Some(b'='), _) => (Punct::Ne, 2),
            (b'>', Some(b'='), _) => (Punct::Ge, 2),
            (b'=', ..) => (Punct::Assign, 1),
            (b'(', ..) => (Punct::LParen, 1),
            (b')', ..) => (Punct::RParen, 1),
            (b'{', ..) => (Punct::LBrace, 1),
            (b'}', ..) => (Punct::RBrace, 1),
            (b';', ..) => (Punct::Semicolon, 1),
            (b',', ..) => (Punct::Comma, 1),
            (b':', ..) => (Punct::Colon, 1),
            (b'!', ..) => (Punct::Bang, 1),
            (b'>', ..) => (Punct::Gt, 1),
            (b'+', ..) => (Punct::Plus, 1),
            (b'-', ..) => (Punct::Minus, 1),
            _ => return Err(self.invalid(b)),
        };
        self.pos += len;
        Ok(TokenKind::Punct(punct))
    }

    fn input_list_token(&mut self, b: u8) -> Result<TokenKind, LexError> {
        match b {
            b'"' => self.string(),
            b'(' | b')' | b',' => {
                self.pos += 1;
                Ok(TokenKind::Punct(match b {
                    b'(' => Punct::LParen,
                    b')' => Punct::RParen,
                    _ => Punct::Comma,
                }))
            }
            b'-' if self.peek(1) == Some(b'l') => {
                let len = self.run(self.pos + 2, is_script_name_char);
                if len == 0 {
                    return Err(LexError::new(self.pos, "'-l' without a library name")
                        .note("write the name right after '-l', as '-lname'"));
                }
                self.pos += 2 + len;
                Ok(TokenKind::Library)
            }
            b'=' => {
                let len = self.run(self.pos + 1, is_script_name_char);
                if len == 0 {
                    return Err(LexError::new(self.pos, "'=' without a file name")
                        .note("'=' is the sysroot prefix of the name right after it"));
                }
                self.pos += 1 + len;
                Ok(TokenKind::Name)
            }
            _ if is_script_name_start(b) => {
                let len = 1 + self.run(self.pos + 1, is_script_name_char);
                let text = &self.src[self.pos..self.pos + len];
                self.pos += len;
                Ok(Keyword::lookup(Mode::InputList, text)
                    .map_or(TokenKind::Name, TokenKind::Keyword))
            }
            _ if b.is_ascii_digit() => Err(LexError::new(
                self.pos,
                format!("file name starts with the digit {}", show_byte(b)),
            )
            .note("GNU ld skips a leading digit of an unquoted file name; quote the name")),
            b'#' => Err(LexError::new(self.pos, "invalid character '#'")
                .note("'#' starts no comment in an input list: GNU ld reads the text after it as file names")),
            _ => Err(self.invalid(b)
                .note("GNU ld skips this character in an input list; lld and mold read it as part of a file name")),
        }
    }

    fn expression_token(&mut self, b: u8) -> Result<TokenKind, LexError> {
        if b == b'$' {
            let int = self.dollar_integer_len();
            let name = 1 + self.run(self.pos + 1, is_expr_name_char);
            if int > 0 && int >= name {
                return self.integer(int);
            }
        }
        if is_expr_name_start(b) {
            let len = 1 + self.run(self.pos + 1, is_expr_name_char);
            let text = &self.src[self.pos..self.pos + len];
            // Hex digits and a base letter are an integer even without a
            // leading digit (`ah`, `ffx`): §1.5.
            if len >= 2
                && text[..len - 1].iter().all(u8::is_ascii_hexdigit)
                && matches!(text[len - 1], b'h' | b'H' | b'x' | b'X')
            {
                return self.integer(len);
            }
            self.pos += len;
            return Ok(
                Keyword::lookup(Mode::Expression, text).map_or(TokenKind::Name, TokenKind::Keyword)
            );
        }
        if b.is_ascii_digit() {
            return self.digit_integer();
        }
        let (punct, len) = match (b, self.peek(1), self.peek(2)) {
            (b'"', ..) => return self.string(),
            (b'<', Some(b'<'), Some(b'=')) => (Punct::ShlAssign, 3),
            (b'>', Some(b'>'), Some(b'=')) => (Punct::ShrAssign, 3),
            (b'<', Some(b'<'), _) => (Punct::Shl, 2),
            (b'>', Some(b'>'), _) => (Punct::Shr, 2),
            (b'<', Some(b'='), _) => (Punct::Le, 2),
            (b'>', Some(b'='), _) => (Punct::Ge, 2),
            (b'=', Some(b'='), _) => (Punct::EqEq, 2),
            (b'!', Some(b'='), _) => (Punct::Ne, 2),
            (b'&', Some(b'&'), _) => (Punct::AndAnd, 2),
            (b'|', Some(b'|'), _) => (Punct::OrOr, 2),
            (b'+', Some(b'='), _) => (Punct::PlusAssign, 2),
            (b'-', Some(b'='), _) => (Punct::MinusAssign, 2),
            (b'*', Some(b'='), _) => (Punct::StarAssign, 2),
            (b'/', Some(b'='), _) => (Punct::SlashAssign, 2),
            (b'&', Some(b'='), _) => (Punct::AmpAssign, 2),
            (b'|', Some(b'='), _) => (Punct::PipeAssign, 2),
            (b'^', Some(b'='), _) => (Punct::CaretAssign, 2),
            (b'(', ..) => (Punct::LParen, 1),
            (b')', ..) => (Punct::RParen, 1),
            (b'{', ..) => (Punct::LBrace, 1),
            (b'}', ..) => (Punct::RBrace, 1),
            (b';', ..) => (Punct::Semicolon, 1),
            (b',', ..) => (Punct::Comma, 1),
            (b':', ..) => (Punct::Colon, 1),
            (b'?', ..) => (Punct::Question, 1),
            (b'!', ..) => (Punct::Bang, 1),
            (b'~', ..) => (Punct::Tilde, 1),
            (b'+', ..) => (Punct::Plus, 1),
            (b'-', ..) => (Punct::Minus, 1),
            (b'*', ..) => (Punct::Star, 1),
            (b'/', ..) => (Punct::Slash, 1),
            (b'%', ..) => (Punct::Percent, 1),
            (b'&', ..) => (Punct::Amp, 1),
            (b'|', ..) => (Punct::Pipe, 1),
            (b'^', ..) => (Punct::Caret, 1),
            (b'<', ..) => (Punct::Lt, 1),
            (b'>', ..) => (Punct::Gt, 1),
            (b'=', ..) => (Punct::Assign, 1),
            _ => return Err(self.invalid(b)),
        };
        self.pos += len;
        Ok(TokenKind::Punct(punct))
    }
}

/// The value of an integer token (`spec/script.md` §1.5). The error is the
/// diagnostic's statement.
///
/// The forms: `0x`/`0X` + hex digits, `$` + hex digits, decimal, octal
/// (leading `0`), each optionally followed by `K`/`k` (x 1024) or `M`/`m`
/// (x 1024 * 1024); or digits followed by a base letter: `h`/`H`/`x`/`X`
/// (hexadecimal), `o`/`O` (octal), `b`/`B` (binary), `d`/`D` (decimal),
/// without a multiplier. A digit outside the base and a value that does
/// not fit in 64 bits are errors (GNU ld truncates the first and
/// saturates the second without a word).
pub fn parse_integer(text: &[u8]) -> Result<u64, String> {
    let shown = String::from_utf8_lossy(text);
    let bad = |why: &str| format!("malformed number '{shown}': {why}");
    let (body, multiplier) = match text.last() {
        Some(b'K' | b'k') => (&text[..text.len() - 1], 1024u64),
        Some(b'M' | b'm') => (&text[..text.len() - 1], 1024 * 1024),
        _ => (text, 1),
    };
    let (digits, radix) = if let Some(rest) = body
        .strip_prefix(b"0x")
        .or_else(|| body.strip_prefix(b"0X"))
        .or_else(|| body.strip_prefix(b"$"))
    {
        (rest, 16)
    } else if multiplier == 1
        && body.len() >= 2
        && matches!(
            body[body.len() - 1],
            b'h' | b'H' | b'x' | b'X' | b'o' | b'O' | b'b' | b'B' | b'd' | b'D'
        )
    {
        let radix = match body[body.len() - 1] {
            b'h' | b'H' | b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 10,
        };
        (&body[..body.len() - 1], radix)
    } else if body.len() >= 2 && body[0] == b'0' {
        (&body[1..], 8)
    } else {
        (body, 10)
    };
    if digits.is_empty() {
        return Err(bad("no digits"));
    }
    let mut value: u64 = 0;
    for &d in digits {
        let Some(digit) = (d as char).to_digit(radix) else {
            let base = match radix {
                16 => "a hexadecimal",
                10 => "a decimal",
                8 => "an octal",
                _ => "a binary",
            };
            return Err(bad(&format!("{} is not {base} digit", show_byte(d))));
        };
        value = value
            .checked_mul(u64::from(radix))
            .and_then(|v| v.checked_add(u64::from(digit)))
            .ok_or_else(|| bad("does not fit in 64 bits"))?;
    }
    value.checked_mul(multiplier).ok_or_else(|| bad("does not fit in 64 bits"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: script.md §1.7 — the table is sorted, so lookup is a binary search.
    #[test]
    fn keyword_table_is_sorted() {
        for pair in Keyword::ALL.windows(2) {
            assert!(pair[0].spelling() < pair[1].spelling(), "{:?} {:?}", pair[0], pair[1]);
        }
    }
}
