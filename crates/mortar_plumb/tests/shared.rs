//! Stage 3 for shared objects (`spec/plumb.md` §12.5, P-D16-P-D26;
//! WP-121): libraries of GNU ld, lld 22 and mold on AArch64 and x86-64,
//! each linked through a driver whose `ld` records its words, under each
//! form of the line that decides the export and preemption sets (plain,
//! `-Bsymbolic`, `-Bsymbolic-functions`, `--dynamic-list`, a version
//! script), verified with the map and the line. The clean links must be
//! clean but for the recorded anomalies and the reference forms §12.5
//! accepts; their mutations must be reported by the rule P-D23's table
//! names. Every source is in `tests/shared/`; every binary is built at
//! test time.
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
    ($($n:literal),* $(,)?) => { &[$(($n, include_str!(concat!("shared/", $n)))),*] };
}

const SOURCES: &[(&str, &str)] = src![
    "lib.c",
    "dep.c",
    "dl.list",
    "v.map",
    "more.c",
    "ar.c",
    "before.c",
    "format-pic.s",
    "flags.c",
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
}

struct Tools {
    cc: [PathBuf; 2],
    bfd: [PathBuf; 2],
    lld: PathBuf,
    mold: PathBuf,
    ar: PathBuf,
}

fn tools() -> Option<Tools> {
    Some(Tools {
        cc: [tool("aarch64-linux-gnu-gcc").or_else(|| tool("gcc"))?, tool("x86_64-linux-gnu-gcc")?],
        bfd: [
            tool("aarch64-linux-gnu-ld.bfd").or_else(|| tool("ld.bfd"))?,
            tool("x86_64-linux-gnu-ld.bfd")?,
        ],
        lld: tool("ld.lld-22")?,
        mold: tool("mold")?,
        ar: tool("ar")?,
    })
}

/// One library: a link of a case, the words the linker saw, and the rules
/// the verifier must find beyond the recorded anomalies.
struct Out {
    case: &'static str,
    arch: Arch,
    link: Link,
    line: PathBuf,
    expect: &'static [&'static str],
}

/// The files of a link, read relative to the directory it ran in.
struct Dir<'a>(&'a Path);

impl Files for Dir<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

impl Out {
    fn label(&self) -> String {
        let file = self.link.output.file_name().map(|f| f.to_string_lossy().into_owned());
        format!("{} [{}] {}", self.link.label(), self.arch.name(), file.unwrap_or_default())
    }

    fn verify(&self) -> Report {
        self.verify_bytes(&self.link.output_bytes())
    }

    fn verify_bytes(&self, out: &[u8]) -> Report {
        let map = self.link.parse_map();
        let files = self.link.inputs(&map);
        let inputs: Vec<Input<'_>> =
            files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        let line: Vec<String> =
            std::fs::read_to_string(&self.line).unwrap().lines().map(str::to_string).collect();
        let ctx = Ctx { line: Some(line), files: &Dir(&self.link.dir) };
        mortar_plumb::verify_with(out, &inputs, &map, &ctx)
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

fn must(dir: &Path, program: &Path, args: &[&str]) {
    assert!(
        run_in(dir, program, args, None),
        "{} {} failed in {}",
        program.display(),
        args.join(" "),
        dir.display()
    );
}

/// mold's `_TLS_MODULE_BASE_` (A3): every mold library with TLSDESC.
const MOLD: &[&str] = &["A3"];
/// W43: lld on x86-64 binds `tv` (unlisted) half: the `DTPMOD64` names
/// it (P-D25), and view B sees the interposer's module (P-D17).
const W43: &[&str] = &["P-D25@names 'tv'", "P-D17@'tv'"];

/// W20: mold binds the preemptible IFUNC `ifn` (an IRELATIVE slot and a
/// direct call: F2), which view A (P-D18) and view B (P-D17) see.
const W20: &[&str] = &["A3", "F2", "P-D18@'ifn'", "P-D17@'ifn'"];

fn more_expect(linker: Linker) -> &'static [&'static str] {
    if linker == Linker::Mold { W20 } else { &[] }
}

/// What a clean library of `form` by `linker` must report.
fn expect(arch: Arch, form: &str, linker: Linker) -> &'static [&'static str] {
    match (arch, form, linker) {
        (_, _, Linker::Mold) => MOLD,
        (Arch::X64, "dynlist", Linker::Lld) => W43,
        // W31: lld binds every key and writes DF_SYMBOLIC; with it the
        // loader binds the listed keys too (view B).
        (Arch::X64, "dlbsym", Linker::Lld) => &["P-D22@DF_SYMBOLIC true", "P-D17", "P-D25"],
        (Arch::A64, "dlbsym", Linker::Lld) => &["P-D22@DF_SYMBOLIC true", "P-D17"],
        _ => &[],
    }
}

