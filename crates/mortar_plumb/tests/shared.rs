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
    "xmain.c",
    "xfoo.c",
    "xdd.c",
    "x.list",
    "sv.c",
    "sv.map",
    "weak.cc",
    "vd.c",
    "vd.map",
    "vd3.map",
    "vd31.map",
    "vdx.c",
    "vdx.map",
    "rks.c",
    "rm0.c",
    "rext.ld",
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
        self.verify_line_as_mortar(out, self.words())
    }

    /// `out` judged as mortar's output of the line `words`.
    fn verify_line_as_mortar(&self, out: &[u8], words: Vec<String>) -> Report {
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
    verdef(&mut b);
    optail(&mut b);
    roots(&mut b);
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

/// WP-161: an executable's `--exclude-libs` and `--dynamic-list` (D-110
/// item 1, resolve §9.6), by each reference on both targets: `-E` with and
/// without the exclusion, and no option with and without the list. Every
/// link is the case `optail`.
fn optail(b: &mut Builder<'_>) {
    for arch in [Arch::A64, Arch::X64] {
        let d = b.dir(arch, "optail");
        std::fs::copy(b.root.join("x.list"), d.join("x.list")).unwrap();
        b.obj(arch, &d, "xmain.c", &["-O1", "-fPIE"], "xmain.o");
        b.obj(arch, &d, "xfoo.c", &["-O1", "-fPIC"], "xfoo.o");
        let ar = b.t.ar.clone();
        must(&d, &ar, &["rc", "libxfoo.a", "xfoo.o"]);
        b.so(arch, &d, "xdd.c", "libxdd.so", "libxdd.so");
        let base = ["-pie", "xmain.o", "libxfoo.a", "libxdd.so"];
        for (out, extra) in [
            ("e-plain", &["-Wl,-E"][..]),
            ("e-ex", &["-Wl,-E", "-Wl,--exclude-libs,ALL"][..]),
            ("d-plain", &[][..]),
            ("d-list", &["-Wl,--dynamic-list,x.list"][..]),
        ] {
            let mut args = base.to_vec();
            args.extend(extra);
            for l in Linker::ALL {
                // Each reference's default anomalies of an executable.
                let want: &'static [&'static str] = match l {
                    Linker::Mold => &["?A3", "?A9", "?A13"],
                    Linker::Bfd => &["?A10"],
                    Linker::Lld => &[],
                };
                b.link("optail", arch, &d, out, &args, l, want);
            }
        }
    }
}

/// WP-163 (resolve.md §4.2, dyn.md D-11 item 2, D-21 item 1, L-103): a
/// root names a definition only an `--as-needed` library has (`u`: the
/// library is not needed, `k_fn` written nowhere; mold makes it needed,
/// A48), a needed library's (`noas`: imported; lld and mold write none, A47), a
/// quoted `EXTERN("k_fn")` of an implicit script with the library needed
/// (`ext`), and `-e k_fn` where the library is not needed (`entry`: GNU
/// ld warns and writes the start of `.text`). Every link is the case
/// `roots`.
fn roots(b: &mut Builder<'_>) {
    use Linker::{Bfd, Lld, Mold};
    const ASN: [&str; 3] = ["-Wl,--as-needed", "-lks", "-Wl,--no-as-needed"];
    for arch in [Arch::A64, Arch::X64] {
        let d = b.dir(arch, "roots");
        std::fs::copy(b.root.join("rext.ld"), d.join("ext.ld")).unwrap();
        b.obj(arch, &d, "rm0.c", &["-O1", "-fPIE"], "m0.o");
        b.so(arch, &d, "rks.c", "libks.so", "libks.so");
        for (pie, sfx) in [("-pie", "pie"), ("-no-pie", "nopie")] {
            for l in Linker::ALL {
                let want: &'static [&'static str] = match l {
                    Mold => &["?A3", "?A9", "?A13", "A48"],
                    Bfd => &["?A10"],
                    Lld => &[],
                };
                let mut args = vec![pie, "m0.o", "-u", "k_fn", "-L."];
                args.extend(ASN);
                b.link("roots", arch, &d, &format!("u-{sfx}"), &args, l, want);
                let noas: &'static [&'static str] = match l {
                    Bfd => &["?A10"],
                    Lld => &["A47"],
                    Mold => &["?A3", "?A9", "?A13", "A47"],
                };
                let args = [pie, "m0.o", "-u", "k_fn", "-L.", "-Wl,--no-as-needed", "-lks"];
                b.link("roots", arch, &d, &format!("noas-{sfx}"), &args, l, noas);
                if l != Mold {
                    // mold 2.37 refuses EXTERN.
                    let args = [pie, "m0.o", "ext.ld", "-L.", "-Wl,--no-as-needed", "-lks"];
                    b.link("roots", arch, &d, &format!("ext-{sfx}"), &args, l, noas);
                }
            }
            let mut args = vec![pie, "m0.o", "-Wl,-e,k_fn", "-L."];
            args.extend(ASN);
            b.link("roots", arch, &d, &format!("entry-{sfx}"), &args, Bfd, &["?A10"]);
        }
    }
}

// WP-163: the roots' links of every reference verify clean but for their
// named differences (A47, A48).
#[test]
fn p_d_roots_of_as_needed_libraries() {
    judge("roots");
}

