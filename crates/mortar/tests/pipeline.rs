//! The whole pipeline (WP-027c): the `mortar` executable links what the
//! compiler drivers compile, and its outputs are held to GNU ld's output
//! of the same link line (the same standard output and exit status when
//! run), to the verifier (`mortar_plumb`, zero findings with mortar's own
//! map), to the readers (readelf, llvm-readelf: no warning) and to
//! eu-elflint (only the findings `support::plumb::ELFLINT_KNOWN` lists).

mod support;

use std::collections::{BTreeMap, BTreeSet};
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

/// mortar's link of `built` to `out`, with its map at `out.map`. An output
/// it writes is judged by the verifier, which must find nothing
/// (`docs/VALIDATION.md` L3, WP-035): no test here links without it.
fn mortar(built: &Built, out: &Path, extra: &[&str], env: &[(&str, &str)]) -> Outcome {
    let o = mortar_unverified(built, out, extra, env);
    if o.success() && out.exists() {
        support::plumb::clean(out, &map_of(out), &built.dir);
    }
    o
}

/// [`mortar`] without the verifier: for a link whose output is compared
/// byte for byte with one [`mortar`] verified.
fn mortar_unverified(built: &Built, out: &Path, extra: &[&str], env: &[(&str, &str)]) -> Outcome {
    let mut args = with_output(&built.args, out);
    args.push(format!("--mortar-map={}", map_of(out).display()).into());
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

/// layout.md L-35 (§20.7): Clang 22 objects built with
/// `-mbranch-protection=standard` carry `SHT_AARCH64_ATTRIBUTES` next to
/// the property note; the section is consumed (it was refused), the
/// output has none and its property note keeps BTI and PAC, as GNU ld's.
#[test]
fn c_with_clang22_branch_protection() {
    if !host() {
        return;
    }
    let (Some(readelf), Some(_)) = (tool("readelf"), tool("clang-22")) else {
        return; // clang-22 is not on every host the tests run on.
    };
    let read = |args: &[&str], path: &Path| {
        let o = run_with(Command::new(&readelf).args(args).arg(path), b"", Duration::from_mins(1));
        o.stdout_text()
    };
    let scratch = Scratch::new("pipe-c-clang22-bp");
    let flags = ["-O2", "-mbranch-protection=standard"];
    let built = build(&scratch, Driver::Clang22, &flags, support::C_PROGRAM).expect("clang-22");
    let obj = built.dir.join("c1.c.o");
    assert!(read(&["-SW"], &obj).contains("AARCH64_ATTRIBUTES"), "the input has the section");
    let ours = built.dir.join("out.mortar");
    let theirs = built.dir.join("out.bfd");
    let m = mortar(&built, &ours, &[], &[]);
    assert!(m.success() && m.stderr_text().is_empty(), "mortar: {}", m.stderr_text());
    let b = bfd(&built, &theirs, &[]);
    assert!(b.success(), "ld.bfd: {}", b.stderr_text());
    assert_eq!(execute(&ours), execute(&theirs), "mortar's program against GNU ld's");
    assert!(!read(&["-SW"], &ours).contains("AARCH64_ATTRIBUTES"), "consumed");
    let feature = |p: &Path| {
        read(&["-nW"], p).lines().find(|l| l.contains("AArch64 feature")).map(str::to_owned)
    };
    assert_eq!(feature(&ours), feature(&theirs));
    assert!(feature(&ours).is_some_and(|f| f.contains("BTI")), "{:?}", feature(&ours));
    support::plumb::readers(&ours);
    support::plumb::elflint(&ours);
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
}

/// The BTI property note (`GNU_PROPERTY_AARCH64_FEATURE_1_AND`, bit BTI):
/// the output's `PT_GNU_PROPERTY` makes the kernel enforce BTI on a host
/// that has it (this one: a `br` to a word that is not a landing pad is
/// `SIGILL`).
const BTI_NOTE: &str = "    .section .note.gnu.property,\"a\"\n    .p2align 3\n\
    .word 4, 16, 5\n    .asciz \"GNU\"\n    .word 0xc0000000, 4, 1, 0\n\
    .section .note.GNU-stack,\"\",@progbits\n";

/// `name.o`: `_start` calls `far` beyond `bl`'s reach (as in
/// `a_far_call_goes_through_a_thunk`) in a BTI-marked object; `far`
/// begins with `head`, sets 42 and ends with `tail` before `ret`.
fn bti_far(scratch: &Scratch, name: &str, head: &str, tail: &str) -> Built {
    let cc = tool("gcc").expect("gcc");
    scratch.write(
        &format!("{name}.s"),
        format!(
            "    .text\n    .globl _start\n_start:\n    bl far\n    mov x8, #93\n    svc #0\n\
             \n    .section .text.gap1,\"ax\",@progbits\n    .skip 0x4000000\n\
             \n    .section .text.gap2,\"ax\",@progbits\n    .skip 0x4000000\n\
             \n    .section .text.far,\"ax\",@progbits\n    .globl far\n    .type far, %function\n\
             far:\n    {head}\n    mov x0, #42\n    {tail}\n    ret\n{BTI_NOTE}"
        ),
    );
    let o = run_with(
        Command::new(cc)
            .args(["-c", &format!("{name}.s"), "-o", &format!("{name}.o")])
            .current_dir(scratch.path()),
        b"",
        Duration::from_mins(2),
    );
    assert!(o.success(), "gcc {name}: {}", o.stderr_text());
    Built {
        dir: scratch.path().to_path_buf(),
        args: ["-maarch64linux", "-static", &format!("{name}.o"), "-o", "out"]
            .map(str::to_owned)
            .to_vec(),
    }
}

/// a64.md §9 (BTI) and Q15: in a BTI output a thunk's `br x16` lands on
/// `bti c`, `bti j`, `bti jc`, `paciasp` or `pacibsp` at its target, each
/// proved an instruction (`$x`) in the input. Run with BTI enforced, each
/// exits 42 as GNU ld's link does.
#[test]
fn a_bti_far_call_lands_on_each_pad() {
    if !host() || tool("gcc").is_none() {
        assert!(!tools_required(), "gcc is required");
        return;
    }
    let scratch = Scratch::new("pipe-bti");
    let pads = [
        ("btic", "bti c", "nop"),
        ("btij", "bti j", "nop"),
        ("btijc", "bti jc", "nop"),
        ("paciasp", "paciasp", "autiasp"),
        ("pacibsp", "pacibsp", "autibsp"),
    ];
    for (name, head, tail) in pads {
        let built = bti_far(&scratch, name, head, tail);
        let ours = built.dir.join(format!("{name}.mortar"));
        let theirs = built.dir.join(format!("{name}.bfd"));
        let m = mortar(&built, &ours, &[], &[]);
        assert!(m.success(), "mortar {name}: {}", m.stderr_text());
        let b = bfd(&built, &theirs, &[]);
        assert!(b.success(), "ld.bfd {name}: {}", b.stderr_text());
        assert_eq!(execute(&ours).1, Some(42), "{name}: the far call lands");
        assert_eq!(execute(&theirs).1, Some(42), "{name}: GNU ld");
        let map = std::fs::read_to_string(map_of(&ours)).unwrap();
        assert!(map.lines().any(|l| l.starts_with("chunk thunk ")), "{name}: a thunk");
        support::plumb::readers(&ours);
        std::fs::remove_file(&ours).unwrap();
        std::fs::remove_file(&theirs).unwrap();
    }
}

/// a64.md §9, §10.4: a far call to a non-preemptible IFUNC goes through a
/// thunk to its IPLT entry, which starts with `bti c` in a BTI output, so
/// the thunk is not refused. (Not run: nothing applies the `IRELATIVE`
/// without a C library. GNU ld does not link the line at all: it reports
/// `relocation truncated to fit: R_AARCH64_CALL26` against the IFUNC.)
#[test]
fn a_bti_far_call_to_an_iplt_entry_is_linked() {
    if !host() || tool("gcc").is_none() {
        assert!(!tools_required(), "gcc is required");
        return;
    }
    let cc = tool("gcc").expect("gcc");
    let scratch = Scratch::new("pipe-bti-iplt");
    scratch.write(
        "ifn.s",
        format!(
            "    .text\n    nop\n\
             \n    .section .text.gap1,\"ax\",@progbits\n    .skip 0x4000000\n\
             \n    .section .text.gap2,\"ax\",@progbits\n    .skip 0x4000000\n\
             \n    .section .text.far,\"ax\",@progbits\n    .globl _start\n_start:\n\
             bl f\n    mov x8, #93\n    svc #0\n    .globl f\n\
             .type f, %gnu_indirect_function\nf:\n    bti c\n    adr x0, impl\n    ret\n\
             impl:\n    bti c\n    mov x0, #42\n    ret\n{BTI_NOTE}"
        ),
    );
    let o = run_with(
        Command::new(cc).args(["-c", "ifn.s", "-o", "ifn.o"]).current_dir(scratch.path()),
        b"",
        Duration::from_mins(2),
    );
    assert!(o.success(), "gcc: {}", o.stderr_text());
    let built = Built {
        dir: scratch.path().to_path_buf(),
        args: ["-maarch64linux", "-static", "ifn.o", "-o", "out"].map(str::to_owned).to_vec(),
    };
    let ours = built.dir.join("out.mortar");
    let m = mortar(&built, &ours, &[], &[]);
    assert!(m.success(), "mortar: {}", m.stderr_text());
    let map = std::fs::read_to_string(map_of(&ours)).unwrap();
    assert!(map.lines().any(|l| l.starts_with("chunk thunk __f@plt_veneer ")), "{map}");
    support::plumb::readers(&ours);
    // `bl` reaches the thunk, the thunk the IPLT entry's `bti c`
    // (`objdump -d`): the verifier follows both (a BTI IPLT entry starts
    // at its BTI, `spec/plumb.md` §4.8) and finds the thunk lands on a
    // landing pad (R9).
}

/// a64.md §9: a target that is not a landing pad, or whose pad-shaped
/// word is data (`.word`, a `$d` range), is refused by name (`BTI
/// landing pad`); no output. GNU ld writes a `bti c; b far` pad veneer
/// for the first, which mortar does not yet.
#[test]
fn a_bti_far_call_without_a_proved_pad_is_refused() {
    if !host() || tool("gcc").is_none() {
        assert!(!tools_required(), "gcc is required");
        return;
    }
    let scratch = Scratch::new("pipe-bti-no");
    for (name, head) in [("nop", "nop"), ("data", ".word 0xd503245f")] {
        let built = bti_far(&scratch, name, head, "nop");
        let ours = built.dir.join(format!("{name}.mortar"));
        let m = mortar(&built, &ours, &[], &[]);
        assert!(!m.success(), "{name}: linked");
        assert!(
            m.stderr_text().contains("error: unsupported feature: BTI landing pad"),
            "{name}: {}",
            m.stderr_text()
        );
        assert!(!ours.exists(), "{name}: an output");
    }
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

/// Determinism (image.md I-81, I-122; ARCHITECTURE.md "deterministic
/// output"): the output, the map and the text map of a C++ link with debug information are byte
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
    // The text map (I-122) names the output: compared without its name.
    let text_of = |out: &Path| {
        let t = String::from_utf8_lossy(&read(&out.with_extension("txt"))).into_owned();
        t.replace(&out.display().to_string(), "OUT")
    };
    let text_option = |out: &Path| format!("-Map={}", out.with_extension("txt").display());
    let first = built.dir.join("first.out");
    let t1 = text_option(&first);
    let m = mortar(&built, &first, &["--threads=1", &t1], &[("MORTAR_STORE", "staged")]);
    assert!(m.success(), "{}", m.stderr_text());
    let want = (read(&first), read(&map_of(&first)), text_of(&first));
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
                let tm = text_option(&out);
                let m = mortar_unverified(&built, &out, &[t.as_str(), &tm], &env);
                assert!(m.success(), "{store} {threads} {seed}: {}", m.stderr_text());
                let got = (read(&out), read(&map_of(&out)), text_of(&out));
                assert!(
                    got.0 == want.0,
                    "{store}, {threads} threads, seed {seed:?}: the output differs"
                );
                assert!(
                    got.1 == want.1,
                    "{store}, {threads} threads, seed {seed:?}: the map differs"
                );
                assert!(
                    got.2 == want.2,
                    "{store}, {threads} threads, seed {seed:?}: the text map differs"
                );
                std::fs::remove_file(&out).unwrap();
                std::fs::remove_file(map_of(&out)).unwrap();
                std::fs::remove_file(out.with_extension("txt")).unwrap();
                runs += 1;
            }
        }
    }
    for _ in 0..3 {
        let again = built.dir.join("again.out");
        let m = mortar_unverified(&built, &again, &[], &[]);
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

/// `-M` (I-120): the text map goes to standard output, the same text
/// `-Map` writes; `--mortar-map` still writes `mortar-map 1` beside it.
#[test]
fn print_map_writes_the_map_to_standard_output() {
    let Some((_s, built)) = c_program("pipe-print-map", &["-O2"]) else { return };
    let out = built.dir.join("m.out");
    let text = built.dir.join("m.txt");
    let map = format!("-Map={}", text.display());
    let m = mortar(&built, &out, &["-M", map.as_str()], &[]);
    assert!(m.success(), "{}", m.stderr_text());
    assert_eq!(m.stdout_text().as_bytes(), read(&text));
    assert!(m.stdout_text().contains("\nLinker script and memory map\n\n"));
    assert!(
        m.stdout_text()
            .ends_with(&format!("\nOUTPUT({} {GNU_TARGET})\nLOAD linker stubs\n", out.display()))
    );
    assert!(read(&map_of(&out)).starts_with(b"mortar-map 1\n"));
}

/// `readelf -p .comment`: the strings, in offset order.
fn comment_strings(path: &Path) -> Vec<String> {
    let readelf = tool("readelf").expect("readelf");
    let o = mortar_testutil::run(&readelf, [OsString::from("-p"), ".comment".into(), path.into()]);
    o.stdout_text()
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix('[')?.split_once(']'))
        .map(|(_, t)| t.trim().to_owned())
        .collect()
}

