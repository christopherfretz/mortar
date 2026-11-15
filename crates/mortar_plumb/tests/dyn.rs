//! Stage 3 on the references (`spec/plumb.md` §12, `spec/dyn.md` §12):
//! dynamic executables of GNU ld, lld 22 and mold on AArch64 and x86-64,
//! each linked through a driver whose `ld` records its words, verified
//! with the map and the line. The clean links must be clean but for the
//! recorded anomalies; each of W1-W13, H1, F2 and the dupdef image must be
//! flagged by the rule §12's table names (or, where the record says so,
//! the rule that catches the form the linker writes). Every source is in
//! `tests/dyn/` or below; every binary is built at test time.
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
    ($($n:literal),* $(,)?) => { &[$(($n, include_str!(concat!("dyn/", $n)))),*] };
}

const SOURCES: &[(&str, &str)] = src![
    "main.c",
    "tu2.c",
    "canon.c",
    "ifunc.c",
    "tls_gd.c",
    "tls_desc.c",
    "libA.c",
    "libT.c",
    "ifeq_a.c",
    "ifeq_b.c",
    "libP.c",
    "pfn.c",
    "ifh_plt.c",
    "envalias.c",
    "libenv.c",
    "textrel_lib.c",
    "textrel_ro.c",
    "canon_nopic.c",
    "canon_main.c",
    "canon_lib.c",
    "weak_main.c",
];

/// Small sources written by the test itself.
const SMALL: &[(&str, &str)] = &[
    ("hello.c", "#include <stdio.h>\nint main(void){puts(\"hi\");return 0;}\n"),
    ("libvis.c", "int vf(void){return 3;} int vg(void){return 4;}\n"),
    ("w1.c", "int vf(void); int main(void){return vf()-3;}\n"),
    (
        "libz.c",
        "__asm__(\".data\\n.globl zsym\\n.type zsym,%object\\nzsym: .byte 0x5a,1,2,3,4,5,6,7\\n\
         .globl a4sym\\n.type a4sym,%object\\n.size a4sym,4\\na4sym: .byte 9,9,9,9\\n.text\\n\");\n",
    ),
    ("w4.c", "extern char zsym[8], a4sym[4]; int main(void){return zsym[0]+a4sym[0];}\n"),
    ("lib6.c", "int dvar = 42; int fa(void){return 1;}\n"),
    (
        "w6.c",
        "extern int dvar; extern int fa(void);\nint loc = 5;\nint *const ro_ploc = &loc;\n\
         void *const ro_pfa = (void *)&fa;\n\
         int main(void){ void *const *volatile q = &ro_pfa; return dvar + *ro_ploc + (*q != 0); }\n",
    ),
    ("hmid.c", "extern void hook(void); void hmid(void){hook();}\n"),
    ("htop.c", "void hmid(void); void htop(void){hmid();}\n"),
    ("w8.c", "void htop(void); void hook(void){} int main(void){htop(); return 0;}\n"),
    ("la.c", "int fa(void){return 1;}\n"),
    ("lb.c", "int fb(void){return 1;}\n"),
    ("lc.c", "int fc(void){return 1;}\n"),
    ("ld.c", "int fd(void){return 1;}\n"),
    ("w9.c", "int fa(void), fb(void), fd(void); int main(void){return fa()+fb()+fd();}\n"),
    (
        "wm_got.c",
        "extern void f(void) __attribute__((weak));\nvoid *wm_got(void) { return (void *)&f; }\n",
    ),
    (
        "wm_abs.c",
        "extern void f(void) __attribute__((weak));\nvoid *wm_abs(void) { return (void *)&f; }\n\
         int wm_abs_call(void) { if (f) { f(); return 1; } return 0; }\n",
    ),
    (
        "wm_main.c",
        "void *wm_got(void); void *wm_abs(void); int wm_abs_call(void);\n\
         int main(void){ return wm_got() != wm_abs() + wm_abs_call(); }\n",
    ),
    (
        "libmxi.c",
        "extern int main_ifunc(void); int lib_call(void){return main_ifunc();}\n\
         void *lib_addr(void){return (void*)&main_ifunc;}\n",
    ),
    (
        "w13.c",
        "static int impl(void){return 3;}\nstatic int (*res(void))(void){return impl;}\n\
         int main_ifunc(void) __attribute__((ifunc(\"res\")));\nint lib_call(void); void *lib_addr(void);\n\
         int main(void){ return lib_call() != 3 || lib_addr() != (void*)&main_ifunc; }\n",
    ),
    ("libr.c", "int fr(void){return 1;}\n"),
    ("dupa.c", "int dv[4] = {1,1,1,1}; int fa(void){return dv[0];}\n"),
    ("dupb.c", "int dv[8] = {2,2,2,2,2,2,2,2}; int fb(void){return dv[7];}\n"),
    (
        "dup.c",
        "extern int dv[]; int fa(void), fb(void);\nint main(void){return dv[0]+dv[7]+fa()+fb();}\n",
    ),
    (
        "f2.c",
        "#include <stdio.h>\nextern void hook(void) __attribute__((weak));\n\
         int main(void) { if (hook) { hook(); puts(\"called\"); } return 0; }\n",
    ),
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
    })
}

