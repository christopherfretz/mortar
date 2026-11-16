//! Stage 3 on static-pie (`spec/plumb.md` §12.4, WP-089): GNU ld, lld 22
//! and mold, AArch64 and x86-64, lazy and `-z now`, each linked
//! `-static-pie` through a driver whose `ld` records its words and
//! verified with the map and the line. Every output must be clean but for
//! the recorded anomalies (A30-A32 are each shown by running a program);
//! the outputs of the host's architecture are run. glibc's order of
//! application is measured on rewritten outputs; the static-pie mutation
//! classes must all be caught where they apply. Every source is in
//! `tests/spie/`; every binary is built at test time.
// ELF fields and addresses read best by the specs' short names.
#![allow(clippy::many_single_char_names, clippy::similar_names)]

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use common::{Link, Linker, known_anomaly};
use mortar_plumb::{Ctx, Files, Input, Report};
use mortar_testutil::{Scratch, tool};

macro_rules! src {
    ($($n:literal),* $(,)?) => { &[$(($n, include_bytes!(concat!("spie/", $n)))),*] };
}

const SOURCES: &[(&str, &[u8])] = src![
    "hello.c",
    "ifunc.c",
    "data.c",
    "cxx.cc",
    "tlsdyn.cc",
    "hello.f90",
    "order.c",
    "absgot.c",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arch {
    A64,
    X64,
}

impl Arch {
    fn name(self) -> &'static str {
        match self {
            Arch::A64 => "a64",
            Arch::X64 => "x64",
        }
    }

    fn host(self) -> bool {
        std::env::consts::ARCH == if self == Arch::A64 { "aarch64" } else { "x86_64" }
    }
}

struct Tools {
    cc: [PathBuf; 2],
    cxx: [PathBuf; 2],
    fc: [Option<PathBuf>; 2],
    bfd: [PathBuf; 2],
    lld: PathBuf,
    mold: PathBuf,
}

fn tools() -> Option<Tools> {
    Some(Tools {
        cc: [tool("aarch64-linux-gnu-gcc").or_else(|| tool("gcc"))?, tool("x86_64-linux-gnu-gcc")?],
        cxx: [
            tool("aarch64-linux-gnu-g++").or_else(|| tool("g++"))?,
            tool("x86_64-linux-gnu-g++")?,
        ],
        fc: [
            tool("aarch64-linux-gnu-gfortran").or_else(|| tool("gfortran")),
            tool("x86_64-linux-gnu-gfortran"),
        ],
        bfd: [
            tool("aarch64-linux-gnu-ld.bfd").or_else(|| tool("ld.bfd"))?,
            tool("x86_64-linux-gnu-ld.bfd")?,
        ],
        lld: tool("ld.lld-22")?,
        mold: tool("mold")?,
    })
}

/// One output and the anomalies it must show beyond the defaults.
struct Out {
    case: &'static str,
    arch: Arch,
    link: Link,
    line: PathBuf,
    expect: Vec<&'static str>,
}

struct Dir<'a>(&'a Path);

impl Files for Dir<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

impl Out {
    fn label(&self) -> String {
        let name = self.link.output.file_name().unwrap_or_default().to_string_lossy();
        format!("{name} [{}]", self.arch.name())
    }

    fn verify_bytes(&self, bytes: &[u8]) -> Report {
        let map = self.link.parse_map();
        let files = self.link.inputs(&map);
        let inputs: Vec<Input<'_>> =
            files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        let line: Vec<String> =
            std::fs::read_to_string(&self.line).unwrap().lines().map(str::to_string).collect();
        let ctx = Ctx { line: Some(line), files: &Dir(&self.link.dir) };
        mortar_plumb::verify_with(bytes, &inputs, &map, &ctx)
    }

    fn verify(&self) -> Report {
        self.verify_bytes(&self.link.output_bytes())
    }
}

struct Corpus {
    _scratch: Scratch,
    outs: Vec<Out>,
}

fn corpus() -> Option<&'static Corpus> {
    static C: OnceLock<Option<Corpus>> = OnceLock::new();
    C.get_or_init(build).as_ref()
}

fn run_in(dir: &Path, program: &Path, args: &[&str], line: Option<&Path>) -> bool {
    let mut c = std::process::Command::new(program);
    c.current_dir(dir).args(args);
    if let Some(l) = line {
        c.env("LINE_OUT", l);
    }
    mortar_testutil::run_with(&mut c, b"", mortar_testutil::DEFAULT_TIMEOUT).success()
}

