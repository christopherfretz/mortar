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
    "tls.c",
    "tlso.c",
    "dep.c",
    "dl.list",
    "v.map",
    "more.c",
    "ar.c",
    "w19.c",
    "w21a.c",
    "w21b.c",
    "w22.c",
    "w23.c",
    "w23.map",
    "w24.c",
    "w32.s",
    "w33.s",
    "w34.s",
    "w38.c",
    "w40.cc",
    "w40.map",
    "w41.c",
    "vdep.c",
    "vdep.map",
    "w42.c",
    "gap.c",
    "before.c",
    "hole.c",
    "format-pic.s",
    "flags.c",
    "sv.c",
    "sv.map",
    "weak.cc",
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
        self.verify_line(out, self.words())
    }

    /// The words the linker saw.
    fn words(&self) -> Vec<String> {
        std::fs::read_to_string(&self.line).unwrap().lines().map(str::to_string).collect()
    }

    /// `out` judged as the output of the line `words`.
    fn verify_line(&self, out: &[u8], words: Vec<String>) -> Report {
        let mut map = self.link.parse_map();
        let files = self.link.inputs(&map);
        let inputs: Vec<Input<'_>> =
            files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        // A map that records its line gets the words too.
        if !map.args.is_empty() {
            map.args = words.iter().map(|w| w.as_bytes().to_vec()).collect();
        }
        let ctx = Ctx { line: Some(words), files: &Dir(&self.link.dir) };
        mortar_plumb::verify_with(out, &inputs, &map, &ctx)
    }

    /// `out` judged as mortar's output of the same line (mortar's rows).
    fn verify_as_mortar(&self, out: &[u8]) -> Report {
        let words = self.words();
        let mut map = self.link.parse_map();
        let files = self.link.inputs(&map);
        map.format = mortar_plumb::Format::Mortar;
        map.args = words.iter().map(|w| w.as_bytes().to_vec()).collect();
        let inputs: Vec<Input<'_>> =
            files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        let ctx = Ctx { line: Some(words), files: &Dir(&self.link.dir) };
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
    /// The driver of the next [`Builder::link`] (`g++`), else `cc`.
    driver: Option<PathBuf>,
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
        let driver = self.driver.take().unwrap_or_else(|| self.cc(arch).to_path_buf());
        assert!(run_in(dir, &driver, &a, Some(&line)), "{case}/{name}: the link failed");
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
    let mut b = Builder { t: &t, root: root.clone(), outs: Vec::new(), driver: None };
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
    rows(&mut b);
    // P-D4's RELRO end at the line's maxpage (WP-121 part 2): the plain
    // library on AArch64 under `-z max-page-size=0x1000`, by each reference.
    let d = b.dir(Arch::A64, "relro");
    b.obj(Arch::A64, &d, "lib.c", &["-O1", "-fPIC"], "lib.o");
    for linker in Linker::ALL {
        let args = ["-shared", "-Wl,-soname,libr.so", "lib.o", "-Wl,-z,max-page-size=0x1000"];
        b.link("relro", Arch::A64, &d, "libr.so", &args, linker, &[]);
    }
    refs(&mut b);
    tls(&mut b);
    tlsgd(&mut b);
    Some(Corpus { _scratch: scratch, outs: b.outs })
}