/// One output: a link of a case, the words the linker saw, and the rules
/// the verifier must find beyond the recorded anomalies (`None`: the
/// linker refuses the link).
struct Out {
    case: &'static str,
    arch: Arch,
    link: Link,
    line: PathBuf,
    expect: Option<&'static [&'static str]>,
    crt: bool,
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
        format!("{} [{}]", self.link.label(), self.arch.name())
    }

    fn verify(&self) -> Report {
        let map = self.link.parse_map();
        let files = self.link.inputs(&map);
        let inputs: Vec<Input<'_>> =
            files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        let line: Vec<String> =
            std::fs::read_to_string(&self.line).unwrap().lines().map(str::to_string).collect();
        let ctx = Ctx { line: Some(line), files: &Dir(&self.link.dir) };
        mortar_plumb::verify_with(&self.link.output_bytes(), &inputs, &map, &ctx)
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

fn capture(program: &Path, args: &[&str]) -> String {
    let mut c = std::process::Command::new(program);
    c.args(args);
    let out = mortar_testutil::run_with(&mut c, b"", mortar_testutil::DEFAULT_TIMEOUT);
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

const CLEAN: Option<&[&str]> = Some(&[]);
/// spec/dyn.md H1: lld and mold apply `IRELATIVE` before `JUMP_SLOT`.
const H1: Option<&[&str]> = Some(&["P-D10"]);
/// The linker refuses the link.
const B: Option<&[&str]> = None;

/// The expectations of one case on one architecture: per linker (bfd,
/// lld, mold). A rule written `P-D9@view 3` must be found with a finding
/// whose message contains `view 3`.
type Expect = [Option<&'static [&'static str]>; 3];

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

    /// A shared object, linked by the driver's own GNU ld.
    fn so(&self, arch: Arch, dir: &Path, src: &str, soname: &str, out: &str, extra: &[&str]) {
        let src = self.root.join(src).display().to_string();
        let soname = format!("-Wl,-soname,{soname}");
        let mut a = vec!["-fPIC", "-O1", "-shared", &soname, &src];
        a.extend(extra);
        a.extend(["-o", out]);
        must(dir, self.cc(arch), &a);
    }

    /// Links `out` in `dir` with each linker through the recording `ld`.
    fn link(
        &mut self,
        case: &'static str,
        arch: Arch,
        dir: &Path,
        out: &str,
        args: &[&str],
        expect: Expect,
    ) {
        for (k, &linker) in Linker::ALL.iter().enumerate() {
            let name = format!("{out}.{}", linker.name());
            let map = format!("{name}.map");
            let line = dir.join(format!("{name}.line"));
            let bin = self.root.join(format!("ld-{}-{}", arch.name(), linker.name()));
            let b = format!("-B{}/", bin.display());
            let m = format!("-Wl,-Map={map}");
            let mut a: Vec<&str> = vec![&b];
            a.extend(args);
            a.extend(["-o", &name, &m]);
            let linked = run_in(dir, self.cc(arch), &a, Some(&line));
            self.push(case, arch, dir, &name, linked, expect[k], linker, line, true);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        case: &'static str,
        arch: Arch,
        dir: &Path,
        name: &str,
        linked: bool,
        expect: Option<&'static [&'static str]>,
        linker: Linker,
        line: PathBuf,
        crt: bool,
    ) {
        let label = format!("{case}/{name}");
        let Some(expect) = expect else {
            assert!(!linked, "{label}: the link was expected to be refused");
            return;
        };
        assert!(linked, "{label}: the link failed");
        self.outs.push(Out {
            case,
            arch,
            link: Link {
                case,
                linker,
                dir: dir.to_path_buf(),
                output: dir.join(name),
                map: dir.join(format!("{name}.map")),
            },
            line,
            expect: Some(expect),
            crt,
        });
    }

    /// A copy of a finished output under a new name, its bytes changed by
    /// `patch`, judged with `map` and `line` of another link.
    #[allow(clippy::too_many_arguments)]
    fn patched(
        &mut self,
        case: &'static str,
        arch: Arch,
        dir: &Path,
        from: &str,
        name: &str,
        map: &str,
        line: &str,
        linker: Linker,
        patch: fn(&mut [u8]),
        expect: &'static [&'static str],
    ) {
        let mut b = std::fs::read(dir.join(from)).unwrap();
        patch(&mut b);
        std::fs::write(dir.join(name), &b).unwrap();
        std::fs::copy(dir.join(map), dir.join(format!("{name}.map"))).unwrap();
        self.push(case, arch, dir, name, true, Some(expect), linker, dir.join(line), true);
    }
}