/// The exit status of `program`, run in `dir`.
fn status(dir: &Path, program: &Path) -> Option<i32> {
    let mut c = std::process::Command::new(program);
    c.current_dir(dir);
    mortar_testutil::run_with(&mut c, b"", mortar_testutil::DEFAULT_TIMEOUT)
        .status
        .and_then(|s| s.code())
}

/// The anomalies each case shows, by architecture and linker (§10.2).
fn expect(case: &str, arch: Arch, linker: Linker) -> Vec<&'static str> {
    let mut v = Vec::new();
    match (arch, linker) {
        // glibc's `_nl_current_*_used` (absolute, value 2) through the GOT.
        (Arch::A64, Linker::Bfd) => v.push("A30"),
        // crtbeginS's `__cxa_finalize@PLT`.
        (Arch::X64, Linker::Bfd) => v.push("A31"),
        _ => {}
    }
    if case == "ifunc" && arch == Arch::X64 && linker != Linker::Lld {
        v.push("A32");
    }
    v
}

#[allow(clippy::too_many_lines)] // one block per case
fn build() -> Option<Corpus> {
    let t = tools()?;
    let scratch = Scratch::new("plumb-spie");
    let root = scratch.path().to_path_buf();
    for (n, text) in SOURCES {
        std::fs::write(root.join(n), text).unwrap();
    }
    for arch in [Arch::A64, Arch::X64] {
        for (l, real) in Linker::ALL.iter().zip([&t.bfd[arch as usize], &t.lld, &t.mold]) {
            let bin = root.join(format!("ld-{}-{}", arch.name(), l.name()));
            std::fs::create_dir_all(&bin).unwrap();
            let script = format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$LINE_OUT\"\nexec {} \"$@\"\n",
                real.display()
            );
            let p = bin.join("ld");
            std::fs::write(&p, script).unwrap();
            let mut perm = std::fs::metadata(&p).unwrap().permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
            std::fs::set_permissions(&p, perm).unwrap();
        }
    }
    let mut outs = Vec::new();
    for arch in [Arch::A64, Arch::X64] {
        let i = arch as usize;
        let dir = root.join(arch.name());
        std::fs::create_dir_all(&dir).unwrap();
        let mut cases: Vec<(&'static str, &Path, &str)> = vec![
            ("hello", &t.cc[i], "hello.c"),
            ("ifunc", &t.cc[i], "ifunc.c"),
            ("data", &t.cc[i], "data.c"),
            ("order", &t.cc[i], "order.c"),
            ("cxx", &t.cxx[i], "cxx.cc"),
            ("tlsdyn", &t.cxx[i], "tlsdyn.cc"),
        ];
        // A30 is AArch64's (x86-64 code reaches an absolute symbol
        // PC-relatively, which no static-pie can relocate).
        if arch == Arch::A64 {
            cases.push(("absgot", &t.cc[i], "absgot.c"));
        }
        if let Some(fc) = &t.fc[i] {
            cases.push(("fhello", fc, "hello.f90"));
        }
        for (case, driver, src) in cases {
            let obj = format!("{case}.o");
            let s = root.join(src).display().to_string();
            assert!(
                run_in(&dir, driver, &["-O1", "-fPIE", "-c", &s, "-o", &obj], None),
                "{case}: compile"
            );
            for linker in Linker::ALL {
                for now in [false, true] {
                    // order and absgot are measured once, lazily.
                    if now && matches!(case, "order" | "absgot") {
                        continue;
                    }
                    let name = format!("{case}.{}{}", linker.name(), if now { ".now" } else { "" });
                    let bin = root.join(format!("ld-{}-{}", arch.name(), linker.name()));
                    let b = format!("-B{}/", bin.display());
                    let m = format!("-Wl,-Map={name}.map");
                    let line = dir.join(format!("{name}.line"));
                    let mut a = vec![b.as_str(), "-static-pie", &obj, "-o", &name, &m];
                    if now {
                        a.push("-Wl,-z,now");
                    }
                    assert!(
                        run_in(&dir, driver, &a, Some(&line)),
                        "{name} [{}]: link",
                        arch.name()
                    );
                    let mut e = expect(case, arch, linker);
                    if case == "absgot" && arch == Arch::A64 && linker == Linker::Bfd {
                        e = vec!["A30"];
                    }
                    outs.push(Out {
                        case,
                        arch,
                        link: Link {
                            case,
                            linker,
                            dir: dir.clone(),
                            output: dir.join(&name),
                            map: dir.join(format!("{name}.map")),
                        },
                        line,
                        expect: e,
                    });
                }
            }
        }
    }
    Some(Corpus { _scratch: scratch, outs })
}

