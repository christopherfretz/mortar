//! The whole pipeline on i386 (WP-030c): the `mortar` executable links
//! static non-PIE programs the i386 cross drivers compile (GCC's
//! `i686-linux-gnu-*`, Clang with `--target=i686-linux-gnu`), and its
//! outputs are held to GNU ld's output of the same link line (the same
//! standard output and exit status when run under `qemu-i386`), to the
//! verifier (`mortar_plumb`, zero findings with mortar's own map, once
//! it has i386's table: [`PLUMB_KNOWS_I386`]), to the readers and to
//! eu-elflint, as `x64.rs` holds x86-64's; and the output is a function
//! of the inputs alone.

mod support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_testutil::{Outcome, Scratch, run_with, tool, tools_required};
use support::{Built, Source, build_by, with_output};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// Whether the verifier has i386's table (WP-028e). Until it does,
/// `mortar_plumb` runs on every output here (it must not fail to run) and
/// its findings are not held: it decodes no `SHT_REL` record, so every
/// relocated byte and every FDE is a finding. The merge that lands
/// WP-028e sets this and the outputs are then held to zero findings.
const PLUMB_KNOWS_I386: bool = false;

/// The verifier on `out` (see [`PLUMB_KNOWS_I386`]).
fn plumb(out: &Path, map: &Path, dir: &Path) {
    let findings = support::plumb::verify(out, map, dir);
    if PLUMB_KNOWS_I386 {
        assert!(findings.is_empty(), "plumb on {}: {findings:#?}", out.display());
    }
}

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
/// compile for i386.
#[derive(Clone, Copy)]
struct Cc {
    name: &'static str,
    target: &'static [&'static str],
    gnu: bool,
}

const GCC: Cc = Cc { name: "i686-linux-gnu-gcc", target: &[], gnu: true };
const GXX: Cc = Cc { name: "i686-linux-gnu-g++", target: &[], gnu: true };
const GFORTRAN: Cc = Cc { name: "i686-linux-gnu-gfortran", target: &[], gnu: true };
const CLANG: Cc = Cc { name: "clang", target: &["--target=i686-linux-gnu"], gnu: false };
const CLANGXX: Cc = Cc { name: "clang++", target: &["--target=i686-linux-gnu"], gnu: false };

/// GNU ld for i386 and the emulator, or `None` (a missing tool fails
/// under `MORTAR_REQUIRE_TOOLS`).
fn host() -> Option<PathBuf> {
    let (Some(ld), Some(_)) = (tool("i686-linux-gnu-ld.bfd"), tool("qemu-i386")) else {
        assert!(!tools_required(), "i686-linux-gnu-ld.bfd and qemu-i386 are required");
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

/// Runs an i386 program under qemu as `prog` (one argv[0] for every
/// linker's output): (standard output, exit status).
fn execute(exe: &Path) -> (String, Option<i32>) {
    let mut c = Command::new("qemu-i386");
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
    plumb(&ours, &map_of(&ours), &built.dir);
    Some(())
}

/// The flag sets every program is linked under: no PIC and both PIC
/// models (each relaxes differently, x86.md §8), debug information,
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
fn x86_c_with_gcc() {
    matrix("x86-c-gcc", GCC, &[], support::C_PROGRAM);
    matrix("x86-tls-gcc", GCC, &["-pthread"], C_TLS);
}

#[test]
fn x86_c_with_clang() {
    matrix("x86-c-clang", CLANG, &[], support::C_PROGRAM);
    matrix("x86-tls-clang", CLANG, &["-pthread"], C_TLS);
}

#[test]
fn x86_cxx_with_gxx() {
    matrix("x86-cxx-gxx", GXX, &[], support::CXX_PROGRAM);
    matrix("x86-thr-gxx", GXX, &["-pthread"], CXX_THREADS);
}

#[test]
fn x86_cxx_with_clangxx() {
    matrix("x86-cxx-clangxx", CLANGXX, &[], support::CXX_PROGRAM);
    matrix("x86-thr-clangxx", CLANGXX, &["-pthread"], CXX_THREADS);
}

/// `--gc-sections` on i386 (WP-031: the collector is
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
    }
}