/// S-100: `.comment` is the inputs' strings, as GNU ld merges them, then
/// `mortar <version>` (lld's place for its own); with no input
/// `.comment` it is mortar's alone.
#[test]
fn the_comment_names_mortar_after_the_inputs() {
    let Some((_s, built)) = c_program("pipe-comment", &["-O2"]) else { return };
    let ident = format!("mortar {}", env!("CARGO_PKG_VERSION"));
    let (ours, gnu) = (built.dir.join("c.out"), built.dir.join("c.gnu"));
    let m = mortar(&built, &ours, &[], &[]);
    assert!(m.success(), "{}", m.stderr_text());
    assert!(bfd(&built, &gnu, &[]).success());
    let mut want = comment_strings(&gnu);
    assert!(!want.is_empty() && !want.contains(&ident), "{want:?}");
    want.push(ident.clone());
    assert_eq!(comment_strings(&ours), want);
    if let Some(lld) = tool("ld.lld") {
        let theirs = built.dir.join("c.lld");
        let args = with_output(&built.args, &theirs);
        assert!(link(lld.to_str().unwrap(), &args, &built.dir, &[]).success());
        let l = comment_strings(&theirs);
        assert!(l.last().is_some_and(|s| s.starts_with("Linker: ")), "{l:?}");
        assert_eq!(l[..l.len() - 1], want[..want.len() - 1], "lld's input strings");
    }
    // An assembled object has no `.comment`.
    let obj = assemble(&built.dir, "nc.s", "\t.globl _start\n_start:\n\tret\n");
    let out = built.dir.join("nc.out");
    let args: Vec<OsString> = vec![
        obj.into(),
        "-o".into(),
        out.clone().into(),
        format!("--mortar-map={}", map_of(&out).display()).into(),
    ];
    let o = link(MORTAR, &args, &built.dir, &[]);
    assert!(o.success(), "{}", o.stderr_text());
    support::plumb::clean(&out, &map_of(&out), &built.dir);
    assert_eq!(comment_strings(&out), [ident]);
}

