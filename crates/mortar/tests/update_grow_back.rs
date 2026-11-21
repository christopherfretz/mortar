//! WP-279: a function that shrinks and then grows back into its region,
//! through the `mortar` binary, static C++ at `-O0`, AArch64 and x86-64
//! by host-aware drivers (`spec/incr.md` IN-72 item 2, IN-3a, IN-78).
//!
//! `c.cc.o` has `.text` (`c_value`, the edited function, and `c_other`)
//! and, after it in its region, the COMDAT section of the inline
//! `c_helper` (`.text._Z8c_helperi`, the same `.text` category). The
//! shrink frees the tail of `.text`: a `Fill` hole INSIDE the file's
//! region, not at its end (protoc's `main.cc.o`, WP-274 section 4.2).
//!
//! Hops: the full link of the grown body; the shrink; the grow back; the
//! shrink again. Every hop after the first is an update, taken; the
//! state's `REGIONS` are the full link's at every hop (the file keeps its
//! region: IN-72 item 2); the program prints the new values. The grow
//! back's output equals the full link of the same inputs byte for byte
//! (its layout is the full link's); a shrink's output keeps the freed
//! tail as a hole, so it is not byte-equal to a full link that packs it:
//! the verifier judges it, and the full link of the same inputs prints the
//! same.
//!
//! At 1bfce906 the shrink's capture split the region at the freed tail
//! (`region_rows`: a run ended at a `Fill` hole), the grow back filled
//! the first part to its end next to the second, and the update was
//! refused `update-not-implemented (layout)` (WP-255g S2-1, invariant I).

mod support;

use std::process::Command;
use std::time::Duration;

use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_with, tools_required};
use support::{Source, build_by, with_output};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

const FLAGS: [&str; 1] = ["-O0"];

const MAIN: &str = "#include <cstdio>\nint c_value(int);\nint c_other(int);\nint u_value(int);\n\
int main(){ std::printf(\"c_value=%d c_other=%d u_value=%d\\n\", c_value(2), c_other(5), u_value(4)); return 0; }\n";
/// After `c.cc.o`: its code keeps its place.
const U: &str = "int u_value(int x){ return x * 3; }\n";
/// `c_other` follows `c_value` in `.text`; `c_helper`'s COMDAT section
/// follows `.text`.
macro_rules! tail {
    () => {
        "inline int c_helper(int x){ return x * 5 - 4; }\nint c_other(int x){ return c_helper(x) - 2; }\n"
    };
}
/// The grown body.
const BIG: &str = concat!(
    "int c_value(int x){ volatile int k = x * 7 + 1; k += 3; k += 5; return k; }\n",
    tail!()
);
/// The shrunk body.
const SMALL: &str =
    concat!("int c_value(int x){ volatile int k = x * 7 + 1; return k; }\n", tail!());

/// One program on one target: its scratch and the line.
struct Prog {
    arch: Arch,
    cxx: String,
    scratch: Scratch,
    args: Vec<String>,
}

impl Prog {
    fn new(arch: Arch) -> Option<Self> {
        let cxx = arch.gnu("g++");
        let scratch = Scratch::new(&format!("wp279-growback-{}", arch.name()));
        let sources = [
            Source { name: "main.cc", text: MAIN },
            Source { name: "c.cc", text: BIG },
            Source { name: "u.cc", text: U },
        ];
        let Some(b) = build_by(&scratch, &cxx, &FLAGS, &sources) else {
            assert!(!tools_required(), "{cxx} is required");
            return None;
        };
        Some(Self { arch, cxx, scratch, args: b.args })
    }

    /// `mortar` on the line, to `out` (its machine map `out.map`), with
    /// `extra`: success and standard error.
    fn link(&self, out: &str, extra: &[&str]) -> (bool, String) {
        let mut c = Command::new(MORTAR);
        c.args(with_output(&self.args, &self.scratch.join(out)))
            .arg("--mortar-map")
            .arg(self.scratch.join(format!("{out}.map")))
            .args(extra)
            .current_dir(self.scratch.path())
            .env("MORTAR_UPDATE_FINDINGS", self.scratch.join("findings.txt"));
        let o = run_with(&mut c, b"", Duration::from_mins(5));
        (o.success(), o.stderr_text())
    }

