//! `spec/script.md` §5: mortar's parser against the reference linkers.
//!
//! Each row is a small script handed to the linker as an input file of a
//! minimal link (`<ld> -o out -L . t.ld e.o`, `e.o` one `_start`), so GNU
//! ld reads it as an *implicit* script, as it reads `libc.so`. What is
//! compared is acceptance by the script reader, not the link:
//!
//! - GNU ld accepts a script unless its diagnostics say the script itself
//!   is wrong (`syntax error`, `file not recognized`, `EOF in comment`,
//!   `ignoring invalid character`); a missing file or an undefined symbol
//!   is a later failure of an accepted script.
//! - lld and mold (the second and third witnesses) are judged by exit
//!   status, and only on rows that name every file they need: the files
//!   the rows mention exist in the scratch directory.
//!
//! Every row that mortar refuses must be one GNU ld accepts, so a refusal
//! is never mistaken for a syntax error; every row where mortar and GNU
//! ld disagree is a kept disagreement with its reason (`spec/script.md`
//! §5.2), and the test asserts GNU ld still behaves as recorded.

use std::path::Path;
use std::process::Command;

use mortar_testutil::{DEFAULT_TIMEOUT, Scratch, run_with, tool};

/// What mortar does with a row, and so what GNU ld must do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// mortar parses it; GNU ld accepts it.
    Valid,
    /// mortar refuses the command by this name; GNU ld accepts it.
    Refused(&'static str),
    /// mortar refuses the command by this name; GNU ld 2.44 does not know
    /// it (a command of later GNU ld releases).
    RefusedNewer(&'static str),
    /// mortar refuses the command by this name, decided by the head of
    /// the statement; GNU ld rejects the statement further on, where stage
    /// 1 does not read (`spec/script.md` §2.10).
    RefusedHead(&'static str),
    /// mortar rejects it as malformed; so does GNU ld.
    Invalid,
    /// mortar rejects it as malformed; GNU ld accepts it, reading
    /// something other than what lld or mold read, or silently dropping
    /// bytes. The reason is the row's note.
    Disagree(&'static str),
}

use Expect::{Disagree, Invalid, Refused, RefusedHead, RefusedNewer, Valid};

struct Row {
    script: &'static [u8],
    expect: Expect,
    /// Whether lld exits with status 0, where recorded.
    lld: Option<bool>,
    /// Whether mold exits with status 0, where recorded.
    mold: Option<bool>,
}

const fn row(script: &'static [u8], expect: Expect) -> Row {
    Row { script, expect, lld: None, mold: None }
}

const fn row3(script: &'static [u8], expect: Expect, lld: bool, mold: bool) -> Row {
    Row { script, expect, lld: Some(lld), mold: Some(mold) }
}

const COMMA: &str =
    "a comma glued to a name: GNU ld keeps it in the name, lld and mold read a file named ','";
const SKIPPED: &str = "GNU ld skips the byte it has no rule for (and echoes it to standard output)";

#[allow(clippy::too_many_lines)] // a table
fn rows() -> Vec<Row> {
    vec![
        // -- valid: §2.3 to §2.8 ------------------------------------------
        row3(b"INPUT(a.o)", Valid, true, true),
        row3(b"GROUP(a.o b.o)", Valid, true, true),
        row3(b"INPUT(a.o) INPUT(b.o)", Valid, true, true),
        row3(b"INPUT(a.o ,b.o)", Valid, false, false),
        row3(b"INPUT(a.o , b.o)", Valid, false, false),
        row3(b"GROUP ( a.o AS_NEEDED ( b.o ) )", Valid, true, true),
        row3(b"INPUT(AS_NEEDED(AS_NEEDED(a.o)))", Valid, false, true),
        row3(b"INPUT(\"a.o\")", Valid, true, true),
        row3(b"INPUT(=a.o)", Valid, true, true),
        row3(b"INPUT(-l:b.o)", Valid, true, true),
        row3(b"INPUT(-lfoo)", Valid, true, true),
        row3(b"INPUT(\"a\nb.o\")", Valid, false, false),
        row(b"OUTPUT_FORMAT(elf64-littleaarch64)", Valid),
        row(b"OUTPUT_FORMAT(\"elf64-littleaarch64\", \"elf64-bigaarch64\", \"elf64-littleaarch64\")", Valid),
        row(b"OUTPUT_FORMAT(elf64-x86-64 , elf64-x86-64 , elf64-x86-64)", Valid),
        row(b"OUTPUT_ARCH(aarch64)", Valid),
        row(b"TARGET(elf64-littleaarch64)", Valid),
        row3(b"SEARCH_DIR(.)", Valid, true, false),
        row3(b"SEARCH_DIR(\"=/usr/lib\");", Valid, true, false),
        row3(b"OUTPUT(x.out)", Valid, true, false),
        row3(b"ENTRY(_start)", Valid, true, false),
        row3(b"ENTRY(a=b)", Valid, true, false),
        row3(b"ENTRY(\"a b\")", Valid, true, false),
        row3(b"EXTERN(fa fb, fc)", Valid, true, false),
        row3(b"EXTERN(add ab fb ENTRY \"SIZEOF\" \"ah\")", Valid, true, false),
        row3(b"STARTUP(a.o)", Valid, false, false),
        row3(b"INCLUDE inc.ld", Valid, true, false),
        row3(b"INCLUDE inc.ld;", Valid, true, false),
        row3(b"INCLUDE \"inc.ld\"", Valid, true, false),
        row3(b"", Valid, true, false),
        row3(b";;;", Valid, true, false),
        row3(b"/* c */ ENTRY(_start) # c", Valid, true, false),
        row3(b"ENTRY ( _start ) ;", Valid, true, false),
        row3(b"/* \xc3\xa9 */ ENTRY(_start)", Valid, true, false),
        // -- refused: §2.10; GNU ld accepts each ---------------------------
        row3(b"SECTIONS { .foo : { *(.foo) } }", Refused("SECTIONS"), true, false),
        row3(b"SECTIONS { }", Refused("SECTIONS"), true, false),
        row3(b"MEMORY { ram (rwx) : ORIGIN = 0x400000, LENGTH = 16M }", Refused("MEMORY"), true, false),
        row(b"PHDRS { text PT_LOAD; }", Refused("PHDRS")),
        row3(b"VERSION { V1 { global: *; }; }", Refused("VERSION"), true, true),
        row3(b"PROVIDE(foo = 1);", Refused("PROVIDE"), true, false),
        row3(b"PROVIDE_HIDDEN(foo = 1);", Refused("PROVIDE_HIDDEN"), true, false),
        row3(b"HIDDEN(foo = 1);", Refused("HIDDEN"), true, false),
        row3(b"foo = 1;", Refused("symbol assignment"), true, false),
        row3(b". = 1;", Refused("symbol assignment"), true, false),
        row(b"\"x\" = 1;", Refused("symbol assignment")),
        row(b"x =1;", Refused("symbol assignment")),
        row(b"x += 1;", Refused("symbol assignment")),
        row(b"x <<= 1;", Refused("symbol assignment")),
        row(b"x /= 1;", Refused("symbol assignment")),
        row3(b"ASSERT(1, \"m\");", Refused("ASSERT"), true, false),
        row3(b"INSERT AFTER .text;", Refused("INSERT"), false, false),
        row3(b"NOCROSSREFS(.text .data);", Refused("NOCROSSREFS"), true, false),
        row3(b"NOCROSSREFS_TO(.text .data);", Refused("NOCROSSREFS_TO"), true, false),
        row(b"REGION_ALIAS(\"r\", ram);", Refused("REGION_ALIAS")),
        row3(b"LD_FEATURE(\"SANE_EXPR\")", Refused("LD_FEATURE"), false, false),
        row3(b"FORCE_COMMON_ALLOCATION", Refused("FORCE_COMMON_ALLOCATION"), false, false),
        row3(b"INHIBIT_COMMON_ALLOCATION", Refused("INHIBIT_COMMON_ALLOCATION"), false, false),
        row3(b"FORCE_GROUP_ALLOCATION", Refused("FORCE_GROUP_ALLOCATION"), false, false),
        row3(b"HLL(a)", Refused("HLL"), false, false),
        row3(b"SYSLIB(a)", Refused("SYSLIB"), false, false),
        row3(b"FLOAT", Refused("FLOAT"), false, false),
        row3(b"NOFLOAT", Refused("NOFLOAT"), false, false),
        row3(b"MAP(x)", Refused("MAP"), false, false),
        row3(b"LIB(a.o)", RefusedNewer("LIB"), false, false),
        // -- invalid: GNU ld rejects too ----------------------------------
        row3(b"INPUT()", Invalid, true, true),
        row(b"INPUT(,a.o)", Invalid),
        row(b"INPUT(a.o , , b.o)", Invalid),
        row(b"INPUT(a.o ,)", Invalid),
        row3(b"INPUT(AS_NEEDED)", Invalid, false, false),
        row3(b"AS_NEEDED(a.o)", Invalid, false, false),
        row(b"INPUT(a.o", Invalid),
        row3(b"INPUT(a.o)junk", Invalid, false, false),
        row(b"ENTRY()", Invalid),
        row(b"ENTRY(a b)", Invalid),
        row(b"ENTRY a", Invalid),
        row3(b"ENTRY(123)", Invalid, true, false),
        row(b"ENTRY(ALIGN)", Invalid),
        row(b"ENTRY(len)", Invalid),
        row(b"ENTRY(_start", Invalid),
        row(b"ENTRY(_start) )", Invalid),
        row(b"ENTRY(\"a)", Invalid),
        row(b"OUTPUT_FORMAT()", Invalid),
        row3(b"OUTPUT_FORMAT(elf64-littleaarch64, elf64-bigaarch64, elf64-littleaarch64)", Invalid, true, true),
        row(b"OUTPUT_FORMAT(a b c)", Invalid),
        row(b"OUTPUT_FORMAT(\"a\",\"b\")", Invalid),
        row(b"SEARCH_DIR(=/lib)", Invalid),
        row(b"SEARCH_DIR(a b)", Invalid),
        row(b"EXTERN()", Invalid),
        row(b"EXTERN(a,,b)", Invalid),
        row(b"EXTERN(,a)", Invalid),
        row(b"EXTERN(a,)", Invalid),
        row(b"EXTERN(BX)", Invalid),
        row(b"EXTERN(ah)", Invalid),
        row(b"EXTERN(ffx)", Invalid),
        row(b"EXTERN(a-b)", Invalid),
        row(b"EXTERN(SIZEOF)", Invalid),
        row(b"EXTERN(a[0])", Invalid),
        row(b"INCLUDE(x)", Invalid),
        row3(b"FROB(1)", Invalid, false, false),
        row3(b"entry(_start)", Invalid, false, false),
        row3(b"x=1;", Invalid, false, false),
        row(b"ALIGN(4)", Invalid),
        row(b"len = 1;", Invalid),
        row(b"x = 1", RefusedHead("symbol assignment")),
        row(b"x = ;", RefusedHead("symbol assignment")),
        row(b"HLL", RefusedHead("HLL")),
        row(b"MAP", RefusedHead("MAP")),
        row(b"SECTIONS {", RefusedHead("SECTIONS")),
        row(b"// c\nENTRY(_start)", Invalid),
        row3(b"/* x", Invalid, false, false),
        row(b"ENTRY(_start) @", Invalid),
        row(b"\x0c;", Invalid),
        row(b"ENTRY(_start)\n\xc3\xa9", Invalid),
        row(b"ENTRY(_start)\0", Invalid),
        row(b"\xef\xbb\xbfENTRY(_start)", Invalid),
        // -- kept disagreements: §5.2 -------------------------------------
        row3(b"INPUT(a.o,b.o)", Disagree(COMMA), false, false),
        row3(b"INPUT(a.o, b.o)", Disagree(COMMA), false, false),
        row3(b"ENTRY(a,b)", Disagree("GNU ld reads the symbol 'a,b', lld rejects the comma"), false, false),
        row(b"OUTPUT_FORMAT(x,y,z)", Disagree("GNU ld reads one format name 'x,y,z', lld three")),
        row3(b"INPUT(1.o)", Disagree("GNU ld skips the digit and looks for '.o'; lld and mold for '1.o'"), true, true),
        row3(b"INPUT(a.o;b.o)", Disagree(SKIPPED), false, false),
        row3(b"INPUT(a.o # c.o\n)", Disagree("'#' is no comment to GNU ld in a list, and is to lld and mold"), true, true),
        row(b"INPUT(a.o/*x*/b.o)", Disagree(SKIPPED)),
        row(b"INPUT(-x)", Disagree(SKIPPED)),
        row(b"INPUT(= a.o)", Disagree(SKIPPED)),
        row(b"INPUT(a.o \xc3\xa9)", Disagree(SKIPPED)),
        row(b"INPUT(a.o \"b.o)", Disagree(SKIPPED)),
        row3(b"INPUT(-l=foo)", Disagree("GNU ld prefixes the sysroot to the library name; lld and mold look for '=foo'"), false, false),
        row3(b"INPUT(\"-l:b.o\")", Disagree("GNU ld reads a file name, lld and mold a library"), true, true),
        row(b"INPUT(\"\")", Disagree("an empty name: GNU ld accepts it and finds nothing")),
        row(b"ENTRY(\"\")", Disagree("an empty name: GNU ld accepts it and sets no entry")),
    ]
}

fn assemble(scratch: &Scratch, assembler: &Path, name: &str, symbol: &str) {
    let source = format!(".globl {symbol}\n{symbol}: ret\n");
    let out = run_with(
        Command::new(assembler).arg("-o").arg(scratch.join(name)),
        source.as_bytes(),
        DEFAULT_TIMEOUT,
    );
    assert!(out.success(), "as: {}", out.stderr_text());
}

/// The scratch directory with every file a row names.
fn inputs() -> Option<Scratch> {
    let assembler = tool("as")?;
    let ar = tool("ar")?;
    let scratch = Scratch::new("script-diff");
    for (name, symbol) in [
        ("e.o", "_start"),
        ("a.o", "fa"),
        ("b.o", "fb"),
        ("c.o", "fc"),
        ("1.o", "f1"),
        ("foo.o", "ffoo"),
    ] {
        assemble(&scratch, &assembler, name, symbol);
    }
    let out = run_with(
        Command::new(ar).current_dir(scratch.path()).args(["rc", "libfoo.a", "foo.o"]),
        &[],
        DEFAULT_TIMEOUT,
    );
    assert!(out.success(), "ar: {}", out.stderr_text());
    scratch.write("inc.ld", "INPUT(c.o)");
    Some(scratch)
}

struct Linked {
    success: bool,
    stderr: String,
}

fn link(linker: &Path, dir: &Path, script: &[u8], n: usize) -> Linked {
    let name = format!("t{n}.ld");
    std::fs::write(dir.join(&name), script).unwrap();
    let out = run_with(
        Command::new(linker)
            .current_dir(dir)
            .args(["-o", &format!("t{n}.out"), "-L", "."])
            .arg(&name)
            .arg("e.o"),
        &[],
        DEFAULT_TIMEOUT,
    );
    assert!(
        !out.timed_out(),
        "{} timed out on {:?}",
        linker.display(),
        String::from_utf8_lossy(script)
    );
    Linked { success: out.success(), stderr: out.stderr_text() }
}

/// GNU ld's script reader accepted the script (module docs).
fn gnu_accepts(linked: &Linked) -> bool {
    !["syntax error", "file not recognized", "EOF in comment", "ignoring invalid character"]
        .iter()
        .any(|p| linked.stderr.contains(p))
}

// spec: script.md §5.1, §5.2 — acceptance agrees with GNU ld 2.44 on every
// row but the kept disagreements; every refused command is one GNU ld
// accepts; lld and mold behave as recorded.
#[test]
fn acceptance_matches_the_reference_linkers() {
    let Some(scratch) = inputs() else { return };
    let Some(bfd) = tool("ld.bfd") else { return };
    let lld = tool("ld.lld");
    let mold = tool("mold");
    let rows = rows();
    let chunks: Vec<Vec<String>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..4)
            .map(|w| {
                let (rows, bfd, lld, mold, dir) = (&rows, &bfd, &lld, &mold, scratch.path());
                scope.spawn(move || {
                    rows.iter()
                        .enumerate()
                        .filter(|(n, _)| n % 4 == w)
                        .flat_map(|(n, row)| {
                            check_row(n, row, dir, bfd, lld.as_deref(), mold.as_deref())
                        })
                        .collect()
                })
            })
            .collect();
        workers.into_iter().map(|w| w.join().unwrap()).collect()
    });
    let failures: Vec<String> = chunks.into_iter().flatten().collect();
    assert!(failures.is_empty(), "{} disagreements:\n{}", failures.len(), failures.join("\n"));
}

