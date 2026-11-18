//! `-r` through the product (RL-7, WP-187): what the records of RL-2..RL-4
//! left to the join, each with its rule. The end-to-end lane is
//! `tests/link/relocatable/` (stage R, LT-R4, the finals).
//!
//! - `r_sizes_agree`: RL-3's pure sizes (the layout's chunk sizes) and
//!   RL-4's `mortar_image::relocatable::sizes` agree on every `-r` case of
//!   the lane (brief item 3);
//! - `r_entry_writes_nothing`: `-e SYM` is a root and writes nothing,
//!   `e_entry` 0 (`spec/layout.md` L-R7, RL-2 ruling 1);
//! - `r_undefined_written`: `-u SYM` with no definition is a global `UND`
//!   `NOTYPE` entry after the keys (`spec/resolve.md` RV-R3, RV-R6);
//! - `r_symver_names`: RV-R6's versioned names, lld's form;
//! - `r_gnu_warning_carried`: L-R1's `.gnu.warning*` (WP-187's measured
//!   correction): both carried, no warning at `-r`, the final link warns;
//! - `r_unknown_property_warns`: `PropertyPlan`'s warnings reach the
//!   link's diagnostics at `-r`.
//!
//! AArch64 only: RL-7's cut (RL-9 lifts x86-64 and i386 at `-r`).

mod support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_core::par::Pool;
use mortar_diag::{Collector, Policy};
use mortar_testutil::{Outcome, Scratch, run_with, tool, tools_required};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// A host that can run these tests: AArch64 with gcc and ar.
fn host() -> Option<(PathBuf, PathBuf)> {
    if std::env::consts::ARCH != "aarch64" {
        return None;
    }
    if let (Some(gcc), Some(ar)) = (tool("gcc"), tool("ar")) {
        Some((gcc, ar))
    } else {
        assert!(!tools_required(), "gcc and ar are required");
        None
    }
}

fn run_in(dir: &Path, program: &Path, args: &[&str]) -> Outcome {
    let mut c = Command::new(program);
    c.args(args).current_dir(dir).env_remove("MORTAR_SCHED_SEED");
    run_with(&mut c, b"", Duration::from_mins(2))
}

fn ok(o: &Outcome, what: &str) {
    assert!(o.success(), "{what}: {}", o.stderr_text());
}

/// `mortar` on `args` in `dir`.
fn mortar(dir: &Path, args: &[&str]) -> Outcome {
    run_in(dir, Path::new(MORTAR), args)
}

/// The lane's `-r` cases (`tests/link/relocatable/`): the sources, how
/// each is compiled, the archives, and the `-r` line after `-r -m`.
struct Case {
    name: &'static str,
    /// (file, text); `.c` by gcc `-O2` (`-fcommon` where `common`), `.s`
    /// by gcc `-c`.
    sources: &'static [(&'static str, &'static str)],
    common: bool,
    /// (archive, `ar` flags, members).
    archives: &'static [(&'static str, &'static str, &'static [&'static str])],
    line: &'static [&'static str],
}

const C1: &str = "int g_common;\nint g_init = 7;\n\
__attribute__((visibility(\"hidden\"))) const char *hid(void) { return \"hidden\"; }\n\
__attribute__((weak)) int wk(void) { return 42; }\n\
extern int maybe(void) __attribute__((weak));\n\
int has_maybe(void) { return maybe ? 1 : 0; }\n\
const char *str(void) { return \"abc\"; }\n\
__attribute__((constructor(200))) static void early(void) { g_common = 100; }\n";
const C2: &str = "extern int g_common;\nconst char *hid(void);\n\
const char *(*get_hid(void))(void) { return hid; }\n\
__attribute__((constructor(300))) static void late(void) { g_common += 1; }\n";
const APPLETS: &str = "#include <stdio.h>\n#include <string.h>\n\
static int answer_main(void) { printf(\"%d\\n\", 42); return 0; }\n\
static int hello_main(void) { puts(\"hello\"); return 0; }\n\
static int true_main(void) { return 0; }\n\
static const struct { const char *name; int (*main)(void); } applets[] = {\n\
  {\"answer\", answer_main}, {\"hello\", hello_main}, {\"true\", true_main},\n};\n\
int list_applets(void) {\n\
  for (unsigned i = 0; i < sizeof applets / sizeof applets[0]; i++) puts(applets[i].name);\n\
  return 0;\n}\n\
int run_applet(const char *name) {\n\
  for (unsigned i = 0; i < sizeof applets / sizeof applets[0]; i++)\n\
    if (strcmp(applets[i].name, name) == 0) return applets[i].main();\n\
  return 127;\n}\n";
