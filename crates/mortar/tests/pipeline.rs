//! The whole pipeline (WP-027c): the `mortar` executable links what the
//! compiler drivers compile, and its outputs are held to GNU ld's output
//! of the same link line (the same standard output and exit status when
//! run), to the verifier (`mortar_plumb`, zero findings with mortar's own
//! map), to the readers (readelf, llvm-readelf: no warning) and to
//! eu-elflint (only the findings `support::plumb::ELFLINT_KNOWN` lists).

mod support;

use std::ffi::OsString;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_testutil::{Outcome, Scratch, run_with, tool, tools_required};
use support::{Built, Driver, Source, build, with_output};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// A C++ program over two objects: an exception thrown in one object and
/// caught in the other with a destructor run during the unwinding,
/// `thread_local`, static constructors and destructors, RTTI
/// (`dynamic_cast`, `typeid`), `std::iostream`, and glibc's IFUNC string
/// functions (`strlen`, `memcpy`).
pub const CXX_FULL: &[Source] = &[
    Source {
        name: "m1.cpp",
        text: r#"#include <cstring>
#include <iostream>
#include <stdexcept>
#include <string>
#include <vector>
struct Base { virtual ~Base() {} virtual int v() const { return 1; } };
struct Derived : Base { int v() const override { return 2; } };
Base *make(int);
int thrower(int);
extern thread_local int tl;
struct Global {
    Global() { std::cout << "ctor\n"; }
    ~Global() { std::cout << "dtor\n"; }
} global;
int main(int argc, char **argv) {
    std::vector<std::string> v{"a", "bb"};
    try {
        thrower(argc + 3);
    } catch (const std::runtime_error &e) {
        std::cout << "caught " << e.what() << "\n";
    }
    Base *b = make(argc);
    Derived *d = dynamic_cast<Derived *>(b);
    char buf[16];
    std::memcpy(buf, "copied", 7);
    std::cout << (d ? d->v() : -1) << " " << typeid(*b).name() << " " << tl << " "
              << std::strlen(argv[0]) << " " << v[1].size() << " " << buf << "\n";
    delete b;
    return 3;
}
"#,
    },
    Source {
        name: "m2.cpp",
        text: r#"#include <cstdio>
#include <stdexcept>
struct Base { virtual ~Base() {} virtual int v() const { return 1; } };
struct Derived : Base { int v() const override { return 2; } };
thread_local int tl = 5;
struct Guard { ~Guard() { std::puts("unwound"); } };
Base *make(int n) { if (n > 0) return new Derived; return new Base; }
int thrower(int n) {
    Guard g;
    if (n > 2) throw std::runtime_error("deep");
    return n;
}
"#,
    },
];

/// Whether the host can run the AArch64 tests, and the tools are there.
fn host() -> bool {
    if std::env::consts::ARCH != "aarch64" {
        return false;
    }
    if tool("ld.bfd").is_none() {
        assert!(!tools_required(), "ld.bfd is required");
        return false;
    }
    true
}

/// Runs a linker on `args` in `dir`.
fn link(exe: &str, args: &[OsString], dir: &Path, env: &[(&str, &str)]) -> Outcome {
    let mut c = Command::new(exe);
    c.args(args).current_dir(dir);
    for (k, v) in env {
        c.env(k, v);
    }
    run_with(&mut c, b"", Duration::from_mins(5))
}

/// mortar's link of `built` to `out`, with its map at `out.map`.
fn mortar(built: &Built, out: &Path, extra: &[&str], env: &[(&str, &str)]) -> Outcome {
    let mut args = with_output(&built.args, out);
    args.push(format!("-Map={}", map_of(out).display()).into());
    args.extend(extra.iter().map(OsString::from));
    link(MORTAR, &args, &built.dir, env)
}

fn map_of(out: &Path) -> PathBuf {
    out.with_extension("map")
}

/// GNU ld's link of `built` to `out`.
fn bfd(built: &Built, out: &Path, extra: &[&str]) -> Outcome {
    let mut args = with_output(&built.args, out);
    args.extend(extra.iter().map(OsString::from));
    let ld = tool("ld.bfd").expect("ld.bfd");
    link(ld.to_str().expect("a UTF-8 path"), &args, &built.dir, &[])
}

/// Runs a program as `prog` (one argv[0] for every linker's output):
/// (standard output, exit status).
fn execute(exe: &Path) -> (String, Option<i32>) {
    let o = run_with(Command::new(exe).arg0("prog"), b"", Duration::from_mins(1));
    (o.stdout_text(), o.status.and_then(|s| s.code()))
}

