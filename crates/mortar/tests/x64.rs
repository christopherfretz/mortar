//! The whole pipeline on x86-64 (WP-029b stage B): the `mortar`
//! executable links what the x86-64 cross drivers compile (GCC's
//! `x86_64-linux-gnu-*`, Clang with `--target=x86_64-linux-gnu`), and its
//! outputs are held to GNU ld's output of the same link line (the same
//! standard output and exit status when run under `qemu-x86_64`), to the
//! verifier (`mortar_plumb`, zero findings with mortar's own map), to the
//! readers and to eu-elflint, as `pipeline.rs` holds AArch64's; and the
//! output is a function of the inputs alone.

mod support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_testutil::{Outcome, Scratch, run_with, tool, tools_required};
use support::{Built, Source, build_by, with_output};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// A threaded C program over every TLS model the drivers emit (GD, LD,
/// IE, LE; TLSDESC under `-mtls-dialect=gnu2`), an over-aligned TLS
/// block, its own IFUNC and glibc's.
const C_TLS: &[Source] = &[Source {
    name: "p.c",
    text: r#"#include <stdio.h>
#include <string.h>
#include <pthread.h>
__thread int tv = 5;
__thread char tb[64] __attribute__((aligned(64)));
static __thread int ltv = 9;
extern __thread int gtv;
__thread int gtv = 11;
static void *th(void *a) { tv += (int)(long)a; ltv++; gtv++; return (void *)(long)(tv + ltv + gtv); }
static int sel_a(void) { return 1; }
static int sel_b(void) { return 2; }
int selflag = 0;
static int (*resolve_sel(void))(void) { return selflag ? sel_b : sel_a; }
int sel(void) __attribute__((ifunc("resolve_sel")));
int main(void) {
    char buf[200], src[200];
    memset(src, 'x', 199); src[199] = 0; memcpy(buf, src, 200);
    pthread_t t; void *r;
    pthread_create(&t, 0, th, (void *)3); pthread_join(t, &r);
    int (*fp)(void) = sel;
    printf("len=%zu tv=%d ltv=%d gtv=%d thr=%ld sel=%d fp=%d tbmod=%lu\n", strlen(buf), tv, ltv,
           gtv, (long)r, sel(), fp(), (unsigned long)tb % 64);
    return 0;
}
"#,
}];

/// A threaded C++ program: a `thread_local` with a constructor and
/// destructor in every thread (libstdc++'s TLS from `libstdc++.a`),
/// exceptions, virtual calls.
const CXX_THREADS: &[Source] = &[Source {
    name: "q.cc",
    text: r#"#include <cstdio>
#include <memory>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>
thread_local std::string ts = "tls";
struct B { virtual ~B() {} virtual int f() const = 0; };
struct D : B { int f() const override { return 42; } };
static int thrower(int x) { if (x > 1) throw std::runtime_error("boom"); return x; }
int main() {
    int caught = 0;
    try { thrower(3); } catch (const std::exception &e) { caught = std::string(e.what()) == "boom"; }
    std::vector<std::thread> v; int sum[4] = {0};
    for (int i = 0; i < 4; i++) v.emplace_back([i, &sum] { ts += std::to_string(i); sum[i] = ts.size(); });
    for (auto &t : v) t.join();
    std::unique_ptr<B> b(new D);
    std::printf("caught=%d ts=%s sizes=%d%d%d%d virt=%d\n", caught, ts.c_str(), sum[0], sum[1],
                sum[2], sum[3], b->f());
}
"#,
}];

/// A compiler of the matrix: the driver's name and the flags that make it
/// compile for x86-64.
#[derive(Clone, Copy)]
struct Cc {
    name: &'static str,
    target: &'static [&'static str],
    gnu: bool,
}