#[allow(clippy::too_many_lines)] // one block per case
fn build() -> Option<Corpus> {
    let t = tools()?;
    let scratch = Scratch::new("plumb-dyn");
    let root = scratch.path().to_path_buf();
    for (n, text) in SOURCES.iter().chain(SMALL) {
        std::fs::write(root.join(n), text).unwrap();
    }
    // Each linker as a recording `ld` in a directory of its own.
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
        let x64 = arch == Arch::X64;
        let a = |r: [Option<&'static [&'static str]>; 3],
                 x: [Option<&'static [&'static str]>; 3]| {
            if x64 { x } else { r }
        };

        // Clean: a hello, PIE and not, lazy and now.
        let d = b.dir(arch, "hello");
        b.obj(arch, &d, "hello.c", &["-O1", "-fPIE"], "h.o");
        for (pm, z) in [("-pie", "lazy"), ("-pie", "now"), ("-no-pie", "lazy"), ("-no-pie", "now")]
        {
            let out = format!("hello{pm}.{z}");
            let zz = format!("-Wl,-z,{z}");
            b.link("hello", arch, &d, &out, &[pm, &zz, "h.o"], [CLEAN; 3]);
        }

        // E1: copies, canonical PLTs, TLS from a library and the
        // executable, an IFUNC; every record type of the matrix.
        let d = b.dir(arch, "e1");
        b.so(arch, &d, "libA.c", "libA.so", "libA.so", &[]);
        b.so(arch, &d, "libT.c", "libT.so", "libT.so", &[]);
        let (gd, desc) = if x64 {
            ("-mtls-dialect=gnu", "-mtls-dialect=gnu2")
        } else {
            ("-mtls-dialect=desc", "-mtls-dialect=desc")
        };
        b.obj(arch, &d, "tls_gd.c", &["-fPIC", "-O1", gd], "tls_gd.o");
        b.obj(arch, &d, "tls_desc.c", &["-fPIC", "-O1", desc], "tls_desc.o");
        for (pm, pic) in [("pie", "-fPIE"), ("nopie", "-fno-pic")] {
            for s in ["main", "tu2", "canon", "ifunc"] {
                b.obj(arch, &d, &format!("{s}.c"), &[pic, "-O1"], &format!("{s}.{pm}.o"));
            }
            let objs: Vec<String> =
                ["main", "tu2", "canon", "ifunc"].iter().map(|s| format!("{s}.{pm}.o")).collect();
            for z in ["lazy", "now"] {
                let out = format!("e1.{pm}.{z}");
                let flag = if pm == "pie" { "-pie" } else { "-no-pie" };
                let zz = format!("-Wl,-z,{z}");
                let mut args = vec![flag, zz.as_str()];
                args.extend(objs.iter().map(String::as_str));
                args.extend(["tls_gd.o", "tls_desc.o", "-L.", "-lA", "-lT"]);
                // spec/dyn.md W3 (x86-64 PIE: GNU ld and mold load the
                // IFUNC's GOT slot, IRELATIVE, where the other uses take
                // the IPLT entry: F3), H1 (lld, mold: P-D10).
                let e = if x64 && pm == "pie" {
                    [Some(&["F3"][..]), H1, Some(&["F3", "P-D10"][..])]
                } else {
                    [CLEAN, H1, H1]
                };
                b.link("e1", arch, &d, &out, &args, e);
            }
        }

        // W1: an import whose only definition is STV_HIDDEN.
        let d = b.dir(arch, "W1");
        b.so(arch, &d, "libvis.c", "libvis.so", "libvis.so", &[]);
        set_dynsym_visibility(&d.join("libvis.so"), "vf", 2);
        b.obj(arch, &d, "w1.c", &["-O1"], "w1.o");
        let w1: Option<&[&str]> = Some(&["P-D7", "P-D9"]);
        b.link("W1", arch, &d, "w1", &["-Wl,--no-as-needed", "w1.o", "libvis.so"], [w1; 3]);

        // W2: a copied object whose alias is left behind (GNU ld).
        let d = b.dir(arch, "W2");
        b.so(arch, &d, "libenv.c", "libenv.so", "libenv.so", &[]);
        b.obj(arch, &d, "envalias.c", &["-O1", "-fno-pie"], "e.o");
        b.link(
            "W2",
            arch,
            &d,
            "w2",
            &["-no-pie", "e.o", "libenv.so"],
            [Some(&["P-D11", "P-D7"]), CLEAN, CLEAN],
        );

        // W3: an IFUNC's address taken in two units.
        let d = b.dir(arch, "W3");
        for (pm, pic, flag) in [("pie", "-fPIE", "-pie"), ("nopie", "-fno-pic", "-no-pie")] {
            b.obj(arch, &d, "ifeq_a.c", &[pic, "-O1"], &format!("a.{pm}.o"));
            b.obj(arch, &d, "ifeq_b.c", &[pic, "-O1"], &format!("b.{pm}.o"));
            let e = if x64 && pm == "pie" {
                [Some(&["F3"][..]), H1, Some(&["F3", "P-D10"][..])]
            } else {
                [CLEAN, H1, H1]
            };
            let (oa, ob) = (format!("a.{pm}.o"), format!("b.{pm}.o"));
            b.link("W3", arch, &d, &format!("w3.{pm}"), &[flag, &oa, &ob], e);
        }
        if !x64 {
            // The AArch64 form: one unit not PIC in a PIE.
            b.obj(arch, &d, "ifeq_b.c", &["-fno-pic", "-O1"], "b.abs.o");
            b.link(
                "W3",
                arch,
                &d,
                "w3.mixed",
                &["-pie", "a.pie.o", "b.abs.o"],
                [Some(&["F3"]), H1, Some(&["F3", "P-D10"])],
            );
        }

        // W4: copies of an object without a size, and its neighbour.
        let d = b.dir(arch, "W4");
        b.so(arch, &d, "libz.c", "libz.so", "libz.so", &[]);
        b.obj(arch, &d, "w4.c", &["-fno-pic", "-O1"], "z.o");
        let w4: Option<&[&str]> = Some(&["P-D11"]);
        b.link("W4", arch, &d, "w4", &["-no-pie", "z.o", "libz.so"], [w4, B, w4]);

        // W5: a canonical PLT of a protected function.
        let d = b.dir(arch, "W5");
        b.so(arch, &d, "libP.c", "libP.so", "libP.so", &[]);
        b.obj(arch, &d, "pfn.c", &["-fno-pic", "-O1"], "pfn.o");
        b.link("W5", arch, &d, "w5", &["-no-pie", "pfn.o", "libP.so"], [B, B, Some(&["P-D13"])]);

        // W6: text relocations from a unit not PIC in a PIE.
        let d = b.dir(arch, "W6");
        b.so(arch, &d, "lib6.c", "libA.so", "libA.so", &[]);
        b.obj(arch, &d, "w6.c", &["-fno-pic", "-O1"], "t.o");
        b.link(
            "W6",
            arch,
            &d,
            "w6",
            &["-pie", "t.o", "libA.so"],
            a([B, B, Some(&["P-D9", "P-D6"])], [B; 3]),
        );

        // W7: an import's address in a read-only section of a PIE.
        let d = b.dir(arch, "W7");
        b.so(arch, &d, "textrel_lib.c", "libtextrel.so", "libtextrel.so", &[]);
        b.obj(arch, &d, "textrel_ro.c", &["-fPIE", "-O1"], "tr.o");
        let tr: Option<&[&str]> = Some(&["P-D9", "P-D6"]);
        b.link(
            "W7",
            arch,
            &d,
            "w7",
            &["-pie", "tr.o", "libtextrel.so"],
            [tr, Some(&["P-D9@view 2"]), tr],
        );

        // W8: a definition a library references, not exported.
        let d = b.dir(arch, "W8");
        std::fs::create_dir_all(d.join("mid")).unwrap();
        std::fs::create_dir_all(d.join("top")).unwrap();
        b.so(arch, &d, "hmid.c", "libhmid.so", "mid/libhmid.so", &[]);
        b.so(arch, &d, "htop.c", "libhtop.so", "top/libhtop.so", &["-Lmid", "-lhmid"]);
        b.obj(arch, &d, "w8.c", &["-O1"], "m.o");
        let w8: Option<&[&str]> = Some(&["P-D7"]);
        b.link(
            "W8",
            arch,
            &d,
            "w8",
            &["-Wl,--no-as-needed", "m.o", "-Ltop", "-lhtop", "-Wl,-rpath-link,mid"],
            [CLEAN, w8, w8],
        );

        // W9: a library --as-needed drops and a later mention needs.
        let d = b.dir(arch, "W9");
        for x in ["a", "b", "c", "d"] {
            b.so(arch, &d, &format!("l{x}.c"), &format!("libn{x}.so"), &format!("libn{x}.so"), &[]);
        }
        b.obj(arch, &d, "w9.c", &["-O1"], "n.o");
        b.link(
            "W9",
            arch,
            &d,
            "w9",
            &[
                "n.o",
                "libna.so",
                "-Wl,--as-needed",
                "libnc.so",
                "-Wl,--no-as-needed",
                "libnb.so",
                "libnc.so",
                "libnd.so",
            ],
            [CLEAN, CLEAN, Some(&["P-D6"])],
        );

        // W10: the linker's own interpreter (no -dynamic-linker).
        let d = b.dir(arch, "W10");
        b.obj(arch, &d, "hello.c", &["-fPIE", "-O1"], "h.o");
        let f = |n: &str| capture(&t.cc[arch as usize], &[&format!("-print-file-name={n}")]);
        let libdir = Path::new(&f("libc.so")).parent().unwrap().display().to_string();
        let crt = [f("Scrt1.o"), f("crti.o"), f("crtbeginS.o"), f("crtendS.o"), f("crtn.o")];
        let (emu, interp) = if x64 {
            ("elf_x86_64", "/lib64/ld-linux-x86-64.so.2")
        } else {
            ("aarch64linux", "/lib/ld-linux-aarch64.so.1")
        };
        for (k, (linker, real)) in
            Linker::ALL.iter().zip([&t.bfd[arch as usize], &t.lld, &t.mold]).enumerate()
        {
            for dl in [false, true] {
                let name = format!("w10.{}.{}", if dl { "dl" } else { "nodl" }, linker.name());
                let map = format!("-Map={name}.map");
                let lib = format!("-L{libdir}");
                let mut args: Vec<&str> = vec!["-m", emu, "-pie"];
                if dl {
                    args.extend(["-dynamic-linker", interp]);
                }
                args.extend([
                    "-o", &name, &crt[0], &crt[1], &crt[2], "h.o", &lib, "-lc", &crt[3], &crt[4],
                    &map,
                ]);
                let line = d.join(format!("{name}.line"));
                std::fs::write(&line, args.join("\n") + "\n").unwrap();
                let linked = run_in(&d, real, &args, None);
                let e: Option<&'static [&'static str]> = if dl {
                    CLEAN
                } else if k == 0 {
                    Some(&["P-D4"])
                } else {
                    Some(&["P-D1", "P-D4"])
                };
                b.push("W10", arch, &d, &name, linked, e, *linker, line, true);
            }
        }

        // W11: a canonical PLT in a PIE from a unit not PIC.
        let d = b.dir(arch, "W11");
        b.so(arch, &d, "canon_lib.c", "liblibp.so", "liblibp.so", &[]);
        b.obj(arch, &d, "canon_nopic.c", &["-O2", "-fno-pic"], "nopic.o");
        b.obj(arch, &d, "canon_main.c", &["-O2"], "main.o");
        b.link(
            "W11",
            arch,
            &d,
            "w11",
            &["main.o", "nopic.o", "liblibp.so"],
            a([B, CLEAN, CLEAN], [B, Some(&["P-D9@view 2"]), B]),
        );

        // W12: an undefined weak resolved to 0 in a field no record moves.
        let d = b.dir(arch, "W12");
        let abs: &[&str] =
            if x64 { &["-O2", "-fno-pic"] } else { &["-O2", "-fno-pic", "-mcmodel=large"] };
        b.obj(arch, &d, "wm_got.c", &["-O2", "-fPIE"], "g.o");
        b.obj(arch, &d, "wm_abs.c", abs, "b.o");
        b.obj(arch, &d, "wm_main.c", &["-O2"], "m.o");
        b.obj(arch, &d, "weak_main.c", &["-O2", "-fPIE"], "s.o");
        let v3: Option<&[&str]> = Some(&["P-D9@view 3"]);
        b.link(
            "W12",
            arch,
            &d,
            "w12",
            &["-no-pie", "m.o", "g.o", "b.o"],
            a([v3, v3, CLEAN], [CLEAN, v3, CLEAN]),
        );
        b.link("W12", arch, &d, "w12s", &["-no-pie", "s.o"], [v3, v3, CLEAN]);

        // W13: the executable's IFUNC referenced by a library.
        let d = b.dir(arch, "W13");
        b.so(arch, &d, "libmxi.c", "libmxi.so", "libmxi.so", &[]);
        for (pm, pic, flag) in [("pie", "-fPIE", "-pie"), ("nopie", "-fno-pic", "-no-pie")] {
            let o = format!("m.{pm}.o");
            b.obj(arch, &d, "w13.c", &[pic, "-O2"], &o);
            let e = match (x64, pm) {
                (true, "pie") => [Some(&["P-D7"][..]), H1, Some(&["P-D7", "P-D10"][..])],
                (false, "nopie") => [Some(&["P-D9"][..]), H1, H1],
                _ => [CLEAN, H1, H1],
            };
            b.link("W13", arch, &d, &format!("w13.{pm}"), &[flag, &o, "libmxi.so"], e);
        }

        // H1: a resolver that calls through the PLT.
        let d = b.dir(arch, "H1");
        b.so(arch, &d, "libr.c", "libr.so", "libr.so", &[]);
        for (pm, pic, flag) in [("pie", "-fPIE", "-pie"), ("nopie", "-fno-pic", "-no-pie")] {
            let o = format!("i.{pm}.o");
            b.obj(arch, &d, "ifh_plt.c", &[pic, "-O1"], &o);
            b.link(
                "H1",
                arch,
                &d,
                &format!("h1.{pm}"),
                &[flag, &o, "-Wl,--no-as-needed", "libr.so"],
                [CLEAN, H1, H1],
            );
        }

        // dupdef: two libraries define `dv`; the image's DT_NEEDED order
        // is the line's (nB first) while the copy is sized from nA's.
        let d = b.dir(arch, "DUP");
        std::fs::create_dir_all(d.join("l")).unwrap();
        b.so(arch, &d, "dupa.c", "libnA.so", "l/libnA.so", &[]);
        b.so(arch, &d, "dupb.c", "libnB.so", "l/libnB.so", &[]);
        b.obj(arch, &d, "dup.c", &["-O1", "-fno-pic"], "m.o");
        let dup_line = [
            "-no-pie",
            "-Ll",
            "-Wl,--as-needed",
            "-lnA",
            "-Wl,--no-as-needed",
            "-lnB",
            "m.o",
            "-lnA",
        ];
        b.link("DUP", arch, &d, "dup", &dup_line, [CLEAN; 3]);
        b.link(
            "DUP",
            arch,
            &d,
            "dupsrc",
            &["-no-pie", "-Ll", "m.o", "-Wl,--no-as-needed", "-lnA", "-lnB"],
            [CLEAN; 3],
        );
        b.patched(
            "DUP",
            arch,
            &d,
            "dupsrc.bfd",
            "dupdef.bfd",
            "dupsrc.bfd.map",
            "dup.bfd.line",
            Linker::Bfd,
            swap_needed,
            &["P-D9", "P-D11"],
        );

        // F2: lld's no-PIE image of a guarded weak call with the call to
        // the PLT entry removed (the guard reads the GOT word: view 3).
        if !x64 {
            let d = b.dir(arch, "F2");
            b.obj(arch, &d, "f2.c", &["-O2"], "f2.o");
            b.link("F2", arch, &d, "f2", &["-no-pie", "f2.o"], [CLEAN; 3]);
            b.patched(
                "F2",
                arch,
                &d,
                "f2.lld",
                "f2img.lld",
                "f2.lld.map",
                "f2.lld.line",
                Linker::Lld,
                nop_hook_call,
                &["P-D9@view 3"],
            );
        }
    }
    Some(Corpus { _scratch: scratch, outs: b.outs })
}