/// P-D23's rows (spec/plumb.md §12.5; dyn.md §17.7): each wrong output
/// the AArch64 and x86-64 references write, linked by the reference that
/// writes it from the row's input, and D-117's address clause on GCC's
/// `-fPIE` code (W44), its known refusal of correct code ("before the
/// start") and its residual hole (NOT caught). Every link is the case
/// `rows`; `ROWS_DEBUG`-free: `SHARED_DEBUG` prints a link's findings.
fn rows(b: &mut Builder<'_>) {
    use Linker::{Bfd, Lld, Mold};
    let a = Arch::A64;
    let d = b.dir(a, "rows");
    for f in ["w23.map", "w40.map", "vdep.map"] {
        std::fs::copy(b.root.join(f), d.join(f)).unwrap();
    }
    let pic = ["-O1", "-fPIC"];
    b.so(a, &d, "dep.c", "libdep.so", "libdep.so");
    b.obj(a, &d, "w19.c", &["-O1", "-fPIC", "-ftls-model=local-exec"], "w19.o");
    b.link("rows", a, &d, "w19.so", &["-shared", "w19.o"], Bfd, row(Bfd, ROW_W19));
    b.obj(a, &d, "w21a.c", &pic, "w21a.o");
    b.obj(a, &d, "w21b.c", &pic, "w21b.o");
    for l in [Bfd, Mold] {
        // GNU ld's IRELATIVE records in `.rela.dyn` run before the
        // JUMP_SLOTs (P-D10, H1: an executable rule kept for `Shared`).
        let want = if l == Bfd { ROW_W21_BFD } else { ROW_W21 };
        b.link("rows", a, &d, "w21.so", &["-shared", "w21a.o", "w21b.o"], l, row(l, want));
    }
    b.obj(a, &d, "w22.c", &["-O1", "-fPIC", "-ffunction-sections"], "w22.o");
    let w22 = ["-shared", "w22.o", "-Wl,--gc-sections", "-Wl,-init,w22_init"];
    b.link("rows", a, &d, "w22.so", &w22, Mold, row(Mold, ROW_W22));
    b.obj(a, &d, "w23.c", &pic, "w23.o");
    let w23 = ["-shared", "w23.o", "-Wl,--gc-sections", "-Wl,--version-script=w23.map"];
    b.link("rows", a, &d, "w23.so", &w23, Lld, row(Lld, ROW_W23));
    b.obj(a, &d, "w24.c", &pic, "w24.o");
    b.link("rows", a, &d, "w24.so", &["-shared", "w24.o"], Lld, row(Lld, ROW_W24));
    b.obj(a, &d, "w32.s", &[], "w32.o");
    b.link("rows", a, &d, "w32.so", &["-shared", "w32.o"], Mold, row(Mold, ROW_W32));
    b.obj(a, &d, "w33.s", &[], "w33.o");
    b.link("rows", a, &d, "w33.so", &["-shared", "w33.o"], Bfd, row(Bfd, ROW_W33));
    b.obj(a, &d, "w38.c", &["-O1", "-fno-pic"], "w38.o");
    b.link("rows", a, &d, "w38.so", &["-shared", "w38.o", "libdep.so"], Mold, row(Mold, ROW_W38));
    b.obj(a, &d, "w40.cc", &pic, "w40.o");
    let w40 = ["-shared", "w40.o", "-Wl,--version-script=w40.map"];
    b.link("rows", a, &d, "w40.so", &w40, Mold, row(Mold, ROW_W40));
    let vdep = b.root.join("vdep.c").display().to_string();
    let cc = b.cc(a).to_path_buf();
    let vs = ["-fPIC", "-shared", "-Wl,-soname,libvdep.so", "-Wl,--version-script=vdep.map"];
    must(&d, &cc, &[&vs[..], &[vdep.as_str(), "-o", "libvdep.so"]].concat());
    b.obj(a, &d, "w42.c", &pic, "w42.o");
    b.link("rows", a, &d, "w42.so", &["-shared", "w42.o", "libvdep.so"], Mold, row(Mold, ROW_W42));
    // W44 and the address clause: bound uses by the analysis (gap.c
    // -fPIE) and by the rule (before.c -O0, a known refusal of correct
    // code); their -fPIC, -O2 and -Bsymbolic forms clean; the residual
    // hole not caught.
    b.obj(a, &d, "gap.c", &["-O2", "-fPIE"], "gap-pie.o");
    b.obj(a, &d, "gap.c", &["-O2", "-fPIC"], "gap-pic.o");
    b.obj(a, &d, "before.c", &["-O0", "-fPIC"], "before-O0.o");
    b.obj(a, &d, "before.c", &["-O2", "-fPIC"], "before-O2.o");
    b.obj(a, &d, "hole.c", &["-O2", "-fPIE"], "hole.o");
    for l in Linker::ALL {
        b.link("rows", a, &d, "gap-pie.so", &["-shared", "gap-pie.o"], l, row(l, ROW_ANALYSIS));
        b.link("rows", a, &d, "gap-pic.so", &["-shared", "gap-pic.o"], l, row(l, &[]));
        let bsym = ["-shared", "gap-pie.o", "-Wl,-Bsymbolic"];
        b.link("rows", a, &d, "gap-bsym.so", &bsym, l, row(l, &[]));
        b.link("rows", a, &d, "before-O0.so", &["-shared", "before-O0.o"], l, row(l, ROW_RULE));
        b.link("rows", a, &d, "before-O2.so", &["-shared", "before-O2.o"], l, row(l, &[]));
        b.link("rows", a, &d, "hole.so", &["-shared", "hole.o"], l, row(l, &[]));
    }
    let x = Arch::X64;
    let d = b.dir(x, "rows");
    b.so(x, &d, "dep.c", "libdep.so", "libdep.so");
    b.obj(x, &d, "w34.s", &[], "w34.o");
    b.link("rows", x, &d, "w34.so", &["-shared", "w34.o"], Bfd, row(Bfd, ROW_W34));
    b.obj(x, &d, "w41.c", &["-O1", "-fPIC", "-mno-direct-extern-access"], "w41.o");
    for l in [Lld, Mold] {
        b.link("rows", x, &d, "w41.so", &["-shared", "w41.o", "libdep.so"], l, row(l, ROW_W41));
    }
}

/// A row's findings, with mold's anomaly A3 (every mold library) and GNU
/// ld's A10 (its label of an empty section it removes: some libraries,
/// so tolerated, not required: `?`).
fn row(l: Linker, want: &'static [&'static str]) -> &'static [&'static str] {
    let extra = match l {
        Linker::Mold => "A3",
        Linker::Bfd => "?A10",
        Linker::Lld => return want,
    };
    let v: Vec<&'static str> = std::iter::once(extra).chain(want.iter().copied()).collect();
    Vec::leak(v)
}

