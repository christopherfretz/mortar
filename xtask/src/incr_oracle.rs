//! `cargo xtask incr-oracle [--quick] [--only S,...] [-j N] [--keep]`: the
//! two-full-link oracle lane of the incremental dry run (`spec/incr.md`
//! IN-63a, WP-063), `tests/incr-oracle/run.py`, over every scenario at O0
//! and O2 with the mutation table (IN-63b). The nightly set; the gate
//! runs the quick subset as `cargo test -p mortar --test incr_oracle`.
//! Until WP-225 M6 the subject is the testhooks build
//! (`crate::incr_update::hooked_binary`): the dry run is computed from the
//! format-4 side state only it writes.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use crate::process;

pub const USAGE: &str = "usage: cargo xtask incr-oracle [--quick] [--only <scenario>,...] [-j <n>] [--keep] [--mortar <path>]\n";

pub fn main(args: &[String]) -> ExitCode {
    let mut pass: Vec<String> = Vec::new();
    let mut mortar: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--quick" | "--keep" => pass.push(args[i].clone()),
            "--only" | "-j" if i + 1 < args.len() => {
                pass.push(args[i].clone());
                pass.push(args[i + 1].clone());
                i += 1;
            }
            "--mortar" if i + 1 < args.len() => {
                mortar = Some(PathBuf::from(&args[i + 1]));
                i += 1;
            }
            other => {
                eprint!("xtask incr-oracle: unknown argument `{other}`\n{USAGE}");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }
    // WP-225 (rulings M1-1): until M6 the subject is the testhooks build,
    // whose dry run run.py turns on (`MORTAR_INCR_STATE4=1`).
    let mortar = match mortar.map_or_else(crate::incr_update::hooked_binary, Ok) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("xtask incr-oracle: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Some(python) = mortar_testutil::find_tool("python3") else {
        eprintln!("xtask incr-oracle: python3 not found");
        return ExitCode::FAILURE;
    };
    let lane = crate::linktest::workspace_root().join("tests/incr-oracle/run.py");
    let work = crate::linktest::build_dir().join("tmp/incr-oracle");
    let mut cmd = Command::new(python);
    cmd.arg(lane)
        .arg("--mutations")
        .arg("--mortar")
        .arg(&mortar)
        .arg("--work")
        .arg(&work)
        .args(&pass);
    match process::run_step(&mut cmd, false) {
        Ok((status, _)) if status.success() => ExitCode::SUCCESS,
        Ok((status, _)) => {
            eprintln!("xtask incr-oracle: {status}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("xtask incr-oracle: cannot run python3: {e}");
            ExitCode::FAILURE
        }
    }
}