const KA: &str = "static int a_count;\nint a_seed = 2;\n\
int a_next(void) { return a_seed + ++a_count; }\n\
const char *a_name(void) { return \"kernel\"; }\n";
const KB: &str = "#include <stdio.h>\nint a_next(void);\nconst char *a_name(void);\n\
int b_table[4] = {4, 5, 6, 7};\n\
void b_report(void) { int a = a_next(); printf(\"a=%d b=%d s=%s\\n\", a, b_table[3], a_name()); }\n";
const M1: &str = "int m2_value(void);\nint m1_value(void) { return 11 + m2_value(); }\n";
const M2: &str = "int m2_value(void) { return 22; }\n";
const M3: &str = "int m3_value(void) { return 33; }\n";
const USER: &str = "int m1_value(void);\nint user_value(void) { return m1_value(); }\n";
const N_S: &str = "\t.text\n\t.globl n_fn\nn_fn:\tmov w0, #3\n\tret\n\
\t.section .note.GNU-stack,\"\",%progbits\n";
const X_S: &str = "\t.text\n\t.globl x_fn\nx_fn:\tmov w0, #4\n\tret\n\
\t.section .note.GNU-stack,\"x\",%progbits\n";
const A_S: &str = "\t.text\n\t.globl a_fn\na_fn:\tmov w0, #5\n\tret\n";
const WARN_S: &str = "\t.section .gnu.warning,\"\",%progbits\n\t.string \"file-level warning\"\n\
\t.section .gnu.warning.wfoo,\"\",%progbits\n\t.string \"wfoo is deprecated\"\n\
\t.text\n\t.globl wfoo\n\t.type wfoo,%function\nwfoo:\tret\n";
const WARN_USER: &str = "int wfoo(void);\nint wuser(void) { return wfoo(); }\n";
const SYMVER_S: &str = "\t.text\n\t.globl vfoo_v1\n\t.type vfoo_v1,%function\nvfoo_v1:\tmov w0, #1\n\tret\n\
\t.globl vfoo_v2\n\t.type vfoo_v2,%function\nvfoo_v2:\tmov w0, #2\n\tret\n\
\t.symver vfoo_v1, vfoo@V1\n\t.symver vfoo_v2, vfoo@@V2\n\
\t.section .note.GNU-stack,\"\",%progbits\n";
const SYMVER_USER: &str = "int vfoo(void);\nint vuser(void) { return vfoo(); }\n";
const PROP_S: &str = "\t.section .note.gnu.property,\"a\",%note\n\t.p2align 3\n\
\t.long 4\n\t.long 16\n\t.long 5\n\t.asciz \"GNU\"\n\
\t.long 0xc0000123\n\t.long 4\n\t.long 1\n\t.long 0\n\
\t.text\n\t.globl p_fn\np_fn:\tret\n\t.section .note.GNU-stack,\"\",%progbits\n";

