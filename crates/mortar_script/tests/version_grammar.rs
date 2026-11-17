//! `version-script-grammar` and `dynamic-list-grammar` (`spec/script.md`
//! §9.3): each sentence of §9.1 and §9.2, the lexer's version mode token by
//! token, each production and each refusal.

use mortar_diag::{Diagnostic, Kind, Place};
use mortar_script::lex::{Lexer, Mode, Punct, TokenKind};
use mortar_script::version::{SourceFile, parse_dynamic_list, parse_version_script};
use mortar_script::{DynamicList, Language, Scope, Span, VersionEntry, VersionScript};

fn vs(text: &str) -> VersionScript {
    parse_version_script(&[SourceFile { path: "v.map", bytes: text.as_bytes() }])
        .unwrap_or_else(|d| panic!("{text:?}: {}", render(&d)))
}

fn dl(text: &str) -> DynamicList {
    parse_dynamic_list(&[SourceFile { path: "d.list", bytes: text.as_bytes() }])
        .unwrap_or_else(|d| panic!("{text:?}: {}", render(&d)))
}

fn render(d: &Diagnostic) -> String {
    d.render(&mortar_diag::Style::plain("ld"))
}

/// What an error said: path, line, column, script word, statement, notes.
fn said(d: &Diagnostic) -> (String, u32, u32, String, String) {
    assert_eq!(d.id(), "version-script-syntax", "{}", render(d));
    let Some(Place::Script { path, line, column }) = d.place() else {
        panic!("not placed: {}", render(d));
    };
    let Kind::VersionScriptSyntax { script, what } = d.kind() else { unreachable!() };
    (path.clone(), *line, *column, script.clone(), what.clone())
}

/// A version script that is refused at `line:column` with a statement
/// holding `fragment`.
#[track_caller]
fn vs_err(text: &[u8], line: u32, column: u32, fragment: &str) -> Diagnostic {
    let d = parse_version_script(&[SourceFile { path: "v.map", bytes: text }])
        .expect_err(&String::from_utf8_lossy(text));
    let (path, l, c, script, what) = said(&d);
    assert_eq!(
        (path.as_str(), l, c, script.as_str()),
        ("v.map", line, column, "VERSION script"),
        "{:?}: {what}",
        String::from_utf8_lossy(text)
    );
    assert!(
        what.contains(fragment),
        "{:?}: {what:?} lacks {fragment:?}",
        String::from_utf8_lossy(text)
    );
    *d
}

#[track_caller]
fn dl_err(text: &[u8], line: u32, column: u32, fragment: &str) -> Diagnostic {
    let d = parse_dynamic_list(&[SourceFile { path: "d.list", bytes: text }])
        .expect_err(&String::from_utf8_lossy(text));
    let (path, l, c, script, what) = said(&d);
    assert_eq!(
        (path.as_str(), l, c, script.as_str()),
        ("d.list", line, column, "dynamic list"),
        "{:?}: {what}",
        String::from_utf8_lossy(text)
    );
    assert!(
        what.contains(fragment),
        "{:?}: {what:?} lacks {fragment:?}",
        String::from_utf8_lossy(text)
    );
    *d
}

fn has_note(d: &Diagnostic, fragment: &str) -> bool {
    render(d).lines().any(|l| l.starts_with(">>> ") && l.contains(fragment))
}

/// The entries of a node as `scope lang text` strings, quoted text in
/// double quotes.
fn shown(entries: &[VersionEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|e| {
            let scope = match e.scope {
                Scope::Global => "g",
                Scope::Local => "l",
            };
            let lang = match e.language {
                Language::Plain => "",
                Language::C => "C:",
                Language::Cxx => "C++:",
            };
            let text = String::from_utf8_lossy(&e.pattern.text);
            if e.pattern.quoted {
                format!("{scope} {lang}\"{text}\"")
            } else {
                format!("{scope} {lang}{text}")
            }
        })
        .collect()
}

fn names(script: &VersionScript) -> Vec<String> {
    script
        .nodes
        .iter()
        .map(|n| n.name.as_ref().map_or_else(|| "<anon>".to_owned(), |w| w.to_string_lossy()))
        .collect()
}