/// `PT_GNU_RELRO`'s `[start, end)` (`readelf -lW`).
fn relro(path: &Path) -> (u64, u64) {
    let readelf = tool("readelf").expect("readelf");
    let o = mortar_testutil::run(&readelf, [OsString::from("-lW"), path.into()]);
    let line = o
        .stdout_text()
        .lines()
        .find(|l| l.trim_start().starts_with("GNU_RELRO"))
        .map(str::to_owned);
    let w: Vec<String> =
        line.expect("a PT_GNU_RELRO").split_whitespace().map(str::to_owned).collect();
    let start = hex(&w[2]).expect("p_vaddr");
    (start, start + hex(&w[5]).expect("p_memsz"))
}

/// `-z now` (layout.md L-60 row 7): `.got.plt`, which a static link fills
/// with IRELATIVE slots, moves into `PT_GNU_RELRO` after `.got`, where
/// lld puts it (GNU ld folds the slots into `.got`: RELRO too). The
/// program still runs: static glibc applies IRELATIVE before it protects
/// RELRO. The last of `-z now` and `-z lazy` wins.
#[test]
fn z_now_puts_the_got_plt_in_relro() {
    let Some((_s, built)) = c_program("pipe-z-now", &["-O2"]) else { return };
    let gnu = built.dir.join("gnu.out");
    assert!(bfd(&built, &gnu, &["-z", "now"]).success());
    let (lo, hi) = relro(&gnu);
    let (secs, _) = elf_facts(&gnu);
    for (name, addr, size, _) in &secs {
        if name.starts_with(".got") {
            assert!(lo <= *addr && addr + size <= hi, "GNU -z now: {name} outside RELRO");
        }
    }
    let want = execute(&gnu);
    for (k, (extra, now)) in [
        (&[][..], false),
        (&["-z", "now"][..], true),
        (&["-z", "now", "-z", "lazy"][..], false),
        (&["-z", "lazy", "-z", "now"][..], true),
    ]
    .into_iter()
    .enumerate()
    {
        let out = built.dir.join(format!("now{k}.out"));
        let m = mortar(&built, &out, extra, &[]);
        assert!(m.success(), "{extra:?}: {}", m.stderr_text());
        assert_eq!(execute(&out), want, "{extra:?}");
        let (lo, hi) = relro(&out);
        let (secs, _) = elf_facts(&out);
        let (_, addr, size, _) =
            secs.iter().find(|s| s.0 == ".got.plt").expect("IRELATIVE slots in .got.plt");
        assert!(*size > 0);
        assert_eq!(lo <= *addr && addr + size <= hi, now, "{extra:?}: .got.plt and RELRO");
        let got = secs.iter().find(|s| s.0 == ".got").expect(".got");
        assert!(lo <= got.1 && got.1 + got.2 <= hi, "{extra:?}: .got is RELRO");
    }
}

