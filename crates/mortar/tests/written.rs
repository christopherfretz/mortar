//! A file the link writes is not a file it reads, or another file it
//! writes (`spec/cli.md` §3.14, WP-087): every clash class is refused by
//! `written-file-clash` before anything is written, the file named is
//! byte-equal afterwards and no output is left; the output is no exception
//! (`spec/image.md` I-62 since WP-087); `-o /dev/null` and
//! `-Map=/dev/stdout` keep working (I-58, I-58a, I-79a).

use std::ffi::OsString;
use std::os::unix::fs::FileTypeExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_testutil::target::Arch;
use mortar_testutil::{Outcome, Scratch, run_with, tools_required};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

const A_S: &str = "\t.text\n\t.globl _start\n_start:\n\tbl f\n\tmov x8, #93\n\tsvc #0\n\
    \t.section .note.GNU-stack,\"\",%progbits\n";
const B_S: &str = "\t.text\n\t.globl f\nf:\n\tret\n\t.section .note.GNU-stack,\"\",%progbits\n";
const C_S: &str = "\t.text\n\t.globl g\ng:\n\tret\n\t.section .note.GNU-stack,\"\",%progbits\n";
/// An object whose reference is never defined: a link that fails after
/// the front.
const U_S: &str =
    "\t.text\n\t.globl h\nh:\n\tbl nowhere\n\t.section .note.GNU-stack,\"\",%progbits\n";

fn run_in(dir: &Path, exe: &Path, args: &[OsString]) -> Outcome {
    let mut c = Command::new(exe);
    c.args(args).current_dir(dir).env_remove("MORTAR_SCHED_SEED").env_remove("SOURCE_DATE_EPOCH");
    run_with(&mut c, b"", Duration::from_mins(2))
}

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

/// The fixture: `a.o` (calls `f`), `b.o` (defines `f`), `c.o`, `u.o`,
/// the archive `libb.a`, the thin archive `libt.a` (member `b.o`), the
/// implicit script `s.ld` (`GROUP(libb.a)`), the response file `r.rsp`
/// (`a.o libb.a`) and the shared object `libc2.so`.
struct Fixture {
    scratch: Scratch,
}

impl Fixture {
    fn new(label: &str) -> Option<Self> {
        let arch = Arch::Aarch64;
        let (Some(gcc), Some(ar)) = (arch.tool("gcc"), arch.tool("ar")) else {
            assert!(!tools_required(), "gcc and ar are required");
            return None;
        };
        let scratch = Scratch::new(label);
        let dir = scratch.path().to_path_buf();
        for (name, text) in [("a", A_S), ("b", B_S), ("c", C_S), ("u", U_S)] {
            let src = scratch.write(&format!("{name}.s"), text);
            let o = run_in(
                &dir,
                &gcc,
                &os(&["-c", &src.display().to_string(), "-o", &format!("{name}.o")]),
            );
            assert!(o.success(), "gcc: {}", o.stderr_text());
        }
        for args in [["rc", "libb.a", "b.o"], ["rcT", "libt.a", "b.o"]] {
            let o = run_in(&dir, &ar, &os(&args));
            assert!(o.success(), "ar: {}", o.stderr_text());
        }
        let o = run_in(&dir, &gcc, &os(&["-shared", "-nostdlib", "c.o", "-o", "libc2.so"]));
        assert!(o.success(), "gcc -shared: {}", o.stderr_text());
        scratch.write("s.ld", "GROUP(libb.a)\n");
        scratch.write("r.rsp", "a.o libb.a\n");
        Some(Self { scratch })
    }

    fn dir(&self) -> &Path {
        self.scratch.path()
    }

    fn path(&self, name: &str) -> PathBuf {
        self.scratch.join(name)
    }

    fn mortar(&self, args: &[&str]) -> Outcome {
        run_in(self.dir(), Path::new(MORTAR), &os(args))
    }
}

/// The read files: (what the link reads, its inputs, the file, the role
/// the diagnostic names).
const READ: &[(&str, &[&str], &str, &str)] = &[
    ("an object", &["a.o", "libb.a"], "a.o", "the input a.o"),
    ("an archive", &["a.o", "libb.a"], "libb.a", "the archive libb.a"),
    ("a thin archive's member", &["a.o", "libt.a"], "b.o", "the input b.o"),
    ("an implicit linker script", &["a.o", "s.ld"], "s.ld", "the linker script s.ld"),
    ("a response file", &["@r.rsp"], "r.rsp", "the response file r.rsp"),
    (
        "a shared object -l found",
        &["a.o", "libb.a", "-L.", "-lc2", "-dynamic-linker", "/lib/ld-linux-aarch64.so.1"],
        "libc2.so",
        "the shared object ./libc2.so",
    ),
];

