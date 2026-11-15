//! SHA-1 and SHA-256 against coreutils' `sha1sum` and `sha256sum`: every
//! length 0..=300 of random bytes, and large buffers. Skipped with a note
//! when a tool is missing, a failure under `MORTAR_REQUIRE_TOOLS=1`.

mod common;

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use common::{Rng, hex};
use mortar_testutil::{Scratch, run_with, tool};

/// `tool FILE…` once; the digests in file order.
fn digests(program: &Path, files: &[std::path::PathBuf]) -> Vec<String> {
    let out = run_with(Command::new(program).args(files), &[], Duration::from_mins(2));
    assert!(out.success(), "{}: {}", program.display(), out.stderr_text());
    let text = out.stdout_text();
    let lines: Vec<String> =
        text.lines().map(|l| l.split_whitespace().next().unwrap_or_default().to_owned()).collect();
    assert_eq!(lines.len(), files.len(), "{text}");
    lines
}

fn inputs() -> Vec<Vec<u8>> {
    let mut rng = Rng(0x5EED_0001);
    let mut v: Vec<Vec<u8>> = (0..=300).map(|n| rng.bytes(n)).collect();
    // Large ones: odd lengths around a leaf, and one of several MiB.
    for n in [(1 << 20) - 1, (1 << 20) + 55, 3 * (1 << 20) + 7] {
        v.push(rng.bytes(n));
    }
    v
}

fn check(name: &str, ours: fn(&[u8]) -> Vec<u8>) {
    let Some(program) = tool(name) else { return };
    let scratch = Scratch::new(&format!("digest-{name}"));
    let data = inputs();
    let files: Vec<_> =
        data.iter().enumerate().map(|(i, d)| scratch.write(&format!("in{i}"), d)).collect();
    for (i, theirs) in digests(&program, &files).into_iter().enumerate() {
        assert_eq!(hex(&ours(&data[i])), theirs, "{name} of {} bytes", data[i].len());
    }
}

#[test]
fn sha1_equals_sha1sum() {
    check("sha1sum", |d| mortar_digest::sha1(d).to_vec());
}

#[test]
fn sha256_equals_sha256sum() {
    check("sha256sum", |d| mortar_digest::sha256(d).to_vec());
}