struct Builder<'t> {
    t: &'t Tools,
    root: PathBuf,
    outs: Vec<Out>,
}

impl Builder<'_> {
    fn dir(&self, arch: Arch, case: &str) -> PathBuf {
        let d = self.root.join(arch.name()).join(case);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn cc(&self, arch: Arch) -> &Path {
        &self.t.cc[arch as usize]
    }

    /// Compiles `src` (in the root) to `out` in `dir`.
    fn obj(&self, arch: Arch, dir: &Path, src: &str, flags: &[&str], out: &str) {
        let src = self.root.join(src).display().to_string();
        let mut a: Vec<&str> = flags.to_vec();
        a.extend(["-c", &src, "-o", out]);
        must(dir, self.cc(arch), &a);
    }

    /// A shared input, linked by the driver's own GNU ld.
    fn so(&self, arch: Arch, dir: &Path, src: &str, soname: &str, out: &str) {
        let src = self.root.join(src).display().to_string();
        let soname = format!("-Wl,-soname,{soname}");
        must(dir, self.cc(arch), &["-fPIC", "-O1", "-shared", &soname, &src, "-o", out]);
    }

    /// Links `out` in `dir` with linker `k` of `Linker::ALL` through the
    /// recording `ld`; a refused link is a test failure.
    #[allow(clippy::too_many_arguments)] // the link's facts, as `tests/dyn.rs`
    fn link(
        &mut self,
        case: &'static str,
        arch: Arch,
        dir: &Path,
        out: &str,
        args: &[&str],
        linker: Linker,
        expect: &'static [&'static str],
    ) {
        let name = format!("{out}.{}", linker.name());
        let map = format!("{name}.map");
        let line = dir.join(format!("{name}.line"));
        let bin = self.root.join(format!("ld-{}-{}", arch.name(), linker.name()));
        let b = format!("-B{}/", bin.display());
        let m = format!("-Wl,-Map={map}");
        let mut a: Vec<&str> = vec![&b];
        a.extend(args);
        a.extend(["-o", &name, &m]);
        assert!(run_in(dir, self.cc(arch), &a, Some(&line)), "{case}/{name}: the link failed");
        self.outs.push(Out {
            case,
            arch,
            link: Link {
                case,
                linker,
                dir: dir.to_path_buf(),
                output: dir.join(&name),
                map: dir.join(map),
            },
            line,
            expect,
        });
    }
}

/// The line forms of D-112 each library is linked under.
const FORMS: &[(&str, &[&str])] = &[
    ("plain", &[]),
    ("bsymbolic", &["-Wl,-Bsymbolic"]),
    ("bsymfunc", &["-Wl,-Bsymbolic-functions"]),
    ("dynlist", &["-Wl,--dynamic-list=dl.list"]),
    ("vscript", &["-Wl,--version-script=v.map"]),
    ("dlbsym", &["-Wl,-Bsymbolic", "-Wl,--dynamic-list=dl.list"]),
];