const GCC: Cc = Cc { name: "x86_64-linux-gnu-gcc", target: &[], gnu: true };
const GXX: Cc = Cc { name: "x86_64-linux-gnu-g++", target: &[], gnu: true };
const GFORTRAN: Cc = Cc { name: "x86_64-linux-gnu-gfortran", target: &[], gnu: true };
const CLANG: Cc = Cc { name: "clang", target: &["--target=x86_64-linux-gnu"], gnu: false };
const CLANGXX: Cc = Cc { name: "clang++", target: &["--target=x86_64-linux-gnu"], gnu: false };

/// GNU ld for x86-64 and the emulator, or `None` (a missing tool fails
/// under `MORTAR_REQUIRE_TOOLS`).
fn host() -> Option<PathBuf> {
    let (Some(ld), Some(_)) = (tool("x86_64-linux-gnu-ld.bfd"), tool("qemu-x86_64")) else {
        assert!(!tools_required(), "x86_64-linux-gnu-ld.bfd and qemu-x86_64 are required");
        return None;
    };
    Some(ld)
}

fn link(exe: &Path, args: &[OsString], dir: &Path, env: &[(&str, &str)]) -> Outcome {
    let mut c = Command::new(exe);
    c.args(args).current_dir(dir);
    for (k, v) in env {
        c.env(k, v);
    }
    run_with(&mut c, b"", Duration::from_mins(5))
}

fn map_of(out: &Path) -> PathBuf {
    out.with_extension("map")
}

/// mortar's link of `built` to `out`, with its map at `out.map`.
fn mortar(built: &Built, out: &Path, extra: &[&str], env: &[(&str, &str)]) -> Outcome {
    let mut args = with_output(&built.args, out);
    args.push(format!("-Map={}", map_of(out).display()).into());
    args.extend(extra.iter().map(OsString::from));
    link(Path::new(MORTAR), &args, &built.dir, env)
}

/// Runs an x86-64 program under qemu as `prog` (one argv[0] for every
/// linker's output): (standard output, exit status).
fn execute(exe: &Path) -> (String, Option<i32>) {
    let mut c = Command::new("qemu-x86_64");
    c.args(["-0".as_ref(), "prog".as_ref(), exe.as_os_str()]);
    let o = run_with(&mut c, b"", Duration::from_mins(2));
    (o.stdout_text(), o.status.and_then(|s| s.code()))
}

/// Compiles `program` with `cc` and `flags`; `None` when the driver is
/// missing (a failure under `MORTAR_REQUIRE_TOOLS`).
fn compile(scratch: &Scratch, cc: Cc, flags: &[&str], program: &[Source]) -> Option<Built> {
    let mut all: Vec<&str> = cc.target.to_vec();
    all.extend_from_slice(flags);
    let built = build_by(scratch, cc.name, &all, program);
    if built.is_none() {
        assert!(!tools_required(), "{} is required", cc.name);
    }
    built
}

/// Links `program` with mortar and GNU ld, runs both, and holds mortar's
/// output to the bar.
fn holds(ld: &Path, label: &str, cc: Cc, flags: &[&str], program: &[Source]) -> Option<()> {
    let scratch = Scratch::new(label);
    let built = compile(&scratch, cc, flags, program)?;
    let ours = built.dir.join("out.mortar");
    let theirs = built.dir.join("out.bfd");
    let m = mortar(&built, &ours, &[], &[]);
    assert!(m.success(), "{label}: mortar: {}", m.stderr_text());
    assert_eq!(m.stderr_text(), "", "{label}: mortar warns");
    let b = link(ld, &with_output(&built.args, &theirs), &built.dir, &[]);
    assert!(b.success(), "{label}: ld.bfd: {}", b.stderr_text());
    let (out_m, out_b) = (execute(&ours), execute(&theirs));
    assert!(out_b.1.is_some(), "{label}: GNU ld's program did not exit");
    assert_eq!(out_m, out_b, "{label}: mortar's program against GNU ld's");
    support::plumb::readers(&ours);
    support::plumb::elflint(&ours);
    support::plumb::clean(&ours, &map_of(&ours), &built.dir);
    Some(())
}

