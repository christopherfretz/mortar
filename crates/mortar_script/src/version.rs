//! Version scripts and dynamic lists (`spec/script.md` §9, M4): the parser
//! of §9.1 into [`crate::VersionScript`] and [`crate::DynamicList`] (WP-125).
//!
//! The files of one option are read as one script (or one list), in
//! command-line order (§9.1): a parent may be a node of an earlier file.
//! The tree keeps what the text says, nothing decided: node names, parents
//! (resolved to the index of their node), and each entry's scope, language,
//! quoting and raw text. What an entry matches is `spec/resolve.md` §9.7's.
//!
//! The first error ends the parse with one located `version-script-syntax`
//! diagnostic (§9.2). No construct nests (a nested `extern` block is
//! refused), so the parser does not recurse: its depth is bounded by
//! construction, and its work is linear in the bytes.
//!
//! ```
//! use mortar_script::version::{SourceFile, parse_version_script};
//! use mortar_script::{Language, Scope};
//!
//! let text = b"V1 { global: foo; local: *; };\nV2 { extern \"C++\" { ns::*; }; } V1;";
//! let script = parse_version_script(&[SourceFile { path: "v.map", bytes: text }]).unwrap();
//! assert_eq!(script.nodes.len(), 2);
//! assert_eq!(script.nodes[1].parents[0].node, 0);
//! let local = &script.nodes[0].entries[1];
//! assert_eq!((local.scope, &local.pattern.text[..]), (Scope::Local, &b"*"[..]));
//! assert_eq!(script.nodes[1].entries[0].language, Language::Cxx);
//!
//! let err = parse_version_script(&[SourceFile { path: "v.map", bytes: b"V1 { foo }" }]);
//! assert_eq!(err.unwrap_err().kind().id(), "version-script-syntax");
//! ```

use mortar_diag::{Diagnostic, Kind, Place};

use crate::ast::{
    DynamicList, Language, Scope, Span, VersionEntry, VersionNode, VersionParent, VersionScript,
    Word,
};
use crate::lex::{LexError, Lexer, Mode, Punct, Token, TokenKind};
use crate::parse::describe;

/// GNU ld's name for a `--version-script` file in its syntax error, the
/// `{script}` of `version-script-syntax` (`spec/diag.md` §3.2).
pub const VERSION_SCRIPT: &str = "VERSION script";

/// GNU ld's name for a `--dynamic-list` file in its syntax error.
pub const DYNAMIC_LIST: &str = "dynamic list";

/// One file of the line: the path diagnostics name (as the user gave it)
/// and its bytes.
#[derive(Debug, Clone, Copy)]
pub struct SourceFile<'a> {
    pub path: &'a str,
    pub bytes: &'a [u8],
}

/// Parses the `--version-script` files of a line, in command-line order,
/// as one script (`spec/script.md` §9.1). Every file holds at least one
/// node. Spans are offsets into the file [`VersionNode::file`] names.
/// No file: the empty script.
pub fn parse_version_script(files: &[SourceFile<'_>]) -> Result<VersionScript, Box<Diagnostic>> {
    let mut script = VersionScript::default();
    for (index, file) in files.iter().enumerate() {
        let file_index = u32::try_from(index).unwrap_or(u32::MAX);
        let mut parser = Parser::new(file.bytes);
        parser
            .version_file(file_index, &mut script)
            .map_err(|f| f.located(VERSION_SCRIPT, file))?;
    }
    Ok(script)
}

/// Parses the `--dynamic-list` files of a line, in command-line order, as
/// one list (`spec/script.md` §9.1). Each file is one `{ … };` block.
/// No file: the empty list.
pub fn parse_dynamic_list(files: &[SourceFile<'_>]) -> Result<DynamicList, Box<Diagnostic>> {
    let mut list = DynamicList::default();
    for file in files {
        let mut parser = Parser::new(file.bytes);
        parser.dynamic_list_file(&mut list).map_err(|f| f.located(DYNAMIC_LIST, file))?;
    }
    Ok(list)
}

/// One error: where, what, and its notes.
struct Failure {
    offset: usize,
    what: String,
    notes: Vec<String>,
}

impl Failure {
    fn new(offset: usize, what: impl Into<String>) -> Self {
        Self { offset, what: what.into(), notes: Vec::new() }
    }

    fn note(mut self, text: impl Into<String>) -> Self {
        self.notes.push(text.into());
        self
    }

    fn located(self, script: &str, file: &SourceFile<'_>) -> Box<Diagnostic> {
        let (line, column) = crate::position(file.bytes, self.offset);
        let mut diagnostic = Diagnostic::error(Kind::VersionScriptSyntax {
            script: script.to_owned(),
            what: self.what,
        });
        for note in self.notes {
            diagnostic = diagnostic.note(note);
        }
        Box::new(diagnostic.at(Place::script(file.path, line, column)))
    }
}

impl From<LexError> for Failure {
    fn from(e: LexError) -> Self {
        Self { offset: e.offset, what: e.what, notes: e.notes }
    }
}

type Parsed<T> = Result<T, Failure>;

/// Whether a body may hold `global:`/`local:` labels.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Body {
    Node,
    DynamicList,
}

struct Parser<'a> {
    lexer: Lexer<'a>,
    src: &'a [u8],
}

