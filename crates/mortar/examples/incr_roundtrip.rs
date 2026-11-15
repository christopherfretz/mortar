//! The link state's round trip on one link (WP-061's corpus gate):
//! `incr_roundtrip STATE REFERENCE REPLAY -- LINK-ARGS...` reads the state
//! (`spec/incr.md` IN-18a), checks that it re-encodes to the same bytes,
//! replays it (IN-61) with LINK-ARGS writing REPLAY, and compares REPLAY
//! with REFERENCE. Prints one word: `ok`, or `read`, `encode`, `replay` or
//! `differ` with a detail; the exit status is 0 only for `ok`.
//!
//! `incr_roundtrip --stat STATE` prints the state's size by table and the
//! median of 9 timed reads (read the file, decode, validate).

#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::path::PathBuf;

fn main() {
    let mut args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|a| a == "--stat") && args.len() == 2 {
        stat(&PathBuf::from(&args[1]));
        return;
    }
    let Some(sep) = args.iter().position(|a| a == "--") else {
        eprintln!("usage: incr_roundtrip STATE REFERENCE REPLAY -- LINK-ARGS...");
        std::process::exit(2);
    };
    let link: Vec<OsString> = args.split_off(sep + 1);
    let [state, reference, replay, _] = <[OsString; 4]>::try_from(args).unwrap_or_else(|_| {
        eprintln!("usage: incr_roundtrip STATE REFERENCE REPLAY -- LINK-ARGS...");
        std::process::exit(2);
    });
    let verdict = check(&PathBuf::from(state), &PathBuf::from(reference), replay, link);
    println!("{verdict}");
    std::process::exit(i32::from(verdict != "ok"));
}

fn check(state: &PathBuf, reference: &PathBuf, replay: OsString, link: Vec<OsString>) -> String {
    let bytes = match std::fs::read(state) {
        Ok(b) => b,
        Err(e) => return format!("read {e}"),
    };
    let s = match mortar_incr::format::decode(&bytes, mortar::VERSION.as_bytes()) {
        Ok(s) => s,
        Err(e) => return format!("read {}: {}", e.reason(), e.detail()),
    };
    if mortar_incr::format::encode(&s) != bytes {
        return "encode the state does not re-encode to its bytes".to_owned();
    }
    // The link's own side files (map, depfile) are left to the plain link.
    let mut words: Vec<OsString> = Vec::new();
    let mut it = link.into_iter();
    while let Some(a) = it.next() {
        let text = a.to_string_lossy();
        if text == "-o" || text == "--incremental-state" {
            it.next();
        } else if !(text.starts_with("--mortar-map")
            || text.starts_with("-Map")
            || text.starts_with("--Map")
            || text.starts_with("--dependency-file")
            || text.starts_with("--incremental")
            || text.starts_with("-o"))
        {
            words.push(a);
        }
    }
    words.push("-o".into());
    words.push(replay.clone());
    let cwd = std::env::current_dir().unwrap_or_default();
    let inv = mortar::replay_link("mortar", &words, &cwd, &s);
    let mortar::Outcome::Linked { .. } = inv.outcome else {
        return "replay the replayed link failed".to_owned();
    };
    match (std::fs::read(reference), std::fs::read(&replay)) {
        (Ok(a), Ok(b)) if a == b => "ok".to_owned(),
        (Ok(_), Ok(_)) => "differ the replay's output is not the reference".to_owned(),
        (Err(e), _) | (_, Err(e)) => format!("read {e}"),
    }
}

fn stat(path: &PathBuf) {
    use mortar_incr::format::{HEADER_LEN, RECORD_LEN, kind};
    let bytes = std::fs::read(path).expect("the state");
    println!("total {} bytes", bytes.len());
    let mut at = HEADER_LEN;
    let word = |at: usize, n: usize| {
        let mut v = [0u8; 8];
        v[..n].copy_from_slice(&bytes[at..at + n]);
        u64::from_le_bytes(v)
    };
    while at + RECORD_LEN <= bytes.len() {
        let k = u32::try_from(word(at, 4)).unwrap_or(0);
        let rows = word(at + 8, 8);
        let len = usize::try_from(word(at + 16, 8)).unwrap_or(usize::MAX);
        println!("{:>14} {rows:>10} rows {:>12} bytes", kind::name(k), len + RECORD_LEN);
        at = at.saturating_add(RECORD_LEN).saturating_add(len);
    }
    let mut times = Vec::new();
    for _ in 0..9 {
        let t = std::time::Instant::now();
        let b = std::fs::read(path).expect("the state");
        let s = mortar_incr::format::decode(&b, mortar::VERSION.as_bytes());
        times.push(t.elapsed());
        assert!(s.is_ok());
    }
    times.sort();
    println!("read+validate median {:?} (min {:?}, max {:?})", times[4], times[0], times[8]);
}