/// Points the `.dynsym` entry named `from` at `to`, a string of `.dynstr`
/// (a suffix of one is a string).
fn rename_dynsym(b: &mut [u8], from: &str, to: &str) {
    let (sym, size) = section(b, ".dynsym");
    let (strs, ssize) = section(b, ".dynstr");
    let name = |b: &[u8], at: usize| b[at..].split(|&c| c == 0).next().unwrap().to_vec();
    let to_at = (0..ssize)
        .find(|&i| b[strs + i..].starts_with(to.as_bytes()) && b[strs + i + to.len()] == 0)
        .expect("no such string") as u32;
    let e = (sym..sym + size)
        .step_by(24)
        .find(|&e| name(b, strs + u32at(b, e) as usize) == from.as_bytes())
        .expect("no such entry");
    put32(b, e, to_at);
}

/// The words of `o`'s line with `-lks` under `--as-needed`, the words
/// after it as they were (GCC's driver passes `--as-needed` before the
/// inputs on this host).
fn ks_as_needed(o: &Out) -> Vec<String> {
    let mut w = o.words();
    let i = w.iter().position(|x| x == "-lks").expect("-lks");
    let after = w.iter().take(i).rev().find(|x| x.ends_with("as-needed")).cloned();
    let mut put = vec!["--as-needed".to_string(), "-lks".to_string()];
    put.extend(after);
    w.splice(i..=i, put);
    w
}

