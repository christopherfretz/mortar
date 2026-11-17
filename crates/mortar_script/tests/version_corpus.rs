//! The 213 version scripts of `tests/shared-design/exp3/vs3/` (WP-110
//! record §G.6: `cases.py`, the union of the second pass's 53, the
//! re-review's 60 and the third pass's 100), read from the directory as
//! `cases.py` reads them, each parsed as `spec/script.md` §9.1 says.
//!
//! Every one of them parses: none is a syntax error under §9.1. The
//! references' disagreements on SYNTAX among them are asserted where the
//! spec decides (the lists below; record `spec/landed/dyn/WP-125.md`).
//! GNU ld's other refusals of these scripts (`duplicate expression`, a
//! `.symver` version missing) are resolve §9.7's, not the parser's.
//!
//! The expected trees are the WP-110 author's model's reading
//! (`model_a.py`'s `parse`, the oracle of `compare.py`), rendered as
//! `NAME{<scope><lang>:<text>,...}` per node: scope `g`/`l`, lang `C` for
//! plain and `extern "C"` entries (the model does not tell them apart),
//! `C++` for `extern "C++"`, a quoted entry's text in its quotes. Parents
//! are not in the model; they are asserted for the cases that have them.

use std::path::{Path, PathBuf};

use mortar_script::version::{SourceFile, parse_version_script};
use mortar_script::{Language, Scope, VersionScript};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/shared-design/exp3/vs3")
}

fn read(name: &str) -> String {
    let path = dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The Python string literals of a line, in order, escapes `\\`, `\'`,
/// `\"` and `\n` processed (the only ones the files use).
fn literals(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\'' && c != '"' {
            continue;
        }
        let mut s = String::new();
        loop {
            match chars.next() {
                None => panic!("unterminated literal in {line:?}"),
                Some('\\') => match chars.next() {
                    Some('n') => s.push('\n'),
                    Some(e @ ('\\' | '\'' | '"')) => s.push(e),
                    other => panic!("escape {other:?} in {line:?}"),
                },
                Some(q) if q == c => break,
                Some(x) => s.push(x),
            }
        }
        out.push(s);
    }
    out
}

/// The `S = { ... }` dict entries of a generator script: `(key, text)`.
fn dict_entries(name: &str) -> Vec<(String, String)> {
    let text = read(name);
    let block = text.split("\nS = {\n").nth(1).expect("an S dict").split("\n}\n").next().unwrap();
    block
        .lines()
        .map(|l| {
            let lits = literals(l);
            assert_eq!(lits.len(), 2, "{l:?}");
            (lits[0].split_whitespace().next().unwrap().to_owned(), lits[1].clone())
        })
        .collect()
}

/// The cases of `cases.py`, in its order: `(id, file texts)`.
fn cases() -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for line in read("scripts_author.txt").lines() {
        if let Some((k, s)) = line.split_once('|') {
            out.push((format!("A-{k}"), vec![s.to_owned()]));
        }
    }
    for line in read("scripts_rev.py").lines().filter(|l| l.starts_with("(\"")) {
        let lits = literals(line);
        assert!(lits.len() >= 4, "{line:?}");
        out.push((format!("R-{}", lits[0]), lits[1..lits.len() - 2].to_vec()));
    }
    for line in read("globs.out").lines() {
        let id = line.split(' ').next().unwrap_or_default();
        if id.len() == 3 && id.starts_with('g') && id[1..].bytes().all(|b| b.is_ascii_digit()) {
            out.push((format!("G-{id}"), vec![read(&format!("{id}.map")).trim().to_owned()]));
        }
    }
    for (k, s) in dict_entries("dups.py") {
        out.push((format!("D-{k}"), vec![s]));
    }
    for file in ["expdyn.py", "atat.py"] {
        for (k, s) in dict_entries(file) {
            out.push((format!("S-{k}"), vec![s.clone()]));
            out.push((format!("S-{k}E"), vec![s]));
        }
    }
    out
}