/// The tokens of `text` in version mode, as their bytes (`"…"` kept for
/// strings), or the first error's statement.
fn tokens(text: &[u8]) -> Result<Vec<String>, String> {
    let mut lexer = Lexer::new(text);
    let mut out = Vec::new();
    loop {
        let t = lexer.next(Mode::Version).map_err(|e| e.what)?;
        match t.kind {
            TokenKind::Eof => return Ok(out),
            TokenKind::Name | TokenKind::String | TokenKind::Punct(_) => {
                out.push(String::from_utf8_lossy(t.span.of(text)).into_owned());
            }
            other => panic!("unexpected token kind {other:?}"),
        }
    }
}

// ---- the lexer's version mode (§9.1 first paragraph, §1.10) -----------

// spec: script.md §9.1 (a name is a maximal run of bytes other than
// whitespace and `{ } ; : "`; it may hold `* ? [ ]` and `\`, kept as
// written), §9.3 (each lexes as one token, text kept)
#[test]
fn version_script_grammar_glob_bytes_lex_as_one_token() {
    for word in [
        "fo*", "f?o", "f[o]o", "[a-c]*", "[!f]*", "[^f]*", "fo\\*", "fo\\o", "fo[o", "foo\\", "*",
        "**", "f[]]o", "_Z*", "-x", "a=b", "a/b", "$foo", "!foo", "a+b", "a,b", "a@b", "a%b",
    ] {
        assert_eq!(tokens(word.as_bytes()), Ok(vec![word.to_owned()]), "{word}");
        let script = vs(&format!("V1 {{ {word}; }};"));
        assert_eq!(shown(&script.nodes[0].entries), [format!("g {word}")], "{word}");
    }
}

// spec: script.md §9.1 (`::` and `~` inside a name: C++ names in
// `extern "C++"`; a single `:` ends a name, a name never starts with one)
#[test]
fn version_script_grammar_colons_and_tilde() {
    assert_eq!(tokens(b"ns::foo"), Ok(vec!["ns::foo".into()]));
    assert_eq!(tokens(b"ns::~Foo*"), Ok(vec!["ns::~Foo*".into()]));
    assert_eq!(tokens(b"a::b::c"), Ok(vec!["a::b::c".into()]));
    assert_eq!(tokens(b"global:"), Ok(vec!["global".into(), ":".into()]));
    assert_eq!(tokens(b"foo:bar"), Ok(vec!["foo".into(), ":".into(), "bar".into()]));
    assert_eq!(tokens(b"foo:::bar"), Ok(vec!["foo::".into(), ":".into(), "bar".into()]));
    assert_eq!(tokens(b"::foo"), Ok(vec![":".into(), ":".into(), "foo".into()]));
    let script = vs("V1 { extern \"C++\" { ns::foo; ns::~Foo*; ns::*; }; };");
    assert_eq!(
        shown(&script.nodes[0].entries),
        ["g C++:ns::foo", "g C++:ns::~Foo*", "g C++:ns::*"]
    );
    // GNU ld and lld refuse these (mold accepts): a syntax error here.
    vs_err(b"V1 { foo:bar; };", 1, 9, "expected ';'");
    vs_err(b"V1 { foo:::bar; };", 1, 11, "expected ';'");
    vs_err(b"V1 { extern \"C++\" { ::foo; }; };", 1, 21, "expected an entry");
}

// spec: script.md §9.1 (a quoted string is a name taken literally: the
// bytes between the quotes, no escape processed, never a glob)
#[test]
fn version_script_grammar_quoted_entries_are_raw() {
    let script = vs("V1 { \"fo*\"; \"foo(int)\"; \"a\\\"; \"x y\"; \"a\nb\"; \"{;}\"; };");
    assert_eq!(
        shown(&script.nodes[0].entries),
        ["g \"fo*\"", "g \"foo(int)\"", "g \"a\\\"", "g \"x y\"", "g \"a\nb\"", "g \"{;}\""]
    );
    let e = &script.nodes[0].entries[0];
    assert!(e.pattern.quoted);
    assert_eq!(e.pattern.span, Span::new(5, 10), "the span holds the quotes");
    // A string directly after a name is two tokens.
    assert_eq!(tokens(b"fo\"o\""), Ok(vec!["fo".into(), "\"o\"".into()]));
    vs_err(b"V1 { fo\"o\"; };", 1, 8, "expected ';'");
    // An empty string is refused (all three accept it and match nothing).
    let d = vs_err(b"V1 { \"\"; };", 1, 6, "empty name");
    assert!(has_note(&d, "never meant"));
    vs_err(b"V1 { \"foo; };", 1, 6, "unterminated string");
    vs_err(b"V1 { \"f\0o\"; };", 1, 8, "NUL byte in a string");
}