/// Links `program` with mortar and GNU ld, runs both, and holds mortar's
/// output to the bar (`None` when a tool is missing).
fn holds(label: &str, driver: Driver, flags: &[&str], program: &[Source]) -> Option<()> {
    let scratch = Scratch::new(label);
    let built = build(&scratch, driver, flags, program)?;
    let ours = built.dir.join("out.mortar");
    let theirs = built.dir.join("out.bfd");
    let m = mortar(&built, &ours, &[], &[]);
    assert!(m.success(), "{label}: mortar: {}", m.stderr_text());
    assert_eq!(m.stderr_text(), "", "{label}: mortar warns");
    let b = bfd(&built, &theirs, &[]);
    assert!(b.success(), "{label}: ld.bfd: {}", b.stderr_text());
    let (out_m, out_b) = (execute(&ours), execute(&theirs));
    assert_eq!(out_m, out_b, "{label}: mortar's program against GNU ld's");
    support::plumb::readers(&ours);
    support::plumb::elflint(&ours);
    support::plumb::clean(&ours, &map_of(&ours), &built.dir);
    Some(())
}

const C_FLAGS: &[&[&str]] = &[
    &["-O0"],
    &["-O2"],
    &["-O0", "-g"],
    &["-O2", "-g"],
    &["-O2", "-ffunction-sections", "-fdata-sections"],
];

fn matrix(name: &str, driver: Driver, program: &[Source]) {
    if !host() {
        return;
    }
    for (i, flags) in C_FLAGS.iter().enumerate() {
        if holds(&format!("{name}-{i}"), driver, flags, program).is_none() {
            assert!(!tools_required(), "{} is required", driver.tool());
            return;
        }
    }
}

#[test]
fn c_with_gcc() {
    matrix("pipe-c-gcc", Driver::Gcc, support::C_PROGRAM);
}

#[test]
fn c_with_clang() {
    matrix("pipe-c-clang", Driver::Clang, support::C_PROGRAM);
}

#[test]
fn cxx_with_gxx() {
    matrix("pipe-cxx-gxx", Driver::Gxx, CXX_FULL);
}

#[test]
fn cxx_with_clangxx() {
    matrix("pipe-cxx-clangxx", Driver::Clangxx, CXX_FULL);
}

#[test]
fn fortran_with_gfortran() {
    matrix("pipe-f-gfortran", Driver::Gfortran, support::FORTRAN_PROGRAM);
}

/// A call beyond the ±128 MiB reach of `bl` (AAELF64 "Call and Jump
/// relocations"): two 64 MiB code sections between the call and its
/// target, so that layout cuts `.text` into parts with an island between
/// them (L-110, L-115) and the call goes through a thunk. The program
/// exits 42 only if the call arrives; GNU ld's veneer gives the same.
#[test]
fn a_far_call_goes_through_a_thunk() {
    if !host() {
        return;
    }
    let Some(cc) = tool("gcc") else {
        assert!(!tools_required(), "gcc is required");
        return;
    };
    let scratch = Scratch::new("pipe-thunk");
    scratch.write(
        "far.s",
        "    .text\n    .globl _start\n_start:\n    bl far\n    mov x8, #93\n    svc #0\n\
         \n    .section .text.gap1,\"ax\",@progbits\n    .skip 0x4000000\n\
         \n    .section .text.gap2,\"ax\",@progbits\n    .skip 0x4000000\n\
         \n    .section .text.far,\"ax\",@progbits\n    .globl far\n    .type far, %function\n\
         far:\n    mov x0, #42\n    ret\n",
    );
    let o = run_with(
        Command::new(cc).args(["-c", "far.s", "-o", "far.o"]).current_dir(scratch.path()),
        b"",
        Duration::from_mins(2),
    );
    assert!(o.success(), "gcc: {}", o.stderr_text());
    let built = Built {
        dir: scratch.path().to_path_buf(),
        args: ["-maarch64linux", "-static", "far.o", "-o", "out"].map(str::to_owned).to_vec(),
    };
    let ours = built.dir.join("out.mortar");
    let theirs = built.dir.join("out.bfd");
    let m = mortar(&built, &ours, &[], &[]);
    assert!(m.success(), "mortar: {}", m.stderr_text());
    let b = bfd(&built, &theirs, &[]);
    assert!(b.success(), "ld.bfd: {}", b.stderr_text());
    assert_eq!(execute(&ours).1, Some(42), "the far call arrives");
    assert_eq!(execute(&theirs).1, Some(42));
    let map = std::fs::read_to_string(map_of(&ours)).unwrap();
    assert!(map.lines().any(|l| l.starts_with("chunk thunk ")), "a thunk in the map");
    support::plumb::readers(&ours);
    support::plumb::elflint(&ours);
    support::plumb::clean(&ours, &map_of(&ours), &built.dir);
}

