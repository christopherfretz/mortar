//! The number of D-12's stages (`spec/dyn.md` D-12, `spec/resolve.md`
//! §4.4; WP-167), read from `MORTAR_STEPS=1`'s `resolve count stages`
//! line, through the compiler driver on AArch64 and x86-64:
//!
//! - a line without a candidate (a default `g++` dynamic hello, whose
//!   libraries are `--as-needed` ones; a `-static` link) runs exactly
//!   one stage: the cost of the rule is nothing there;
//! - a line whose needed `--as-needed` library references a member of a
//!   later archive (`m.o --as-needed libu.so libg.a`) runs two, and
//!   links silently.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use mortar_testutil::{Outcome, Scratch, run_with, tool};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

const HELLO: &str = "#include <iostream>\nint main() { std::cout << \"hello\\n\"; }\n";
const LIBU: &str = "int g(void);\nint u(void) { return g() + 1; }\n";
const G: &str = "int g(void) { return 10; }\n";
const M: &str = "int u(void);\nint main(void) { return u() == 11 ? 0 : 1; }\n";

struct Cx {
    scratch: Scratch,
    _bin: Scratch,
    b: String,
    gxx: PathBuf,
}

impl Cx {
    fn new(t: &str) -> Option<Self> {
        let gxx = tool(&format!("{t}-linux-gnu-g++"))?;
        let bin = Scratch::new(&format!("as-needed-stages-{t}-bin"));
        std::os::unix::fs::symlink(MORTAR, bin.join("ld")).unwrap();
        let b = format!("-B{}/", bin.path().display());
        let cx = Self { scratch: Scratch::new(&format!("as-needed-stages-{t}")), _bin: bin, b, gxx };
        cx.scratch.write("hello.cc", HELLO);
        cx.scratch.write("u.c", LIBU);
        cx.scratch.write("g.c", G);
        cx.scratch.write("m.c", M);
        cx.ok(&["-x", "c", "-O1", "-fPIC", "-c", "u.c", "-o", "u.o"]);
        cx.ok(&["-fuse-ld=bfd", "-shared", "-Wl,-soname,libu.so", "u.o", "-o", "libu.so"]);
        cx.ok(&["-x", "c", "-O1", "-fPIC", "-c", "g.c", "-o", "g.o"]);
        cx.ok(&["-x", "c", "-O1", "-fPIC", "-c", "m.c", "-o", "m.o"]);
        cx.ok(&["-O1", "-c", "hello.cc", "-o", "hello.o"]);
        let ar = tool(&format!("{t}-linux-gnu-ar"))?;
        let mut c = Command::new(ar);
        c.args(["rcs", "libg.a", "g.o"]).current_dir(cx.scratch.path());
        assert!(run_with(&mut c, b"", Duration::from_mins(1)).success());
        Some(cx)
    }

    fn gxx(&self, args: &[&str]) -> Outcome {
        let mut c = Command::new(&self.gxx);
        c.args(args).current_dir(self.scratch.path());
        run_with(&mut c, b"", Duration::from_mins(2))
    }

    fn ok(&self, args: &[&str]) {
        let o = self.gxx(args);
        assert!(o.success(), "g++ {args:?}: {}", o.stderr_text());
    }

    /// Links `args` with mortar under `MORTAR_STEPS=1`; returns the
    /// stage count and the rest of stderr (the lines not `mortar: steps:`).
    fn stages(&self, args: &[&str]) -> (u64, String) {
        let mut c = Command::new(&self.gxx);
        c.arg(&self.b).args(args).current_dir(self.scratch.path()).env("MORTAR_STEPS", "1");
        let o = run_with(&mut c, b"", Duration::from_mins(2));
        let err = o.stderr_text();
        assert!(o.success(), "link {args:?}: {err}");
        let mut stages = None;
        let mut rest = String::new();
        for line in err.lines() {
            if let Some(n) = line.strip_prefix("mortar: steps: resolve count stages: ") {
                assert!(stages.is_none(), "two stage counts: {err}");
                stages = Some(n.trim().parse().unwrap());
            } else if !line.starts_with("mortar: steps:") {
                rest.push_str(line);
                rest.push('\n');
            }
        }
        (stages.unwrap_or_else(|| panic!("no stage count in: {err}")), rest)
    }
}

fn check(t: &str) {
    let Some(cx) = Cx::new(t) else { return };
    let (n, rest) = cx.stages(&["hello.o", "-o", "hello"]);
    assert_eq!((n, rest.as_str()), (1, ""), "{t}: a default dynamic hello");
    let (n, rest) = cx.stages(&["-static", "hello.o", "-o", "hello-static"]);
    assert_eq!((n, rest.as_str()), (1, ""), "{t}: a static hello");
    let (n, rest) =
        cx.stages(&["m.o", "-Wl,--no-as-needed", "-L.", "-lu", "-lg", "-o", "control"]);
    assert_eq!((n, rest.as_str()), (1, ""), "{t}: the line under --no-as-needed");
    let (n, rest) = cx.stages(&["m.o", "-Wl,--as-needed", "-L.", "-lu", "-lg", "-o", "row1"]);
    assert_eq!((n, rest.as_str()), (2, ""), "{t}: a needed --as-needed library's reference");
}

#[test]
fn stages_aarch64() {
    check("aarch64");
}

#[test]
fn stages_x86_64() {
    check("x86_64");
}