// spec: script.md §9.1 (comments: `#` to the end of the line, whole-line
// or after an entry, `foo; #bar;` lists only `foo`; `/* … */` also across
// lines; `//` is not a comment), §1.3
#[test]
fn version_script_grammar_comments() {
    let text = "# a libxml2-style comment\nV1 {\n  global:\n# section\n    foo; bar; # trailing\n  baz; #qux;\n  /* block\n spanning */ local: /* x */ *;\n};\n";
    let script = vs(text);
    assert_eq!(shown(&script.nodes[0].entries), ["g foo", "g bar", "g baz", "l *"]);
    // `#` ends a name and starts a comment (GNU ld, lld and mold agree).
    assert_eq!(shown(&vs("V1 { foo#bar\n; };").nodes[0].entries), ["g foo"]);
    // Comments at the end, no final newline.
    assert_eq!(names(&vs("V1 { foo; }; # end")), ["V1"]);
    assert_eq!(names(&vs("V1 { foo; }; /* end */")), ["V1"]);
    // `//` is no comment, anywhere.
    let d = vs_err(b"// comment\nV1 { foo; };", 1, 1, "'//' starts no comment");
    assert!(has_note(&d, "'#' to the end of the line"));
    vs_err(b"V1 { foo; // x\n};", 1, 11, "'//' starts no comment");
    vs_err(b"V1 { a//b; };", 1, 7, "'//' inside a name");
    // `/*` inside a name: GNU ld ends the name there, lld and mold read on.
    let d = vs_err(b"V1 { foo/*c*/; };", 1, 9, "'/*' inside a name");
    assert!(has_note(&d, "GNU ld ends the name before it"));
    vs_err(b"V1 { foo; };\n/* open", 2, 1, "unterminated comment");
    vs_err(b"V1 { foo; }; # x\0y", 1, 17, "NUL byte in a comment");
}

// spec: script.md §9.1 (whitespace: space, tab, CR, LF; control bytes and
// NUL are invalid characters, §1.3, §1.9; bytes above 0x7f may stand in a
// name)
#[test]
fn version_script_grammar_whitespace_and_bytes() {
    let script = vs("V1\t{\r\n global :\tfoo ;\r\n}\r\n;");
    assert_eq!(shown(&script.nodes[0].entries), ["g foo"]);
    vs_err(b"V1 { f\x0co; };", 1, 7, "invalid character byte 0x0c");
    vs_err(b"V1 { f\x0bo; };", 1, 7, "invalid character byte 0x0b");
    vs_err(b"V1 { f\0o; };", 1, 7, "invalid character byte 0x00");
    vs_err(b"V1 { f\x7fo; };", 1, 7, "invalid character byte 0x7f");
    let script = vs("V1 { f\u{e9}; };");
    assert_eq!(script.nodes[0].entries[0].pattern.text, "f\u{e9}".as_bytes());
    let script =
        parse_version_script(&[SourceFile { path: "v.map", bytes: b"V1 { f\xff; };" }]).unwrap();
    assert_eq!(script.nodes[0].entries[0].pattern.text, b"f\xff");
}

// ---- the grammar of version scripts (§9.1) ------------------------------