#[test]
fn x86_gc_sections_c_and_cxx() {
    gc_matrix("x86-gc-tls-gcc", GCC, C_TLS);
    gc_matrix("x86-gc-tls-clang", CLANG, C_TLS);
    gc_matrix("x86-gc-thr-gxx", GXX, CXX_THREADS);
    gc_matrix("x86-gc-thr-clangxx", CLANGXX, CXX_THREADS);
}

#[test]
fn x86_fortran_with_gfortran() {
    matrix("x86-f-gfortran", GFORTRAN, &[], support::FORTRAN_PROGRAM);
}

/// Determinism (image.md I-81) on i386: the output and the map of a
/// threaded C++ link with debug information are byte for byte the same
/// under both stores, at 1, 4 and 8 threads, and under a scheduler seed.
#[test]
fn x86_the_output_is_a_function_of_the_inputs_alone() {
    if host().is_none() {
        return;
    }
    let scratch = Scratch::new("x86-determinism");
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

/// Rewrites the header of section `name` of the ELF32 object at `path`:
/// `sh_type` and `sh_entsize`.
fn retype(path: &Path, name: &[u8], sh_type: u32, entsize: u32) {
    let mut b = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let u16_at = |b: &[u8], o: usize| usize::from(u16::from_le_bytes([b[o], b[o + 1]]));
    let u32_at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    let (shoff, shnum, shstrndx) = (u32_at(&b, 0x20), u16_at(&b, 0x30), u16_at(&b, 0x32));
    let strtab = u32_at(&b, shoff + shstrndx * 40 + 16);
    let mut found = false;
    for i in 0..shnum {
        let h = shoff + i * 40;
        let at = strtab + u32_at(&b, h);
        let end = at + b[at..].iter().position(|&c| c == 0).unwrap();
        if &b[at..end] == name {
            b[h + 4..h + 8].copy_from_slice(&sh_type.to_le_bytes());
            b[h + 36..h + 40].copy_from_slice(&entsize.to_le_bytes());
            found = true;
        }
    }
    assert!(found, "no section {}", String::from_utf8_lossy(name));
    std::fs::write(path, b).unwrap();
}

/// X86-2 (§15.2 W12: mold leaves the field untouched): an `SHT_RELA`
/// section in an `EM_386` object is refused by name (class
/// `section-type`, name `SHT_RELA`), never read with the wrong addend.
/// Three REL records (24 bytes) are two RELA records' worth.
#[test]
fn x86_2_sht_rela_is_refused_by_name() {
    if host().is_none() {
        return;
    }
    let scratch = Scratch::new("x86-2-rela");
    let program = [Source {
        name: "ra.c",
        text: "int a, b, c;\nint *p[3] = {&a, &b, &c};\nint main(void) { return *p[0]; }\n",
    }];
    let Some(built) = compile(&scratch, GCC, &["-O2", "-fPIE"], &program) else { return };
    retype(&built.dir.join("ra.c.o"), b".rel.data.rel.local", 4, 12);
    let out = built.dir.join("out.mortar");
    let m = mortar(&built, &out, &[], &[]);
    assert!(!m.success(), "mortar linked an i386 SHT_RELA section");
    let want = "ra.c.o:(.rel.data.rel.local+0x0): unsupported section type: SHT_RELA";
    assert!(m.stderr_text().contains(want), "{}", m.stderr_text());
    assert!(!out.exists(), "an output was left behind");
}

/// X86-1: an input `e_flags` other than 0 is refused by name (class
/// `feature`), the value shown.
#[test]
fn x86_1_e_flags_is_refused_with_its_value() {
    if host().is_none() {
        return;
    }
    let scratch = Scratch::new("x86-1-e-flags");
    let program = [Source { name: "ef.c", text: "int main(void) { return 0; }\n" }];
    let Some(built) = compile(&scratch, GCC, &["-O2"], &program) else { return };
    let obj = built.dir.join("ef.c.o");
    let mut b = std::fs::read(&obj).unwrap();
    b[0x24..0x28].copy_from_slice(&5u32.to_le_bytes());
    std::fs::write(&obj, b).unwrap();
    let out = built.dir.join("out.mortar");
    let m = mortar(&built, &out, &[], &[]);
    assert!(!m.success(), "mortar linked e_flags 5");
    assert!(m.stderr_text().contains("unsupported feature: e_flags 0x5"), "{}", m.stderr_text());
    assert!(!out.exists(), "an output was left behind");
}
