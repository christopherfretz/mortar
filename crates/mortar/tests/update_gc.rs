//! WP-225 gc-lift: the update under `--gc-sections` (IN-70 U6, IN-71a),
//! through `update::run_with` (the `mortar` binary), static C++ with
//! `-ffunction-sections -fdata-sections`, AArch64 and x86-64 by host-aware
//! drivers.
//!
//! - An edit that keeps every section's liveness: an update, `live::live`
//!   judged against the front's mark (`gc-lift: live agrees`, 0 flips),
//!   the delta's placement judged under `--gc-sections` too (M2b-2d: no
//!   producer refuses, `layout_in`'s `(live)` lifted; the identity case
//!   requires the full link's offsets), the output byte-equal to the full
//!   link of the new inputs, and the program runs.
//! - A function's last caller removed (`helper`'s section in the unchanged
//!   `a.cc.o` dies): `live` agrees with the front, the section flips, so
//!   the placement is not the stored one: `update-not-implemented
//!   (layout)` by name (until M2b-3); the delta's placement is refused by
//!   name first, since `live` draws in `a.cc.o`, whose layout inputs are
//!   not produced (M2b-2d, gc-3); the full link that follows equals the
//!   full link of the new inputs.
//! - The same edit with `-g`: `new-output-section` (G-4: its debug output
//!   sections are new).
//!
//! No line of the findings file (`MORTAR_UPDATE_FINDINGS`) is a FINDING.
//! `UPDATE_GC_KEEP=<dir>` copies each output and its machine map there
//! (for `cargo xtask plumb`).

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_with, tools_required};
use support::{Source, build_by, with_output};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

const FLAGS: [&str; 3] = ["-O1", "-ffunction-sections", "-fdata-sections"];

const MAIN: &str = "#include <cstdio>\nint user(int);\nint keep(int);\n\
int main(int c, char **){ std::printf(\"user=%d keep=%d\\n\", user(c + 4), keep(c)); return 0; }\n";
const A: &str = "int helper(int x){ return x * 3; }\nint keep(int x){ return x - 1; }\n";
/// v0: `user` is `helper`'s only caller.
const B0: &str = "int helper(int);\nint user(int x){ return helper(x) + 5; }\n";
/// Every section's liveness kept (the same call, another immediate).
const B_KEEP: &str = "int helper(int);\nint user(int x){ return helper(x) + 7; }\n";
/// `helper`'s last caller removed.
const B_LAST: &str = "int user(int x){ return x + 5; }\n";

/// One program on one target: its scratch, the line, the findings file.
struct Prog {
    arch: Arch,
    cxx: String,
    /// `None` once [`Prog::keep`] left it for the verifier.
    scratch: Option<Scratch>,
    dir: PathBuf,
    args: Vec<String>,
    findings: PathBuf,
}

impl Prog {
    fn new(arch: Arch, case: &str) -> Option<Self> {
        let cxx = arch.gnu("g++");
        let scratch = Scratch::new(&format!("gclift-{case}-{}", arch.name()));
        let sources = [
            Source { name: "main.cc", text: MAIN },
            Source { name: "a.cc", text: A },
            Source { name: "b.cc", text: B0 },
        ];
        let Some(b) = build_by(&scratch, &cxx, &FLAGS, &sources) else {
            assert!(!tools_required(), "{cxx} is required");
            return None;
        };
        let mut args = b.args;
        args.push("--gc-sections".into());
        let findings = scratch.join("findings.txt");
        Some(Self { arch, cxx, scratch: Some(scratch), dir: b.dir, args, findings })
    }

    /// `mortar` on the line, to `out` (its machine map `out.map`), with
    /// `extra`: success and standard error.
    fn link(&self, out: &str, extra: &[&str]) -> (bool, String) {
        let mut c = Command::new(MORTAR);
        c.args(with_output(&self.args, &self.dir.join(out)))
            .arg("--mortar-map")
            .arg(self.dir.join(format!("{out}.map")))
            .args(extra)
            .current_dir(&self.dir)
            .env("MORTAR_UPDATE_FINDINGS", &self.findings);
        let o = run_with(&mut c, b"", Duration::from_mins(5));
        (o.success(), o.stderr_text())
    }