// Each row: the rules P-D23's table names, as the verifier reports them.
// View A's findings keep stage 2's ids (R1 a field's value, R2 its range);
// view B's are P-D17's (P-D17's sentence).
/// W19: the LE form itself (P-D26's LE row is P-D20's).
const ROW_W19: &[&str] = &["P-D20@a local-exec TLS form"];
/// W21: `&hif` is the IPLT entry in one unit, the resolved function in
/// the other (a GOT slot and data words with IRELATIVE records). P-D18's
/// bound-key clause is stage 2's F3, reported under its id.
const ROW_W21: &[&str] = &["F3@two addresses for one function"];
const ROW_W21_BFD: &[&str] =
    &["F3@two addresses for one function", "P-D10@an IRELATIVE record is applied before"];
/// W22: mold collects the hidden `-init` function.
const ROW_W22: &[&str] = &["P-D22@DT_INIT"];
/// W23: lld drops `cid_gc`; `__start_cid_gc` is the link's own (L-96),
/// bound by the script, and left undefined with a `GLOB_DAT` naming it.
const ROW_W23: &[&str] = &[
    "P-D17@'__start_cid_gc' computes the address of section 'cid_gc'",
    "P-D9@no definition of '__start_cid_gc'",
    "P-D25@'__start_cid_gc'",
    "P-D7@'__start_cid_gc'",
];
const ROW_W24: &[&str] = &["P-D20@an output section .ctors"];
/// W32: `ADR_PREL_LO21`, `LD_PREL_LO19` and `PREL32` against `w32_g`.
const ROW_W32: &[&str] = &[
    "P-D17@view B (an interposer defines 'w32_g'",
    "P-D17@R_AARCH64_ADR_PREL_LO21",
    "P-D17@R_AARCH64_LD_PREL_LO19",
    "P-D17@R_AARCH64_PREL32",
];
/// W33: the preemptible key and the weak undefined one, view B (P-D9's
/// view 3 is view B for `Shared`, P-D16).
const ROW_W33: &[&str] = &[
    "P-D17@view B (an interposer defines 'w33_g'",
    "P-D17@view B (an interposer defines 'w33_wu'",
];
/// W34: GNU ld's dynamic `PC32` records (type 2), which no loader applies
/// to 64 bits: P-D9 names the record, view A finds the place's link-time
/// value (R1), view B the interposer's keys.
const ROW_W34: &[&str] = &[
    "P-D9@a dynamic record of unknown type 2",
    "R1",
    "P-D17@view B (an interposer defines 'w34_g'",
    "P-D17@view B (an interposer defines 'w34_ext'",
];
/// W38: view A's value is `imp`'s definition, the place holds the stub.
const ROW_W38: &[&str] = &["R1", "R2", "P-D17@view B (an interposer defines 'imp'"];
/// W40: the verifier has no demangler: `extern "C++"` is outside stage 3
/// by name (D-114), so P-D19 never sees the empty export set.
const ROW_W40: &[&str] = &["P-D1@extern \"C++\" needs the demangler"];
const ROW_W41: &[&str] = &["P-D22@NEEDED_INDIRECT_EXTERN_ACCESS"];
/// W42: `vf@VA` imported unversioned, `libvdep.so` not needed.
const ROW_W42: &[&str] = &[
    "P-D6@DT_NEEDED lacks 'libvdep.so'",
    "P-D8@the input reference asks for 'VA'",
    "P-D9@no definition of 'vf'",
    "P-D7@'vf'",
];
const ROW_ANALYSIS: &[&str] = &["P-D17@the address clause (D-117 analysis)"];
const ROW_RULE: &[&str] = &["P-D17@the address clause (D-117):"];

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
        // `?id`: allowed, not required (an anomaly some links show).
        let rule = |e: &str| {
            let e = e.trim_start_matches('?');
            e.split_once('@').map_or(e, |(r, _)| r).to_string()
        };
        let allowed: BTreeSet<String> = o.expect.iter().map(|e| rule(e)).collect();
        let missing: Vec<&&str> = o
            .expect
            .iter()
            .filter(|e| !e.starts_with('?'))
            .filter(|e| match e.split_once('@') {
                None => !found.contains(**e),
                Some((id, text)) => !r.findings.iter().any(|f| {
                    f.rule.id() == id
                        && (f.message.contains(text)
                            || f.reloc.as_deref().is_some_and(|x| x == text))
                }),
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
        // The target is named: on an x86-64 host a bare `clang` writes an
        // x86-64 object, which the AArch64 link below refuses.
        let t = "--target=aarch64-linux-gnu";
        must(&d, &clang, &[t, "-O2", "-fPIC", "-c", &src, "-o", "c2.o"]);
        b.link("refs", a, &d, "libc2.so", &["-shared", "c2.o", "-Wl,-soname,libc2.so"], Bfd, &[]);
    }
    // WP-123's V3: two `.symver` definitions of `foo` (`foo@V1` hidden,
    // `foo@@V2`) under a version script (D-115), by each reference.
    b.obj(a, &d, "sv.c", &["-O1", "-fPIC"], "sv.o");
    std::fs::copy(b.root.join("sv.map"), d.join("sv.map")).unwrap();
    for l in Linker::ALL {
        let args = ["-shared", "sv.o", "-Wl,--version-script,sv.map"];
        b.link("refs", a, &d, "libsv.so", &args, l, row(l, &[]));
    }
    // WP-123's V5 (`cxx-weak-only-libc`): g++'s `--as-needed` drops the C
    // library, whose `__cxa_finalize` crtbeginS references weakly.
    for (arch, gxx) in [(Arch::A64, "aarch64-linux-gnu-g++"), (Arch::X64, "x86_64-linux-gnu-g++")] {
        let Some(gxx) = tool(gxx).or_else(|| (arch == Arch::A64).then(|| tool("g++")).flatten())
        else {
            continue;
        };
        let d = b.dir(arch, "refs");
        let src = b.root.join("weak.cc").display().to_string();
        must(&d, &gxx, &["-O1", "-fPIC", "-c", &src, "-o", "weak.o"]);
        for l in Linker::ALL {
            b.driver = Some(gxx.clone());
            b.link("refs", arch, &d, "libwo.so", &["-shared", "weak.o"], l, row(l, &[]));
        }
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

/// The x86-64 TLS dialects of WP-144: (name, the compiler's flags).
const TLS_DIALECTS: &[(&str, &[&str])] = &[
    ("desc", &["-O1", "-fPIC", "-mtls-dialect=gnu2"]),
    ("gd", &["-O1", "-fPIC"]),
    ("ie", &["-O1", "-fPIC", "-ftls-model=initial-exec"]),
];

/// WP-144 (P-D26, X-61): `tls.c` in each dialect, `-shared` by each
/// reference, the imported variable from `libtlso.so`. mold's library
/// defines `_TLS_MODULE_BASE_` at an address in every dialect (A3).
fn tls(b: &mut Builder<'_>) {
    let a = Arch::X64;
    let d = b.dir(a, "tls");
    b.so(a, &d, "tlso.c", "libtlso.so", "libtlso.so");
    for (name, flags) in TLS_DIALECTS {
        let obj = format!("tls-{name}.o");
        b.obj(a, &d, "tls.c", flags, &obj);
        for l in Linker::ALL {
            let out = format!("libt-{name}.so");
            let args = ["-shared", &obj, "libtlso.so", "-Wl,-soname,libt.so"];
            let want: &'static [&'static str] = if l == Linker::Mold { MOLD } else { &[] };
            b.link("tls", a, &d, &out, &args, l, want);
        }
    }
}

// WP-144: the x86-64 TLS libraries of every dialect and reference are
// clean (P-D26; R1's kept descriptor, X-61; Y5's `_TLS_MODULE_BASE_`).
#[test]
fn p_d26_x64_tls_libraries_verify_clean() {
    judge("tls");
}

/// WP-144 (A46): `tls.c` in AArch64's traditional dialect (general-
/// dynamic pairs; three local thread-locals), `-shared` by GNU ld and
/// mold (lld takes no traditional dialect). GNU ld writes a spare
/// `R_AARCH64_NONE` per local pair: A46; mold's `_TLS_MODULE_BASE_` is
/// A3 here too.
fn tlsgd(b: &mut Builder<'_>) {
    let a = Arch::A64;
    let d = b.dir(a, "tlsgd");
    b.so(a, &d, "tlso.c", "libtlso.so", "libtlso.so");
    let flags = ["-O1", "-fPIC", "-fno-section-anchors", "-mtls-dialect=trad"];
    b.obj(a, &d, "tls.c", &flags, "tls.o");
    for l in [Linker::Bfd, Linker::Mold] {
        let args = ["-shared", "tls.o", "libtlso.so", "-Wl,-soname,libtg.so"];
        let want: &'static [&'static str] = if l == Linker::Bfd { &["A46"] } else { MOLD };
        b.link("tlsgd", a, &d, "libtg.so", &args, l, want);
    }
}

