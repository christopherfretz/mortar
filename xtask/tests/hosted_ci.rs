//! `spec/harness.md` "Hosted CI": the workflows run the gate as the
//! development machine does, and the one list of reference tools,
//! `scripts/ci-tools.sh`, is what this machine has.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_testutil::{run_with, tool, tools_required};
use xtask::ci::STEP_NAMES;

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

/// The jobs of a workflow file: each job's id and its lines that are not
/// comments, trimmed (the lines under `jobs:` at two spaces start a job).
fn jobs(text: &str) -> Vec<(&str, Vec<&str>)> {
    let mut jobs: Vec<(&str, Vec<&str>)> = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line == "jobs:" {
            inside = true;
            continue;
        }
        if !inside || line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let id = line.strip_prefix("  ").and_then(|rest| rest.strip_suffix(':'));
        match id {
            Some(id) if !id.starts_with(' ') => jobs.push((id, Vec::new())),
            _ => jobs.last_mut().expect("a line before the first job").1.push(line.trim()),
        }
    }
    jobs
}

/// The steps that `job`'s `cargo xtask ci` line selects with `--step`
/// (one such line per job).
fn selected_steps<'a>(id: &str, job: &[&'a str]) -> Vec<&'a str> {
    let gate: Vec<&&str> = job.iter().filter(|line| line.starts_with("cargo xtask")).collect();
    assert_eq!(gate.len(), 1, "job {id} runs one `cargo xtask` line: {gate:?}");
    let words: Vec<&str> = gate[0].split_whitespace().collect();
    assert_eq!(words[..3], ["cargo", "xtask", "ci"], "job {id}: {}", gate[0]);
    // `"$@"` is empty but on a by-hand run that asks for `--no-fail-fast`;
    // neither `--quick` nor anything else.
    assert_eq!(words.last(), Some(&"\"$@\""), "job {id}: {}", gate[0]);
    let options = &words[3..words.len() - 1];
    assert!(options.len().is_multiple_of(2), "job {id}: {}", gate[0]);
    options
        .chunks(2)
        .map(|pair| {
            assert_eq!(pair[0], "--step", "job {id}: {}", gate[0]);
            pair[1]
        })
        .collect()
}

// "Hosted CI": each lane runs the whole gate in two jobs on its runner,
// `tests` (fmt, clippy, the tests) and `links` (every other step of
// `xtask::ci`), each step in exactly one of them, each through
// `scripts/env.sh` with tools required and with the set-up action; the
// set-up installs the tools from `scripts/ci-tools.sh`, starts the
// jobserver pool and installs the Rust that `rust-toolchain.toml` pins;
// both architectures are called and have their fan-in checks, whose
// names do not change; ci.yml runs on a push to main, pull requests, the
// merge queue and by hand.
#[test]
fn the_hosted_lanes_run_the_gate_as_the_development_machine_does() {
    let text = read(".github/workflows/ci-lane.yml");
    let lane = lines(&text);
    assert!(lane.contains(&"MORTAR_REQUIRE_TOOLS: \"1\""), "the lane requires the tools");
    let lane_jobs = jobs(&text);
    let ids: Vec<&str> = lane_jobs.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, ["tests", "links"], "the lane's jobs");
    let mut runs: Vec<&str> = Vec::new();
    for (id, job) in &lane_jobs {
        assert!(job.contains(&format!("name: {id}").as_str()), "job {id} is shown as `{id}`");
        assert!(job.contains(&"- uses: ./.github/actions/mortar-ci"), "job {id}: the set-up");
        assert!(job.contains(&"scripts/jobserver.sh status"), "job {id}: the pool's status");
        assert!(job.contains(&"source scripts/env.sh"), "job {id}");
        assert!(
            job.contains(&"if [ \"$NO_FAIL_FAST\" = true ]; then set -- --no-fail-fast; fi"),
            "job {id}: --no-fail-fast from the input"
        );
        assert!(
            job.contains(&format!("cache-key: \"${{{{ inputs.cache-key }}}}-{id}\"").as_str()),
            "job {id}: a cargo cache of its own"
        );
        let steps = selected_steps(id, job);
        let expected: Vec<&str> = match *id {
            "tests" => vec!["fmt", "clippy", "test", "debug-join"],
            _ => STEP_NAMES
                .into_iter()
                .filter(|s| !["fmt", "clippy", "test", "debug-join"].contains(s))
                .collect(),
        };
        assert_eq!(steps, expected, "job {id}'s steps");
        runs.extend(steps);
        // Only `links` has link-test results: it removes an old file and
        // uploads the new one.
        let results = job.contains(&"rm -f \"$CARGO_TARGET_DIR/linktest/results.tsv\"");
        let upload = job.contains(&"- uses: actions/upload-artifact@v4");
        assert_eq!((results, upload), (*id == "links", *id == "links"), "job {id}'s results");
    }
    // Every step of `cargo xtask ci` runs in exactly one job.
    for step in STEP_NAMES {
        let times = runs.iter().filter(|run| **run == step).count();
        assert_eq!(times, 1, "step {step} runs in {times} jobs of the lane, not 1");
    }
    assert_eq!(runs.len(), STEP_NAMES.len(), "the lane runs only steps of xtask::ci: {runs:?}");

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

    let entry_text = read(".github/workflows/ci.yml");
    let entry = lines(&entry_text);
    // The fan-in jobs keep the names that are the required checks
    // (rule 9), and require each job of their lane.
    let entry_ids: Vec<&str> = jobs(&entry_text).iter().map(|(id, _)| *id).collect();
    assert_eq!(entry_ids, ["arm", "x86_64", "check", "check-x86_64"], "ci.yml's jobs");
    for arch in ["arm", "x86_64"] {
        for result in [
            format!("test \"${{{{ needs.{arch}.result }}}}\" = success"),
            format!("test \"${{{{ needs.{arch}.outputs.tests }}}}\" = success"),
            format!("test \"${{{{ needs.{arch}.outputs.links }}}}\" = success"),
        ] {
            assert!(entry.contains(&result.as_str()), "ci.yml: {result}");
        }
    }
    for output in [
        "tests: { value: \"${{ jobs.tests.result }}\" }",
        "links: { value: \"${{ jobs.links.result }}\" }",
    ] {
        assert!(lane.contains(&output), "ci-lane.yml outputs: {output}");
    }
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