// WP-163's mutations (spec/plumb.md §9.2's form), on the references'
// outputs judged as mortar's: the library in DT_NEEDED though only a root
// names it (P-D6); `k_fn` imported though no needed library defines it,
// and not imported though one does (P-D7); `e_entry` not 0 for an entry
// symbol the output does not define, and 0 for one it does (P-D4).
#[test]
fn p_d_roots_mutations_are_caught() {
    let Some(c) = corpus() else { return };
    let has = |r: &Report, rule: &str, want: &str| {
        r.findings.iter().any(|f| f.rule.id() == rule && f.message.contains(want))
    };
    let mut n = 0;
    let mut caught = |what: String, r: &Report, rule: &str, want: &str| {
        n += 1;
        assert!(has(r, rule, want), "{what}: no {rule} '{want}' in\n{}", r.render("o"));
    };
    let get = |arch: Arch, l: Linker, out: &str| {
        c.outs
            .iter()
            .find(|o| {
                o.case == "roots"
                    && o.arch == arch
                    && o.link.linker == l
                    && o.link
                        .output
                        .file_name()
                        .is_some_and(|f| f.to_string_lossy() == format!("{out}.{}", l.name()))
            })
            .unwrap_or_else(|| panic!("no {out} of {}", l.name()))
    };
    for arch in [Arch::A64, Arch::X64] {
        for sfx in ["pie", "nopie"] {
            let what = |s: &str| format!("{} {sfx}: {s}", arch.name());
            // Clean as mortar's: GNU ld's needed set, imports and entry.
            for out in ["u", "noas", "ext", "entry"] {
                let o = get(arch, Linker::Bfd, &format!("{out}-{sfx}"));
                let bytes = o.link.output_bytes();
                let mut b = bytes.clone();
                if out == "entry" {
                    // L-103: mortar writes 0 for the undefined entry.
                    b[0x18..0x20].copy_from_slice(&0u64.to_le_bytes());
                }
                // A reference's output judged as mortar's has mortar's
                // other rules to meet (the map, D-26, D-30); the roots'
                // rules are clean.
                let r = o.verify_as_mortar(&b);
                let k: Vec<String> = keys(&r)
                    .into_iter()
                    .filter(|k| k.starts_with("P-D4") || k.contains("k_fn") || k.contains("libks"))
                    .collect();
                assert!(k.is_empty(), "{}: {k:#?}", what(&format!("GNU ld's {out} as mortar's")));
            }
            // P-D6: a root made the library needed (mold's output; GNU
            // ld's noas output judged against the --as-needed line).
            let o = get(arch, Linker::Mold, &format!("u-{sfx}"));
            let r = o.verify_as_mortar(&o.link.output_bytes());
            caught(what("mold's u"), &r, "P-D6", "only a root names a definition of it");
            let o = get(arch, Linker::Bfd, &format!("noas-{sfx}"));
            let r = o.verify_line_as_mortar(&o.link.output_bytes(), ks_as_needed(o));
            caught(
                what("noas under --as-needed"),
                &r,
                "P-D6",
                "only a root names a definition of it",
            );
            // P-D7: k_fn imported though no needed library defines it.
            caught(
                what("noas under --as-needed"),
                &r,
                "P-D7",
                "'k_fn' is imported and no library of the needed set defines it",
            );
            // P-D7: k_fn not imported though a needed library defines it
            // (GNU ld's noas output with its .dynsym entry renamed; lld's
            // noas and ext outputs, whose A47 is no difference of mortar's).
            let o = get(arch, Linker::Bfd, &format!("noas-{sfx}"));
            let mut b = o.link.output_bytes();
            rename_dynsym(&mut b, "k_fn", "ks.so");
            let r = o.verify_as_mortar(&b);
            caught(what("noas, k_fn renamed"), &r, "P-D7", "D-21 item 1: a root is imported");
            for out in ["noas", "ext"] {
                let o = get(arch, Linker::Lld, &format!("{out}-{sfx}"));
                let r = o.verify_as_mortar(&o.link.output_bytes());
                caught(
                    what(&format!("lld's {out}")),
                    &r,
                    "P-D7",
                    "D-21 item 1: a root is imported",
                );
            }
            // P-D4: GNU ld's e_entry for the undefined k_fn (the start of
            // .text) is not mortar's 0.
            let o = get(arch, Linker::Bfd, &format!("entry-{sfx}"));
            let bytes = o.link.output_bytes();
            assert_ne!(u64at(&bytes, 0x18), 0, "{}", what("GNU ld's e_entry"));
            let r = o.verify_as_mortar(&bytes);
            caught(what("entry as mortar's"), &r, "P-D4", "L-103 writes 0");
            // P-D4: e_entry 0 though _start is defined, from every linker.
            for l in [Linker::Bfd, Linker::Lld, Linker::Mold] {
                let o = get(arch, l, &format!("u-{sfx}"));
                let mut b = o.link.output_bytes();
                b[0x18..0x20].copy_from_slice(&0u64.to_le_bytes());
                let r = o.verify_bytes(&b);
                caught(
                    what(&format!("{}'s u, e_entry 0", l.name())),
                    &r,
                    "P-D4",
                    "the entry point",
                );
                if l == Linker::Bfd {
                    let r = o.verify_line_as_mortar(&b, o.words());
                    caught(what("u as mortar's, e_entry 0"), &r, "P-D4", "the entry point");
                }
            }
        }
    }
    assert_eq!(n, 2 * 2 * 11, "mutations");
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

// WP-161 (P-D22, D-103): `-z origin` sets `DF_ORIGIN` in `DT_FLAGS`:
// the flag cleared, and the line read without `-z origin`, are findings.
#[test]
fn p_d22_df_origin_follows_the_line() {
    let Some(c) = corpus() else { return };
    let mut n = 0;
    for o in c
        .outs
        .iter()
        .filter(|o| o.case == "refs" && o.link.output.to_string_lossy().contains("libflags.so"))
    {
        n += 1;
        let mut b = o.link.output_bytes();
        let h = pt_dynamic(&b);
        let (off, size) = (u64at(&b, h + 8) as usize, u64at(&b, h + 32) as usize);
        let e = (off..off + size).step_by(16).find(|&e| u64at(&b, e) == 0x1e).unwrap();
        let v = u64at(&b, e + 8) & !1;
        b[e + 8..e + 16].copy_from_slice(&v.to_le_bytes());
        let r = o.verify_bytes(&b);
        assert!(
            r.findings
                .iter()
                .any(|f| f.rule.id() == "P-D22" && f.message.contains("DF_ORIGIN false")),
            "{}: {:#?}",
            o.label(),
            r.findings
        );
        let mut words = o.words();
        let at = words.iter().position(|w| w == "origin").unwrap();
        words.drain(at - 1..=at);
        let r = o.verify_line(&o.link.output_bytes(), words);
        for want in ["DF_ORIGIN true; the line asks for -z origin false", "bits 0x80 no -z word"] {
            assert!(
                r.findings.iter().any(|f| f.rule.id() == "P-D22" && f.message.contains(want)),
                "{}: no '{want}' in {:#?}",
                o.label(),
                r.findings
            );
        }
    }
    assert!(n == 0 || n == 6, "{n} libflags outputs");
}

// WP-161 (P-D7, D-110 item 1, resolve §9.6): an executable's exports
// under `--exclude-libs` and `--dynamic-list`, by each reference.
#[test]
fn s12_5_executable_option_tail_verifies_clean() {
    judge("optail");
}

/// `words` with `add` inserted before the first input, or every word of
/// `drop` (and the value after a two-word one) removed.
fn edit_words(mut words: Vec<String>, add: &[&str], drop: &[&str]) -> Vec<String> {
    let mut i = 0;
    while i < words.len() {
        if drop.iter().any(|d| words[i] == *d) {
            let two = !words[i].contains(',') && !words[i].contains('=');
            words.drain(i..if two { i + 2 } else { i + 1 });
        } else {
            i += 1;
        }
    }
    let at = words
        .iter()
        .position(|w| Path::new(w).extension().is_some_and(|e| e.eq_ignore_ascii_case("o")))
        .unwrap_or(0);
    for (k, a) in add.iter().enumerate() {
        words.insert(at + k, (*a).to_string());
    }
    words
}

// WP-161 (P-D7, P-D6): mutations of the line on each reference's
// executables: `-E` output judged as if `--exclude-libs ALL` were on the
// line (an excluded definition exported); the excluded output without
// the option (`foo_fn`, defined by `libxdd.so` too, not exported: W8);
// the plain output as if `--dynamic-list x.list` listed `keep_fn` (a
// listed definition not exported).
#[test]
fn p_d7_option_tail_mutations_are_caught() {
    let Some(c) = corpus() else { return };
    let mut n = 0;
    for o in c.outs.iter().filter(|o| o.case == "optail") {
        let file = o.link.output.file_name().unwrap().to_string_lossy().into_owned();
        let (words, want) = if file.starts_with("e-plain") {
            (
                edit_words(o.words(), &["--exclude-libs", "ALL"], &[]),
                "'foo_fn' is exported, and its definition is in an archive member --exclude-libs names",
            )
        } else if file.starts_with("e-ex") {
            (
                edit_words(o.words(), &[], &["--exclude-libs"]),
                "'foo_fn' is defined by the program and referenced by a needed or used object, and is not exported",
            )
        } else if file.starts_with("d-plain") {
            (
                edit_words(o.words(), &["--dynamic-list", "x.list"], &[]),
                "'keep_fn' is defined by the program and listed by the line's --dynamic-list, and is not exported",
            )
        } else {
            continue;
        };
        n += 1;
        let r = o.verify_line(&o.link.output_bytes(), words.clone());
        assert!(
            r.findings.iter().any(|f| f.rule.id() == "P-D7" && f.message.contains(want)),
            "{}: no '{want}' under {words:?} in {:#?}",
            o.label(),
            r.findings
        );
    }
    assert!(n == 0 || n == 18, "{n} optail mutations");
}

// WP-161 (P-D19, D-110 item 1): `libmore.so` (`--exclude-libs ALL`) read
// without the option: the archive member's `arfn` is an export missing
// from `.dynsym`.
#[test]
fn p_d19_exclude_libs_mutation_is_caught() {
    let Some(c) = corpus() else { return };
    let mut n = 0;
    for o in c.outs.iter().filter(|o| o.case == "more") {
        n += 1;
        let words = edit_words(o.words(), &[], &["--exclude-libs"]);
        let r = o.verify_line(&o.link.output_bytes(), words);
        assert!(
            r.findings.iter().any(|f| f.rule.id() == "P-D19" && f.message.contains("'arfn'")),
            "{}: {:#?}",
            o.label(),
            r.findings
        );
    }
    assert!(n == 0 || n == 6, "{n} libmore outputs");
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

/// WP-149 (P-D27, D-115): `vd.c`'s library by each reference on both
/// arches: `libvd.so` under `vd.map` (V1, V2 with parent V1; `-soname`)
/// and `libvd3.so` under `vd3.map` (V3 with parents V1 V2; no `-soname`:
/// the base is the output's name, lld's the `-o` as written; lld and mold
/// refuse two parents, measured, so theirs is `vd31.map`, V3's one parent
/// V2), each with
/// `foo@V1` hidden and `foo@@V2`, the local `hid`, and `ext` imported
/// from `libvdx.so`'s `X1` (a Vernaux after the Verdefs). Every link is
/// the case `verdef`.
fn verdef(b: &mut Builder<'_>) {
    for arch in [Arch::A64, Arch::X64] {
        let d = b.dir(arch, "verdef");
        for f in ["vd.map", "vd3.map", "vd31.map", "vdx.map"] {
            std::fs::copy(b.root.join(f), d.join(f)).unwrap();
        }
        let src = b.root.join("vdx.c").display().to_string();
        let args = [
            "-fPIC",
            "-O1",
            "-shared",
            "-Wl,-soname,libvdx.so",
            "-Wl,--version-script,vdx.map",
            &src,
            "-o",
            "libvdx.so",
        ];
        must(&d, b.cc(arch), &args);
        b.obj(arch, &d, "vd.c", &["-O1", "-fPIC"], "vd.o");
        for l in Linker::ALL {
            let args = ["-shared", "-Wl,-soname,libvd.so", "vd.o", "libvdx.so"];
            let mut a = args.to_vec();
            a.push("-Wl,--version-script,vd.map");
            b.link("verdef", arch, &d, "libvd.so", &a, l, row(l, &[]));
            let s = if l == Linker::Bfd { "vd3.map" } else { "vd31.map" };
            let s = format!("-Wl,--version-script,{s}");
            let a = ["-shared", "vd.o", "libvdx.so", &s];
            b.link("verdef", arch, &d, "libvd3.so", &a, l, row(l, &[]));
        }
    }
}

// WP-149: the versioned libraries of every reference are clean (P-D27's
// nodes, indices, parents, hashes and `.gnu.version`).
#[test]
fn p_d27_verdef_libraries_verify_clean() {
    judge("verdef");
}

fn u32at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// The file offset of the value of `.dynamic`'s first `tag` (ELF64).
fn dyn_value_at(b: &[u8], tag: u64) -> Option<usize> {
    let h = pt_dynamic(b);
    let (off, size) = (u64at(b, h + 8) as usize, u64at(b, h + 32) as usize);
    (off..off + size).step_by(16).find(|&e| u64at(b, e) == tag).map(|e| e + 8)
}

/// The `.gnu.version` slot of the defined `.dynsym` entry named `name`
/// whose raw version has the hidden bit `hidden` (ELF64).
fn versym_at(b: &[u8], name: &str, defined: bool, hidden: Option<bool>) -> Option<usize> {
    let (ds, dsz, _) = section_at(b, ".dynsym")?;
    let (st, _, _) = section_at(b, ".dynstr")?;
    let (vs, _, _) = section_at(b, ".gnu.version")?;
    (0..dsz / 24).find_map(|i| {
        let e = ds + i * 24;
        let at = st + u32at(b, e) as usize;
        let n = b[at..].split(|&c| c == 0).next()?;
        let slot = vs + 2 * i;
        let raw = u16at(b, slot);
        (n == name.as_bytes()
            && (u16at(b, e + 6) != 0) == defined
            && hidden.is_none_or(|h| (raw & 0x8000 != 0) == h))
        .then_some(slot)
    })
}

/// The Verdef records of `.gnu.version_d`: (offset, the offsets of its
/// Verdaux records), by `vd_next`/`vda_next`.
fn verdefs(b: &[u8]) -> Vec<(usize, Vec<usize>)> {
    let Some((off, size, _)) = section_at(b, ".gnu.version_d") else { return Vec::new() };
    let mut out = Vec::new();
    let mut at = off;
    while at + 20 <= off + size {
        let cnt = u16at(b, at + 6);
        let mut aux = Vec::new();
        let mut x = at + u32at(b, at + 12) as usize;
        for _ in 0..cnt {
            aux.push(x);
            let next = u32at(b, x + 4) as usize;
            if next == 0 {
                break;
            }
            x += next;
        }
        out.push((at, aux));
        let next = u32at(b, at + 16) as usize;
        if next == 0 {
            break;
        }
        at += next;
    }
    out
}

/// WP-149's mutations of a library of `vd.c` (P-D27; D-115): each must
/// be a new P-D27 finding. The parent's only where the library has one.
#[allow(clippy::too_many_lines)] // one mutation each
fn verdef_mutants(b: &[u8]) -> Vec<Mutant> {
    const DT_VERDEFNUM: u64 = 0x6fff_fffd;
    let mut v: Vec<Mutant> = Vec::new();
    let mut m = |what: &str, f: &dyn Fn(&mut Vec<u8>)| {
        let mut c = b.to_vec();
        f(&mut c);
        v.push((what.to_string(), c));
    };
    let vds = verdefs(b);
    assert!(vds.len() >= 3, "no V1 and V2 in .gnu.version_d");
    let (base, ref aux0) = vds[0];
    let (v1, ref aux1) = vds[1];
    let (v2, ref aux2) = vds[2];
    m("a wrong vd_hash (V2 + 1)", &|c| {
        let h = u32at(c, v2 + 8);
        put32(c, v2 + 8, h.wrapping_add(1));
    });
    m("VER_FLG_BASE cleared on the base entry", &|c| put16(c, base + 2, 0));
    m("VER_FLG_BASE set on V1", &|c| put16(c, v1 + 2, 1));
    m("V1 and V2's vd_ndx swapped", &|c| {
        let (a, z) = (u16at(c, v1 + 4), u16at(c, v2 + 4));
        put16(c, v1 + 4, u16::try_from(z).unwrap());
        put16(c, v2 + 4, u16::try_from(a).unwrap());
    });
    if let Some(at) = dyn_value_at(b, DT_VERDEFNUM) {
        m("DT_VERDEFNUM + 1", &|c| {
            let n = u64at(c, at);
            c[at..at + 8].copy_from_slice(&(n + 1).to_le_bytes());
        });
        m("DT_VERDEFNUM - 1", &|c| {
            let n = u64at(c, at);
            c[at..at + 8].copy_from_slice(&(n - 1).to_le_bytes());
        });
        let last2 = vds[vds.len() - 2].0;
        m("the last node missing (vd_next 0 before it, DT_VERDEFNUM - 1)", &|c| {
            put32(c, last2 + 16, 0);
            let n = u64at(c, at);
            c[at..at + 8].copy_from_slice(&(n - 1).to_le_bytes());
        });
    }
    m("the base's name is V1's", &|c| {
        let n = u32at(c, aux1[0]);
        put32(c, aux0[0], n);
    });
    m("V1's name is V2's", &|c| {
        let n = u32at(c, aux2[0]);
        put32(c, aux1[0], n);
    });
    if aux2.len() >= 2 {
        m("a wrong parent (V2's parent is V2)", &|c| {
            let n = u32at(c, aux2[0]);
            put32(c, aux2[1], n);
        });
        m("vd_cnt - 1 (V2's parent unread)", &|c| put16(c, v2 + 6, 1));
    } else {
        m("vd_cnt + 1 (a parent of V2 read past its names)", &|c| put16(c, v2 + 6, 2));
    }
    let slot = |n: &str, h: Option<bool>| versym_at(b, n, true, h).unwrap();
    let (bar, bz) = (slot("bar", None), slot("baz", None));
    m("bar's and baz's .gnu.version swapped", &|c| {
        let (x, y) = (u16at(c, bar), u16at(c, bz));
        put16(c, bar, u16::try_from(y).unwrap());
        put16(c, bz, u16::try_from(x).unwrap());
    });
    let hid = slot("foo", Some(true));
    m("VERSYM_HIDDEN cleared on foo@V1", &|c| c[hid + 1] &= 0x7f);
    m("VERSYM_HIDDEN set on bar", &|c| c[bar + 1] |= 0x80);
    let def = slot("foo", Some(false));
    m("VERSYM_HIDDEN set on foo@@V2", &|c| c[def + 1] |= 0x80);
    m("bar's .gnu.version 0 (local)", &|c| put16(c, bar, 0));
    m("bar's .gnu.version 1 (the base)", &|c| put16(c, bar, 1));
    if let Some(ext) = versym_at(b, "ext", false, None) {
        m("ext's .gnu.version 2 (a definition's index)", &|c| put16(c, ext, 2));
    }
    v
}

/// The rule ids and messages of a report.
fn keys(r: &Report) -> BTreeSet<String> {
    r.findings.iter().map(|f| format!("{} {}", f.rule.id(), f.message)).collect()
}

// WP-149 (P-D27): every mutation of `verdef_mutants` of each reference's
// versioned library is a new P-D27 finding.
#[test]
fn p_d27_verdef_mutations_are_caught() {
    let Some(c) = corpus() else { return };
    let mut n = 0;
    let mut missed = Vec::new();
    for o in c.outs.iter().filter(|o| o.case == "verdef") {
        let b = o.link.output_bytes();
        let clean = keys(&o.verify_bytes(&b));
        for (what, m) in verdef_mutants(&b) {
            n += 1;
            let new: Vec<String> =
                keys(&o.verify_bytes(&m)).into_iter().filter(|k| !clean.contains(k)).collect();
            if !new.iter().any(|k| k.starts_with("P-D27 ")) {
                missed.push(format!("{} {what}: {new:?}", o.label()));
            }
            if std::env::var("SHARED_DEBUG").is_ok_and(|v| v == "vdmut") {
                eprintln!("{} {what}: {:?}", o.label(), new.first());
            }
        }
    }
    assert!(n > 0 || c.outs.is_empty(), "no verdef mutation");
    assert!(missed.is_empty(), "NOT caught by P-D27:\n{}", missed.join("\n"));
}

// WP-149: the reference's alignment of `.gnu.version_d` is not judged
// (GNU ld's 8, lld's and mold's 4); mortar's is (4), so GNU ld's library
// judged as mortar's output is a P-D27 finding.
#[test]
fn p_d27_mortars_alignment_only() {
    let Some(c) = corpus() else { return };
    let o = c
        .outs
        .iter()
        .find(|o| o.case == "verdef" && o.link.linker == Linker::Bfd)
        .expect("GNU ld's verdef library");
    let r = o.verify_as_mortar(&o.link.output_bytes());
    assert!(
        r.findings.iter().any(|f| f.rule.id() == "P-D27" && f.message.contains("sh_addralign")),
        "{}: {:#?}",
        o.label(),
        r.findings
    );
}

/// A mortar fixture of `tests/shared/<dir>` (its `gen.sh`): the output
/// `bytes` judged under the map text `map`, the line's files read by
/// `files`.
fn mortar_fixture(dir: &Path, map: &str, bytes: &[u8], files: &dyn Files) -> Report {
    let map = mortar_plumb::map::parse(mortar_plumb::Format::Mortar, map).unwrap();
    let inputs: Vec<(String, Vec<u8>)> = map
        .paths()
        .into_iter()
        .filter_map(|p| std::fs::read(dir.join(p)).ok().map(|b| (p.to_string(), b)))
        .collect();
    let inputs: Vec<Input<'_>> = inputs.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    mortar_plumb::verify_with(bytes, &inputs, &map, &Ctx { line: None, files })
}

fn fixture_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shared").join(name)
}

// WP-149 (P-D27): mortar's versioned libraries (`tests/shared/verdef`,
// two nodes with `-soname`, three with two parents and the output's name
// as the base) are clean under their maps, and every mutation of
// `verdef_mutants` is a new P-D27 finding.
#[test]
fn p_d27_mortar_libraries() {
    let d = fixture_dir("verdef");
    let mut n = 0;
    for name in ["libvd-a64", "libvd-x64", "libvd3-a64", "libvd3-x64"] {
        let map = std::fs::read_to_string(d.join(format!("{name}.map"))).unwrap();
        let b = std::fs::read(d.join(format!("{name}.so"))).unwrap();
        let clean = keys(&mortar_fixture(&d, &map, &b, &Dir(&d)));
        assert!(clean.is_empty(), "{name}.so: {clean:#?}");
        for (what, m) in verdef_mutants(&b) {
            n += 1;
            let new = keys(&mortar_fixture(&d, &map, &m, &Dir(&d)));
            assert!(new.iter().any(|k| k.starts_with("P-D27 ")), "{name}.so {what}: {new:#?}");
        }
    }
    assert!(n > 40);
}

/// The line's files of a fixture, with `from` read as `to`.
struct Swap<'a> {
    dir: &'a Path,
    from: &'a str,
    to: &'a str,
}