/// The flag sets every program is linked under: no PIC and both PIC
/// models (each relaxes differently, x64.md §8), debug information,
/// section-per-function, and, for GCC, TLS descriptors and CET.
fn flag_sets(cc: Cc) -> Vec<&'static [&'static str]> {
    let mut v: Vec<&'static [&'static str]> = vec![
        &["-O0", "-fno-pic"],
        &["-O2", "-fPIE"],
        &["-O2", "-fPIC", "-g"],
        &["-O0", "-fPIC", "-ffunction-sections", "-fdata-sections"],
        &["-O2", "-fno-plt", "-fPIC", "-fcf-protection=full"],
    ];
    if cc.gnu {
        v.push(&["-O2", "-fPIC", "-mtls-dialect=gnu2"]);
    }
    v
}

fn matrix(name: &str, cc: Cc, extra: &[&str], program: &[Source]) {
    let Some(ld) = host() else { return };
    for (i, flags) in flag_sets(cc).into_iter().enumerate() {
        let mut all = flags.to_vec();
        all.extend_from_slice(extra);
        if holds(&ld, &format!("{name}-{i}"), cc, &all, program).is_none() {
            return;
        }
    }
}

#[test]
fn x64_c_with_gcc() {
    matrix("x64-c-gcc", GCC, &[], support::C_PROGRAM);
    matrix("x64-tls-gcc", GCC, &["-pthread"], C_TLS);
}

#[test]
fn x64_c_with_clang() {
    matrix("x64-c-clang", CLANG, &[], support::C_PROGRAM);
    matrix("x64-tls-clang", CLANG, &["-pthread"], C_TLS);
}

#[test]
fn x64_cxx_with_gxx() {
    matrix("x64-cxx-gxx", GXX, &[], support::CXX_PROGRAM);
    matrix("x64-thr-gxx", GXX, &["-pthread"], CXX_THREADS);
}

#[test]
fn x64_cxx_with_clangxx() {
    matrix("x64-cxx-clangxx", CLANGXX, &[], support::CXX_PROGRAM);
    matrix("x64-thr-clangxx", CLANGXX, &["-pthread"], CXX_THREADS);
}

#[test]
fn x64_fortran_with_gfortran() {
    matrix("x64-f-gfortran", GFORTRAN, &[], support::FORTRAN_PROGRAM);
}

/// Determinism (image.md I-81) on x86-64: the output and the map of a
/// threaded C++ link with debug information are byte for byte the same
/// under both stores, at 1, 4 and 8 threads, and under a scheduler seed.
#[test]
fn x64_the_output_is_a_function_of_the_inputs_alone() {
    if host().is_none() {
        return;
    }
    let scratch = Scratch::new("x64-determinism");
    let Some(built) = compile(&scratch, GXX, &["-O2", "-g", "-fPIC", "-pthread"], CXX_THREADS)
    else {
        return;
    };
    let first = built.dir.join("first.out");
    let m = mortar(&built, &first, &["--threads=1"], &[("MORTAR_STORE", "staged")]);
    assert!(m.success(), "{}", m.stderr_text());
    let read = |p: &Path| std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let want = (read(&first), read(&map_of(&first)));
    for store in ["staged", "resident"] {
        for threads in ["1", "4", "8"] {
            for seed in ["", "977"] {
                let out = built.dir.join(format!("o-{store}-{threads}-{seed}.out"));
                let mut env = vec![("MORTAR_STORE", store)];
                if !seed.is_empty() {
                    env.push(("MORTAR_SCHED_SEED", seed));
                }
                let t = format!("--threads={threads}");
                let m = mortar(&built, &out, &[t.as_str()], &env);
                assert!(m.success(), "{store} {threads} {seed}: {}", m.stderr_text());
                assert!(read(&out) == want.0, "{store}, {threads} threads, seed {seed:?}: output");
                assert!(read(&map_of(&out)) == want.1, "{store}, {threads}, {seed:?}: map");
                std::fs::remove_file(&out).unwrap();
                std::fs::remove_file(map_of(&out)).unwrap();
            }
        }
    }
}