// ---- the driver: identities, options and failures ---------------------------

fn read(p: &Path) -> Vec<u8> {
    std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The status a link exited with.
fn code(o: &Outcome) -> Option<i32> {
    o.status.and_then(|s| s.code())
}

/// The C program, compiled by gcc with `flags`, or `None` (with the tool
/// check) where the host cannot run it.
fn c_program(label: &str, flags: &[&str]) -> Option<(Scratch, Built)> {
    if !host() {
        return None;
    }
    let scratch = Scratch::new(label);
    let Some(built) = build(&scratch, Driver::Gcc, flags, support::C_PROGRAM) else {
        assert!(!tools_required(), "gcc is required");
        return None;
    };
    Some((scratch, built))
}

/// Assembles `text` as `name` in `dir` with gcc.
fn assemble(dir: &Path, name: &str, text: &str) -> PathBuf {
    std::fs::write(dir.join(name), text).unwrap();
    let obj = dir.join(format!("{name}.o"));
    let o = run_with(
        Command::new(tool("gcc").expect("gcc"))
            .arg("-c")
            .arg(name)
            .arg("-o")
            .arg(&obj)
            .current_dir(dir),
        b"",
        Duration::from_mins(1),
    );
    assert!(o.success(), "gcc -c {name}: {}", o.stderr_text());
    obj
}

/// Determinism (image.md I-81; ARCHITECTURE.md "deterministic output"):
/// the output and the map of a C++ link with debug information are byte
/// for byte the same under both stores, at 1, 2, 4 and 8 threads, under
/// three scheduler seeds, and over repeated runs.
#[test]
fn the_output_is_a_function_of_the_inputs_alone() {
    if !host() {
        return;
    }
    let scratch = Scratch::new("pipe-determinism");
    let Some(built) = build(&scratch, Driver::Gxx, &["-O2", "-g"], CXX_FULL) else {
        assert!(!tools_required(), "g++ is required");
        return;
    };
    let first = built.dir.join("first.out");
    let m = mortar(&built, &first, &["--threads=1"], &[("MORTAR_STORE", "staged")]);
    assert!(m.success(), "{}", m.stderr_text());
    let want = (read(&first), read(&map_of(&first)));
    let mut runs = 1;
    for store in ["staged", "resident"] {
        for threads in ["1", "2", "4", "8"] {
            for seed in ["", "1", "977"] {
                let out = built.dir.join(format!("o-{store}-{threads}-{seed}.out"));
                let mut env = vec![("MORTAR_STORE", store)];
                if !seed.is_empty() {
                    env.push(("MORTAR_SCHED_SEED", seed));
                }
                let t = format!("--threads={threads}");
                let m = mortar(&built, &out, &[t.as_str()], &env);
                assert!(m.success(), "{store} {threads} {seed}: {}", m.stderr_text());
                let got = (read(&out), read(&map_of(&out)));
                assert!(
                    got.0 == want.0,
                    "{store}, {threads} threads, seed {seed:?}: the output differs"
                );
                assert!(
                    got.1 == want.1,
                    "{store}, {threads} threads, seed {seed:?}: the map differs"
                );
                std::fs::remove_file(&out).unwrap();
                std::fs::remove_file(map_of(&out)).unwrap();
                runs += 1;
            }
        }
    }
    for _ in 0..3 {
        let again = built.dir.join("again.out");
        let m = mortar(&built, &again, &[], &[]);
        assert!(m.success(), "{}", m.stderr_text());
        assert!(read(&again) == want.0, "a repeated run differs");
        runs += 1;
    }
    assert_eq!(runs, 28);
}

/// `--time-phases` (I-88, I-89): standard error ends with one line per
/// phase, the image (100) and the output file (105) among them, then the
/// total; nothing goes to standard output.
#[test]
fn time_phases_lists_the_image_and_the_output_file() {
    let Some((_s, built)) = c_program("pipe-time", &["-O2"]) else { return };
    let out = built.dir.join("t.out");
    let m = mortar(&built, &out, &["--time-phases"], &[]);
    assert!(m.success(), "{}", m.stderr_text());
    assert_eq!(m.stdout_text(), "");
    let err = m.stderr_text();
    let names: Vec<&str> = err
        .lines()
        .filter_map(|l| l.strip_prefix("mortar: time-phases: "))
        .map(|l| l.split(':').next().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "10 command line",
            "20 input",
            "30 resolve",
            "45 assignment",
            "60 split",
            "50 liveness",
            "70 scan",
            "80 synthetic",
            "90 layout",
            "100 image",
            "105 output file",
            "total"
        ],
        "{err}"
    );
}