impl Files for Swap<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.dir.join(if path == self.from { self.to } else { path })).ok()
    }
}

// WP-149 with WP-147 (§3.3 M10; resolve §4.7, §9.7; D-114): under
// `-shared --gc-sections` a version script's `local:` takes `gone` and
// `data_gone` out of the export set, so mortar's collection of their
// sections is clean (`tests/shared/gcver`, loc.map); the same records
// under a script that exports them (exp.map read for loc.map), or with
// no script, are M10 findings: they are roots. mortar's library under
// exp.map keeps both sections and is clean.
#[test]
fn m10_export_roots_follow_the_version_script() {
    let d = fixture_dir("gcver");
    let m10 = |r: &Report, name: &str| {
        r.findings.iter().any(|f| {
            f.rule.id() == "M10" && f.message.contains(&format!("defines '{name}', an export"))
        })
    };
    for a in ["a64", "x64"] {
        for v in ["loc", "exp"] {
            let map = std::fs::read_to_string(d.join(format!("{v}-{a}.map"))).unwrap();
            let b = std::fs::read(d.join(format!("{v}-{a}.so"))).unwrap();
            let r = keys(&mortar_fixture(&d, &map, &b, &Dir(&d)));
            assert!(r.is_empty(), "{v}-{a}.so: {r:#?}");
        }
        let map = std::fs::read_to_string(d.join(format!("loc-{a}.map"))).unwrap();
        assert!(map.contains("discard 0 5 gc") && map.contains("discard 0 6 gc"), "{map}");
        let b = std::fs::read(d.join(format!("loc-{a}.so"))).unwrap();
        let swap = Swap { dir: &d, from: "loc.map", to: "exp.map" };
        let r = mortar_fixture(&d, &map, &b, &swap);
        for name in ["gone", "data_gone"] {
            assert!(m10(&r, name), "loc-{a}.so under exp.map, {name}:\n{}", r.render("o"));
        }
        // No script: every default global is an export (WP-147's rule).
        let plain: String = map
            .lines()
            .filter(|l| {
                !matches!(*l, "version-script loc.map" | "arg --version-script" | "arg loc.map")
            })
            .fold(String::new(), |mut a, l| {
                a.push_str(l);
                a.push('\n');
                a
            });
        assert_ne!(plain, map);
        let r = mortar_fixture(&d, &plain, &b, &Dir(&d));
        for name in ["gone", "data_gone"] {
            assert!(m10(&r, name), "loc-{a}.so without a script, {name}:\n{}", r.render("o"));
        }
    }
}