// spec: script.md §9.1 `node := NAME "{" body "}" parents ";"`, `body`;
// entries before the first label are `global`
#[test]
fn version_script_grammar_nodes_and_scopes() {
    let script = vs("V1 { a; global: b; local: c; \"d\"; };\nV2 { local: *; };\nV3 { };");
    assert_eq!(names(&script), ["V1", "V2", "V3"]);
    assert_eq!(shown(&script.nodes[0].entries), ["g a", "g b", "l c", "l \"d\""]);
    assert_eq!(shown(&script.nodes[1].entries), ["l *"]);
    assert!(script.nodes[2].entries.is_empty());
    assert!(!script.is_anonymous());
    let node = &script.nodes[0];
    assert_eq!((node.file, node.span), (0, Span::new(0, 36)));
    assert_eq!(node.name.as_ref().unwrap().span, Span::new(0, 2));
    // `global :` with a space is the label (all three).
    assert_eq!(shown(&vs("V1 { local : x; global : y; };").nodes[0].entries), ["l x", "g y"]);
}

// spec: script.md §9.1 (`local:` before `global:`: accepted, lld and mold;
// a label twice, labels with no entry: accepted, lld and mold; GNU ld
// rejects all three)
#[test]
fn version_script_grammar_label_order() {
    assert_eq!(shown(&vs("V1 { local: *; global: foo; };").nodes[0].entries), ["l *", "g foo"]);
    assert_eq!(shown(&vs("V1 { global: global: foo; };").nodes[0].entries), ["g foo"]);
    assert!(vs("V1 { global: local: };").nodes[0].entries.is_empty());
}

// spec: script.md §9.1 (`global`, `local` and `extern` are labels or a
// block only before `:` or a string; otherwise names: GNU ld, lld and mold
// read `global;` as an entry)
#[test]
fn version_script_grammar_label_words_as_names() {
    assert_eq!(shown(&vs("V1 { global; local; };").nodes[0].entries), ["g global", "g local"]);
    assert_eq!(shown(&vs("V1 { local: extern; };").nodes[0].entries), ["l extern"]);
    assert_eq!(shown(&vs("V1 { \"global\"; };").nodes[0].entries), ["g \"global\""]);
}

// spec: script.md §9.1 (`parents := NAME*`; several parents accepted, GNU
// ld; a parent must be a node defined earlier)
#[test]
fn version_script_grammar_parents() {
    let script = vs("V1 { foo; }; V2 { bar; } V1; V3 { baz; } V2 V1;");
    let parents: Vec<Vec<(String, usize)>> = script
        .nodes
        .iter()
        .map(|n| n.parents.iter().map(|p| (p.name.to_string_lossy(), p.node)).collect())
        .collect();
    assert_eq!(
        parents,
        [vec![], vec![("V1".to_owned(), 0)], vec![("V2".to_owned(), 1), ("V1".to_owned(), 0)]]
    );
    let d = vs_err(
        b"V2 { bar; } V1;\nV1 { foo; };",
        1,
        13,
        "parent version 'V1' is not a node defined before",
    );
    assert!(has_note(&d, "unable to find version dependency"));
    vs_err(b"V1 { foo; };\nV2 { bar; } V9;", 2, 13, "parent version 'V9'");
    vs_err(b"V1 { foo; }; V2 { bar; } V2;", 1, 26, "parent version 'V2'");
    vs_err(b"V1 { foo; }; V2 { bar; } \"V1\";", 1, 26, "quoted parent version name");
    vs_err(b"V1 { foo; }; V2 { bar; } V1 {", 1, 29, "a parent version name or ';'");
}

// spec: script.md §9.1 (a node name used twice: version-script-syntax)
#[test]
fn version_script_grammar_duplicate_node() {
    let d = vs_err(b"V1 { foo; };\nV1 { bar; };", 2, 1, "version node 'V1' is defined twice");
    assert!(has_note(&d, "duplicate version tag"));
}

