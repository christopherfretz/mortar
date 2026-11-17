//! The command line at `-r` (`spec/cli.md` §3.16, C-R1..C-R6;
//! `spec/diag.md` §4.4; WP-182, RL-2): the kind, the conflicts, the
//! refusals `<option> with -r`, the no-ops, and the census's 399 driver
//! lines replayed.
//!
//! `-r` itself stays refused (`unsupported option: -r`) until the join
//! (RL-7). These tests see the line as it will be after the join through
//! the TEST-ONLY seam `Parsed::refusals_landed(&["RL-7"])`.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fmt::Write as _;

use mortar_cli::table::{AtRClass, Lands, rows};
use mortar_cli::{OutputKind, Parsed, parse};
use mortar_diag::Style;
use mortar_diag::relocatable::{
    CONFLICTS_WITH_R, GC_SECTIONS_NOTE, OPTIONS_REFUSED_UNTIL, OPTIONS_REFUSED_WITH_R, SCRIPT_NOTE,
    WRAP_NOTE, Z_KEYWORDS_REFUSED_WITH_R,
};

fn p(list: &[&str]) -> Parsed {
    let args: Vec<OsString> = list.iter().map(OsString::from).collect();
    parse(&args)
}

fn render(list: &[(mortar_diag::Key, mortar_diag::Diagnostic)]) -> Vec<String> {
    list.iter().map(|(_, d)| d.render(&Style::plain("ld")).trim_end().to_owned()).collect()
}

/// The parse's errors, rendered.
fn errors(parsed: &Parsed) -> Vec<String> {
    render(&parsed.diagnostics)
}

/// The refusals once the join (RL-7) has landed.
fn after_join(parsed: &Parsed) -> Vec<String> {
    render(&parsed.refusals_landed(&["RL-7"]))
}

/// `-r LINE a.o`, its refusals after the join; the parse must be clean.
fn refused_at_r(extra: &[&str]) -> Vec<String> {
    let mut line = vec!["-r"];
    line.extend_from_slice(extra);
    line.push("a.o");
    let parsed = p(&line);
    assert!(errors(&parsed).is_empty(), "{line:?}: {:?}", errors(&parsed));
    assert_eq!(parsed.plan.kind, OutputKind::Relocatable, "{line:?}");
    after_join(&parsed)
}

// spec: cli.md §3.16 C-R1
#[test]
fn r_kind_spellings() {
    for spelling in ["-r", "-i", "--relocatable", "-Ur"] {
        let parsed = p(&[spelling, "a.o"]);
        assert!(errors(&parsed).is_empty(), "{spelling}: {:?}", errors(&parsed));
        assert_eq!(parsed.plan.kind, OutputKind::Relocatable, "{spelling}");
        assert_eq!(parsed.plan.ur, spelling == "-Ur", "{spelling}");
        // Until the join: the final-link refusal of the option itself.
        assert_eq!(
            render(&parsed.refusals()),
            [format!("ld: error: unsupported option: {spelling}")],
            "{spelling}"
        );
        assert!(after_join(&parsed).is_empty(), "{spelling}: {:?}", after_join(&parsed));
    }
    // `-no-pie` with `-r` is `-r` (C-R2), in either order.
    for line in [["-r", "-no-pie", "a.o"], ["-no-pie", "-r", "a.o"]] {
        let parsed = p(&line);
        assert!(errors(&parsed).is_empty(), "{line:?}");
        assert_eq!(parsed.plan.kind, OutputKind::Relocatable);
        assert!(after_join(&parsed).is_empty(), "{line:?}");
    }
}

// spec: cli.md §3.16 C-R1; diag.md §4.4 (`conflicting-options`)
#[test]
fn r_conflicting_pie_shared() {
    for (line, second) in [
        (&["-r", "-shared", "a.o"][..], "-shared"),
        (&["-shared", "-r", "a.o"], "-shared"),
        (&["-r", "-pie", "a.o"], "-pie"),
        (&["-pie", "-r", "a.o"], "-pie"),
        (&["-r", "--pic-executable", "a.o"], "--pic-executable"),
        (&["-Ur", "-pie", "a.o"], "-pie"),
        // GCC's `-static-pie` line: `-static -pie --no-dynamic-linker`.
        (&["-static", "-pie", "--no-dynamic-linker", "-r", "a.o"], "-pie"),
    ] {
        let parsed = p(line);
        let first = if line.contains(&"-Ur") { "-Ur" } else { "-r" };
        assert_eq!(
            errors(&parsed),
            [format!("ld: error: {first} and {second} cannot be used together")],
            "{line:?}"
        );
        assert_eq!(parsed.plan.kind, OutputKind::Relocatable);
    }
}

