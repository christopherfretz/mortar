//! WP-225 M2b-4 item 4 (finding P3-4): the `incremental:` line without
//! `--incremental-report` (`spec/diag.md`: a link that falls back says one
//! line, `incremental: full (<reason>)`, on standard error; `Session::
//! emit_report`).
//!
//! A successful update says nothing: before the fix its line was
//! `incremental: full (update-not-implemented)` (the session's reason,
//! `UPDATABLE`, whatever the line). A link that falls back still says its
//! reason.
//!
//! An update is taken only in a testhooks build (no `small-output` bound,
//! `MORTAR_INCR_SMALL_OUTPUT=0`); without it the update refuses
//! `small-output` by name and the fall-back's line is checked alone.
//!
//! On AArch64 and x86-64.

mod support;

use std::ffi::OsString;
use std::time::Duration;

use mortar_testutil::target::Arch;
use mortar_testutil::{Outcome, Scratch, run, run_with, tool, tools_required};
use support::{Driver, Source, build, build_by, with_output};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

const MAIN: &str = "#include <stdio.h>\nint c_value(int);\n\
    int main(void) { printf(\"%d\\n\", c_value(9)); return 0; }\n";
const V0: &str = "int c_value(int x) { return x * 7 + 1; }\n";
// The same size: the identity case of the update.
const V1: &str = "int c_value(int x) { return x * 7 + 2; }\n";

struct Prog {
    _scratch: Scratch,
    dir: std::path::PathBuf,
    args: Vec<String>,
    cc: std::path::PathBuf,
    flags: Vec<String>,
}

impl Prog {
    fn of(arch: Arch) -> Option<Self> {
        let s = Scratch::new(if arch == Arch::Aarch64 { "wp225m4-a64" } else { "wp225m4-x64" });
        let sources = [Source { name: "main.c", text: MAIN }, Source { name: "c.c", text: V0 }];
        let (built, cc, flags) = if arch == Arch::Aarch64 {
            let d = Driver::Gcc;
            let mut f = d.target_args();
            f.push("-O1".into());
            (build(&s, d, &["-O1"], &sources), d.tool(), f)
        } else {
            let cc = arch.gnu("gcc");
            (build_by(&s, &cc, &["-O1"], &sources), cc, vec!["-O1".into()])
        };
        let Some(b) = built else {
            assert!(!tools_required(), "{cc} is required");
            return None;
        };
        let cc = tool(&cc).expect("the compiler");
        Some(Self { _scratch: s, dir: b.dir, args: b.args, cc, flags })
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
        assert!(o.success(), "c.c: {}", o.stderr_text());
    }

    fn link(&self, extra: &[&str]) -> Outcome {
        let mut a = with_output(&self.args, &self.dir.join("u"));
        a.extend(extra.iter().map(OsString::from));
        let mut c = std::process::Command::new(MORTAR);
        c.args(&a).current_dir(&self.dir).env_remove("MORTAR_SCHED_SEED");
        c.env_remove("SOURCE_DATE_EPOCH").env_remove("MORTAR_UPDATE_FINDINGS");
        c.env("MORTAR_INCR_SMALL_OUTPUT", "0");
        run_with(&mut c, b"", Duration::from_mins(2))
    }
}

// spec: diag.md (the fall-back's one line), incr.md IN-10 (the report's
// words): a taken update without `--incremental-report` prints no
// `incremental: full`; a fall-back prints its reason.
#[test]
fn a_taken_update_says_no_full_link_without_the_report() {
    let mut ran = 0;
    let mut taken = 0;
    for arch in [Arch::Aarch64, Arch::X86_64] {
        let Some(p) = Prog::of(arch) else { continue };
        ran += 1;
        let label = arch.name();
        let o = p.link(&["--incremental=full"]);
        assert!(o.success(), "{label}: the first link: {}", o.stderr_text());
        p.compile_c(V1);
        let o = p.link(&["--incremental=update"]);
        let err = o.stderr_text();
        if o.success() {
            taken += 1;
            assert!(!err.contains("incremental:"), "{label}: a taken update says: {err}");
        } else {
            // No update to take (a build without testhooks: `small-output`):
            // refused by name, as the error.
            assert!(err.contains("cannot update: "), "{label}: {err}");
            let hooks = cfg!(feature = "testhooks");
            assert!(!hooks, "{label}: the update was not taken: {err}");
        }
        // A fall-back (the options changed) still says its reason.
        let o = p.link(&["--incremental", "--gc-sections"]);
        assert!(o.success(), "{label}: {}", o.stderr_text());
        assert!(
            o.stderr_text().contains("incremental: full (options-changed"),
            "{label}: the fall-back says: {}",
            o.stderr_text()
        );
    }
    assert!(ran == 2 || !tools_required(), "{ran} of the 2 targets");
    assert!(taken == ran || !cfg!(feature = "testhooks"), "{taken} of {ran} updates taken");
}
