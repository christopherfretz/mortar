//! `spec/harness.md` "Hosted CI": the workflows run the gate as the
//! development machine does, and the one list of reference tools,
//! `scripts/ci-tools.sh`, is what this machine has.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_testutil::{run_with, tool, tools_required};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(path: &str) -> String {
    std::fs::read_to_string(root().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The lines of `text` that are not comments, trimmed.
fn lines(text: &str) -> Vec<&str> {
    text.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')).collect()
}

// "Hosted CI": each lane runs the whole gate (`cargo xtask ci`, neither
// `--quick` nor `--step`) through `scripts/env.sh`, with tools required;
// the set-up installs the tools from `scripts/ci-tools.sh`, starts the
// jobserver pool and installs the Rust that `rust-toolchain.toml` pins;
// both architectures are called and have their fan-in checks; ci.yml
// runs on a push to main, pull requests, the merge queue and by hand.
#[test]
fn the_hosted_lanes_run_the_gate_as_the_development_machine_does() {
    let lane = read(".github/workflows/ci-lane.yml");
    let lane = lines(&lane);
    assert!(lane.contains(&"MORTAR_REQUIRE_TOOLS: \"1\""), "the lane requires the tools");
    let gate: Vec<&&str> = lane.iter().filter(|line| line.starts_with("cargo xtask")).collect();
    // `"$@"` is empty but on a by-hand run that asks for `--no-fail-fast`.
    assert_eq!(gate, [&"cargo xtask ci \"$@\""], "the lane runs the whole gate, once");
    assert!(lane.contains(&"if [ \"$NO_FAIL_FAST\" = true ]; then set -- --no-fail-fast; fi"));
    assert!(lane.contains(&"source scripts/env.sh"));

    let setup = read(".github/actions/mortar-ci/action.yml");
    let setup = lines(&setup);
    assert!(setup.contains(&"\"$GITHUB_WORKSPACE/scripts/ci-tools.sh\""));
    assert!(setup.iter().any(|line| line.ends_with("scripts/jobserver.sh\" start")));
    let toolchain = read("rust-toolchain.toml");
    let channel = lines(&toolchain)
        .into_iter()
        .find_map(|line| line.strip_prefix("channel = "))
        .expect("rust-toolchain.toml names a channel");
    let installed = format!("with: {{ toolchain: {channel}, components: \"rustfmt, clippy\" }}");
    assert!(setup.contains(&installed.as_str()), "the action installs Rust {channel}");

    let entry = read(".github/workflows/ci.yml");
    let entry = lines(&entry);
    for call in [
        "with: { runner: ubuntu-24.04-arm, cache-key: ci-arm, no-fail-fast: \"${{ inputs.no_fail_fast == true }}\" }",
        "with: { runner: ubuntu-24.04, cache-key: ci-x86_64, no-fail-fast: \"${{ inputs.no_fail_fast == true }}\" }",
        "check:",
        "check-x86_64:",
        "uses: ./.github/workflows/ci-lane.yml",
    ] {
        assert!(entry.contains(&call), "ci.yml: {call}");
    }
    // The triggers: a push to main, pull requests, the merge queue, and by
    // hand (rule 6).
    let on = entry.iter().position(|line| *line == "on:").expect("ci.yml: on:");
    let end = entry.iter().position(|line| *line == "permissions:").expect("ci.yml: permissions:");
    let triggers = &entry[on + 1..end];
    for trigger in
        ["push:", "branches: [main]", "pull_request:", "merge_group:", "workflow_dispatch:"]
    {
        assert!(triggers.contains(&trigger), "ci.yml triggers: {trigger}: {triggers:?}");
    }
}

// "Hosted CI", "Tools": every package and tool of `scripts/ci-tools.sh`
// is on this machine, at the pinned versions (a failure under
// MORTAR_REQUIRE_TOOLS=1, a note otherwise: a machine that is not Debian
// has no such packages).
#[test]
fn this_machine_has_the_reference_tools_of_the_hosted_lanes() {
    if tool("bash").is_none() || tool("dpkg-query").is_none() {
        return;
    }
    let script = root().join("scripts/ci-tools.sh");
    let outcome =
        run_with(Command::new("bash").arg(&script).arg("--check"), b"", Duration::from_mins(1));
    let said = format!("{}{}", outcome.stdout_text(), outcome.stderr_text());
    if outcome.success() {
        assert!(said.contains("every reference tool is here"), "{said}");
        return;
    }
    assert!(!tools_required(), "scripts/ci-tools.sh --check (MORTAR_REQUIRE_TOOLS=1):\n{said}");
    eprintln!("note: this machine lacks reference tools of the hosted lanes:\n{said}");
}