fn parse(id: &str, files: &[String]) -> VersionScript {
    let paths: Vec<String> = (0..files.len()).map(|i| format!("{id}.{i}.map")).collect();
    let sources: Vec<SourceFile<'_>> = files
        .iter()
        .zip(&paths)
        .map(|(t, p)| SourceFile { path: p, bytes: t.as_bytes() })
        .collect();
    parse_version_script(&sources)
        .unwrap_or_else(|d| panic!("{id}: {}", d.render(&mortar_diag::Style::plain("ld"))))
}

fn digest(script: &VersionScript) -> String {
    let nodes: Vec<String> = script
        .nodes
        .iter()
        .map(|n| {
            let entries: Vec<String> = n
                .entries
                .iter()
                .map(|e| {
                    let scope = if e.scope == Scope::Global { 'g' } else { 'l' };
                    let lang = if e.language == Language::Cxx { "C++" } else { "C" };
                    let text = e.pattern.to_string_lossy();
                    if e.pattern.quoted {
                        format!("{scope}{lang}:\"{text}\"")
                    } else {
                        format!("{scope}{lang}:{text}")
                    }
                })
                .collect();
            let name = n.name.as_ref().map_or_else(|| "<anon>".to_owned(), |w| w.to_string_lossy());
            format!("{name}{{{}}}", entries.join(","))
        })
        .collect();
    nodes.join(" ")
}

// spec: script.md §9.1, §9.3 (the 213 scripts of exp3/vs3 parse, each to
// the tree the WP-110 model reads)
#[test]
fn vs3_scripts_parse_to_the_models_trees() {
    let cases = cases();
    assert_eq!(cases.len(), 213, "cases.py's union");
    assert_eq!(cases.len(), EXPECTED.len());
    for ((id, files), (want_id, want)) in cases.iter().zip(EXPECTED) {
        assert_eq!(id, want_id, "the order of cases.py");
        assert_eq!(digest(&parse(id, files)), *want, "{id}: {files:?}");
    }
}

// spec: script.md §9.1 (gnuref.json, GNU ld's link of each case, names the
// same 213 cases: the reference this corpus was measured against)
#[test]
fn vs3_cases_are_gnurefs_cases() {
    let json = read("gnuref.json");
    let mut ids: Vec<&str> = json
        .lines()
        .filter_map(|l| {
            // A case is a top-level key: its value is a `{` or "REFUSED"
            // (a symbol's value is a version, `base` or `local`).
            let (key, value) = l.strip_prefix('"')?.split_once("\": ")?;
            (value == "{" || value.starts_with("\"REFUSED\"")).then_some(key)
        })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let mut ours: Vec<String> = cases().into_iter().map(|(id, _)| id).collect();
    ours.sort();
    assert_eq!(ids, ours);
}

/// `local:` before `global:` in a node (rule 8's family): GNU ld reports a
/// syntax error; lld, mold and mortar accept (§9.1). Measured on AArch64
/// with GNU ld 2.44, lld 22, mold 2.37 (record).
const LOCAL_BEFORE_GLOBAL: &[&str] =
    &["A-S8", "A-L1", "A-L2", "A-E6", "R-r11", "R-r32", "R-r47", "R-s10"];

/// Several parents (`V3 { … } V2 V1;`): GNU ld and mortar accept; lld and
/// mold report ``; expected, but got V1``.
const SEVERAL_PARENTS: &[&str] = &["R-r27"];

/// Globs lld and mold refuse (an unterminated `[`, a final `\\`, an empty or
/// reversed range): GNU ld accepts each, and so does mortar's parser,
/// which keeps the raw text (§9.1; their meaning is resolve §9.7's).
const GLOBS_LLD_OR_MOLD_REFUSE: &[&str] =
    &["G-g05", "G-g10", "G-g11", "G-g26", "G-g35", "G-g36", "G-g37", "G-g38", "G-g39", "G-g41"];

