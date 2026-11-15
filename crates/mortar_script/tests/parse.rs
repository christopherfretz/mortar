//! `spec/script.md` §2: the commands of implicit scripts, refusals and
//! syntax errors.

mod common;

use common::{malformed, ok, refused, summary};
use mortar_script::{
    ASSIGNMENT, CommandKind, FileForm, InputItem, MAX_AS_NEEDED_DEPTH, Span, parse,
};

// spec: script.md §2.2 — statements in sequence; `;` anywhere between them;
// the empty script.
#[test]
fn statements_and_empty_statements() {
    assert!(ok("").commands.is_empty());
    assert!(ok(" ;;; /* c */ ;").commands.is_empty());
    let s = ok("ENTRY(a);;;ENTRY(b) ENTRY(c)\n;");
    assert_eq!(summary(&s), ["ENTRY(a)", "ENTRY(b)", "ENTRY(c)"]);
    // Spans: keyword to closing parenthesis.
    assert_eq!(s.commands[0].span, Span::new(0, 8));
    assert_eq!(s.commands[1].span, Span::new(11, 19));
}

// spec: script.md §2.3 — INPUT and GROUP lists: whitespace or one comma
// between entries, AS_NEEDED nested, order kept.
#[test]
fn input_and_group() {
    let s = ok("INPUT(a.o b.o) GROUP ( x.a , y.a\n z.a ) INPUT(a.o ,b.o)");
    assert_eq!(summary(&s), ["INPUT(a.o b.o)", "GROUP(x.a y.a z.a)", "INPUT(a.o b.o)"]);
    let s = ok("GROUP ( /lib/libc.so.6 /usr/lib/libc_nonshared.a  AS_NEEDED ( /lib/ld.so.1 ) )");
    assert_eq!(
        summary(&s),
        ["GROUP(/lib/libc.so.6 /usr/lib/libc_nonshared.a AS_NEEDED(/lib/ld.so.1))"]
    );
    let CommandKind::Group(items) = &s.commands[0].kind else { panic!() };
    let InputItem::AsNeeded { span, .. } = &items[2] else { panic!() };
    assert_eq!(*span, Span::new(50, 76));
    // AS_NEEDED nests in itself and may be first, last or alone.
    assert_eq!(
        summary(&ok("INPUT(AS_NEEDED(AS_NEEDED(x.so) y.so))")),
        ["INPUT(AS_NEEDED(AS_NEEDED(x.so) y.so))"]
    );
    assert_eq!(summary(&ok("INPUT(AS_NEEDED (a), b)")), ["INPUT(AS_NEEDED(a) b)"]);
    // Keywords of other modes are file names in a list.
    assert_eq!(
        summary(&ok("INPUT(GROUP INCLUDE ENTRY SECTIONS)")),
        ["INPUT(GROUP INCLUDE ENTRY SECTIONS)"]
    );
    // `#` is a comment after the list closes, not inside it.
    assert_eq!(summary(&ok("INPUT(a) # c\n")), ["INPUT(a)"]);
}

// spec: script.md §2.3 — STARTUP takes one file name.
#[test]
fn startup() {
    assert_eq!(summary(&ok("STARTUP(crt0.o)")), ["STARTUP(crt0.o)"]);
    malformed(b"STARTUP(a b)", 1, 11, "expected ')'");
}