/// The bytes GNU ld reads in a version name (measured, WP-125 record):
/// `[A-Za-z_.$][A-Za-z0-9_.$]*`. Other bytes it drops or rejects while
/// lld and mold keep them, so mortar refuses them.
fn version_name_problem(text: &[u8]) -> Option<String> {
    let first = text.first()?;
    if first.is_ascii_digit() {
        return Some(format!("version name '{}' starts with a digit", lossy(text)));
    }
    let bad =
        text.iter().find(|&&b| !(b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'$')))?;
    Some(format!("version name '{}' holds {}", lossy(text), crate::lex::show_byte(*bad)))
}

fn lossy(bytes: &[u8]) -> String {
    let mut shown = String::from_utf8_lossy(&bytes[..bytes.len().min(40)]).into_owned();
    if bytes.len() > 40 {
        shown.push_str("...");
    }
    shown
}

const VERSION_NAME_NOTE: &str = "GNU ld reads only letters, digits, '_', '.' and '$' of a version name, and no leading digit; lld and mold read the whole name";

impl<'a> Parser<'a> {
    fn new(src: &'a [u8]) -> Self {
        Self { lexer: Lexer::new(src), src }
    }

    fn next(&mut self) -> Parsed<Token> {
        Ok(self.lexer.next(Mode::Version)?)
    }

    fn peek(&self) -> Parsed<Token> {
        Ok(self.lexer.clone().next(Mode::Version)?)
    }

