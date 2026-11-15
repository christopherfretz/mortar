//! `spec/image.md` §4, end to end: static AArch64 C, C++ and Fortran
//! programs, compiled by the AArch64 GCC (`support::ARCH`, on any host),
//! through the phases before the scan and the scan itself
//! (`support::link`), against GNU ld's and lld's outputs.

mod support;

use std::ffi::OsString;

use mortar_core::par::Pool;
use mortar_testutil::Scratch;
use support::{compile, driver_link_args, fuse_lld, link, need, need_gnu};

const HELLO: &str = "#include <stdio.h>\nint main(void){printf(\"hi %d\\n\", 3);return 0;}\n";

#[test]
fn i13_e2e_smoke_c() {
    let Some(gcc) = need_gnu("gcc") else { return };
    let dir = Scratch::new("scan-e2e-smoke");
    let Some(_o) = compile(dir.path(), "gcc", "hello.o", "c", HELLO, &["-O1"]) else { return };
    let Some(args) = driver_link_args(&gcc, &["-static", "-o", "hello", "hello.o"], dir.path())
    else {
        panic!("no link line");
    };
    let args: Vec<OsString> = args;
    let pool = Pool::new(4);
    let l = link(dir.path(), &args, &pool).expect("load");
    assert!(l.before.is_empty(), "{:?}", l.before);
    eprintln!("{:?}", l.scanned.stats);
    assert!(l.scanned.diagnostics.is_empty(), "{}", l.rendered());
}

const CXX: &str = r#"#include <iostream>
#include <stdexcept>
#include <string>
#include <vector>
#include <cmath>
thread_local int tls_count = 1;
static int thrower(int x) { if (x > 2) throw std::runtime_error("big " + std::to_string(x)); return x; }
int main(int argc, char **argv) {
    std::vector<std::string> v;
    for (int i = 0; i < argc + 3; ++i) v.push_back(std::to_string(i));
    try { thrower(argc + 5); } catch (const std::exception &e) { std::cout << e.what() << '\n'; }
    tls_count += v.size();
    std::cout << std::sqrt(double(tls_count)) << ' ' << v.back() << std::endl;
    return 0;
}
"#;

const FORTRAN: &str = "program p\n  implicit none\n  real(8), allocatable :: a(:)\n  integer :: i\n  allocate(a(10))\n  do i = 1, 10\n    a(i) = sqrt(real(i, 8))\n  end do\n  write(*, '(F10.4)') sum(a)\n  deallocate(a)\nend program p\n";

const CTLS: &str = "#include <stdio.h>\n#include <string.h>\n#include <math.h>\n__thread int t1 = 3;\nextern __thread int t2;\n__thread int t2 = 4;\nint main(int c, char **v){char b[64];strcpy(b, v[0]);t1 += strlen(b);printf(\"%d %f\\n\", t1 + t2, sin((double)c));return 0;}\n";

/// The GOT-word, IPLT and TLS demand counts of a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Counts {
    got: usize,
    got_norelax: usize,
    tprel: usize,
    tprel_norelax: usize,
    gd: usize,
    desc: usize,
    iplt: usize,
}

fn counts(s: &mortar_scan::Scanned) -> Counts {
    use mortar_core::demands::DemandFlags as F;
    let d = &s.demands;
    let n = |f| d.keys_with(f).count();
    let both = |a: F, b: F| d.keys_with(a).filter(|k| d.of(*k).contains(b)).count();
    Counts {
        got: n(F::GOT),
        got_norelax: both(F::GOT, F::GOT_NORELAX),
        tprel: n(F::GOT_TPREL),
        tprel_norelax: both(F::GOT_TPREL, F::TLSIE_NORELAX),
        gd: n(F::TLS_GD),
        desc: n(F::TLS_DESC),
        iplt: n(F::IPLT),
    }
}

