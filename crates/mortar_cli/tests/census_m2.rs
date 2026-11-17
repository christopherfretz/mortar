//! The M2 census (`spec/dyn.md` §2, §2.7, R-C1; WP-042): every one of the
//! 426 link lines the census's drivers pass to `ld` for a dynamic link
//! (`tests/data/census-m2.tsv`) parses without an error, names a program
//! interpreter, gives the kind of its variation, and uses no option the
//! command line refuses, except the options a variation asks for outside
//! M2 (since the M2 join, WP-053, the three gates of a dynamic output,
//! `-dynamic-linker`, `-pie` and `-Bdynamic`, are supported rows).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use mortar_cli::{DynamicLinker, OutputKind, Parsed, Use, parse};
use mortar_diag::Style;

/// The census: `(id, arguments)`, in file order.
fn lines() -> Vec<(String, Vec<OsString>)> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/census-m2.tsv");
    let text = std::fs::read_to_string(&path).expect("tests/data/census-m2.tsv");
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| {
            let mut fields = l.split('\t');
            let id = fields.next().unwrap_or_default().to_owned();
            (id, fields.map(OsString::from).collect())
        })
        .collect()
}

/// The variation of a line's id (`gcc_aarch64_c.no-pie` is `no-pie`).
fn variation(id: &str) -> &str {
    id.rsplit_once('.').map_or("", |(_, v)| v)
}

/// The uses as written: `spelling` or `spelling=value`.
fn written(uses: &[&Use]) -> Vec<String> {
    uses.iter()
        .map(|u| match &u.value {
            Some(v) => format!("{}={}", u.spelling, v.to_string_lossy()),
            None => u.spelling.clone(),
        })
        .collect()
}

/// What a line's variation asks for that M2 refuses by name
/// (`spec/dyn.md` D-20): the hash styles other than `gnu`. Every other
/// line refuses nothing (clang's `AddressSanitizer` runtime under
/// `--whole-archive` was refused until WP-152).
fn refused_outside_m2(id: &str) -> &'static [&'static str] {
    match variation(id) {
        "hash-sysv" => &["--hash-style=sysv"],
        "hash-both" => &["--hash-style=both"],
        _ => &[],
    }
}

/// The kind each variation's line gives (`spec/dyn.md` D-1): `-no-pie`
/// drops the driver's `-pie`; every other line is a PIE.
fn kind(variation: &str) -> OutputKind {
    if variation == "no-pie" { OutputKind::Executable } else { OutputKind::PieExecutable }
}

/// The gates of a dynamic output (`spec/dyn.md` §2.7 items 1, 3, 5),
/// `supported` rows since WP-053.
const GATES: [&str; 3] = ["-dynamic-linker", "-pie", "-Bdynamic"];

fn check(id: &str, line: &[OsString]) -> (Parsed, Vec<String>) {
    let parsed = parse(line);
    let errors: Vec<String> =
        parsed.diagnostics.iter().map(|(_, d)| d.render(&Style::plain("ld"))).collect();
    assert!(errors.is_empty(), "{id}: {errors:?}");
    let v = variation(id);
    assert_eq!(parsed.plan.kind, kind(v), "{id}");
    assert!(
        matches!(parsed.plan.dynamic.dynamic_linker, Some(DynamicLinker::Path(_))),
        "{id}: no interpreter (D-31)"
    );
    let refused = parsed.refused_uses();
    assert_eq!(written(&refused), refused_outside_m2(id), "{id}: refused by the command line");
    // Each gate once per line, at its first use.
    let mut gates: Vec<String> = Vec::new();
    for u in &parsed.uses {
        if GATES.contains(&u.spelling.as_str()) && !gates.contains(&u.spelling) {
            gates.push(u.spelling.clone());
        }
    }
    (parsed, gates)
}

// spec: dyn.md §2 (426 lines), §2.7, R-C1 (no option of a default line is
// refused by the command line, the gates included)
#[test]
fn every_census_line_parses_and_none_is_refused() {
    let lines = lines();
    assert_eq!(lines.len(), 426, "the census has 426 lines");
    let mut ids = std::collections::BTreeSet::new();
    let mut default_lines = 0;
    let mut gates: BTreeMap<String, usize> = BTreeMap::new();
    for (id, line) in &lines {
        assert!(ids.insert(id.clone()), "{id} twice");
        let (_, g) = check(id, line);
        if refused_outside_m2(id).is_empty() {
            default_lines += 1;
        }
        for gate in g {
            *gates.entry(gate).or_default() += 1;
        }
    }
    // 426 - 22 sysv - 22 both (the 12 clang asan lines since WP-152).
    assert_eq!(default_lines, 382);
    // The ranks of dyn.md §2.7: every line names an interpreter, 404 are
    // PIEs (all but the 22 -no-pie lines), 9 use -Bdynamic
    // (-static-libstdc++ on the gcc and clang C++ drivers).
    let want: BTreeMap<String, usize> = [("-Bdynamic", 9), ("-dynamic-linker", 426), ("-pie", 404)]
        .into_iter()
        .map(|(g, n)| (g.to_owned(), n))
        .collect();
    assert_eq!(gates, want);
}

// spec: dyn.md §5 D-11, D-19a (the as-needed state of each input on the
// drivers' default lines, as GNU ld reads it: gcc's global --as-needed,
// clang's pairs, gcc's --push-state pairs around libgcc_s)
#[test]
fn the_default_lines_as_needed_state() {
    let lines = lines();
    let find = |id: &str| -> Parsed {
        let (_, line) = lines.iter().find(|(i, _)| i == id).unwrap_or_else(|| panic!("{id}"));
        parse(line)
    };
    let libs = |parsed: &Parsed| -> Vec<(String, bool, bool)> {
        parsed
            .plan
            .inputs
            .iter()
            .filter_map(|i| match &i.spec {
                mortar_cli::InputSpec::Library(l) => {
                    Some((l.to_string_lossy().into_owned(), i.state.as_needed, i.state.link_static))
                }
                _ => None,
            })
            .collect()
    };
    let l = |n: &str, a: bool| (n.to_owned(), a, false);
    // gcc, C: `--as-needed` at the front of the line covers everything.
    assert_eq!(
        libs(&find("gcc_aarch64_c.default")),
        [l("gcc", true), l("gcc_s", true), l("c", true), l("gcc", true), l("gcc_s", true)]
    );
    // clang 22, C: as-needed only around libgcc_s.
    assert_eq!(
        libs(&find("clang22_aarch64_c.default")),
        [l("gcc", false), l("gcc_s", true), l("c", false), l("gcc", false), l("gcc_s", true)]
    );
    // g++ -static-libstdc++: `-Bstatic -lstdc++ -Bdynamic` under gcc's
    // global --as-needed.
    let got = libs(&find("gcc_aarch64_c++.static-libstdc++"));
    assert_eq!(got[0], ("stdc++".to_owned(), true, true), "{got:?}");
    assert!(got[1..].iter().all(|(_, _, s)| !s), "{got:?}");
    // gfortran on x86-64: --as-needed -lquadmath --no-as-needed in a line
    // that has the global --as-needed before it.
    let got = libs(&find("gcc_x86_64_fortran.default"));
    let quad = got.iter().position(|(n, ..)| n == "quadmath").expect("-lquadmath");
    assert!(got[quad].1, "{got:?}");
    assert!(got[quad + 1..].iter().all(|(_, a, _)| !a), "{got:?}");
}