fn build() -> Option<Corpus> {
    let t = tools()?;
    let scratch = Scratch::new("plumb-shared");
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
    let mut b = Builder { t: &t, root: root.clone(), outs: Vec::new() };
    for arch in [Arch::A64, Arch::X64] {
        let d = b.dir(arch, "forms");
        for f in ["dl.list", "v.map"] {
            std::fs::copy(root.join(f), d.join(f)).unwrap();
        }
        b.so(arch, &d, "dep.c", "libdep.so", "libdep.so");
        b.obj(arch, &d, "lib.c", &["-O1", "-g", "-fPIC"], "lib.o");
        for (form, extra) in FORMS {
            for linker in Linker::ALL {
                let mut args = vec!["-shared", "-Wl,-soname,libx.so", "lib.o", "libdep.so"];
                args.extend(*extra);
                let out = format!("libx-{form}.so");
                b.link("forms", arch, &d, &out, &args, linker, expect(arch, form, linker));
            }
        }
    }
    for arch in [Arch::A64, Arch::X64] {
        let d = b.dir(arch, "more");
        b.obj(arch, &d, "more.c", &["-O1", "-g", "-fPIC"], "more.o");
        b.obj(arch, &d, "ar.c", &["-O1", "-fPIC"], "ar.o");
        let ar = b.t.ar.clone();
        must(&d, &ar, &["rc", "libar.a", "ar.o"]);
        for linker in Linker::ALL {
            let args = [
                "-shared",
                "-Wl,-soname,libmore.so",
                "more.o",
                "libar.a",
                "-Wl,--exclude-libs,ALL",
                "-Wl,-init,myinit",
                "-Wl,-fini,myfini",
            ];
            b.link("more", arch, &d, "libmore.so", &args, linker, more_expect(linker));
        }
    }
    // P-D4's RELRO end at the line's maxpage (WP-121 part 2): the plain
    // library on AArch64 under `-z max-page-size=0x1000`, by each reference.
    let d = b.dir(Arch::A64, "relro");
    b.obj(Arch::A64, &d, "lib.c", &["-O1", "-fPIC"], "lib.o");
    for linker in Linker::ALL {
        let args = ["-shared", "-Wl,-soname,libr.so", "lib.o", "-Wl,-z,max-page-size=0x1000"];
        b.link("relro", Arch::A64, &d, "libr.so", &args, linker, &[]);
    }
    refs(&mut b);
    Some(Corpus { _scratch: scratch, outs: b.outs })
}