// WP-144: GNU ld's spare records are A46 and nothing else; mold's
// library has only A3.
#[test]
fn a46_aarch64_gd_libraries_verify_clean() {
    judge("tlsgd");
}

// WP-144 (A46, P-D25): A46 accepts exactly its shape. Every mutation of
// a spare record (a non-zero `r_offset`, `r_addend` or `r_sym`; the
// records moved inside `DT_RELACOUNT`'s prefix; one more type-0 record
// than local pairs) gives a new finding no anomaly names; and the clean
// library judged as mortar's output keeps P-D25's plain finding (D-30).
#[test]
fn a46_accepts_only_its_shape() {
    let Some(c) = corpus() else { return };
    let o = c
        .outs
        .iter()
        .find(|o| o.case == "tlsgd" && o.link.linker == Linker::Bfd)
        .expect("GNU ld's libtg.so");
    let b = o.link.output_bytes();
    let a46 = |f: &mortar_plumb::Finding| {
        known_anomaly(&o.link, f).is_some_and(|a| mortar_plumb::anomaly::id(a) == "A46")
    };
    let clean = o.verify_bytes(&b);
    let key = |f: &mortar_plumb::Finding| format!("{} {}", f.rule.id(), f.message);
    let clean_keys: BTreeSet<String> = clean.findings.iter().map(key).collect();
    let spare = clean.findings.iter().filter(|f| a46(f)).count();
    assert!(spare > 0, "GNU ld's libtg.so has no A46 finding");
    let (off, size, _) = section_at(&b, ".rela.dyn").unwrap();
    let recs: Vec<usize> = (off..off + size).step_by(24).collect();
    let nones: Vec<usize> = recs.iter().copied().filter(|&e| u64at(&b, e + 8) == 0).collect();
    assert_eq!(nones.len(), 3, "three local pairs, three spare records");
    let mut ms: Vec<Mutant> = Vec::new();
    for &e in &nones {
        for (field, at, v) in
            [("r_offset", e, 8u64), ("r_sym", e + 8, 1 << 32), ("r_addend", e + 16, 8)]
        {
            let mut m = b.clone();
            m[at..at + 8].copy_from_slice(&v.to_le_bytes());
            ms.push((format!("the type-0 record at {e:#x}: {field} {v:#x}"), m));
        }
    }
    // One more: the last record (a GLOB_DAT) zeroed.
    let mut m = b.clone();
    let last = *recs.last().unwrap();
    m[last..last + 24].fill(0);
    ms.push(("the last record zeroed: one type-0 record past the local pairs".into(), m));
    // DT_RELACOUNT raised past the spare records.
    let (doff, dsize, _) = section_at(&b, ".dynamic").unwrap();
    let rc = (doff..doff + dsize)
        .step_by(16)
        .find(|&e| u64at(&b, e) == 0x6fff_fff9)
        .expect("DT_RELACOUNT");
    let mut m = b.clone();
    let k = u64at(&b, rc + 8) + nones.len() as u64;
    m[rc + 8..rc + 16].copy_from_slice(&k.to_le_bytes());
    ms.push((format!("DT_RELACOUNT {k}: the spare records inside its prefix"), m));
    for (what, m) in ms {
        let r = o.verify_bytes(&m);
        let new = r
            .findings
            .iter()
            .find(|f| known_anomaly(&o.link, f).is_none() && !clean_keys.contains(&key(f)));
        assert!(new.is_some(), "{what}: no new finding outside A46");
        if std::env::var("SHARED_DEBUG").is_ok_and(|v| v == "a46") {
            let n = r.findings.iter().filter(|f| a46(f)).count();
            eprintln!("{what}: A46 {n}; {:?}", new.map(|f| &f.message));
        }
    }
    let r = o.verify_as_mortar(&b);
    assert!(!r.findings.iter().any(|f| f.message == mortar_plumb::shared::SPARE_NONE));
    let plain = r
        .findings
        .iter()
        .filter(|f| f.rule == mortar_plumb::Rule::Dyn(25) && f.message.contains("of type 0"));
    assert!(plain.count() > 0, "as mortar's output: no P-D25 plain finding");
}

