//! WP-267 P-5: the update on ONE front (front 2, the update's placement
//! replayed); front 1 runs only in shadow, when findings are recorded,
//! after the decision.
//!
//! - an identity hop goes through front 2 (`taken: front 2 (identity`),
//!   its bytes equal to a full link's of the same inputs, and a hop back
//!   to the first sources gives the stored output's bytes again;
//! - R-A1 over front 2's capture is binding: one stored row changed (an
//!   `INDEX` row of a file the hop does not change) makes the check
//!   disagree with identity', and the update refills every chunk (its
//!   output equal to the full link's, its state front 2's capture);
//! - with findings unset, front 1 does not run
//!   (`mortar::update::FRONT_ONE_RUNS`).
//!
//! On AArch64 and x86-64.

mod support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::time::Duration;

use mortar_testutil::target::Arch;
use mortar_testutil::{Outcome, Scratch, run, run_exec_with, run_with, tool, tools_required};
use support::{Driver, Source, build, build_by, with_output};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

const MAIN: &str = "#include <stdio.h>\nint c_value(int);\nint c_other(int);\nint u_value(int);\n\
    int main(void) { printf(\"%d %d %d\\n\", c_value(9), c_other(9), u_value(9)); return 0; }\n";
const SMALL: &str =
    "int c_value(int x) { return x * 7 + 1; }\nint c_other(int x) { return x - 2; }\n";
/// `SMALL` with one constant changed: the same sizes (an identity hop).
const SAME: &str =
    "int c_value(int x) { return x * 7 + 3; }\nint c_other(int x) { return x - 2; }\n";