/// The anomalies a linker shows on every static-pie (the run-wide
/// defaults of `tests/link/anomalies`), allowed but not required.
fn defaults(linker: Linker) -> &'static [&'static str] {
    match linker {
        Linker::Mold => &["A1", "A2", "A3", "A9", "A13"],
        Linker::Bfd => &["A10"],
        Linker::Lld => &[],
    }
}

/// The finding ids of `r` (anomaly ids where a finding is one).
fn found(o: &Out, r: &Report) -> BTreeSet<String> {
    r.findings
        .iter()
        .map(|f| match known_anomaly(&o.link, f) {
            Some(a) => mortar_plumb::anomaly::id(a).to_string(),
            None => f.rule.id(),
        })
        .collect()
}

fn listing(o: &Out, r: &Report) -> String {
    r.findings
        .iter()
        .filter(|f| known_anomaly(&o.link, f).is_none())
        .take(6)
        .map(|f| format!("  {} {}", f.rule.id(), f.message))
        .collect::<Vec<_>>()
        .join("\n")
}

// spec/plumb.md §12.4: every static-pie of the references is judged by
// stage 3 and clean but for its recorded anomalies; those of the host's
// architecture run.
#[test]
fn s12_4_static_pie_references_verify_clean() {
    let Some(c) = corpus() else { return };
    let mut failures = Vec::new();
    for o in &c.outs {
        let r = o.verify();
        let f = found(o, &r);
        assert!(r.coverage.records > 0, "{}: no records", o.label());
        let allowed: BTreeSet<String> =
            o.expect.iter().chain(defaults(o.link.linker)).map(|s| (*s).to_string()).collect();
        let missing: Vec<&&str> = o.expect.iter().filter(|e| !f.contains(**e)).collect();
        let extra: Vec<&String> = f.iter().filter(|x| !allowed.contains(*x)).collect();
        if !missing.is_empty() || !extra.is_empty() {
            failures.push(format!(
                "{}: missing {missing:?}, unexpected {extra:?}\n{}",
                o.label(),
                listing(o, &r)
            ));
        }
        if o.arch.host() && o.case != "order" && o.case != "absgot" {
            assert_eq!(status(&o.link.dir, &o.link.output), Some(0), "{}: run", o.label());
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// A30 is real: GNU ld's AArch64 GOT slot of an absolute symbol gets a
// RELATIVE record, and the program reads 2 plus the load bias (exit 1);
// lld's and mold's read 2.
#[test]
fn s12_4_a30_absolute_got_slot_runs_wrong() {
    let Some(c) = corpus() else { return };
    for o in c.outs.iter().filter(|o| o.case == "absgot" && o.arch == Arch::A64 && o.arch.host()) {
        let want = i32::from(o.link.linker == Linker::Bfd);
        assert_eq!(status(&o.link.dir, &o.link.output), Some(want), "{}", o.label());
    }
}

/// `.dynamic`'s entries of `b`: `(file offset of the entry, tag, value)`.
fn dynamic(b: &[u8]) -> Vec<(usize, u64, u64)> {
    let q = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    let (phoff, phnum) = (q(32) as usize, usize::from(u16::from_le_bytes([b[56], b[57]])));
    let mut v = Vec::new();
    for i in 0..phnum {
        let h = phoff + 56 * i;
        if u32::from_le_bytes(b[h..h + 4].try_into().unwrap()) == 2 {
            let (o, z) = (q(h + 8) as usize, q(h + 32) as usize);
            for k in (o..o + z).step_by(16) {
                v.push((k, q(k), q(k + 8)));
                if q(k) == 0 {
                    break;
                }
            }
        }
    }
    v
}

/// The word at file offset `o` set to `v`.
fn set(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

/// lld's (and mold's) table with its IRELATIVE records moved before the
/// RELATIVEs, `DT_RELACOUNT` 0.
fn irelative_first(b: &mut [u8], irelative: u32) {
    let d = dynamic(b);
    let get = |t: u64| d.iter().find(|e| e.1 == t).map(|e| (e.0, e.2));
    let (_, rela) = get(7).unwrap();
    let (_, size) = get(8).unwrap();
    let (o, n) = (rela as usize, size as usize / 24);
    let recs: Vec<Vec<u8>> = (0..n).map(|i| b[o + 24 * i..o + 24 * i + 24].to_vec()).collect();
    let ty = |r: &Vec<u8>| u32::from_le_bytes(r[8..12].try_into().unwrap());
    let mut sorted: Vec<&Vec<u8>> = recs.iter().filter(|r| ty(r) == irelative).collect();
    sorted.extend(recs.iter().filter(|r| ty(r) != irelative));
    let flat: Vec<u8> = sorted.into_iter().flatten().copied().collect();
    b[o..o + 24 * n].copy_from_slice(&flat);
    if let Some((e, _)) = get(0x6fff_fff9) {
        set(b, e + 8, 0);
    }
}

/// GNU ld's `DT_RELA` and `DT_JMPREL` ranges swapped: the IRELATIVE table is
/// glibc's range 0, applied before the RELATIVE one.
fn ranges_swapped(b: &mut [u8]) {
    let d = dynamic(b);
    let get = |t: u64| d.iter().find(|e| e.1 == t).map(|e| (e.0, e.2)).unwrap();
    let ((ra, rv), (sa, sv), (ja, jv), (pa, pv)) = (get(7), get(8), get(23), get(2));
    set(b, ra + 8, jv);
    set(b, sa + 8, pv);
    set(b, ja + 8, rv);
    set(b, pa + 8, sv);
    if let Some(&(e, ..)) = d.iter().find(|e| e.1 == 0x6fff_fff9) {
        set(b, e + 8, 0);
    }
}

// spec/plumb.md §12.4, measured: order.c's resolver picks impl2 only if
// its pointer's RELATIVE record was applied first. glibc defers a range's
// IRELATIVE records to its end on x86-64 (ELF_MACHINE_IRELATIVE) and not
// on AArch64; a second range (DT_JMPREL apart from DT_RELA) is applied
// after the first. The verifier's P-D15 says so exactly where the
// program goes wrong.
#[test]
fn s12_4_irelative_order_is_measured_then_judged() {
    let Some(c) = corpus() else { return };
    for o in c.outs.iter().filter(|o| o.case == "order") {
        let a64 = o.arch == Arch::A64;
        let (patch, wrong): (Box<dyn Fn(&mut [u8])>, bool) = match o.link.linker {
            Linker::Bfd => (Box::new(ranges_swapped), true),
            _ => {
                (Box::new(move |b: &mut [u8]| irelative_first(b, if a64 { 1032 } else { 37 })), a64)
            }
        };
        let mut b = o.link.output_bytes();
        patch(&mut b);
        let r = o.verify_bytes(&b);
        let pd15 = r.findings.iter().any(|f| {
            f.rule.id() == "P-D15" && f.message.starts_with("a RELATIVE record is applied after")
        });
        assert_eq!(pd15, wrong, "{}: P-D15\n{}", o.label(), listing(o, &r));
        if o.arch.host() {
            let p = o.link.dir.join(format!("{}.order", o.link.output.display()));
            std::fs::write(&p, &b).unwrap();
            let mut perm = std::fs::metadata(&p).unwrap().permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
            std::fs::set_permissions(&p, perm).unwrap();
            let s = status(&o.link.dir, &p);
            assert_eq!(s != Some(0), wrong, "{}: the rewritten program's status {s:?}", o.label());
        }
    }
}

// spec/plumb.md §12.4: the static-pie mutation classes are caught on the
// references' C outputs, both architectures (the kill table is WP-089's
// record; `spie-relacount` minus one and a dropped record of a GOT word
// no reference uses are equivalent and may survive).
#[test]
fn s12_4_static_pie_mutants_are_caught() {
    use mortar_plumb::mutate::{Config, Outcome, run_with};
    let Some(c) = corpus() else { return };
    let cfg = Config { per_class: 8, threads: 4, ..Config::default() };
    let mut total = Outcome::default();
    for o in
        c.outs.iter().filter(|o| matches!(o.case, "hello" | "ifunc") && !o.label().contains(".now"))
    {
        let map = o.link.parse_map();
        let files = o.link.inputs(&map);
        let inputs: Vec<Input<'_>> =
            files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        let line: Vec<String> =
            std::fs::read_to_string(&o.line).unwrap().lines().map(str::to_string).collect();
        let ctx = Ctx { line: Some(line), files: &Dir(&o.link.dir) };
        let r = run_with(&o.link.output_bytes(), &inputs, &map, &cfg, Some(&ctx)).unwrap();
        total.add(&r, &o.label());
    }
    let table = total.render();
    println!("{table}");
    for class in
        ["spie-irel-twice", "spie-interp", "spie-needed", "spie-dynamic-word", "spie-rel-extra"]
    {
        let r = total.classes.iter().find(|r| r.class == class).unwrap();
        assert!(r.applied > 0 && r.missed.is_empty(), "{class}\n{table}");
    }
    for class in ["spie-relacount", "spie-rel-drop"] {
        let r = total.classes.iter().find(|r| r.class == class).unwrap();
        assert!(r.caught > 0, "{class}\n{table}");
    }
}
