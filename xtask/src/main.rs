//! `cargo xtask <task>`: the developer and CI entry point
//! (`spec/harness.md`).
//!
//!   cargo xtask ci [--quick]    the gate: fmt, clippy, tests

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("ci") {
        return ci(&args[1..]);
    }
    eprintln!("usage: cargo xtask ci [--quick]");
    ExitCode::from(2)
}

/// The gate. `--quick` is what an implementer runs before handing back; the
/// full gate adds the link-test corpus and the differential lanes as they
/// land (`docs/VALIDATION.md`). Today both run the same three steps.
fn ci(args: &[String]) -> ExitCode {
    if let Some(bad) = args.iter().find(|a| a.as_str() != "--quick") {
        eprintln!("xtask ci: unknown argument `{bad}`");
        return ExitCode::from(2);
    }
    let steps: [(&str, &[&str]); 3] = [
        ("fmt", &["fmt", "--all", "--check"]),
        ("clippy", &["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]),
        ("test", &["test", "--workspace"]),
    ];
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    for (name, argv) in steps {
        eprintln!("[ci] {name}");
        match Command::new(&cargo).args(argv).status() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("[ci] {name}: FAILED ({status})");
                return ExitCode::FAILURE;
            }
            Err(err) => {
                eprintln!("[ci] {name}: cannot run cargo: {err}");
                return ExitCode::FAILURE;
            }
        }
    }
    eprintln!("[ci] ok");
    ExitCode::SUCCESS
}