// spec: cli.md §3.14 (a written file naming a file the link reads, by the
// same path, another spelling, a symbolic link or a hard link: refused by
// name before anything is written; the file is byte-equal afterwards; the
// output too, image.md I-62); diag.md §3.1 `written-file-clash`
#[test]
fn a_written_file_never_names_a_file_the_link_reads() {
    let Some(fx) = Fixture::new("written-reads") else { return };
    for (class, inputs, file, role) in READ {
        for writer in ["-o", "-Map", "--dependency-file", "--mortar-map"] {
            for spelling in ["same", "./", "symlink", "hard link"] {
                let named = match spelling {
                    "same" => (*file).to_owned(),
                    "./" => format!("./{file}"),
                    "symlink" => {
                        let l = fx.path("sym");
                        let _ = std::fs::remove_file(&l);
                        std::os::unix::fs::symlink(file, &l).unwrap();
                        "sym".to_owned()
                    }
                    _ => {
                        let l = fx.path("hard");
                        let _ = std::fs::remove_file(&l);
                        std::fs::hard_link(fx.path(file), &l).unwrap();
                        "hard".to_owned()
                    }
                };
                let before = std::fs::read(fx.path(file)).unwrap();
                let w = if writer == "-o" {
                    format!("-o {named}")
                } else {
                    format!("{writer}={named}")
                };
                let mut args: Vec<&str> = inputs.to_vec();
                if writer == "-o" {
                    args.extend(["-o", &named]);
                } else {
                    args.extend(["-o", "out", &w]);
                }
                let o = fx.mortar(&args);
                let err = o.stderr_text();
                let case = format!("{class}, {writer}, {spelling}");
                assert!(!o.success(), "{case}: linked\n{err}");
                assert!(
                    err.contains(&format!("error: {w} names {role}; nothing is written to it")),
                    "{case}: {err}"
                );
                assert_eq!(std::fs::read(fx.path(file)).unwrap(), before, "{case}: {file} changed");
                assert!(!fx.path("out").exists(), "{case}: an output was written");
                for t in [format!(".{named}.mortar-tmp"), format!(".{named}.mortar-old")] {
                    let t = t.replace("./", "");
                    assert!(!fx.path(&t).exists(), "{case}: {t} was left");
                }
            }
        }
    }
}

// spec: cli.md §3.14 (two written files are one file: by path, by another
// spelling of a file not yet there, the output by default; refused by
// name, the later one naming the earlier's role)
#[test]
fn written_files_never_name_each_other() {
    let Some(fx) = Fixture::new("written-each-other") else { return };
    let cases: &[(&[&str], &str)] = &[
        (&["-o", "out", "-Map=out"], "-Map=out names the output out"),
        (&["-o", "out", "--dependency-file=./out"], "--dependency-file=./out names the output out"),
        (&["-o", "out", "-Map=x", "--dependency-file=x"], "--dependency-file=x names the map x"),
        (&["-o", "out", "-Map=x", "--mortar-map=./x"], "--mortar-map=./x names the map x"),
        (&["-Map=a.out"], "-Map=a.out names the output a.out"),
        (
            &["-o", "out", "-static", "--incremental", "--incremental-state=x", "-Map=x"],
            "names the map x",
        ),
    ];
    for (extra, text) in cases {
        let _ = std::fs::remove_file(fx.path("x"));
        let mut args = vec!["a.o", "libb.a"];
        args.extend(*extra);
        let o = fx.mortar(&args);
        let err = o.stderr_text();
        assert!(!o.success(), "{extra:?}: linked");
        assert!(err.contains(text), "{extra:?}: {err}");
        assert!(err.contains("; nothing is written to it"), "{extra:?}: {err}");
        for f in ["out", "a.out", "x"] {
            assert!(!fx.path(f).exists(), "{extra:?}: {f} was written");
        }
    }
}