/// GNU's BFD target name for the host (I-121 item 5).
#[cfg(target_arch = "aarch64")]
const GNU_TARGET: &str = "elf64-littleaarch64";
#[cfg(target_arch = "x86_64")]
const GNU_TARGET: &str = "elf64-x86-64";
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
const GNU_TARGET: &str = "elf32-i386";

/// A GNU-style text map, read by one parser for GNU's and mortar's
/// (I-121): output sections, placed input sections, symbols, discarded
/// sections and archive members.
#[derive(Default, Debug)]
struct TextMap {
    /// (name, address, size)
    out: Vec<(String, u64, u64)>,
    /// (output section, name, address, size, file)
    inputs: Vec<(String, String, u64, u64, String)>,
    /// (output section, address, name)
    syms: Vec<(String, u64, String)>,
    /// (name, size, file)
    discarded: Vec<(String, u64, String)>,
    /// archive members taken
    members: Vec<String>,
}

fn hex(w: &str) -> Option<u64> {
    u64::from_str_radix(w.strip_prefix("0x")?, 16).ok()
}

/// `addr size [file...]` after a name: GNU's two hex columns.
fn columns(rest: &str) -> Option<(u64, u64, String)> {
    let rest = rest.trim_start();
    let (a, rest) = rest.split_once(' ')?;
    let rest = rest.trim_start();
    let (sz, file) = rest.split_once(' ').unwrap_or((rest, ""));
    Some((hex(a)?, hex(sz)?, file.to_owned()))
}