/// `-M` (I-77): the map goes to standard output, the same text `-Map`
/// writes.
#[test]
fn print_map_writes_the_map_to_standard_output() {
    let Some((_s, built)) = c_program("pipe-print-map", &["-O2"]) else { return };
    let out = built.dir.join("m.out");
    let m = mortar(&built, &out, &["-M"], &[]);
    assert!(m.success(), "{}", m.stderr_text());
    assert_eq!(m.stdout_text().as_bytes(), read(&map_of(&out)));
    assert!(m.stdout_text().starts_with("mortar-map 1\n"));
}

/// Of `--gc-sections` and `--no-gc-sections`, the last one wins
/// (`spec/cli.md`'s boolean pairs): with it the output is smaller and runs
/// the same (the rest of `--gc-sections` is `tests/gc.rs`).
#[test]
fn gc_sections_the_last_one_wins() {
    let Some((_s, built)) =
        c_program("pipe-gc", &["-O2", "-ffunction-sections", "-fdata-sections"])
    else {
        return;
    };
    let mut sizes = Vec::new();
    for extra in [
        &["--gc-sections"][..],
        &["--no-gc-sections", "--gc-sections"][..],
        &["--gc-sections", "--no-gc-sections"][..],
    ] {
        let out = built.dir.join("gc.out");
        let m = mortar(&built, &out, extra, &[]);
        assert!(m.success(), "{extra:?}: {}", m.stderr_text());
        assert_eq!(execute(&out).1, Some(0), "{extra:?}");
        sizes.push(read(&out).len());
        std::fs::remove_file(&out).unwrap();
    }
    assert_eq!(sizes[0], sizes[1]);
    assert!(sizes[0] < sizes[2], "{sizes:?}");
}

/// I-110 through the driver: a key referenced only from `.debug_info`
/// fails the link with `undefined-symbol`, status 1, no output and no map.
#[test]
fn i110_undefined_from_debug_only_fails_through_the_driver() {
    let Some((_s, mut built)) = c_program("pipe-i110", &["-O2"]) else { return };
    let d = assemble(
        &built.dir,
        "d.s",
        "    .section .debug_info,\"\",@progbits\n    .xword nowhere_defined\n",
    );
    built.args.push(d.display().to_string());
    let out = built.dir.join("u.out");
    let m = mortar(&built, &out, &[], &[]);
    assert_eq!(code(&m), Some(1));
    assert!(
        m.stderr_text().contains("error: undefined symbol: nowhere_defined"),
        "{}",
        m.stderr_text()
    );
    assert!(!out.exists() && !map_of(&out).exists(), "no output, no map");
}

/// The files in `dir` whose names start with `.` (the temporary and the
/// set-aside output of I-59, I-113).
fn litter(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with('.'))
        .collect()
}

