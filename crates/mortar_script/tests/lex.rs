//! `spec/script.md` §1: the lexer, mode by mode.

use mortar_script::lex::{Keyword, LexError, Lexer, Mode, Punct, TokenKind, parse_integer};
use mortar_script::{Span, position};

/// The tokens of `src` in one mode, rendered: `N:text`, `S:text`, `I:value`,
/// `K:kw`, `P:op`, `L:text`; or the error `E@offset:what` last.
fn toks(mode: Mode, src: &str) -> Vec<String> {
    let mut lexer = Lexer::new(src.as_bytes());
    let mut out = Vec::new();
    loop {
        match lexer.next(mode) {
            Ok(t) => {
                let text = String::from_utf8_lossy(t.span.of(src.as_bytes())).into_owned();
                out.push(match t.kind {
                    TokenKind::Eof => return out,
                    TokenKind::Name => format!("N:{text}"),
                    TokenKind::String => format!("S:{text}"),
                    TokenKind::Integer(v) => format!("I:{v}"),
                    TokenKind::Keyword(k) => format!("K:{}", k.spelling()),
                    TokenKind::Punct(p) => format!("P:{}", p.spelling()),
                    TokenKind::Library => format!("L:{text}"),
                });
            }
            Err(LexError { offset, what, .. }) => {
                out.push(format!("E@{offset}:{what}"));
                return out;
            }
        }
    }
}

fn err_at(mode: Mode, src: &str) -> usize {
    let last = toks(mode, src).pop().unwrap();
    let rest = last.strip_prefix("E@").unwrap_or_else(|| panic!("{src:?}: no error: {last}"));
    rest.split(':').next().unwrap().parse().unwrap()
}

// spec: script.md §1.1 — tokens carry byte offsets; line and column are
// derived, both from 1, the column in bytes, lines ended by LF.
#[test]
fn offsets_lines_and_columns() {
    let src = b"ENTRY(a)\n  INPUT(\xc3\xa9)\r\nx";
    let mut lexer = Lexer::new(src);
    let first = lexer.next(Mode::Script).unwrap();
    assert_eq!(first.span, Span::new(0, 5));
    assert_eq!(position(src, 0), (1, 1));
    assert_eq!(position(src, 11), (2, 3));
    assert_eq!(position(src, 9), (2, 1));
    // The CR is the last byte of line 2; `x` is on line 3.
    assert_eq!(position(src, src.len() - 1), (3, 1));
    // Past the end: the end.
    assert_eq!(position(src, 9999), (3, 2));
    assert_eq!(position(b"", 0), (1, 1));
}

// spec: script.md §1.3 — whitespace is space, tab, CR and LF in every mode.
#[test]
fn whitespace() {
    for mode in [Mode::Script, Mode::InputList, Mode::Expression] {
        assert_eq!(toks(mode, " \t\r\n a \n"), ["N:a"], "{mode:?}");
        // Form feed and vertical tab are not whitespace.
        assert!(toks(mode, "\x0ca")[0].starts_with("E@0:invalid character byte 0x0c"), "{mode:?}");
        assert!(toks(mode, "\x0ba")[0].starts_with("E@0:"), "{mode:?}");
    }
}

// spec: script.md §1.3 — `/* */` comments in every mode, not nested; an
// unterminated one is an error at its start; `//` is no comment.
#[test]
fn block_comments() {
    for mode in [Mode::Script, Mode::InputList, Mode::Expression] {
        assert_eq!(toks(mode, "/* x */a /**/ /* /* */ b"), ["N:a", "N:b"], "{mode:?}");
        assert_eq!(toks(mode, "/* a\n b */c"), ["N:c"]);
        assert_eq!(toks(mode, "a /* never"), ["N:a", "E@2:unterminated comment"]);
        assert_eq!(toks(mode, "/*/"), ["E@0:unterminated comment"]);
        assert_eq!(toks(mode, "/* \u{e9} */a"), ["N:a"], "UTF-8 in a comment");
        assert_eq!(toks(mode, "/* \0 */"), ["E@3:NUL byte in a comment"]);
    }
    // `//` is a name in SCRIPT mode, as in GNU ld.
    assert_eq!(toks(Mode::Script, "// c"), ["N://", "N:c"]);
    // A `/*` inside a name does not start a comment: the name takes the `/`.
    assert_eq!(toks(Mode::Script, "a/*b*/")[0], "N:a/");
}