// ---- ELF64 little-endian edits ----

fn le16(b: &[u8], o: usize) -> usize {
    u16::from_le_bytes([b[o], b[o + 1]]) as usize
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// (name, type, addr, offset, size, link) of each section.
fn sections(b: &[u8]) -> Vec<(String, u32, u64, usize, usize, usize)> {
    let (shoff, shentsize, shnum, shstrndx) =
        (le64(b, 0x28) as usize, le16(b, 0x3a), le16(b, 0x3c), le16(b, 0x3e));
    let raw: Vec<_> = (0..shnum)
        .map(|i| {
            let o = shoff + i * shentsize;
            (
                le32(b, o),
                le32(b, o + 4),
                le64(b, o + 0x10),
                le64(b, o + 0x18) as usize,
                le64(b, o + 0x20) as usize,
                le32(b, o + 0x28) as usize,
            )
        })
        .collect();
    let strs = raw[shstrndx].3;
    raw.iter()
        .map(|&(n, ty, addr, off, size, link)| {
            (cstr(b, strs + n as usize), ty, addr, off, size, link)
        })
        .collect()
}

fn cstr(b: &[u8], o: usize) -> String {
    let end = b[o..].iter().position(|&c| c == 0).unwrap();
    String::from_utf8_lossy(&b[o..o + end]).into_owned()
}

fn section<'s>(
    s: &'s [(String, u32, u64, usize, usize, usize)],
    name: &str,
) -> &'s (String, u32, u64, usize, usize, usize) {
    s.iter().find(|x| x.0 == name).unwrap_or_else(|| panic!("no section {name}"))
}

