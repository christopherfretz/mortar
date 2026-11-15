//! The i386 corpus (`spec/plumb.md` §9.1, §5.11): C, C++ with exceptions
//! and TLS, IFUNC and Fortran from GCC and Clang at -O0 and -O2; not PIC,
//! PIE and PIC objects; `-fno-plt`; both TLS dialects; and the
//! `spec/x86.md` §15.2 inputs W1–W5, W10, W11 and W14. Each is linked
//! static by GNU ld, lld 22 and mold with a map. Every link must be judged
//! with no finding but the anomalies it expects (§10.2), and every
//! expected anomaly or violation must be found: the defects of the
//! references are caught, never "ok". Static-pie outputs are outside the
//! stage-1 scope (S1, `e_type` 3) and are not linked here.

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use common::{Link, Linker, known_anomaly};
use mortar_plumb::Report;
use mortar_testutil::{Scratch, tool};

const RELAX_S: &str = include_str!("x86/relax.s");
const WFORMS_S: &str = include_str!("x86/wforms.s");
const BITFIELD_S: &str = include_str!("x86/bitfield.s");
const DYNTYPES_S: &str = include_str!("x86/dyntypes.s");
const IFUNC_C: &str = include_str!("x86/ifunc.c");
const IFUNC_MAIN_C: &str = include_str!("x86/ifunc_main.c");
const SD_C: &str = include_str!("x86/sd.c");
const HELLO_C: &str = include_str!("x86/hello.c");
const HELLO_CC: &str = include_str!("x86/hello.cc");
const HELLO_F90: &str = include_str!("x86/hello.f90");
const TLS_C: &str = include_str!("x86/tls.c");
const TLS_MAIN_C: &str = include_str!("x86/tls_main.c");

struct Tools {
    gcc: PathBuf,
    gxx: PathBuf,
    gfortran: PathBuf,
    clang: PathBuf,
    asm: PathBuf,
    ld: [PathBuf; 3],
}

fn tools() -> Option<Tools> {
    Some(Tools {
        gcc: tool("i686-linux-gnu-gcc")?,
        gxx: tool("i686-linux-gnu-g++")?,
        gfortran: tool("i686-linux-gnu-gfortran")?,
        clang: tool("clang-22")?,
        asm: tool("i686-linux-gnu-as")?,
        ld: [tool("i686-linux-gnu-ld.bfd")?, tool("ld.lld-22")?, tool("mold")?],
    })
}

fn run_in(dir: &Path, program: &Path, args: &[&str]) -> bool {
    let mut c = std::process::Command::new(program);
    c.current_dir(dir).args(args);
    mortar_testutil::run_with(&mut c, b"", mortar_testutil::DEFAULT_TIMEOUT).success()
}

fn must(dir: &Path, program: &Path, args: &[&str]) {
    assert!(
        run_in(dir, program, args),
        "{} {} failed in {}",
        program.display(),
        args.join(" "),
        dir.display()
    );
}

/// One case: what to link and, per linker, what the verifier must say.
struct Case {
    name: &'static str,
    driver: fn(&Tools) -> &Path,
    args: &'static [&'static str],
    /// Per linker (bfd, lld, mold): `None` when it refuses the link, else
    /// the anomalies and rules the report must contain beyond the
    /// run-wide defaults.
    expect: [Option<&'static [&'static str]>; 3],
}

fn gcc(t: &Tools) -> &Path {
    &t.gcc
}
fn gxx(t: &Tools) -> &Path {
    &t.gxx
}
fn gf(t: &Tools) -> &Path {
    &t.gfortran
}

const CLEAN: Option<&[&str]> = Some(&[]);

/// The C cases: one per object, each linked with the C runtime.
const C_OBJECTS: &[&str] = &[
    "c-O0.o",
    "c-O2.o",
    "c-pie.o",
    "c-pic.o",
    "c-picnoplt.o",
    "c-clang.o",
    "c-clang-pie.o",
    "c-clang-pic-O0.o",
];

