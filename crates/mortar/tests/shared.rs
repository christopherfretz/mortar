//! mortar links a shared object (`spec/dyn.md` D-100..D-125, WP-123: the
//! join of WP-117, WP-118 and WP-119), AArch64 only: the option rows the
//! join turns on, each with an observable effect, through the compiler
//! driver (`gcc -B<dir with ld = mortar>`), loaded and run by a host GNU
//! ld links, and judged by the verifier. The link tests of the first
//! target are under `tests/link/dyn/shared/`; these are the proofs the
//! option audit (`crates/mortar/tests/options.rs`) names.

mod support;

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use mortar_testutil::target::Arch;
use mortar_testutil::{Outcome, Scratch, run_with};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");
const A: Arch = Arch::Aarch64;

/// A library: `f` and `d` are called and read from inside it as well as
/// exported; `g` is protected; `pid` imports from the C library.
const LIB: &str = "#include <unistd.h>\nint pid(void) { return getpid(); }\n\
    int d = 10;\n\
    int f(void) { return 1; }\n\
    __attribute__((visibility(\"protected\"))) int g(void) { return 7; }\n\
    int via_f(void) { return f(); }\n\
    int via_d(void) { return d; }\n\
    int via_g(void) { return g(); }\n";
/// The host: interposes `f` and `d` when `INTERPOSE` is defined.
const HOST: &str = "#include <stdio.h>\n\
    int via_f(void); int via_d(void); int via_g(void);\n\
    #ifdef INTERPOSE\nint d = 20;\nint f(void) { return 2; }\n#endif\n\
    int main(void) { printf(\"%d %d %d\\n\", via_f(), via_d(), via_g()); return 0; }\n";
/// A library with a reference nothing in it defines.
const UNDEF: &str = "int u(void);\nint call_u(void) { return u() + 1; }\n";
const UNDEF_HOST: &str = "#include <stdio.h>\nint call_u(void);\n\
    int u(void) { return 41; }\nint main(void) { printf(\"%d\\n\", call_u()); return 0; }\n";

struct Cx {
    scratch: Scratch,
    _bin: Scratch,
    b: String,
    gcc: std::path::PathBuf,
}

impl Cx {
    fn new(label: &str) -> Option<Self> {
        let gcc = A.tool("gcc")?;
        let bin = Scratch::new(&format!("{label}-bin"));
        std::os::unix::fs::symlink(MORTAR, bin.join("ld")).unwrap();
        let b = format!("-B{}/", bin.path().display());
        let cx = Self { scratch: Scratch::new(label), _bin: bin, b, gcc };
        cx.scratch.write("lib.c", LIB);
        cx.scratch.write("host.c", HOST);
        cx.scratch.write("undef.c", UNDEF);
        cx.scratch.write("uhost.c", UNDEF_HOST);
        for (src, obj) in [("lib.c", "lib.o"), ("undef.c", "undef.o")] {
            cx.ok(&["-O1", "-fPIC", "-c", src, "-o", obj]);
        }
        Some(cx)
    }

    fn gcc(&self, args: &[&str]) -> Outcome {
        let mut c = Command::new(&self.gcc);
        c.args(args).current_dir(self.scratch.path());
        run_with(&mut c, b"", Duration::from_mins(2))
    }

    fn ok(&self, args: &[&str]) {
        let o = self.gcc(args);
        assert!(o.success(), "gcc {args:?}: {}", o.stderr_text());
    }

    /// mortar links `-shared` `objects` with `extra` driver words to
    /// `out`, with mortar's map next to it.
    fn mortar_shared(&self, objects: &[&str], extra: &[&str], out: &str) -> Outcome {
        let map = format!("-Wl,--mortar-map={out}.mortar-map");
        let mut a = vec![self.b.as_str(), "-shared"];
        a.extend_from_slice(objects);
        a.extend_from_slice(extra);
        a.extend([map.as_str(), "-o", out]);
        self.gcc(&a)
    }

    /// [`Self::mortar_shared`], which must succeed and verify.
    fn library(&self, objects: &[&str], extra: &[&str], out: &str) {
        let o = self.mortar_shared(objects, extra, out);
        assert!(o.success(), "mortar -shared {extra:?}: {}", o.stderr_text());
        verify(self.scratch.path(), out);
    }