// spec: script.md §9.1 (where GNU ld, lld and mold disagree on the syntax
// of these scripts, the spec's choice: each list parses, with the
// structure that makes it disagree)
#[test]
fn vs3_syntax_disagreements_take_the_specs_side() {
    let cases = cases();
    let get = |id: &str| {
        let (_, files) = cases.iter().find(|(i, _)| i == id).unwrap_or_else(|| panic!("{id}"));
        parse(id, files)
    };
    for id in LOCAL_BEFORE_GLOBAL {
        let script = get(id);
        assert!(
            script.nodes.iter().any(|n| {
                let first_local = n.entries.iter().position(|e| e.scope == Scope::Local);
                let last_global = n.entries.iter().rposition(|e| e.scope == Scope::Global);
                matches!((first_local, last_global), (Some(l), Some(g)) if l < g)
            }),
            "{id}"
        );
    }
    for id in SEVERAL_PARENTS {
        let script = get(id);
        let v3 = &script.nodes[2];
        let parents: Vec<(String, usize)> =
            v3.parents.iter().map(|p| (p.name.to_string_lossy(), p.node)).collect();
        assert_eq!(parents, [("V2".to_owned(), 1), ("V1".to_owned(), 0)], "{id}");
    }
    for id in GLOBS_LLD_OR_MOLD_REFUSE {
        let script = get(id);
        let text = String::from_utf8_lossy(&script.nodes[0].entries[0].pattern.text).into_owned();
        assert!(text.contains('[') || text.ends_with('\\'), "{id}: {text}");
    }
}

// spec: script.md §9.1 (parents of the corpus resolve to earlier nodes,
// also across files: R-r38's second file names the first file's V1)
#[test]
fn vs3_parents() {
    let cases = cases();
    let with_parents: Vec<(String, Vec<Vec<usize>>)> = cases
        .iter()
        .map(|(id, files)| {
            let s = parse(id, files);
            (
                id.clone(),
                s.nodes
                    .iter()
                    .map(|n| n.parents.iter().map(|p| p.node).collect())
                    .collect::<Vec<Vec<usize>>>(),
            )
        })
        .filter(|(_, p)| p.iter().any(|x| !x.is_empty()))
        .collect();
    let ids: Vec<&str> = with_parents.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["R-r27", "R-r38", "R-s01", "R-s06", "S-e12", "S-e12E"]);
    let r38 = cases.iter().find(|(i, _)| i == "R-r38").unwrap();
    let s = parse(&r38.0, &r38.1);
    assert_eq!((s.nodes[1].file, s.nodes[1].parents[0].node), (1, 0));
}

