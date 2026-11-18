//! The names of `.symtab`'s entries (WP-246, V-2; `spec/plumb.md` §6.5
//! Y2, Y3, Y5's closed set, Y6, Y7; §6.9 P-E3).
//!
//! An incremental-mode output (AArch64, glibc, static) whose `.strtab`
//! named the middle of `__libc_single_threaded` (`readed`) and left four
//! `STT_FILE` entries unnamed verified with zero findings. The fixtures
//! (`tests/symnames/`, `gen.sh`) are mortar's static links of one program
//! on AArch64 and x86-64, each linked twice from the same objects: `x-<arch>`
//! in incremental mode, `f-<arch>` plain. `tests/symnames/mut.py` (bytes
//! only, no linker) makes one mutation of `.symtab` per sentence; each
//! good link is clean, each mutation is a finding whose words name the
//! entry.
//!
//! The evidence itself (`MORTAR_WP246_EVIDENCE`, by default WP-217's
//! `t1/` directory; its `inc.bad`, the fixed `inc` and the `plain` link)
//! is used when present, copied into a scratch directory; absent, the
//! tests that need it skip by name.
#![allow(clippy::many_single_char_names)]

use std::path::{Path, PathBuf};

use mortar_plumb::{Ctx, Files, Format, Input, Map, Report};
use mortar_testutil::{Scratch, run_with};

struct Dir<'a>(&'a Path);

impl Files for Dir<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/symnames")
}

const ARCHS: [&str; 2] = ["a64", "x64"];

/// One link: its directory (the inputs its map names are relative to it),
/// output and map.
struct Link {
    dir: PathBuf,
    out: Vec<u8>,
    map: Map,
}

fn read_map(path: &Path) -> Map {
    let text = std::fs::read_to_string(path).unwrap();
    mortar_plumb::map::parse(Format::Mortar, &text)
        .unwrap_or_else(|e| panic!("{}: map line {}: {}", path.display(), e.line, e.message))
}

fn link(d: &Path, out: &str, map: &str) -> Link {
    Link { dir: d.to_path_buf(), out: std::fs::read(d.join(out)).unwrap(), map: read_map(&d.join(map)) }
}

/// The verifier on one link.
fn verify(l: &Link) -> Report {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for p in l.map.paths() {
        if !files.iter().any(|(q, _)| q == p)
            && let Ok(b) = std::fs::read(l.dir.join(p))
        {
            files.push((p.to_string(), b));
        }
    }
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    mortar_plumb::verify_with(&l.out, &inputs, &l.map, &Ctx { line: None, files: &Dir(&l.dir) })
}

/// `rule message` of each finding.
fn lines(r: &Report) -> Vec<String> {
    r.findings.iter().map(|f| format!("{} {}", f.rule.id(), f.message)).collect()
}

/// `mut.py` on `l`'s output with `op`, into `s`: the mutated link and
/// the entry's index. `None` (the test skips) when python3 is missing and
/// tools are optional.
fn mutate(l: &Link, s: &Scratch, op: &str) -> Option<(Link, u32)> {
    let py = mortar_testutil::tool("python3")?;
    let (i, o) = (s.join("in"), s.join("out"));
    std::fs::write(&i, &l.out).unwrap();
    let mut c = std::process::Command::new(py);
    c.arg(dir().join("mut.py")).arg(&i).arg(&o).arg(op);
    let r = run_with(&mut c, b"", mortar_testutil::DEFAULT_TIMEOUT);
    assert!(r.success(), "mut.py {op}: {}", r.stderr_text());
    let index: u32 = String::from_utf8_lossy(&r.stdout).trim().parse().unwrap();
    let out = std::fs::read(&o).unwrap();
    Some((Link { dir: l.dir.clone(), out, map: l.map.clone() }, index))
}

/// The report of `what` has a finding of each rule in `rules`, and every
/// one of them names `needle` (the entry, by index or name).
fn found(what: &str, r: &Report, rules: &[&str], needle: &str) {
    let got = lines(r);
    for rule in rules {
        assert!(
            got.iter().any(|l| l.starts_with(&format!("{rule} ")) && l.contains(needle)),
            "{what}: want {rule} naming '{needle}', got {got:#?}"
        );
    }
}

fn clean(what: &str, r: &Report) {
    let got = lines(r);
    assert!(got.is_empty(), "{what}: want no finding, got {got:#?}\n{}", r.summary());
}

/// The fixture links: (label, link).
fn fixtures() -> Vec<(String, Link)> {
    let d = dir();
    let mut v = Vec::new();
    for a in ARCHS {
        for base in ["x", "f"] {
            v.push((format!("{base}-{a}"), link(&d, &format!("{base}-{a}"), &format!("{base}-{a}.map"))));
        }
    }
    v
}