// spec: script.md §1.3 — `#` to the end of the line is a comment in SCRIPT
// and EXPRESSION mode, and an error in INPUTLIST mode.
#[test]
fn hash_comments() {
    assert_eq!(toks(Mode::Script, "a # b ( c\nd"), ["N:a", "N:d"]);
    assert_eq!(toks(Mode::Script, "a#b"), ["N:a"]);
    assert_eq!(toks(Mode::Expression, "a # b\n c"), ["N:a", "N:c"]);
    assert_eq!(toks(Mode::Script, "# \0"), ["E@2:NUL byte in a comment"]);
    assert_eq!(toks(Mode::InputList, "a # b"), ["N:a", "E@2:invalid character '#'"]);
}

// spec: script.md §1.4 — SCRIPT names: first [A-Za-z_/.\\$~], then also
// digits and + : [ ] , = -.
#[test]
fn script_names() {
    assert_eq!(
        toks(Mode::Script, "a-b x=1 /lib/x.so.6 i386:x86-64 a,b ~a \\b .x $zz"),
        [
            "N:a-b",
            "N:x=1",
            "N:/lib/x.so.6",
            "N:i386:x86-64",
            "N:a,b",
            "N:~a",
            "N:\\b",
            "N:.x",
            "N:$zz"
        ]
    );
    assert_eq!(toks(Mode::Script, "a[0]+b"), ["N:a[0]+b"]);
    // Characters that start no name are tokens of their own.
    assert_eq!(toks(Mode::Script, "-a"), ["P:-", "N:a"]);
    assert_eq!(toks(Mode::Script, "=a"), ["P:=", "N:a"]);
    assert_eq!(toks(Mode::Script, ":a"), ["P::", "N:a"]);
    assert_eq!(toks(Mode::Script, "+a"), ["P:+", "N:a"]);
    // `$` alone is a name.
    assert_eq!(toks(Mode::Script, "$"), ["N:$"]);
}

// spec: script.md §1.4 — INPUTLIST names: the SCRIPT sets; `=name` and
// `-l<name chars>` are one token; no integers.
#[test]
fn input_list_names() {
    assert_eq!(
        toks(Mode::InputList, "a.o,b.o =/x.so =x -lfoo -l:bar.a -l=x $abc"),
        ["N:a.o,b.o", "N:=/x.so", "N:=x", "L:-lfoo", "L:-l:bar.a", "L:-l=x", "N:$abc"]
    );
    assert_eq!(toks(Mode::InputList, "a , b"), ["N:a", "P:,", "N:b"]);
    assert_eq!(toks(Mode::InputList, "(AS_NEEDED)"), ["P:(", "K:AS_NEEDED", "P:)"]);
    assert_eq!(err_at(Mode::InputList, "= x.so"), 0);
    assert_eq!(err_at(Mode::InputList, "-l foo"), 0);
    assert_eq!(err_at(Mode::InputList, "-L/x"), 0);
    assert_eq!(err_at(Mode::InputList, "a 1.o"), 2);
    assert_eq!(err_at(Mode::InputList, "a.o;b.o"), 3);
    assert_eq!(err_at(Mode::InputList, "{"), 0);
}

