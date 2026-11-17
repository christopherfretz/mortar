//! The command line of a shared-object link (`spec/cli.md` §4.2's M4
//! block, WP-117): the refusals by name of `spec/dyn.md` D-112, the
//! options a later package implements in a `Shared` link, and the
//! `-shared` row's text (`spec/landed/dyn/WP-110.md` §G.4).

use std::ffi::OsString;

use mortar_cli::table::{Status, rows};
use mortar_cli::{OutputKind, Parsed, parse};
use mortar_core::plan::Symbolic;
use mortar_diag::Style;

fn p(list: &[&str]) -> Parsed {
    let args: Vec<OsString> = list.iter().map(OsString::from).collect();
    parse(&args)
}

/// The refusals of a line, rendered.
fn refusals(parsed: &Parsed) -> Vec<String> {
    parsed.refusals().iter().map(|(_, d)| d.render(&Style::plain("ld"))).collect()
}

fn refused(parsed: &Parsed) -> Vec<&str> {
    parsed.refused_uses().iter().map(|u| u.spelling.as_str()).collect()
}

// spec: cli.md §4.2 (M4 block: "Refused by name, class `option`, in every
// kind"), dyn.md D-112 (the last two rows of the table)
#[test]
fn d112_non_weak_and_dynamic_list_data_are_refused_in_every_kind() {
    for kind in [&[][..], &["-pie"][..], &["-shared"][..]] {
        for option in [
            "--Bsymbolic-non-weak",
            "--Bsymbolic-non-weak-functions",
            "--dynamic-list-data",
            "--dynamic-list-cpp-new",
            "--dynamic-list-cpp-typeinfo",
        ] {
            let mut line = kind.to_vec();
            line.extend([option, "a.o"]);
            let parsed = p(&line);
            assert!(refused(&parsed).contains(&option), "{line:?}");
            let text = refusals(&parsed).join("\n");
            assert!(text.contains(&format!("unsupported option: {option}")), "{text}");
            assert!(text.contains("dyn.md D-112"), "{option}: {text}");
        }
    }
    // `Plan::symbolic()` is `None` for the two `NonWeak` forms: the hook
    // of their refusal (WP-116).
    assert!(p(&["--Bsymbolic-non-weak", "a.o"]).plan.symbolic().is_none());
    assert!(p(&["--Bsymbolic-non-weak-functions", "a.o"]).plan.symbolic().is_none());
    // The `-Bsymbolic` field is not one of `Lands::last_wins`'s (cli.md
    // §4.4): a use is refused even when a later form overrides it.
    let parsed = p(&["--Bsymbolic-non-weak", "-Bsymbolic-functions", "a.o"]);
    assert_eq!(refused(&parsed), ["--Bsymbolic-non-weak"]);
    assert_eq!(parsed.plan.symbolic().map(|s| s.form), Some(Symbolic::Functions));
}

// spec: cli.md §4.2 (M4 block, WP-128's rows): accepted in a `Shared`
// link; outside it `-Bsymbolic` is a `noop`, `-init`/`-fini`,
// `--dynamic-list` and `--exclude-libs` keep their rows, and D-103's five
// `-z` flags are refused by name (`rows::SHARED_ONLY`: an executable keeps
// its M2 status, dyn.md D-103)
#[test]
fn s4_2_options_wp128_implements_are_accepted_in_shared() {
    let rows128 = [
        &["-Bsymbolic"][..],
        &["--dynamic-list", "l.list"],
        &["--exclude-libs", "ALL"],
        &["-init", "f"],
        &["-fini", "f"],
        &["-z", "nodelete"],
        &["-z", "nodlopen"],
        &["-z", "initfirst"],
        &["-z", "interpose"],
        &["-z", "origin"],
    ];
    for words in rows128 {
        let mut line = vec!["-shared"];
        line.extend_from_slice(words);
        line.push("a.o");
        let parsed = p(&line);
        assert_eq!(parsed.plan.kind, OutputKind::Shared);
        assert!(refused(&parsed).is_empty(), "{line:?}: {:?}", refusals(&parsed));
        let exec: Vec<&str> = words.iter().copied().chain(["a.o"]).collect();
        let parsed = p(&exec);
        if words[0] == "-z" {
            let spelling = format!("-z {}", words[1]);
            assert_eq!(refused(&parsed), [spelling.as_str()], "{exec:?}");
            let text = refusals(&parsed).join("\n");
            assert!(text.contains("dyn.md D-103"), "{text}");
            assert!(text.contains("(-shared) only"), "{text}");
        } else {
            assert!(refused(&parsed).is_empty(), "{exec:?}: {:?}", refusals(&parsed));
        }
    }
    // The last `-Bsymbolic` form decides the line (D-112).
    let form = |words: &[&str]| {
        let mut line = vec!["-shared"];
        line.extend_from_slice(words);
        line.push("a.o");
        p(&line).plan.symbolic().map(|s| (s.form, s.writes_symbolic_tags()))
    };
    assert_eq!(form(&["-Bsymbolic"]), Some((Symbolic::All, true)));
    assert_eq!(form(&["-Bsymbolic", "-Bsymbolic-functions"]), Some((Symbolic::Functions, false)));
    assert_eq!(form(&["-Bsymbolic", "-Bno-symbolic"]), Some((Symbolic::None, false)));
    assert_eq!(form(&["-Bno-symbolic", "-Bsymbolic"]), Some((Symbolic::All, true)));
    // With `--dynamic-list` the list decides: no DT_SYMBOLIC.
    assert_eq!(
        form(&["-Bsymbolic", "--dynamic-list", "l.list"]).map(|f| f.1),
        Some(false)
    );
}