/// Sets the visibility of `name`'s `.dynsym` entry.
fn set_dynsym_visibility(path: &Path, name: &str, vis: u8) {
    let mut b = std::fs::read(path).unwrap();
    let s = sections(&b);
    let dynsym = section(&s, ".dynsym").clone();
    let strs = s[dynsym.5].3;
    let mut n = 0;
    for o in (dynsym.3..dynsym.3 + dynsym.4).step_by(24) {
        if cstr(&b, strs + le32(&b, o) as usize) == name {
            b[o + 5] = (b[o + 5] & !3) | vis;
            n += 1;
        }
    }
    assert_eq!(n, 1, "{name} in {}", path.display());
    std::fs::write(path, b).unwrap();
}

/// Swaps the first two `DT_NEEDED` entries.
fn swap_needed(b: &mut [u8]) {
    let s = sections(b);
    let d = section(&s, ".dynamic");
    let needed: Vec<usize> = (d.3..d.3 + d.4).step_by(16).filter(|&o| le64(b, o) == 1).collect();
    assert!(needed.len() >= 2, "fewer than two DT_NEEDED");
    let (x, y) = (le64(b, needed[0] + 8), le64(b, needed[1] + 8));
    b[needed[0] + 8..needed[0] + 16].copy_from_slice(&y.to_le_bytes());
    b[needed[1] + 8..needed[1] + 16].copy_from_slice(&x.to_le_bytes());
}

