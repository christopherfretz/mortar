//! This build's mortar binary, for the tests that link with it (`mod built;`).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// This build's mortar binary: `target/<profile>/mortar`, next to the
/// test's own directory, when it is no older than every file its
/// dep-info (`mortar.d`, which cargo writes with it) lists. Absent or
/// stale it is a named skip (`test`), a FAIL under
/// `MORTAR_REQUIRE_TOOLS=1` (WP-210: a stale binary judged an old
/// linker, and its absence was an unnamed skip, WP-241).
#[allow(dead_code)]
pub fn mortar_bin(test: &str) -> Option<PathBuf> {
    let why = match find() {
        Ok(p) => return Some(p),
        Err(why) => why,
    };
    assert!(
        !mortar_testutil::tools_required(),
        "{test}: mortar's binary of this build: {why} (MORTAR_REQUIRE_TOOLS=1; `cargo build -p mortar`)"
    );
    mortar_testutil::named_skip(test, &format!("mortar's binary of this build: {why}"));
    None
}

fn find() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("no test path: {e}"))?;
    let dir = exe.parent().and_then(Path::parent).ok_or("no profile directory")?;
    let bin = dir.join("mortar");
    let built = mtime(&bin).ok_or_else(|| format!("{} is not built", bin.display()))?;
    let dep = dir.join("mortar.d");
    let text = std::fs::read_to_string(&dep)
        .map_err(|e| format!("{} unreadable ({e}): its freshness is unknown", dep.display()))?;
    let deps = text.split_once(": ").map_or("", |(_, d)| d);
    for d in split_deps(deps) {
        match mtime(Path::new(&d)) {
            Some(t) if t <= built => {}
            Some(_) => return Err(format!("{} is older than {d}", bin.display())),
            None => return Err(format!("{} lists {d}, which is gone", dep.display())),
        }
    }
    Ok(bin)
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

/// The paths of a make-style dependency list (`\ ` escapes a space).
fn split_deps(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut it = s.trim().chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\\' if it.peek().is_some_and(|n| n.is_whitespace()) => {
                if let Some(n) = it.next() {
                    if n == '\n' {
                        continue;
                    }
                    cur.push(n);
                }
            }
            c if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