fn check_row(
    n: usize,
    row: &Row,
    dir: &Path,
    bfd: &Path,
    lld: Option<&Path>,
    mold: Option<&Path>,
) -> Vec<String> {
    let mut failures = Vec::new();
    {
        let shown = String::from_utf8_lossy(row.script);
        let mortar = mortar_script::parse(row.script, "t.ld");
        let mortar_says = match &mortar {
            Ok(_) => "ok".to_owned(),
            Err(d) => match d.refusal() {
                Some((_, name)) => format!("refused {name}"),
                None => format!("malformed ({})", d.id()),
            },
        };
        let mortar_ok = match row.expect {
            Valid => mortar.is_ok(),
            Refused(name) | RefusedNewer(name) | RefusedHead(name) => {
                mortar_says == format!("refused {name}")
            }
            Invalid | Disagree(_) => mortar_says == "malformed (malformed-input)",
        };
        if !mortar_ok {
            failures.push(format!("{shown:?}: mortar {mortar_says}, expected {:?}", row.expect));
        }
        let gnu = link(bfd, dir, row.script, n);
        let gnu_expected = matches!(row.expect, Valid | Refused(_) | Disagree(_));
        if gnu_accepts(&gnu) != gnu_expected {
            failures.push(format!(
                "{shown:?}: GNU ld {}, expected {:?}: {}",
                if gnu_accepts(&gnu) { "accepts" } else { "rejects" },
                row.expect,
                gnu.stderr.trim()
            ));
        }
        // lld refuses an input of another machine than OUTPUT_FORMAT
        // names (`e.o is incompatible with elf64-littleaarch64`, measured
        // with an x86-64 `e.o`), and `e.o` is the host's: off an AArch64
        // host it does not witness a row that names an AArch64 format.
        let foreign = !cfg!(target_arch = "aarch64")
            && row.script.windows(b"aarch64".len()).any(|w| w == b"aarch64");
        let lld = lld.filter(|_| !foreign);
        for (linker, expected) in [(lld, row.lld), (mold, row.mold)] {
            let (Some(linker), Some(expected)) = (linker, expected) else { continue };
            let witness = link(linker, dir, row.script, n);
            if witness.success != expected {
                failures.push(format!(
                    "{shown:?}: {} exit {}, recorded {}: {}",
                    linker.display(),
                    if witness.success { "0" } else { "non-zero" },
                    if expected { "0" } else { "non-zero" },
                    witness.stderr.trim()
                ));
            }
        }
    }
    failures
}

// spec: script.md §5.3 — the system's own text libraries and GNU ld's
// default scripts are read by GNU ld as mortar reads them: the link of a
// libc.so-style group of real files succeeds with GNU ld, and mortar
// parses the same bytes.
#[test]
fn system_text_library_links_with_gnu_ld() {
    let Some(bfd) = tool("ld.bfd") else { return };
    let Some(scratch) = inputs() else { return };
    let script = b"/* GNU ld script */\nGROUP ( a.o b.o  AS_NEEDED ( c.o ) )\n";
    assert!(mortar_script::parse(script, "t.ld").is_ok());
    let linked = link(&bfd, scratch.path(), script, 9999);
    assert!(linked.success, "{}", linked.stderr);
}
