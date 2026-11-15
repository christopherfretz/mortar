//! The option table and the texts made from it (`spec/cli.md` §4, §5).

use std::collections::HashSet;
use std::path::PathBuf;

use mortar_cli::table::{Arg, Form, Status};
use mortar_cli::{
    VersionStyle, help_text, print_version_text, rows, status_markdown, version_line, version_text,
};

// spec: cli.md §4.1 (every spelling names one row)
#[test]
fn spellings_are_unique() {
    let mut long = HashSet::new();
    let mut short = HashSet::new();
    let mut z = HashSet::new();
    for row in rows() {
        for sp in row.spellings {
            let fresh = match sp.form {
                Form::Short => {
                    assert_eq!(sp.text.len(), 1, "{}", sp.text);
                    short.insert(sp.text)
                }
                Form::Z => z.insert(sp.text),
                Form::Single | Form::Long | Form::LongTwo => long.insert(sp.text),
            };
            assert!(fresh, "{} is spelled twice", sp.display());
        }
    }
}

// spec: cli.md §2.2 (a name after one dash never starts with l, m or o)
#[test]
fn one_dash_names_do_not_start_with_l_m_o() {
    for row in rows() {
        for sp in row.spellings {
            if sp.takes_dashes(1) && sp.is_long() {
                assert!(!sp.text.starts_with(['l', 'm', 'o']), "{}", sp.display());
            }
        }
    }
}

// spec: cli.md §4.2 (status: supported today, noop with a reason, refused
// otherwise)
#[test]
fn statuses() {
    let supported: Vec<String> = rows()
        .iter()
        .filter(|r| r.status == Status::Supported)
        .map(mortar_cli::table::Row::name)
        .collect();
    assert_eq!(
        supported,
        [
            // The whole pipeline writes the output (WP-027c).
            "-o",
            // The default kind, position-dependent (WP-042).
            "-no-pie",
            // The front of the pipeline honors these (WP-027a).
            "-m",
            "-e",
            "-s",
            "-S",
            "-x",
            "-X",
            "--discard-none",
            "--no-build-id",
            "--eh-frame-hdr",
            "--no-eh-frame-hdr",
            // The mark and its report (WP-031).
            "--gc-sections",
            "--no-gc-sections",
            "--print-gc-sections",
            "--no-print-gc-sections",
            "--gc-keep-exported",
            "-l",
            "-(",
            "-)",
            "-Bstatic",
            // WP-027e.
            "--push-state",
            "--pop-state",
            "--warn-backrefs",
            "--no-warn-backrefs",
            "-L",
            "--sysroot",
            "--error-limit",
            "--fatal-warnings",
            "--no-fatal-warnings",
            "-w",
            "--color-diagnostics",
            "--no-color-diagnostics",
            // The one-definition-rule switches (WP-072, odr.md O-41).
            "--odr",
            "--no-odr-check",
            "--odr-stats",
            "--version",
            "-v",
            "-V",
            "--help",
            "-Map",
            "-M",
            "--dependency-file",
            "--threads",
            "--mortar-map",
            "--time-phases",
            "-z now",
            "-z lazy",
            "-z relro",
            "-z norelro",
            "-z execstack",
            "-z noexecstack",
            "-z separate-code",
            "-z noseparate-code",
            "-z max-page-size",
            "-z memory-seal",
            "-z nomemory-seal",
        ]
    );
    for row in rows() {
        if let Status::Noop(reason) = row.status {
            assert!(!reason.is_empty(), "{}", row.name());
        }
        assert!(!row.help.is_empty(), "{}", row.name());
        if !matches!(row.arg, Arg::None) {
            assert!(!row.meta.is_empty(), "{} names its value", row.name());
        }
        // A row of mortar's own is reserved, so refused; `--time-phases`
        // (image.md §11), `--mortar-map` (I-77) and the one-definition-rule
        // switches of WP-072 (odr.md O-41) are mortar's own and implemented.
        if !row.linkers.gnu()
            && !row.linkers.lld()
            && !row.linkers.mold()
            && !matches!(
                row.name().as_str(),
                "--time-phases"
                    | "--mortar-map"
                    | "--odr"
                    | "--no-odr-check"
                    | "--odr-stats"
                    | "--detect-odr-violations"
            )
        {
            assert_eq!(row.status, Status::Refused, "{}", row.name());
        }
        // An option of some targets only is GNU ld's.
        if !row.targets.is_empty() {
            assert!(row.linkers.gnu(), "{}", row.name());
        }
    }
}