/// One mutation on every fixture: `op` and the words each finding must
/// have (`{i}` is replaced by the entry's index).
fn each(op: &str, incremental_only: bool, want: &[(&str, &str)]) {
    for (label, l) in fixtures() {
        let incremental = label.starts_with('x');
        if incremental_only && !incremental {
            continue;
        }
        let s = Scratch::new("symnames");
        let Some((m, i)) = mutate(&l, &s, op) else { return };
        let r = verify(&m);
        for (rule, needle) in want {
            let needle = needle.replace("{i}", &i.to_string());
            found(&format!("{label} {op}"), &r, &[rule], &needle);
        }
    }
}

// §6.5 (WP-246): the fixtures are clean, with every local of the program
// judged, none missing, the hidden global demoted, and both files' and the
// start file's STT_FILE judged.
#[test]
fn wp246_good_links_are_clean() {
    for (label, l) in fixtures() {
        let r = verify(&l);
        clean(&label, &r);
        let c = &r.coverage;
        assert!(c.input_locals_kept >= 9, "{label}: {} kept", c.input_locals_kept);
        assert_eq!(c.input_locals_missing, 0, "{label}");
        assert_eq!(c.output_locals_extra, 0, "{label}");
        assert!(c.output_locals_demoted >= 1, "{label}: hidden_b not demoted");
        assert_eq!(c.file_symbols_judged, 3, "{label}");
        assert!(c.linker_defined_not_judged.is_empty(), "{label}: {:?}", c.linker_defined_not_judged);
    }
}

// Y5's closed set: a local's st_name moved into the middle of its string
// (the evidence's form) reads as a name no input has.
#[test]
fn wp246_y5_name_in_the_middle_of_a_string() {
    // `other_helper_a` + 6 = `helper_a`: an input local's name, at another
    // local's value: Y4 and Y6.
    each(
        "mid=other_helper_a:6",
        false,
        &[("Y4", "'helper_a' is not where"), ("Y6", "entry {i} ('helper_a'"), ("Y6", "'other_helper_a'")],
    );
    // `greeting_a` + 3 = `eting_a`: no input has it: Y5 and Y6.
    each(
        "mid=greeting_a:3",
        false,
        &[("Y5", ".symtab entry {i} ('eting_a') is defined, but no input"), ("Y6", "'greeting_a'")],
    );
}

// Y7: an STT_FILE emptied.
#[test]
fn wp246_y7_file_symbol_without_a_name() {
    each(
        "fileempty=a.c",
        false,
        &[("Y7", ".symtab entry {i} is an STT_FILE with no name"), ("Y6", "the local 'a.c'")],
    );
}

// Y6: a local dropped, as a filler (incremental) or removed (plain).
#[test]
fn wp246_y6_local_dropped() {
    each("fill=helper_b", true, &[("Y6", "the local 'helper_b' (b-")]);
    for (label, l) in fixtures() {
        if label.starts_with('x') {
            continue;
        }
        let s = Scratch::new("symnames");
        let Some((m, _)) = mutate(&l, &s, "drop=helper_b") else { return };
        let r = verify(&m);
        found(&format!("{label} drop"), &r, &["Y6"], "the local 'helper_b' (b-");
    }
}

// Y4, Y6: two locals of one file exchange their names.
#[test]
fn wp246_y6_names_swapped() {
    each(
        "swap=helper_a,counter_a",
        false,
        &[("Y4", "'helper_a'"), ("Y6", "the local 'helper_a'"), ("Y6", "the local 'counter_a'")],
    );
}

// Y3, Y5: a global renamed to a name no input has.
#[test]
fn wp246_y3_y5_global_renamed() {
    each(
        "gmid=afun:1",
        false,
        &[("Y5", ".symtab entry {i} ('fun')"), ("Y3", "symbol 'afun'")],
    );
    // The demoted hidden global: before WP-246 Y3 judged no hidden name
    // missing.
    each("mid=hidden_b:2", false, &[("Y3", "symbol 'hidden_b'"), ("Y5", "('dden_b')")]);
}

// Y4, Y6: a local's value moved by one.
#[test]
fn wp246_y6_value_moved() {
    each("value=helper_b", false, &[("Y4", "'helper_b'"), ("Y6", "the local 'helper_b'")]);
}

