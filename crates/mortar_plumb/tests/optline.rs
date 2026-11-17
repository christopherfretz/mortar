//! `spec/plumb.md` §3.2a (WP-201): the options a rule reads come from the
//! caller's line for every format; a mortar map's `arg` records are the
//! options only when the caller gives none (§12.1a's rule, WP-193). M9
//! still judges the map's own records.
//!
//! The mutants: mortar's static output `tests/ehowner/opts.mortar`
//! (linked with `--build-id -z execstack`), judged with its map's `arg`
//! records dropping, adding or changing an option the verifier reads
//! (`opts.rs`): with the caller's line (the link's real words) the
//! verdict is the unmutated one, word for word; without it the mutated
//! records are the options, and the verdict follows them.

use std::path::{Path, PathBuf};

use mortar_plumb::{Ctx, Files, Format, Input};

struct Dir<'a>(&'a Path);

impl Files for Dir<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ehowner")
}

fn line() -> Vec<String> {
    std::fs::read_to_string(dir().join("opts.mortar.line"))
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

/// The verdict on `opts.mortar` with the map's `arg` records `args`, and
/// the caller's line or none: each finding's rule and text.
fn verdict(args: &[String], caller: bool) -> Vec<String> {
    let d = dir();
    let text = std::fs::read_to_string(d.join("opts.mortar.map")).unwrap();
    let mut out = String::new();
    let mut done = false;
    for l in text.lines() {
        if l.starts_with("arg ") {
            if !done {
                for a in args {
                    out.push_str("arg ");
                    out.push_str(a);
                    out.push('\n');
                }
                done = true;
            }
            continue;
        }
        out.push_str(l);
        out.push('\n');
    }
    assert!(done, "the map has arg records");
    let map = mortar_plumb::map::parse(Format::Mortar, &out).unwrap();
    let inputs: Vec<(String, Vec<u8>)> = map
        .paths()
        .into_iter()
        .map(|p| (p.to_string(), std::fs::read(d.join(p)).unwrap()))
        .collect();
    let inputs: Vec<Input<'_>> = inputs.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let bytes = std::fs::read(d.join("opts.mortar")).unwrap();
    let ctx = Ctx { line: caller.then(line), files: &Dir(&d) };
    let r = mortar_plumb::verify_with(&bytes, &inputs, &map, &ctx);
    r.findings.iter().map(|f| format!("{} {} @{:x?}", f.rule.id(), f.message, f.addr)).collect()
}

/// The link's words with `edit` applied.
fn mutated(edit: &dyn Fn(&mut Vec<String>)) -> Vec<String> {
    let mut w = line();
    edit(&mut w);
    w
}

fn drop_word(w: &mut Vec<String>, s: &str) {
    let i = w.iter().position(|x| x == s).unwrap_or_else(|| panic!("no word {s}"));
    w.remove(i);
}

fn drop_z(w: &mut Vec<String>, k: &str) {
    let i = w.windows(2).position(|p| p[0] == "-z" && p[1] == k).unwrap();
    w.drain(i..i + 2);
}

fn add(w: &mut Vec<String>, s: &[&str]) {
    let at = w.iter().position(|x| x == "-o").unwrap();
    for (k, x) in s.iter().enumerate() {
        w.insert(at + k, (*x).to_string());
    }
}

#[allow(clippy::type_complexity)]
fn mutants() -> Vec<(&'static str, Box<dyn Fn(&mut Vec<String>)>, bool)> {
    // (name, edit, whether the verdict without the caller's line must
    // differ from the true one: the option is one a rule of this static
    // output reads and the image shows)
    vec![
        ("drop --build-id", Box::new(|w| drop_word(w, "--build-id")), true),
        ("drop -z execstack", Box::new(|w| drop_z(w, "execstack")), true),
        ("change --build-id to --no-build-id", Box::new(|w| {
            drop_word(w, "--build-id");
            add(w, &["--no-build-id"]);
        }), true),
        ("change --build-id to --build-id=sha256", Box::new(|w| {
            drop_word(w, "--build-id");
            add(w, &["--build-id=sha256"]);
        }), true),
        ("change -z execstack to -z noexecstack", Box::new(|w| {
            drop_z(w, "execstack");
            add(w, &["-z", "noexecstack"]);
        }), true),
        ("add --gdb-index", Box::new(|w| add(w, &["--gdb-index"])), false),
        ("add --strip-all", Box::new(|w| add(w, &["--strip-all"])), false),
        ("add --strip-debug", Box::new(|w| add(w, &["--strip-debug"])), false),
        ("add --gc-sections", Box::new(|w| add(w, &["--gc-sections"])), false),
        ("add --fix-cortex-a53-843419", Box::new(|w| add(w, &["--fix-cortex-a53-843419"])), false),
        ("add --keep-mason-meta", Box::new(|w| add(w, &["--keep-mason-meta"])), false),
        ("add --odr-table=full", Box::new(|w| add(w, &["--odr-table=full"])), false),
        ("add --no-odr-check", Box::new(|w| add(w, &["--no-odr-check"])), false),
        ("add -e fa", Box::new(|w| add(w, &["-e", "fa"])), false),
        ("add --export-dynamic", Box::new(|w| add(w, &["--export-dynamic"])), false),
        ("add --wrap=g", Box::new(|w| add(w, &["--wrap=g"])), false),
        ("add --defsym=zz=0x10", Box::new(|w| add(w, &["--defsym=zz=0x10"])), false),
        ("add -pie", Box::new(|w| add(w, &["-pie"])), false),
        ("add -z now -z relro", Box::new(|w| add(w, &["-z", "now", "-z", "relro"])), false),
        ("add --hash-style=gnu", Box::new(|w| add(w, &["--hash-style=gnu"])), false),
        ("change max-page-size", Box::new(|w| {
            drop_z(w, "max-page-size=4096");
            add(w, &["-z", "max-page-size=65536"]);
        }), false),
        ("drop -static", Box::new(|w| drop_word(w, "-static")), false),
    ]
}

#[test]
fn s3_2a_the_true_records_judge_alike_with_and_without_the_line() {
    let truth = line();
    assert_eq!(verdict(&truth, true), verdict(&truth, false));
}

#[test]
fn s3_2a_mutated_arg_records_give_the_callers_verdict() {
    let truth = verdict(&line(), true);
    let mut differ = Vec::new();
    for (name, edit, must_differ) in mutants() {
        let args = mutated(&*edit);
        assert_eq!(verdict(&args, true), truth, "{name}: the caller's line decides");
        let alone = verdict(&args, false);
        if alone != truth {
            differ.push(name);
        }
        if must_differ {
            assert_ne!(alone, truth, "{name}: without the caller's line the records decide");
        }
    }
    // Every mutant the image shows is seen without the line.
    assert!(differ.len() >= 5, "{differ:?}");
}

#[test]
fn s3_2a_m9_judges_the_maps_records_whatever_the_caller_gives() {
    let args = mutated(&|w| add(w, &["--no-such-option"]));
    for caller in [true, false] {
        let v = verdict(&args, caller);
        assert!(
            v.iter().any(|f| f.starts_with("M9 ") && f.contains("--no-such-option")),
            "caller {caller}: {v:#?}"
        );
    }
}
