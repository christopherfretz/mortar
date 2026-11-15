//! The x86-64 corpus (`spec/plumb.md` §9.1, §9.5): C, C++ and Fortran
//! from GCC and Clang, not PIC, PIE and PIC objects, `-fno-plt`, the TLS
//! dialects, and the `spec/x64.md` §14.2 cases W1, W4, W8, W9 and W10,
//! each linked static by GNU ld, lld 22 and mold with a map. Every link
//! must be judged with no finding but the anomalies it expects (§10.2),
//! and every expected anomaly or violation must be found: the defects of
//! the references are caught, never "ok".

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use common::{Link, Linker, known_anomaly};
use mortar_plumb::Report;
use mortar_testutil::{Scratch, tool};

const RELAX_S: &str = include_str!("x64/relax.s");
const BITFIELD_S: &str = include_str!("x64/bitfield.s");
const GD_C: &str = include_str!("x64/gd.c");
const F2_S: &str = include_str!("x64/f2.s");
const LG_C: &str = include_str!("x64/lg.c");
const SD_C: &str = include_str!("x64/sd.c");
const HELLO_CC: &str = include_str!("x64/hello.cc");
const TLS_C: &str = include_str!("x64/tls.c");
const TLS_MAIN_C: &str = include_str!("x64/tls_main.c");
const HELLO_F90: &str = include_str!("x64/hello.f90");
const HELLO_C: &str = "#include <stdio.h>\n#include <string.h>\n\
    int main(int c, char **v) { printf(\"%zu\\n\", strlen(v[0]) + c); return 0; }\n";

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
        gcc: tool("x86_64-linux-gnu-gcc")?,
        gxx: tool("x86_64-linux-gnu-g++")?,
        gfortran: tool("x86_64-linux-gnu-gfortran")?,
        clang: tool("clang-22")?,
        asm: tool("x86_64-linux-gnu-as")?,
        ld: [tool("x86_64-linux-gnu-ld.bfd")?, tool("ld.lld-22")?, tool("mold")?],
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

fn cases() -> Vec<Case> {
    let nostd: &[&str] = &["-static", "-nostdlib"];
    vec![
        // §5.9: every form, per linker. lld writes W4 over the large GD.
        Case { name: "relax", driver: gcc, args: nostd, expect: [CLEAN, Some(&["A12"]), CLEAN] },
        Case { name: "c-gcc", driver: gcc, args: &["-static", "c-gcc.o"], expect: [CLEAN; 3] },
        Case { name: "c-gcc-pie", driver: gcc, args: &["-static", "c-pie.o"], expect: [CLEAN; 3] },
        Case {
            name: "c-gcc-noplt",
            driver: gcc,
            args: &["-static", "c-noplt.o"],
            expect: [CLEAN; 3],
        },
        Case { name: "c-clang", driver: gcc, args: &["-static", "c-clang.o"], expect: [CLEAN; 3] },
        Case {
            name: "c-clang-pic",
            driver: gcc,
            args: &["-static", "c-clang-pic.o"],
            expect: [CLEAN; 3],
        },
        Case { name: "cxx", driver: gxx, args: &["-static", "cxx.o"], expect: [CLEAN; 3] },
        Case { name: "cxx-O0", driver: gxx, args: &["-static", "cxx0.o"], expect: [CLEAN; 3] },
        Case { name: "fortran", driver: gf, args: &["-static", "f.o"], expect: [CLEAN; 3] },
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
            name: "tls-ie",
            driver: gcc,
            args: &["-static", "tls-nopic.o", "tls_main.o"],
            expect: [CLEAN; 3],
        },
        // W1: GNU ld truncates; lld and mold refuse. A violation (R2).
        Case {
            name: "w1",
            driver: gcc,
            args: &["-static", "-nostdlib", "bf.o"],
            expect: [Some(&["R2"]), None, None],
        },
        // W4: lld's GD rewriting of the large model.
        Case {
            name: "w4",
            driver: gcc,
            args: &["-static", "gd.o"],
            expect: [CLEAN, Some(&["A12"]), CLEAN],
        },
        // W8: lld and mold trap in `.init` padding.
        Case {
            name: "w8",
            driver: gcc,
            args: &["-static", "c-gcc.o", "f2.o"],
            expect: [CLEAN, Some(&["A16"]), Some(&["A16"])],
        },
        // W9: lld makes the large common absolute; mold refuses.
        Case {
            name: "w9",
            driver: gcc,
            args: &["-static", "lg.o"],
            expect: [CLEAN, Some(&["A14"]), None],
        },
        // W10: GNU ld and mold write template addresses in .debug_addr.
        Case {
            name: "w10",
            driver: gcc,
            args: &["-static", "sd.o"],
            expect: [Some(&["A15"]), CLEAN, Some(&["A15"])],
        },
    ]
}

struct Corpus {
    _scratch: Scratch,
    links: Vec<(Link, &'static [&'static str], bool)>,
}