fn parse_text_map(text: &str) -> TextMap {
    let mut m = TextMap::default();
    let lines: Vec<&str> = text.lines().collect();
    let mut part = "";
    let mut cur = String::new();
    let mut i = 0;
    while i < lines.len() {
        let mut ln = lines[i].to_owned();
        i += 1;
        for heading in [
            "Archive member included",
            "Discarded input sections",
            "Memory Configuration",
            "Linker script and memory map",
            "Merging program properties",
        ] {
            if ln.starts_with(heading) {
                part = heading;
            }
        }
        if part == "Archive member included" {
            if !ln.is_empty() && !ln.starts_with(' ') && !ln.starts_with("Archive") {
                m.members.push(ln.get(..30).map_or(ln.as_str(), str::trim_end).to_owned());
            }
            continue;
        }
        if part != "Discarded input sections" && part != "Linker script and memory map" {
            continue;
        }
        // A name long enough to wrap: its columns are on the next line.
        let word = ln.trim_start_matches(' ');
        let indent = ln.len() - word.len();
        if indent <= 1 && !word.is_empty() && !word.contains(' ') {
            let next = lines.get(i).copied().unwrap_or("");
            if next.starts_with(&" ".repeat(16)) && columns(next).is_some() {
                ln.push_str(next);
                i += 1;
            } else {
                continue;
            }
        }
        if ln.starts_with("LOAD ") || ln.starts_with("OUTPUT(") {
            continue;
        }
        let word = ln.trim_start_matches(' ');
        let indent = ln.len() - word.len();
        let (name, rest) = word.split_once(' ').unwrap_or((word, ""));
        match indent {
            0 if part == "Linker script and memory map" => {
                if let Some((a, sz, _)) = columns(rest) {
                    name.clone_into(&mut cur);
                    m.out.push((cur.clone(), a, sz));
                }
            }
            1 if name != "*fill*" => {
                if let Some((a, sz, file)) = columns(rest).filter(|c| !c.2.is_empty()) {
                    if part == "Discarded input sections" {
                        m.discarded.push((name.to_owned(), sz, file));
                    } else {
                        m.inputs.push((cur.clone(), name.to_owned(), a, sz, file));
                    }
                }
            }
            16 => {
                let (a, sym) = word.split_once(' ').unwrap_or((word, ""));
                let sym = sym.trim_start();
                if let Some(a) = hex(a)
                    && !sym.is_empty()
                    && !sym.contains(' ')
                {
                    m.syms.push((cur.clone(), a, sym.to_owned()));
                }
            }
            _ => {}
        }
    }
    m
}