const CASES: &[Case] = &[
    Case {
        name: "partial-mortar",
        sources: &[("c1.c", C1), ("c2.c", C2)],
        common: true,
        archives: &[],
        line: &["c1.o", "c2.o"],
    },
    Case {
        name: "r-busybox-gcc",
        sources: &[("applets.c", APPLETS)],
        common: false,
        archives: &[],
        line: &[
            "--eh-frame-hdr",
            "--hash-style=gnu",
            "--as-needed",
            "-dynamic-linker",
            "/lib/ld-linux-aarch64.so.1",
            "-X",
            "-EL",
            "--fix-cortex-a53-843419",
            "applets.o",
        ],
    },
    Case {
        name: "r-busybox-build-id",
        sources: &[("applets.c", APPLETS)],
        common: false,
        archives: &[],
        line: &["-EL", "--hash-style=gnu", "--build-id", "--eh-frame-hdr", "applets.o"],
    },
    Case {
        name: "r-kernel-style",
        sources: &[("a.c", KA), ("b.c", KB)],
        common: false,
        archives: &[],
        line: &["a.o", "b.o"],
    },
    Case {
        name: "r-archive-alone",
        sources: &[("m1.c", M1), ("m2.c", M2)],
        common: false,
        archives: &[("libm12.a", "rcs", &["m1.o", "m2.o"])],
        line: &["libm12.a"],
    },
    Case {
        name: "r-archive-u",
        sources: &[("m1.c", M1), ("m2.c", M2)],
        common: false,
        archives: &[("libm12.a", "rcs", &["m1.o", "m2.o"])],
        line: &["-u", "m2_value", "libm12.a"],
    },
    Case {
        name: "r-archive-after-ref",
        sources: &[("m1.c", M1), ("m2.c", M2), ("m3.c", M3), ("user.c", USER)],
        common: false,
        archives: &[("libm.a", "rcs", &["m1.o", "m2.o", "m3.o"])],
        line: &["user.o", "libm.a"],
    },
    Case {
        name: "r-thin-whole-archive",
        sources: &[("m1.c", M1), ("m2.c", M2)],
        common: false,
        archives: &[("libthin.a", "rcsT", &["m1.o", "m2.o"])],
        line: &["--whole-archive", "libthin.a", "--no-whole-archive"],
    },
    Case {
        name: "r-start-group",
        sources: &[("m1.c", M1), ("m2.c", M2), ("user.c", USER)],
        common: false,
        archives: &[("liba.a", "rcs", &["m1.o"]), ("libb.a", "rcs", &["m2.o"])],
        line: &["user.o", "--start-group", "liba.a", "libb.a", "--end-group"],
    },
    Case { name: "r-dev-null-m", sources: &[], common: false, archives: &[], line: &["/dev/null"] },
    Case {
        name: "r-stack-note-nx",
        sources: &[("n.s", N_S), ("x.s", X_S)],
        common: false,
        archives: &[],
        line: &["n.o", "x.o"],
    },
    Case {
        name: "r-stack-note-a-exec",
        sources: &[("a.s", A_S)],
        common: false,
        archives: &[],
        line: &["-z", "execstack", "a.o"],
    },
    Case {
        name: "r-stack-note-n-exec",
        sources: &[("n.s", N_S), ("a.s", A_S)],
        common: false,
        archives: &[],
        line: &["-z", "execstack", "n.o", "a.o"],
    },
    Case {
        name: "r-gnu-warning",
        sources: &[("w.s", WARN_S), ("wuser.c", WARN_USER)],
        common: false,
        archives: &[],
        line: &["w.o", "wuser.o"],
    },
    Case {
        name: "r-symver-names",
        sources: &[("v.s", SYMVER_S), ("vuser.c", SYMVER_USER)],
        common: false,
        archives: &[],
        line: &["v.o", "vuser.o"],
    },
    Case {
        name: "r-undefined",
        sources: &[("a.s", A_S)],
        common: false,
        archives: &[],
        line: &["-u", "missing", "a.o"],
    },
];

/// Builds `case`'s inputs in a fresh scratch.
fn build(case: &Case, gcc: &Path, ar: &Path) -> Scratch {
    let s = Scratch::new(&format!("rl7-{}", case.name));
    for (file, text) in case.sources {
        s.write(file, text);
        let obj = format!("{}.o", file.rsplit_once('.').unwrap().0);
        let mut args = vec!["-O2", "-c", file, "-o", &obj];
        if case.common {
            args.push("-fcommon");
        }
        ok(&run_in(s.path(), gcc, &args), file);
    }
    for (archive, flags, members) in case.archives {
        let mut args = vec![*flags, *archive];
        args.extend_from_slice(members);
        ok(&run_in(s.path(), ar, &args), archive);
    }
    s
}

fn line(case: &Case, out: &str) -> Vec<String> {
    let mut v: Vec<String> = ["-m", "aarch64linux", "-r"].iter().map(|s| (*s).to_owned()).collect();
    v.extend(case.line.iter().map(|s| (*s).to_owned()));
    v.extend(["-o".to_owned(), out.to_owned()]);
    v
}