    /// The host `src` (with `defs`), linked by GNU ld against `lib` in the
    /// scratch directory, run with the library found there: its stdout.
    fn run_host(&self, src: &str, defs: &[&str], lib: &str, exe: &str) -> String {
        let mut a = vec!["-O1", "-fuse-ld=bfd"];
        a.extend_from_slice(defs);
        a.extend([src, lib, "-Wl,-rpath,$ORIGIN", "-o", exe]);
        self.ok(&a);
        let mut c = A.command(&self.scratch.join(exe), None);
        c.current_dir(self.scratch.path());
        let o = run_with(&mut c, b"", Duration::from_mins(1));
        assert!(o.success(), "{exe}: {}", o.stderr_text());
        o.stdout_text()
    }

    fn readelf(&self, args: &[&str], file: &str) -> String {
        let readelf = A.tool("readelf").expect("readelf");
        let mut c = Command::new(readelf);
        c.args(args).arg(file).current_dir(self.scratch.path());
        let o = run_with(&mut c, b"", Duration::from_mins(1));
        assert!(o.success(), "readelf {args:?} {file}: {}", o.stderr_text());
        o.stdout_text()
    }
}

/// The verifier on mortar's library `out` (its map `<out>.mortar-map`):
/// no finding, but one: `P-D6` on the `.dynamic` order, which the
/// verifier gives a mortar library whose tags are in GNU ld's library
/// order (reported to the verifier agent, WP-123's V2).
fn verify(dir: &Path, out: &str) {
    verify_but(dir, out, &[]);
}

/// [`verify`], and the findings that name one of `known` are not counted
/// either.
fn verify_but(dir: &Path, out: &str, known: &[&str]) {
    let findings = support::plumb::verify_staged(
        &dir.join(out),
        &dir.join(format!("{out}.mortar-map")),
        dir,
    );
    let left: Vec<_> = findings
        .iter()
        .filter(|f| {
            let text = format!("{f:?}");
            !text.contains("the .dynamic tags are not in D-30's order")
                && !known.iter().any(|k| text.contains(k))
        })
        .collect();
    assert!(left.is_empty(), "plumb on {out}: {left:#?}");
}

