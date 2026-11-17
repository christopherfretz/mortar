//! The parser of stage 1 (`spec/script.md` §2).
//!
//! Recursive descent over [`Lexer`], which is told the mode of every
//! token. The first error, or the first command mortar does not implement,
//! ends the parse: what follows a refused command cannot be read by stage
//! 1's grammar, and one located diagnostic is what a script error needs.

use mortar_diag::{Diagnostic, Kind, Place, RefusalClass};

use crate::ast::{Command, CommandKind, FileForm, InputFile, InputItem, Script, Span, Word};
use crate::lex::{Keyword, LexError, Lexer, Mode, Punct, Token, TokenKind};

/// The deepest `AS_NEEDED` nesting accepted (§2.3).
pub const MAX_AS_NEEDED_DEPTH: usize = 16;

/// Statement-level keywords GNU ld accepts and mortar refuses by name (§2.10).
const REFUSED_KEYWORDS: &[Keyword] = &[
    Keyword::ASSERT,
    Keyword::FLOAT,
    Keyword::FORCE_COMMON_ALLOCATION,
    Keyword::FORCE_GROUP_ALLOCATION,
    Keyword::HIDDEN,
    Keyword::HLL,
    Keyword::INHIBIT_COMMON_ALLOCATION,
    Keyword::INSERT,
    Keyword::LD_FEATURE,
    Keyword::MAP,
    Keyword::MEMORY,
    Keyword::NOCROSSREFS,
    Keyword::NOCROSSREFS_TO,
    Keyword::NOFLOAT,
    Keyword::PHDRS,
    Keyword::PROVIDE,
    Keyword::PROVIDE_HIDDEN,
    Keyword::REGION_ALIAS,
    Keyword::SECTIONS,
    Keyword::SYSLIB,
    Keyword::VERSION,
];

/// The name a symbol assignment is refused under (§2.10).
pub const ASSIGNMENT: &str = "symbol assignment";

/// One error: where, and the diagnostic without its place.
struct Failure {
    offset: usize,
    diagnostic: Box<Diagnostic>,
}

impl From<LexError> for Failure {
    fn from(e: LexError) -> Self {
        let mut diagnostic = Diagnostic::error(Kind::Malformed { what: e.what });
        for note in e.notes {
            diagnostic = diagnostic.note(note);
        }
        Self { offset: e.offset, diagnostic: Box::new(diagnostic) }
    }
}

type Parsed<T> = Result<T, Failure>;

fn malformed(offset: usize, what: impl Into<String>) -> Failure {
    Failure {
        offset,
        diagnostic: Box::new(Diagnostic::error(Kind::Malformed { what: what.into() })),
    }
}

impl Failure {
    fn note(mut self, text: impl Into<String>) -> Self {
        *self.diagnostic = self.diagnostic.as_ref().clone().note(text);
        self
    }
}

pub(crate) struct Parser<'a> {
    lexer: Lexer<'a>,
    src: &'a [u8],
}

/// Parses `source`; errors are placed in `path` (`spec/script.md` §4).
pub(crate) fn parse(source: &[u8], path: &str) -> Result<Script, Box<Diagnostic>> {
    let mut parser = Parser { lexer: Lexer::new(source), src: source };
    parser.script().map_err(|f| {
        let (line, column) = crate::position(source, f.offset);
        Box::new((*f.diagnostic).at(Place::script(path, line, column)))
    })
}

/// A short description of a token for "expected X, found Y".
pub(crate) fn describe(src: &[u8], token: &Token) -> String {
    let text = || {
        let bytes = token.span.of(src);
        let mut shown = String::from_utf8_lossy(&bytes[..bytes.len().min(40)]).into_owned();
        if bytes.len() > 40 {
            shown.push_str("...");
        }
        shown
    };
    match &token.kind {
        TokenKind::Eof => "end of file".to_owned(),
        TokenKind::Name => format!("name '{}'", text()),
        TokenKind::String => format!("string {}", text()),
        TokenKind::Integer(_) => format!("number '{}'", text()),
        TokenKind::Keyword(k) => format!("keyword '{}'", k.spelling()),
        TokenKind::Punct(p) => format!("'{}'", p.spelling()),
        TokenKind::Library => format!("library '{}'", text()),
    }
}

