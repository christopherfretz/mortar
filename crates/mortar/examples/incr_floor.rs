//! The measured floor of an incremental update (WP-061 stage B,
//! `spec/incr.md` §1.5): `incr_floor copy|in-place -- LINK-ARGS...` runs
//! the test-only update path (`mortar::floor::update`) of the link
//! LINK-ARGS in the current directory, whose state an `--incremental=full`
//! link of the same arguments wrote. Prints one line `ok changed=...
//! inputs=... hashed=... windows=... jobs=... refilled=... size=...
//! state=...` (or `failed: <reason>` and the link's report), then the
//! phase lines; the exit status is 0 only for `ok`.

#![forbid(unsafe_code)]

use std::ffi::OsString;

use mortar::floor::{WritePath, update};

fn main() {
    let mut args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let usage = || -> ! {
        eprintln!("usage: incr_floor copy|in-place -- LINK-ARGS...");
        std::process::exit(2);
    };
    let Some(sep) = args.iter().position(|a| a == "--") else { usage() };
    let link: Vec<OsString> = args.split_off(sep + 1);
    let write = match args.first().and_then(|a| a.to_str()) {
        Some("copy") => WritePath::Copy,
        Some("in-place") => WritePath::InPlace,
        _ => usage(),
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let u = update(&link, &cwd, write);
    match &u.outcome {
        Ok(s) => eprintln!(
            "ok changed={} inputs={} hashed={} windows={} jobs={} refilled={} dirty={} written={} size={} state={} kinds={}",
            s.changed,
            s.inputs,
            s.hashed,
            s.windows,
            s.jobs,
            s.refilled,
            s.dirty,
            s.written,
            s.file_size,
            s.state_size,
            s.kinds
        ),
        Err(why) => {
            eprintln!("failed: {why}");
            if let Some(r) = &u.report {
                eprint!("{}", r.render(&mortar_diag::Style::plain("mortar")));
            }
        }
    }
    eprint!("{}", u.phases);
    std::process::exit(i32::from(u.outcome.is_err()));
}