// spec: script.md §9.1 (`"{" body "}" ";"`: one anonymous node, alone in
// the file; combined with named nodes, before or after them: refused)
#[test]
fn version_script_grammar_anonymous_node() {
    let script = vs("{ global: foo; local: *; };");
    assert!(script.is_anonymous());
    assert_eq!(shown(&script.nodes[0].entries), ["g foo", "l *"]);
    assert_eq!(script.nodes[0].span, Span::new(0, 27));
    let d = vs_err(
        b"{ global: foo; };\nV1 { global: bar; };",
        2,
        1,
        "anonymous version node cannot be combined",
    );
    assert!(has_note(&d, "anonymous version tag cannot be combined"));
    vs_err(
        b"V1 { global: foo; };\n{ global: bar; };",
        2,
        1,
        "anonymous version node cannot be combined",
    );
    vs_err(b"{ foo; }; { bar; };", 1, 11, "anonymous version node cannot be combined");
    vs_err(b"{ foo; } V1;", 1, 10, "';' after the anonymous node's '}'");
}

// spec: script.md §9.1 (`extern STRING { … } ;`: "C" plain names, "C++"
// demangled; any other language refused; the last `;` inside is optional,
// the `;` after the block is not; no nesting, no labels inside)
#[test]
fn version_script_grammar_extern_blocks() {
    let script = vs(
        "V1 { global: extern \"C\" { foo; \"bar\"; }; extern \"C++\" { \"foo(int)\"; ns::*; }; local: extern \"C++\" { *; }; plain; };",
    );
    assert_eq!(
        shown(&script.nodes[0].entries),
        ["g C:foo", "g C:\"bar\"", "g C++:\"foo(int)\"", "g C++:ns::*", "l C++:*", "l plain"]
    );
    assert_eq!(shown(&vs("V1 { extern \"C\" { foo }; };").nodes[0].entries), ["g C:foo"]);
    assert_eq!(
        shown(&vs("V1 { extern \"C\" { foo; bar }; };").nodes[0].entries),
        ["g C:foo", "g C:bar"]
    );
    assert_eq!(shown(&vs("V1 { extern \"C\" { }; foo; };").nodes[0].entries), ["g foo"]);
    vs_err(b"V1 { extern \"C\" { foo; } };", 1, 26, "';' after the extern block's '}'");
    vs_err(b"V1 { extern \"C\" { foo bar }; };", 1, 23, "';' or '}' after the entry");
    vs_err(b"V1 { extern \"C\" { foo;; }; };", 1, 23, "an entry or '}'");
    let d = vs_err(b"V1 { extern \"Java\" { foo; }; };", 1, 13, "unknown language \"Java\"");
    assert!(has_note(&d, "GNU ld accepts any string"));
    vs_err(b"V1 { extern \"c\" { foo; }; };", 1, 13, "unknown language \"c\"");
    vs_err(b"V1 { extern \"c++\" { foo; }; };", 1, 13, "unknown language \"c++\"");
    vs_err(b"V1 { extern C { foo; }; };", 1, 13, "expected ';'");
    vs_err(b"V1 { extern \"C++\" \"x\" { foo; }; };", 1, 19, "'{' after the extern language");
    let d =
        vs_err(b"V1 { extern \"C++\" { extern \"C\" { foo; }; }; };", 1, 21, "nested extern block");
    assert!(has_note(&d, "lld refuses it"));
    let d = vs_err(b"V1 { extern \"C\" { global: foo; }; };", 1, 25, "';' or '}' after the entry");
    assert!(has_note(&d, "not allowed inside an extern block"));
}

// spec: script.md §9.1 (`entry := pattern ";"`; the node's `;`; no empty
// statements; at least one node per file)
#[test]
fn version_script_grammar_terminators() {
    let d = vs_err(b"V1 { bar; foo };", 1, 15, "';' after the entry 'foo'");
    assert!(has_note(&d, "mold accepts a last entry without one"));
    vs_err(b"V1 { foo;; };", 1, 10, "an entry, 'global:', 'local:' or '}'");
    vs_err(b"V1 { foo; }", 1, 12, "a parent version name or ';'");
    vs_err(b"{ foo; }", 1, 9, "';' after the anonymous node's '}'");
    vs_err(b"V1 { foo; };;", 1, 13, "a version node name or '{'");
    vs_err(b";", 1, 1, "a version node name or '{'");
    vs_err(b"V1 { foo;", 1, 10, "'}' to close the block");
    vs_err(b"V1 foo;", 1, 4, "'{' after the version name");
    let d = vs_err(b"", 1, 1, "empty version script");
    assert!(has_note(&d, "mold accepts it"));
    vs_err(b"  # only a comment\n", 2, 1, "empty version script");
}

