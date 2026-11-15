//! `cargo xtask <task>`: the developer and CI entry point
//! (`spec/harness.md`).
//!
//!   cargo xtask ci [--quick] [--step <name>]...    the gate: fmt, clippy, tests, link tests
//!   cargo xtask linktest [options] [paths...]      the link tests ("Link tests")

use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask ci [--quick] [--step fmt|clippy|test|linktest]...\n       \
                     cargo xtask linktest [options] [<file or directory>...]\n";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ci") => match xtask::ci::Args::parse(&args[1..]) {
            Ok(ci) if xtask::ci::run(&ci) => ExitCode::SUCCESS,
            Ok(_) => ExitCode::FAILURE,
            Err(message) => {
                eprint!("xtask ci: {message}\n{USAGE}");
                ExitCode::from(2)
            }
        },
        Some("linktest") => ExitCode::from(xtask::linktest::main(&args[1..])),
        _ => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