// spec: cli.md §4.2 (M4 block, WP-117's rows): accepted in a `Shared`
// link (their effect is resolution's: dyn.md D-112, D-118)
#[test]
fn s4_2_options_wp117_implements_are_not_refused_in_shared() {
    for words in [
        &["-Bsymbolic-functions"][..],
        &["-Bno-symbolic"][..],
        &["-z", "defs"][..],
        &["--no-undefined"][..],
    ] {
        let mut line = vec!["-shared"];
        line.extend_from_slice(words);
        line.push("a.o");
        assert!(refused(&p(&line)).is_empty(), "{line:?}");
    }
    let parsed = p(&["-shared", "-z", "defs", "-Bsymbolic-functions", "a.o"]);
    assert_eq!(parsed.plan.z.defs, Some(true));
    assert_eq!(parsed.plan.symbolic().map(|s| s.form), Some(Symbolic::Functions));
}

// spec: landed/dyn/WP-110.md §G.4 (the `-shared` row's text), cli.md §4.2
// (`-shared` reads `partial` once WP-123 joins the first target; x86-64
// joins it with WP-127; WP-124 lifts TLS and IFUNC on AArch64 but
// local-exec TLS)
#[test]
fn g4_shared_row_text() {
    let row = rows().iter().find(|r| r.name() == "-shared").expect("the -shared row");
    let Status::Partial { refused: values, note } = row.status else {
        panic!("-shared is partial since WP-123: {:?}", row.status);
    };
    assert!(values.is_empty(), "no refused value: its refusals are by name, in the link");
    for part in [
        "AArch64 and x86-64 (i386 refused, `feature`, until WP-130; local-exec TLS in a \
         shared object refused, dyn.md D-119; `extern \"C++\"` version-script entries \
         refused, dyn.md D-114)",
        "Not judged on x86-64 (x64.md X-62)",
        "3 of 306 measured bound definitions",
        "Refused although correct: -fPIC code that forms an address before the start of a \
         static object placed after exported data",
        "remedy -fdata-sections",
    ] {
        assert!(note.contains(part), "{part}");
    }
    assert!(!note.contains("WP-123"), "{note}");
    assert!(refusals(&p(&["-shared", "a.o"])).is_empty());
    let md = mortar_cli::status_markdown();
    assert!(md.contains(&format!("partial: {note}")), "options.md shows the row's text");
    assert!(mortar_cli::help_text("ld").contains("Produce a shared object [partial]"));
}

// spec: dyn.md D-102 (WP-123): `-soname`/`-h` is supported in a shared
// object and refused by name elsewhere (mortar writes DT_SONAME only in a
// shared object)
#[test]
fn d102_soname_only_in_shared() {
    for line in
        [&["-shared", "-soname", "libx.so.1", "a.o"][..], &["-shared", "-h", "libx.so.1", "a.o"]]
    {
        let parsed = p(line);
        assert!(refused(&parsed).is_empty(), "{line:?}");
        assert_eq!(parsed.plan.dynamic.soname.as_deref(), Some(std::ffi::OsStr::new("libx.so.1")));
    }
    for kind in [&[][..], &["-pie"][..], &["-static"][..]] {
        let mut line = kind.to_vec();
        line.extend(["-soname", "libx.so.1", "a.o"]);
        let parsed = p(&line);
        assert_eq!(refused(&parsed), ["-soname"], "{line:?}");
        let text = refusals(&parsed).join("\n");
        assert!(text.contains("unsupported option: -soname"), "{text}");
        assert!(text.contains("dyn.md D-102"), "{text}");
    }
}

// spec: dyn.md D-114 (WP-125): `--version-script` is supported in a
// shared object and refused by name elsewhere (the references disagree on
// an executable); `--no-undefined-version` and `--undefined-version` land
// in `dynamic.undefined_version`, the last wins
#[test]
fn d114_version_script_only_in_shared() {
    let parsed = p(&["-shared", "--version-script", "v.map", "--version-script=w.map", "a.o"]);
    assert!(refused(&parsed).is_empty());
    let paths: Vec<_> = parsed.plan.version_scripts().map(|s| s.path.clone()).collect();
    assert_eq!(paths, ["v.map", "w.map"]);
    for kind in [&[][..], &["-pie"][..], &["-static"][..]] {
        let mut line = kind.to_vec();
        line.extend(["--version-script", "v.map", "a.o"]);
        let parsed = p(&line);
        assert_eq!(refused(&parsed), ["--version-script"], "{line:?}");
        let text = refusals(&parsed).join("\n");
        assert!(text.contains("unsupported option: --version-script"), "{text}");
        assert!(text.contains("dyn.md D-114"), "{text}");
    }
    let undef = |words: &[&str]| {
        let mut line = vec!["-shared"];
        line.extend_from_slice(words);
        line.push("a.o");
        let parsed = p(&line);
        assert!(refused(&parsed).is_empty(), "{line:?}");
        parsed.plan.dynamic.undefined_version
    };
    assert_eq!(undef(&[]), None);
    assert_eq!(undef(&["--no-undefined-version"]), Some(false));
    assert_eq!(undef(&["--no-undefined-version", "--undefined-version"]), Some(true));
    assert_eq!(undef(&["--undefined-version", "--no-undefined-version"]), Some(false));
}

// spec: cli.md §4.2 (the M4 block's list is the table's): every name of
// the `Shared` list is a row
#[test]
fn s4_2_shared_list_names_rows() {
    // Empty since WP-128.
    assert!(rows().iter().all(|r| r.shared_later().is_none()));
}