#[allow(clippy::too_many_lines)] // one entry per case
fn cases() -> Vec<Case> {
    let nostd: &[&str] = &["-static", "-nostdlib"];
    let mut v = vec![
        // §5.11: every form, per linker.
        Case { name: "relax", driver: gcc, args: nostd, expect: [CLEAN; 3] },
        // W3 in GCC's own output: `call *puts@GOT` with no base.
        Case {
            name: "c-noplt",
            driver: gcc,
            args: &["-static", "c-noplt.o"],
            expect: [CLEAN, CLEAN, Some(&["R4"])],
        },
        Case { name: "cxx", driver: gxx, args: &["-static", "cxx.o"], expect: [CLEAN; 3] },
        Case { name: "cxx-O0", driver: gxx, args: &["-static", "cxx0.o"], expect: [CLEAN; 3] },
        Case {
            name: "cxx-clang",
            driver: gxx,
            args: &["-static", "cxx-clang.o"],
            expect: [CLEAN; 3],
        },
        // glibc's clone3.o has an FDE of address range 0: mold lists it in
        // .eh_frame_hdr (A19).
        Case {
            name: "fortran",
            driver: gf,
            args: &["-static", "f.o"],
            expect: [CLEAN, CLEAN, Some(&["A19"])],
        },
        Case {
            name: "tls-gd-ld",
            driver: gcc,
            args: &["-static", "tls-pic.o", "tls_main.o"],
            expect: [CLEAN; 3],
        },
        Case {
            name: "tls-desc",
            driver: gcc,
            args: &["-static", "tls-desc.o", "tls_main.o"],
            expect: [CLEAN; 3],
        },
        Case {
            name: "tls-clang-desc",
            driver: gcc,
            args: &["-static", "tls-clang-desc.o", "tls_main.o"],
            expect: [CLEAN; 3],
        },
        Case {
            name: "tls-ie",
            driver: gcc,
            args: &["-static", "tls-nopic.o", "tls_main.o"],
            expect: [CLEAN; 3],
        },
        // W5: GNU ld's GOTOFF of a hidden IFUNC is its IPLT entry − 12 (F4).
        Case {
            name: "w5",
            driver: gcc,
            args: &["-static", "ifunc.o", "ifunc_main.o"],
            expect: [Some(&["F4"]), CLEAN, CLEAN],
        },
        // W1: two readings of the operand (X86-13): R8 under every map.
        Case {
            name: "w1",
            driver: gcc,
            args: &["-static", "-nostdlib", "w1.o"],
            expect: [Some(&["R8"]); 3],
        },
        // W2: the moffs store reads as based (X86-13); accepted risk.
        Case { name: "w2", driver: gcc, args: &["-static", "-nostdlib", "w2.o"], expect: [CLEAN; 3] },
        // W3: mold's no-base GOT forms: G − .got (R4), and a `lea` (R1).
        Case {
            name: "w3",
            driver: gcc,
            args: &["-static", "-nostdlib", "w3.o"],
            expect: [CLEAN, CLEAN, Some(&["R1", "R4"])],
        },
        // W4: mold's LE form from P − 3 over D3.
        Case {
            name: "w4",
            driver: gcc,
            args: &["-static", "-nostdlib", "w4.o"],
            expect: [CLEAN, CLEAN, Some(&["R1"])],
        },
        // W10: GNU ld and mold write template addresses in .debug_addr.
        Case {
            name: "w10",
            driver: gcc,
            args: &["-static", "sd.o"],
            expect: [Some(&["A15"]), CLEAN, Some(&["A15"])],
        },
        // W11: GNU ld truncates; lld refuses; mold refuses the --defsym.
        Case {
            name: "w11",
            driver: gcc,
            args: &[
                "-static",
                "-nostdlib",
                "-Wl,--defsym,m1=-32769",
                "-Wl,--defsym,m2=-129",
                "bf.o",
            ],
            expect: [Some(&["R2"]), None, None],
        },
        // W14: GNU ld writes S + A for dynamic-only types; lld and mold refuse.
        Case {
            name: "w14",
            driver: gcc,
            args: &["-static", "-nostdlib", "dyn.o"],
            expect: [Some(&["R8"]), None, None],
        },
    ];
    for o in C_OBJECTS {
        let args: &'static [&'static str] = Box::leak(Box::new(["-static", o]));
        v.push(Case { name: o.trim_end_matches(".o"), driver: gcc, args, expect: [CLEAN; 3] });
    }
    v
}

struct Corpus {
    _scratch: Scratch,
    links: Vec<(Link, &'static [&'static str], bool)>,
}