// spec: script.md §2.3 — the list grammar's errors: empty lists, stray and
// doubled commas, unclosed lists, AS_NEEDED without a list, nesting depth.
#[test]
fn input_list_errors() {
    malformed(b"INPUT()", 1, 7, "expected a file name in INPUT");
    malformed(b"GROUP(AS_NEEDED())", 1, 17, "expected a file name in AS_NEEDED");
    malformed(b"INPUT(,a)", 1, 7, "found ','");
    malformed(b"INPUT(a , , b)", 1, 11, "found ','");
    malformed(b"INPUT(a ,)", 1, 10, "found ')'");
    malformed(b"INPUT(a\n b", 2, 3, "expected ')' to close INPUT");
    malformed(b"INPUT(AS_NEEDED a)", 1, 17, "expected '(' after AS_NEEDED");
    malformed(b"INPUT a", 1, 7, "expected '(' after INPUT");
    malformed(b"INPUT(a (b))", 1, 9, "found '('");
    let deep = |n: usize| format!("INPUT({}x{})", "AS_NEEDED(".repeat(n), ")".repeat(n));
    assert!(parse(deep(MAX_AS_NEEDED_DEPTH).as_bytes(), "t.ld").is_ok());
    let at = 7 + MAX_AS_NEEDED_DEPTH * 10;
    malformed(deep(MAX_AS_NEEDED_DEPTH + 1).as_bytes(), 1, at as u32, "nested deeper");
}