/// One line per row whose class at `-r` is `Conflicting`, with its value
/// when it takes one, and the `{second}` reported.
const CONFLICT_LINES: &[(&[&str], &str)] = &[
    (&["-shared"], "-shared"),
    (&["-pie"], "-pie"),
    (&["--pic-executable"], "--pic-executable"),
    (&["-Bshareable"], "-Bshareable"),
    (&["-z", "nosectionheader"], "-z nosectionheader"),
    (&["-Bsymbolic"], "-Bsymbolic"),
    (&["-Bsymbolic-functions"], "-Bsymbolic-functions"),
    (&["--Bsymbolic-non-weak"], "--Bsymbolic-non-weak"),
    (&["--Bsymbolic-non-weak-functions"], "--Bsymbolic-non-weak-functions"),
    (&["--dynamic-list=l"], "--dynamic-list"),
    (&["--dynamic-list-data"], "--dynamic-list-data"),
    (&["--dynamic-list-cpp-new"], "--dynamic-list-cpp-new"),
    (&["--dynamic-list-cpp-typeinfo"], "--dynamic-list-cpp-typeinfo"),
    (&["--export-dynamic"], "--export-dynamic"),
    (&["-E"], "-E"),
    (&["--export-dynamic-symbol=s"], "--export-dynamic-symbol"),
    (&["--export-dynamic-symbol-list=l"], "--export-dynamic-symbol-list"),
    (&["-soname", "x.so"], "-soname"),
    (&["-h", "x.so"], "-h"),
    (&["-rpath", "/x"], "-rpath"),
    (&["--rpath=/x"], "--rpath"),
    (&["--enable-new-dtags"], "--enable-new-dtags"),
    (&["-init", "f"], "-init"),
    (&["-fini", "f"], "-fini"),
    (&["--pack-dyn-relocs=relr"], "--pack-dyn-relocs"),
    (&["--android-memtag-mode=sync"], "--android-memtag-mode"),
];

// spec: cli.md §3.16 C-R2a (last row), C-R4, C-R2b; diag.md §4.4 (the
// `{second}` list is exactly the rows whose class is `Conflicting`)
#[test]
fn r_conflicting_rows() {
    let mut hit = BTreeSet::new();
    for (extra, second) in CONFLICT_LINES {
        let mut line = vec!["-r"];
        line.extend_from_slice(extra);
        line.push("a.o");
        let parsed = p(&line);
        assert_eq!(
            errors(&parsed),
            [format!("ld: error: -r and {second} cannot be used together")],
            "{line:?}"
        );
        assert!(after_join(&parsed).is_empty(), "{line:?}: {:?}", after_join(&parsed));
        for u in &parsed.uses {
            if u.row().at_r.map(|a| a.class) == Some(AtRClass::Conflicting) {
                hit.insert(u.row);
            }
        }
        // Without `-r` none of them conflicts.
        let mut alone: Vec<&str> = extra.to_vec();
        alone.push("a.o");
        assert!(
            !errors(&p(&alone)).iter().any(|e| e.contains("cannot be used together")),
            "{alone:?}"
        );
    }
    let conflicting: BTreeSet<usize> = rows()
        .iter()
        .enumerate()
        .filter(|(_, r)| r.at_r.map(|a| a.class) == Some(AtRClass::Conflicting))
        .map(|(i, _)| i)
        .collect();
    let missed: Vec<String> = conflicting.difference(&hit).map(|&i| rows()[i].name()).collect();
    assert!(missed.is_empty(), "rows conflicting at -r without a line here: {missed:?}");
    // Every `{second}` of diag.md §4.4's constant appears as a line's.
    for second in CONFLICTS_WITH_R {
        if second == "-static-pie" {
            continue; // the driver passes `-pie` (r_conflicting_pie_shared)
        }
        assert!(CONFLICT_LINES.iter().any(|(_, s)| *s == second), "{second}");
    }
    // A conflict is reported once per row; a later last-wins switch that
    // undoes the option is not one.
    let parsed = p(&["-r", "-E", "-E", "a.o"]);
    assert_eq!(errors(&parsed).len(), 1);
}

