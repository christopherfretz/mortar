//! `incr_state`: format-5 state tool (WP-214, U-2; `spec/incr.md` IN-18c,
//! IN-78).
//!
//! ```text
//! incr_state convert <format-4 state> <out> <mortar version string>
//! incr_state canon <state>
//! incr_state compare <state A> <state B>
//! incr_state size <state> [<format-4 state> <mortar version string>]
//! ```
//!
//! `convert` writes the format-5 state of a format-4 one (the full link's
//! writer over its rows: `encode(normalize(from_format4(..)))`); `canon`
//! prints the canonical rows' digest per table; `compare` prints the
//! canonical differences (exit 1 when any); `size` prints the per-table
//! sizes and, given the format-4 state, the ratio (U-2's 1.15 gate). Every
//! command refuses a format-5 state whose updating flag is set.

use std::process::ExitCode;

use mortar_core::incr::{MemoryLayout, StoredLayout};
use mortar_incr::format;
use mortar_incr::state5::{self, compact, read, write};
use mortar_incr::update::testing::MemoryState;

fn load5(path: &str) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    read::refuse_updating(&bytes).map_err(|e| format!("{path}: {e}"))?;
    Ok(bytes)
}

fn convert(path4: &str, version: &str) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path4).map_err(|e| format!("{path4}: {e}"))?;
    let s = format::decode(&bytes, version.as_bytes()).map_err(|e| format!("{path4}: {e}"))?;
    let m = MemoryState::from_format4(&s);
    let n = compact::normalize(&m).map_err(|e| format!("{path4}: {e}"))?;
    write::encode(&n).map_err(|e| format!("{path4}: {e}"))
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["convert", from, to, version] => {
            let bytes = convert(from, version)?;
            std::fs::write(to, &bytes).map_err(|e| format!("{to}: {e}"))?;
            // the round trip: the written state reads back to its own bytes
            let back = read::decode(&bytes).map_err(|e| format!("{to}: {e}"))?;
            let again = write::encode(&compact::normalize(&back).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            if again != bytes {
                return Err(format!("{to}: the round trip differs"));
            }
            println!("converted {from} -> {to}: {} bytes", bytes.len());
            Ok(ExitCode::SUCCESS)
        }
        ["canon", path] => {
            let c = state5::canon(&load5(path)?).map_err(|e| format!("{path}: {e}"))?;
            for (k, rows) in &c.tables {
                let mut all = Vec::new();
                for r in rows {
                    all.extend_from_slice(&(r.len() as u64).to_le_bytes());
                    all.extend_from_slice(r);
                }
                let h = mortar_digest::l128(&all);
                let hex: String = h.iter().map(|b| format!("{b:02x}")).collect();
                println!("{:<12} {:>10} rows {hex}", state5::name(*k), rows.len());
            }
            Ok(ExitCode::SUCCESS)
        }
        ["compare", a, b] => {
            let ca = state5::canon(&load5(a)?).map_err(|e| format!("{a}: {e}"))?;
            let cb = state5::canon(&load5(b)?).map_err(|e| format!("{b}: {e}"))?;
            let rows = MemoryLayout::default();
            let diffs = state5::compare(&ca, &cb, &StoredLayout::new(&rows));
            for d in &diffs {
                println!("{} {}: {}", state5::name(d.kind), d.row, d.what);
            }
            println!("{} differences", diffs.len());
            Ok(if diffs.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
        }
        ["size", path] => {
            print!("{}", state5::size_report(&load5(path)?).map_err(|e| format!("{path}: {e}"))?);
            Ok(ExitCode::SUCCESS)
        }
        ["size", path4, version] => {
            let four = std::fs::metadata(path4).map_err(|e| format!("{path4}: {e}"))?.len();
            let bytes = convert(path4, version)?;
            print!("{}", state5::size_report(&bytes).map_err(|e| e.to_string())?);
            #[allow(clippy::cast_precision_loss)] // a ratio for people
            let ratio = bytes.len() as f64 / four as f64;
            println!("format 4 {four} bytes, format 5 {} bytes, ratio {ratio:.3} (gate 1.15)", bytes.len());
            Ok(ExitCode::SUCCESS)
        }
        _ => Err("usage: incr_state convert <4> <out> <version> | canon <s> | compare <a> <b> | size <s> | size <4> <version>"
            .to_owned()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("incr_state: {e}");
            ExitCode::from(2)
        }
    }
}
