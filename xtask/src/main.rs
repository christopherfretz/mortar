//! `cargo xtask <task>`: the developer and CI entry point
//! (`spec/harness.md`).
//!
//!   cargo xtask ci [--quick] [--step <name>]...    the gate: fmt, clippy, tests
//!   cargo xtask bench [options]                    link speed and memory

use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask ci [--quick] [--step fmt|clippy|test]...\n       \
                     cargo xtask bench [options]   (cargo xtask bench --help)\n";

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
        #[cfg(unix)]
        Some("bench") => match xtask::bench::Options::parse(&args[1..]) {
            Ok(options) => ExitCode::from(xtask::bench::run(&options)),
            Err(message) => {
                eprint!("xtask bench: {message}\n{}", xtask::bench::USAGE);
                ExitCode::from(2)
            }
        },
        _ => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