// spec: cli.md §5.2 (--help is the table: every row, and what tools look
// for)
#[test]
fn help_lists_every_row() {
    let help = help_text("ld");
    for row in rows() {
        for sp in row.spellings {
            assert!(help.contains(&sp.display()), "--help is missing {}", sp.display());
        }
        assert!(help.contains(row.help), "--help is missing the help of {}", row.name());
    }
    assert!(help.starts_with("Usage: ld [options] file...\n"));
    // libtool: `$LD --help | grep no-whole-archive` and
    // `': supported targets:.* elf'`.
    assert!(help.contains("--no-whole-archive"));
    assert!(help.contains("ld: supported targets: elf"));
    assert!(
        help.contains("ld: supported emulations: aarch64linux aarch64elf elf_x86_64 elf_i386\n")
    );
}

// spec: cli.md §5.1 (the version line, and -V)
#[test]
fn version_texts() {
    let line = version_line();
    assert_eq!(line, format!("mortar {} (compatible with GNU ld)\n", env!("CARGO_PKG_VERSION")));
    assert_eq!(version_text(), line);
    assert_eq!(print_version_text(VersionStyle::Version), line);
    assert_eq!(
        print_version_text(VersionStyle::Emulations),
        format!(
            "{line}  Supported emulations:\n   aarch64linux\n   aarch64elf\n   elf_x86_64\n   elf_i386\n"
        )
    );
}

// spec: cli.md §4.5 (`docs/status/options.md` is generated from the
// table; `MORTAR_BLESS=1` rewrites it)
#[test]
fn status_file_is_current() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/status/options.md");
    let want = status_markdown();
    if std::env::var_os("MORTAR_BLESS").is_some_and(|v| v == "1") {
        std::fs::write(&path, &want).expect("write docs/status/options.md");
        return;
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        have == want,
        "docs/status/options.md is stale: run `MORTAR_BLESS=1 cargo test -p mortar_cli --test table`"
    );
}

/// The row named `name`.
fn row(name: &str) -> &'static mortar_cli::table::Row {
    rows().iter().find(|r| r.name() == name).unwrap_or_else(|| panic!("no row {name}"))
}

// spec: dyn.md §11.2 R-C1 (the M2 statuses of the dynamic rows, at the
// join; `spec/cli.md` §4.2's M2 list)
#[test]
fn r_c1_statuses_at_the_join() {
    for name in [
        "-pie",
        "-I",
        "-Bdynamic",
        "--as-needed",
        "--no-as-needed",
        "-E",
        "-rpath",
        "-init",
        "-fini",
        "-z text",
        "--export-dynamic-symbol",
        "--enable-new-dtags",
        "--disable-new-dtags",
        // Already supported, and still so.
        "-z norelro",
        "-z relro",
        "-z now",
        "-z lazy",
        "-Bstatic",
        "--push-state",
        "--pop-state",
        "-no-pie",
    ] {
        assert_eq!(row(name).status_at_join(), Status::Supported, "{name}");
    }
    // `gnu` only (D-20).
    match row("--hash-style").status_at_join() {
        Status::Partial { refused, .. } => assert_eq!(refused, ["sysv", "both", "none"]),
        other => panic!("--hash-style: {other:?}"),
    }
    // Refused by name at M2, today and at the join.
    for name in [
        "-z notext",
        "-z nocopyreloc",
        "--spare-dynamic-tags",
        "--copy-dt-needed-entries",
        "--dynamic-list",
        "-z nodynamic-undefined-weak",
        "-z dynamic-undefined-weak",
        "-shared",
        "-h",
        "-z force-bti",
        "-z pac-plt",
        "-z ibtplt",
        "-z ibt",
        "-z shstk",
        "-z undefs",
    ] {
        let r = row(name);
        assert_eq!(r.status, Status::Refused, "{name} today");
        assert_eq!(r.status_at_join(), Status::Refused, "{name} at the join");
    }
    // The default (D-15).
    assert!(matches!(row("--no-copy-dt-needed-entries").status_at_join(), Status::Noop(_)));
}

// spec: dyn.md R-C1 (no status that rests on the dynamic refusal outlives
// it: every gate, and every noop row whose reason is the refusal, changes
// at the join; and the join's list names rows that exist, once each)
#[test]
fn the_join_changes_every_status_that_rests_on_the_refusal() {
    use mortar_cli::table::{KindSet, Lands, StateOp, join_changes};
    let mut names = HashSet::new();
    for (name, status) in join_changes() {
        assert!(names.insert(*name), "{name} twice");
        let r = row(name);
        assert_ne!(r.status, *status, "{name}: the join changes nothing");
        if let Status::Noop(reason) = status {
            assert!(!reason.contains("(M2)"), "{name}: the join's reason is the refusal");
        }
    }
    for r in rows() {
        let gate = matches!(
            r.lands,
            Lands::Kind(KindSet::Pie) | Lands::DynamicLinker | Lands::State(StateOp::Static(false))
        );
        let rests = matches!(r.status, Status::Noop(reason) if reason.contains("(M2)"));
        if gate || rests {
            assert!(names.contains(r.name().as_str()), "{}: not in the join's list", r.name());
        }
    }
}