    fn text(&self, token: &Token) -> &'a [u8] {
        token.span.of(self.src)
    }

    fn unexpected(&self, token: &Token, expected: &str) -> Failure {
        Failure::new(
            token.span.start,
            format!("expected {expected}, found {}", describe(self.src, token)),
        )
    }

    fn expect(&mut self, punct: Punct, after: &str) -> Parsed<Token> {
        let token = self.next()?;
        if token.kind == TokenKind::Punct(punct) {
            Ok(token)
        } else {
            Err(self.unexpected(&token, &format!("'{}' after {after}", punct.spelling())))
        }
    }

    /// One `--version-script` file: `node+`, or one anonymous node.
    fn version_file(&mut self, file: u32, script: &mut VersionScript) -> Parsed<()> {
        let mut nodes_here = 0usize;
        loop {
            let token = self.next()?;
            let start = token.span.start;
            let name = match token.kind {
                TokenKind::Eof if nodes_here > 0 => return Ok(()),
                TokenKind::Eof => {
                    return Err(Failure::new(start, "empty version script").note(
                        "a version script holds at least one node (GNU ld and lld reject an empty file; mold accepts it)",
                    ));
                }
                TokenKind::Punct(Punct::LBrace) => None,
                TokenKind::Name => Some(self.node_name(&token)?),
                TokenKind::String => {
                    return Err(Failure::new(start, "quoted version name").note(
                        "GNU ld strips the quotes, lld and mold keep them in the version name: write the name unquoted",
                    ));
                }
                _ => return Err(self.unexpected(&token, "a version node name or '{'")),
            };
            // The anonymous node stands alone (§9.1).
            if script.nodes.first().is_some_and(|n| n.name.is_none())
                || (name.is_none() && !script.nodes.is_empty())
            {
                return Err(Failure::new(start, "an anonymous version node cannot be combined with other version nodes").note(
                    "GNU ld: anonymous version tag cannot be combined with other version tags (lld refuses too; mold accepts)",
                ));
            }
            if let Some(word) = &name {
                if script.nodes.iter().any(|n| n.name.as_ref().is_some_and(|m| m.text == word.text))
                {
                    return Err(Failure::new(
                        start,
                        format!("version node '{}' is defined twice", lossy(&word.text)),
                    )
                    .note("GNU ld: duplicate version tag; lld and mold merge the two nodes"));
                }
                self.expect(Punct::LBrace, "the version name")?;
            }
            let entries = self.body(Body::Node)?;
            let mut parents = Vec::new();
            loop {
                let token = self.next()?;
                match token.kind {
                    TokenKind::Punct(Punct::Semicolon) => break,
                    TokenKind::Name if name.is_some() => {
                        let parent = self.node_name(&token)?;
                        let Some(node) = script
                            .nodes
                            .iter()
                            .position(|n| n.name.as_ref().is_some_and(|m| m.text == parent.text))
                        else {
                            return Err(Failure::new(
                                token.span.start,
                                format!("parent version '{}' is not a node defined before this one", lossy(&parent.text)),
                            )
                            .note("GNU ld: unable to find version dependency (lld and mold accept a forward or missing parent)"));
                        };
                        parents.push(VersionParent { name: parent, node });
                    }
                    TokenKind::String if name.is_some() => {
                        return Err(Failure::new(token.span.start, "quoted parent version name")
                            .note("write the parent's name unquoted, as its node is named"));
                    }
                    _ if name.is_some() => {
                        return Err(
                            self.unexpected(&token, "a parent version name or ';' after '}'")
                        );
                    }
                    _ => return Err(self.unexpected(&token, "';' after the anonymous node's '}'")),
                }
            }
            let end = self.lexer.offset();
            script.nodes.push(VersionNode {
                name,
                parents,
                entries,
                file,
                span: Span::new(start, end),
            });
            nodes_here += 1;
        }
    }

    /// A node name or a parent's name, checked against GNU ld's reading.
    fn node_name(&self, token: &Token) -> Parsed<Word> {
        let text = self.text(token);
        if let Some(problem) = version_name_problem(text) {
            return Err(Failure::new(token.span.start, problem).note(VERSION_NAME_NOTE));
        }
        Ok(Word { text: text.to_vec(), quoted: false, span: token.span })
    }

    /// One `--dynamic-list` file: `{ entry* } ;`.
    fn dynamic_list_file(&mut self, list: &mut DynamicList) -> Parsed<()> {
        let token = self.next()?;
        match token.kind {
            TokenKind::Punct(Punct::LBrace) => {}
            TokenKind::Eof => {
                return Err(Failure::new(token.span.start, "empty dynamic list")
                    .note("a dynamic list is one '{ ... };' block (GNU ld, lld and mold reject an empty file)"));
            }
            _ => return Err(self.unexpected(&token, "'{' to open the dynamic list")),
        }
        list.entries.extend(self.body(Body::DynamicList)?);
        self.expect(Punct::Semicolon, "the dynamic list's '}'")?;
        let token = self.next()?;
        if token.kind != TokenKind::Eof {
            return Err(self.unexpected(&token, "end of file after the dynamic list").note(
                "a dynamic list file is one '{ ... };' block (GNU ld accepts several, lld and mold do not)",
            ));
        }
        Ok(())
    }

    /// A body after its `{`, through its `}`: labels and entries.
    fn body(&mut self, body: Body) -> Parsed<Vec<VersionEntry>> {
        let mut entries = Vec::new();
        let mut scope = Scope::Global;
        loop {
            let token = self.next()?;
            match token.kind {
                TokenKind::Punct(Punct::RBrace) => return Ok(entries),
                TokenKind::Name => {
                    let text = self.text(&token);
                    let next = self.peek()?;
                    if matches!(text, b"global" | b"local")
                        && next.kind == TokenKind::Punct(Punct::Colon)
                    {
                        if body == Body::DynamicList {
                            return Err(Failure::new(
                                token.span.start,
                                format!("'{}:' in a dynamic list", lossy(text)),
                            )
                            .note("a dynamic list has no scopes (GNU ld and mold reject a label; lld accepts 'global:')"));
                        }
                        self.next()?;
                        scope = if text == b"global" { Scope::Global } else { Scope::Local };
                        continue;
                    }
                    if text == b"extern" && next.kind == TokenKind::String {
                        self.extern_block(scope, &mut entries)?;
                        continue;
                    }
                    let pattern = Word { text: text.to_vec(), quoted: false, span: token.span };
                    self.entry_end(&pattern)?;
                    entries.push(VersionEntry { scope, language: Language::Plain, pattern });
                }
                TokenKind::String => {
                    let pattern = self.string_word(&token)?;
                    self.entry_end(&pattern)?;
                    entries.push(VersionEntry { scope, language: Language::Plain, pattern });
                }
                TokenKind::Eof => return Err(self.unexpected(&token, "'}' to close the block")),
                _ => return Err(self.unexpected(&token, "an entry, 'global:', 'local:' or '}'")),
            }
        }
    }

    /// The `;` after an entry outside an `extern` block.
    fn entry_end(&mut self, pattern: &Word) -> Parsed<()> {
        self.expect(Punct::Semicolon, &format!("the entry '{}'", lossy(&pattern.text)))
            .map(|_| ())
            .map_err(|f| {
                if pattern.text == b"global" || pattern.text == b"local" {
                    f
                } else {
                    f.note("every entry ends with ';' (GNU ld and lld; mold accepts a last entry without one)")
                }
            })
    }

    fn string_word(&self, token: &Token) -> Parsed<Word> {
        let span = token.span;
        let text = Span::new(span.start + 1, span.end - 1).of(self.src);
        if text.is_empty() {
            return Err(Failure::new(span.start, "empty name")
                .note("GNU ld, lld and mold accept \"\" as an entry that matches no symbol; mortar refuses it as never meant"));
        }
        Ok(Word { text: text.to_vec(), quoted: true, span })
    }

    /// `extern STRING { (pattern ;)* pattern? } ;`, the lexer at the string.
    fn extern_block(&mut self, scope: Scope, entries: &mut Vec<VersionEntry>) -> Parsed<()> {
        let token = self.next()?;
        let language = match Span::new(token.span.start + 1, token.span.end - 1).of(self.src) {
            b"C" => Language::C,
            b"C++" => Language::Cxx,
            other => {
                return Err(Failure::new(
                    token.span.start,
                    format!("unknown language \"{}\" in extern", lossy(other)),
                )
                .note("the languages are \"C\" and \"C++\" (GNU ld accepts any string, lld and mold refuse it)"));
            }
        };
        self.expect(Punct::LBrace, "the extern language")?;
        loop {
            let token = self.next()?;
            let pattern = match token.kind {
                TokenKind::Punct(Punct::RBrace) => break,
                TokenKind::Name => {
                    let text = self.text(&token);
                    if text == b"extern" && self.peek()?.kind == TokenKind::String {
                        return Err(Failure::new(token.span.start, "nested extern block").note(
                            "GNU ld and mold accept an extern block inside another, lld refuses it",
                        ));
                    }
                    Word { text: text.to_vec(), quoted: false, span: token.span }
                }
                TokenKind::String => self.string_word(&token)?,
                _ => return Err(self.unexpected(&token, "an entry or '}' in the extern block")),
            };
            entries.push(VersionEntry { scope, language, pattern });
            let after = self.next()?;
            match after.kind {
                TokenKind::Punct(Punct::Semicolon) => {}
                TokenKind::Punct(Punct::RBrace) => break,
                _ => {
                    let failure = self.unexpected(&after, "';' or '}' after the entry");
                    let label = matches!(
                        &entries.last().map(|e| &e.pattern.text[..]),
                        Some(b"global" | b"local")
                    );
                    return Err(if label && after.kind == TokenKind::Punct(Punct::Colon) {
                        failure.note("'global:' and 'local:' are not allowed inside an extern block (GNU ld and lld; mold accepts them)")
                    } else {
                        failure
                    });
                }
            }
        }
        self.expect(Punct::Semicolon, "the extern block's '}'")?;
        Ok(())
    }
}
