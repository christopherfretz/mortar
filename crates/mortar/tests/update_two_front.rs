//! WP-225 M2b-3 Part 3: the two fronts (R-1).
//!
//! A non-identity update (one function grows, shrinks, grows again in its
//! file's region): front 1 makes the delta, `materialize` turns it into
//! the front's layout and `d_F`, front 2 replays it (its computed layout
//! checked against front 1's) and every chunk is refilled with `d_F`
//! (R-3). Each hop's output runs and prints what the full link's does,
//! and each hop either says `taken: front 2` (its report line
//! `incremental: update`) or falls back by name (a recorded reason). The lane
//! (`cargo xtask incr-update`, `grow_one_function`) judges the same hops
//! with the verifier and the equivalence oracle.
//!
//! On AArch64 and x86-64.

mod support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_testutil::target::Arch;
use mortar_testutil::{Outcome, Scratch, run, run_exec_with, run_with, tool, tools_required};
use support::{Driver, Source, build, build_by, with_output};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

const MAIN: &str = "#include <stdio.h>\nint c_value(int);\nint c_other(int);\nint u_value(int);\n\
    int main(void) { printf(\"%d %d %d\\n\", c_value(9), c_other(9), u_value(9)); return 0; }\n";
const SMALL: &str =
    "int c_value(int x) { return x * 7 + 1; }\nint c_other(int x) { return x - 2; }\n";
const GROWN: &str = "int c_value(int x) {\n  int y = x * 7 + 1;\n  if (y > 100)\n    y -= 3;\n\
    for (int i = 0; i < x; ++i)\n    y += i & 1;\n  return y - x / 2;\n}\n\
    int c_other(int x) { return x - 2; }\n";
const U: &str = "int u_value(int x) { return x + 40; }\n";

struct Prog {
    _scratch: Scratch,
    label: &'static str,
    arch: Arch,
    dir: PathBuf,
    args: Vec<String>,
    cc: PathBuf,
    flags: Vec<String>,
}

impl Prog {
    fn of(label: &'static str, arch: Arch) -> Option<Self> {
        let s = Scratch::new(if arch == Arch::Aarch64 { "wp225p3-a64" } else { "wp225p3-x64" });
        let sources = [
            Source { name: "main.c", text: MAIN },
            Source { name: "c.c", text: SMALL },
            Source { name: "u.c", text: U },
        ];
        let (built, cc, flags) = if arch == Arch::Aarch64 {
            let d = Driver::Gcc;
            (build(&s, d, &["-O1"], &sources), d.tool(), {
                let mut f = d.target_args();
                f.push("-O1".into());
                f
            })
        } else {
            let cc = arch.gnu("gcc");
            (build_by(&s, &cc, &["-O1"], &sources), cc, vec!["-O1".into()])
        };
        let Some(b) = built else {
            assert!(!tools_required(), "{cc} is required");
            return None;
        };
        let cc = tool(&cc).expect("the compiler");
        Some(Self { _scratch: s, label, arch, dir: b.dir, args: b.args, cc, flags })
    }

    fn compile_c(&self, text: &str) {
        let src = self.dir.join("c.c");
        std::fs::write(&src, text).expect("c.c");
        let mut args: Vec<OsString> = self.flags.iter().map(OsString::from).collect();
        args.extend([
            "-c".into(),
            src.into_os_string(),
            "-o".into(),
            self.dir.join("c.c.o").into(),
        ]);
        let o = run(&self.cc, &args);
        assert!(o.success(), "{}: c.c: {}", self.label, o.stderr_text());
    }

    fn link(&self, out: &str, extra: &[&str], findings: &Path) -> Outcome {
        let mut a = with_output(&self.args, &self.dir.join(out));
        a.extend(extra.iter().map(OsString::from));
        let mut c = Command::new(MORTAR);
        c.args(&a).current_dir(&self.dir).env_remove("MORTAR_SCHED_SEED");
        c.env_remove("SOURCE_DATE_EPOCH").env("MORTAR_UPDATE_FINDINGS", findings);
        // A testhooks build: no `small-output` bound (the lane's program size).
        c.env("MORTAR_INCR_SMALL_OUTPUT", "0");
        let o = run_with(&mut c, b"", Duration::from_mins(2));
        assert!(o.success(), "{}: mortar {extra:?}: {}", self.label, o.stderr_text());
        o
    }

    fn output(&self, out: &str) -> Vec<u8> {
        let runme = self.dir.join("runme");
        std::fs::copy(self.dir.join(out), &runme).expect("copy");
        let (exe, args) = self.arch.runner(&runme);
        let o = run_exec_with(
            || {
                let mut c = Command::new("sh");
                c.arg("-c").arg("ulimit -c 0; exec \"$0\" \"$@\"").arg(&exe).args(&args);
                c.current_dir(&self.dir);
                c
            },
            b"",
            Duration::from_mins(1),
        );
        assert_eq!(o.status.and_then(|s| s.code()), Some(0), "{}: {out} runs", self.label);
        o.stdout.clone()
    }
}

#[test]
fn a_grown_function_is_taken_through_front_two_or_refused_by_name() {
    let mut taken = Vec::new();
    for (label, arch) in [("aarch64 gcc", Arch::Aarch64), ("x86-64 gcc", Arch::X86_64)] {
        let Some(p) = Prog::of(label, arch) else { continue };
        let findings = p.dir.join("findings.txt");
        p.link("u", &["--incremental=full"], &findings);
        for (hop, text) in [(1, GROWN), (2, SMALL), (3, GROWN)] {
            p.compile_c(text);
            std::fs::write(&findings, b"").expect("findings");
            let rep = p.dir.join("rep.txt");
            std::fs::write(&rep, b"").expect("rep.txt");
            let report = format!("--incremental-report={}", rep.display());
            let o = p.link("u", &["--incremental", &report], &findings);
            let lines = std::fs::read_to_string(&findings).expect("findings");
            let err = std::fs::read_to_string(&rep).expect("rep.txt");
            assert!(!o.stderr_text().contains("incremental:"), "{label}: {}", o.stderr_text());
            let front2 = lines.lines().any(|l| l.starts_with("M2b-3 Part 3: taken: front 2"));
            let fell_back = !err.starts_with("incremental: update");
            assert!(
                !lines.lines().any(|l| l.contains("FINDING")),
                "{label}: hop {hop}: a finding: {lines}"
            );
            if front2 {
                assert!(!fell_back, "{label}: hop {hop}: taken and fell back: {err}\n{lines}");
                taken.push(format!("{label} hop {hop}"));
            } else if fell_back {
                // Named in the findings, or in the report's reason
                // (`small-output` without the testhooks build).
                let named = err.starts_with("incremental: full (")
                    && !err.starts_with("incremental: full ()");
                assert!(
                    !lines.trim().is_empty() || named,
                    "{label}: hop {hop}: a fall-back with no name: {err}"
                );
            }
            // The full link of the same inputs: the same program output.
            p.link("full", &[], &findings);
            assert_eq!(p.output("u"), p.output("full"), "{label}: hop {hop}: the output");
        }
    }
    // The shrink (hop 2) and the second growth (hop 3) are taken on both
    // targets (the lane's `grow_one_function`, at this commit); without
    // the testhooks build every hop is `small-output`.
    if cfg!(feature = "testhooks") {
        for label in ["aarch64 gcc", "x86-64 gcc"] {
            for hop in [2, 3] {
                let want = format!("{label} hop {hop}");
                assert!(
                    taken.contains(&want) || !tools_required(),
                    "{want} was not taken through front 2: {taken:?}"
                );
            }
        }
    }
}