// spec: cli.md §3.16 C-R3 (until), C-R4, C-R2a; diag.md §4.4 (the names)
#[test]
fn r_refused_options() {
    let cases: &[(&[&str], &str, Option<&str>)] = &[
        (&["-T", "x.ld"], "-T", Some(SCRIPT_NOTE)),
        (&["--script=x.ld"], "--script", Some(SCRIPT_NOTE)),
        (&["--defsym", "a=1"], "--defsym", None),
        (&["--gc-sections"], "--gc-sections", Some(GC_SECTIONS_NOTE)),
        (&["--icf=all"], "--icf", None),
        (&["--unique"], "--unique", None),
        (&["--unique=.text*"], "--unique", None),
        (&["--relax"], "--relax", None),
        (&["--wrap=foo"], "--wrap", Some(WRAP_NOTE)),
        (&["--compress-debug-sections=zlib"], "--compress-debug-sections", None),
        (&["--compress-debug-sections=zstd"], "--compress-debug-sections", None),
        (&["--gdb-index"], "--gdb-index", None),
        (&["-d"], "-d", None),
        (&["-dc"], "-dc", None),
        (&["-dp"], "-dp", None),
        (&["-s"], "-s", None),
        (&["--strip-all"], "--strip-all", None),
        (&["-S"], "-S", None),
        (&["--strip-debug"], "--strip-debug", None),
        (&["-x"], "-x", None),
        (&["--discard-all"], "--discard-all", None),
        (&["-b", "binary"], "-b", None),
        (&["--format=binary"], "--format=binary", None),
        (&["--force-group-allocation"], "--force-group-allocation", None),
    ];
    let mut named = BTreeSet::new();
    for (extra, option, note) in cases {
        let got = refused_at_r(extra);
        let mut want = format!("ld: error: unsupported option: {option} with -r");
        if let Some(note) = note {
            let _ = write!(want, "\n>>> {note}");
        }
        assert_eq!(got, [want], "{extra:?}");
        named.insert(*option);
    }
    for option in OPTIONS_REFUSED_WITH_R.iter().chain(&OPTIONS_REFUSED_UNTIL) {
        assert!(named.contains(option), "no case for {option}");
    }
    // A refused row is reported once, at its first use.
    assert_eq!(refused_at_r(&["-s", "-s"]).len(), 1);
    // `--gc-sections --no-gc-sections` is the plan without it (§4.4).
    assert!(refused_at_r(&["--gc-sections", "--no-gc-sections"]).is_empty());
}

// spec: cli.md §3.16 C-R2a (the property and `unique-symbol` rows)
#[test]
fn r_z_refused() {
    let mut named = BTreeSet::new();
    for keyword in [
        "force-bti",
        "gcs=always",
        "stack-size=4096",
        "memory-seal",
        "ibt",
        "shstk",
        "indirect-extern-access",
        "x86-64-v2",
        "x86-64-v3",
        "x86-64-v4",
        "lam-u48",
        "lam-u57",
        "unique-symbol",
    ] {
        let got = refused_at_r(&["-z", keyword]);
        assert_eq!(got, [format!("ld: error: unsupported option: -z {keyword} with -r")]);
        named.insert(keyword.split_once('=').map_or(keyword.to_owned(), |(k, v)| {
            if k == "gcs" { format!("{k}={v}") } else { format!("{k}=") }
        }));
    }
    for keyword in Z_KEYWORDS_REFUSED_WITH_R {
        assert!(named.contains(keyword), "no case for -z {keyword}");
    }
}

// spec: cli.md §3.16 C-R2, C-R2a (first row, the report keywords: ruling
// (a)), C-R2b (no-ops), C-R4 (`=none`; `-rpath-link`: ruling (b))
#[test]
fn r_noop_options() {
    for extra in [
        &["-no-pie"][..],
        &["-static"],
        &["-Bstatic"],
        &["-Bdynamic"],
        &["-dynamic-linker", "/lib/ld.so"],
        &["--no-dynamic-linker"],
        &["--eh-frame-hdr"],
        &["--hash-style=gnu"],
        &["--as-needed"],
        &["--no-build-id"],
        &["--build-id=none"],
        &["-X"],
        &["-EL"],
        &["--fix-cortex-a53-843419"],
        &["--emit-relocs"],
        &["-q"],
        &["--no-undefined"],
        &["-z", "defs"],
        &["--no-relax"],
        &["-rpath-link", "/x"],
        &["-rpath-link=/x"],
        &["--compress-debug-sections=none"],
        &["-z", "gcs=never"],
        &["-z", "text"],
        &["-z", "notext"],
        &["-z", "nodelete"],
        &["-z", "origin"],
        &["-z", "combreloc"],
        &["-z", "bti-report=error"],
        &["-z", "gcs-report=warning"],
        &["-z", "gcs-report-dynamic=warning"],
        &["-z", "isa-level-report=all"],
        &["-z", "start-stop-visibility=hidden"],
        &["-z", "pac-plt"],
        &["--no-export-dynamic"],
        &["-Bno-symbolic"],
        &["--disable-new-dtags"],
        &["--detect-odr-violations"],
        &["--no-incremental"],
    ] {
        assert!(refused_at_r(extra).is_empty(), "{extra:?}: {:?}", refused_at_r(extra));
    }
}