/// The file offset and address of section `name` (ELF64), if any.
fn section_at(b: &[u8], name: &str) -> Option<(usize, usize, u64)> {
    let (off, ent, n, str_i) =
        (u64at(b, 0x28) as usize, u16at(b, 0x3a), u16at(b, 0x3c), u16at(b, 0x3e));
    let strs = u64at(b, off + str_i * ent + 24) as usize;
    (0..n).map(|i| off + i * ent).find_map(|h| {
        let at = strs + u32::from_le_bytes(b[h..h + 4].try_into().unwrap()) as usize;
        (b[at..].split(|&c| c == 0).next() == Some(name.as_bytes()))
            .then(|| (u64at(b, h + 24) as usize, u64at(b, h + 32) as usize, u64at(b, h + 16)))
    })
}

/// One mutation of an x86-64 TLS library: what it changes, and the bytes.
type Mutant = (String, Vec<u8>);

/// WP-144's mutations of an x86-64 TLS library, one per TLS form: each
/// TLS record of `.rela.dyn` and `.rela.plt` (`DTPOFF64`, `TPOFF64` and
/// `TLSDESC` addend + 8; `DTPMOD64` retyped `TPOFF64`), each static
/// `DTPOFF` word beside a `DTPMOD64` with `r_sym` 0 and no record (+ 8),
/// and each descriptor's `lea` reaching the next 16 bytes.
fn tls_mutants(b: &[u8]) -> Vec<Mutant> {
    const DTPMOD64: u64 = 16;
    const DTPOFF64: u64 = 17;
    const TPOFF64: u64 = 18;
    const TLSDESC: u64 = 36;
    let mut recs = Vec::new();
    for name in [".rela.dyn", ".rela.plt"] {
        let Some((off, size, _)) = section_at(b, name) else { continue };
        recs.extend((off..off + size).step_by(24).map(|e| (e, u64at(b, e), u64at(b, e + 8))));
    }
    let placed: BTreeSet<u64> = recs.iter().map(|r| r.1).collect();
    let file_of = |addr: u64| -> Option<usize> {
        [".got", ".got.plt"].iter().find_map(|n| {
            let (off, size, at) = section_at(b, n)?;
            (at..at + size as u64).contains(&addr).then(|| off + (addr - at) as usize)
        })
    };
    let mut out = Vec::new();
    for &(e, place, info) in &recs {
        let (ty, sym) = (info & 0xffff_ffff, info >> 32);
        let mut m = b.to_vec();
        match ty {
            DTPOFF64 | TPOFF64 | TLSDESC => {
                let a = u64at(b, e + 16).wrapping_add(8);
                m[e + 16..e + 24].copy_from_slice(&a.to_le_bytes());
                out.push((format!("type {ty} r_sym {sym} at {place:#x}: addend + 8"), m));
            }
            DTPMOD64 => {
                let i = (info & !0xffff_ffff) | TPOFF64;
                m[e + 8..e + 16].copy_from_slice(&i.to_le_bytes());
                out.push((format!("DTPMOD64 r_sym {sym} at {place:#x}: retyped TPOFF64"), m));
                if sym == 0
                    && !placed.contains(&(place + 8))
                    && let Some(w) = file_of(place + 8)
                {
                    let mut m = b.to_vec();
                    let v = u64at(b, w).wrapping_add(8);
                    m[w..w + 8].copy_from_slice(&v.to_le_bytes());
                    out.push((format!("the static DTPOFF word at {:#x}: + 8", place + 8), m));
                }
            }
            _ => {}
        }
        if ty == TLSDESC
            && let Some((off, size, at)) = section_at(b, ".text")
        {
            for i in off..off + size.saturating_sub(7) {
                let (rex, op, modrm) = (b[i], b[i + 1], b[i + 2]);
                if rex & 0xf8 != 0x48 || op != 0x8d || modrm & 0xc7 != 0x05 {
                    continue;
                }
                let disp = i32::from_le_bytes(b[i + 3..i + 7].try_into().unwrap());
                let next = at + (i + 7 - off) as u64;
                if next.wrapping_add(i64::from(disp) as u64) == place {
                    let mut m = b.to_vec();
                    m[i + 3..i + 7].copy_from_slice(&(disp + 16).to_le_bytes());
                    out.push((
                        format!("the lea at {:#x} of the descriptor at {place:#x}: + 16", next - 7),
                        m,
                    ));
                }
            }
        }
    }
    out
}