// §6.9 P-E3: a local's name changed on one side is reported by the
// multiset of the locals (WP-246 item 5).
#[test]
fn wp246_p_e3_a_local_renamed_on_one_side() {
    for a in ARCHS {
        let d = dir();
        let x = link(&d, &format!("x-{a}"), &format!("x-{a}.map"));
        let f = link(&d, &format!("f-{a}"), &format!("f-{a}.map"));
        let s = Scratch::new("symnames");
        let Some((xm, _)) = mutate(&x, &s, "mid=greeting_a:3") else { return };
        let files: Vec<(String, Vec<u8>)> = x
            .map
            .paths()
            .into_iter()
            .map(|p| (p.to_string(), std::fs::read(d.join(p)).unwrap()))
            .collect();
        let inputs: Vec<Input<'_>> =
            files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        let e = mortar_plumb::equiv::check(&xm.out, &xm.map, &f.out, &f.map, &inputs);
        let got: Vec<String> = e.findings.iter().map(|f| format!("{} {}", f.rule.id(), f.message)).collect();
        assert!(
            got.iter().any(|l| l.starts_with("P-E3 ") && l.contains("eting_a")),
            "{a}: want P-E3 naming 'eting_a', got {got:#?}"
        );
        assert!(
            got.iter().any(|l| l.starts_with("P-E3 ") && l.contains("greeting_a")),
            "{a}: want P-E3 naming 'greeting_a', got {got:#?}"
        );
        // The unmutated pair is equivalent.
        let e = mortar_plumb::equiv::check(&x.out, &x.map, &f.out, &f.map, &inputs);
        assert!(e.findings.is_empty() && e.x.findings.is_empty() && e.f.findings.is_empty(), "{a}");
    }
}

/// WP-217's `t1/` directory, copied into `s`: `None` (a named skip) when
/// it is absent.
fn evidence(s: &Scratch) -> Option<PathBuf> {
    let from = std::env::var_os("MORTAR_WP246_EVIDENCE").map_or_else(
        || PathBuf::from("/Volumes/AgentWork/mortar-helper/.build/target-wp-140/wp217/t1"),
        PathBuf::from,
    );
    let names =
        ["inc.bad", "inc.bad.map", "inc", "inc.map", "plain", "plain.map", "a.o", "b.o"];
    if !names.iter().all(|n| from.join(n).is_file()) {
        eprintln!("SKIP wp246 evidence: {} is absent", from.display());
        return None;
    }
    let to = s.join("t1");
    std::fs::create_dir_all(&to).unwrap();
    for n in names {
        std::fs::copy(from.join(n), to.join(n)).unwrap();
    }
    Some(to)
}

// The evidence: `inc.bad` draws findings naming `readed` and the unnamed
// file symbols; the fixed `inc` and the `plain` link are clean; the
// mutations of the tests above on the glibc links.
#[test]
fn wp246_the_evidence() {
    let s = Scratch::new("symnames-evidence");
    let Some(d) = evidence(&s) else { return };
    let bad = link(&d, "inc.bad", "inc.bad.map");
    let r = verify(&bad);
    found("inc.bad", &r, &["Y5", "Y6"], "readed");
    found("inc.bad", &r, &["Y3"], "__libc_single_threaded");
    for i in [32, 52, 60, 3140] {
        found("inc.bad", &r, &["Y7"], &format!(".symtab entry {i} is an STT_FILE with no name"));
    }
    let inc = link(&d, "inc", "inc.map");
    let plain = link(&d, "plain", "plain.map");
    clean("inc", &verify(&inc));
    clean("plain", &verify(&plain));
    // The evidence reproduced by the script on the fixed output.
    let Some((m, i)) = mutate(&inc, &s, "mid=__libc_single_threaded:16") else { return };
    assert_eq!(i, 2489);
    found("inc mid", &verify(&m), &["Y5", "Y6"], "readed");
    for (l, label) in [(&inc, "inc"), (&plain, "plain")] {
        let Some((m, i)) = mutate(l, &s, "fileempty=a.c") else { return };
        found(label, &verify(&m), &["Y7"], &format!("entry {i} is an STT_FILE with no name"));
        let Some((m, _)) = mutate(l, &s, "value=deregister_tm_clones") else { return };
        found(label, &verify(&m), &["Y4", "Y6"], "'deregister_tm_clones'");
    }
    let crt = "the local 'deregister_tm_clones' (/usr/lib/gcc/aarch64-linux-gnu/14/crtbeginT.o";
    let Some((m, _)) = mutate(&inc, &s, "fill=deregister_tm_clones") else { return };
    found("inc fill", &verify(&m), &["Y6"], crt);
    let Some((m, _)) = mutate(&plain, &s, "drop=deregister_tm_clones") else { return };
    found("plain drop", &verify(&m), &["Y6"], crt);
}