/// `model_a.py`'s reading of each case (generated by running its `parse`
/// over the cases in `cases.py`'s order; parents left out).
const EXPECTED: &[(&str, &str)] = &[
    ("A-S1", r#"V1{lC:foo*} V2{gC:fo*}"#),
    ("A-S2", r#"V1{gC:fo*} V2{lC:foo*}"#),
    ("A-S5", r#"V1{gC:f?o} V2{gC:[f]oo}"#),
    ("A-S6", r#"V1{gC:*} V2{gC:foo*}"#),
    ("A-S6r", r#"V1{gC:foo*} V2{gC:*}"#),
    ("A-S7", r#"V1{gC:foo*,lC:fo*}"#),
    ("A-S8", r#"V1{lC:foo*,gC:fo*}"#),
    ("A-S9", r#"V1{gC:*,lC:foo*}"#),
    ("A-S10", r#"V1{gC:*} V2{lC:foo*}"#),
    ("A-S10r", r#"V1{lC:foo*} V2{gC:*}"#),
    ("A-S11", r#"V1{gC:fo*,lC:*} V2{lC:foo*}"#),
    ("A-S12", r#"V1{gC:foo} V2{lC:foo}"#),
    ("A-S13", r#"V1{lC:foo} V2{gC:foo}"#),
    ("A-S14", r#"V1{gC:f*} V2{gC:fo*} V3{gC:foo*}"#),
    ("A-S15", r#"V1{gC:foo*} V2{gC:fo*} V3{gC:f*}"#),
    ("A-S16", r#"V1{gC:*,lC:*}"#),
    ("A-S17", r#"V1{lC:*} V2{gC:foo*}"#),
    ("A-S18", r#"V1{gC:*o*} V2{lC:*}"#),
    ("A-S19", r#"V1{gC:foo} V2{gC:foo}"#),
    ("A-Q1", r#"V1{gC:"foo*"}"#),
    ("A-Q2", r#"V1{gC:"foo",lC:*}"#),
    ("A-Q3", r#"V1{gC:f*} V2{gC:"foo"}"#),
    ("A-Q4", r#"V1{gC:"fo*",lC:*}"#),
    ("A-D1", r#"V1{gC:foo*} V2{gC:foo*}"#),
    ("A-D2", r#"V1{gC:foo*} V2{lC:foo*}"#),
    ("A-D3", r#"V1{lC:foo*} V2{gC:foo*}"#),
    ("A-D4", r#"V1{gC:*} V2{lC:*}"#),
    ("A-D5", r#"V1{gC:foo,lC:foo}"#),
    ("A-D6", r#"V1{gC:foo,gC:foo}"#),
    ("A-D7", r#"V1{lC:foo} V2{lC:foo}"#),
    ("A-L1", r#"V1{lC:*,gC:foo}"#),
    ("A-L2", r#"V1{lC:foo,gC:fo*}"#),
    ("A-E1", r#"V1{gC:foo} V2{lC:fo*}"#),
    ("A-E2", r#"V1{lC:foo} V2{gC:fo*}"#),
    ("A-E3", r#"V1{gC:fo*} V2{gC:foo}"#),
    ("A-E4", r#"V1{gC:fo*} V2{lC:foo}"#),
    ("A-E5", r#"V1{gC:foo*,lC:*} V2{gC:*}"#),
    ("A-E6", r#"V1{lC:f*} V2{lC:fo*,gC:*}"#),
    ("A-E7", r#"V1{gC:[fb]oo,lC:*}"#),
    ("A-E8", r#"V1{gC:f?o,lC:f*}"#),
    ("A-E9", r#"V1{gC:*,lC:foo}"#),
    ("A-E10", r#"<anon>{gC:fo*,lC:*}"#),
    ("A-X1", r#"V1{gC++:"foo(int)",lC:*}"#),
    ("A-X2", r#"V1{gC++:foo*,lC:*}"#),
    ("A-X3", r#"V1{gC++:ns::*,lC:*}"#),
    ("A-X4", r#"V1{gC:_Z3foo*,lC:*}"#),
    ("A-X5", r#"V1{gC++:foo*} V2{gC:_Z3fooi}"#),
    ("A-X6", r#"V1{gC++:"foo(int)"} V2{gC:_Z*}"#),
    ("A-X7", r#"V1{gC++:*,lC:*}"#),
    ("A-X8", r#"V1{gC++:"ns::foo()"} V2{lC++:ns::*}"#),
    ("A-X9", r#"V1{gC++:fo*,lC:*}"#),
    ("A-X10", r#"V1{gC++:foo,lC:*}"#),
    ("A-X11", r#"V1{gC:fo*,lC:*}"#),
    ("R-r01", r#"V1{gC:foo,gC:bar} V2{gC:foo,gC:baz} V3{gC:foo,lC:*}"#),
    ("R-r02", r#"V1{gC:f*,gC:fo*} V2{gC:foo*}"#),
    ("R-r03", r#"V1{gC:fo?} V2{gC:f??2,lC:*}"#),
    ("R-r04", r#"V1{gC:f[o]o} V2{gC:[bf]a*,lC:*}"#),
    ("R-r05", r#"V1{gC:*} V2{gC:*} V3{lC:*}"#),
    ("R-r06", r#"V1{gC:*} V2{gC:*}"#),
    ("R-r07", r#"V1{lC:*} V2{lC:*} V3{gC:foo}"#),
    ("R-r08", r#"V1{gC:"foo"} V2{lC:foo}"#),
    ("R-r09", r#"V1{gC:"fo*",lC:*}"#),
    ("R-r10", r#"V1{gC:"f*"} V2{gC:f*}"#),
    ("R-r11", r#"V1{lC:foo,gC:*}"#),
    ("R-r12", r#"V1{gC:foo,lC:foo}"#),
    ("R-r13", r#"V1{gC:f*,lC:foo}"#),
    ("R-r14", r#"V1{gC:foo} V2{gC:*,lC:foo*}"#),
    ("R-r15", r#"V1{gC:fo*} V2{gC:*,lC:f*}"#),
    ("R-r16", r#"V1{lC:fo*} V2{gC:f*}"#),
    ("R-r17", r#"V1{lC:f*} V2{gC:*}"#),
    ("R-r18", r#"<anon>{gC:foo,gC:bar,lC:*}"#),
    ("R-r19", r#"<anon>{gC:f*}"#),
    ("R-r20", r#"V1{gC++:"foo(int)",lC:*}"#),
    ("R-r21", r#"V1{gC++:ns::*,lC:*}"#),
    ("R-r22", r#"V1{gC++:"ns::bar<int>()",gC++:ns::foo*} V2{gC:_Z*}"#),
    ("R-r23", r#"V1{gC++:*foo*,gC:foo,lC:*}"#),
    ("R-r24", r#"V1{gC:foo,gC++:foo*,lC:*}"#),
    ("R-r25", r#"V1{gC:_Z*} V2{gC++:"foo(int)"}"#),
    ("R-r26", r#"V1{gC++:"foo(int)"} V2{lC:_Z3fooi}"#),
    ("R-r27", r#"V1{gC:foo} V2{gC:bar} V3{gC:baz}"#),
    ("R-r28", r#"V1{gC:f?o,lC:fo?}"#),
    ("R-r29", r#"V1{lC:fo?} V2{lC:*} V3{gC:*}"#),
    ("R-r30", r#"V1{gC:*o*} V2{gC:*oo} V3{lC:foo}"#),
    ("R-r31", r#"V1{lC:foo} V2{gC:f*}"#),
    ("R-r32", r#"V1{gC:f*} V2{lC:fo*,gC:foo2}"#),
    ("R-r33", r#"V1{gC:[a-c]*,lC:*}"#),
    ("R-r34", r#"V1{gC:[!f]*,lC:*}"#),
    ("R-r35", r#"V1{gC:[^f]*,lC:*}"#),
    ("R-r36", r#"V1{gC:?a?,lC:*}"#),
    ("R-r37", r#"V1{gC:foo} V1b{lC:*}"#),
    ("R-r38", r#"V1{gC:foo} V2{gC:foo*,lC:*}"#),
    ("R-r39", r#"V1{gC:f*} V2{lC:foo}"#),
    ("R-r40", r#"V1{gC:bar} V2{lC:bar}"#),
    ("R-r41", r#"V1{gC:foo,lC:*} V2{gC:foo2}"#),
    ("R-r42", r#"V1{gC:*,lC:*}"#),
    ("R-r43", r#"V1{gC:f*,lC:f*}"#),
    ("R-r44", r#"V1{gC:fo*} V2{gC:fo*}"#),
    ("R-r45", r#"V1{gC:foo} V2{gC:"foo"}"#),
    ("R-r46", r#"V1{lC:"foo"} V2{gC:foo}"#),
    ("R-r47", r#"V1{gC:*} V2{lC:f*,gC:foo}"#),
    ("R-r48", r#"V1{gC++:foo*} V2{lC:*}"#),
    ("R-r49", r#"V1{gC:f*} V2{gC:foo,lC:*}"#),
    ("R-r50", r#"V1{gC:fo\*,lC:*}"#),
    ("R-s01", r#"V1{gC:bar,lC:*} V2{gC:cur}"#),
    ("R-s02", r#"V1{gC:old,lC:*} V2{gC:cur}"#),
    ("R-s03", r#"V1{gC:o*,lC:old} V2{gC:cur}"#),
    ("R-s04", r#"V1{gC:*,lC:old} V2{gC:cur}"#),
    ("R-s05", r#"V1{lC:o*} V2{gC:cur}"#),
    ("R-s06", r#"V1{gC:bar,lC:*} V2{gC:cur}"#),
    ("R-s07", r#"V1{} V2{gC:cur,lC:*}"#),
    ("R-s08", r#"V1{gC:old} V2{gC:cur,lC:*}"#),
    ("R-s09", r#"V1{gC:ol?,lC:old} V2{gC:cur}"#),
    ("R-s10", r#"V1{lC:*,gC:old} V2{gC:cur}"#),
    ("G-g00", r#"V1{gC:f[^o]o,lC:*}"#),
    ("G-g01", r#"V1{gC:f[!o]o,lC:*}"#),
    ("G-g02", r#"V1{gC:f[^o]*,lC:*}"#),
    ("G-g03", r#"V1{gC:f[!o]*,lC:*}"#),
    ("G-g04", r#"V1{gC:f[a-o]?,lC:*}"#),
    ("G-g05", r#"V1{gC:f[o-a]?,lC:*}"#),
    ("G-g06", r#"V1{gC:[]]oo,lC:*}"#),
    ("G-g07", r#"V1{gC:[]o]oo,lC:*}"#),
    ("G-g08", r#"V1{gC:[!]]oo,lC:*}"#),
    ("G-g09", r#"V1{gC:[^]]oo,lC:*}"#),
    ("G-g10", r#"V1{gC:f[o,lC:*}"#),
    ("G-g11", r#"V1{gC:[oo,lC:*}"#),
    ("G-g12", r#"V1{gC:fo\*,lC:*}"#),
    ("G-g13", r#"V1{gC:fo*,lC:*}"#),
    ("G-g14", r#"V1{gC:"fo*",lC:*}"#),
    ("G-g15", r#"V1{gC:fo\o,lC:*}"#),
    ("G-g16", r#"V1{gC:foo\,lC:*}"#),
    ("G-g17", r#"V1{gC:"foo\\",lC:*}"#),
    ("G-g18", r#"V1{gC:foo\\,lC:*}"#),
    ("G-g19", r#"V1{gC:foo*,lC:*}"#),
    ("G-g20", r#"V1{gC:fo[o]*,lC:*}"#),
    ("G-g21", r#"V1{gC:f*o,lC:*}"#),
    ("G-g22", r#"V1{gC:f*,lC:*}"#),
    ("G-g23", r#"V1{gC:*o,lC:*}"#),
    ("G-g24", r#"V1{gC:[[]oo,lC:*}"#),
    ("G-g25", r#"V1{gC:[-]oo,lC:*}"#),
    ("G-g26", r#"V1{gC:[a-]oo,lC:*}"#),
    ("G-g27", r#"V1{gC:[\]]oo,lC:*}"#),
    ("G-g28", r#"V1{gC:f[.]o,lC:*}"#),
    ("G-g29", r#"V1{gC:f?o,lC:*}"#),
    ("G-g30", r#"V1{gC:???,lC:*}"#),
    ("G-g31", r#"V1{gC:\fo?,lC:*}"#),
    ("G-g32", r#"V1{gC:fo[*],lC:*}"#),
    ("G-g33", r#"V1{gC:[!a-z]oo,lC:*}"#),
    ("G-g34", r#"V1{gC:[^a-z]oo,lC:*}"#),
    ("G-g35", r#"V1{gC:[o*,lC:*}"#),
    ("G-g36", r#"V1{gC:f[o*,lC:*}"#),
    ("G-g37", r#"V1{gC:f*\,lC:*}"#),
    ("G-g38", r#"V1{gC:foo*\,lC:*}"#),
    ("G-g39", r#"V1{gC:fo[o-o]\,lC:*}"#),
    ("G-g40", r#"V1{gC:*,lC:*}"#),
    ("G-g41", r#"V1{gC:fo[],lC:*}"#),
    ("G-g42", r#"V1{gC:fo[!],lC:*}"#),
    ("D-d01", r#"V1{gC:foo} V2{lC:"foo"}"#),
    ("D-d02", r#"V1{gC:fo\o} V2{lC:foo}"#),
    ("D-d03", r#"V1{gC:"fo*"} V2{lC:fo\*}"#),
    ("D-d04", r#"V1{gC:fo*} V2{lC:"fo*"}"#),
    ("D-d05", r#"V1{gC:fo*} V2{lC:fo*}"#),
    ("D-d06", r#"V1{gC:f[o]o} V2{lC:foo}"#),
    ("D-d07", r#"V1{gC:fo?} V2{lC:fo[?]}"#),
    ("D-d08", r#"V1{gC:fo\*} V2{lC:fo\*}"#),
    ("D-d09", r#"V1{gC:foo} V2{lC:foo}"#),
    ("D-d10", r#"V1{gC:foo} V2{lC:foo} V3{gC:foo}"#),
    ("D-d11", r#"V1{gC:foo,lC:foo}"#),
    ("D-d12", r#"V1{gC:foo} V2{gC:foo}"#),
    ("D-d13", r#"V1{lC:foo} V2{lC:foo}"#),
    ("D-d14", r#"V1{gC:*} V2{lC:*}"#),
    ("D-d15", r#"V1{gC:fo*} V2{lC:fo**}"#),
    ("D-d16", r#"V1{gC++:foo} V2{lC:foo}"#),
    ("D-d17", r#"V1{gC:fo\*} V2{lC:"fo*"}"#),
    ("S-e01", r#"V1{gC:bar,lC:foo}"#),
    ("S-e01E", r#"V1{gC:bar,lC:foo}"#),
    ("S-e02", r#"V1{gC:bar,lC:f*}"#),
    ("S-e02E", r#"V1{gC:bar,lC:f*}"#),
    ("S-e03", r#"V1{gC:bar,lC:*}"#),
    ("S-e03E", r#"V1{gC:bar,lC:*}"#),
    ("S-e04", r#"<anon>{gC:bar,lC:*}"#),
    ("S-e04E", r#"<anon>{gC:bar,lC:*}"#),
    ("S-e05", r#"V1{gC:bar} V2{lC:*}"#),
    ("S-e05E", r#"V1{gC:bar} V2{lC:*}"#),
    ("S-e06", r#"V1{gC:bar,lC:old}"#),
    ("S-e06E", r#"V1{gC:bar,lC:old}"#),
    ("S-e07", r#"V1{gC:bar,lC:o*}"#),
    ("S-e07E", r#"V1{gC:bar,lC:o*}"#),
    ("S-e08", r#"V1{lC:*}"#),
    ("S-e08E", r#"V1{lC:*}"#),
    ("S-e09", r#"V1{gC:o*,lC:old}"#),
    ("S-e09E", r#"V1{gC:o*,lC:old}"#),
    ("S-e10", r#"V1{gC:bar} V2{lC:old}"#),
    ("S-e10E", r#"V1{gC:bar} V2{lC:old}"#),
    ("S-e11", r#"V1{gC:bar,lC:cur}"#),
    ("S-e11E", r#"V1{gC:bar,lC:cur}"#),
    ("S-e12", r#"V1{gC:bar,lC:*} V2{gC:foo}"#),
    ("S-e12E", r#"V1{gC:bar,lC:*} V2{gC:foo}"#),
    ("S-e13", r#"V1{gC:bar,lC:old_impl}"#),
    ("S-e13E", r#"V1{gC:bar,lC:old_impl}"#),
    ("S-a01", r#"V1{gC:bar} V2{gC:cur}"#),
    ("S-a01E", r#"V1{gC:bar} V2{gC:cur}"#),
    ("S-a02", r#"V1{gC:bar} V2{gC:c*}"#),
    ("S-a02E", r#"V1{gC:bar} V2{gC:c*}"#),
    ("S-a03", r#"V1{gC:bar,lC:c*}"#),
    ("S-a03E", r#"V1{gC:bar,lC:c*}"#),
    ("S-a04", r#"V1{gC:c*,lC:cur}"#),
    ("S-a04E", r#"V1{gC:c*,lC:cur}"#),
    ("S-a05", r#"V1{gC:bar} V2{gC:old}"#),
    ("S-a05E", r#"V1{gC:bar} V2{gC:old}"#),
    ("S-a06", r#"V1{gC:old,lC:*}"#),
    ("S-a06E", r#"V1{gC:old,lC:*}"#),
    ("S-a07", r#"V1{gC:bar} V2{gC:*}"#),
    ("S-a07E", r#"V1{gC:bar} V2{gC:*}"#),
];
