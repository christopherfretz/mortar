//! Scan of a reduced corpus (`spec/synth.md` §16): C, C++ (exceptions,
//! templates, inline statics), Fortran and assembler with labels inside
//! string pieces, compiled by GCC, Clang 19 and Clang 22 for the three
//! targets under six flag sets. Every configuration's objects go through
//! phase 60 with no refusal (S-6 to S-12, S-34 to S-40), with `readelf`'s
//! CIE, FDE and terminator counts per object.
mod support;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use mortar_core::par::Pool;
use mortar_merge::phase;
use mortar_testutil::{Scratch, run, tool, tools_required};
use support::{Objects, readelf_frames, relocs};

const STRS_C: &str = r#"
#include <stddef.h>
#include <wchar.h>
const char *past(void) { return "abc" + 4; }
const char *before(void) { return "ijk" - 1; }
const char *mid(void) { return "hello world" + 6; }
static const char nuls[] = "p\0q\0r";
const char *nul(void) { return nuls; }
const wchar_t *wide(void) { return L"wide" + 4; }
const unsigned short *u16(void) { return (const unsigned short *)u"sixteen" + 7; }
const unsigned *u32(void) { return (const unsigned *)U"thirtytwo"; }
const char *tbl[] = { "x", "y" + 2, "zz", "end" + 4, "hello world" };
double dc(double x) { return x * 3.25 + 1.5; }
long double ldc(long double x) { return x * 3.25L; }
const char *end_of(void) { static const char s[] = "endptr"; return s + sizeof s; }
struct P { const char *a, *b; } pair = { "str_a", "str_a" + 5 };
"#;

const CXX_A: &str = r#"
#include <map>
#include <stdexcept>
#include <string>
#include <vector>
template <class T> struct Box { T v; T get() const { if (v == T()) throw std::runtime_error("zero"); return v; } };
inline int inl(int x) { static int n; ++n; if (x < 0) throw x; return x + n; }
int use1(int x) { Box<int> b{x}; try { return b.get() + inl(x); } catch (const std::exception &) { return -1; } }
std::string cat(const std::string &a) { std::map<std::string, int> m; m[a] = 1; return a + "tail" + std::to_string(m.size()); }
struct D { ~D(); };
D::~D() {}
int thrower(int x) { D d; std::vector<int> v(3); return v.at(x); }
"#;

const CXX_B: &str = r#"
#include <stdexcept>
template <class T> struct Box { T v; T get() const { if (v == T()) throw std::runtime_error("zero"); return v; } };
inline int inl(int x) { static int n; ++n; if (x < 0) throw x; return x + n; }
int use2(int x) { Box<long> b{x}; return (int)b.get() + inl(x); }
"#;

const F90: &str = r"
module m
contains
  subroutine say(s)
    character(len=*), intent(in) :: s
    print *, 'msg: ', s, ' len ', len(s)
  end subroutine
end module
subroutine run_it
  use m
  character(len=20) :: buf
  write(buf, '(A,I3)') 'value', 42
  call say(trim(buf))
  call say('literal string')
end subroutine
";

/// Labels at, inside and one past string pieces; constants addressed
/// mid-record; a function without a size.
const ASM: &str = r#"
        .section .rodata.str1.1,"aMS",@progbits,1
.Lstart: .string "first"
lab_mid: .string "second"
.Lend:
        .section .rodata.cst16,"aM",@progbits,16
        .p2align 4
cst:    .quad 1, 2
        .quad 3, 4
        .data
        .p2align 3
        .quad .Lstart
        .quad .Lend
        .quad lab_mid + 3
        .quad cst + 8
        .quad cst + 24
        .text
        .globl asmfn
asmfn:  .cfi_startproc
        .cfi_def_cfa_offset 0
        ret
        .cfi_endproc
        .size asmfn, .-asmfn
nosize: .cfi_startproc
        ret
        .cfi_endproc
"#;

struct Target {
    triple: &'static str,
    gnu: &'static str,
    hardening: &'static str,
    fortran: bool,
}

const TARGETS: [Target; 3] = [
    Target { triple: "aarch64-linux-gnu", gnu: "aarch64-linux-gnu-", hardening: "-mbranch-protection=standard", fortran: true },
    Target { triple: "x86_64-linux-gnu", gnu: "x86_64-linux-gnu-", hardening: "-fcf-protection", fortran: true },
    Target { triple: "i686-linux-gnu", gnu: "i686-linux-gnu-", hardening: "-fcf-protection", fortran: false },
];

const FLAGS: [&str; 6] = [
    "-O0 -g",
    "-O2 -ffunction-sections -fdata-sections",
    "-O2 -g -fPIC HW",
    "-O1 -fsanitize=address -fno-omit-frame-pointer",
    "-O2 -fmerge-all-constants -fno-pic -fno-PIE",
    "-Os -fno-asynchronous-unwind-tables -fno-pic -fno-PIE",
];

/// One compile: the program, its arguments, the output object.
struct Job {
    program: PathBuf,
    args: Vec<String>,
    out: PathBuf,
}

fn need(name: &str) -> Option<PathBuf> {
    let t = tool(name);
    assert!(t.is_some() || !tools_required(), "{name} is required");
    t
}

