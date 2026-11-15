//! Response files on disk (`spec/cli.md` §2.6).

use std::ffi::OsString;

use mortar_cli::{InputSpec, parse};
use mortar_diag::Style;
use mortar_testutil::Scratch;

fn at(path: &std::path::Path) -> OsString {
    let mut arg = OsString::from("@");
    arg.push(path);
    arg
}

fn errors(parsed: &mortar_cli::Parsed) -> String {
    parsed.diagnostics.iter().map(|(_, d)| d.render(&Style::plain("ld"))).collect()
}

// spec: cli.md §2.6 (expansion in place, nested; indexes count in the
// expanded line)
#[test]
fn expands_in_place_and_nested() {
    let scratch = Scratch::new("rsp-nested");
    let inner = scratch.write("inner.rsp", "-lm 'b c.o'\n");
    let outer = scratch.write("outer.rsp", format!("-o out @{} d.o", inner.display()));
    let parsed = parse(&["a.o".into(), at(&outer), "e.o".into()]);
    assert_eq!(errors(&parsed), "");
    let expanded: Vec<String> =
        parsed.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    assert_eq!(expanded, ["a.o", "-o", "out", "-lm", "b c.o", "d.o", "e.o"]);
    assert_eq!(parsed.plan.output, Some("out".into()));
    let inputs: Vec<(u32, InputSpec)> =
        parsed.plan.inputs.iter().map(|i| (i.arg, i.spec.clone())).collect();
    assert_eq!(
        inputs,
        [
            (0, InputSpec::Path("a.o".into())),
            (3, InputSpec::Library("m".into())),
            (4, InputSpec::Path("b c.o".into())),
            (5, InputSpec::Path("d.o".into())),
            (6, InputSpec::Path("e.o".into())),
        ]
    );
}

// spec: cli.md §2.6.2 (a file that cannot be opened is an ordinary
// argument; an empty file is no arguments; `@` alone is an argument)
#[test]
fn missing_and_empty_files() {
    let scratch = Scratch::new("rsp-missing");
    let missing = scratch.join("nope.rsp");
    let empty = scratch.write("empty.rsp", "");
    let parsed = parse(&[at(&missing), at(&empty), "@".into()]);
    assert_eq!(errors(&parsed), "");
    assert_eq!(parsed.args, [at(&missing), "@".into()]);
}

// spec: cli.md §2.6.2 (a directory is an error)
#[test]
fn a_directory_is_an_error() {
    let scratch = Scratch::new("rsp-dir");
    let parsed = parse(&[at(scratch.path()), "a.o".into()]);
    assert_eq!(
        errors(&parsed),
        format!(
            "ld: error: argument '@{0}': cannot open {0}: is a directory\n",
            scratch.path().display()
        )
    );
}

// spec: cli.md §2.6.3 (at most 1999 expansions: a file that names itself)
#[test]
fn the_expansion_limit() {
    let scratch = Scratch::new("rsp-limit");
    let path = scratch.join("self.rsp");
    std::fs::write(&path, format!("x.o @{}", path.display())).expect("write");
    let parsed = parse(&[at(&path)]);
    let report = errors(&parsed);
    assert!(report.ends_with("too many response files: more than 1999 expanded\n"), "{report}");
    assert_eq!(parsed.plan.inputs.len(), 1999);
    // 1999 nested files are fine.
    let mut next = scratch.write("leaf.rsp", "leaf.o");
    for n in 0..1998 {
        next = scratch.write(&format!("f{n}.rsp"), format!("@{}", next.display()));
    }
    let parsed = parse(&[at(&next)]);
    assert_eq!(errors(&parsed), "");
    assert_eq!(parsed.args, ["leaf.o"]);
}