// spec: script.md §9.1 (node and parent names: the bytes GNU ld reads,
// `[A-Za-z_.$][A-Za-z0-9_.$]*`; others GNU ld drops while lld and mold keep
// them: refused; a quoted node name likewise)
#[test]
fn version_script_grammar_version_names() {
    assert_eq!(
        names(&vs("GLIBC_2.2.5 { a; }; .V { b; }; $V { c; }; ZLIB_1.2.0.2 { d; }; _ { e; };")),
        ["GLIBC_2.2.5", ".V", "$V", "ZLIB_1.2.0.2", "_"]
    );
    for (text, byte) in [
        ("V-1", "'-'"),
        ("V+1", "'+'"),
        ("V@1", "'@'"),
        ("V/1", "'/'"),
        ("V*", "'*'"),
        ("V?", "'?'"),
        ("V[1]", "'['"),
        ("V~1", "'~'"),
        ("V!1", "'!'"),
        ("V\\1", "'\\'"),
        ("V::1", "':'"),
    ] {
        let d = vs_err(format!("{text} {{ foo; }};").as_bytes(), 1, 1, &format!("holds {byte}"));
        assert!(has_note(&d, "GNU ld reads only letters"), "{text}");
    }
    vs_err(b"1V { foo; };", 1, 1, "starts with a digit");
    vs_err(b"1.0 { foo; };", 1, 1, "starts with a digit");
    vs_err(b"V1 { foo; }; V2 { bar; } V-1;", 1, 26, "holds '-'");
    let d = vs_err(b"\"V1\" { foo; };", 1, 1, "quoted version name");
    assert!(has_note(&d, "lld and mold keep them"));
}

// spec: script.md §9.3 (the zlib.map of `spec/dyn.md` record §A parses to
// 14 nodes and 13 parent links)
#[test]
fn version_script_grammar_zlib_map() {
    let script = vs(ZLIB_MAP);
    assert_eq!(script.nodes.len(), 14);
    let links: Vec<(usize, usize)> = script
        .nodes
        .iter()
        .enumerate()
        .flat_map(|(i, n)| n.parents.iter().map(move |p| (i, p.node)))
        .collect();
    assert_eq!(links.len(), 13);
    assert!(
        links.iter().all(|&(i, p)| p + 1 == i),
        "a chain: each node's parent is the one before"
    );
    let first = &script.nodes[0];
    assert_eq!(first.name.as_ref().unwrap().as_str(), Some("ZLIB_1.2.0"));
    assert_eq!(first.entries.iter().filter(|e| e.scope == Scope::Global).count(), 6);
    assert_eq!(first.entries.iter().filter(|e| e.scope == Scope::Local).count(), 10);
    assert_eq!(shown(&first.entries).last().map(String::as_str), Some("l _*"));
    // The 13 later nodes have no label: every entry is global.
    assert!(script.nodes[1..].iter().all(|n| n.entries.iter().all(|e| e.scope == Scope::Global)));
    assert_eq!(names(&script).last().map(String::as_str), Some("ZLIB_1.2.12"));
    assert_eq!(script.nodes.iter().map(|n| n.entries.len()).sum::<usize>(), 16 + 41);
}