fn judge(case: &str) {
    let Some(c) = corpus() else { return };
    let mut failures = Vec::new();
    let mut n = 0;
    for o in c.outs.iter().filter(|o| o.case == case) {
        n += 1;
        let r = o.verify();
        if std::env::var("SHARED_DEBUG").is_ok_and(|v| o.label().contains(&v)) {
            eprintln!("== {} ({})", o.label(), o.link.output.display());
            for f in &r.findings {
                eprintln!("  {f:?}");
            }
        }
        let mut found = BTreeSet::new();
        for f in &r.findings {
            match known_anomaly(&o.link, f) {
                Some(a) => found.insert(mortar_plumb::anomaly::id(a).to_string()),
                None => found.insert(f.rule.id()),
            };
        }
        let rule = |e: &str| e.split_once('@').map_or(e, |(r, _)| r).to_string();
        let allowed: BTreeSet<String> = o.expect.iter().map(|e| rule(e)).collect();
        let missing: Vec<&&str> = o
            .expect
            .iter()
            .filter(|e| match e.split_once('@') {
                None => !found.contains(**e),
                Some((id, text)) => {
                    !r.findings.iter().any(|f| f.rule.id() == id && f.message.contains(text))
                }
            })
            .collect();
        let extra: Vec<&String> = found.iter().filter(|f| !allowed.contains(*f)).collect();
        if !missing.is_empty() || !extra.is_empty() {
            failures.push(format!(
                "{}: missing {missing:?}, unexpected {extra:?}\n{}",
                o.label(),
                r.findings
                    .iter()
                    .filter(|f| known_anomaly(&o.link, f).is_none())
                    .take(12)
                    .map(|f| format!("  {} {}", f.rule.id(), f.message))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
    }
    assert!(n > 0 || c.outs.is_empty(), "no outputs of case {case}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The reference `-shared` links of the link tests that stage 3 judged
/// falsely when WP-121 part 1 landed (`tests/link/dyn/shared/`): a library
/// whose imports no shared input defines, without `-z defs` (D-118: the
/// host's; `nonpic-analysis-format`); Clang's `.comment` mapping symbol in
/// a GNU ld library (`nonpic-analysis-before-start`); D-103's `-z` words
/// (`refused-shared-later`). Every link is the case `refs`.
fn refs(b: &mut Builder<'_>) {
    use Linker::Bfd;
    let a = Arch::A64;
    let d = b.dir(a, "refs");
    b.obj(a, &d, "format-pic.s", &[], "format-pic.o");
    for l in Linker::ALL {
        let args = ["-shared", "format-pic.o", "-Wl,-soname,libpic.so"];
        b.link("refs", a, &d, "libpic.so", &args, l, row(l, &[]));
    }
    if let Some(clang) = tool("clang") {
        let src = b.root.join("before.c").display().to_string();
        must(&d, &clang, &["-O2", "-fPIC", "-c", &src, "-o", "c2.o"]);
        b.link("refs", a, &d, "libc2.so", &["-shared", "c2.o", "-Wl,-soname,libc2.so"], Bfd, &[]);
    }
    for arch in [Arch::A64, Arch::X64] {
        let d = b.dir(arch, "refs");
        b.obj(arch, &d, "flags.c", &["-O1", "-fPIC"], "flags.o");
        let args = [
            "-shared",
            "flags.o",
            "-Wl,-z,nodelete",
            "-Wl,-z,nodlopen",
            "-Wl,-z,initfirst",
            "-Wl,-z,interpose",
            "-Wl,-z,origin",
        ];
        // A10: GNU ld's label of the empty section it removes.
        for l in Linker::ALL {
            let want: &'static [&'static str] = if l == Bfd { &["A10"] } else { row(l, &[]) };
            b.link("refs", arch, &d, "libflags.so", &args, l, want);
        }
    }
}

/// A row's findings, with mold's anomaly A3 (every mold library).
fn row(l: Linker, want: &'static [&'static str]) -> &'static [&'static str] {
    if l != Linker::Mold {
        return want;
    }
    let v: Vec<&'static str> = std::iter::once("A3").chain(want.iter().copied()).collect();
    Vec::leak(v)
}

// The link tests' reference libraries (WP-121, after part 1): clean.
#[test]
fn s12_5_reference_link_tests_verify_clean() {
    judge("refs");
}

// P-D22 (D-103): a `DT_FLAGS_1` bit no `-z` word asks for, and one the
// line asks for that is absent, are findings.
#[test]
fn p_d22_flags_1_follows_the_z_words() {
    let Some(c) = corpus() else { return };
    let mut n = 0;
    for o in c
        .outs
        .iter()
        .filter(|o| o.case == "refs" && o.link.output.to_string_lossy().contains("libflags.so"))
    {
        n += 1;
        for (flip, want) in [(0x4u64, "bits 0x4 no -z word"), (0x8, "bits 0x8 the line asks for")] {
            let mut b = o.link.output_bytes();
            let h = pt_dynamic(&b);
            let (off, size) = (u64at(&b, h + 8) as usize, u64at(&b, h + 32) as usize);
            let e = (off..off + size).step_by(16).find(|&e| u64at(&b, e) == 0x6fff_fffb).unwrap();
            let v = u64at(&b, e + 8) ^ flip;
            b[e + 8..e + 16].copy_from_slice(&v.to_le_bytes());
            let r = o.verify_bytes(&b);
            assert!(
                r.findings.iter().any(|f| f.rule.id() == "P-D22" && f.message.contains(want)),
                "{}: no P-D22 '{want}' in {:#?}",
                o.label(),
                r.findings
            );
        }
    }
    assert!(n == 0 || n == 6, "{n} libflags outputs");
}

// spec/plumb.md §12.5: a library under each line form of D-112, by each
// reference, is clean.
#[test]
fn s12_5_clean_libraries_verify_clean() {
    judge("forms");
}

// spec/plumb.md §12.5: IE TLS, an IFUNC, `__start_`/`__stop_`, `-init`/
// `-fini` and `--exclude-libs`, by each reference.
#[test]
fn s12_5_more_forms() {
    judge("more");
}

fn u16at(b: &[u8], o: usize) -> usize {
    usize::from(u16::from_le_bytes([b[o], b[o + 1]]))
}

fn u64at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// The file offset of `PT_DYNAMIC`'s program header (ELF64).
fn pt_dynamic(b: &[u8]) -> usize {
    let (off, ent, n) = (u64at(b, 0x20) as usize, u16at(b, 0x36), u16at(b, 0x38));
    (0..n).map(|i| off + i * ent).find(|&h| b[h..h + 4] == 2u32.to_le_bytes()).unwrap()
}

/// The file range of the section named `name` (ELF64).
fn section(b: &[u8], name: &str) -> (usize, usize) {
    let (off, ent, n, str_i) =
        (u64at(b, 0x28) as usize, u16at(b, 0x3a), u16at(b, 0x3c), u16at(b, 0x3e));
    let strs = u64at(b, off + str_i * ent + 24) as usize;
    (0..n)
        .map(|i| off + i * ent)
        .find(|&h| {
            let at = strs + u32::from_le_bytes(b[h..h + 4].try_into().unwrap()) as usize;
            b[at..].split(|&c| c == 0).next() == Some(name.as_bytes())
        })
        .map(|h| (u64at(b, h + 24) as usize, u64at(b, h + 32) as usize))
        .unwrap()
}

/// The verifier never panics: a damaged `.dynamic` or record is a finding
/// by name (WP-121's brief).
fn hostile(edit: impl Fn(&mut Vec<u8>), want: &str) {
    let Some(c) = corpus() else { return };
    for o in
        c.outs.iter().filter(|o| o.case == "forms" && o.link.output.ends_with("libx-plain.so.bfd"))
    {
        let mut b = o.link.output_bytes();
        edit(&mut b);
        let r = o.verify_bytes(&b);
        assert!(
            r.findings.iter().any(|f| format!("{} {}", f.rule.id(), f.message).contains(want)),
            "{}: no finding '{want}' in {:#?}",
            o.label(),
            r.findings
        );
    }
}

// The brief: a truncated `.dynamic` (one entry, no DT_NULL).
#[test]
fn s12_5_truncated_dynamic_is_a_finding() {
    hostile(
        |b| {
            let h = pt_dynamic(b);
            b[h + 32..h + 48].copy_from_slice(&[16, 0, 0, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0]);
        },
        "P-D6 PT_DYNAMIC has no DT_NULL entry",
    );
}

// The brief: a `PT_DYNAMIC` outside the file.
#[test]
fn s12_5_dynamic_outside_the_file_is_a_finding() {
    hostile(
        |b| {
            let h = pt_dynamic(b);
            b[h + 8..h + 16].copy_from_slice(&0x7000_0000u64.to_le_bytes());
            b[h + 16..h + 24].copy_from_slice(&0x7000_0000u64.to_le_bytes());
        },
        "is not in the output's memory",
    );
}

// The brief: a record whose `.dynsym` index is out of range.
#[test]
fn s12_5_dynsym_index_out_of_range_is_a_finding() {
    hostile(
        |b| {
            let (off, size) = section(b, ".rela.dyn");
            let r = (off..off + size)
                .step_by(24)
                .find(|&r| u64at(b, r + 8) >> 32 != 0)
                .expect("a record naming a symbol");
            let info = (0xf_ffffu64 << 32) | (u64at(b, r + 8) & 0xffff_ffff);
            b[r + 8..r + 16].copy_from_slice(&info.to_le_bytes());
        },
        "P-D25 a dynamic record names .dynsym entry 1048575, past the table's",
    );
}

/// The file offset of the first program header of type `ty` (ELF64).
fn phdr(b: &[u8], ty: u32) -> Option<usize> {
    let (off, ent, n) = (u64at(b, 0x20) as usize, u16at(b, 0x36), u16at(b, 0x38));
    (0..n).map(|i| off + i * ent).find(|&h| b[h..h + 4] == ty.to_le_bytes())
}

const PT_GNU_RELRO: u32 = 0x6474_e552;

// spec/plumb.md P-D4 (WP-121 part 2): under mortar's map, PT_GNU_RELRO
// ends on the line's maxpage, S2's (`-z max-page-size`; L-74), not on the
// machine's default. Each reference's AArch64 library linked under
// `-z max-page-size=0x1000` whose RELRO ends on 4 KiB, read as mortar's
// map: no RELRO-end finding; the same output under a line without the
// option (`max-page-size=0`, 64 KiB): the finding when the end is not on 64 KiB; the end
// moved by 8: the finding. `-z common-page-size` changes nothing (L-75).
#[test]
fn p_d4_relro_ends_on_the_lines_maxpage() {
    let Some(c) = corpus() else { return };
    let mut n = 0;
    for o in c.outs.iter().filter(|o| o.case == "relro") {
        let out = o.link.output_bytes();
        let Some(h) = phdr(&out, PT_GNU_RELRO) else { continue };
        let end = u64at(&out, h + 16) + u64at(&out, h + 40);
        if !end.is_multiple_of(0x1000) {
            continue; // lld's AArch64 RELRO ends mid-page (D-61)
        }
        n += 1;
        let line: Vec<String> =
            std::fs::read_to_string(&o.line).unwrap().lines().map(str::to_string).collect();
        let relro = |bytes: &[u8], words: &[String]| -> Vec<String> {
            let mut map = o.link.parse_map();
            let files = o.link.inputs(&map);
            map.format = mortar_plumb::Format::Mortar;
            map.args = words.iter().map(|w| w.as_bytes().to_vec()).collect();
            let inputs: Vec<Input<'_>> =
                files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
            let ctx = Ctx { line: Some(words.to_vec()), files: &Dir(&o.link.dir) };
            mortar_plumb::verify_with(bytes, &inputs, &map, &ctx)
                .findings
                .iter()
                .filter(|f| f.rule.id() == "P-D4" && f.message.starts_with("PT_GNU_RELRO ends"))
                .map(|f| f.message.clone())
                .collect()
        };
        let got = relro(&out, &line);
        assert!(got.is_empty(), "{}: {got:#?}", o.label());
        let mut common = line.clone();
        common.extend(["-z".to_string(), "common-page-size=0x10000".to_string()]);
        let got = relro(&out, &common);
        assert!(got.is_empty(), "{} common-page-size: {got:#?}", o.label());
        // `max-page-size=0`: the default (L-74).
        let default: Vec<String> =
            line.iter()
                .map(|w| {
                    if w.starts_with("max-page-size") {
                        "max-page-size=0".into()
                    } else {
                        w.clone()
                    }
                })
                .collect();
        assert_ne!(default, line, "{}: the line has no -z max-page-size", o.label());
        let got = relro(&out, &default);
        assert_eq!(got.is_empty(), end.is_multiple_of(0x1_0000), "{} default: {got:#?}", o.label());
        let mut moved = out.clone();
        let memsz = u64at(&moved, h + 40) - 8;
        moved[h + 40..h + 48].copy_from_slice(&memsz.to_le_bytes());
        let got = relro(&moved, &line);
        assert!(got.iter().any(|g| g.contains("maxpage 0x1000")), "{} moved: {got:#?}", o.label());
    }
    assert!(n > 0 || c.outs.is_empty(), "no reference ends RELRO on 4 KiB");
}
