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
/// no finding.
fn verify(dir: &Path, out: &str) {
    let findings =
        support::plumb::verify_staged(&dir.join(out), &dir.join(format!("{out}.mortar-map")), dir);
    assert!(findings.is_empty(), "plumb on {out}: {findings:#?}");
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
    cx.library(&["undef.o"], &[], "libu.so");
    let syms = cx.readelf(&["--dyn-syms", "-W"], "libu.so");
    assert!(
        syms.lines().any(|l| l.ends_with(" u") && l.contains("NOTYPE  GLOBAL DEFAULT  UND")),
        "{syms}"
    );
    assert_eq!(cx.run_host("uhost.c", &[], "libu.so", "uhost"), "42\n");
}

// spec: dyn.md D-112 (`-Bsymbolic`, the line's last form, without
// `--dynamic-list`, binds every own definition: the interposed host's
// `f` and `d` do not reach the library's own uses), D-102/D-103
// (DT_SYMBOLIC after DT_SONAME, DF_SYMBOLIC; GNU ld's tags in GNU ld's
// order), P-R10 (no dynamic record names an own definition)
#[test]
fn bsymbolic_binds_every_own_definition() {
    let Some(cx) = Cx::new("shared-bsym") else { return };
    cx.library(&["lib.o"], &["-Wl,-Bsymbolic", "-Wl,-soname,libs.so"], "libs.so");
    std::fs::copy(cx.scratch.join("libs.so"), cx.scratch.join("libs.so.1")).unwrap();
    assert_eq!(cx.run_host("host.c", &["-DINTERPOSE"], "libs.so", "sym"), "1 10 7\n");
    assert_eq!(cx.run_host("host.c", &[], "libs.so", "plain"), "1 10 7\n");
    let dynamic = cx.readelf(&["-dW"], "libs.so");
    let soname = dynamic.find("(SONAME)").expect("SONAME");
    let symbolic = dynamic.find("(SYMBOLIC)").expect("SYMBOLIC");
    assert!(soname < symbolic, "{dynamic}");
    assert!(
        dynamic.lines().any(|l| l.contains("(FLAGS)") && l.ends_with(" SYMBOLIC")),
        "{dynamic}"
    );
    let relocs = cx.readelf(&["-rW"], "libs.so");
    for own in [" f + 0", " d + 0", " g + 0"] {
        assert!(!relocs.contains(own), "{own}: {relocs}");
    }
    // The tags, in order, are GNU ld's for the same line.
    cx.ok(&[
        "-fuse-ld=bfd",
        "-shared",
        "lib.o",
        "-Wl,-Bsymbolic",
        "-Wl,-soname,libs.so",
        "-o",
        "gnu.so",
    ]);
    assert_eq!(tags(&cx, "libs.so"), tags(&cx, "gnu.so"));
    // A later `-Bno-symbolic` undoes it: preempted, no tag.
    cx.library(&["lib.o"], &["-Wl,-Bsymbolic", "-Wl,-Bno-symbolic"], "libn.so");
    assert!(!cx.readelf(&["-dW"], "libn.so").contains("SYMBOLIC"));
    assert_eq!(cx.run_host("host.c", &["-DINTERPOSE"], "libn.so", "nosym"), "2 20 7\n");
}