/// The section header named `name` of the ELF64 file: (type, flags, size).
fn section(image: &[u8], name: &str) -> Option<(u32, u64, u64)> {
    let u16_at = |o: usize| u16::from_le_bytes(image[o..o + 2].try_into().unwrap());
    let u32_at = |o: usize| u32::from_le_bytes(image[o..o + 4].try_into().unwrap());
    let u64_at = |o: usize| u64::from_le_bytes(image[o..o + 8].try_into().unwrap());
    let shoff = u64_at(0x28) as usize;
    let shnum = u16_at(0x3c) as usize;
    let shstrndx = u16_at(0x3e) as usize;
    let str_off = u64_at(shoff + shstrndx * 64 + 0x18) as usize;
    (0..shnum).find_map(|i| {
        let h = shoff + i * 64;
        let n = str_off + u32_at(h) as usize;
        let end = n + image[n..].iter().position(|&b| b == 0)?;
        (&image[n..end] == name.as_bytes())
            .then(|| (u32_at(h + 4), u64_at(h + 8), u64_at(h + 0x20)))
    })
}

/// The symbols of the `-r` output by `readelf -sW`: (bind, ndx, name).
fn symbols(dir: &Path, file: &str) -> Vec<(String, String, String)> {
    let Some(readelf) = tool("readelf") else { return Vec::new() };
    let o = run_in(dir, &readelf, &["-sW", file]);
    ok(&o, "readelf");
    o.stdout_text()
        .lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            (w.len() >= 8 && w[0].ends_with(':'))
                .then(|| (w[4].to_owned(), w[6].to_owned(), w[7].to_owned()))
        })
        .collect()
}

// Brief item 3: RL-3's pure sizes and RL-4's `sizes` on every `-r` case of
// the lane. A disagreement is a finding with its input, not something the
// driver papers over.
#[test]
fn r_sizes_agree() {
    let Some((gcc, ar)) = host() else { return };
    let mut report = Vec::new();
    for case in CASES {
        let s = build(case, &gcc, &ar);
        // One store per case, as one per link in the product: the store
        // reads a (device, inode) once for its lifetime (`spec/input.md`
        // §9.3), and the previous case's scratch is gone, so its inodes
        // are free for this case's files (an ext4 or overlay build
        // directory reuses them at once: hosted run 44248824680 read
        // `libb.a` as the previous case's `user.o`). A store made after
        // this case's files exist and dropped before they are removed
        // sees only live files, whose identities are distinct.
        let store = support::store();
        let args: Vec<OsString> = line(case, "out.o").into_iter().map(OsString::from).collect();
        let parsed = mortar_cli::parse(&args);
        assert!(!parsed.failed(), "{}: {:?}", case.name, parsed.diagnostics);
        let diag = Collector::new(Policy::default());
        let env =
            mortar::front::Env { cwd: s.path(), pool: Pool::new(2), replay: None, slack: false };
        let mut timer = mortar::timing::Timer::new(false);
        let f = mortar::relocatable::front(parsed.plan, env, &store, &diag, &mut timer);
        let failed = diag.finish();
        let Some(mut f) = f.filter(|_| !failed.failed()) else {
            panic!("{}: the front failed: {failed:?}", case.name)
        };
        f.args = parsed.args.iter().map(|a| a.as_encoded_bytes().to_vec()).collect();
        match mortar::relocatable::size_disagreements(&f) {
            Ok(d) if d.is_empty() => {}
            Ok(d) => report.push(format!("{}: {}", case.name, d.join("; "))),
            Err(e) => report.push(format!("{}: the image cannot size it: {e:?}", case.name)),
        }
    }
    assert!(report.is_empty(), "RL-3 and RL-4 disagree:\n{}", report.join("\n"));
}

// C-R1: `-r`, `-i`, `--relocatable` and `-Ur` link the same `ET_REL`,
// byte for byte (`-Ur`'s `plan.ur` is the map's alone; GNU ld 2.44 writes
// the same sections for it).
#[test]
fn r_spellings_link() {
    let Some((gcc, ar)) = host() else { return };
    let case = CASES.iter().find(|c| c.name == "r-kernel-style").unwrap();
    let s = build(case, &gcc, &ar);
    let mut first: Option<Vec<u8>> = None;
    for spelling in ["-r", "-i", "--relocatable", "-Ur"] {
        ok(&mortar(s.path(), &[spelling, "a.o", "b.o", "-o", "k.o"]), spelling);
        let bytes = std::fs::read(s.join("k.o")).unwrap();
        assert_eq!(u16::from_le_bytes([bytes[16], bytes[17]]), 1, "{spelling}: ET_REL");
        match &first {
            None => first = Some(bytes),
            Some(f) => assert!(*f == bytes, "{spelling}: differs from -r"),
        }
    }
}