    /// `c.cc.o` rebuilt from `text`, as a new inode (a compiler's write).
    fn rebuild_c(&self, text: &str) {
        let src = self.scratch.write("c.cc", text);
        let tmp = self.scratch.join("c.cc.o.new");
        let mut c = Command::new(&self.cxx);
        c.args(FLAGS).arg("-c").arg(&src).arg("-o").arg(&tmp);
        let o = run_with(&mut c, b"", Duration::from_mins(2));
        assert!(o.success(), "{}: {}", self.cxx, o.stderr_text());
        std::fs::rename(&tmp, self.scratch.join("c.cc.o")).expect("rename c.cc.o");
    }

    fn bytes(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.scratch.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    /// The `REGIONS` rows of the state `o` wrote.
    fn regions(
        &self,
        label: &str,
    ) -> Vec<(mortar_core::FileId, mortar_incr::update::rows::RegionRow)> {
        let bytes = self.bytes("o.mortar-state");
        mortar_incr::state5::read::decode(&bytes)
            .unwrap_or_else(|e| panic!("{label}: the state: {e:?}"))
            .regions
    }

    /// An update of `o`, taken, with no finding.
    fn update(&self, label: &str) {
        let (ok, err) = self.link("o", &["--incremental=update", "--incremental-report"]);
        let findings =
            std::fs::read_to_string(self.scratch.join("findings.txt")).unwrap_or_default();
        assert!(ok, "{label}: not an update: {err}\nfindings:\n{findings}");
        assert!(err.contains("incremental: update ("), "{label}: {err}");
        let bad: Vec<&str> = findings.lines().filter(|l| l.contains("finding")).collect();
        assert!(bad.is_empty(), "{label}: FINDINGS:\n{}", bad.join("\n"));
    }

    /// The full link of the current inputs, to `full`.
    fn full(&self, label: &str) {
        let (ok, err) = self.link("full", &["--incremental=full"]);
        assert!(ok, "{label}: the full link: {err}");
    }

    /// The program's standard output (under its emulator off the host).
    fn run(&self, out: &str) -> String {
        let mut c = self.arch.command(&self.scratch.join(out), None);
        let o = run_with(&mut c, b"", Duration::from_mins(2));
        assert!(!o.timed_out(), "{out}: TIMEOUT");
        assert!(o.success(), "{out}: {}", o.stderr_text());
        o.stdout_text()
    }
}

const PRINT_BIG: &str = "c_value=23 c_other=19 u_value=12\n";
const PRINT_SMALL: &str = "c_value=15 c_other=19 u_value=12\n";

// spec: incr.md IN-72 item 2 (the file keeps every region it has had in
// (O, c)), IN-3a (a section's freed tail is a hole, exactly like slack),
// IN-78 (the state an update writes is the capture of the full link with
// its placement): shrink, grow back, shrink again: every hop an update,
// taken, with the full link's regions.
#[test]
fn a_function_grows_back_into_its_region() {
    let mut ran = 0;
    for arch in [Arch::Aarch64, Arch::X86_64] {
        let Some(p) = Prog::new(arch) else { continue };
        ran += 1;
        let label = |hop: &str| format!("{} {hop}", arch.name());
        let (ok, err) = p.link("o", &["--incremental"]);
        assert!(ok, "{}: the first link: {err}", label("full"));
        assert_eq!(p.run("o"), PRINT_BIG, "{}", label("full"));
        let first = p.regions(&label("full"));
        for (hop, body, print) in [
            ("shrink", SMALL, PRINT_SMALL),
            ("grow back", BIG, PRINT_BIG),
            ("shrink again", SMALL, PRINT_SMALL),
        ] {
            let label = label(hop);
            p.rebuild_c(body);
            p.update(&label);
            assert_eq!(p.regions(&label), first, "{label}: the file keeps its region");
            assert_eq!(p.run("o"), print, "{label}");
            p.full(&label);
            assert_eq!(p.run("full"), print, "{label}: the full link");
            if body == BIG {
                let (got, want) = (p.bytes("o"), p.bytes("full"));
                if let Some(i) = got.iter().zip(&want).position(|(a, b)| a != b) {
                    panic!("{label}: the output differs from the full link at {i:#x}");
                }
                assert_eq!(got.len(), want.len(), "{label}: sizes");
            } else {
                let dir = p.scratch.path();
                let found = support::plumb::verify(&dir.join("o"), &dir.join("o.map"), dir);
                assert!(found.is_empty(), "{label}: the verifier: {found:#?}");
            }
        }
    }
    assert!(ran == 2 || !tools_required(), "{ran} of the 2 targets");
}