// spec: script.md §2.4 — the file forms: path, `=`/`$SYSROOT` (sysroot),
// `-lname`, `-l:file`, quoted; spelling kept.
#[test]
fn file_forms() {
    let s = ok(
        r#"INPUT(/abs/a.o rel/b.o =/usr/lib/c.so =d.so $SYSROOT/e.a -lm -l:libx.a "q r.o" "=/s.o")"#,
    );
    assert_eq!(
        summary(&s),
        [r#"INPUT(/abs/a.o rel/b.o =/usr/lib/c.so =d.so =/e.a -lm -l:libx.a "q r.o" "=/s.o")"#]
    );
    let CommandKind::Input(items) = &s.commands[0].kind else { panic!() };
    let file = |i: usize| match &items[i] {
        InputItem::File(f) => f,
        InputItem::AsNeeded { .. } => panic!(),
    };
    assert_eq!(file(2).form, FileForm::Sysroot);
    assert_eq!(file(2).name, b"/usr/lib/c.so");
    assert_eq!(file(2).spelling.text, b"=/usr/lib/c.so");
    assert_eq!(file(4).spelling.text, b"$SYSROOT/e.a");
    assert_eq!((file(5).form, &file(5).name[..]), (FileForm::Library, &b"m"[..]));
    assert_eq!((file(6).form, &file(6).name[..]), (FileForm::LibraryVerbatim, &b"libx.a"[..]));
    assert!(file(7).spelling.quoted && file(7).form == FileForm::Path);
    assert_eq!(file(7).spelling.span, Span::new(71, 78));
    // Names may contain `=`, `+`, `:`, `[`, `]`, `-` after their first byte.
    assert_eq!(summary(&ok("INPUT(a=b c+d e:f g[1] h-i)")), ["INPUT(a=b c+d e:f g[1] h-i)"]);
}

// spec: script.md §2.4 — spellings the reference linkers read differently
// are errors, located, with the remedy in a note.
#[test]
fn file_form_errors() {
    // A comma glued to a name: GNU ld keeps it in the name, lld and mold split.
    malformed(b"INPUT(a.o,b.o)", 1, 10, "',' inside an unquoted name");
    malformed(b"INPUT(a.o, b.o)", 1, 10, "',' inside an unquoted name");
    malformed(b"INPUT(-lfoo,)", 1, 12, "',' inside an unquoted name");
    // GNU ld skips a leading digit and other stray bytes.
    malformed(b"INPUT(1.o)", 1, 7, "starts with the digit '1'");
    malformed(b"INPUT(a.o;b.o)", 1, 10, "invalid character ';'");
    malformed(b"INPUT(a.o # c\n)", 1, 11, "invalid character '#'");
    malformed(b"INPUT(a.o/*c*/b.o)", 1, 11, "invalid character '*'");
    malformed(b"INPUT(-x)", 1, 7, "invalid character '-'");
    malformed(b"INPUT(-l foo)", 1, 7, "'-l' without a library name");
    malformed(b"INPUT(-l=foo)", 1, 7, "'-l=' is not supported");
    malformed(b"INPUT(-l:)", 1, 7, "'-l:' without a file name");
    malformed(b"INPUT(\"-lfoo\")", 1, 7, "starts with '-l'");
    malformed(b"INPUT(= a)", 1, 7, "'=' without a file name");
    malformed(b"INPUT(\"=\")", 1, 7, "'=' without a file name");
    malformed(b"INPUT($SYSROOTx)", 1, 7, "'$SYSROOT' not followed by '/'");
    malformed(b"INPUT(\"\")", 1, 7, "empty name");
    // Quoted, a comma is part of the name.
    assert_eq!(summary(&ok("INPUT(\"a,b\")")), ["INPUT(\"a,b\")"]);
}

// spec: script.md §2.5 — OUTPUT_FORMAT with one or three names, unquoted
// or quoted; OUTPUT_ARCH; TARGET.
#[test]
fn formats_and_architectures() {
    let s = ok(concat!(
        "OUTPUT_FORMAT(elf64-littleaarch64)\n",
        "OUTPUT_FORMAT(\"elf64-littleaarch64\", \"elf64-bigaarch64\", \"elf64-littleaarch64\")\n",
        "OUTPUT_FORMAT(elf64-x86-64 , elf64-x86-64 , elf64-x86-64)\n",
        "OUTPUT_ARCH(i386:x86-64) OUTPUT_ARCH(aarch64) TARGET(elf32-i386)",
    ));
    assert_eq!(
        summary(&s),
        [
            "OUTPUT_FORMAT(elf64-littleaarch64)",
            "OUTPUT_FORMAT(elf64-littleaarch64,elf64-bigaarch64,elf64-littleaarch64)",
            "OUTPUT_FORMAT(elf64-x86-64,elf64-x86-64,elf64-x86-64)",
            "OUTPUT_ARCH(i386:x86-64)",
            "OUTPUT_ARCH(aarch64)",
            "TARGET(elf32-i386)",
        ]
    );
    malformed(b"OUTPUT_FORMAT()", 1, 15, "expected a format name");
    malformed(b"OUTPUT_FORMAT(\"a\", \"b\")", 1, 23, "expected ','");
    malformed(b"OUTPUT_FORMAT(a b c)", 1, 17, "expected ',' or ')'");
    // Unquoted, `a, b` glues the comma to `a` (GNU ld: a syntax error) and
    // `a,b,c` is one name to GNU ld and three to lld.
    malformed(b"OUTPUT_FORMAT(a, b, c)", 1, 16, "',' inside an unquoted name");
    malformed(b"OUTPUT_FORMAT(a,b,c)", 1, 16, "',' inside an unquoted name");
}

// spec: script.md §2.6 — SEARCH_DIR takes one path; the ldscripts' quoted
// `=` form keeps its spelling.
#[test]
fn search_dir() {
    let s = ok("SEARCH_DIR(\"=/usr/lib\"); SEARCH_DIR(/opt/lib)");
    assert_eq!(summary(&s), ["SEARCH_DIR(=/usr/lib)", "SEARCH_DIR(/opt/lib)"]);
    let CommandKind::SearchDir(w) = &s.commands[0].kind else { panic!() };
    assert!(w.quoted);
    // Unquoted `=` is an operator at statement level, as in GNU ld.
    malformed(b"SEARCH_DIR(=/lib)", 1, 12, "found '='");
    malformed(b"SEARCH_DIR(a b)", 1, 14, "expected ')'");
}

// spec: script.md §2.7 — OUTPUT, ENTRY, EXTERN.
#[test]
fn output_entry_extern() {
    let s = ok(
        "OUTPUT(a.out) ENTRY(_start) ENTRY(\"x y\") ENTRY(a=b) EXTERN(a b, c ,d \"SIZEOF\" ENTRY)",
    );
    assert_eq!(
        summary(&s),
        [
            "OUTPUT(a.out)",
            "ENTRY(_start)",
            "ENTRY(x y)",
            "ENTRY(a=b)",
            "EXTERN(a b c d SIZEOF ENTRY)",
        ]
    );
    malformed(b"ENTRY()", 1, 7, "expected the argument of ENTRY");
    malformed(b"ENTRY(a b)", 1, 9, "expected ')' after the argument of ENTRY");
    malformed(b"ENTRY a", 1, 7, "expected '(' after ENTRY");
    malformed(b"ENTRY(a,b)", 1, 8, "',' inside an unquoted name");
    malformed(b"ENTRY(123)", 1, 7, "found number '123'");
    malformed(b"ENTRY(ALIGN)", 1, 7, "found keyword 'ALIGN'");
    malformed(b"ENTRY(len)", 1, 7, "found keyword 'len'");
    malformed(b"ENTRY(\"\")", 1, 7, "empty name");
    malformed(b"OUTPUT(a", 1, 9, "found end of file");
    malformed(b"EXTERN()", 1, 8, "expected a symbol name in EXTERN");
    malformed(b"EXTERN(a,,b)", 1, 10, "found ','");
    malformed(b"EXTERN(,a)", 1, 8, "found ','");
    malformed(b"EXTERN(a,)", 1, 10, "found ')'");
    malformed(b"EXTERN(a-b)", 1, 9, "found '-'");
    malformed(b"EXTERN(SIZEOF)", 1, 8, "found keyword 'SIZEOF'");
    // The hex trap: `BX` and `ah` are numbers in an expression.
    let d = malformed(b"EXTERN(a BX)", 1, 10, "found number 'BX'");
    assert!(format!("{:?}", d.context()).contains("quote it"));
    malformed(b"EXTERN(ah)", 1, 8, "found number 'ah'");
    assert_eq!(summary(&ok("EXTERN(add ab fb \"ah\")")), ["EXTERN(add ab fb ah)"]);
}

// spec: script.md §2.8 — INCLUDE takes a name without parentheses; `;`
// after it is the empty statement.
#[test]
fn include() {
    assert_eq!(
        summary(&ok("INCLUDE inc.ld; INCLUDE \"b c.ld\" INCLUDE /x/y.ld")),
        ["INCLUDE inc.ld", "INCLUDE b c.ld", "INCLUDE /x/y.ld"]
    );
    malformed(b"INCLUDE(x)", 1, 8, "expected a file name after INCLUDE");
    malformed(b"INCLUDE", 1, 8, "found end of file");
}

// spec: script.md §2.9 — what starts no command is an error at its first
// byte; line and column, also at the end of the file.
#[test]
fn syntax_errors_are_located() {
    malformed(b"FROB(1)", 1, 1, "unknown linker script command name 'FROB'");
    malformed(b"\n\n  entry(_start)", 3, 3, "unknown linker script command name 'entry'");
    let d = malformed(b"x=1;", 1, 1, "unknown linker script command name 'x=1'");
    assert!(format!("{:?}", d.context()).contains("put spaces around"));
    let d = malformed(b"AS_NEEDED(a)", 1, 1, "unknown linker script command");
    assert!(format!("{:?}", d.context()).contains("only inside the list of INPUT or GROUP"));
    malformed(b"ALIGN(4)", 1, 1, "keyword 'ALIGN' cannot start a linker script command");
    malformed(b"len = 1;", 1, 1, "keyword 'len' cannot start");
    malformed(b"INPUT(a.o)junk", 1, 11, "unknown linker script command name 'junk'");
    malformed(b"ENTRY(a) )", 1, 10, "expected a linker script command, found ')'");
    malformed(b"ENTRY(a) 5", 1, 10, "found number '5'");
    malformed(b"// c\nENTRY(a)", 1, 1, "name '//'");
    malformed(b"ENTRY(a) @", 1, 10, "invalid character '@'");
    malformed(b"ENTRY(a)\n/* x", 2, 1, "unterminated comment");
    malformed(b"ENTRY(\"a)", 1, 7, "unterminated string");
    malformed(b"ENTRY(a)\n\xc3\xa9", 2, 1, "invalid character byte 0xc3");
    malformed(b"ENTRY(a)\0", 1, 9, "invalid character byte 0x00");
    malformed(b"\x0c;", 1, 1, "invalid character byte 0x0c");
}

// spec: script.md §2.10 — every statement-level command GNU ld 2.44 accepts
// and stage 1 does not implement is refused by name, at its keyword.
#[test]
fn refused_commands() {
    let cases: &[(&str, &str, u32, u32)] = &[
        ("SECTIONS { .text : { *(.text) } }", "SECTIONS", 1, 1),
        ("ENTRY(a)\n  MEMORY { ram : ORIGIN = 0, LENGTH = 1M }", "MEMORY", 2, 3),
        ("PHDRS { text PT_LOAD; }", "PHDRS", 1, 1),
        ("VERSION { V1 { global: *; }; }", "VERSION", 1, 1),
        ("PROVIDE(foo = 1);", "PROVIDE", 1, 1),
        ("PROVIDE_HIDDEN(foo = 1);", "PROVIDE_HIDDEN", 1, 1),
        ("HIDDEN(foo = 1);", "HIDDEN", 1, 1),
        ("ASSERT(1, \"m\");", "ASSERT", 1, 1),
        ("INSERT AFTER .text;", "INSERT", 1, 1),
        ("NOCROSSREFS(.text .data);", "NOCROSSREFS", 1, 1),
        ("NOCROSSREFS_TO(.text .data);", "NOCROSSREFS_TO", 1, 1),
        ("REGION_ALIAS(\"r\", ram);", "REGION_ALIAS", 1, 1),
        ("LD_FEATURE(\"SANE_EXPR\")", "LD_FEATURE", 1, 1),
        ("FORCE_COMMON_ALLOCATION", "FORCE_COMMON_ALLOCATION", 1, 1),
        ("INHIBIT_COMMON_ALLOCATION", "INHIBIT_COMMON_ALLOCATION", 1, 1),
        ("FORCE_GROUP_ALLOCATION", "FORCE_GROUP_ALLOCATION", 1, 1),
        ("HLL(a)", "HLL", 1, 1),
        ("SYSLIB(a)", "SYSLIB", 1, 1),
        ("FLOAT", "FLOAT", 1, 1),
        ("NOFLOAT", "NOFLOAT", 1, 1),
        ("MAP(x)", "MAP", 1, 1),
        ("LIB(a.o)", "LIB", 1, 1),
        ("foo = 1;", ASSIGNMENT, 1, 1),
        ("ENTRY(a); . = 0x1000;", ASSIGNMENT, 1, 11),
        ("\"x\" = 1;", ASSIGNMENT, 1, 1),
        ("a-b = 1;", ASSIGNMENT, 1, 1),
        ("x += 1;", ASSIGNMENT, 1, 1),
        ("x -= 1;", ASSIGNMENT, 1, 1),
        ("x *= 1;", ASSIGNMENT, 1, 1),
        ("x /= 1;", ASSIGNMENT, 1, 1),
        ("x <<= 1;", ASSIGNMENT, 1, 1),
        ("x >>= 1;", ASSIGNMENT, 1, 1),
        ("x &= 1;", ASSIGNMENT, 1, 1),
        ("x |= 1;", ASSIGNMENT, 1, 1),
        ("x ^= 1;", ASSIGNMENT, 1, 1),
        ("x =1;", ASSIGNMENT, 1, 1),
    ];
    for &(src, name, line, column) in cases {
        refused(src, name, line, column);
    }
    // A refusal ends the parse: nothing after it is read, also no error.
    refused("SECTIONS { @@@ } INPUT(", "SECTIONS", 1, 1);
}

// spec: script.md §4 — the AST is plain data: Clone, PartialEq, Debug, and
// equal for equal bytes.
#[test]
fn ast_is_plain_data() {
    let a = ok("GROUP(a AS_NEEDED(b)) ENTRY(c)");
    let b = a.clone();
    assert_eq!(a, b);
    assert_ne!(a, ok("GROUP(a AS_NEEDED(b)) ENTRY(d)"));
    assert!(format!("{a:?}").contains("AsNeeded"));
    assert_eq!(a.commands[0].kind.keyword(), "GROUP");
}