/// Replaces the `BL` to `hook`'s PLT entry by a `NOP` (lld's AArch64 PLT:
/// a 32-byte header, then 16 bytes per `.rela.plt` record, in order).
fn nop_hook_call(b: &mut [u8]) {
    let s = sections(b);
    let rela = section(&s, ".rela.plt");
    let dynsym = &s[rela.5];
    let strs = s[dynsym.5].3;
    let idx = (0..rela.4 / 24)
        .find(|i| {
            let sym = (le64(b, rela.3 + i * 24 + 8) >> 32) as usize;
            cstr(b, strs + le32(b, dynsym.3 + sym * 24) as usize) == "hook"
        })
        .expect("a JUMP_SLOT of hook");
    let entry = section(&s, ".plt").2 + 32 + 16 * idx as u64;
    let text = section(&s, ".text").clone();
    let mut n = 0;
    for o in (text.3..text.3 + text.4).step_by(4) {
        let w = le32(b, o);
        if w & 0xfc00_0000 == 0x9400_0000 {
            let imm = i64::from(((w & 0x03ff_ffff) << 6) as i32 >> 6) * 4;
            let pc = text.2 + (o - text.3) as u64;
            if pc.wrapping_add_signed(imm) == entry {
                b[o..o + 4].copy_from_slice(&0xd503_201f_u32.to_le_bytes());
                n += 1;
            }
        }
    }
    assert_eq!(n, 1, "the calls to hook@plt");
}