// spec: cli.md §3.16 C-R2b (as in a final link; an `acts` row keeps its
// final-link refusals of values), C-R3
#[test]
fn r_as_final_and_acting_rows() {
    assert_eq!(
        refused_at_r(&["--build-id=md5"]).len(),
        1,
        "a refused value of an acting row stays refused"
    );
    assert!(refused_at_r(&["--build-id"]).is_empty());
    assert!(refused_at_r(&["-u", "s"]).is_empty());
    assert!(refused_at_r(&["-e", "s"]).is_empty());
    assert!(refused_at_r(&["--require-defined=s"]).is_empty());
    assert!(refused_at_r(&["-z", "noexecstack"]).is_empty());
    assert_eq!(
        refused_at_r(&["--undefined-glob=s*"]),
        ["ld: error: unsupported option: --undefined-glob"]
    );
    // `-z gcs=implicit` keeps its final-link status (C-R2a).
    let parsed = p(&["-r", "-z", "gcs=implicit", "a.o"]);
    let final_link = p(&["-z", "gcs=implicit", "a.o"]);
    assert_eq!(render(&parsed.refusals_landed(&["RL-7"])), render(&final_link.refusals()));
}

// spec: cli.md §3.16 C-R2b (last bullet: an `acts` row is refused until
// its package lands), the seam
#[test]
fn r_until_lifted_by_the_package() {
    for (extra, package) in
        [(&["-s"][..], "RL-11"), (&["-x"], "RL-11"), (&["--force-group-allocation"], "RL-8")]
    {
        let mut line = vec!["-r"];
        line.extend_from_slice(extra);
        line.push("a.o");
        let parsed = p(&line);
        assert_eq!(render(&parsed.refusals_landed(&["RL-7"])).len(), 1, "{extra:?}");
        assert!(parsed.refusals_landed(&["RL-7", package]).is_empty(), "{extra:?}");
    }
    // The other kinds are not touched by the classes at `-r`.
    assert_eq!(render(&p(&["-s", "a.o"]).refusals()), Vec::<String>::new());
    assert_eq!(render(&p(&["--gc-sections", "a.o"]).refusals()), Vec::<String>::new());
    assert!(
        render(&p(&["-z", "nodelete", "a.o"]).refusals()).iter().any(|r| r.contains("-z nodelete")),
        "SHARED_ONLY outside a Shared or -r link"
    );
    let _ = Lands::Output;
}

/// What a census variant gives at the plan (C-R6): the conflicts and the
/// refusals after the join.
fn census_expectation(variant: &str) -> (Vec<String>, Vec<String>) {
    let conflict =
        |second: &str| vec![format!("ld: error: -r and {second} cannot be used together")];
    match variant {
        "-r -shared" => (conflict("-shared"), vec![]),
        "-r -static-pie" | "-Wl,-r" | "-nostdlib -Wl,-r" => (conflict("-pie"), vec![]),
        "-r -s" => (vec![], vec!["ld: error: unsupported option: -s with -r".to_owned()]),
        "-r -Wl,--gc-sections" | "-r -Wl,--gc-sections,-e,a" => (
            vec![],
            vec![format!(
                "ld: error: unsupported option: --gc-sections with -r\n>>> {GC_SECTIONS_NOTE}"
            )],
        ),
        _ => (vec![], vec![]),
    }
}

// spec: cli.md §3.16 C-R6 (the drivers' lines, WP-154 §A.2)
#[test]
fn r_driver_lines() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/relocatable-design/work/census-lines.txt"
    );
    let text = std::fs::read_to_string(path).expect("census lines");
    let mut lines = 0;
    let mut not_run = 0;
    let mut variants = BTreeSet::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let fields: Vec<&str> = line.split('|').collect();
        assert!(fields.len() >= 5, "{line}");
        let (driver, variant, args) = (fields[0], fields[2], fields[4]);
        lines += 1;
        variants.insert(variant.to_owned());
        // `-fuse-ld=lld`/`mold`: the driver runs that linker, so the
        // recording `ld` saw no line (42 lines, WP-154 §A.2).
        if args.trim() == "(ld not run)" {
            assert!(variant.starts_with("-r -fuse-ld="), "{driver} {variant}");
            not_run += 1;
            continue;
        }
        let args: Vec<&str> = args.split_whitespace().collect();
        let parsed = p(&args);
        let (want_errors, want_refusals) = census_expectation(variant);
        assert_eq!(errors(&parsed), want_errors, "{driver} {variant}: {args:?}");
        assert_eq!(parsed.plan.kind, OutputKind::Relocatable, "{driver} {variant}");
        assert_eq!(after_join(&parsed), want_refusals, "{driver} {variant}: {args:?}");
        // Until the join every one of them is refused by `-r` itself.
        assert!(
            render(&parsed.refusals()).contains(&"ld: error: unsupported option: -r".to_owned()),
            "{driver} {variant}"
        );
    }
    assert_eq!(lines, 399);
    assert_eq!(not_run, 42);
    assert_eq!(variants.len(), 19);
}