// spec: dyn.md D-103 (`-z nodelete`, `nodlopen`, `initfirst`, `interpose`,
// `origin`: DF_1_* in DT_FLAGS_1, `origin` also DF_ORIGIN; GNU ld's
// bytes of the tags); refused by name in an executable (`rows::SHARED_ONLY`)
#[test]
fn z_flags_are_written_in_a_shared_object() {
    let Some(cx) = Cx::new("shared-zflags") else { return };
    // The verifier judges the four `DF_1_*` flags (P-D22); DF_ORIGIN in
    // DT_FLAGS is outside its P-D6 today (reported to the verifier
    // package), so `-z origin`'s library is compared with GNU ld's only.
    let four = ["-Wl,-z,nodelete,-z,nodlopen,-z,initfirst,-z,interpose"];
    let o = cx.mortar_shared(&["lib.o"], &four, "libz.so");
    assert!(o.success(), "{}", o.stderr_text());
    verify_but_the_mirror(cx.scratch.path(), "libz.so");
    let dynamic = cx.readelf(&["-dW"], "libz.so");
    assert!(dynamic.contains("Flags: NODELETE INITFIRST NOOPEN INTERPOSE\n"), "{dynamic}");
    assert!(!dynamic.contains("(FLAGS) "), "{dynamic}");
    let all = ["-Wl,-z,nodelete,-z,nodlopen,-z,initfirst,-z,interpose,-z,origin"];
    let o = cx.mortar_shared(&["lib.o"], &all, "liball.so");
    assert!(o.success(), "{}", o.stderr_text());
    let dynamic = cx.readelf(&["-dW"], "liball.so");
    assert!(dynamic.contains("Flags: NODELETE INITFIRST NOOPEN ORIGIN INTERPOSE\n"), "{dynamic}");
    assert!(dynamic.lines().any(|l| l.contains("(FLAGS)") && l.ends_with(" ORIGIN")), "{dynamic}");
    let mut gnu = vec!["-fuse-ld=bfd", "-shared", "lib.o"];
    gnu.extend(all);
    gnu.extend(["-o", "gnu.so"]);
    cx.ok(&gnu);
    assert_eq!(values(&cx, "liball.so"), values(&cx, "gnu.so"));
    assert_eq!(tags(&cx, "liball.so"), tags(&cx, "gnu.so"));
    for one in ["nodelete", "nodlopen", "initfirst", "interpose", "origin"] {
        let word = format!("-Wl,-z,{one}");
        let o = cx.gcc(&[cx.b.as_str(), "-pie", "-fPIE", "host.c", "lib.o", &word, "-o", "exe"]);
        assert!(!o.success(), "{one}");
        let e = o.stderr_text();
        assert!(e.contains(&format!("unsupported option: -z {one}")), "{e}");
        assert!(e.contains("dyn.md D-103"), "{e}");
    }
}

// spec: dyn.md D-110 item 1 (`--exclude-libs`: the definitions of the
// named archives' members leave the export set; matched by the archive's
// file name, `ALL` every archive), D-112 (an excluded definition binds
// locally: an interposer of it does not reach the library's own use)
#[test]
fn exclude_libs_hides_archive_definitions() {
    let Some(cx) = Cx::new("shared-exclude") else { return };
    let ar = A.tool("ar").expect("ar");
    let mut c = Command::new(ar);
    c.args(["rcs", "libundef.a", "undef.o"]).current_dir(cx.scratch.path());
    assert!(run_with(&mut c, b"", Duration::from_mins(1)).success());
    // `undef.o`'s `call_u` comes from the archive, which `use.o` needs.
    cx.scratch.write("use.c", "int call_u(void);\nint use_u(void) { return call_u(); }\n");
    cx.ok(&["-O1", "-fPIC", "-c", "use.c", "-o", "use.o"]);
    for (words, exported) in [
        (&[][..], true),
        (&["-Wl,--exclude-libs,ALL"][..], false),
        (&["-Wl,--exclude-libs,libundef.a"][..], false),
        (&["-Wl,--exclude-libs,libother.a"][..], true),
    ] {
        let o = cx.mortar_shared(&["lib.o", "use.o", "libundef.a"], words, "libe.so");
        assert!(o.success(), "{words:?}: {}", o.stderr_text());
        verify_but_the_mirror(cx.scratch.path(), "libe.so");
        let syms = cx.readelf(&["--dyn-syms", "-W"], "libe.so");
        assert_eq!(syms.lines().any(|l| l.ends_with(" call_u")), exported, "{words:?}: {syms}");
        assert!(syms.lines().any(|l| l.ends_with(" use_u")), "{syms}");
    }
    // `libfoo` names `libfoo.a` in GNU ld only: refused by name.
    let o = cx.mortar_shared(&["use.o", "libundef.a"], &["-Wl,--exclude-libs,libundef"], "libb.so");
    assert!(!o.success());
    assert!(
        o.stderr_text().contains("--exclude-libs: invalid value 'libundef'"),
        "{}",
        o.stderr_text()
    );
}