// ---- verdicts ----

/// The anomalies a linker's dynamic outputs show in every link
/// (spec/plumb.md §10.2), allowed but not required.
fn defaults(linker: Linker) -> &'static [&'static str] {
    match linker {
        Linker::Mold => &["A1", "A2", "A3", "A9", "A13"],
        Linker::Bfd => &["A10"],
        Linker::Lld => &[],
    }
}

fn judge(case: &str) {
    let Some(c) = corpus() else { return };
    let mut failures = Vec::new();
    let mut n = 0;
    for o in c.outs.iter().filter(|o| o.case == case) {
        n += 1;
        let r = o.verify();
        let mut found = BTreeSet::new();
        for f in &r.findings {
            match known_anomaly(&o.link, f) {
                Some(a) => found.insert(mortar_plumb::anomaly::id(a).to_string()),
                None => found.insert(f.rule.id()),
            };
        }
        let expect = o.expect.unwrap_or(&[]);
        let rule = |e: &str| e.split_once('@').map_or(e, |(r, _)| r).to_string();
        let allowed: BTreeSet<String> = expect
            .iter()
            .map(|e| rule(e))
            .chain(defaults(o.link.linker).iter().filter(|_| o.crt).map(|s| (*s).to_string()))
            .collect();
        let missing: Vec<&&str> = expect
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
                    .take(6)
                    .map(|f| format!("  {} {}", f.rule.id(), f.message))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        assert!(r.coverage.records > 0, "{}: no records", o.label());
    }
    assert!(n > 0 || c.outs.is_empty(), "no outputs of case {case}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// spec/plumb.md §12: the clean references are clean.
#[test]
fn s12_clean_dynamic_links_verify_clean() {
    judge("hello");
}

// spec/plumb.md §12: the E1 matrix: every record type each linker writes.
#[test]
fn s12_e1_matrix_is_judged_with_only_its_hazards() {
    judge("e1");
}

#[test]
fn s12_w1_hidden_import_is_p_d7() {
    judge("W1");
}

#[test]
fn s12_w2_copy_alias_is_p_d11() {
    judge("W2");
}

#[test]
fn s12_w3_ifunc_address_equality() {
    judge("W3");
}

#[test]
fn s12_w4_sizeless_copy_is_p_d11() {
    judge("W4");
}

#[test]
fn s12_w5_protected_canonical_plt_is_p_d13() {
    judge("W5");
}

#[test]
fn s12_w6_text_relocations() {
    judge("W6");
}

#[test]
fn s12_w7_read_only_import_address() {
    judge("W7");
}

#[test]
fn s12_w8_unexported_definition_is_p_d7() {
    judge("W8");
}

#[test]
fn s12_w9_as_needed_needed_is_p_d6() {
    judge("W9");
}

#[test]
fn s12_w10_linker_default_interpreter_is_p_d4() {
    judge("W10");
}

#[test]
fn s12_w11_canonical_plt_in_a_pie() {
    judge("W11");
}

#[test]
fn s12_w12_undefined_weak_is_p_d9_view_3() {
    judge("W12");
}

#[test]
fn s12_w13_exported_ifunc() {
    judge("W13");
}

#[test]
fn s12_h1_irelative_before_jump_slot_is_p_d10() {
    judge("H1");
}

#[test]
fn s12_dupdef_is_p_d9() {
    judge("DUP");
}

#[test]
fn s12_f2_is_p_d9_view_3() {
    judge("F2");
}