// L-R7, RL-2 ruling 1: `-e SYM` at `-r` is an extraction root and writes
// nothing: the output is the line's without it, `e_entry` 0.
#[test]
fn r_entry_writes_nothing() {
    let Some((gcc, ar)) = host() else { return };
    let case = CASES.iter().find(|c| c.name == "r-archive-alone").unwrap();
    let s = build(case, &gcc, &ar);
    ok(&mortar(s.path(), &["-r", "m1.o", "m2.o", "-o", "plain.o"]), "plain");
    ok(&mortar(s.path(), &["-r", "-e", "m2_value", "m1.o", "m2.o", "-o", "e.o"]), "-e");
    let plain = std::fs::read(s.join("plain.o")).unwrap();
    let e = std::fs::read(s.join("e.o")).unwrap();
    assert_eq!(plain, e, "-e writes nothing at -r");
    assert_eq!(u64::from_le_bytes(e[24..32].try_into().unwrap()), 0, "e_entry");
    // A root: `-e` extracts the member that defines it, as `-u` does.
    ok(&mortar(s.path(), &["-r", "-e", "m2_value", "libm12.a", "-o", "root.o"]), "-e root");
    let syms = symbols(s.path(), "root.o");
    if !syms.is_empty() {
        assert!(syms.iter().any(|(b, n, name)| b == "GLOBAL" && n != "UND" && name == "m2_value"));
        assert!(!syms.iter().any(|(_, _, name)| name == "m1_value"), "{syms:?}");
        let root = std::fs::read(s.join("root.o")).unwrap();
        assert_eq!(u64::from_le_bytes(root[24..32].try_into().unwrap()), 0, "e_entry");
    }
}

// RV-R3, RV-R6: `-u missing` is written as a global UND NOTYPE entry after
// the keys (GNU ld's form; `undefined_without_key`).
#[test]
fn r_undefined_written() {
    let Some((gcc, ar)) = host() else { return };
    let case = CASES.iter().find(|c| c.name == "r-undefined").unwrap();
    let s = build(case, &gcc, &ar);
    let args = line(case, "u.o");
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    ok(&mortar(s.path(), &args), "-u");
    let syms = symbols(s.path(), "u.o");
    if syms.is_empty() {
        return;
    }
    let last = syms.last().unwrap();
    assert_eq!(last, &("GLOBAL".to_owned(), "UND".to_owned(), "missing".to_owned()), "{syms:?}");
    let a = syms.iter().position(|(_, _, n)| n == "a_fn").unwrap();
    assert!(a < syms.len() - 1, "after the keys: {syms:?}");
}

// RV-R6: versioned names written unchanged; a plain reference is bound to
// the `@@` definition and there is no separate `UND vfoo` (lld's form).
#[test]
fn r_symver_names() {
    let Some((gcc, ar)) = host() else { return };
    let case = CASES.iter().find(|c| c.name == "r-symver-names").unwrap();
    let s = build(case, &gcc, &ar);
    let args = line(case, "v.o.r");
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    ok(&mortar(s.path(), &args), "-r");
    let syms = symbols(s.path(), "v.o.r");
    if syms.is_empty() {
        return;
    }
    let names: Vec<&str> = syms.iter().map(|(_, _, n)| n.as_str()).collect();
    assert!(
        syms.iter().any(|(b, n, name)| b == "GLOBAL" && n != "UND" && name == "vfoo@@V2"),
        "{syms:?}"
    );
    assert!(names.contains(&"vfoo@V1"), "{syms:?}");
    assert!(!names.contains(&"vfoo"), "no separate UND vfoo: {syms:?}");
    // The call's record names the `@@` entry (a tail call at -O2: JUMP26).
    if let Some(readelf) = tool("readelf") {
        let o = run_in(s.path(), &readelf, &["-rW", "v.o.r"]);
        ok(&o, "readelf -r");
        assert!(
            o.stdout_text().lines().any(|l| l.contains("R_AARCH64_") && l.contains("vfoo@@V2")),
            "{}",
            o.stdout_text()
        );
    }
}