/// The value of the defined `.dynsym` entry named `name` (ELF64).
fn dynsym_value(b: &[u8], name: &str) -> Option<u64> {
    let (sym, size) = section(b, ".dynsym");
    let (strs, _) = section(b, ".dynstr");
    (sym..sym + size).step_by(24).find_map(|e| {
        let at = strs + u32at(b, e) as usize;
        let shndx = u16at(b, e + 6);
        (shndx != 0 && b[at..].split(|&c| c == 0).next() == Some(name.as_bytes()))
            .then(|| u64at(b, e + 8))
    })
}

/// `map` with each run of `drop`'s lines removed, and the lines `add`
/// inserted before the map's own `arg --mortar-map=` line.
fn edit_map(map: &str, drop: &[&str], add: &[&str]) -> String {
    let lines: Vec<&str> = map.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !drop.is_empty() && lines[i..].starts_with(drop) {
            i += drop.len();
            continue;
        }
        if lines[i].starts_with("arg --mortar-map=") {
            out.extend_from_slice(add);
        }
        out.push(lines[i]);
        i += 1;
    }
    let mut s = out.join("\n");
    s.push('\n');
    assert_ne!(s, map, "edit_map changed nothing");
    s
}

// WP-161 (P-D22, P-D19, P-D7; dyn.md D-103, D-110, resolve §9.6): mortar's
// outputs of the option tail (`tests/shared/optail`) are clean under their
// maps, and each mutation of the bytes or of the map's line is a finding
// of the rule named.
#[test]
#[allow(clippy::too_many_lines)] // one mutation per rule, both targets
fn p_d_mortar_option_tail() {
    let d = fixture_dir("optail");
    let load = |name: &str, ext: &str| {
        let map = std::fs::read_to_string(d.join(format!("{name}.map"))).unwrap();
        let b = std::fs::read(d.join(format!("{name}{ext}"))).unwrap();
        (map, b)
    };
    let has = |r: &Report, rule: &str, want: &str| {
        r.findings.iter().any(|f| f.rule.id() == rule && f.message.contains(want))
    };
    let mut n = 0;
    for a in ["a64", "x64"] {
        for (name, ext) in [
            ("libz", ".so"),
            ("libex", ".so"),
            ("libdl", ".so"),
            ("e-plain", ""),
            ("e-ex", ""),
            ("d-plain", ""),
        ] {
            let name = format!("{name}-{a}");
            let (map, b) = load(&name, ext);
            let clean = keys(&mortar_fixture(&d, &map, &b, &Dir(&d)));
            assert!(clean.is_empty(), "{name}{ext}: {clean:#?}");
        }
        let mut caught = |what: String, r: Report, rule: &str, want: &str| {
            n += 1;
            assert!(has(&r, rule, want), "{what}: no {rule} '{want}' in\n{}", r.render("o"));
        };

        // libz: every DF_1 bit the -z words ask for, and DF_ORIGIN.
        let (map, b) = load(&format!("libz-{a}"), ".so");
        let f1 = dyn_value_at(&b, 0x6fff_fffb).expect("DT_FLAGS_1");
        let v1 = u64at(&b, f1);
        for bit in [0x8u64, 0x20, 0x40, 0x400, 0x80] {
            assert!(v1 & bit != 0, "libz-{a}: DF_1 {bit:#x} not set ({v1:#x})");
            let mut m = b.clone();
            m[f1..f1 + 8].copy_from_slice(&(v1 & !bit).to_le_bytes());
            let r = mortar_fixture(&d, &map, &m, &Dir(&d));
            caught(format!("libz-{a} DF_1 {bit:#x} cleared"), r, "P-D22", "DT_FLAGS_1");
        }
        let f = dyn_value_at(&b, 0x1e).expect("DT_FLAGS");
        let mut m = b.clone();
        m[f..f + 8].copy_from_slice(&(u64at(&b, f) & !1).to_le_bytes());
        let r = mortar_fixture(&d, &map, &m, &Dir(&d));
        caught(format!("libz-{a} DF_ORIGIN cleared"), r, "P-D22", "DF_ORIGIN false");
        let r = mortar_fixture(&d, &edit_map(&map, &["arg -z", "arg origin"], &[]), &b, &Dir(&d));
        caught(format!("libz-{a} without -z origin"), r, "P-D22", "-z origin false");
        // DT_INIT at the data object: the loader would call data.
        let data = dynsym_value(&b, "opt_data").expect("opt_data");
        let init = dyn_value_at(&b, 12).expect("DT_INIT");
        let mut m = b.clone();
        m[init..init + 8].copy_from_slice(&data.to_le_bytes());
        let r = mortar_fixture(&d, &map, &m, &Dir(&d));
        caught(format!("libz-{a} DT_INIT at opt_data"), r, "P-D22", "DT_INIT");
        let r = mortar_fixture(
            &d,
            &edit_map(&map, &["arg -init", "arg opt_init"], &["arg -init", "arg opt_data"]),
            &m,
            &Dir(&d),
        );
        caught(
            format!("libz-{a} -init opt_data"),
            r,
            "P-D22",
            "which is not in an executable segment",
        );

        // libex: without --exclude-libs, foo_fn (an archive member's) is an
        // export that is missing.
        let (map, b) = load(&format!("libex-{a}"), ".so");
        let r = mortar_fixture(
            &d,
            &edit_map(&map, &["arg --exclude-libs", "arg ALL"], &[]),
            &b,
            &Dir(&d),
        );
        caught(format!("libex-{a} without --exclude-libs"), r, "P-D19", "foo_fn");

        // e-plain: the same line with --exclude-libs exports nothing of
        // the archive.
        let (map, b) = load(&format!("e-plain-{a}"), "");
        let r = mortar_fixture(
            &d,
            &edit_map(&map, &[], &["arg --exclude-libs", "arg ALL"]),
            &b,
            &Dir(&d),
        );
        caught(format!("e-plain-{a} with --exclude-libs"), r, "P-D7", "--exclude-libs names");

        // e-ex: without --exclude-libs, -E exports the archive's foo_fn.
        let (map, b) = load(&format!("e-ex-{a}"), "");
        let r = mortar_fixture(
            &d,
            &edit_map(&map, &["arg --exclude-libs", "arg ALL"], &[]),
            &b,
            &Dir(&d),
        );
        caught(format!("e-ex-{a} without --exclude-libs"), r, "P-D7", "foo_fn");

        // d-plain: a dynamic list names main and foo_fn, which are not
        // exported.
        let (map, b) = load(&format!("d-plain-{a}"), "");
        let r = mortar_fixture(
            &d,
            &edit_map(&map, &[], &["arg --dynamic-list", "arg x.list"]),
            &b,
            &Dir(&d),
        );
        caught(
            format!("d-plain-{a} with x.list"),
            r,
            "P-D7",
            "listed by the line's --dynamic-list",
        );

        // libdl: without the list, -Bsymbolic binds every export.
        let (map, b) = load(&format!("libdl-{a}"), ".so");
        let plain = edit_map(
            &edit_map(&map, &["arg --dynamic-list", "arg dl.list"], &[]),
            &["dynamic-list dl.list"],
            &[],
        );
        for (rule, want) in [
            ("P-D22", "the line's symbolic form is all without --dynamic-list"),
            ("P-D25", "names 'opt_fn', which the line binds"),
            ("P-D17", "an interposer defines 'opt_fn'"),
        ] {
            let r = mortar_fixture(&d, &plain, &b, &Dir(&d));
            caught(format!("libdl-{a} without its list"), r, rule, want);
        }
    }
    assert_eq!(n, 32, "mutations");
}

