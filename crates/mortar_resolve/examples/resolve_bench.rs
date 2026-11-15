//! Hand-run benchmark of resolution (`spec/landed/resolve/WP-022.md`):
//! `cargo run --release -p mortar_resolve --example resolve_bench
//! [response.txt]`. Times `resolve` at 1, 2, 4 and 8 threads on a real
//! link's inputs (a collect2 response file; `-l` names resolve to static
//! archives only, shared libraries are skipped: M1 is static) and on a
//! synthetic 10^6-key input. Short (seconds), at most 8 threads: the host
//! is shared. Lives here rather than in `mortar_bench` because the
//! dependency edge points the other way.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    // Linker arguments are case-sensitive.
    clippy::case_sensitive_file_extension_comparisons
)]

#[path = "../tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mortar_core::OptId;
use mortar_core::par::Pool;
use mortar_elf::Machine;
use mortar_resolve::options::{Entry, Options, Output};
use mortar_resolve::resolve;
use mortar_resolve::view::{FileKind, View};
use support::Model;

const THREADS: [usize; 4] = [1, 2, 4, 8];
const REPEATS: usize = 7;

fn load_average() -> String {
    std::fs::read_to_string("/proc/loadavg").map_or_else(
        |_| "unknown".into(),
        |s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" "),
    )
}

/// A static-link model of a collect2 response file. Relative paths are
/// against `<dir>/build`.
fn real(response: &Path) -> Result<(Model, usize), String> {
    let text = std::fs::read_to_string(response).map_err(|e| e.to_string())?;
    let base = response.parent().unwrap_or(Path::new(".")).join("build");
    let words: Vec<&str> = text.split_whitespace().collect();
    let machine = if text.contains("aarch64") { Machine::AArch64 } else { Machine::X86_64 };
    let mut m = Model::new(machine);
    let mut search: Vec<PathBuf> = Vec::new();
    let mut whole = false;
    let mut group: Option<u32> = None;
    let mut groups = 0;
    let mut skipped = 0;
    let mut i = 0;
    let take = |i: &mut usize| {
        *i += 1;
        words.get(*i).copied()
    };
    while i < words.len() {
        let w = words[i];
        let path =
            |p: &str| if Path::new(p).is_absolute() { PathBuf::from(p) } else { base.join(p) };
        match w {
            "-o" | "-plugin" | "-dynamic-linker" | "-rpath" | "-m" | "-z" => {
                take(&mut i);
            }
            "--whole-archive" => whole = true,
            "--no-whole-archive" => whole = false,
            "--start-group" | "-(" => {
                groups += 1;
                group = Some(groups);
            }
            "--end-group" | "-)" => group = None,
            _ if w.starts_with("-L") => search.push(path(&w[2..])),
            _ if w.starts_with("-l") => {
                let name = format!("lib{}.a", &w[2..]);
                // A linker script named `.a` (glibc's `libm.a`) is skipped.
                let is_archive =
                    |p: &PathBuf| std::fs::read(p).is_ok_and(|b| b.starts_with(b"!<arch>\n"));
                match search.iter().map(|d| d.join(&name)).find(is_archive) {
                    Some(p) => {
                        m.load_archive(&p, whole, group)?;
                    }
                    None => skipped += 1,
                }
            }
            _ if w.starts_with('-') => {}
            _ if w.ends_with(".a") => {
                m.load_archive(&path(w), whole, group)?;
            }
            _ if w.ends_with(".o") => {
                let p = path(w);
                let bytes = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                m.load_object(&bytes, w, FileKind::Object)?;
            }
            _ => {}
        }
        i += 1;
    }
    Ok((m, skipped))
}

/// 10^6 keys in 1000 members, all referenced from one object.
fn synthetic() -> Model {
    let mut m = Model::new(Machine::X86_64);
    let user = m.obj("user.o");
    let names: Vec<String> = (0..1_000_000).map(|i| format!("k{i}")).collect();
    for n in &names {
        m.undef(user, n);
    }
    let ar = m.archive();
    for (c, chunk) in names.chunks(1000).enumerate() {
        let f = m.member(ar, &format!("m{c}.o"));
        for n in chunk {
            m.def(f, n);
        }
    }
    m
}

fn bench(what: &str, m: &Model, opts: &Options) {
    println!("{what}: {} files, {} keys, {} groups", m.files(), m.keys(), m.comdats());
    let reference = resolve(m, opts, &Pool::sequential());
    let live = reference.file_live.iter().filter(|l| l.is_live()).count();
    println!(
        "  {live} live files, {} rounds, {} problems",
        reference.rounds,
        reference.problems.len()
    );
    let mut kinds = std::collections::BTreeMap::<String, usize>::new();
    for p in &reference.problems {
        let text = format!("{:?}", p.what);
        let kind = text.split([' ', '{', '(']).next().unwrap_or_default().to_owned();
        *kinds.entry(kind).or_default() += 1;
    }
    if !kinds.is_empty() {
        println!("  problems by kind: {kinds:?}");
    }
    for p in reference.problems.iter().take(3) {
        if let mortar_resolve::problem::What::ArchiveChoice { key, mortar, gnu, .. } = &p.what {
            println!(
                "  e.g. {}: mortar {}, GNU {}",
                String::from_utf8_lossy(m.key_name(*key).0),
                m.label(mortar.file),
                m.label(gnu.file)
            );
        }
    }
    for threads in THREADS {
        let pool = Pool::new(threads);
        let mut times: Vec<Duration> = Vec::with_capacity(REPEATS);
        for _ in 0..REPEATS {
            let t = Instant::now();
            let r = resolve(m, opts, &pool);
            times.push(t.elapsed());
            assert_eq!(r, reference, "{threads} threads");
        }
        times.sort();
        println!(
            "  {threads} threads: median {:.2} ms, min {:.2} ms (load {})",
            times[REPEATS / 2].as_secs_f64() * 1e3,
            times[0].as_secs_f64() * 1e3,
            load_average()
        );
    }
}

fn main() {
    let response = std::env::args().nth(1).map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../../../.build/bench-work/workloads/cxx-medium/response.txt")
        },
        PathBuf::from,
    );
    println!("load average at start: {}", load_average());
    match real(&response) {
        Ok((mut m, skipped)) => {
            let start = m.key("_start");
            let opts = Options {
                entry: Entry::Default(OptId::some(start)),
                warn_backrefs: true,
                output: Output { pie: true, dynamic: false },
                ..Options::default()
            };
            bench(
                &format!("{} ({skipped} -l without a static archive skipped)", response.display()),
                &m,
                &opts,
            );
        }
        Err(e) => println!("{}: not run: {e}", response.display()),
    }
    let m = synthetic();
    let opts = Options { entry: Entry::None, ..Options::default() };
    bench("synthetic 10^6 keys", &m, &opts);
    println!("load average at end: {}", load_average());
}