/// Linking over an existing output (I-59, I-60, I-113, I-66): the new
/// output is a new inode with the same bytes, a hard link to the old one
/// keeps the old bytes, and no temporary is left. A link that fails after
/// the command line removes the existing output (GNU ld's behaviour); a
/// failure on the command line keeps it. Exit status 0 and 1.
#[test]
fn linking_over_an_existing_output() {
    use std::os::unix::fs::MetadataExt as _;
    let Some((_s, built)) = c_program("pipe-over", &["-O2"]) else { return };
    let out = built.dir.join("p.out");
    let m = mortar(&built, &out, &[], &[]);
    assert_eq!(code(&m), Some(0), "{}", m.stderr_text());
    let bytes = read(&out);
    let keep = built.dir.join("p.keep");
    std::fs::hard_link(&out, &keep).unwrap();
    let ino = std::fs::metadata(&out).unwrap().ino();
    let m = mortar(&built, &out, &[], &[]);
    assert_eq!(code(&m), Some(0), "{}", m.stderr_text());
    assert_ne!(std::fs::metadata(&out).unwrap().ino(), ino, "a new inode (I-60)");
    assert!(read(&out) == bytes, "the same bytes");
    assert!(read(&keep) == bytes, "the hard link keeps the old output");
    assert_eq!(std::fs::metadata(&keep).unwrap().ino(), ino);
    assert_eq!(litter(&built.dir), Vec::<String>::new(), "no temporary left (I-59, I-113)");
    assert_eq!(execute(&out).1, Some(0));
    // A failure on the command line keeps the output.
    let m = mortar(&built, &out, &["--frobnicate"], &[]);
    assert_eq!(code(&m), Some(1));
    assert!(out.exists(), "a command-line failure keeps the output (I-66)");
    // A failure after it removes the output, and not the hard link's file.
    let d = assemble(
        &built.dir,
        "d.s",
        "    .section .debug_info,\"\",@progbits\n    .xword nowhere_defined\n",
    );
    let mut bad = Built { dir: built.dir.clone(), args: built.args.clone() };
    bad.args.push(d.display().to_string());
    let m = mortar(&bad, &out, &[], &[]);
    assert_eq!(code(&m), Some(1), "{}", m.stderr_text());
    assert!(!out.exists(), "a failed link removes the existing output (I-66): {}", m.stderr_text());
    assert!(read(&keep) == bytes);
    assert_eq!(litter(&built.dir), Vec::<String>::new(), "no temporary left (I-65)");
}

/// A missing entry symbol (L-90's ruling, WP-027c) is a warning with
/// `e_entry` 0, as GNU ld warns (it defaults to the start of `.text`).
#[test]
fn a_missing_entry_warns_and_writes_zero() {
    if !host() {
        return;
    }
    let scratch = Scratch::new("pipe-no-entry");
    let o = assemble(scratch.path(), "f.s", "    .text\n    .globl f\nf:\n    ret\n");
    let built = Built {
        dir: scratch.path().to_path_buf(),
        args: vec!["-static".into(), o.display().to_string(), "-o".into(), "out".into()],
    };
    let out = built.dir.join("e.out");
    let m = mortar(&built, &out, &[], &[]);
    assert_eq!(code(&m), Some(0));
    assert_eq!(m.stderr_text(), "mortar: warning: cannot find entry symbol _start\n");
    let h = read(&out);
    assert_eq!(&h[24..32], &[0u8; 8], "e_entry");
}

/// Bare `--build-id` (synth.md S-91, WP-027c's ruling): a 20-byte
/// identifier, the tree hash, the same on every run.
#[test]
fn bare_build_id_is_twenty_bytes() {
    let Some((_s, built)) = c_program("pipe-build-id", &["-O2"]) else { return };
    let mut ids = Vec::new();
    for n in 0..2 {
        let out = built.dir.join(format!("b{n}.out"));
        let m = mortar(&built, &out, &["--build-id"], &[]);
        assert!(m.success(), "{}", m.stderr_text());
        let o = mortar_testutil::run(
            tool("readelf").expect("readelf"),
            ["-n".as_ref(), out.as_os_str()],
        );
        let text = o.stdout_text();
        let id = text
            .lines()
            .find_map(|l| l.trim().strip_prefix("Build ID: "))
            .unwrap_or_else(|| panic!("no build id: {text}"))
            .to_owned();
        assert_eq!(id.len(), 40, "{id}");
        assert!(text.contains("0x00000014\tNT_GNU_BUILD_ID"), "{text}");
        ids.push(id);
    }
    assert_eq!(ids[0], ids[1]);
}

/// `SHF_COMPRESSED` input sections (`-gz`) are refused by name with no
/// output (resolve.md's table); with `-S` the debug sections are dropped
/// unread and the link succeeds. `--compress-debug-sections`, which the
/// driver passes with `-gz`, is refused by name (`docs/status/options.md`).
#[test]
fn compressed_debug_is_refused_by_name() {
    let Some((_s, mut built)) = c_program("pipe-gz", &["-O2", "-g", "-gz"]) else { return };
    let out = built.dir.join("z.out");
    // The driver asks for compressed output debug sections too, which is
    // refused on the command line.
    let m = mortar(&built, &out, &[], &[]);
    assert_eq!(code(&m), Some(1));
    assert!(
        m.stderr_text().contains("error: unsupported option: --compress-debug-sections"),
        "{}",
        m.stderr_text()
    );
    built.args.retain(|a| !a.starts_with("--compress-debug-sections"));
    let m = mortar(&built, &out, &[], &[]);
    assert_eq!(code(&m), Some(1));
    assert!(
        m.stderr_text()
            .contains("(.debug_info+0x0): unsupported feature: SHF_COMPRESSED input sections"),
        "{}",
        m.stderr_text()
    );
    assert!(!out.exists());
    let m = mortar(&built, &out, &["-S"], &[]);
    assert!(m.success(), "{}", m.stderr_text());
    assert_eq!(execute(&out).1, Some(0));
}