// WP-144 (P-D26, X-61): every TLS form of the x86-64 libraries is
// checked, not skipped: each mutation of `tls_mutants` gives a finding
// the clean library does not have. Every dialect is mutated at least once.
#[test]
fn p_d26_x64_tls_records_are_checked() {
    let Some(c) = corpus() else { return };
    let mut n = 0;
    let key = |f: &mortar_plumb::Finding| format!("{} {}", f.rule.id(), f.message);
    for o in c.outs.iter().filter(|o| o.case == "tls") {
        let b = o.link.output_bytes();
        let clean: BTreeSet<String> = o.verify_bytes(&b).findings.iter().map(key).collect();
        let ms = tls_mutants(&b);
        assert!(!ms.is_empty(), "{}: no TLS form found", o.label());
        for (what, m) in ms {
            n += 1;
            let r = o.verify_bytes(&m);
            let new: Vec<String> =
                r.findings.iter().map(key).filter(|k| !clean.contains(k)).collect();
            assert!(!new.is_empty(), "{} {what}: NOT caught", o.label());
            if std::env::var("SHARED_DEBUG").is_ok_and(|v| v == "tlsmut") {
                eprintln!("{} {what}: {}", o.label(), new[0]);
            }
        }
    }
    assert!(n > 0 || c.outs.is_empty(), "no TLS mutation");
}

/// mortar's x86-64 TLS library `name` of `tests/shared/x64tls`
/// (`gen.sh`) judged under its map, as `bytes`.
fn mortar_x64tls(name: &str, bytes: Option<&[u8]>) -> Report {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shared/x64tls");
    let text = std::fs::read_to_string(d.join(format!("libt-{name}.map"))).unwrap();
    let map = mortar_plumb::map::parse(mortar_plumb::Format::Mortar, &text).unwrap();
    let files: Vec<(String, Vec<u8>)> = map
        .paths()
        .into_iter()
        .filter_map(|p| std::fs::read(d.join(p)).ok().map(|b| (p.to_string(), b)))
        .collect();
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let own = std::fs::read(d.join(format!("libt-{name}.so"))).unwrap();
    let out = bytes.unwrap_or(&own);
    mortar_plumb::verify_with(out, &inputs, &map, &Ctx { line: None, files: &Dir(&d) })
}

// WP-144 (P-D26, X-61, D-46): mortar's x86-64 shared objects of WP-127
// part 2 (fixtures) are clean: the kept descriptor pair (R1) with
// `_TLS_MODULE_BASE_`'s block-start descriptor, the general-dynamic pair
// of a local key (`gd0`: `DTPMOD64` with `r_sym` 0 and the static DTPOFF
// word no record writes, which D-46's mortar rows now count as asked
// for), LD and IE; and every mutation of `tls_mutants` is caught.
#[test]
fn p_d26_mortar_x64_tls_libraries() {
    let key = |f: &mortar_plumb::Finding| format!("{} {}", f.rule.id(), f.message);
    for name in ["gd0", "desc", "gd", "ie"] {
        let r = mortar_x64tls(name, None);
        let got: Vec<String> = r.findings.iter().map(key).collect();
        assert!(got.is_empty(), "libt-{name}.so: {got:#?}");
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shared/x64tls");
        let b = std::fs::read(d.join(format!("libt-{name}.so"))).unwrap();
        let ms = tls_mutants(&b);
        assert!(!ms.is_empty(), "libt-{name}.so: no TLS form found");
        for (what, m) in ms {
            let r = mortar_x64tls(name, Some(&m));
            assert!(!r.findings.is_empty(), "libt-{name}.so {what}: NOT caught");
        }
    }
}