impl Parser<'_> {
    fn next(&mut self, mode: Mode) -> Parsed<Token> {
        Ok(self.lexer.next(mode)?)
    }

    fn peek(&self, mode: Mode) -> Parsed<Token> {
        Ok(self.lexer.clone().next(mode)?)
    }

    fn unexpected(&self, token: &Token, expected: &str) -> Failure {
        malformed(
            token.span.start,
            format!("expected {expected}, found {}", describe(self.src, token)),
        )
    }

    fn expect(&mut self, mode: Mode, punct: Punct, after: &str) -> Parsed<Token> {
        let token = self.next(mode)?;
        if token.kind == TokenKind::Punct(punct) {
            Ok(token)
        } else {
            Err(self.unexpected(&token, &format!("'{}' after {after}", punct.spelling())))
        }
    }

    fn script(&mut self) -> Parsed<Script> {
        let mut commands = Vec::new();
        loop {
            let token = self.next(Mode::Script)?;
            let start = token.span.start;
            let kind = match token.kind {
                TokenKind::Eof => return Ok(Script { commands }),
                TokenKind::Punct(Punct::Semicolon) => continue,
                TokenKind::Keyword(keyword) => self.command(keyword, &token)?,
                TokenKind::Name | TokenKind::String => return Err(self.statement_name(&token)),
                _ => return Err(self.unexpected(&token, "a linker script command")),
            };
            let end = self.lexer.offset();
            commands.push(Command { kind, span: Span::new(start, end) });
        }
    }

    /// A statement that starts with a name: an assignment (refused), `LIB`
    /// (refused) or an error.
    fn statement_name(&self, token: &Token) -> Failure {
        let at = token.span.start;
        let next = self.peek(Mode::Script);
        if let Ok(Token { kind: TokenKind::Punct(p), .. }) = &next {
            if p.is_assignment() {
                return refused(at, ASSIGNMENT);
            }
            let text = token.span.of(self.src);
            if token.kind == TokenKind::Name && text == b"LIB" && *p == Punct::LParen {
                return refused(at, "LIB")
                    .note("LIB is a command of newer GNU ld releases than 2.44");
            }
        }
        let text = token.span.of(self.src);
        let failure =
            malformed(at, format!("unknown linker script command {}", describe(self.src, token)));
        if token.kind == TokenKind::Name && text == b"AS_NEEDED" {
            return failure.note("AS_NEEDED is allowed only inside the list of INPUT or GROUP");
        }
        if token.kind == TokenKind::Name && text.contains(&b'=') {
            return failure.note(
                "an unquoted name takes '=' into itself here: put spaces around an assignment operator",
            );
        }
        failure
    }

    fn command(&mut self, keyword: Keyword, token: &Token) -> Parsed<CommandKind> {
        let at = token.span.start;
        if REFUSED_KEYWORDS.contains(&keyword) {
            return Err(refused(at, keyword.spelling()));
        }
        let name = keyword.spelling();
        Ok(match keyword {
            Keyword::INPUT | Keyword::GROUP => {
                self.expect(Mode::Script, Punct::LParen, name)?;
                let items = self.input_list(name, 0)?;
                if keyword == Keyword::INPUT {
                    CommandKind::Input(items)
                } else {
                    CommandKind::Group(items)
                }
            }
            Keyword::OUTPUT_FORMAT => self.output_format()?,
            Keyword::OUTPUT_ARCH => CommandKind::OutputArch(self.one_argument(name)?),
            Keyword::TARGET => CommandKind::Target(self.one_argument(name)?),
            Keyword::SEARCH_DIR => CommandKind::SearchDir(self.one_argument(name)?),
            Keyword::OUTPUT => CommandKind::Output(self.one_argument(name)?),
            Keyword::ENTRY => CommandKind::Entry(self.one_argument(name)?),
            Keyword::STARTUP => CommandKind::Startup(self.one_argument(name)?),
            Keyword::EXTERN => CommandKind::Extern(self.extern_list()?),
            Keyword::INCLUDE => {
                let token = self.next(Mode::Script)?;
                CommandKind::Include(self.word(&token, "a file name after INCLUDE")?)
            }
            _ => {
                return Err(malformed(
                    at,
                    format!("keyword '{name}' cannot start a linker script command"),
                ));
            }
        })
    }

    /// A name or string read in SCRIPT mode, as a [`Word`] (§2.1).
    fn word(&self, token: &Token, expected: &str) -> Parsed<Word> {
        match token.kind {
            TokenKind::Name => {
                let text = token.span.of(self.src);
                if let Some(comma) = text.iter().position(|&b| b == b',') {
                    return Err(comma_in_name(token.span.start + comma));
                }
                Ok(Word { text: text.to_vec(), quoted: false, span: token.span })
            }
            TokenKind::String => self.string_word(token),
            TokenKind::Keyword(k) => Err(self.unexpected(token, expected).note(format!(
                "'{}' is a keyword here: quote it to use it as a name",
                k.spelling()
            ))),
            TokenKind::Integer(_) => Err(self
                .unexpected(token, expected)
                .note("GNU ld reads this as a number: quote it to use it as a name")),
            _ => Err(self.unexpected(token, expected)),
        }
    }

    fn string_word(&self, token: &Token) -> Parsed<Word> {
        let span = token.span;
        let text = Span::new(span.start + 1, span.end - 1).of(self.src);
        if text.is_empty() {
            return Err(malformed(span.start, "empty name"));
        }
        Ok(Word { text: text.to_vec(), quoted: true, span })
    }

    /// `( word )` in SCRIPT mode.
    fn one_argument(&mut self, name: &str) -> Parsed<Word> {
        self.expect(Mode::Script, Punct::LParen, name)?;
        let token = self.next(Mode::Script)?;
        let word = self.word(&token, &format!("the argument of {name}"))?;
        self.expect(Mode::Script, Punct::RParen, &format!("the argument of {name}"))?;
        Ok(word)
    }

    fn output_format(&mut self) -> Parsed<CommandKind> {
        let name = "OUTPUT_FORMAT";
        self.expect(Mode::Script, Punct::LParen, name)?;
        let token = self.next(Mode::Script)?;
        let default = self.word(&token, "a format name")?;
        let token = self.next(Mode::Script)?;
        match token.kind {
            TokenKind::Punct(Punct::RParen) => {
                return Ok(CommandKind::OutputFormat { default, big: None, little: None });
            }
            TokenKind::Punct(Punct::Comma) => {}
            _ => return Err(self.unexpected(&token, "',' or ')' after the first format name")),
        }
        let token = self.next(Mode::Script)?;
        let big = self.word(&token, "the big-endian format name")?;
        self.expect(Mode::Script, Punct::Comma, "the big-endian format name")?;
        let token = self.next(Mode::Script)?;
        let little = self.word(&token, "the little-endian format name")?;
        self.expect(Mode::Script, Punct::RParen, "the little-endian format name")?;
        Ok(CommandKind::OutputFormat { default, big: Some(big), little: Some(little) })
    }

    /// The list of `EXTERN`, after the keyword (EXPRESSION mode).
    fn extern_list(&mut self) -> Parsed<Vec<Word>> {
        self.expect(Mode::Script, Punct::LParen, "EXTERN")?;
        let mut symbols = Vec::new();
        let mut comma_ok = false;
        loop {
            let token = self.next(Mode::Expression)?;
            match token.kind {
                TokenKind::Punct(Punct::RParen) if comma_ok => return Ok(symbols),
                TokenKind::Punct(Punct::Comma) if comma_ok => comma_ok = false,
                TokenKind::Name => {
                    let text = token.span.of(self.src).to_vec();
                    symbols.push(Word { text, quoted: false, span: token.span });
                    comma_ok = true;
                }
                TokenKind::String => {
                    symbols.push(self.string_word(&token)?);
                    comma_ok = true;
                }
                TokenKind::Integer(_) => {
                    return Err(self.unexpected(&token, "a symbol name in EXTERN").note(
                        "GNU ld reads this as a number (hex digits and a base letter are one): quote it to use it as a name",
                    ));
                }
                TokenKind::Keyword(k) => {
                    return Err(self.unexpected(&token, "a symbol name in EXTERN").note(format!(
                        "'{}' is a keyword here: quote it to use it as a name",
                        k.spelling()
                    )));
                }
                _ => return Err(self.unexpected(&token, "a symbol name in EXTERN")),
            }
        }
    }

    /// An input list after its `(`, through its `)` (INPUTLIST mode).
    fn input_list(&mut self, owner: &str, depth: usize) -> Parsed<Vec<InputItem>> {
        let mut items = Vec::new();
        let mut comma_ok = false;
        loop {
            let token = self.next(Mode::InputList)?;
            match token.kind {
                TokenKind::Punct(Punct::RParen) if comma_ok => return Ok(items),
                TokenKind::Punct(Punct::Comma) if comma_ok => comma_ok = false,
                TokenKind::Keyword(Keyword::AS_NEEDED) => {
                    if depth >= MAX_AS_NEEDED_DEPTH {
                        return Err(malformed(
                            token.span.start,
                            format!("AS_NEEDED nested deeper than {MAX_AS_NEEDED_DEPTH} levels"),
                        ));
                    }
                    self.expect(Mode::InputList, Punct::LParen, "AS_NEEDED")?;
                    let inner = self.input_list("AS_NEEDED", depth + 1)?;
                    let span = Span::new(token.span.start, self.lexer.offset());
                    items.push(InputItem::AsNeeded { items: inner, span });
                    comma_ok = true;
                }
                TokenKind::Name | TokenKind::String | TokenKind::Library => {
                    items.push(InputItem::File(self.input_file(&token)?));
                    comma_ok = true;
                }
                TokenKind::Punct(Punct::RParen) => {
                    let failure = self.unexpected(&token, &format!("a file name in {owner}"));
                    return Err(if items.is_empty() {
                        failure.note("GNU ld rejects an empty list")
                    } else {
                        failure.note("GNU ld rejects a ',' before ')'")
                    });
                }
                TokenKind::Eof => {
                    return Err(self.unexpected(&token, &format!("')' to close {owner}")));
                }
                _ => return Err(self.unexpected(&token, &format!("a file name in {owner}"))),
            }
        }
    }

    fn input_file(&self, token: &Token) -> Parsed<InputFile> {
        let at = token.span.start;
        let spelling = match token.kind {
            TokenKind::String => self.string_word(token)?,
            _ => Word { text: token.span.of(self.src).to_vec(), quoted: false, span: token.span },
        };
        if !spelling.quoted
            && let Some(comma) = spelling.text.iter().position(|&b| b == b',')
        {
            return Err(comma_in_name(at + comma));
        }
        let text = &spelling.text;
        let (form, name) = if token.kind == TokenKind::Library {
            let name = &text[2..];
            if name.first() == Some(&b'=') {
                return Err(malformed(at, "'-l=' is not supported").note(
                    "GNU ld prefixes the sysroot to the library name itself; lld and mold look for a library named '=...'",
                ));
            }
            match name.strip_prefix(b":") {
                Some([]) => {
                    return Err(malformed(at, "'-l:' without a file name"));
                }
                Some(file) => (FileForm::LibraryVerbatim, file.to_vec()),
                None => (FileForm::Library, name.to_vec()),
            }
        } else if spelling.quoted && text.starts_with(b"-l") {
            return Err(malformed(at, "a quoted name that starts with '-l'").note(
                "GNU ld reads it as a file name, lld and mold as a library: write -lname unquoted, or ./-l... for a file",
            ));
        } else if let Some(rest) = text.strip_prefix(b"=") {
            if rest.is_empty() {
                return Err(malformed(at, "'=' without a file name"));
            }
            (FileForm::Sysroot, rest.to_vec())
        } else if let Some(rest) = text.strip_prefix(b"$SYSROOT") {
            if rest.first() != Some(&b'/') {
                return Err(malformed(at, "'$SYSROOT' not followed by '/'"));
            }
            (FileForm::Sysroot, rest.to_vec())
        } else {
            (FileForm::Path, text.clone())
        };
        Ok(InputFile { form, name, spelling })
    }
}

fn refused(offset: usize, name: &str) -> Failure {
    Failure { offset, diagnostic: Box::new(Diagnostic::refused(RefusalClass::ScriptCommand, name)) }
}

fn comma_in_name(offset: usize) -> Failure {
    malformed(offset, "',' inside an unquoted name").note(
        "GNU ld reads the ',' as part of the name, lld and mold as a separator: put a space before it, or quote the name",
    )
}
