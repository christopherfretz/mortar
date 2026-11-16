//! Stage 3 on the references (`spec/plumb.md` §12):
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
    "tlsdesc_regs.s",
    "libtx.c",
    "gd_ie.c",
    "libgdie.c",
    "unique.h",
    "libunique.cc",
    "unique_main.cc",
];

/// Small sources written by the test itself.
const SMALL: &[(&str, &str)] = &[
    ("hello.c", "#include <stdio.h>\nint main(void){puts(\"hi\");return 0;}\n"),
    // AArch64's large-model local-exec sequence against the executable's
    // own TLS variable, defined in another file (WP-082 follow-up 4).
    ("tlsle_u.c", "extern __thread int gv;\nint main(void){return gv - 5;}\n"),
    ("tlsle_d.c", "__thread int gv = 5;\n"),
    // P-D9's non-address rule in a Pie (WP-088): `rx_v` holds a number,
    // `rx_p` its address.
    (
        "relx.c",
        "long rx_v = 12345; long *rx_p = &rx_v;\nint main(void){return (int)(*rx_p - 12345);}\n",
    ),
    // x86-64 PC-relative code to an absolute symbol (WP-088): `absv` is
    // `SHN_ABS` 0x1234; `main` calls it.
    ("abspc_a.s", "\t.globl absv\n\t.set absv, 0x1234\n"),
    (
        "abspc_c.s",
        "\t.text\n\t.globl main\nmain:\tlea 1f(%rip), %rdi\n\tcall absv@PLT\n1:\tret\n\t.section .note.GNU-stack,\"\",@progbits\n",
    ),
    (
        "abspc_g.s",
        "\t.text\n\t.globl main\nmain:\tmovq absv@GOTPCREL(%rip), %rax\n\tcmp $0x1234, %rax\n\tsetne %al\n\tmovzbl %al, %eax\n\tret\n\t.section .note.GNU-stack,\"\",@progbits\n",
    ),
    // dyn.md W17 (WP-088): general-dynamic access to the executable's own
    // TLS, x86-64 large code model.
    ("gd17.c", "__thread int own = 7;\nint main(void){return own - 7;}\n"),
    // P-D4's PT_GNU_STACK (WP-088): an object whose stack note asks for
    // an executable stack.
    (
        "xstack.s",
        "\t.text\n\t.globl xs_f\nxs_f:\tnop\n\t.section .note.GNU-stack,\"x\",%progbits\n",
    ),
    // W13 (WP-088): an exported IFUNC the program never references.
    (
        "ifx.c",
        "static int ifx_impl(void){return 1;}\nstatic void *ifx_resolve(void){return (void *)ifx_impl;}\nint ifx(void) __attribute__((ifunc(\"ifx_resolve\")));\nint main(void){return 0;}\n",
    ),
    // dyn.md W14 (WP-088): a non-TLS reference to a shared TLS definition.
    ("libtv.c", "__thread int tv = 4; int get_tv(void){return tv;}\n"),
    ("w14.c", "extern int tv; int get_tv(void);\nint main(void){return tv + get_tv() - 8;}\n"),
    // dyn.md W16 (WP-088): a used object's reference no member of N ∪ U
    // satisfies (D-13).
    ("libleaf.c", "int leaf_fn(void){return 7;}\n"),
    ("libmid2.c", "int leaf_fn(void); int mid2_fn(void){return 100 + leaf_fn();}\n"),
    ("libtop2.c", "int mid2_fn(void); int top2_fn(void){return 1000 + mid2_fn();}\n"),
    ("w16.c", "int top2_fn(void); int main(void){return top2_fn() - 1107;}\n"),
    ("libvis.c", "int vf(void){return 3;} int vg(void){return 4;}\n"),
    ("w1.c", "int vf(void); int main(void){return vf()-3;}\n"),
    (
        "libz.c",
        "__asm__(\".data\\n.globl zsym\\n.type zsym,%object\\nzsym: .byte 0x5a,1,2,3,4,5,6,7\\n\
         .globl a4sym\\n.type a4sym,%object\\n.size a4sym,4\\na4sym: .byte 9,9,9,9\\n.text\\n\");\n",
    ),
    ("w4.c", "extern char zsym[8], a4sym[4]; int main(void){return zsym[0]+a4sym[0];}\n"),
    ("lib6.c", "int dvar = 42; int fa(void){return 1;}\n"),
    // P-D11 under -z norelro (WP-088): a copy of a read-only datum (the
    // library's `.data.rel.ro`, inside its PT_GNU_RELRO) and of a
    // writable one.
    ("libro.c", "int rw_v = 3; static int x = 5; int *const ro_p = &x;\n"),
    (
        "ro.c",
        "extern int *const ro_p; extern int rw_v;\nint main(void){return *ro_p + rw_v - 8;}\n",
    ),
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
const H1: Option<&[&str]> = Some(&["P-D10", "A20"]);
/// The linker refuses the link.
const B: Option<&[&str]> = None;
/// lld leaves a `RELATIVE` word 0 in the file (no `--apply-dynamic-relocs`),
/// so `rx_p` without its record also holds the wrong value: R1, measured.
const LLD_RELX: &[&str] =
    &["P-D9@that no input relocation makes an address of the executable", "R1"];
/// mold's `call 0x1234` in a PIE (plumb.md §5.1a).
const A35: Option<&[&str]> = Some(&["R1@to an absolute symbol", "A35"]);
/// dyn.md W14's outputs (lld, mold).
const W14: Option<&[&str]> = Some(&["P-D12@whose shared definition is STT_TLS"]);
/// mold's x86-64 W14 output: a `COPY` of the `STT_TLS` definition (a PIE's
/// `PC32` to it; lld refuses that link).
const W14_COPY: Option<&[&str]> = Some(&["S8@TLS symbol 'tv' and no PT_TLS"]);
/// mold's `-no-pie -E` export of an unreferenced IFUNC, outside every
/// section.
const MOLD_IFX: Option<&[&str]> = Some(&[
    "S8@is outside its section",
    "Y1@is not STT_GNU_IFUNC at its resolver",
    "P-D7@which does not hold it",
]);
/// dyn.md W16's outputs (lld, mold).
const W16: Option<&[&str]> = Some(&["P-D6@(D-13: the link is refused)", "A36"]);
/// lld's x86-64 large-model GD outputs (dyn.md W17).
const A12: Option<&[&str]> = Some(&["A12"]);

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

    /// [`Builder::link`] with the linker `k` of `Linker::ALL` only.
    #[allow(clippy::too_many_arguments)]
    fn link_one(
        &mut self,
        case: &'static str,
        arch: Arch,
        dir: &Path,
        out: &str,
        args: &[&str],
        k: usize,
        expect: Option<&'static [&'static str]>,
    ) {
        let linker = Linker::ALL[k];
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
        self.push(case, arch, dir, &name, linked, expect, linker, line, true);
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

        // P-D11 (WP-088): copies with and without RELRO. Without it
        // every reference copies the read-only datum into ordinary
        // writable memory (GNU ld `.data.rel.ro`, lld `.bss.rel.ro`, mold
        // `.copyrel`), which P-D11 accepts; with it, inside RELRO.
        let dr = b.dir(arch, "copy-relro");
        b.so(arch, &dr, "libro.c", "libro.so", "libro.so", &[]);
        b.obj(arch, &dr, "ro.c", &["-O1", "-fno-pic"], "ro.o");
        for z in ["relro", "norelro"] {
            let zz = format!("-Wl,-z,{z}");
            let out = format!("ro.{z}");
            let args = ["-no-pie", zz.as_str(), "ro.o", "libro.so"];
            b.link("copy-relro", arch, &dr, &out, &args, [CLEAN; 3]);
        }

        // P-D10 (WP-082): the PLT header reads the wrong word. x86-64:
        // PLT0's `jmp *GOT+16(%rip)` displacement (at `.plt + 8`) lowered
        // by 0x1030 (the x86-64 join's wrong output: every lazy call
        // crashed, stage 3 said nothing); AArch64: the header's `ldr x17`
        // offset moved one word. Lazy and `-z now` (D-49: the shapes stay).
        for z in ["lazy", "now"] {
            for linker in Linker::ALL {
                let from = format!("hello-pie.{z}.{}", linker.name());
                let name = format!("plt0-{z}.{}", linker.name());
                let patch: fn(&mut [u8]) =
                    if x64 { plt0_jump_lowered } else { a64_header_ldr_moved };
                b.patched(
                    "PLT0",
                    arch,
                    &d,
                    &from,
                    &name,
                    &format!("{from}.map"),
                    &format!("{from}.line"),
                    linker,
                    patch,
                    &["P-D10"],
                );
            }
        }

        // The PLT shapes of IBT and BTI links (WP-082: P-D10 judges them):
        // GNU ld's `.plt.sec` with `endbr64` lazy stubs (`-z ibtplt`), lld's
        // (`-z force-ibt`: the crt objects have no IBT property, P1), the
        // BTI header and entries of GNU ld and lld (`-z force-bti`).
        let d = b.dir(arch, "shapes");
        let (prot, zs): (&str, [Option<&str>; 3]) = if x64 {
            ("-fcf-protection=full", [Some("-Wl,-z,ibtplt"), Some("-Wl,-z,force-ibt"), None])
        } else {
            (
                "-mbranch-protection=standard",
                [Some("-Wl,-z,force-bti"), Some("-Wl,-z,force-bti"), None],
            )
        };
        b.obj(arch, &d, "hello.c", &["-O1", "-fPIE", prot], "h.o");
        for z in ["lazy", "now"] {
            let zz = format!("-Wl,-z,{z}");
            for (k, flag) in zs.iter().enumerate() {
                let Some(flag) = flag else { continue };
                let e = if x64 && k == 1 { Some(&["P1"][..]) } else { CLEAN };
                let out = format!("shapes.{z}");
                b.link_one("shapes", arch, &d, &out, &["-pie", &zz, flag, "h.o"], k, e);
            }
        }

        // E6 (WP-088, WP-082 follow-up 1): GNU ld's x86-64 PLT FDEs
        // (`.plt`, `.plt.got`, `.plt.sec`), judged against the PLT's
        // stack at each instruction: the lazy PIE, the `-z now` no-PIE
        // and the IBT shape, each with its `.plt` FDE's range run past
        // the section, the expression's `lit11`/`lit9` raised by one (the
        // CFA at each entry's `jmp PLT0` 8 short) and PLT0's
        // `def_cfa_offset 24` lowered to 16.
        if x64 {
            let patches: [Patch; 3] = [
                ("range", plt_fde_range_past),
                ("lit", plt_fde_lit_raised),
                ("offset", plt_fde_offset_lowered),
            ];
            for (dir, from) in [
                ("hello", "hello-pie.lazy"),
                ("hello", "hello-no-pie.now"),
                ("shapes", "shapes.lazy"),
            ] {
                let d = b.dir(arch, dir);
                let from = format!("{from}.bfd");
                for (what, patch) in patches {
                    b.patched(
                        "plt-fde",
                        arch,
                        &d,
                        &from,
                        &format!("{from}.fde-{what}"),
                        &format!("{from}.map"),
                        &format!("{from}.line"),
                        Linker::Bfd,
                        patch,
                        &["E6"],
                    );
                }
            }
        }

        // P-D4, PT_GNU_STACK's PF_X (WP-088): `-z execstack` (all three
        // set it), `-z noexecstack`, and an input whose `.note.GNU-stack`
        // is executable (GNU ld sets PF_X, mold does not: both clean;
        // lld 22 refuses the link); each `-z` output with PF_X flipped is P-D4, and so is
        // the lazy hello PIE with PF_X set.
        let d = b.dir(arch, "stack");
        b.obj(arch, &d, "hello.c", &["-O1", "-fPIE"], "h.o");
        b.obj(arch, &d, "xstack.s", &[], "x.o");
        for (out, z) in [("stack.exec", "-Wl,-z,execstack"), ("stack.noexec", "-Wl,-z,noexecstack")]
        {
            b.link("stack", arch, &d, out, &["-pie", z, "h.o"], [CLEAN; 3]);
            for linker in Linker::ALL {
                let from = format!("{out}.{}", linker.name());
                b.patched(
                    "stack",
                    arch,
                    &d,
                    &from,
                    &format!("{from}.flipped"),
                    &format!("{from}.map"),
                    &format!("{from}.line"),
                    linker,
                    gnu_stack_x_flipped,
                    &["P-D4@PT_GNU_STACK"],
                );
            }
        }
        // lld 22 refuses the executable note without `-z execstack`.
        for k in [0, 2] {
            b.link_one("stack", arch, &d, "stack.note", &["-pie", "h.o", "x.o"], k, CLEAN);
        }
        // P-D7 (WP-082 follow-up 3, WP-088): GNU ld's AArch64 PIE writes
        // `.dynsym` section symbols (`.init`, `.data`); one moved 8 bytes
        // into its section, and one naming the next section.
        if !x64 {
            let hello = b.dir(arch, "hello");
            let from = "hello-pie.lazy.bfd";
            let secsyms: [Patch; 2] =
                [("secsym-value", dynsym_section_value), ("secsym-shndx", dynsym_section_shndx)];
            for (what, patch) in secsyms {
                b.patched(
                    "secsym",
                    arch,
                    &hello,
                    from,
                    &format!("{from}.{what}"),
                    &format!("{from}.map"),
                    &format!("{from}.line"),
                    Linker::Bfd,
                    patch,
                    &["P-D7@which does not hold it"],
                );
            }
        }
        // R1 in a dynamic executable (WP-082 follow-up 4, WP-088): the
        // AArch64 large-model local-exec `movz`/`movk`/`movk` (TLSLE
        // MOVW_TPREL_G2/G1_NC/G0_NC) against the executable's own `gv`;
        // clean from the three linkers, and GNU ld's with the `G0_NC`
        // `movk`'s immediate + 8 is R1.
        if !x64 {
            let d = b.dir(arch, "tlsle");
            let large =
                ["-O1", "-fno-pie", "-mcmodel=large", "-ftls-model=local-exec", "-mtls-size=48"];
            b.obj(arch, &d, "tlsle_u.c", &large, "u.o");
            b.obj(arch, &d, "tlsle_d.c", &["-O1"], "d.o");
            b.link("tlsle", arch, &d, "le", &["-no-pie", "u.o", "d.o"], [CLEAN; 3]);
            b.patched(
                "tlsle",
                arch,
                &d,
                "le.bfd",
                "le.bfd.g0",
                "le.bfd.map",
                "le.bfd.line",
                Linker::Bfd,
                movk_g0_moved,
                &["R1"],
            );
        }
        // P-D14's entry rule and §12.4's non-address RELATIVE rule, in a
        // Pie (WP-088): each linker's output with `e_entry` one
        // instruction on (P-D4), and with `rx_p`'s RELATIVE record moved
        // to `rx_v` with the number as its addend (view 1 unchanged; P-D9
        // by that rule, and view 2 for `rx_p`).
        let d = b.dir(arch, "pie-rules");
        b.obj(arch, &d, "relx.c", &["-O1", "-fPIE"], "rx.o");
        b.link("pie-rules", arch, &d, "rx", &["-pie", "rx.o"], [CLEAN; 3]);
        let rules: [Patch; 2] = [("entry", entry_moved), ("relx", relative_to_number)];
        let expect: [&'static [&'static str]; 2] = [
            &["P-D4@the entry point is not '_start'"],
            &["P-D9@that no input relocation makes an address of the executable"],
        ];
        for linker in Linker::ALL {
            for ((what, patch), e) in rules.into_iter().zip(expect) {
                let e = if what == "relx" && linker.name() == "lld" { LLD_RELX } else { e };
                let from = format!("rx.{}", linker.name());
                b.patched(
                    "pie-rules",
                    arch,
                    &d,
                    &from,
                    &format!("{from}.{what}"),
                    &format!("{from}.map"),
                    &format!("{from}.line"),
                    linker,
                    patch,
                    e,
                );
            }
        }
        // An x86-64 PC-relative field against an `SHN_ABS` symbol in a
        // PIE (WP-088): GNU ld and lld refuse the `PLT32` call, mold
        // writes `call 0x1234`, right at load bias 0 only.
        if x64 {
            let d = b.dir(arch, "abspc");
            b.obj(arch, &d, "abspc_a.s", &[], "a.o");
            b.obj(arch, &d, "abspc_c.s", &[], "c.o");
            b.link("abspc", arch, &d, "ac", &["-pie", "c.o", "a.o"], [B, B, A35]);
            // The GOT load: GNU ld writes `mov $0x1234`, lld and mold keep
            // it (all clean); mold's with the mov made `lea absv(%rip)`
            // (its X1 form) is the rule's.
            b.obj(arch, &d, "abspc_g.s", &[], "g.o");
            b.link("abspc", arch, &d, "ag", &["-pie", "g.o", "a.o"], [CLEAN; 3]);
            b.patched(
                "abspc",
                arch,
                &d,
                "ag.mold",
                "ag.mold.lea",
                "ag.mold.map",
                "ag.mold.line",
                Linker::Mold,
                got_load_to_lea,
                &["R1@to an absolute symbol"],
            );
        }
        // dyn.md W17: lld's x86-64 large-model general-dynamic outputs
        // crash (PIE and `-no-pie`); GNU ld's and mold's run and are
        // clean, lld's are A12 (R1: the 16-byte local-exec form written
        // over the 22-byte sequence, x64.md W4).
        if x64 {
            let d = b.dir(arch, "w17");
            b.obj(arch, &d, "gd17.c", &["-O1", "-fPIC", "-mcmodel=large"], "g.o");
            for (out, pm) in [("gd.pie", "-pie"), ("gd.nopie", "-no-pie")] {
                b.link("w17", arch, &d, out, &[pm, "g.o"], [CLEAN, A12, CLEAN]);
            }
        }
        // S5 (every format since WP-088): the text PT_LOAD made PF_W, the
        // data PT_LOAD made PF_X, in each linker's lazy hello PIE.
        let hello = b.dir(arch, "hello");
        let perms: [Patch; 2] = [("textw", text_writable), ("datax", data_executable)];
        for linker in Linker::ALL {
            for (what, patch) in perms {
                let from = format!("hello-pie.lazy.{}", linker.name());
                b.patched(
                    "perm",
                    arch,
                    &hello,
                    &from,
                    &format!("{from}.{what}"),
                    &format!("{from}.map"),
                    &format!("{from}.line"),
                    linker,
                    patch,
                    &["S5@holds no section that needs it"],
                );
            }
        }
        let hello = b.dir(arch, "hello");
        for linker in Linker::ALL {
            let from = format!("hello-pie.lazy.{}", linker.name());
            b.patched(
                "stack",
                arch,
                &hello,
                &from,
                &format!("{from}.xstack"),
                &format!("{from}.map"),
                &format!("{from}.line"),
                linker,
                gnu_stack_x_flipped,
                &["P-D4@PT_GNU_STACK"],
            );
        }

        // X-58 (WP-082 V2): a TLS descriptor relaxed to initial-exec
        // into each of the sixteen registers (`f_R` in the source; the
        // programs exit 0 when every one reads the library's 5).
        if x64 {
            let d = b.dir(arch, "tlsdesc-regs");
            b.so(arch, &d, "libtx.c", "libtx.so", "libtx.so", &[]);
            b.obj(arch, &d, "tlsdesc_regs.s", &[], "tr.o");
            for pm in ["-pie", "-no-pie"] {
                let out = format!("tr{pm}");
                b.link("tlsdesc-regs", arch, &d, &out, &[pm, "tr.o", "-L.", "-ltx"], [CLEAN; 3]);
            }
        }

        // tlsgd-to-ie (WP-082, WP-054's request): AArch64's traditional
        // general-dynamic sequence against a library's variable, relaxed
        // to initial-exec by GNU ld and kept by mold (lld refuses the
        // traditional dialect's relocations).
        if !x64 {
            let d = b.dir(arch, "tlsgd-ie");
            b.so(arch, &d, "libgdie.c", "libgdie.so", "libgdie.so", &[]);
            b.obj(arch, &d, "gd_ie.c", &["-fPIC", "-O1", "-mtls-dialect=trad"], "gd.o");
            for pm in ["-pie", "-no-pie"] {
                let out = format!("gd{pm}");
                for k in [0, 2] {
                    b.link_one(
                        "tlsgd-ie",
                        arch,
                        &d,
                        &out,
                        &[pm, "gd.o", "-L.", "-lgdie"],
                        k,
                        CLEAN,
                    );
                }
            }
        }

        // D-23's export row (WP-082, WP-054 case (a)): a template's
        // inline variable, defined STB_GNU_UNIQUE by the program and a
        // library, exported and kept unique (mutation `dynsym-unique`).
        let d = b.dir(arch, "unique");
        b.so(arch, &d, "libunique.cc", "libunique.so", "libunique.so", &[]);
        b.obj(arch, &d, "unique_main.cc", &["-O1", "-fPIE"], "um.o");
        b.link("unique", arch, &d, "unique", &["-pie", "um.o", "-L.", "-lunique"], [CLEAN; 3]);

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
                    [Some(&["F3"][..]), H1, Some(&["F3", "P-D10", "A20"][..])]
                } else {
                    [CLEAN, H1, H1]
                };
                b.link("e1", arch, &d, &out, &args, e);
            }
        }

        // P-D2: a library of the line the verifier cannot read; P-D12: a
        // TLS record turned against a function.
        let line = std::fs::read_to_string(d.join("e1.pie.now.bfd.line")).unwrap();
        let gone = line.replace("libT.so", "libGone.so").replace("-lT\n", "-lGone\n");
        assert_ne!(gone, line, "the line names libT");
        std::fs::write(d.join("gone.line"), gone).unwrap();
        b.patched(
            "PD2",
            arch,
            &d,
            "e1.pie.now.bfd",
            "pd2.bfd",
            "e1.pie.now.bfd.map",
            "gone.line",
            Linker::Bfd,
            |_| {},
            &["P-D2"],
        );
        b.patched(
            "PD12",
            arch,
            &d,
            "e1.pie.now.bfd",
            "pd12.bfd",
            "e1.pie.now.bfd.map",
            "e1.pie.now.bfd.line",
            Linker::Bfd,
            tls_record_to_function,
            // The program's TLS uses then see the wrong offset (R1, R4);
            // x86-64 PIE keeps GNU ld's F3 (W3).
            if x64 { &["F3", "P-D12", "R1", "R4"] } else { &["P-D12", "R1", "R4"] },
        );

        // W13 measured (WP-088): `-E` exports `ifx`, an IFUNC nothing
        // references, from a PIE and from a `-no-pie` executable. Every
        // reference exports it at its resolver (clean); mold's `-no-pie`
        // export (and `.symtab` entry) is 0xfffffff0 / 0xfffffff8, in no
        // section.
        let d = b.dir(arch, "W13E");
        b.obj(arch, &d, "ifx.c", &["-O1", "-fPIE"], "p.o");
        b.obj(arch, &d, "ifx.c", &["-O1", "-fno-pie"], "n.o");
        b.link("W13E", arch, &d, "ifx.pie", &["-pie", "-Wl,-E", "p.o"], [CLEAN; 3]);
        b.link(
            "W13E",
            arch,
            &d,
            "ifx.nopie",
            &["-no-pie", "-Wl,-E", "n.o"],
            [CLEAN, CLEAN, MOLD_IFX],
        );

        // W14: data.o reads `tv` as an address; libtv.so's `tv` is
        // STT_TLS. GNU ld refuses (D-16); on AArch64 lld (a TPREL64 GOT
        // slot) and mold (a GLOB_DAT) link it and the program runs
        // wrong: P-D12. On x86-64 lld refuses the PIE's PC32 and mold
        // copies the TLS definition: S8.
        let d = b.dir(arch, "W14");
        b.so(arch, &d, "libtv.c", "libtv.so", "libtv.so", &[]);
        b.obj(arch, &d, "w14.c", &["-O1", "-fPIE"], "w14.o");
        b.link(
            "W14",
            arch,
            &d,
            "w14",
            &["-pie", "w14.o", "libtv.so"],
            a([B, W14, W14], [B, B, W14_COPY]),
        );

        // W16: libmid2.so (used: libtop2.so needs it) calls leaf_fn, which
        // only the as-needed libleaf.so defines. GNU ld refuses (D-13);
        // lld and mold drop libleaf.so: P-D6.
        let d = b.dir(arch, "W16");
        b.so(arch, &d, "libleaf.c", "libleaf.so", "libleaf.so", &[]);
        b.so(arch, &d, "libmid2.c", "libmid2.so", "libmid2.so", &[]);
        b.so(arch, &d, "libtop2.c", "libtop2.so", "libtop2.so", &["-L.", "-lmid2"]);
        b.obj(arch, &d, "w16.c", &["-O1", "-fPIE"], "w16.o");
        b.link(
            "W16",
            arch,
            &d,
            "w16",
            &["-pie", "w16.o", "-L.", "-ltop2", "-lmid2", "-lleaf"],
            [B, W16, W16],
        );

        // W1: an import whose only definition is STV_HIDDEN.
        let d = b.dir(arch, "W1");
        b.so(arch, &d, "libvis.c", "libvis.so", "libvis.so", &[]);
        set_dynsym_visibility(&d.join("libvis.so"), "vf", 2);
        b.obj(arch, &d, "w1.c", &["-O1"], "w1.o");
        let w1: Option<&[&str]> = Some(&["P-D7", "P-D9", "A27"]);
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
            [Some(&["P-D11", "P-D7", "A21"]), CLEAN, CLEAN],
        );

        // W3: an IFUNC's address taken in two units.
        let d = b.dir(arch, "W3");
        for (pm, pic, flag) in [("pie", "-fPIE", "-pie"), ("nopie", "-fno-pic", "-no-pie")] {
            b.obj(arch, &d, "ifeq_a.c", &[pic, "-O1"], &format!("a.{pm}.o"));
            b.obj(arch, &d, "ifeq_b.c", &[pic, "-O1"], &format!("b.{pm}.o"));
            let e = if x64 && pm == "pie" {
                [Some(&["F3"][..]), H1, Some(&["F3", "P-D10", "A20"][..])]
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
                [Some(&["F3"]), H1, Some(&["F3", "P-D10", "A20"])],
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
            a([B, B, Some(&["P-D9", "P-D6", "A24"])], [B; 3]),
        );

        // W7: an import's address in a read-only section of a PIE.
        let d = b.dir(arch, "W7");
        b.so(arch, &d, "textrel_lib.c", "libtextrel.so", "libtextrel.so", &[]);
        b.obj(arch, &d, "textrel_ro.c", &["-fPIE", "-O1"], "tr.o");
        let tr: Option<&[&str]> = Some(&["P-D9", "P-D6", "A24"]);
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
                    Some(&["P-D4", "A22"])
                } else {
                    Some(&["P-D1", "P-D4", "A23"])
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
                (true, "pie") => [Some(&["P-D7"][..]), H1, Some(&["P-D7", "P-D10", "A20"][..])],
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

/// A named edit of an output's bytes.
type Patch = (&'static str, fn(&mut [u8]));

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

/// x86-64: PLT0's `jmp *(GOT+16)(%rip)` displacement (at `.plt + 8` for
/// GNU ld and lld, after mold's `endbr64; push %r11` at `.plt + 14`)
/// lowered by 0x1030 (the record's reproducer).
fn plt0_jump_lowered(b: &mut [u8]) {
    let s = sections(b);
    let p = section(&s, ".plt").3;
    let o = (p..p + 16).find(|&o| b[o..o + 2] == [0xff, 0x25]).expect("PLT0's jmp") + 2;
    let v = le32(b, o).wrapping_sub(0x1030);
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// R1: the first `movk x<n>, #imm` (no shift) after a `movz ..., lsl #32`
/// in `.text` gets imm + 8 (the large-model local-exec sequence's G0).
fn movk_g0_moved(b: &mut [u8]) {
    let s = sections(b);
    let t = section(&s, ".text");
    let w = |o: usize| le32(b, o);
    let o = (t.3..t.3 + t.4 - 8)
        .step_by(4)
        .find(|&o| {
            w(o) & 0xffe0_0000 == 0xd2c0_0000
                && w(o + 4) & 0xffe0_0000 == 0xf2a0_0000
                && w(o + 8) & 0xffe0_0000 == 0xf280_0000
        })
        .expect("movz lsl 32; movk lsl 16; movk")
        + 8;
    let v = w(o) + (8 << 5);
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// The value of `.symtab`'s symbol `name`.
fn sym_value(b: &[u8], name: &str) -> u64 {
    let s = sections(b);
    let t = section(&s, ".symtab");
    let strs = s[t.5].3;
    (t.3..t.3 + t.4)
        .step_by(24)
        .find(|&o| cstr(b, strs + le32(b, o) as usize) == name)
        .map_or_else(|| panic!("no symbol {name}"), |o| le64(b, o + 8))
}

/// §5.1a: `main`'s `movq absv@GOTPCREL(%rip), %rax` made `lea
/// absv(%rip), %rax` (mold's X1 rewriting, wrong for an absolute symbol in
/// a PIE).
fn got_load_to_lea(b: &mut [u8]) {
    let s = sections(b);
    let text = section(&s, ".text");
    let main = sym_value(b, "main");
    let o = text.3 + (main - text.2) as usize;
    assert_eq!(b[o..o + 3], [0x48, 0x8b, 0x05], "main's GOT load");
    b[o + 1] = 0x8d;
    let disp = 0x1234_i64 - (main as i64 + 7);
    b[o + 3..o + 7].copy_from_slice(&(disp as i32).to_le_bytes());
}

/// P-D4: `e_entry` one instruction on (4 bytes: an AArch64 instruction;
/// inside x86-64's `_start`, whose first is `endbr64` or `xor %ebp`).
fn entry_moved(b: &mut [u8]) {
    let v = le64(b, 0x18) + 4;
    b[0x18..0x20].copy_from_slice(&v.to_le_bytes());
}

/// P-D9: the `RELATIVE` record of `rx_p` (addend `&rx_v`) moved to
/// `rx_v`, its addend the number `rx_v` holds.
fn relative_to_number(b: &mut [u8]) {
    let (p, v) = (sym_value(b, "rx_p"), sym_value(b, "rx_v"));
    let s = sections(b);
    let r = section(&s, ".rela.dyn");
    let o = (r.3..r.3 + r.4).step_by(24).find(|&o| le64(b, o) == p).expect("rx_p's record");
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
    b[o + 16..o + 24].copy_from_slice(&12345u64.to_le_bytes());
}

/// The file offset of the last `STT_SECTION` entry of `.dynsym`.
fn dynsym_section(b: &[u8]) -> usize {
    let s = sections(b);
    let d = section(&s, ".dynsym");
    (d.3..d.3 + d.4).step_by(24).rfind(|&o| b[o + 4] & 0xf == 3).expect("a section symbol")
}

/// P-D7: a `.dynsym` section symbol's value 8 bytes into its section.
fn dynsym_section_value(b: &mut [u8]) {
    let o = dynsym_section(b) + 8;
    let v = le64(b, o) + 8;
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

/// P-D7: a `.dynsym` section symbol naming the next section.
fn dynsym_section_shndx(b: &mut [u8]) {
    let o = dynsym_section(b) + 6;
    let v = le16(b, o) as u16 + 1;
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

/// The file offset of the first `PT_LOAD` whose flags have `has`.
fn load_with(b: &[u8], has: u32) -> usize {
    let (phoff, phentsize, phnum) = (le64(b, 0x20) as usize, le16(b, 0x36), le16(b, 0x38));
    (0..phnum)
        .map(|i| phoff + i * phentsize)
        .find(|&o| le32(b, o) == 1 && le32(b, o + 4) & has != 0)
        .expect("a PT_LOAD with the flag")
}

/// S5: the text `PT_LOAD` made `PF_W`.
fn text_writable(b: &mut [u8]) {
    let o = load_with(b, 1);
    b[o + 4] |= 2;
}

/// S5: the data `PT_LOAD` made `PF_X`.
fn data_executable(b: &mut [u8]) {
    let o = load_with(b, 2);
    b[o + 4] |= 1;
}

/// P-D4: `PT_GNU_STACK`'s `PF_X` flipped.
fn gnu_stack_x_flipped(b: &mut [u8]) {
    let (phoff, phentsize, phnum) = (le64(b, 0x20) as usize, le16(b, 0x36), le16(b, 0x38));
    let o = (0..phnum)
        .map(|i| phoff + i * phentsize)
        .find(|&o| le32(b, o) == 0x6474_e551)
        .expect("PT_GNU_STACK");
    b[o + 4] ^= 1;
}

/// The file offset of the `.eh_frame` FDE whose initial location (GNU
/// ld's `pcrel|sdata4`) is `.plt`'s address, and its length.
fn plt_fde(b: &[u8]) -> (usize, usize) {
    let s = sections(b);
    let plt = section(&s, ".plt").2;
    let eh = section(&s, ".eh_frame");
    let (mut o, end) = (eh.3, eh.3 + eh.4);
    while o + 16 <= end {
        let len = le32(b, o) as usize;
        if len == 0 {
            break;
        }
        if le32(b, o + 4) != 0 {
            let field = eh.2 + (o + 8 - eh.3) as u64;
            if field.wrapping_add_signed(i64::from(le32(b, o + 8) as i32)) == plt {
                return (o, len + 4);
            }
        }
        o += len + 4;
    }
    panic!("no FDE for .plt");
}

/// E6: the `.plt` FDE's address range 16 bytes longer (past `.plt`).
fn plt_fde_range_past(b: &mut [u8]) {
    let (o, _) = plt_fde(b);
    let v = le32(b, o + 12) + 16;
    b[o + 12..o + 16].copy_from_slice(&v.to_le_bytes());
}

/// E6: the `.plt` FDE's expression compares the entry offset with one
/// more than the `push` ends at (`DW_OP_and; DW_OP_lit11` or `lit9`).
fn plt_fde_lit_raised(b: &mut [u8]) {
    let (o, n) = plt_fde(b);
    let i = (o + 17..o + n - 1)
        .find(|&i| b[i] == 0x1a && (0x30..0x4f).contains(&b[i + 1]))
        .expect("DW_OP_and; DW_OP_lit<n>");
    b[i + 1] += 1;
}

/// E6: the `.plt` FDE's second `DW_CFA_def_cfa_offset` (24, after PLT0's
/// `push`) lowered to 16.
fn plt_fde_offset_lowered(b: &mut [u8]) {
    let (o, n) = plt_fde(b);
    let i = (o + 17..o + n - 1).find(|&i| b[i..i + 2] == [0x0e, 0x18]).expect("def_cfa_offset 24");
    b[i + 1] = 0x10;
}

/// AArch64: the PLT header's `ldr x17, [x16, #lo]` (its third word, after
/// `stp` and `adrp`) reads the next word.
fn a64_header_ldr_moved(b: &mut [u8]) {
    let s = sections(b);
    let o = section(&s, ".plt").3 + 8;
    let w = le32(b, o);
    assert_eq!(w & 0xffc0_03ff, 0xf940_0211, "the header's ldr x17, [x16, #lo]");
    let w = (w & !(0xfff << 10)) | ((((w >> 10) & 0xfff) + 1) & 0xfff) << 10;
    b[o..o + 4].copy_from_slice(&w.to_le_bytes());
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

/// Points the first symbolic TLS record (`DTPMOD`, `DTPOFF`, `TPOFF`,
/// `TLSDESC`) at the `.dynsym` entry of `lib_get_dvar`, a function.
fn tls_record_to_function(b: &mut [u8]) {
    let s = sections(b);
    let dynsym = section(&s, ".dynsym").clone();
    let strs = s[dynsym.5].3;
    let f = (0..dynsym.4 / 24)
        .find(|i| cstr(b, strs + le32(b, dynsym.3 + i * 24) as usize) == "lib_get_dvar")
        .expect("lib_get_dvar in .dynsym") as u64;
    let tls = |ty: u64| matches!(ty, 1028..=1031 | 16..=18 | 36);
    for r in s.iter().filter(|x| x.1 == 4) {
        for o in (r.3..r.3 + r.4).step_by(24) {
            let info = le64(b, o + 8);
            if info >> 32 != 0 && tls(info & 0xffff_ffff) {
                b[o + 8..o + 16].copy_from_slice(&((f << 32) | (info & 0xffff_ffff)).to_le_bytes());
                return;
            }
        }
    }
    panic!("no symbolic TLS record");
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
        // `found`: each finding's anomaly id, or its rule when it is none;
        // `rules`: every finding's rule (a W row names both).
        let mut found = BTreeSet::new();
        let mut rules = BTreeSet::new();
        for f in &r.findings {
            rules.insert(f.rule.id());
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
                None => !found.contains(**e) && !rules.contains(**e),
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
        // P-D2 stops stage 3 before any record is judged.
        assert!(r.coverage.records > 0 || o.case == "PD2", "{}: no records", o.label());
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

// spec/plumb.md §12 P-D11 (WP-088): copies of a read-only and a writable
// datum, with RELRO and under -z norelro, are clean for every reference.
#[test]
fn s12_p_d11_copies_with_and_without_relro() {
    judge("copy-relro");
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

// spec/plumb.md §12 P-D10 (WP-082): the PLT header and the way back.
#[test]
fn s12_p_d10_plt_header_reads_its_words() {
    judge("PLT0");
}

// spec/plumb.md §4.7, R1 in stage 3 (WP-082 follow-up 4, WP-088): AArch64's
// large-model local-exec against the executable's own TLS variable.
#[test]
fn r1_a64_large_model_local_exec_in_a_dynamic_executable() {
    judge("tlsle");
}

// W13 (WP-088): an exported, unreferenced IFUNC under `-E`.
#[test]
fn w13_exported_unreferenced_ifunc() {
    judge("W13E");
}

// dyn.md W14 and W16 (WP-088): P-D12's non-TLS reference to a shared TLS
// definition, P-D6's D-13 check of the used objects' references.
#[test]
fn w14_non_tls_reference_to_shared_tls() {
    judge("W14");
}

#[test]
fn w16_used_object_reference_unsatisfied() {
    judge("W16");
}

// dyn.md W17: what the verifier reports on lld's large-model GD outputs.
#[test]
fn w17_x64_large_model_general_dynamic() {
    judge("w17");
}

// An x86-64 PC-relative field against an absolute symbol in a PIE (WP-088).
#[test]
fn x64_pc_relative_to_an_absolute_symbol_in_a_pie() {
    judge("abspc");
}

// spec/plumb.md §12.4's entry and non-address rules in a Pie (WP-088).
#[test]
fn s12_pie_entry_and_non_address_relative() {
    judge("pie-rules");
}

// spec/plumb.md §12 P-D7 (WP-088): a reference's `.dynsym` section symbol
// is at its section's start.
#[test]
fn s12_p_d7_dynsym_section_symbols() {
    judge("secsym");
}

// spec/plumb.md §6.2 S5 (every format since WP-088): no PT_LOAD asks for
// a permission none of its sections needs.
#[test]
fn s6_2_s5_no_unneeded_permission() {
    judge("perm");
}

// spec/plumb.md §12 P-D4 (WP-088): PT_GNU_STACK's PF_X against the line's
// `-z execstack`/`-z noexecstack` and the inputs' stack notes.
#[test]
fn s12_p_d4_gnu_stack_execute() {
    judge("stack");
}

// spec/plumb.md §6.4 E6 (WP-088): GNU ld's x86-64 PLT FDEs are judged
// against the PLT's stack; the clean ones are the other cases'.
#[test]
fn s6_4_e6_plt_fde_against_the_plt() {
    judge("plt-fde");
}

// spec/plumb.md §12 P-D10 (WP-082): the IBT and BTI PLT shapes are judged.
#[test]
fn s12_p_d10_ibt_and_bti_shapes() {
    judge("shapes");
}

// spec/x64.md X-58, plumb.md R1 (WP-082 V2): the initial-exec `mov` into
// any register is the descriptor's relaxation, not a finding.
#[test]
fn r1_tlsdesc_relaxed_to_ie_into_every_register() {
    judge("tlsdesc-regs");
}

#[test]
fn r1_a64_tlsgd_relaxed_to_ie() {
    judge("tlsgd-ie");
}

#[test]
fn p_d7_unique_export_stays_unique() {
    judge("unique");
}

#[test]
fn s12_dupdef_is_p_d9() {
    judge("DUP");
}

#[test]
fn s12_p_d2_unreadable_library_stops_stage_3() {
    judge("PD2");
}

#[test]
fn s12_p_d12_tls_record_against_a_function() {
    judge("PD12");
}

#[test]
fn s12_f2_is_p_d9_view_3() {
    judge("F2");
}

/// `(st_value, st_size, st_shndx)` of `name` in `.dynsym`.
fn dynsym(b: &[u8], name: &str) -> (u64, u64, usize) {
    let s = sections(b);
    let t = section(&s, ".dynsym");
    let strs = s[t.5].3;
    (t.3..t.3 + t.4)
        .step_by(24)
        .find(|&o| le16(b, o + 6) != 0 && cstr(b, strs + le32(b, o) as usize) == name)
        .map_or_else(
            || panic!("no defined .dynsym {name}"),
            |o| (le64(b, o + 8), le64(b, o + 16), le16(b, o + 6)),
        )
}

/// The alignment a copy of `name` from the library `lib` takes (D-42,
/// P-D11): its section's `sh_addralign`, no more than its value allows.
fn copy_align(lib: &[u8], name: &str) -> u64 {
    let (value, _, shndx) = dynsym(lib, name);
    let (shoff, shentsize) = (le64(lib, 0x28) as usize, le16(lib, 0x3a));
    let sec = le64(lib, shoff + shndx * shentsize + 0x30).max(1);
    sec.min(1 << value.trailing_zeros().min(63))
}

// spec/plumb.md §3.3 M8 (WP-096): lld's copies of `ro_p` and `rw_v`,
// its map in mortar's format with the `copy` records mortar's states
// (the place and size of each copy, the alignment D-42 gives it): no M8
// finding; each field changed, a second record, or a record where no
// COPY record is, an M8 finding.
#[test]
fn s3_3_m8_copy_records_are_the_copies() {
    use mortar_plumb::Format;
    use mortar_plumb::map::{CopyHint, DynRecords, Map};
    type Case = (&'static str, &'static str, fn(&mut CopyHint));
    let Some(c) = corpus() else { return };
    let mut n = 0;
    for o in c.outs.iter().filter(|o| o.case == "copy-relro" && o.link.linker == Linker::Lld) {
        n += 1;
        let out = o.link.output_bytes();
        let lib = std::fs::read(o.link.dir.join("libro.so")).unwrap();
        let mut base = o.link.parse_map();
        let files = o.link.inputs(&base);
        base.format = Format::Mortar;
        let copies = ["ro_p", "rw_v"]
            .map(|name| {
                let (addr, size, _) = dynsym(&out, name);
                CopyHint { name: name.into(), addr, size, align: copy_align(&lib, name) }
            })
            .to_vec();
        base.dynamic = Some(DynRecords { copies, ..DynRecords::default() });
        let line: Vec<String> =
            std::fs::read_to_string(&o.line).unwrap().lines().map(str::to_string).collect();
        // The map's line is the link's (plumb.md §3.2a).
        base.args = line.iter().map(|w| w.as_bytes().to_vec()).collect();
        let m8 = |map: &Map| -> Vec<String> {
            let text = mortar_plumb::map::mortar::write(&with_line(map));
            let map = mortar_plumb::map::parse(Format::Mortar, &text).unwrap();
            let inputs: Vec<Input<'_>> =
                files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
            let ctx = Ctx { line: Some(line.clone()), files: &Dir(&o.link.dir) };
            mortar_plumb::verify_with(&out, &inputs, &map, &ctx)
                .findings
                .iter()
                .filter(|f| f.rule.id() == "M8")
                .map(|f| f.message.clone())
                .collect()
        };
        assert!(m8(&base).is_empty(), "{}: {:#?}", o.label(), m8(&base));
        let cases: [Case; 7] = [
            ("name", "names 'rw_v'", |c| c.name = "rw_v".into()),
            ("size", "is stated of size", |c| c.size += 1),
            ("align up", "is stated aligned to", |c| c.align *= 2),
            ("align down", "is stated aligned to", |c| c.align /= 2),
            ("align past the address", "not a multiple", |c| c.align = 1 << 40),
            ("addr", "names no COPY record", |c| c.addr += 0x100),
            ("addr unaligned", "names no COPY record", |c| c.addr += 1),
        ];
        for (what, needle, f) in cases {
            let mut m = base.clone();
            f(&mut m.dynamic.as_mut().unwrap().copies[0]);
            let got = m8(&m);
            assert!(got.iter().any(|g| g.contains(needle)), "{} {what}: {got:#?}", o.label());
        }
        let mut m = base.clone();
        let again = m.dynamic.as_ref().unwrap().copies[1].clone();
        m.dynamic.as_mut().unwrap().copies.push(again);
        let got = m8(&m);
        assert!(got.iter().any(|g| g.contains("second record")), "{} twice: {got:#?}", o.label());
        // Required: a COPY record whose copy record is left out.
        let mut m = base.clone();
        m.dynamic.as_mut().unwrap().copies.remove(0);
        let got = m8(&m);
        assert!(
            got.iter().any(|g| g.contains("'ro_p'") && g.contains("has no copy record")),
            "{} left out: {got:#?}",
            o.label()
        );
    }
    assert!(n > 0 || c.outs.is_empty(), "no lld outputs of copy-relro");
}

/// `map` with a line (`spec/plumb.md` §3.2a: every mortar map states its
/// link's words, and mortar's reader refuses one without). A reference's
/// map has none: these links gave none of the options §3.2a's rules read,
/// so the map's input paths stand for the words.
fn with_line(map: &mortar_plumb::Map) -> mortar_plumb::Map {
    let mut m = map.clone();
    if m.args.is_empty() {
        m.args = m.paths().iter().map(|p| p.as_bytes().to_vec()).collect();
        if m.args.is_empty() {
            m.args.push(b"a.o".to_vec());
        }
    }
    m
}