// spec: script.md §9.1 (several `--version-script` files are one script,
// in order: a parent may be a node of an earlier file; `file` is the
// index; errors are placed in their own file)
#[test]
fn version_script_grammar_two_files_one_script() {
    let a = SourceFile { path: "a.map", bytes: b"V1 { global: foo; };\n" };
    let b = SourceFile { path: "b.map", bytes: b"V2 { global: bar; } V1;\n" };
    let script = parse_version_script(&[a, b]).unwrap();
    assert_eq!(names(&script), ["V1", "V2"]);
    assert_eq!((script.nodes[0].file, script.nodes[1].file), (0, 1));
    assert_eq!(script.nodes[1].span, Span::new(0, 23), "spans are offsets into the node's file");
    assert_eq!(script.nodes[1].parents[0].node, 0);
    // The reverse order: V1 is not yet defined.
    let d = parse_version_script(&[b, a]).unwrap_err();
    assert_eq!(
        said(&d),
        (
            "b.map".into(),
            1,
            21,
            "VERSION script".into(),
            "parent version 'V1' is not a node defined before this one".into()
        )
    );
    // A node twice across files; an anonymous node in one file, named in another.
    let d = parse_version_script(&[a, a]).unwrap_err();
    assert_eq!((said(&d).0, said(&d).1, said(&d).2), ("a.map".into(), 1, 1));
    assert!(said(&d).4.contains("defined twice"));
    let anon = SourceFile { path: "anon.map", bytes: b"{ local: *; };" };
    let d = parse_version_script(&[a, anon]).unwrap_err();
    assert_eq!(said(&d).0, "anon.map");
    let d = parse_version_script(&[anon, a]).unwrap_err();
    assert_eq!(said(&d).0, "a.map");
    // An empty file among several is refused.
    let empty = SourceFile { path: "e.map", bytes: b"" };
    assert_eq!(said(&parse_version_script(&[a, empty]).unwrap_err()).0, "e.map");
    assert_eq!(parse_version_script(&[]).unwrap(), VersionScript::default());
}

// spec: script.md §9.2 (a syntax error is `version-script-syntax` with
// file, line and column, and GNU ld's text)
#[test]
fn version_script_grammar_diagnostic_text() {
    let d = vs_err(b"V1 {\n  foo\n};", 3, 1, "");
    assert_eq!(
        render(&d),
        "ld: error: v.map:3:1: syntax error in VERSION script: expected ';' after the entry 'foo', found '}'\n\
         >>> every entry ends with ';' (GNU ld and lld; mold accepts a last entry without one)\n"
    );
}

// spec: script.md §9.1 (lexing in version mode does not change the other
// modes: `ENTRY(a::b)` and friends read as before)
#[test]
fn version_mode_leaves_other_modes_alone() {
    let mut lexer = Lexer::new(b"global: foo;");
    let t = lexer.next(Mode::Script).unwrap();
    assert_eq!(t.kind, TokenKind::Name);
    assert_eq!(t.span, Span::new(0, 7), "in Script mode ':' is a name byte");
    let mut lexer = Lexer::new(b"global: foo;");
    assert_eq!(lexer.next(Mode::Version).unwrap().span, Span::new(0, 6));
    assert_eq!(lexer.next(Mode::Version).unwrap().kind, TokenKind::Punct(Punct::Colon));
}

// ---- dynamic lists (§9.1 `dynamic-list`) -----------------------------

// spec: script.md §9.1 (`dynamic-list := "{" entry* "}" ";"`; every entry
// global; extern blocks)
#[test]
fn dynamic_list_grammar_entries() {
    let list =
        dl("{ foo; \"bar\"; ba*; extern \"C++\" { ns::*; \"f(int)\" }; extern \"C\" { c; }; };");
    assert_eq!(
        shown(&list.entries),
        ["g foo", "g \"bar\"", "g ba*", "g C++:ns::*", "g C++:\"f(int)\"", "g C:c"]
    );
    assert!(dl("{ };").entries.is_empty());
}

// spec: script.md §9.1 (comments in dynamic lists: `#` whole-line and
// trailing, `foo; #bar;` lists only `foo`; `/* */`; `//` an error)
#[test]
fn dynamic_list_grammar_comments() {
    let list = dl("# head\n{\n  foo; #bar;\n  /* block\n */ baz; # end\n};\n# tail\n");
    assert_eq!(shown(&list.entries), ["g foo", "g baz"]);
    dl_err(b"// x\n{ foo; };", 1, 1, "'//' starts no comment");
    dl_err(b"{ foo; // x\n};", 1, 8, "'//' starts no comment");
}