/// The configurations: (label, directory, compile jobs).
fn configurations(root: &Path, src: &Path) -> Vec<(String, PathBuf, Vec<Job>)> {
    let mut v = Vec::new();
    for t in &TARGETS {
        let gnu = |n: &str| need(&format!("{}{n}", t.gnu));
        let mut compilers: Vec<(&str, Option<PathBuf>, Option<PathBuf>, Vec<String>)> =
            vec![("gcc", gnu("gcc"), gnu("g++"), vec![])];
        for c in ["19", "22"] {
            compilers.push((
                if c == "19" { "clang-19" } else { "clang-22" },
                need(&format!("clang-{c}")),
                need(&format!("clang++-{c}")),
                vec![format!("--target={}", t.triple)],
            ));
        }
        let fortran = if t.fortran { gnu("gfortran") } else { None };
        for (cname, cc, cxx, base) in compilers {
            let (Some(cc), Some(cxx)) = (cc, cxx) else { continue };
            for (k, flags) in FLAGS.iter().enumerate() {
                let label = format!("{}-{cname}-{k}", t.triple);
                let dir = root.join(&label);
                std::fs::create_dir_all(&dir).unwrap();
                let mut args: Vec<String> = base.clone();
                args.extend(flags.replace("HW", t.hardening).split_whitespace().map(str::to_owned));
                let mut jobs = Vec::new();
                let mut job = |program: &Path, file: &str, extra: &[String]| {
                    let out = dir.join(format!("{file}.o"));
                    let mut a = args.clone();
                    a.extend_from_slice(extra);
                    a.extend([
                        "-c".into(),
                        src.join(file).display().to_string(),
                        "-o".into(),
                        out.display().to_string(),
                    ]);
                    jobs.push(Job { program: program.to_owned(), args: a, out });
                };
                job(&cc, "strs.c", &[]);
                job(&cxx, "a.cc", &[]);
                job(&cxx, "b.cc", &[]);
                if t.triple != "i686-linux-gnu" {
                    job(&cc, "s.S", &[]);
                }
                if let Some(f) = fortran.as_ref().filter(|_| cname == "gcc") {
                    job(f, "f.f90", &["-J".into(), dir.display().to_string()]);
                }
                v.push((label, dir, jobs));
            }
        }
    }
    v
}

#[derive(Default)]
struct Totals {
    configurations: usize,
    objects: usize,
    merge_sections: usize,
    pieces: usize,
    eh_records: usize,
}

#[test]
fn s6_s40_corpus_has_no_refusal_and_readelf_counts() {
    let Some(readelf) = need("readelf") else { return };
    let s = Scratch::new("merge-corpus");
    let src = s.join("src");
    std::fs::create_dir_all(&src).unwrap();
    for (n, b) in [("strs.c", STRS_C), ("a.cc", CXX_A), ("b.cc", CXX_B), ("f.f90", F90), ("s.S", ASM)] {
        std::fs::write(src.join(n), b).unwrap();
    }
    let configs = configurations(s.path(), &src);
    // Compile on three threads (each process takes a jobserver token).
    let jobs: Vec<&Job> = configs.iter().flat_map(|c| &c.2).collect();
    let next = Mutex::new(0usize);
    std::thread::scope(|scope| {
        for _ in 0..3 {
            scope.spawn(|| {
                loop {
                    let i = {
                        let mut n = next.lock().unwrap();
                        *n += 1;
                        *n - 1
                    };
                    let Some(j) = jobs.get(i) else { break };
                    let o = run(&j.program, &j.args);
                    assert!(o.success(), "{} {:?}: {}", j.program.display(), j.args, o.stderr_text());
                }
            });
        }
    });
    let pool = Pool::new(4);
    let mut t = Totals::default();
    for (label, _, jobs) in &configs {
        let paths: Vec<&PathBuf> = jobs.iter().map(|j| &j.out).collect();
        let objs = Objects::new(
            paths.iter().map(|p| (p.display().to_string(), std::fs::read(p).unwrap())).collect(),
        );
        let r = relocs(objs.machine);
        let out = phase::run(&pool, &objs, r.as_ref()).unwrap_or_else(|p| panic!("{label}: {p:?}"));
        let mut ours = vec![(0usize, 0usize, 0usize); objs.len()];
        for (k, sec) in out.eh.sections.iter().enumerate() {
            let f = objs.file_of(sec.section);
            for rec in out.eh.records_of(k) {
                if rec.cie().is_some() {
                    ours[f].0 += 1;
                } else {
                    ours[f].1 += 1;
                }
            }
            ours[f].2 += usize::from(sec.terminator.is_some());
        }
        for (f, p) in paths.iter().enumerate() {
            let theirs = readelf_frames(&readelf, p);
            assert_eq!(theirs, [ours[f]], "{label}: {} (CIEs, FDEs, terminators)", p.display());
        }
        t.configurations += 1;
        t.objects += objs.len();
        t.merge_sections += out.split.merge_of.len();
        t.pieces += out.split.piece_off.len();
        t.eh_records += out.eh.records.len();
    }
    eprintln!(
        "corpus: {} configurations, {} objects, {} merge sections, {} pieces, {} .eh_frame records",
        t.configurations, t.objects, t.merge_sections, t.pieces, t.eh_records
    );
    assert!(t.configurations >= 54 || !tools_required(), "{} configurations", t.configurations);
}