// L-R1 (corrected by WP-187 from a measurement, `.build` rl7/m2): GNU ld
// 2.44 drops the file-level `.gnu.warning` at -r and prints it, keeps
// `.gnu.warning.SYM` and warns for the references among its inputs; lld 19
// and mold 2.37 copy both and warn nothing. mortar carries both and warns
// nothing at -r (lld's and mold's form): the file-level warning reaches the
// final link, which gives it as the direct link does.
#[test]
fn r_gnu_warning_carried() {
    let Some((gcc, ar)) = host() else { return };
    let case = CASES.iter().find(|c| c.name == "r-gnu-warning").unwrap();
    let s = build(case, &gcc, &ar);
    let args = line(case, "w.r.o");
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let o = mortar(s.path(), &args);
    ok(&o, "-r");
    assert!(!o.stderr_text().contains("warning"), "{}", o.stderr_text());
    let image = std::fs::read(s.join("w.r.o")).unwrap();
    assert!(section(&image, ".gnu.warning").is_some(), "the file-level warning is carried");
    assert!(section(&image, ".gnu.warning.wfoo").is_some(), "the symbol's warning is carried");
    // The final link of the -r output gives the file-level warning as the
    // direct link does. `wfoo`'s reference is now inside its defining file,
    // so no final link warns for it (GNU ld warned at -r; L-R1's gap).
    s.write("main.c", "int wuser(void);\nint main(void) { return wuser(); }\n");
    ok(&run_in(s.path(), &gcc, &["-O2", "-c", "main.c", "-o", "main.o"]), "main.c");
    let direct = mortar(
        s.path(),
        &["-m", "aarch64linux", "-e", "main", "main.o", "w.o", "wuser.o", "-o", "d"],
    );
    let via = mortar(s.path(), &["-m", "aarch64linux", "-e", "main", "main.o", "w.r.o", "-o", "v"]);
    for (what, o) in [("direct", &direct), ("via -r", &via)] {
        let e = o.stderr_text();
        assert!(e.contains("file-level warning"), "{what}: {e}");
    }
    assert!(direct.stderr_text().contains("wfoo is deprecated"), "{}", direct.stderr_text());
}

// The `PropertyPlan`'s warnings (`unknown-property`) reach the link's
// collector at -r instead of being dropped (WP-184 "For RL-7").
#[test]
fn r_unknown_property_warns() {
    let Some((gcc, _)) = host() else { return };
    let s = Scratch::new("rl7-property");
    s.write("p.s", PROP_S);
    ok(&run_in(s.path(), &gcc, &["-c", "p.s", "-o", "p.o"]), "p.s");
    let o = mortar(s.path(), &["-r", "p.o", "-o", "p.r.o"]);
    ok(&o, "-r");
    assert!(o.stderr_text().contains("warning"), "{}", o.stderr_text());
    assert!(o.stderr_text().contains("0xc0000123"), "{}", o.stderr_text());
}

// Brief item 2, `spec/diag.md` §4.4: a COMDAT group (what every C++ object
// has: an inline function) is refused by name until RL-8, never an internal
// error. The group section's own row is never `Kept` in the resolution;
// the census's C++ `-r` lines found the layout check failing first.
#[test]
fn r_section_groups_refused() {
    let Some((gcc, _)) = host() else { return };
    let s = Scratch::new("rl7-groups");
    s.write(
        "g.s",
        "\t.section .text.inl,\"axG\",@progbits,inl,comdat\n\t.globl inl\n\t.type inl, %function\ninl:\n\tret\n\
         \t.text\n\t.globl user\n\t.type user, %function\nuser:\n\tb inl\n",
    );
    ok(&run_in(s.path(), &gcc, &["-c", "g.s", "-o", "g.o"]), "g.s");
    for line in [
        &["-r", "g.o", "-o", "g.r.o"][..],
        &["-m", "aarch64linux", "-Ur", "g.o", "-o", "g2.r.o"][..],
    ] {
        let o = mortar(s.path(), line);
        let e = o.stderr_text();
        assert!(!o.success(), "{line:?}: refused");
        assert!(e.contains("unsupported feature: section groups with -r"), "{line:?}: {e}");
        assert!(!e.contains("internal error"), "{line:?}: {e}");
        assert!(!s.join(line[line.len() - 1]).exists(), "{line:?}: no output");
    }
}