/// `readelf -SW`: (name, address, size, alloc) per section header;
/// `readelf -sW`: the values each symbol name has (a TLS symbol's from
/// the TLS block's start).
/// Section headers and symbol values, as `elf_facts` reads them.
type ElfFacts = (Vec<(String, u64, u64, bool)>, BTreeMap<String, BTreeSet<u64>>);

fn elf_facts(path: &Path) -> ElfFacts {
    let readelf = tool("readelf").expect("readelf");
    let o = mortar_testutil::run(&readelf, [OsString::from("-SW"), path.into()]);
    assert!(o.success(), "readelf -SW: {}", o.stderr_text());
    let mut sections = Vec::new();
    for ln in o.stdout_text().lines() {
        let Some((_, rest)) = ln.split_once(']') else { continue };
        let w: Vec<&str> = rest.split_whitespace().collect();
        if w.len() < 9 || w[0] == "Name" {
            continue;
        }
        let (Ok(a), Ok(sz)) = (u64::from_str_radix(w[2], 16), u64::from_str_radix(w[4], 16)) else {
            continue;
        };
        let alloc = w.len() == 10 && w[6].contains('A');
        sections.push((w[0].to_owned(), a, sz, alloc));
    }
    let tls = sections
        .iter()
        .filter(|s| s.0.starts_with(".tdata") || s.0.starts_with(".tbss"))
        .map(|s| s.1)
        .min()
        .unwrap_or(0);
    let o = mortar_testutil::run(&readelf, [OsString::from("-sW"), path.into()]);
    assert!(o.success(), "readelf -sW: {}", o.stderr_text());
    let mut syms: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();
    for ln in o.stdout_text().lines() {
        let w: Vec<&str> = ln.split_whitespace().collect();
        if w.len() < 8 || !w[0].ends_with(':') || w[6] == "UND" {
            continue;
        }
        let Ok(v) = u64::from_str_radix(w[1], 16) else { continue };
        let v = if w[3] == "TLS" { v + tls } else { v };
        syms.entry(w[7].to_owned()).or_default().insert(v);
    }
    (sections, syms)
}

/// The text map (I-121) against the output and GNU ld's map of the same
/// line: every section line is the section header (address 0 when not
/// allocated), every input line lies inside its section, every symbol line
/// is the `.symtab` value; and the placed input sections, read as
/// (section, size, file), are GNU's, but for what mortar makes itself
/// (merged strings, `.eh_frame`, the notes it consumes, `.comment`) and the
/// copy of a COMDAT group the two keep.
#[test]
fn the_text_map_matches_the_output_and_gnus_map() {
    let Some((_s, built)) = c_program("pipe-text-map", &["-O2", "-g"]) else { return };
    text_map_against_gnu(&built);
    let scratch = Scratch::new("pipe-text-map-cxx");
    let Some(built) = build(&scratch, Driver::Gxx, &["-O2", "-g"], CXX_FULL) else {
        assert!(!tools_required(), "g++ is required");
        return;
    };
    text_map_against_gnu(&built);
}