/// A reference link's GOT and IRELATIVE counts: `.got` and `.got.plt`
/// sizes in words, and the number of `R_AARCH64_IRELATIVE` relocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Reference {
    got: u64,
    got_plt: u64,
    irelative: usize,
}

fn reference(dir: &std::path::Path, out: &str) -> Reference {
    let readelf = need("readelf").expect("readelf");
    let p = std::path::PathBuf::from(out);
    let s = support::run_in(&readelf, &["-SW".into(), p.clone().into()], dir);
    assert!(s.success(), "{}", s.stderr_text());
    let size = |name: &str| {
        s.stdout_text()
            .lines()
            .find_map(|l| {
                let l = l.split_once(']')?.1;
                let w: Vec<&str> = l.split_whitespace().collect();
                (w.first() == Some(&name)).then(|| u64::from_str_radix(w[4], 16).unwrap() / 8)
            })
            .unwrap_or(0)
    };
    let r = support::run_in(&readelf, &["-rW".into(), p.into()], dir);
    assert!(r.success(), "{}", r.stderr_text());
    let irelative = r.stdout_text().lines().filter(|l| l.contains("R_AARCH64_IRELATIVE")).count();
    Reference { got: size(".got"), got_plt: size(".got.plt"), irelative }
}

/// Compiles `src`, then: mortar's phases to the scan on the driver's
/// static link line, ld.bfd's and ld.lld's links of the same objects.
fn differential(
    name: &str,
    driver: &str,
    ext: &str,
    src: &str,
    libs: &[&str],
) -> Option<(Counts, Reference, Reference)> {
    let cc = need_gnu(driver)?;
    need_gnu("ld.bfd")?;
    let lld = need("ld.lld")?;
    let dir = Scratch::new(&format!("scan-e2e-{name}"));
    compile(dir.path(), driver, "main.o", ext, src, &["-O2", "-g"])?;
    let mut line = vec!["-static", "-o", "out", "main.o"];
    line.extend(libs);
    let args = driver_link_args(&cc, &line, dir.path()).expect("no link line");
    let l = link(dir.path(), &args, &Pool::new(4)).expect("load");
    assert!(l.before.is_empty(), "{name}: {:?}", l.before);
    assert!(l.scanned.diagnostics.is_empty(), "{name}: {}", l.rendered());
    l.scanned.demands.check(l.inputs.symbols.len()).unwrap();
    let mut refs = Vec::new();
    for ld in ["bfd", "lld"] {
        let out = format!("out.{ld}");
        let mut a: Vec<OsString> = vec!["-static".into()];
        if ld == "lld" {
            a.extend(fuse_lld(dir.path(), &lld));
        } else {
            a.push("-fuse-ld=bfd".into());
        }
        a.extend(["-o".into(), out.clone().into(), "main.o".into()]);
        a.extend(libs.iter().map(OsString::from));
        let r = support::run_in(&cc, &a, dir.path());
        assert!(r.success(), "{name} {ld}: {}", r.stderr_text());
        refs.push(reference(dir.path(), &out));
    }
    let c = counts(&l.scanned);
    eprintln!(
        "{name}: mortar {c:?} words {}; ld.bfd {:?}; ld.lld {:?}; {:?}",
        c.got + c.tprel + 2 * c.gd + 2 * c.desc,
        refs[0],
        refs[1],
        l.scanned.stats
    );
    Some((c, refs[0], refs[1]))
}