// spec: image.md I-62 (the output naming an input is refused: GNU's
// `ld -r -o a.o a.o b.o` refusal, which mortar now shares) and I-66 (the
// refused link, a failure after the front, never removes the input as a
// stale output: it exists and is byte-equal)
#[test]
fn the_output_never_replaces_an_input_and_a_refusal_keeps_it() {
    let Some(fx) = Fixture::new("written-output") else { return };
    let before = std::fs::read(fx.path("a.o")).unwrap();
    // GNU's `ld -r -o a.o a.o b.o`, as an executable link (mortar refuses
    // `-r` today).
    let o = fx.mortar(&["-o", "a.o", "a.o", "b.o"]);
    let err = o.stderr_text();
    assert!(!o.success(), "linked: {err}");
    assert!(err.contains("error: -o a.o names the input a.o; nothing is written to it"), "{err}");
    assert_eq!(std::fs::read(fx.path("a.o")).unwrap(), before, "a.o changed");
    // A response file named as the output: refused by the rule; and a link
    // that fails before the rule runs (an undefined symbol) keeps it too.
    for (text, expected) in [
        ("a.o libb.a\n", "-o u.rsp names the response file u.rsp"),
        ("a.o libb.a u.o\n", "undefined symbol: nowhere"),
    ] {
        fx.scratch.write("u.rsp", text);
        let before = std::fs::read(fx.path("u.rsp")).unwrap();
        let o = fx.mortar(&["@u.rsp", "-o", "u.rsp"]);
        let err = o.stderr_text();
        assert!(!o.success(), "linked: {err}");
        assert!(err.contains(expected), "{err}");
        assert_eq!(std::fs::read(fx.path("u.rsp")).unwrap(), before, "the response file changed");
    }
}

// spec: cli.md §3.14 (the names the writer touches count: an input named
// as the output's set-aside name, image.md I-113, is refused)
#[test]
fn a_temporary_name_of_a_written_file_is_checked() {
    let Some(fx) = Fixture::new("written-temp") else { return };
    std::fs::copy(fx.path("a.o"), fx.path(".out.mortar-old")).unwrap();
    fx.scratch.write("out", "an old output\n");
    let before = std::fs::read(fx.path(".out.mortar-old")).unwrap();
    let o = fx.mortar(&[".out.mortar-old", "libb.a", "-o", "out"]);
    let err = o.stderr_text();
    assert!(!o.success(), "linked: {err}");
    assert!(
        err.contains(
            "the temporary name ./.out.mortar-old of -o out names the input .out.mortar-old"
        ),
        "{err}"
    );
    assert_eq!(std::fs::read(fx.path(".out.mortar-old")).unwrap(), before);
}

// spec: cli.md §3.14 (non-regular files are outside the rule: `-o
// /dev/null` and every written file at /dev/null link), image.md I-58a
// and I-79a (`-Map=/dev/stdout` on a pipe and redirected to a file is
// written in place; /dev/stdout stays a symbolic link)
#[test]
fn dev_null_and_dev_stdout_keep_working() {
    let Some(fx) = Fixture::new("written-dev") else { return };
    // image.md I-58b: the guard restores /dev/stdout and fails this test
    // if the linker replaced or removed it.
    let guard = mortar_testutil::dev_stdout::Guard::new("dev_null_and_dev_stdout_keep_working");
    let o = fx.mortar(&["a.o", "libb.a", "-o", "/dev/null"]);
    assert!(o.success(), "{}", o.stderr_text());
    let o = fx.mortar(&[
        "a.o",
        "libb.a",
        "-o",
        "/dev/null",
        "-Map=/dev/null",
        "--dependency-file=/dev/null",
        "--mortar-map=/dev/null",
    ]);
    assert!(o.success(), "{}", o.stderr_text());
    assert!(std::fs::metadata("/dev/null").unwrap().file_type().is_char_device());
    // Standard output is a pipe here (run_with).
    let o = fx.mortar(&["a.o", "libb.a", "-o", "out", "-Map=/dev/stdout"]);
    assert!(o.success(), "{}", o.stderr_text());
    assert!(o.stdout_text().contains("Archive member included"), "{}", o.stdout_text());
    // Redirected to a file.
    let mut c = Command::new("sh");
    c.arg("-c")
        .arg("exec \"$0\" a.o libb.a -o out2 -Map=/dev/stdout > redirected.map")
        .arg(MORTAR)
        .current_dir(fx.dir());
    let o = run_with(&mut c, b"", Duration::from_mins(1));
    assert!(o.success(), "{}", o.stderr_text());
    let map = std::fs::read_to_string(fx.path("redirected.map")).unwrap();
    assert!(map.contains("Archive member included"), "{map}");
    guard.check();
}