// The link tests' reference libraries (WP-121, after part 1): clean.
#[test]
fn s12_5_reference_link_tests_verify_clean() {
    judge("refs");
}

// P-D22 (D-115): `foo@V1` without VERSYM_HIDDEN is a second default
// definition of `foo` (an unversioned lookup finds the first).
#[test]
fn p_d22_hidden_version_is_kept() {
    let Some(c) = corpus() else { return };
    let mut n = 0;
    for o in c.outs.iter().filter(|o| o.link.output.to_string_lossy().contains("libsv.so")) {
        n += 1;
        let mut b = o.link.output_bytes();
        let (off, size) = section(&b, ".gnu.version");
        let at = (off..off + size).step_by(2).find(|&i| u16at(&b, i) & 0x8000 != 0).unwrap();
        b[at + 1] &= 0x7f;
        let r = o.verify_bytes(&b);
        assert!(
            r.findings.iter().any(|f| f.rule.id() == "P-D22"
                && f.message.contains("'foo' has 2 definitions without VERSYM_HIDDEN")),
            "{}: {:#?}",
            o.label(),
            r.findings
        );
    }
    assert!(n == 0 || n == 3, "{n} libsv outputs");
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

// spec/plumb.md P-D23 (WP-121 part 2): each row's wrong output reported
// by the rule the table names; the address clause's refusals; the
// residual hole not caught.
#[test]
fn p_d23_rows() {
    judge("rows");
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

const PT_GNU_EH_FRAME: u32 = 0x6474_e550;

// spec/plumb.md P-D23, W28 (NOT caught): a C++ library linked without
// `--eh-frame-hdr` has no `.eh_frame_hdr` and no `PT_GNU_EH_FRAME`, which
// is what its line asks for; only a check of the line could name it
// (WP-130). GNU ld re-run on each g++ library's recorded words without
// `--eh-frame-hdr` (and without the LTO plugin's): the clean library's
// verdict.
#[test]
fn p_d23_w28_is_not_caught() {
    let Some(c) = corpus() else { return };
    let mut n = 0;
    let verdict =
        |r: &Report| -> BTreeSet<String> { r.findings.iter().map(|f| f.rule.id()).collect() };
    for o in c.outs.iter().filter(|o| {
        o.link.linker == Linker::Bfd && o.link.output.to_string_lossy().contains("libwo.so")
    }) {
        let name = "libwo-w28.so.bfd";
        let mut words = Vec::new();
        let mut it = o.words().into_iter();
        while let Some(w) = it.next() {
            match w.as_str() {
                "--eh-frame-hdr" => {}
                "-plugin" => drop(it.next()),
                "-o" => {
                    drop(it.next());
                    words.extend(["-o".to_string(), name.to_string()]);
                }
                _ if w.starts_with("-plugin-opt") => {}
                _ if w.starts_with("-Map=") => words.push(format!("-Map={name}.map")),
                _ => words.push(w),
            }
        }
        let root = o.link.dir.parent().and_then(Path::parent).unwrap();
        let ld = root.join(format!("ld-{}-bfd", o.arch.name())).join("ld");
        let line = o.link.dir.join(format!("{name}.line"));
        let w: Vec<&str> = words.iter().map(String::as_str).collect();
        assert!(run_in(&o.link.dir, &ld, &w, Some(&line)), "{}: the W28 link failed", o.label());
        let w28 = Out {
            case: "w28",
            arch: o.arch,
            link: Link {
                case: "w28",
                linker: Linker::Bfd,
                dir: o.link.dir.clone(),
                output: o.link.dir.join(name),
                map: o.link.dir.join(format!("{name}.map")),
            },
            line,
            expect: &[],
        };
        let b = w28.link.output_bytes();
        assert!(
            phdr(&b, PT_GNU_EH_FRAME).is_none(),
            "{}: W28's form has no PT_GNU_EH_FRAME",
            o.label()
        );
        n += 1;
        assert_eq!(
            verdict(&w28.verify()),
            verdict(&o.verify()),
            "{}: W28 is asserted NOT caught (P-D23)",
            o.label()
        );
    }
    assert!(n > 0 || c.outs.is_empty(), "no g++ library of GNU ld");
}

// spec/plumb.md P-D19 and P-D7 (mortar) (WP-140; dyn.md D-10 as WP-137
// amends it): in `Shared` `bind(k)` ranges over N alone. g++'s
// `--as-needed` drops the C library; crtbeginS's weak `__cxa_finalize`
// is defined only there (libstdc++ still lists it). Judged under mortar's
// rows: GNU ld's form (`WEAK NOTYPE UND`, unversioned, no `libc.so.6`) is
// no longer asked to be the C library's `STT_FUNC`; lld's `WEAK FUNC` is
// P-D7's type finding; mold's (versioned, `libc.so.6` needed) with the
// `DT_NEEDED` of `libc.so.6` removed is P-D8's `vn_file` finding and P-D7's
// type finding. Every reference's own verdict is `refs`' (clean).
#[test]
fn p_d19_weak_key_outside_n_is_unbound() {
    let Some(c) = corpus() else { return };
    let mut n = 0;
    let about = |r: &Report, id: &str| -> Vec<String> {
        r.findings
            .iter()
            .filter(|f| f.rule.id() == id && f.symbol.as_deref() == Some("__cxa_finalize"))
            .map(|f| f.message.clone())
            .collect()
    };
    for o in c.outs.iter().filter(|o| o.link.output.to_string_lossy().contains("libwo.so")) {
        n += 1;
        let b = o.link.output_bytes();
        match o.link.linker {
            Linker::Bfd => {
                let r = o.verify_as_mortar(&b);
                for id in ["P-D7", "P-D8"] {
                    let got = about(&r, id);
                    assert!(got.is_empty(), "{}: {id} {got:#?}", o.label());
                }
            }
            Linker::Lld => {
                let got = about(&o.verify_as_mortar(&b), "P-D7");
                assert!(
                    got.iter().any(|m| m.contains("has type 2 (D-23: 0)")),
                    "{}: {got:#?}",
                    o.label()
                );
            }
            Linker::Mold => {
                let mut b = b;
                drop_needed(&mut b, "libc.so.6");
                let r = o.verify_as_mortar(&b);
                assert!(
                    r.findings.iter().any(|f| f.rule.id() == "P-D8"
                        && f.message.contains("vn_file 'libc.so.6' is not a DT_NEEDED name")),
                    "{}: {:#?}",
                    o.label(),
                    r.findings
                );
                let got = about(&r, "P-D7");
                assert!(
                    got.iter().any(|m| m.contains("has type 2 (D-23: 0)")),
                    "{}: {got:#?}",
                    o.label()
                );
            }
        }
    }
    assert!(n >= 3 || c.outs.is_empty(), "{n} libwo outputs");
}

/// Removes the `DT_NEEDED` entry naming `soname` (the later entries move
/// up one, a `DT_NULL` last).
fn drop_needed(b: &mut [u8], soname: &str) {
    let h = pt_dynamic(b);
    let (off, size) = (u64at(b, h + 8) as usize, u64at(b, h + 32) as usize);
    let (str_off, str_size) = section(b, ".dynstr");
    let name_at = |b: &[u8], v: usize| -> String {
        let s = &b[str_off + v..str_off + str_size];
        String::from_utf8_lossy(&s[..s.iter().position(|&c| c == 0).unwrap()]).into_owned()
    };
    let e = (off..off + size)
        .step_by(16)
        .find(|&e| u64at(b, e) == 1 && name_at(b, u64at(b, e + 8) as usize) == soname)
        .unwrap_or_else(|| panic!("no DT_NEEDED {soname}"));
    b.copy_within(e + 16..off + size, e);
    b[off + size - 16..off + size].fill(0);
}

// spec/plumb.md P-D23: the i386 rows (W25-W29, W35-W37, W39) are outside
// stage 3 (i386 dynamic outputs are P-D1 by name, §12.1): an i386 library
// by GNU ld is one P-D1 finding naming i386, and nothing else is judged.
#[test]
fn p_d23_i386_is_outside_stage_3() {
    let (Some(cc), Some(ld)) = (tool("i686-linux-gnu-gcc"), tool("i686-linux-gnu-ld.bfd")) else {
        return;
    };
    let scratch = Scratch::new("plumb-shared-i386");
    let d = scratch.path();
    std::fs::write(d.join("dep.c"), SOURCES.iter().find(|s| s.0 == "dep.c").unwrap().1).unwrap();
    let bin = d.join("ld-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script =
        format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$LINE_OUT\"\nexec {} \"$@\"\n", ld.display());
    std::fs::write(bin.join("ld"), script).unwrap();
    let mut perm = std::fs::metadata(bin.join("ld")).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
    std::fs::set_permissions(bin.join("ld"), perm).unwrap();
    must(d, &cc, &["-O1", "-fPIC", "-c", "dep.c", "-o", "dep.o"]);
    let b = format!("-B{}/", bin.display());
    let line = d.join("lib.line");
    let args = [b.as_str(), "-shared", "dep.o", "-o", "lib.so", "-Wl,-Map=lib.map"];
    assert!(run_in(d, &cc, &args, Some(&line)), "the i386 link failed");
    let o = Out {
        case: "i386",
        arch: Arch::X64,
        link: Link {
            case: "i386",
            linker: Linker::Bfd,
            dir: d.to_path_buf(),
            output: d.join("lib.so"),
            map: d.join("lib.map"),
        },
        line,
        expect: &[],
    };
    let r = o.verify();
    let ids: Vec<String> = r.findings.iter().map(|f| f.rule.id()).collect();
    assert!(
        r.findings.iter().any(|f| f.rule.id() == "P-D1" && f.message.contains("i386 (EM_386")),
        "{:#?}",
        r.findings
    );
    assert!(ids.iter().all(|i| i == "P-D1"), "{:#?}", r.findings);
}