    /// `b.cc.o` rebuilt from `text` with `extra` flags, as a new inode (a
    /// compiler's write).
    fn rebuild_b(&self, text: &str, extra: &[&str]) {
        let src = self.dir.join("b.cc");
        std::fs::write(&src, text).expect("b.cc");
        let tmp = self.dir.join("b.cc.o.new");
        let mut c = Command::new(&self.cxx);
        c.args(FLAGS).args(extra).arg("-c").arg(&src).arg("-o").arg(&tmp);
        let o = run_with(&mut c, b"", Duration::from_mins(2));
        assert!(o.success(), "{}: {}", self.cxx, o.stderr_text());
        std::fs::rename(&tmp, self.dir.join("b.cc.o")).expect("rename b.cc.o");
    }

    fn bytes(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    /// The output `out` equals the full link of the current inputs
    /// (`--incremental=full`, to `full`) byte for byte.
    fn equals_full_link(&self, out: &str, label: &str) {
        let (ok, err) = self.link("full", &["--incremental=full"]);
        assert!(ok, "{label}: the full link: {err}");
        let (got, want) = (self.bytes(out), self.bytes("full"));
        if let Some(i) = got.iter().zip(&want).position(|(a, b)| a != b) {
            panic!("{label}: the output differs from the full link at {i:#x}");
        }
        assert_eq!(got.len(), want.len(), "{label}: sizes");
    }

    /// The program's standard output (under its emulator off the host).
    fn run(&self, out: &str) -> String {
        let mut c = self.arch.command(&self.dir.join(out), None);
        let o = run_with(&mut c, b"", Duration::from_mins(2));
        assert!(!o.timed_out(), "{out}: TIMEOUT");
        assert!(o.success(), "{out}: {}", o.stderr_text());
        o.stdout_text()
    }

    fn findings(&self) -> String {
        std::fs::read_to_string(&self.findings).unwrap_or_default()
    }

    /// `UPDATE_GC_KEEP`: the output and its map copied there, and the
    /// scratch directory (the inputs the map names) left in place, its
    /// path in `<label>.dir`, for the verifier (removed by hand).
    fn keep(&mut self, out: &str, label: &str) {
        let Some(dir) = std::env::var_os("UPDATE_GC_KEEP") else { return };
        let dir = Path::new(&dir);
        std::fs::create_dir_all(dir).expect("UPDATE_GC_KEEP");
        let name = label.replace(' ', "-");
        for (from, to) in
            [(out.to_owned(), name.clone()), (format!("{out}.map"), format!("{name}.map"))]
        {
            std::fs::copy(self.dir.join(from), dir.join(to)).expect("a kept copy");
        }
        std::fs::write(dir.join(format!("{name}.dir")), self.dir.as_os_str().as_encoded_bytes())
            .expect("the kept directory's name");
        std::mem::forget(self.scratch.take());
    }
}

fn targets() -> [Arch; 2] {
    [Arch::Aarch64, Arch::X86_64]
}

/// No line of the findings file is a FINDING.
fn no_finding(p: &Prog, label: &str) -> String {
    let text = p.findings();
    let bad: Vec<&str> = text.lines().filter(|l| l.contains("finding")).collect();
    assert!(bad.is_empty(), "{label}: FINDINGS:\n{}", bad.join("\n"));
    text
}

// spec: incr.md IN-70 (U6), IN-71a (WP-225 gc-lift): under --gc-sections
// an edit that keeps every section's liveness is an update; `live` agrees
// with the front's mark; the output equals the full link of the new
// inputs; the program runs with the new value.
#[test]
fn gc_update_that_keeps_liveness_is_taken() {
    let mut ran = 0;
    for arch in targets() {
        let Some(mut p) = Prog::new(arch, "keep") else { continue };
        ran += 1;
        let label = format!("keep {}", arch.name());
        let (ok, err) = p.link("o", &["--incremental"]);
        assert!(ok, "{label}: the first link: {err}");
        assert_eq!(p.run("o"), "user=20 keep=0\n", "{label}: v0");
        p.rebuild_b(B_KEEP, &[]);
        let (ok, err) = p.link("o", &["--incremental=update", "--incremental-report"]);
        assert!(ok, "{label}: not an update: {err}\nfindings:\n{}", p.findings());
        assert!(err.contains("incremental: update ("), "{label}: {err}");
        let text = no_finding(&p, &label);
        assert!(
            text.lines().any(|l| l.starts_with("gc-lift: live agrees") && l.ends_with(" 0 flips")),
            "{label}: no judgement of live:\n{text}"
        );
        // M2b-2d: the placement ran to its judgement (no producer's
        // refusal recorded; in the identity case a judged placement at
        // another offset is a FINDING).
        assert!(
            !text.lines().any(|l| l.contains("fallback")),
            "{label}: the placement is not judged:\n{text}"
        );
        p.equals_full_link("o", &label);
        assert_eq!(p.run("o"), "user=22 keep=0\n", "{label}: v1");
        // A second update from the state the first one wrote (its
        // `META.live_hash` is the one `live`'s S4 check reads).
        p.rebuild_b(B0, &[]);
        let (ok, err) = p.link("o", &["--incremental=update", "--incremental-report"]);
        assert!(ok, "{label}: the second update: {err}\nfindings:\n{}", p.findings());
        no_finding(&p, &label);
        p.equals_full_link("o", &label);
        assert_eq!(p.run("o"), "user=20 keep=0\n", "{label}: v2");
        p.keep("o", &label);
    }
    assert!(ran == 2 || !tools_required(), "{ran} of the 2 targets");
}

// spec: incr.md IN-71a (WP-225 gc-lift): a function's last caller removed:
// `live` agrees with the front, the section flips and its unchanged file is
// drawn in (M2b-4 item 5: read with fresh ids, its layout inputs produced);
// the materializer then falls back by name, `(symtab)` (the file's local
// span or `.symtab`'s size changes; no `.symtab` plan is wired). The full
// link that follows is the full link of the new inputs.
#[test]
fn gc_update_whose_section_dies_falls_back_layout() {
    let mut ran = 0;
    for arch in targets() {
        let Some(mut p) = Prog::new(arch, "last") else { continue };
        ran += 1;
        let label = format!("last-caller {}", arch.name());
        let (ok, err) = p.link("o", &["--incremental"]);
        assert!(ok, "{label}: the first link: {err}");
        p.rebuild_b(B_LAST, &[]);
        let (ok, err) = p.link("o", &["--incremental=update", "--incremental-report"]);
        assert!(!ok, "{label}: updated: {err}");
        assert!(
            err.contains("cannot update: update-not-implemented (symtab)"),
            "{label}: {err}\nfindings:\n{}",
            p.findings()
        );
        let text = no_finding(&p, &label);
        let flips = text.lines().find(|l| l.starts_with("gc-lift: live agrees"));
        assert!(
            flips.is_some_and(|l| !l.ends_with(" 0 flips")),
            "{label}: no flip judged:\n{text}"
        );
        // M2b-4 item 5: the drawn-in file is read with fresh ids (its
        // layout inputs produced), and a later producer refuses by name.
        assert!(
            text.lines().any(|l| l.starts_with("M2b-4: placement: live draws in 1 unchanged")
                && l.contains("read with fresh ids")),
            "{label}: the drawn-in file is not read:\n{text}"
        );
        // Then the materializer: the drawn-in file's local span (its dead
        // section's locals go) or `.symtab`'s size differs from the stored
        // one, and no `.symtab` plan is wired (U-5): `(symtab)`, by name.
        assert!(
            text.lines().any(|l| l.starts_with("M2b-3 Part 3: materialize (")
                && l.ends_with("update-not-implemented (symtab)")),
            "{label}: no (symtab) refusal named:\n{text}"
        );
        // A fall-back's full link (`small-output` comes first here).
        let (ok, err) = p.link("o", &["--incremental", "--incremental-report"]);
        assert!(ok, "{label}: {err}");
        p.keep("o", &label);
        p.equals_full_link("o", &label);
        assert_eq!(p.run("o"), "user=10 keep=0\n", "{label}: v1");
    }
    assert!(ran == 2 || !tools_required(), "{ran} of the 2 targets");
}

// spec: incr.md IN-70 (the debug condition at the end of U2, before U6):
// the same edit with `-g` (debug sections that the old version has not):
// it falls back by name under --gc-sections too. G-4 (WP-230): U2's debug
// condition refuses a debug output section that appears, with no
// `OUTSECS` row, `new-output-section`, before path (A)'s front.
#[test]
fn gc_update_with_debug_falls_back_by_name() {
    let mut ran = 0;
    for arch in targets() {
        let Some(p) = Prog::new(arch, "debug") else { continue };
        ran += 1;
        let label = format!("debug {}", arch.name());
        let (ok, err) = p.link("o", &["--incremental"]);
        assert!(ok, "{label}: the first link: {err}");
        p.rebuild_b(B_LAST, &["-g"]);
        let (ok, err) = p.link("o", &["--incremental=update", "--incremental-report"]);
        assert!(!ok, "{label}: updated: {err}");
        assert!(err.contains("cannot update: new-output-section: .debug_"), "{label}: {err}");
    }
    assert!(ran == 2 || !tools_required(), "{ran} of the 2 targets");
}