/// The run-wide defaults (`tests/link/anomalies`): mold's on every link
/// with the C runtime (A1, A2, A3, A9; A3 and A9 without).
fn defaults(linker: Linker, crt: bool) -> &'static [&'static str] {
    match (linker, crt) {
        (Linker::Mold, true) => &["A1", "A2", "A3", "A9"],
        (Linker::Mold, false) => &["A3", "A9"],
        _ => &[],
    }
}

fn corpus() -> Option<&'static Corpus> {
    static C: OnceLock<Option<Corpus>> = OnceLock::new();
    C.get_or_init(build).as_ref()
}

#[allow(clippy::too_many_lines)] // the objects, then the links
fn build() -> Option<Corpus> {
    let t = tools()?;
    let scratch = Scratch::new("plumb-x86");
    let dir = scratch.path().to_path_buf();
    for (n, text) in [
        ("relax.s", RELAX_S),
        ("wforms.s", WFORMS_S),
        ("bitfield.s", BITFIELD_S),
        ("dyntypes.s", DYNTYPES_S),
        ("ifunc.c", IFUNC_C),
        ("ifunc_main.c", IFUNC_MAIN_C),
        ("sd.c", SD_C),
        ("hello.c", HELLO_C),
        ("hello.cc", HELLO_CC),
        ("hello.f90", HELLO_F90),
        ("tls.c", TLS_C),
        ("tls_main.c", TLS_MAIN_C),
    ] {
        std::fs::write(dir.join(n), text).unwrap();
    }
    let (cc, cxx, as_) = (&t.gcc, &t.gxx, &t.asm);
    must(&dir, as_, &["relax.s", "-o", "relax.o"]);
    for w in ["W1", "W2", "W3", "W4"] {
        let out = format!("{}.o", w.to_lowercase());
        must(&dir, as_, &["--defsym", &format!("{w}=1"), "wforms.s", "-o", &out]);
    }
    must(&dir, as_, &["bitfield.s", "-o", "bf.o"]);
    must(&dir, as_, &["dyntypes.s", "-o", "dyn.o"]);
    let c = |flags: &[&str], src: &str, out: &str| {
        let mut a: Vec<&str> = flags.to_vec();
        a.extend(["-c", src, "-o", out]);
        must(&dir, cc, &a);
    };
    c(&["-O0"], "hello.c", "c-O0.o");
    c(&["-O2"], "hello.c", "c-O2.o");
    c(&["-O2", "-fPIE"], "hello.c", "c-pie.o");
    c(&["-O2", "-fPIC"], "hello.c", "c-pic.o");
    c(&["-O2", "-fno-pic", "-fno-plt"], "hello.c", "c-noplt.o");
    c(&["-O2", "-fPIC", "-fno-plt"], "hello.c", "c-picnoplt.o");
    c(&["-O2", "-fPIE"], "ifunc.c", "ifunc.o");
    c(&["-O2", "-fPIE"], "ifunc_main.c", "ifunc_main.o");
    c(&["-g", "-O2", "-gsplit-dwarf"], "sd.c", "sd.o");
    c(&["-O2", "-fPIC"], "tls.c", "tls-pic.o");
    c(&["-O2", "-fPIC", "-mtls-dialect=gnu2"], "tls.c", "tls-desc.o");
    c(&["-O2", "-g"], "tls.c", "tls-nopic.o");
    c(&["-O2"], "tls_main.c", "tls_main.o");
    let clang = |flags: &[&str], src: &str, out: &str| {
        let mut a: Vec<&str> = vec!["--target=i686-linux-gnu"];
        a.extend(flags);
        a.extend(["-c", src, "-o", out]);
        must(&dir, &t.clang, &a);
    };
    clang(&["-O2", "-fno-pic"], "hello.c", "c-clang.o");
    clang(&["-O2", "-fPIE"], "hello.c", "c-clang-pie.o");
    clang(&["-O0", "-fPIC"], "hello.c", "c-clang-pic-O0.o");
    clang(&["-O2", "-fPIC", "-mtls-dialect=gnu2"], "tls.c", "tls-clang-desc.o");
    clang(&["-O2", "-fPIC", "-g", "-x", "c++"], "hello.cc", "cxx-clang.o");
    must(&dir, cxx, &["-O2", "-fPIC", "-c", "hello.cc", "-o", "cxx.o"]);
    must(&dir, cxx, &["-O0", "-g", "-c", "hello.cc", "-o", "cxx0.o"]);
    must(&dir, &t.gfortran, &["-O2", "-c", "hello.f90", "-o", "f.o"]);
    // Each linker as `ld` in a directory of its own, for the driver's -B.
    for (l, path) in Linker::ALL.iter().zip(&t.ld) {
        let bin = dir.join(format!("ld-{}", l.name()));
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(path, bin.join("ld")).unwrap();
    }
    let mut links = Vec::new();
    for case in cases() {
        for (k, &linker) in Linker::ALL.iter().enumerate() {
            let out = format!("{}.{}", case.name, linker.name());
            let map = format!("{out}.map");
            let bin = dir.join(format!("ld-{}", linker.name())).display().to_string();
            let mut args: Vec<String> = vec![format!("-B{bin}/")];
            args.extend(case.args.iter().map(|s| (*s).to_string()));
            if case.name == "relax" {
                args.push("relax.o".into());
            }
            args.extend(["-o".into(), out.clone(), format!("-Wl,-Map,{map}")]);
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let linked = run_in(&dir, (case.driver)(&t), &refs);
            let Some(expect) = case.expect[k] else {
                assert!(
                    !linked,
                    "{}/{}: the link was expected to be refused",
                    case.name,
                    linker.name()
                );
                continue;
            };
            assert!(linked, "{}/{}: the link failed", case.name, linker.name());
            let crt = !case.args.contains(&"-nostdlib");
            links.push((
                Link {
                    case: case.name,
                    linker,
                    dir: dir.clone(),
                    output: dir.join(&out),
                    map: dir.join(&map),
                },
                expect,
                crt,
            ));
        }
    }
    Some(Corpus { _scratch: scratch, links })
}