// spec: script.md §9.1 (a dynamic list has no labels, no name, one block
// per file; an empty file is refused)
#[test]
fn dynamic_list_grammar_refusals() {
    let d = dl_err(b"{ global: foo; };", 1, 3, "'global:' in a dynamic list");
    assert!(has_note(&d, "lld accepts 'global:'"));
    dl_err(b"{ local: foo; };", 1, 3, "'local:' in a dynamic list");
    let d = dl_err(b"{ foo; }; { bar; };", 1, 11, "end of file after the dynamic list");
    assert!(has_note(&d, "GNU ld accepts several"));
    dl_err(b"V1 { foo; };", 1, 1, "'{' to open the dynamic list");
    dl_err(b"", 1, 1, "empty dynamic list");
    dl_err(b"{ foo; }", 1, 9, "';' after the dynamic list's '}'");
    dl_err(b"{ bar; foo };", 1, 12, "';' after the entry 'foo'");
    dl_err(b"{ \"\"; };", 1, 3, "empty name");
    // `global` alone is still a name.
    assert_eq!(shown(&dl("{ global; };").entries), ["g global"]);
}

// spec: script.md §9.1 (several `--dynamic-list` files are one list, in
// order)
#[test]
fn dynamic_list_grammar_two_files_one_list() {
    let a = SourceFile { path: "a.list", bytes: b"{ foo; };" };
    let b = SourceFile { path: "b.list", bytes: b"{ bar; baz; };" };
    let list = parse_dynamic_list(&[a, b]).unwrap();
    assert_eq!(shown(&list.entries), ["g foo", "g bar", "g baz"]);
    let bad = SourceFile { path: "c.list", bytes: b"{ foo }" };
    assert_eq!(said(&parse_dynamic_list(&[a, bad]).unwrap_err()).0, "c.list");
    assert_eq!(parse_dynamic_list(&[]).unwrap(), DynamicList::default());
}

/// zlib 1.3.1's `zlib.map` (zlib license), the script of `spec/dyn.md`
/// record §A.
const ZLIB_MAP: &str = "ZLIB_1.2.0 {
  global:
    compressBound;
    deflateBound;
    inflateBack;
    inflateBackEnd;
    inflateBackInit_;
    inflateCopy;
  local:
    deflate_copyright;
    inflate_copyright;
    inflate_fast;
    inflate_table;
    zcalloc;
    zcfree;
    z_errmsg;
    gz_error;
    gz_intmax;
    _*;
};

ZLIB_1.2.0.2 {
    gzclearerr;
    gzungetc;
    zlibCompileFlags;
} ZLIB_1.2.0;

ZLIB_1.2.0.8 {
    deflatePrime;
} ZLIB_1.2.0.2;

ZLIB_1.2.2 {
    adler32_combine;
    crc32_combine;
    deflateSetHeader;
    inflateGetHeader;
} ZLIB_1.2.0.8;

ZLIB_1.2.2.3 {
    deflateTune;
    gzdirect;
} ZLIB_1.2.2;

ZLIB_1.2.2.4 {
    inflatePrime;
} ZLIB_1.2.2.3;

ZLIB_1.2.3.3 {
    adler32_combine64;
    crc32_combine64;
    gzopen64;
    gzseek64;
    gztell64;
    inflateUndermine;
} ZLIB_1.2.2.4;

ZLIB_1.2.3.4 {
    inflateReset2;
    inflateMark;
} ZLIB_1.2.3.3;

ZLIB_1.2.3.5 {
    gzbuffer;
    gzoffset;
    gzoffset64;
    gzclose_r;
    gzclose_w;
} ZLIB_1.2.3.4;

ZLIB_1.2.5.1 {
    deflatePending;
} ZLIB_1.2.3.5;

ZLIB_1.2.5.2 {
    deflateResetKeep;
    gzgetc_;
    inflateResetKeep;
} ZLIB_1.2.5.1;

ZLIB_1.2.7.1 {
    inflateGetDictionary;
    gzvprintf;
} ZLIB_1.2.5.2;

ZLIB_1.2.9 {
    inflateCodesUsed;
    inflateValidate;
    uncompress2;
    gzfread;
    gzfwrite;
    deflateGetDictionary;
    adler32_z;
    crc32_z;
} ZLIB_1.2.7.1;

ZLIB_1.2.12 {
\tcrc32_combine_gen;
\tcrc32_combine_gen64;
\tcrc32_combine_op;
} ZLIB_1.2.9;
";