// spec: script.md §1.4 — EXPRESSION names: first [A-Za-z_.\\$], then also
// digits, / and ~; `-`, `,`, `:`, `+`, `=` end a name.
#[test]
fn expression_names() {
    assert_eq!(
        toks(Mode::Expression, "a.b a$b a/b a~b a\\b _x .y \\z $ $g"),
        ["N:a.b", "N:a$b", "N:a/b", "N:a~b", "N:a\\b", "N:_x", "N:.y", "N:\\z", "N:$", "N:$g"]
    );
    assert_eq!(toks(Mode::Expression, "a-b"), ["N:a", "P:-", "N:b"]);
    assert_eq!(toks(Mode::Expression, "a,b"), ["N:a", "P:,", "N:b"]);
    assert_eq!(toks(Mode::Expression, "a:b"), ["N:a", "P::", "N:b"]);
    assert_eq!(toks(Mode::Expression, "/a"), ["P:/", "N:a"]);
    assert_eq!(toks(Mode::Expression, "~a"), ["P:~", "N:a"]);
}

// spec: script.md §1.5 — the integer forms and their values.
#[test]
fn integer_values() {
    let cases: &[(&str, u64)] = &[
        ("0", 0),
        ("10", 10),
        ("010", 8),
        ("0x10", 16),
        ("0X1f", 31),
        ("$10", 16),
        ("$ff", 255),
        ("1K", 1024),
        ("1k", 1024),
        ("2M", 2 << 20),
        ("2m", 2 << 20),
        ("0x10K", 0x4000),
        ("010K", 0x2000),
        ("$10K", 0x4000),
        ("10h", 16),
        ("10H", 16),
        ("10x", 16),
        ("10X", 16),
        ("10o", 8),
        ("10O", 8),
        ("10b", 2),
        ("10B", 2),
        ("10d", 10),
        ("10D", 10),
        ("0ffh", 255),
        ("ffh", 255),
        ("FFH", 255),
        ("18446744073709551615", u64::MAX),
        ("0xffffffffffffffff", u64::MAX),
    ];
    for &(text, value) in cases {
        assert_eq!(parse_integer(text.as_bytes()), Ok(value), "{text}");
    }
}

// spec: script.md §1.5 — what GNU ld truncates or saturates silently is an
// error, and so is every other malformed form.
#[test]
fn integer_errors() {
    for text in [
        "08",
        "019",
        "0778",
        "08K",
        "18o",
        "12b",
        "0x",
        "$",
        "0x10h",
        "10bK",
        "1KK",
        "1MK",
        "1e3",
        "1a",
        "0x1g",
        "18446744073709551616",
        "0x1ffffffffffffffff",
        "17592186044416M",
        "10KB",
    ] {
        let e = parse_integer(text.as_bytes()).expect_err(text);
        assert!(e.starts_with(&format!("malformed number '{text}'")), "{text}: {e}");
    }
}

// spec: script.md §1.5 — where integers are tokens: after a digit in SCRIPT
// and EXPRESSION mode; `$` + hex digits unless a longer name; in
// EXPRESSION mode also hex digits + one of h H x X; never in INPUTLIST.
#[test]
fn integer_tokens_by_mode() {
    assert_eq!(toks(Mode::Script, "123 $ab $10K"), ["I:123", "I:171", "I:16384"]);
    assert_eq!(toks(Mode::Script, "$ab_ $1g"), ["N:$ab_", "N:$1g"]);
    // The hex-suffix form needs a digit first in SCRIPT mode.
    assert_eq!(toks(Mode::Script, "ffh ah"), ["N:ffh", "N:ah"]);
    assert_eq!(
        toks(Mode::Expression, "ah BX ffx C6X abh"),
        ["I:10", "I:11", "I:255", "I:198", "I:171"]
    );
    // ... but not with any other last letter, and not when the name goes on.
    assert_eq!(
        toks(Mode::Expression, "ab add bad fb fd ao aB ahx gh g1h"),
        ["N:ab", "N:add", "N:bad", "N:fb", "N:fd", "N:ao", "N:aB", "N:ahx", "N:gh", "N:g1h"]
    );
    assert_eq!(toks(Mode::Expression, "1+2"), ["I:1", "P:+", "I:2"]);
    assert_eq!(toks(Mode::InputList, "$ab"), ["N:$ab"]);
    // A malformed number is an error where it starts.
    assert_eq!(
        toks(Mode::Script, "a 08"),
        ["N:a", "E@2:malformed number '08': '8' is not an octal digit"]
    );
    assert_eq!(err_at(Mode::Expression, "x 1a"), 2);
}