// spec: dyn.md D-100 (ET_DYN, no PT_INTERP), D-102 (DT_SONAME after
// DT_NEEDED), the `-shared` and `-h`/`-soname` rows (cli.md §4.2)
#[test]
fn shared_with_soname_links_loads_and_runs() {
    let Some(cx) = Cx::new("shared-soname") else { return };
    cx.library(&["lib.o"], &["-Wl,-soname,libt.so.1"], "libt.so");
    let header = cx.readelf(&["-h", "-l"], "libt.so");
    assert!(header.contains("DYN (Shared object file)"), "{header}");
    assert!(!header.contains("INTERP"), "{header}");
    let dynamic = cx.readelf(&["-d"], "libt.so");
    assert!(dynamic.contains("Library soname: [libt.so.1]"), "{dynamic}");
    let needed = dynamic.find("(NEEDED)").expect("NEEDED");
    assert!(needed < dynamic.find("(SONAME)").unwrap(), "{dynamic}");
    // The tags, in order, are GNU ld's for the same line.
    cx.ok(&["-fuse-ld=bfd", "-shared", "lib.o", "-Wl,-soname,libt.so.1", "-o", "gnu.so"]);
    let tags = |file: &str| -> Vec<String> {
        cx.readelf(&["-d"], file)
            .lines()
            .filter_map(|l| l.split_whitespace().nth(1).filter(|t| t.starts_with('(')))
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(tags("libt.so"), tags("gnu.so"));
    assert!(dynamic.contains("Shared library: [libc.so.6]"), "{dynamic}");
    // No NEEDED of the loader: D-11 item 3 reads the edges of the
    // walk's libraries in a shared object too (input.md §8.6).
    assert!(!dynamic.contains("ld-linux"), "{dynamic}");
    // `-h` is the same row; without it there is no DT_SONAME.
    cx.library(&["lib.o"], &["-Wl,-h,libt.so.1"], "libh.so");
    assert!(cx.readelf(&["-d"], "libh.so").contains("Library soname: [libt.so.1]"));
    cx.library(&["lib.o"], &[], "libn.so");
    assert!(!cx.readelf(&["-d"], "libn.so").contains("SONAME"));
    std::fs::copy(cx.scratch.join("libt.so"), cx.scratch.join("libt.so.1")).unwrap();
    assert_eq!(cx.run_host("host.c", &[], "libt.so", "plain"), "1 10 7\n");
    // Preemption (D-112): the host's definitions win for both.
    assert_eq!(cx.run_host("host.c", &["-DINTERPOSE"], "libt.so", "interposed"), "2 20 7\n");
}

// spec: dyn.md D-112 (`-Bsymbolic-functions` binds the library's
// functions to its own definitions, data stays preemptible; the last of
// `-Bsymbolic-functions`/`-Bno-symbolic` wins); protected `g` is never
// preempted
#[test]
fn bsymbolic_functions_binds_functions_not_data() {
    let Some(cx) = Cx::new("shared-bsymf") else { return };
    cx.library(&["lib.o"], &["-Wl,-Bsymbolic-functions"], "libs.so");
    assert_eq!(cx.run_host("host.c", &["-DINTERPOSE"], "libs.so", "symf"), "1 20 7\n");
    // GNU ld writes no DT_SYMBOLIC and no DF_SYMBOLIC for it.
    assert!(!cx.readelf(&["-d"], "libs.so").contains("SYMBOLIC"));
    cx.library(&["lib.o"], &["-Wl,-Bsymbolic-functions", "-Wl,-Bno-symbolic"], "libb.so");
    assert_eq!(cx.run_host("host.c", &["-DINTERPOSE"], "libb.so", "nosym"), "2 20 7\n");
    cx.library(&["lib.o"], &["-Wl,-Bno-symbolic", "-Wl,-Bsymbolic-functions"], "libc2.so");
    assert_eq!(cx.run_host("host.c", &["-DINTERPOSE"], "libc2.so", "symf2"), "1 20 7\n");
}

// spec: dyn.md D-118 (an undefined reference of a shared object is an
// error only under `-z defs`/`--no-undefined`; without them it is a
// GLOBAL NOTYPE UND entry the loader resolves, D-111)
#[test]
fn z_defs_refuses_an_undefined_reference() {
    let Some(cx) = Cx::new("shared-defs") else { return };
    for words in [&["-Wl,-z,defs"][..], &["-Wl,--no-undefined"]] {
        let o = cx.mortar_shared(&["undef.o"], words, "libd.so");
        assert!(!o.success(), "{words:?}");
        assert!(o.stderr_text().contains("undefined symbol: u"), "{}", o.stderr_text());
    }
    // The verifier's P-D9 and P-D7 on `u` are the known false findings
    // on a library linked without its dependencies (the coordinator's
    // note to WP-123; the verifier agent fixes them).
    let o = cx.mortar_shared(&["undef.o"], &[], "libu.so");
    assert!(o.success(), "{}", o.stderr_text());
    verify_but(
        cx.scratch.path(),
        "libu.so",
        &["no needed or used object defines it visibly", "the loader finds no definition of 'u'"],
    );
    let syms = cx.readelf(&["--dyn-syms", "-W"], "libu.so");
    assert!(
        syms.lines().any(|l| l.ends_with(" u") && l.contains("NOTYPE  GLOBAL DEFAULT  UND")),
        "{syms}"
    );
    assert_eq!(cx.run_host("uhost.c", &[], "libu.so", "uhost"), "42\n");
}

// spec: dyn.md D-112 / landed/dyn/WP-110.md §D (`-Bsymbolic` in a shared
// object is refused by name until WP-128); in an executable it is a noop
#[test]
fn bsymbolic_is_refused_in_a_shared_object() {
    let Some(cx) = Cx::new("shared-bsym") else { return };
    let o = cx.mortar_shared(&["lib.o"], &["-Wl,-Bsymbolic"], "libx.so");
    assert!(!o.success());
    let e = o.stderr_text();
    assert!(e.contains("unsupported option: -Bsymbolic"), "{e}");
    assert!(e.contains("WP-128"), "{e}");
    assert!(!cx.scratch.join("libx.so").exists());
}