// spec: dyn.md D-112 (`--dynamic-list`: a listed key preemptible, an
// unlisted one bound, whatever `-Bsymbolic` form is given with it; no
// DT_SYMBOLIC), resolve.md §9.6 (the export set unchanged; in an
// executable the listed definitions are exported)
#[test]
fn dynamic_list_decides_preemption_and_exports() {
    let Some(cx) = Cx::new("shared-dynlist") else { return };
    cx.scratch.write("f.list", "{ f; };\n");
    for words in [
        &["-Wl,--dynamic-list,f.list"][..],
        &["-Wl,-Bsymbolic", "-Wl,--dynamic-list,f.list"],
        &["-Wl,--dynamic-list,f.list", "-Wl,-Bsymbolic"],
        &["-Wl,--dynamic-list,f.list", "-Wl,-Bsymbolic-functions"],
        &["-Wl,--dynamic-list,f.list", "-Wl,-Bno-symbolic"],
    ] {
        let o = cx.mortar_shared(&["lib.o"], words, "libl.so");
        assert!(o.success(), "{words:?}: {}", o.stderr_text());
        verify_but_the_mirror(cx.scratch.path(), "libl.so");
        assert!(!cx.readelf(&["-dW"], "libl.so").contains("SYMBOLIC"), "{words:?}");
        let syms = cx.readelf(&["--dyn-syms", "-W"], "libl.so");
        for name in [" f", " d", " g", " via_f"] {
            assert!(syms.lines().any(|l| l.ends_with(name)), "{words:?} {name}: {syms}");
        }
        // `f` listed: the interposer reaches the library's call; `d` not
        // listed: the library reads its own.
        let out = cx.run_host("host.c", &["-DINTERPOSE"], "libl.so", "dl");
        assert_eq!(out, "2 10 7\n", "{words:?}");
    }
    // In an executable the listed definitions are exported, and only they.
    cx.scratch
        .write("exe.c", "int f(void) { return 1; }\nint d = 2;\nint main(void) { return 0; }\n");
    cx.ok(&["-O1", "-fPIE", "-c", "exe.c", "-o", "exe.o"]);
    let o = cx.gcc(&[
        cx.b.as_str(),
        "-pie",
        "exe.o",
        "-Wl,--dynamic-list,f.list",
        "-Wl,--mortar-map=exe.mortar-map",
        "-o",
        "exe",
    ]);
    assert!(o.success(), "{}", o.stderr_text());
    verify_but_the_mirror(cx.scratch.path(), "exe");
    let syms = cx.readelf(&["--dyn-syms", "-W"], "exe");
    assert!(syms.lines().any(|l| l.ends_with(" f")), "{syms}");
    assert!(!syms.lines().any(|l| l.ends_with(" d") || l.ends_with(" main")), "{syms}");
}

/// [`verify`], but for M9's findings on the line's words: the verifier's
/// option mirror (`crates/mortar_plumb/src/opts.rs`) predates WP-128's
/// rows and is regenerated at the merge, after which this is [`verify`].
fn verify_but_the_mirror(dir: &Path, out: &str) {
    let findings: Vec<String> =
        support::plumb::verify_staged(&dir.join(out), &dir.join(format!("{out}.mortar-map")), dir)
            .iter()
            .map(|f| format!("{f:?}"))
            .filter(|f| !f.contains("rule: Map(9)"))
            .collect();
    assert!(findings.is_empty(), "plumb on {out}: {findings:#?}");
}

/// The `.dynamic` tags of `file`, in order.
fn tags(cx: &Cx, file: &str) -> Vec<String> {
    cx.readelf(&["-dW"], file)
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1).filter(|t| t.starts_with('(')))
        .map(str::to_owned)
        .collect()
}

/// The `DT_FLAGS` and `DT_FLAGS_1` lines of `file`, without the tag value.
fn values(cx: &Cx, file: &str) -> Vec<String> {
    cx.readelf(&["-dW"], file)
        .lines()
        .filter(|l| l.contains("(FLAGS"))
        .map(|l| l.split_whitespace().skip(1).collect::<Vec<_>>().join(" "))
        .collect()
}