/// The file offset of the virtual address `va` (ELF64, its `PT_LOAD`).
fn file_offset(b: &[u8], va: u64) -> usize {
    let (off, ent, n) = (u64at(b, 0x20) as usize, u16at(b, 0x36), u16at(b, 0x38));
    (0..n)
        .map(|i| off + i * ent)
        .filter(|&h| b[h..h + 4] == 1u32.to_le_bytes())
        .find_map(|h| {
            let (o, v, fsz) = (u64at(b, h + 8), u64at(b, h + 16), u64at(b, h + 32));
            (v..v + fsz).contains(&va).then(|| (va - v + o) as usize)
        })
        .unwrap()
}

// WP-161 (P-D9 (mortar), D-119; P-D7, AAELF64): what the `Shared` sweep
// left unjudged in WP-128's measure. Each word of a TLS descriptor
// (`libz`: `-Bsymbolic`, so `r_sym` 0) holds 0 in mortar's file; an
// export's `STO_AARCH64_VARIANT_PCS` is its definition's.
#[test]
fn p_d9_tls_descriptor_words_and_variant_pcs() {
    let d = fixture_dir("optail");
    for (a, ty) in [("a64", 1031u64), ("x64", 36)] {
        let map = std::fs::read_to_string(d.join(format!("libz-{a}.map"))).unwrap();
        let b = std::fs::read(d.join(format!("libz-{a}.so"))).unwrap();
        let (rela, size) = section(&b, ".rela.dyn");
        let place = (rela..rela + size)
            .step_by(24)
            .find(|&e| u64at(&b, e + 8) & 0xffff_ffff == ty)
            .map(|e| u64at(&b, e))
            .expect("a TLSDESC record");
        for w in 0..2 {
            let at = file_offset(&b, place + 8 * w);
            assert_eq!(u64at(&b, at), 0, "libz-{a} descriptor word {w}");
            let mut m = b.clone();
            m[at] ^= 1;
            let r = mortar_fixture(&d, &map, &m, &Dir(&d));
            assert!(
                r.findings.iter().any(|f| f.rule.id() == "P-D9"
                    && f.message.contains(&format!("word {w} of a TlsDesc record"))),
                "libz-{a} word {w}:\n{}",
                r.render("o")
            );
        }
    }
    let map = std::fs::read_to_string(d.join("libz-a64.map")).unwrap();
    let b = std::fs::read(d.join("libz-a64.so")).unwrap();
    let (sym, size) = section(&b, ".dynsym");
    let (strs, _) = section(&b, ".dynstr");
    let e = (sym..sym + size)
        .step_by(24)
        .find(|&e| {
            let at = strs + u32at(&b, e) as usize;
            b[at..].split(|&c| c == 0).next() == Some(b"opt_fn".as_slice())
        })
        .unwrap();
    let mut m = b.clone();
    m[e + 5] |= 0x80;
    let r = mortar_fixture(&d, &map, &m, &Dir(&d));
    assert!(
        r.findings
            .iter()
            .any(|f| f.rule.id() == "P-D7" && f.message.contains("STO_AARCH64_VARIANT_PCS set")),
        "{}",
        r.render("o")
    );
}
