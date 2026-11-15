//! `cargo xtask <task>`: the developer and CI entry point
//! (`spec/harness.md`).
//!
//!   cargo xtask ci [--quick] [--step <name>]...    the gate: fmt, clippy, tests
//!   cargo xtask plumb <output> --map <file> ...    the verifier (spec/plumb.md §8)

use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask ci [--quick] [--step fmt|clippy|test]...\n";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("plumb") {
        return xtask::plumb::main(&args[1..]);
    }
    if args.first().map(String::as_str) != Some("ci") {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    }
    match xtask::ci::Args::parse(&args[1..]) {
        Ok(ci) if xtask::ci::run(&ci) => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(message) => {
            eprint!("xtask ci: {message}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