fn text_map_against_gnu(built: &Built) {
    let out = built.dir.join("t.out");
    let text = built.dir.join("t.txt");
    let map = format!("-Map={}", text.display());
    let m = mortar(built, &out, &[map.as_str()], &[]);
    assert!(m.success(), "{}", m.stderr_text());
    let gnu_out = built.dir.join("g.out");
    let gnu_text = built.dir.join("g.txt");
    let gmap = format!("-Map={}", gnu_text.display());
    let b = bfd(built, &gnu_out, &[gmap.as_str()]);
    assert!(b.success(), "{}", b.stderr_text());
    let ours = parse_text_map(&String::from_utf8_lossy(&read(&text)));
    let gnus = parse_text_map(&String::from_utf8_lossy(&read(&gnu_text)));
    let (sections, syms) = elf_facts(&out);
    assert!(!ours.out.is_empty() && !ours.inputs.is_empty() && !ours.syms.is_empty());
    let mut ranges = BTreeMap::new();
    for (name, a, sz) in &ours.out {
        let sh = sections.iter().find(|s| &s.0 == name);
        let want = sh.map(|s| (if s.3 { s.1 } else { 0 }, s.2));
        assert_eq!(want, Some((*a, *sz)), "section {name}");
        ranges.insert(name.clone(), (*a, *sz));
    }
    for (sec, name, a, sz, file) in &ours.inputs {
        let (start, size) = ranges[sec];
        assert!(start <= *a && a + sz <= start + size, "{sec}: {name} {a:#x} {sz:#x} {file}");
    }
    for (sec, a, name) in &ours.syms {
        if name.ends_with("_veneer") || name.starts_with("__erratum") {
            continue;
        }
        let values = syms.get(name);
        assert!(values.is_some_and(|v| v.contains(a)), "{sec}: {name} {a:#x}: {values:?}");
    }
    // What mortar makes itself (its `mortar` lines; GNU's same sections
    // name an input), so is not GNU's per-input line.
    let synthetic: BTreeSet<&str> =
        ours.inputs.iter().filter(|r| r.4 == "mortar").map(|r| r.1.as_str()).collect();
    let made = |name: &str| {
        synthetic.contains(name)
            // GNU's own IFUNC sections, which mortar's `.plt`/`.got` hold.
            || name == ".iplt"
            || name == ".igot.plt"
            || name == ".eh_frame"
            || name == ".comment"
            || name == ".group"
            || name.starts_with(".note.GNU-stack")
            || name == ".note.gnu.property"
            || name.starts_with(".rela")
            || name.starts_with(".rodata.str")
            || name.starts_with(".rodata.cst")
            || name.starts_with(".debug_str")
            || name.starts_with(".debug_line_str")
    };
    let comdat: BTreeSet<&str> =
        ours.discarded.iter().chain(&gnus.discarded).map(|d| d.0.as_str()).collect();
    let norm = |f: &str| {
        let p = Path::new(f);
        let mut out = PathBuf::new();
        for c in p.components() {
            match c {
                std::path::Component::ParentDir => {
                    out.pop();
                }
                c => out.push(c),
            }
        }
        out.display().to_string()
    };
    // Merged input sections (`.str1.8`, `.cst16`, ...), GNU's one line
    // each, are mortar's one `mortar` line per output section.
    let merged = |name: &str| {
        name.split('.').any(|w| {
            let digits = w.strip_prefix("str").or_else(|| w.strip_prefix("cst"));
            digits.is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
        })
    };
    // A COMDAT group's sections: which copy is kept (and so its size and
    // file, when copies differ) is each linker's own.
    let compared = |r: &&(String, String, u64, u64, String)| {
        !made(&r.1) && !merged(&r.1) && !comdat.contains(r.1.as_str()) && r.4 != "linker stubs"
    };
    let key = |(_, name, _, sz, file): &(String, String, u64, u64, String)| {
        (name.clone(), *sz, norm(file))
    };
    let theirs: BTreeSet<_> = gnus.inputs.iter().filter(compared).map(key).collect();
    let mine: BTreeSet<_> =
        ours.inputs.iter().filter(|r| r.4 != "mortar").filter(compared).map(key).collect();
    assert!(mine.len() > 100, "{} input sections compared", mine.len());
    let only_gnu: Vec<_> = theirs.difference(&mine).collect();
    let only_ours: Vec<_> = mine.difference(&theirs).collect();
    assert!(
        only_gnu.is_empty() && only_ours.is_empty(),
        "GNU only {only_gnu:?}; mortar only {only_ours:?}"
    );
    let members = |m: &TextMap| m.members.iter().map(|f| norm(f)).collect::<BTreeSet<_>>();
    assert_eq!(members(&ours), members(&gnus), "the archive members taken");
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
    // refused on the command line (only `=none` is written, cli.md §4.2).
    let m = mortar(&built, &out, &[], &[]);
    assert_eq!(code(&m), Some(1));
    assert!(
        m.stderr_text().contains("error: unsupported option value: --compress-debug-sections=z"),
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
}

/// Erratum 843419 under `--gc-sections` (WP-031, `spec/image.md` I-7;
/// WP-032e's windows): two sites, each an ADRP at a page's `0xff8` whose
/// target is beyond the ADR's range. One is in `dead`, which nothing calls;
/// a 2 MiB gap before `f` is collected too, and a live 2 MiB gap keeps
/// `val` far. Without gc both sites get a patch; with gc only `f`'s does
/// (the erratum scan reads live code only) and the program still exits 42
/// through it, as GNU ld's with gc does.
#[test]
fn an_erratum_site_in_collected_code_gets_no_patch() {
    if !host() {
        return;
    }
    let scratch = Scratch::new("pipe-erratum-gc");
    let site = |name: &str| {
        format!(
            "\n    .section .text.{name},\"ax\",@progbits\n    .balign 4096\n    .skip 0xff8\n\
             \n    .globl {name}\n    .type {name}, %function\n{name}:\n    adrp x0, val\n\
             \n    ldr x2, [sp]\n    ldr x0, [x0, #:lo12:val]\n    ret\n"
        )
    };
    let src = format!(
        "    .text\n    .globl _start\n_start:\n    bl gap_end\n    bl f\n    mov x8, #93\n    svc #0\n\
         {}\n    .section .text.dead_gap,\"ax\",@progbits\n    .skip 0x200000\n{}\
         \n    .section .text.gap,\"ax\",@progbits\n    .skip 0x200000\ngap_end:\n    ret\n\
         \n    .data\n    .balign 8\nval:\n    .xword 42\n",
        site("dead"),
        site("f"),
    );
    let obj = assemble(scratch.path(), "e.s", &src);
    let patches = |map: &str| {
        map.lines().filter(|l| l.starts_with("chunk patch __erratum_843419_veneer ")).count()
    };
    for gc in [false, true] {
        let mut args: Vec<String> = vec!["-static".into(), "--fix-cortex-a53-843419".into()];
        if gc {
            args.push("--gc-sections".into());
        }
        args.extend([obj.display().to_string(), "-o".into(), "out".into()]);
        let built = Built { dir: scratch.path().to_path_buf(), args };
        let ours = built.dir.join(format!("e{gc}.mortar"));
        let theirs = built.dir.join(format!("e{gc}.bfd"));
        let ran = mortar(&built, &ours, &[], &[]);
        assert!(ran.success(), "mortar: {}", ran.stderr_text());
        let gnu = bfd(&built, &theirs, &[]);
        assert!(gnu.success(), "ld.bfd: {}", gnu.stderr_text());
        assert_eq!(execute(&theirs).1, Some(42));
        assert_eq!(execute(&ours).1, Some(42), "the load through the patch (gc {gc})");
        let map = std::fs::read_to_string(map_of(&ours)).unwrap();
        assert_eq!(patches(&map), if gc { 1 } else { 2 }, "gc {gc}: {map}");
        if gc {
            // `.text.dead` and `.text.dead_gap`.
            let gc_lines = map.lines().filter(|l| l.starts_with("discard ") && l.ends_with(" gc"));
            assert_eq!(gc_lines.count(), 2, "both dead sections are collected: {map}");
        }
        support::plumb::readers(&ours);
        support::plumb::elflint(&ours);
    }
}

/// Erratum 843419 with `W1` at a page's `0xffc` and the final word `F`
/// three words after it, across the page (`0x1008`): the patch holds the
/// load as fill writes it, its `lo12` relocation applied (WP-032e found
/// the patch's word taken before that relocation, so the load read the
/// page's first doubleword). Exits 42 only if the load reads `val`.
#[test]
fn an_erratum_site_across_the_page_takes_its_relocated_word() {
    if !host() {
        return;
    }
    let scratch = Scratch::new("pipe-erratum-ffc");
    let obj = assemble(
        scratch.path(),
        "e.s",
        "    .text\n    .globl _start\n_start:\n    bl f\n    mov x8, #93\n    svc #0\n\
         \n    .section .text.f,\"ax\",@progbits\n    .balign 4096\n    .skip 0xffc\n\
         \n    .globl f\n    .type f, %function\nf:\n    adrp x0, val\n    ldr x2, [sp]\n\
         \n    nop\n    ldr x0, [x0, #:lo12:val]\n    ret\n\
         \n    .section .text.gap,\"ax\",@progbits\n    .skip 0x200000\n\
         \n    .data\n    .balign 8\n    .xword 7, 7, 7\nval:\n    .xword 42\n",
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
    assert_eq!(execute(&theirs).1, Some(42));
    assert_eq!(execute(&ours).1, Some(42), "the load through the patch");
    let map = std::fs::read_to_string(map_of(&ours)).unwrap();
    assert!(map.contains("chunk patch __erratum_843419_veneer "), "a patch in the map: {map}");
    support::plumb::readers(&ours);
    support::plumb::elflint(&ours);
}