/// The explained relation of mortar's demands to a reference link's
/// (`spec/image.md` §4, a64.md §8), each difference by its rule:
///
/// - one IPLT entry per non-preemptible IFUNC, an `IRELATIVE` each, its
///   slot in `.got.plt` (I-16), after ld.bfd's three reserved words;
/// - a `.got` slot per `GOT` key: R5 relaxes the code, not the slot
///   (kind 2), as lld does; ld.bfd adds its reserved word `.got[0]`;
/// - a `GOT_TPREL` slot per IE key: R3 relaxes the code and keeps the
///   slot (kind 2); lld drops every one in a static link; ld.bfd keeps
///   some (at most one per mortar `GOT_TPREL` key).
fn explained(name: &str, c: Counts, r: Reference, ld: &str) {
    assert_eq!(c.iplt, r.irelative, "{name} {ld}: IPLT entries against IRELATIVE relocations");
    let (reserved_plt, reserved_got) = if ld == "ld.bfd" { (3, 1) } else { (0, 0) };
    assert_eq!(r.got_plt, reserved_plt + r.irelative as u64, "{name} {ld}: .got.plt");
    let got = c.got as u64 + reserved_got;
    assert!(r.got >= got, "{name} {ld}: .got {} < {got}", r.got);
    let tls_kept = r.got - got;
    if ld == "ld.lld" {
        assert_eq!(tls_kept, 0, "{name} {ld}: .got against GOT keys");
    }
    assert!(
        tls_kept <= (c.tprel + 2 * c.gd + 2 * c.desc) as u64,
        "{name} {ld}: {tls_kept} TLS words beyond mortar's TLS demands"
    );
}

#[test]
fn i16_e2e_differential_c() {
    let Some((c, bfd, lld)) = differential("c", "gcc", "c", CTLS, &["-lm"]) else { return };
    explained("c", c, bfd, "ld.bfd");
    explained("c", c, lld, "ld.lld");
}

#[test]
fn i16_e2e_differential_cxx() {
    let Some((c, bfd, lld)) = differential("cxx", "g++", "cc", CXX, &[]) else { return };
    explained("cxx", c, bfd, "ld.bfd");
    explained("cxx", c, lld, "ld.lld");
}

#[test]
fn i16_e2e_differential_fortran() {
    let Some((c, bfd, lld)) = differential("f", "gfortran", "f90", FORTRAN, &[]) else { return };
    explained("f", c, bfd, "ld.bfd");
    explained("f", c, lld, "ld.lld");
}

/// The symbols of GNU ld's `undefined reference to` lines.
fn gnu_undefined(text: &str) -> std::collections::BTreeSet<String> {
    text.lines()
        .filter_map(|l| {
            let at = l.find("undefined reference to `")? + "undefined reference to `".len();
            let rest = &l[at..];
            Some(rest[..rest.find('\'')?].to_owned())
        })
        .collect()
}

#[test]
fn i20_e2e_undefined_set_equals_gnu_ld() {
    let Some(cc) = need_gnu("g++") else { return };
    let Some(bfd) = need_gnu("ld.bfd") else { return };
    let dir = Scratch::new("scan-e2e-undef");
    compile(dir.path(), "g++", "main.o", "cc", CXX, &["-O2"]).expect("g++");
    let args = driver_link_args(&cc, &["-static", "-o", "out", "main.o"], dir.path())
        .expect("no link line");
    // Without the C++ library (and libm, which it pulls).
    let args: Vec<OsString> = args.into_iter().filter(|a| a != "-lstdc++" && a != "-lm").collect();
    let l = link(dir.path(), &args, &Pool::new(4)).expect("load");
    let ours: std::collections::BTreeSet<String> = l
        .scanned
        .diagnostics
        .iter()
        .filter_map(|(_, d)| match d.kind() {
            mortar_diag::Kind::UndefinedSymbol { name } => Some(name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(l.ids().iter().filter(|i| **i != "undefined-symbol").count(), 0, "{}", l.rendered());
    let mut a: Vec<OsString> = vec!["--no-demangle".into()];
    a.extend(args);
    let r = support::run_in(&bfd, &a, dir.path());
    assert!(!r.success());
    let theirs = gnu_undefined(&r.stderr_text());
    eprintln!("{} undefined symbols", theirs.len());
    assert!(!theirs.is_empty());
    assert_eq!(ours, theirs);
}
