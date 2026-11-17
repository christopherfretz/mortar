//! resolve.md §9.7 over the 213 version scripts of
//! `tests/shared-design/exp3/vs3/` (WP-110 record §G.6), table-driven:
//! each case's scripts compiled by [`VersionScript::load`] and every name
//! of its object decided ([`VersionScript::decide`], or
//! [`VersionScript::symver`] for its `.symver` definitions), against the
//! author's model's mortar reading (`model_a.py`, mode `mortar`), the
//! fixture `tests/data/version_cases.txt` that `mortar_expect.py` writes.
//!
//! They agree on 195 of the 213. The model and mortar disagree on exactly
//! the 18 cases in [`EXTERN_CXX`]:
//! the model matches `extern "C++"` entries against demangled names,
//! mortar refuses every such script by name (§9.7 rule 6, D-114, kind
//! `script-command`, until WP-129's demangler).

use mortar_resolve::version::{Decision, SourceFile, VersionScript};

/// The cases with an `extern "C++"` entry: mortar refuses the script.
const EXTERN_CXX: &[&str] = &[
    "A-X1", "A-X2", "A-X3", "A-X5", "A-X6", "A-X7", "A-X8", "A-X9", "A-X10", "R-r20", "R-r21",
    "R-r22", "R-r23", "R-r24", "R-r25", "R-r26", "R-r48", "D-d16",
];

struct Case {
    id: String,
    export_dynamic: bool,
    names: Vec<String>,
    versioned: Vec<(String, String)>,
    files: Vec<String>,
    expect: String,
}

fn cases() -> Vec<Case> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/version_cases.txt");
    let text = std::fs::read_to_string(path).expect("the fixture");
    let mut out: Vec<Case> = Vec::new();
    for line in text.lines() {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        match word {
            "case" => {
                let (id, e) = rest.split_once(' ').expect("case <id> <E|->");
                out.push(Case {
                    id: id.to_owned(),
                    export_dynamic: e == "E",
                    names: Vec::new(),
                    versioned: Vec::new(),
                    files: Vec::new(),
                    expect: String::new(),
                });
            }
            _ => {
                let c = out.last_mut().expect("a case first");
                match word {
                    "names" => c.names = rest.split(' ').map(str::to_owned).collect(),
                    "versioned" => {
                        c.versioned = rest
                            .split(' ')
                            .filter(|s| !s.is_empty())
                            .map(|kv| {
                                let (k, v) = kv.split_once('=').expect("name=V");
                                (k.to_owned(), v.to_owned())
                            })
                            .collect();
                    }
                    "file" => c.files.push(String::new()),
                    "|" => {
                        let f = c.files.last_mut().expect("file first");
                        if !f.is_empty() {
                            f.push('\n');
                        }
                        f.push_str(rest);
                    }
                    "expect" => c.expect = rest.to_owned(),
                    _ => panic!("fixture line {line:?}"),
                }
            }
        }
    }
    out
}

/// Mortar's reading of a case, in the fixture's `expect` form.
fn mortar(c: &Case) -> String {
    let paths: Vec<String> = (0..c.files.len()).map(|i| format!("{}.{i}.map", c.id)).collect();
    let files: Vec<SourceFile<'_>> = c
        .files
        .iter()
        .zip(&paths)
        .map(|(t, p)| SourceFile { path: p, bytes: t.as_bytes() })
        .collect();
    let Ok(s) = VersionScript::load(&files) else {
        return "REFUSED".to_owned();
    };
    let name = |d: Decision| match d {
        Decision::Local => "local".to_owned(),
        Decision::Base => "base".to_owned(),
        Decision::Node(n) => s.nodes()[n]
            .name
            .as_deref()
            .map_or_else(|| "base".to_owned(), |b| String::from_utf8_lossy(b).into_owned()),
    };
    let mut out = Vec::new();
    for n in &c.names {
        let d = match c.versioned.iter().find(|(k, _)| k == n) {
            Some((_, v)) => match s.symver(n.as_bytes(), v.as_bytes(), c.export_dynamic) {
                Some(d) => d,
                None => return "REFUSED".to_owned(),
            },
            None => s.decide(n.as_bytes()),
        };
        out.push(format!("{n}={}", name(d)));
    }
    out.join(" ")
}

#[test]
fn s9_7_the_213_scripts() {
    let cases = cases();
    assert_eq!(cases.len(), 213);
    let mut agree = 0;
    let mut differ = Vec::new();
    for c in &cases {
        let got = mortar(c);
        if got == c.expect {
            agree += 1;
        } else {
            let cxx = c.files.iter().any(|f| f.contains("\"C++\""));
            differ.push(c.id.clone());
            eprintln!("{} (C++ entry: {cxx}):\n  model  {}\n  mortar {got}", c.id, c.expect);
        }
    }
    eprintln!("agree {agree} of {}", cases.len());
    assert_eq!(differ, EXTERN_CXX, "the disagreements are the extern \"C++\" cases");
    for id in EXTERN_CXX {
        let c = cases.iter().find(|c| c.id == *id).unwrap();
        assert!(c.files.iter().any(|f| f.contains("\"C++\"")), "{id}");
        assert_eq!(mortar(c), "REFUSED", "{id}");
    }
}