/// `c_value` grown: not an identity hop.
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
    fn of(label: &'static str, arch: Arch, name: &str) -> Option<Self> {
        let s = Scratch::new(name);
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

    fn line(&self, out: &str, extra: &[&str]) -> Vec<OsString> {
        let mut a = with_output(&self.args, &self.dir.join(out));
        a.extend(extra.iter().map(OsString::from));
        a
    }

    /// The linker as a process, findings recorded to `findings`.
    fn link(&self, out: &str, extra: &[&str], findings: &Path) -> Outcome {
        let mut c = Command::new(MORTAR);
        c.args(self.line(out, extra)).current_dir(&self.dir).env_remove("MORTAR_SCHED_SEED");
        c.env_remove("SOURCE_DATE_EPOCH").env("MORTAR_UPDATE_FINDINGS", findings);
        // A testhooks build: no `small-output` bound (the lane's program size).
        c.env("MORTAR_INCR_SMALL_OUTPUT", "0");
        let o = run_with(&mut c, b"", Duration::from_mins(2));
        assert!(o.success(), "{}: mortar {extra:?}: {}", self.label, o.stderr_text());
        o
    }

    /// An update hop: `text` compiled, the update with its report; the
    /// findings and the report's text.
    fn hop(&self, text: &str) -> (String, String) {
        self.compile_c(text);
        let findings = self.dir.join("findings.txt");
        std::fs::write(&findings, b"").expect("findings");
        let rep = self.dir.join("rep.txt");
        std::fs::write(&rep, b"").expect("rep.txt");
        let report = format!("--incremental-report={}", rep.display());
        // `--incremental=update`: no `small-output` bound (U1) without
        // the testhooks build.
        let o = self.link("u", &["--incremental=update", &report], &findings);
        assert!(!o.stderr_text().contains("incremental:"), "{}: {}", self.label, o.stderr_text());
        let lines = std::fs::read_to_string(&findings).expect("findings");
        let err = std::fs::read_to_string(&rep).expect("rep.txt");
        (lines, err)
    }

    /// A full link of the same inputs into `full` (incremental mode, the
    /// layout an update keeps when no size changes), its bytes.
    fn full(&self) -> Vec<u8> {
        let findings = self.dir.join("full-findings.txt");
        self.link("full", &["--incremental=full"], &findings);
        std::fs::read(self.dir.join("full")).expect("full")
    }

    fn bytes(&self, out: &str) -> Vec<u8> {
        std::fs::read(self.dir.join(out)).expect("output")
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

fn targets() -> [(&'static str, Arch); 2] {
    [("aarch64 gcc", Arch::Aarch64), ("x86-64 gcc", Arch::X86_64)]
}

fn taken(lines: &str, how: &str) -> bool {
    lines.lines().any(|l| l.starts_with(&format!("M2b-3 Part 3: taken: front 2 ({how}")))
}

fn findings_of(lines: &str) -> Vec<&str> {
    lines.lines().filter(|l| l.contains("finding") || l.contains("FINDING")).collect()
}

// spec: incr.md R-A1 (the identity check over the capture of the front
// that wrote the output), WP-267 P-5 item 2 (one front).
#[test]
fn an_identity_hop_goes_through_front_two_with_the_stored_bytes() {
    for (label, arch) in targets() {
        let Some(p) = Prog::of(label, arch, "wp267p5-id") else { continue };
        let findings = p.dir.join("findings.txt");
        p.link("u", &["--incremental=full"], &findings);
        let stored = p.bytes("u");
        let first = p.output("u");
        // Hop 1: one constant changed, the same sizes.
        let (lines, err) = p.hop(SAME);
        assert!(err.starts_with("incremental: update"), "{label}: hop 1: {err}\n{lines}");
        assert!(taken(&lines, "identity"), "{label}: hop 1 not identity: {lines}");
        assert!(findings_of(&lines).is_empty(), "{label}: hop 1: {lines}");
        assert!(lines.contains("P-5 identity: agree (identity)"), "{label}: hop 1: {lines}");
        assert!(lines.contains("P-5 shadow: agree (outcome:"), "{label}: hop 1: {lines}");
        let updated = p.bytes("u");
        assert_ne!(updated, stored, "{label}: hop 1 changed nothing");
        assert!(updated == p.full(), "{label}: hop 1: the update is not the full link's bytes");
        assert_ne!(p.output("u"), first, "{label}: hop 1 prints what hop 0 does");
        // Hop 2: back to the first sources; the stored output's bytes.
        let (lines, err) = p.hop(SMALL);
        assert!(err.starts_with("incremental: update"), "{label}: hop 2: {err}\n{lines}");
        assert!(taken(&lines, "identity"), "{label}: hop 2 not identity: {lines}");
        assert!(findings_of(&lines).is_empty(), "{label}: hop 2: {lines}");
        assert!(p.bytes("u") == stored, "{label}: hop 2: not the stored output's bytes");
        assert_eq!(p.output("u"), first, "{label}: hop 2's output");
    }
}

/// The state beside `out` with one `INDEX` row of `keep` (a file the hop
/// does not change) given another row's target: a row front 2's capture
/// writes otherwise, and that only R-A1 compares.
fn mutate_one_row(p: &Prog, out: &str, keep: &str) {
    let path = p.dir.join(format!("{out}.mortar-state"));
    let bytes = std::fs::read(&path).expect("the state");
    let mut m = mortar_incr::state5::read::decode(&bytes).expect("the state decodes");
    let text = |s: mortar_incr::format::Str| {
        m.strings.get(s.off as usize..(s.off + s.len) as usize).unwrap_or_default().to_vec()
    };
    let file = m
        .files
        .iter()
        .position(|f| text(f.path).ends_with(keep.as_bytes()))
        .unwrap_or_else(|| panic!("{}: no FILES row of {keep}", p.label));
    let rows: Vec<usize> =
        (0..m.index.len()).filter(|&i| m.index[i].0.0 as usize == file).collect();
    let (a, b) = rows
        .iter()
        .flat_map(|&a| rows.iter().map(move |&b| (a, b)))
        .find(|&(a, b)| m.index[a].1.target != m.index[b].1.target)
        .unwrap_or_else(|| panic!("{}: {keep} has no two INDEX targets: {rows:?}", p.label));
    m.index[a].1.target = m.index[b].1.target;
    let again = mortar_incr::capture::state5_encode(&m).expect("the state encodes");
    std::fs::write(&path, again).expect("the state written");
}

// spec: incr.md R-A1 (BINDING over the capture), WP-267 P-5 item 2 (a
// disagreement with identity' refills every chunk from front 2).
#[test]
fn a_changed_stored_row_makes_the_check_refill_every_chunk() {
    for (label, arch) in targets() {
        let Some(p) = Prog::of(label, arch, "wp267p5-ra1") else { continue };
        let findings = p.dir.join("findings.txt");
        p.link("u", &["--incremental=full"], &findings);
        mutate_one_row(&p, "u", "main.c.o");
        let (lines, err) = p.hop(SAME);
        assert!(err.starts_with("incremental: update"), "{label}: {err}\n{lines}");
        assert!(
            lines.contains(
                "finding: P-5 identity: identity' says identity, the check over front 2 does not"
            ),
            "{label}: no R-A1 disagreement: {lines}"
        );
        assert!(taken(&lines, "every chunk"), "{label}: not refilled: {lines}");
        assert!(p.bytes("u") == p.full(), "{label}: the update is not the full link's bytes");
        // The state written is front 2's capture: the next hop agrees.
        let (lines, err) = p.hop(SMALL);
        assert!(err.starts_with("incremental: update"), "{label}: hop 2: {err}\n{lines}");
        assert!(taken(&lines, "identity"), "{label}: hop 2 not identity: {lines}");
        assert!(findings_of(&lines).is_empty(), "{label}: hop 2: {lines}");
    }
}

// spec: WP-267 P-5 item 2 (front 1 runs only when findings are recorded).
#[test]
fn front_one_does_not_run_with_findings_unset() {
    if std::env::var_os("MORTAR_UPDATE_FINDINGS").is_some() {
        eprintln!("MORTAR_UPDATE_FINDINGS is set: front 1 runs in shadow, nothing to check");
        return;
    }
    for (label, arch) in targets() {
        let Some(p) = Prog::of(label, arch, "wp267p5-off") else { continue };
        let link = |extra: &[&str]| {
            let inv = mortar::invoke_in("mortar", &p.line("u", extra), &p.dir);
            assert!(
                matches!(inv.outcome, mortar::Outcome::Linked { .. }),
                "{label}: {:?}\n{}",
                inv.outcome,
                inv.incremental
            );
            inv
        };
        link(&["--incremental=full"]);
        let before = mortar::update::FRONT_ONE_RUNS.load(Ordering::Relaxed);
        for (hop, text) in [(1, SAME), (2, GROWN), (3, SMALL)] {
            p.compile_c(text);
            let inv = link(&["--incremental=update"]);
            assert!(
                !inv.incremental.contains("incremental: full"),
                "{label}: hop {hop} fell back: {}",
                inv.incremental
            );
            assert_eq!(
                mortar::update::FRONT_ONE_RUNS.load(Ordering::Relaxed),
                before,
                "{label}: hop {hop}: front 1 ran with findings unset"
            );
        }
    }
}
