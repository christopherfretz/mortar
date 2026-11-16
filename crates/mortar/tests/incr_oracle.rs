//! The two-full-link oracle lane of the dry run (`spec/incr.md` IN-63a,
//! WP-063): `tests/incr-oracle/run.py --quick` over the quick subset of
//! its scenarios. Every unit that differs between the two full links must
//! be in the `--incremental-report` refill set. The whole set, at O0 and
//! O2, and the mutation table (IN-63b) are `cargo xtask incr-oracle`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_with, tool};

fn lane() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/incr-oracle")
}

/// The tools run.py calls, by the names the tool guard knows: the
/// compilers and `ar` of both 64-bit architectures. `None` (with the
/// guard's note) when one is missing.
fn tools() -> Option<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    for arch in [Arch::Aarch64, Arch::X86_64] {
        for t in ["gcc", "g++", "ar"] {
            let name = if arch.is_host() { t.to_owned() } else { format!("{}-{t}", arch.triple()) };
            out.push((name, arch.tool(t)?));
        }
    }
    Some(out)
}

#[test]
fn the_dry_run_covers_every_differing_unit() {
    let Some(python) = tool("python3") else { return };
    let Some(host) = Arch::host() else { return };
    if host == Arch::I386 {
        mortar_testutil::named_skip(
            "incr_oracle::the_dry_run_covers_every_differing_unit",
            "the lane compiles for AArch64 and x86-64 with the host's tools",
        );
        return;
    }
    let Some(tools) = tools() else { return };
    let work = Scratch::new("incr-oracle");
    let mut cmd = std::process::Command::new(python);
    cmd.arg(lane().join("run.py"))
        .arg("--quick")
        .arg("--mutations")
        .args(["-j", "2", "--mortar", env!("CARGO_BIN_EXE_mortar"), "--work"])
        .arg(work.path());
    for (name, path) in &tools {
        cmd.arg("--tool").arg(format!("{name}={}", path.display()));
    }
    let out = run_with(&mut cmd, b"", Duration::from_mins(15));
    let text = out.stdout_text();
    eprintln!("{text}{}", out.stderr_text());
    assert!(out.success(), "the oracle lane failed (above)");
    // The names of the report's rows and the oracle's agree: each row
    // dropped is judged (a name the oracle did not know would be judged
    // as nothing dropped, and caught by nothing).
    for (row, _, _) in mortar_incr::delta::ROWS {
        assert!(text.contains(&format!("MUTATION {row} ")), "the oracle does not know row {row}");
    }
}