/// The anomaly ids of `r`'s findings, and the rule ids of the rest.
fn verdict(link: &Link, r: &Report) -> (BTreeSet<String>, BTreeSet<String>) {
    let (mut anomalies, mut rules) = (BTreeSet::new(), BTreeSet::new());
    for f in &r.findings {
        match known_anomaly(link, f) {
            Some(a) => anomalies.insert(mortar_plumb::anomaly::id(a).to_string()),
            None => rules.insert(f.rule.id()),
        };
    }
    (anomalies, rules)
}

// spec/plumb.md §9.1, §10.2; spec/x86.md §15.2.
#[test]
fn x86_corpus_is_judged_with_only_its_expected_findings() {
    let Some(c) = corpus() else { return };
    let mut failures = Vec::new();
    for (link, expect, crt) in &c.links {
        let r = link.verify();
        let (anomalies, rules) = verdict(link, &r);
        let allowed: BTreeSet<String> =
            expect.iter().chain(defaults(link.linker, *crt)).map(|s| (*s).to_string()).collect();
        let found: BTreeSet<String> = anomalies.iter().chain(&rules).cloned().collect();
        let missing: Vec<&&str> = expect.iter().filter(|e| !found.contains(**e)).collect();
        let extra: Vec<&String> = found.iter().filter(|f| !allowed.contains(*f)).collect();
        if !missing.is_empty() || !extra.is_empty() {
            failures.push(format!(
                "{}: missing {missing:?}, unexpected {extra:?}\n{}",
                link.label(),
                r.findings
                    .iter()
                    .filter(|f| extra.iter().any(|e| **e == f.rule.id()))
                    .take(5)
                    .map(|f| format!("  {} {}", f.rule.id(), f.message))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        // The table judges the i386 records: none is `machine`.
        assert!(r.coverage.records > 0, "{}: no records", link.label());
        assert!(!r.summary().contains("machine"), "{}: records not judged", link.label());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// spec/plumb.md §5.11: the relax case exercises every rewriting family
// its linker writes.
#[test]
fn x86_relax_case_accepts_every_rewriting_family() {
    let Some(c) = corpus() else { return };
    for (link, ..) in c.links.iter().filter(|(l, ..)| l.case == "relax") {
        let r = link.verify();
        let s = r.summary();
        let families: &[&str] = match link.linker {
            Linker::Bfd => &["tlsie", "tlsdesc", "tlsgd", "tlsld", "got-to-imm", "got-to-direct"],
            Linker::Lld => &["tlsie", "tlsdesc", "tlsgd", "tlsld"],
            // mold keeps IE in the GOT (X86-32).
            Linker::Mold => &["tlsdesc", "tlsgd", "tlsld", "got-to-lea"],
        };
        for family in families {
            assert!(s.contains(family), "{}: no {family} rewriting accepted:\n{s}", link.label());
        }
    }
}
