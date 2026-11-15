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
    args.push(format!("--mortar-map={}", map_of(out).display()).into());
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
    holds_with(ld, label, cc, flags, program, &[])
}

/// [`holds`], with `link_extra` on both links.
fn holds_with(
    ld: &Path,
    label: &str,
    cc: Cc,
    flags: &[&str],
    program: &[Source],
    link_extra: &[&str],
) -> Option<()> {
    let scratch = Scratch::new(label);
    let built = compile(&scratch, cc, flags, program)?;
    let ours = built.dir.join("out.mortar");
    let theirs = built.dir.join("out.bfd");
    let m = mortar(&built, &ours, link_extra, &[]);
    assert!(m.success(), "{label}: mortar: {}", m.stderr_text());
    assert_eq!(m.stderr_text(), "", "{label}: mortar warns");
    let mut args = with_output(&built.args, &theirs);
    args.extend(link_extra.iter().map(OsString::from));
    let b = link(ld, &args, &built.dir, &[]);
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

/// `--gc-sections` on x86-64 (WP-031: the collector is
/// architecture-independent): C with every TLS model and IFUNC, C++ with
/// exceptions, `thread_local` and threads, compiled section-per-function
/// and not, run under qemu against GNU ld's `--gc-sections` output of the
/// same line, verifier-clean; and the collection removed something, so
/// the output is smaller than without it.
fn gc_matrix(name: &str, cc: Cc, program: &[Source]) {
    let Some(ld) = host() else { return };
    let sets: [&[&str]; 3] = [
        &["-O2", "-fPIE", "-pthread"],
        &["-O0", "-fPIC", "-g", "-ffunction-sections", "-fdata-sections", "-pthread"],
        &["-O2", "-fno-pic", "-ffunction-sections", "-fdata-sections", "-pthread"],
    ];
    for (i, flags) in sets.into_iter().enumerate() {
        let label = format!("{name}-{i}");
        if holds_with(&ld, &label, cc, flags, program, &["--gc-sections"]).is_none() {
            return;
        }
        let scratch = Scratch::new(&format!("{label}-size"));
        let built = compile(&scratch, cc, flags, program).expect("compiled once already");
        let (gc, plain) = (built.dir.join("gc"), built.dir.join("plain"));
        let m = mortar(&built, &gc, &["--gc-sections", "--print-gc-sections"], &[]);
        assert!(m.success(), "{label}: {}", m.stderr_text());
        assert!(
            m.stderr_text().lines().any(|l| l.contains("removing unused section '.text")),
            "{label}: no code collected"
        );
        assert!(mortar(&built, &plain, &[], &[]).success());
        let size = |p: &Path| std::fs::metadata(p).map_or(0, |m| m.len());
        assert!(size(&gc) < size(&plain), "{label}: {} >= {}", size(&gc), size(&plain));
        support::plumb::clean(&gc, &map_of(&gc), &built.dir);
        support::plumb::clean(&plain, &map_of(&plain), &built.dir);
    }
}

#[test]
fn x64_gc_sections_c_and_cxx() {
    gc_matrix("x64-gc-tls-gcc", GCC, C_TLS);
    gc_matrix("x64-gc-tls-clang", CLANG, C_TLS);
    gc_matrix("x64-gc-thr-gxx", GXX, CXX_THREADS);
    gc_matrix("x64-gc-thr-clangxx", CLANGXX, CXX_THREADS);
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
    // The verifier on the output every other run must equal (WP-035).
    support::plumb::clean(&first, &map_of(&first), &built.dir);
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

/// `_TLS_MODULE_BASE_` (x64.md X-47, resolve.md §8.1): a TLS descriptor
/// call on the module base, then a `DTPOFF32` load from it (the code a
/// local-dynamic TLSDESC sequence is), in a static link. mortar defines the
/// symbol, relaxes the descriptor to `mov $0` (T = 0 by name, so TP is
/// the base), writes `TpRel` in the code, and writes the symbol `STT_TLS`
/// at template offset 0; the program reads what GNU ld's reads.
#[test]
fn x64_tls_module_base() {
    let Some(ld) = host() else { return };
    let program: &[Source] = &[
        Source {
            name: "base.s",
            text: "\t.text\n\t.globl get_b\n\t.type get_b,@function\nget_b:\n\
                   \tleaq _TLS_MODULE_BASE_@tlsdesc(%rip), %rax\n\
                   \tcall *_TLS_MODULE_BASE_@tlscall(%rax)\n\
                   \tmovq %fs:0, %rdx\n\taddq %rdx, %rax\n\
                   \tmovl tb@dtpoff(%rax), %eax\n\tret\n\
                   \t.globl tbp\n\t.type tbp,@function\ntbp:\n\
                   \tmovq %fs:0, %rax\n\tleaq tb@tpoff(%rax), %rax\n\tret\n\
                   \t.section .tbss,\"awT\",@nobits\n\t.align 4\n\
                   \t.type tb,@object\n\t.size tb,4\ntb:\t.zero 4\n\
                   \t.section .note.GNU-stack,\"\",@progbits\n",
        },
        Source {
            name: "main.c",
            text: "#include <stdio.h>\n__thread int td = 7;\nint get_b(void);\nint *tbp(void);\n\
                   int main(void) { *tbp() = 42; printf(\"%d %d\\n\", get_b(), td); return 0; }\n",
        },
    ];
    holds(&ld, "x64-tls-base", GCC, &["-O2"], program);
}

/// W8 (x64.md §14.2): a `.init` fragment aligned above crti's end. The
/// gap before it runs, so it is `code_nop` (0x90, X-50, layout L-82), not
/// the trap fill lld and mold write; the program runs as GNU ld's does.
#[test]
fn x64_w8_init_padding_runs() {
    let Some(ld) = host() else { return };
    let program: &[Source] = &[
        Source {
            name: "init.s",
            text: "\t.section .init,\"ax\",@progbits\n\t.p2align 4\n\
                   \tmovl $7, init_ran(%rip)\n\t.data\n\t.globl init_ran\n\
                   init_ran:\t.long 0\n\t.section .note.GNU-stack,\"\",@progbits\n",
        },
        Source {
            name: "main.c",
            text: "#include <stdio.h>\nextern int init_ran;\n\
                   int main(void) { printf(\"init %d\\n\", init_ran); return init_ran; }\n",
        },
    ];
    holds(&ld, "x64-w8", GCC, &["-O2"], program);
}

/// The medium and large code models' program (x64.md X-52..X-55, WP-084):
/// a large `static` array (`.lbss`), large initialised and read-only data
/// (`.ldata`, `.lrodata`), a large common under `-fcommon` (GCC:
/// `SHN_X86_64_LCOMMON`, R-16), small data beside them, and a call into a
/// second, small-model object.
const C_LARGE: &[Source] = &[
    Source {
        name: "lg.c",
        text: r#"#include <stdio.h>
#ifndef BIG
#define BIG (1<<20)
#endif
static char lb[BIG];
char ld_[BIG] = {1};
const char lr[1<<20] = {2};
char lc[BIG];
int small_d = 3; int small_b; const int small_r = 4;
extern int ext_fn(int);
static const char *strs[] = {"a", "b"};
int main(void) { lb[BIG-1] = 5; lc[BIG-1] = 6;
  printf("%d %d %d %d %d %d %d %s %d\n", lb[BIG-1], ld_[0], lr[0], lc[BIG-1], small_d, small_b,
         small_r, strs[1], ext_fn(1));
  return 0; }
"#,
    },
    Source { name: "ext.c", text: "int ext_fn(int x) { return x + 41; }\n" },
];

/// C++ in the large models: a large object with a constructor and a
/// vtable (`.lbss`), large tables, TLS and an exception.
const CXX_LARGE: &[Source] = &[Source {
    name: "lg.cc",
    text: r#"#include <cstdio>
#include <string>
#include <stdexcept>
struct Big { int v[1<<18]; Big() { v[0] = 7; v[(1<<18)-1] = 8; } virtual ~Big() {} virtual int f() const { return v[0]; } };
Big big_obj;
static int table[1<<17] = {1, 2, 3};
const double ctab[1<<15] = {1.5};
thread_local int tl = 9;
static int thrower(int x) { if (x > 1) throw std::runtime_error("boom"); return x; }
int main() {
  int caught = 0;
  try { thrower(3); } catch (const std::exception &e) { caught = std::string(e.what()) == "boom"; }
  std::printf("%d %d %d %g %d %d\n", big_obj.f(), big_obj.v[(1<<18)-1], table[2], ctab[0], tl, caught);
}
"#,
}];

/// Fortran in the large models: a module array and a large COMMON block
/// (`SHN_X86_64_LCOMMON` `bigcom_`).
const F_LARGE: &[Source] = &[Source {
    name: "lg.f90",
    text: "module bigmod\n  implicit none\n  real(8) :: marr(200000)\nend module bigmod\n\
program lgf\n  use bigmod\n  implicit none\n  real(8) :: cblk(300000)\n  integer :: n\n\
  common /bigcom/ cblk, n\n  marr(200000) = 2.5d0\n  cblk(300000) = 4.5d0\n  n = 3\n\
  call sub()\n  print '(F4.1,1X,F4.1,1X,I0)', marr(200000), cblk(300000), n\nend program lgf\n\
subroutine sub()\n  implicit none\n  real(8) :: cblk(300000)\n  integer :: n\n\
  common /bigcom/ cblk, n\n  n = n + 1\nend subroutine sub\n",
}];

/// The flag sets of the large-model links: each model with no PIC and
/// both PIC models (`-mcmodel=large -fPIC` is the `GOTOFF64`/`GOTPC64`/
/// `PLTOFF64`/`GOT64` form, `-mcmodel=medium -fPIE` the `GOTPC32` one).
fn large_flag_sets() -> Vec<Vec<&'static str>> {
    let mut v = Vec::new();
    for model in ["-mcmodel=medium", "-mcmodel=large"] {
        for pic in ["-fno-pic", "-fPIE", "-fPIC"] {
            v.push(vec!["-O2", model, pic]);
        }
    }
    v.push(vec!["-O0", "-mcmodel=medium", "-fPIC", "-g", "-fdata-sections", "-ffunction-sections"]);
    v
}

/// Links `program` with mortar alone and runs it: for the links GNU ld
/// refuses (C++ in the large model, static: GNU ld 2.44 refuses
/// `PLTOFF64`/`GOT64` against glibc's IFUNC `strlen`, x64.md X-40).
fn runs_alone(label: &str, cc: Cc, flags: &[&str], program: &[Source], want: &str) -> Option<()> {
    host()?;
    let scratch = Scratch::new(label);
    let built = compile(&scratch, cc, flags, program)?;
    let ours = built.dir.join("out.mortar");
    let m = mortar(&built, &ours, &[], &[]);
    assert!(m.success(), "{label}: mortar: {}", m.stderr_text());
    assert_eq!(m.stderr_text(), "", "{label}: mortar warns");
    assert_eq!(execute(&ours), (want.to_owned(), Some(0)), "{label}");
    support::plumb::readers(&ours);
    support::plumb::elflint(&ours);
    support::plumb::clean(&ours, &map_of(&ours), &built.dir);
    Some(())
}

// spec: x64.md X-52, X-54, X-55; layout.md L-36a; resolve.md §3.3 (R-15,
// R-16): C in the medium and large models from GCC and Clang, static.
#[test]
fn x64_large_models_c() {
    let Some(ld) = host() else { return };
    for (cc, name) in [(GCC, "gcc"), (CLANG, "clang")] {
        for (i, flags) in large_flag_sets().into_iter().enumerate() {
            let mut all = flags.clone();
            all.push("-fcommon");
            if holds(&ld, &format!("x64-large-c-{name}-{i}"), cc, &all, C_LARGE).is_none() {
                return;
            }
        }
    }
}

// spec: x64.md X-52, X-54, X-40: C++ in the medium and large models.
#[test]
fn x64_large_models_cxx() {
    let Some(ld) = host() else { return };
    for (cc, name) in [(GXX, "gxx"), (CLANGXX, "clangxx")] {
        for (i, flags) in large_flag_sets().into_iter().enumerate() {
            let label = format!("x64-large-cxx-{name}-{i}");
            let done = if flags.contains(&"-mcmodel=large") {
                runs_alone(&label, cc, &flags, CXX_LARGE, "7 8 3 1.5 9 1\n")
            } else {
                holds(&ld, &label, cc, &flags, CXX_LARGE)
            };
            if done.is_none() {
                return;
            }
        }
    }
}

// spec: x64.md X-55, resolve.md §3.3 (R-16): a large Fortran COMMON
// block and module array.
#[test]
fn x64_large_models_fortran() {
    let Some(ld) = host() else { return };
    for (i, flags) in large_flag_sets().into_iter().enumerate() {
        if holds(&ld, &format!("x64-large-f-{i}"), GFORTRAN, &flags, F_LARGE).is_none() {
            return;
        }
    }
}

/// More than 2 GiB of `.lbss` (x64.md X-54: the small sections stay
/// within reach of each other whatever the large sections' size): a
/// 3 GiB array, as a section (`-fno-common`) and as a large common
/// (`-fcommon`, `SHN_X86_64_LCOMMON`). The array has no file bytes.
#[test]
fn x64_lbss_over_2_gib_runs() {
    const HUGE: &[Source] = &[Source {
        name: "hb.c",
        text: "#include <stdio.h>\nchar lb[3UL<<30];\nchar ld2[1<<17] = {1};\n\
               int small_d = 3; int small_b;\n\
               int main(int c, char **v) { lb[(3UL<<30)-1] = 5; small_b = c;\n\
               printf(\"%d %d %d %d\\n\", lb[(3UL<<30)-1], ld2[0], small_d, small_b); return 0; }\n",
    }];
    let Some(ld) = host() else { return };
    for (label, common) in [("x64-lbss-3g", "-fno-common"), ("x64-lcommon-3g", "-fcommon")] {
        let flags = ["-O2", "-mcmodel=medium", "-fno-pic", common];
        if holds(&ld, label, GCC, &flags, HUGE).is_none() {
            return;
        }
    }
}

/// x64.md X-54, layout.md L-36a: the small sections' addresses do not
/// depend on the large sections' sizes (the program header table is the
/// same: the same large output sections exist), and every large section
/// keeps `SHF_X86_64_LARGE` and comes after every small alloc section.
#[test]
fn x64_small_sections_do_not_move_with_the_large() {
    const SHF_X86_64_LARGE: u64 = 0x1000_0000;
    if host().is_none() {
        return;
    }
    let mut seen = Vec::new();
    for big in ["(1<<20)", "(1<<26)"] {
        let label = format!("x64-large-fixed-{}", seen.len());
        let scratch = Scratch::new(&label);
        let define = format!("-DBIG={big}");
        let flags = ["-O2", "-mcmodel=medium", "-fno-pic", "-fcommon", define.as_str()];
        let Some(built) = compile(&scratch, GCC, &flags, C_LARGE) else { return };
        let out = built.dir.join("out.mortar");
        let m = mortar(&built, &out, &[], &[]);
        assert!(m.success(), "{label}: {}", m.stderr_text());
        let sum = support::summarize_elf(&out);
        let alloc: Vec<&support::Sec> =
            sum.sections.iter().filter(|s| s.flags & support::SHF_ALLOC != 0).collect();
        let large: Vec<&str> = alloc
            .iter()
            .filter(|s| s.flags & SHF_X86_64_LARGE != 0)
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(large, [".lrodata", ".ldata", ".lbss"], "{label}: {:#?}", sum.sections);
        let first_large = alloc.iter().position(|s| s.flags & SHF_X86_64_LARGE != 0).unwrap();
        assert!(
            alloc[first_large..].iter().all(|s| s.flags & SHF_X86_64_LARGE != 0),
            "{label}: a small section after a large one: {:#?}",
            sum.sections
        );
        let small_end = alloc[..first_large].iter().map(|s| s.addr + s.size).max().unwrap();
        assert!(alloc[first_large].addr >= small_end, "{label}");
        let small: Vec<(String, u64, u64)> = alloc[..first_large]
            .iter()
            .map(|s| (s.name.clone(), s.addr, s.size))
            .collect();
        // The symbols GNU ld places at the end of the small data (Y5).
        let marks: Vec<Option<u64>> = ["_edata", "__bss_start", "etext"]
            .iter()
            .map(|n| sum.symbols.get(*n).copied())
            .collect();
        seen.push((small, marks));
    }
    assert_eq!(seen[0], seen[1], "the small sections moved with the large sections' size");
}

/// x64.md X-33, X-34: a small-model `PC32`, `32S` or `32` that cannot
/// reach a symbol placed past 4 GiB of large sections is the named overflow,
/// never a truncated value, and no output is left.
#[test]
fn x64_small_model_reference_to_far_large_data_overflows_by_name() {
    if host().is_none() {
        return;
    }
    for (label, insn, r_type) in [
        ("x64-large-pc32", "movl far_(%rip), %eax", "R_X86_64_PC32"),
        ("x64-large-32s", "movl far_, %eax", "R_X86_64_32S"),
        ("x64-large-32", "movl $far_, %eax", "R_X86_64_32 "),
    ] {
        let text = format!(
            "\t.text\n\t.globl main\nmain:\n\t{insn}\n\txorl %eax, %eax\n\tret\n\
             \t.section .lbss,\"awl\",@nobits\n\t.skip 0x100000000\n\t.globl far_\nfar_:\n\t.skip 4\n\
             \t.section .note.GNU-stack,\"\",@progbits\n"
        );
        let scratch = Scratch::new(label);
        let src = Source { name: "far.s", text: Box::leak(text.into_boxed_str()) };
        let Some(built) = compile(&scratch, GCC, &["-fno-pic"], &[src]) else { return };
        let out = built.dir.join("out.mortar");
        let m = mortar(&built, &out, &[], &[]);
        assert!(!m.success(), "{label}: mortar linked a truncated value");
        let e = m.stderr_text();
        assert!(e.contains("relocation truncated to fit") && e.contains(r_type), "{label}: {e}");
        assert!(e.contains("far_"), "{label}: {e}");
        assert!(!out.exists(), "{label}: an output was left behind");
    }
}

/// gdb on a `-g` medium-model static executable reads its large globals
/// (`.ldata`, `.lrodata`) and their addresses from the file.
#[test]
fn x64_gdb_reads_a_large_global() {
    let Some(gdb) = tool("gdb-multiarch") else {
        assert!(!tools_required(), "gdb-multiarch is required");
        return;
    };
    if host().is_none() {
        return;
    }
    let scratch = Scratch::new("x64-large-gdb");
    let flags = ["-O0", "-g", "-mcmodel=medium", "-fno-pic", "-fcommon"];
    let Some(built) = compile(&scratch, GCC, &flags, C_LARGE) else { return };
    let out = built.dir.join("out.mortar");
    let m = mortar(&built, &out, &[], &[]);
    assert!(m.success(), "{}", m.stderr_text());
    let sum = support::summarize_elf(&out);
    let addr = sum.symbols["ld_"];
    let mut c = Command::new(gdb);
    c.args(["-nx", "-batch", "-ex", "print ld_[0]", "-ex", "print lr[0]", "-ex", "print sizeof(ld_)"])
        .args(["-ex", "print/x &ld_", "-ex", "info symbol lc"])
        .arg(&out)
        .env("DEBUGINFOD_URLS", "")
        .env("TERM", "dumb");
    let o = run_with(&mut c, b"", Duration::from_mins(2));
    let text = o.stdout_text();
    assert!(o.success(), "gdb: {}{}", text, o.stderr_text());
    let want = format!("$1 = 1 '\\001'\n$2 = 2 '\\002'\n$3 = 1048576\n$4 = {addr:#x}\nlc in section .lbss\n");
    assert_eq!(text, want, "gdb: {}", o.stderr_text());
}