/// The run-wide defaults (`tests/link/anomalies`): mold's on every link
/// with the C runtime (A1, A2, A3, A9, A13; A3 and A9 without), GNU ld's
/// A10 with the C runtime.
fn defaults(linker: Linker, crt: bool) -> &'static [&'static str] {
    match (linker, crt) {
        (Linker::Mold, true) => &["A1", "A2", "A3", "A9", "A13"],
        (Linker::Mold, false) => &["A3", "A9"],
        (Linker::Bfd, true) => &["A10"],
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
    let scratch = Scratch::new("plumb-x64");
    let dir = scratch.path().to_path_buf();
    for (n, text) in [
        ("relax.s", RELAX_S),
        ("bitfield.s", BITFIELD_S),
        ("gd.c", GD_C),
        ("f2.s", F2_S),
        ("lg.c", LG_C),
        ("sd.c", SD_C),
        ("hello.cc", HELLO_CC),
        ("tls.c", TLS_C),
        ("tls_main.c", TLS_MAIN_C),
        ("hello.f90", HELLO_F90),
        ("hello.c", HELLO_C),
    ] {
        std::fs::write(dir.join(n), text).unwrap();
    }
    let (cc, cxx, as_) = (&t.gcc, &t.gxx, &t.asm);
    must(&dir, as_, &["--defsym", "GDL=1", "--defsym", "LDL=1", "relax.s", "-o", "relax.o"]);
    must(&dir, as_, &["--defsym", "GDL=1", "relax.s", "-o", "relax-nold.o"]);
    must(&dir, as_, &["--defsym", "m1=-32769", "--defsym", "m2=-129", "bitfield.s", "-o", "bf.o"]);
    must(&dir, as_, &["f2.s", "-o", "f2.o"]);
    must(&dir, cc, &["-O2", "-c", "hello.c", "-o", "c-gcc.o"]);
    must(&dir, cc, &["-O2", "-fPIE", "-c", "hello.c", "-o", "c-pie.o"]);
    must(&dir, cc, &["-O2", "-fPIC", "-fno-plt", "-c", "hello.c", "-o", "c-noplt.o"]);
    let clang = |pic: &str, out: &str| {
        must(
            &dir,
            &t.clang,
            &["--target=x86_64-linux-gnu", "-O2", pic, "-c", "hello.c", "-o", out],
        );
    };
    clang("-fno-pic", "c-clang.o");
    clang("-fPIC", "c-clang-pic.o");
    must(&dir, cxx, &["-O2", "-c", "hello.cc", "-o", "cxx.o"]);
    must(&dir, cxx, &["-O0", "-g", "-c", "hello.cc", "-o", "cxx0.o"]);
    must(&dir, &t.gfortran, &["-O2", "-c", "hello.f90", "-o", "f.o"]);
    must(&dir, cc, &["-O2", "-fPIC", "-c", "tls.c", "-o", "tls-pic.o"]);
    must(&dir, cc, &["-O2", "-fPIC", "-mtls-dialect=gnu2", "-c", "tls.c", "-o", "tls-desc.o"]);
    must(&dir, cc, &["-O2", "-g", "-c", "tls.c", "-o", "tls-nopic.o"]);
    must(&dir, cc, &["-O2", "-c", "tls_main.c", "-o", "tls_main.o"]);
    must(&dir, cc, &["-O2", "-fPIC", "-mcmodel=large", "-c", "gd.c", "-o", "gd.o"]);
    must(&dir, cc, &["-O2", "-mcmodel=medium", "-fcommon", "-c", "lg.c", "-o", "lg.o"]);
    must(&dir, cc, &["-g", "-O2", "-gsplit-dwarf", "-c", "sd.c", "-o", "sd.o"]);
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
                args.push(if linker == Linker::Lld { "relax-nold.o" } else { "relax.o" }.into());
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

// spec/plumb.md §9.1, §10.2; spec/x64.md §14.2.
#[test]
fn x64_corpus_is_judged_with_only_its_expected_findings() {
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
        // The table judges the x86-64 records: most are verified.
        assert!(r.coverage.records > 0, "{}: no records", link.label());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// spec/plumb.md §5.9: the relax case exercises every rewriting family.
#[test]
fn x64_relax_case_accepts_every_rewriting_family() {
    let Some(c) = corpus() else { return };
    for (link, ..) in c.links.iter().filter(|(l, ..)| l.case == "relax") {
        let r = link.verify();
        let s = r.summary();
        for family in ["tlsie", "tlsdesc", "tlsgd", "tlsld", "got-to"] {
            // mold keeps GD (a GOT pair) and IE `add`; its LD is its own form.
            if link.linker == Linker::Mold && family == "tlsgd" {
                continue;
            }
            assert!(s.contains(family), "{}: no {family} rewriting accepted:\n{s}", link.label());
        }
    }
}
