//! `cargo xtask <task>`: the developer and CI entry point
//! (`spec/harness.md`).
//!
//!   cargo xtask ci [--quick] [--no-fail-fast] [--step <name>]...    the gate: fmt, clippy, tests, link tests
//!   cargo xtask linktest [options] [paths...]      the link tests ("Link tests")
//!   cargo xtask census [--quick] [options]         the driver-line census (spec/dyn.md §2)
//!   cargo xtask sweep [--quick] ...                the byte sweep on mortar's outputs (spec/plumb.md §12.3)
//!   cargo xtask static-identical [--update]        static links' digests ("static-identical")
//!   cargo xtask bench [options]                    link speed and memory
//!   cargo xtask plumb <output> --map <file> ...    the verifier (spec/plumb.md §8)
//!   cargo xtask mason [programs|census|setup|libcxx|verify] ...  the Mason lane (tests/mason/README.md)
//!   cargo xtask meta [write|dump] ...              Mason metadata: test producer, dumper (spec/meta.md §12)
//!   cargo xtask incr-oracle [--quick] ...          the incremental dry run's oracle lane (tests/incr-oracle)

use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask ci [--quick] [--no-fail-fast] [--step fmt|clippy|test|linktest|census|mason|sweep|static-identical]...\n       \
                     cargo xtask linktest [options] [<file or directory>...]\n       \
                     cargo xtask census [--quick] [--arch <a>[,<a>]] [--filter <s>]... [--mortar <path>] [-j <n>] [--keep]\n       \
                     cargo xtask sweep [--quick] [--arch <a>[,<a>]] [--threads <n>]\n       \
                     cargo xtask static-identical [--update] [-j <n>]\n       \
                     cargo xtask bench [options]   (cargo xtask bench --help)\n       \
                     cargo xtask plumb <output> --map <file> --map-format <format> ...\n       \
                     cargo xtask mason [programs|census|setup|libcxx|verify] ...   (tests/mason/README.md)\n       \
                     cargo xtask meta [write|dump] ...   (cargo xtask meta)\n       \
                     cargo xtask incr-oracle [--quick] [--only <scenario>,...] [-j <n>] [--keep]\n";

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
        #[cfg(unix)]
        Some("census") => ExitCode::from(xtask::census::main(&args[1..])),
        #[cfg(unix)]
        Some("sweep") => ExitCode::from(xtask::sweep::main(&args[1..])),
        #[cfg(unix)]
        Some("static-identical") => ExitCode::from(xtask::identical::main(&args[1..])),
        Some("plumb") => xtask::plumb::main(&args[1..]),
        #[cfg(unix)]
        Some("bench") => match xtask::bench::Options::parse(&args[1..]) {
            Ok(options) => ExitCode::from(xtask::bench::run(&options)),
            Err(message) => {
                eprint!("xtask bench: {message}\n{}", xtask::bench::USAGE);
                ExitCode::from(2)
            }
        },
        #[cfg(unix)]
        Some("mason") => xtask::mason::main(&args[1..]),
        #[cfg(unix)]
        Some("meta") => xtask::meta::main(&args[1..]),
        #[cfg(unix)]
        Some("incr-oracle") => xtask::incr_oracle::main(&args[1..]),
        _ => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