// spec: script.md §1.6 — quoted strings: no escapes, may hold newlines and
// any byte but NUL; unterminated is an error at the opening quote.
#[test]
fn strings() {
    for mode in [Mode::Script, Mode::InputList, Mode::Expression] {
        assert_eq!(toks(mode, r#""a b" "" "a\" "x"#)[..3], [r#"S:"a b""#, r#"S:"""#, r#"S:"a\""#]);
        assert_eq!(toks(mode, "\"a\nb\""), ["S:\"a\nb\""]);
        assert_eq!(toks(mode, "a \"b"), ["N:a", "E@2:unterminated string"]);
        assert_eq!(toks(mode, "\"\0\""), ["E@1:NUL byte in a string"]);
        // Quoting defeats keywords.
        assert_eq!(toks(mode, "\"ENTRY\" \"AS_NEEDED\""), ["S:\"ENTRY\"", "S:\"AS_NEEDED\""]);
    }
    let src = b"\"\xff\xfe\"";
    assert_eq!(Lexer::new(src).next(Mode::Script).unwrap().kind, TokenKind::String);
}

// spec: script.md §1.7 — keyword sets: SCRIPT, EXPRESSION, INPUTLIST, case
// sensitive.
#[test]
fn keywords_by_mode() {
    let kw = |mode, text: &str| Keyword::lookup(mode, text.as_bytes());
    assert_eq!(kw(Mode::Script, "ENTRY"), Some(Keyword::ENTRY));
    assert_eq!(kw(Mode::Script, "entry"), None);
    for word in ["l", "len", "o", "org", "SECTIONS", "AFTER", "INCLUDE", "HLL"] {
        assert!(kw(Mode::Script, word).is_some(), "{word}");
    }
    for word in ["AS_NEEDED", "LIB", "COPY", "NOLOAD", "KEEP", "SORT", "sizeof_headers", "lenx"] {
        assert_eq!(kw(Mode::Script, word), None, "{word}");
    }
    for word in ["SIZEOF", "COPY", "TYPE", "INCLUDE", "PROVIDE"] {
        assert!(kw(Mode::Expression, word).is_some(), "{word}");
    }
    for word in ["ENTRY", "INPUT", "GROUP", "EXTERN", "len", "AFTER"] {
        assert_eq!(kw(Mode::Expression, word), None, "{word}");
    }
    assert_eq!(kw(Mode::InputList, "AS_NEEDED"), Some(Keyword::AS_NEEDED));
    for word in ["INPUT", "GROUP", "INCLUDE", "ENTRY"] {
        assert_eq!(kw(Mode::InputList, word), None, "{word}");
    }
    // Every keyword is recognized in at least one mode.
    for k in Keyword::ALL {
        assert!(
            [Mode::Script, Mode::InputList, Mode::Expression].iter().any(|&m| k.in_mode(m)),
            "{k:?}"
        );
    }
    assert_eq!(toks(Mode::Script, "lenx len"), ["N:lenx", "K:len"]);
}

// spec: script.md §1.8 — operators and separators of SCRIPT mode,
// including the assignment operators and the `/=` tie.
#[test]
fn script_operators() {
    assert_eq!(
        toks(Mode::Script, "( ) { } ; , : ! > = + -"),
        ["P:(", "P:)", "P:{", "P:}", "P:;", "P:,", "P::", "P:!", "P:>", "P:=", "P:+", "P:-"]
    );
    assert_eq!(
        toks(Mode::Script, "+= -= *= /= <<= >>= &= |= ^="),
        ["P:+=", "P:-=", "P:*=", "P:/=", "P:<<=", "P:>>=", "P:&=", "P:|=", "P:^="]
    );
    for p in [Punct::PlusAssign, Punct::Assign, Punct::ShlAssign] {
        assert!(p.is_assignment());
    }
    assert!(!Punct::EqEq.is_assignment());
    // A longer name beats the operator.
    assert_eq!(toks(Mode::Script, "/=x"), ["N:/=x"]);
    for c in ["<", "*", "&", "|", "^", "%", "?", "@", "`", "'"] {
        assert_eq!(err_at(Mode::Script, c), 0, "{c}");
    }
}

// spec: script.md §1.8 — operators of EXPRESSION mode.
#[test]
fn expression_operators() {
    assert_eq!(
        toks(Mode::Expression, "a<<=b>>=c<=d>=e==f!=g&&h||i"),
        [
            "N:a", "P:<<=", "N:b", "P:>>=", "N:c", "P:<=", "N:d", "P:>=", "N:e", "P:==", "N:f",
            "P:!=", "N:g", "P:&&", "N:h", "P:||", "N:i"
        ]
    );
    assert_eq!(
        toks(Mode::Expression, "? : ~ ! % * / & | ^ < > ="),
        ["P:?", "P::", "P:~", "P:!", "P:%", "P:*", "P:/", "P:&", "P:|", "P:^", "P:<", "P:>", "P:="]
    );
    for c in ["@", "[", "]", "`", "'"] {
        assert_eq!(err_at(Mode::Expression, c), 0, "{c}");
    }
}

// spec: script.md §1.9 — bytes outside ASCII and control characters are
// invalid outside comments and strings, in every mode.
#[test]
fn invalid_bytes() {
    for mode in [Mode::Script, Mode::InputList, Mode::Expression] {
        for src in [&b"\xc3\xa9"[..], b"\x80", b"\x00", b"\x01", b"\x7f"] {
            let mut lexer = Lexer::new(src);
            let e = lexer.next(mode).expect_err("invalid");
            assert_eq!(e.offset, 0);
            assert!(e.what.starts_with("invalid character byte 0x"), "{}", e.what);
        }
    }
}

// spec: script.md §1.2 — the mode is chosen per token, so the same bytes
// lex differently at a mode boundary.
#[test]
fn mode_boundaries() {
    let src = b"a,b";
    let mut lexer = Lexer::new(src);
    // SCRIPT: one name.
    assert_eq!(lexer.clone().next(Mode::Script).unwrap().span, Span::new(0, 3));
    // EXPRESSION: name, comma, name.
    assert_eq!(lexer.next(Mode::Expression).unwrap().span, Span::new(0, 1));
    assert_eq!(lexer.next(Mode::Script).unwrap().kind, TokenKind::Punct(Punct::Comma));
    assert_eq!(lexer.next(Mode::InputList).unwrap().span, Span::new(2, 3));
    assert_eq!(lexer.next(Mode::InputList).unwrap().kind, TokenKind::Eof);
    assert_eq!(toks(Mode::Script, "=x"), ["P:=", "N:x"]);
    assert_eq!(toks(Mode::InputList, "=x"), ["N:=x"]);
    assert_eq!(toks(Mode::Script, "-lc"), ["P:-", "N:lc"]);
    assert_eq!(toks(Mode::InputList, "-lc"), ["L:-lc"]);
}

// spec: script.md §1.1 — the end of input is a token with an empty span at
// the end, returned again on every later call.
#[test]
fn eof_repeats() {
    let mut lexer = Lexer::new(b" a ");
    lexer.next(Mode::Script).unwrap();
    for _ in 0..3 {
        let t = lexer.next(Mode::Script).unwrap();
        assert_eq!((t.kind, t.span), (TokenKind::Eof, Span::new(3, 3)));
    }
}
