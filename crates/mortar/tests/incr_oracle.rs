//! The two-full-link oracle lane of the dry run (`spec/incr.md` IN-63a,
//! WP-063): `tests/incr-oracle/run.py --quick` over the quick subset of
//! its scenarios. Every unit that differs between the two full links must
//! be in the `--incremental-report` refill set. The whole set, at O0 and
//! O2, and the mutation table (IN-63b) are `cargo xtask incr-oracle`.
//!
//! Until WP-225 M6 the subject is a testhooks build of mortar: the dry run
//! is computed from the format-4 side state only such a build writes, under
//! `MORTAR_INCR_STATE4=1` (run.py sets it).

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

/// The testhooks mortar (WP-225, rulings M1-1): this test's own binary when
/// the test build has the feature; otherwise the crate built with it into
/// `<target>/testhooks`, the directory and flags `cargo xtask incr-update`
/// builds it with (one build serves both).
#[cfg(feature = "testhooks")]
fn subject() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mortar"))
}

#[cfg(not(feature = "testhooks"))]
fn subject() -> PathBuf {
    // `<target>/<profile>/mortar`
    let exe = Path::new(env!("CARGO_BIN_EXE_mortar"));
    let target = exe.parent().and_then(Path::parent).expect("the binary's target directory");
    let dir = target.join("testhooks");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = std::process::Command::new(cargo);
    cmd.args(["build", "--quiet", "--package", "mortar", "--features", "testhooks"])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .env("CARGO_TARGET_DIR", &dir)
        .env("CARGO_PROFILE_DEV_DEBUG", "0");
    let out = run_with(&mut cmd, b"", Duration::from_mins(30));
    assert!(out.success(), "cargo build --features testhooks:\n{}", out.stderr_text());
    dir.join("debug").join("mortar")
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
    let subject = subject();
    let work = Scratch::new("incr-oracle");
    let mut cmd = std::process::Command::new(python);
    cmd.arg(lane().join("run.py"))
        .arg("--quick")
        .arg("--mutations")
        .args(["-j", "2", "--mortar"])
        .arg(&subject)
        .arg("--work")
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