/// Erratum 843419 (`spec/a64.md` §11.2) under `--fix-cortex-a53-843419`,
/// which gcc passes by default on this host: an ADRP at a page's `0xff8`
/// whose target is beyond the ADR's ±1 MiB (a 2 MiB code gap) gets a
/// patch: the final word becomes `b patch`, the patch holds the word fill
/// writes there and `b F + 4`. The program exits 42 only if the load
/// through the patch reads `val`; GNU ld's stub gives the same.
#[test]
fn an_erratum_site_goes_through_a_patch() {
    if !host() {
        return;
    }
    let scratch = Scratch::new("pipe-erratum");
    let obj = assemble(
        scratch.path(),
        "e.s",
        "    .text\n    .globl _start\n_start:\n    bl f\n    mov x8, #93\n    svc #0\n\
         \n    .section .text.f,\"ax\",@progbits\n    .balign 4096\n    .skip 0xff8\n\
         \n    .globl f\n    .type f, %function\nf:\n    adrp x0, val\n    ldr x2, [sp]\n\
         \n    ldr x0, [x0, #:lo12:val]\n    ret\n\
         \n    .section .text.gap,\"ax\",@progbits\n    .skip 0x200000\n\
         \n    .data\n    .balign 8\nval:\n    .xword 42\n",
    );
    let built = Built {
        dir: scratch.path().to_path_buf(),
        args: vec![
            "-static".into(),
            "--fix-cortex-a53-843419".into(),
            obj.display().to_string(),
            "-o".into(),
            "out".into(),
        ],
    };
    let ours = built.dir.join("e.mortar");
    let theirs = built.dir.join("e.bfd");
    let ran = mortar(&built, &ours, &[], &[]);
    assert!(ran.success(), "mortar: {}", ran.stderr_text());
    let gnu = bfd(&built, &theirs, &[]);
    assert!(gnu.success(), "ld.bfd: {}", gnu.stderr_text());
    assert_eq!(execute(&ours).1, Some(42), "the load through the patch");
    assert_eq!(execute(&theirs).1, Some(42));
    let map = std::fs::read_to_string(map_of(&ours)).unwrap();
    let patch = map
        .lines()
        .find_map(|l| l.strip_prefix("chunk patch __erratum_843419_veneer "))
        .unwrap_or_else(|| panic!("a patch in the map: {map}"));
    let addr = u64::from_str_radix(patch.split(' ').next().unwrap(), 16).unwrap();
    // The patch's words: `ldr x0, [x0, #8]` (`val` at offset 8 of its
    // page), then a `b`.
    // `.text`'s address and offset, from `readelf -SW`.
    let sections =
        mortar_testutil::run(tool("readelf").expect("readelf"), ["-SW".as_ref(), ours.as_os_str()]);
    let line =
        sections.stdout_text().lines().find(|l| l.contains(" .text ")).expect(".text").to_owned();
    let fields: Vec<&str> = line.split(']').nth(1).unwrap().split_whitespace().collect();
    let (text_addr, text_off) =
        (u64::from_str_radix(fields[2], 16).unwrap(), u64::from_str_radix(fields[3], 16).unwrap());
    let image = read(&ours);
    let at = (text_off + (addr - text_addr)) as usize;
    let w = |i: usize| u32::from_le_bytes(image[at + 4 * i..at + 4 * i + 4].try_into().unwrap());
    assert_eq!(w(0) & 0xffc0_03ff, 0xf940_0000, "W4' is the load: {:#x}", w(0));
    assert_eq!(w(1) >> 26, 0b00_0101, "then a b: {:#x}", w(1));
    support::plumb::readers(&ours);
    support::plumb::elflint(&ours);
    support::plumb::clean(&ours, &map_of(&ours), &built.dir);
}
