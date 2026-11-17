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

/// The supported rows, in table order (cli.md §4.2).
const SUPPORTED: &[&str] = &[
    // The whole pipeline writes the output (WP-027c).
    "-o",
    // The dynamic executable (WP-053).
    "-pie",
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
    // WP-097 (synth.md S-G1).
    "--gdb-index",
    "-l",
    "-(",
    "-)",
    "--as-needed",
    "--no-as-needed",
    "-Bstatic",
    "-Bdynamic",
    // WP-027e.
    "--push-state",
    "--pop-state",
    "--warn-backrefs",
    "--no-warn-backrefs",
    "-L",
    "--sysroot",
    // The dynamic executable (WP-053).
    "-rpath-link",
    // Version scripts in a shared object (WP-125, dyn.md D-114).
    "--version-script",
    "--sort-common",
    "--sort-section",
    "--export-dynamic-symbol",
    "-init",
    "-fini",
    "--undefined-version",
    "--no-undefined-version",
    "-E",
    "--no-export-dynamic",
    "-Bsymbolic-functions",
    "-Bno-symbolic",
    "-h",
    "-rpath",
    "-I",
    "--no-dynamic-linker",
    "--enable-new-dtags",
    "--disable-new-dtags",
    "--error-limit",
    "--fatal-warnings",
    "--no-fatal-warnings",
    "-w",
    "--color-diagnostics",
    "--no-color-diagnostics",
    "--no-undefined",
    "--warn-common",
    "--no-warn-common",
    // The one-definition-rule switches (WP-072, odr.md O-41).
    "--odr",
    "--no-odr-check",
    "--odr-stats",
    "--odr-check-archives",
    "--odr-table",
    "--keep-mason-meta",
    "--version",
    "-v",
    "-V",
    "--help",
    "-Map",
    "-M",
    "--verbose",
    "--dependency-file",
    "--threads",
    "--mortar-map",
    "--time-phases",
    "--incremental",
    "--no-incremental",
    "--incremental-state",
    "--incremental-report",
    "-z now",
    "-z lazy",
    "-z relro",
    "-z norelro",
    "-z execstack",
    "-z noexecstack",
    "-z text",
    "-z separate-code",
    "-z noseparate-code",
    "-z defs",
    "-z max-page-size",
    "-z memory-seal",
    "-z nomemory-seal",
];

// spec: cli.md §4.2 (status: supported today, noop with a reason, refused
// otherwise)
#[test]
fn statuses() {
    let supported: Vec<String> = rows()
        .iter()
        .filter(|r| r.status == Status::Supported)
        .map(mortar_cli::table::Row::name)
        .collect();
    assert_eq!(supported, SUPPORTED);
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
        // switches of WP-072 and WP-076 (odr.md O-41) and the incremental
        // options of WP-061 (incr.md IN-10) are mortar's own and implemented.
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
                    | "--odr-check-archives"
                    | "--odr-table"
                    | "--keep-mason-meta"
                    | "--detect-odr-violations"
                    | "--incremental"
                    | "--no-incremental"
                    | "--incremental-state"
                    | "--incremental-report"
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

// spec: dyn.md §11.2 R-C1 (the M2 statuses of the dynamic rows;
// `spec/cli.md` §4.2's M2 list)
#[test]
fn r_c1_statuses() {
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
        assert_eq!(row(name).status, Status::Supported, "{name}");
    }
    // `gnu` only (D-20).
    match row("--hash-style").status {
        Status::Partial { refused, .. } => assert_eq!(refused, ["sysv", "both", "none"]),
        other => panic!("--hash-style: {other:?}"),
    }
    // Refused by name at M2.
    for name in [
        "-z notext",
        "-z nocopyreloc",
        "--spare-dynamic-tags",
        "--copy-dt-needed-entries",
        "--dynamic-list",
        "-z nodynamic-undefined-weak",
        "-z dynamic-undefined-weak",
        "-z force-bti",
        "-z pac-plt",
        "-z ibtplt",
        "-z ibt",
        "-z shstk",
        "-z undefs",
    ] {
        assert_eq!(row(name).status, Status::Refused, "{name}");
    }
    // Since WP-123: `-shared` is partial (its refusals are by name, in the
    // link) and `-h` is supported in a shared object (refused elsewhere,
    // `rows::SHARED_ONLY`).
    assert!(matches!(row("-shared").status, Status::Partial { refused: [], .. }));
    assert_eq!(row("-h").status, Status::Supported);
    // The default (D-15).
    assert!(matches!(row("--no-copy-dt-needed-entries").status, Status::Noop(_)));
}

// spec: dyn.md R-C1 (no status rests on the dynamic refusal once the M2
// join lifted it, WP-053: every gate is supported, and no noop row's reason
// is the refusal)
#[test]
fn no_status_rests_on_the_dynamic_refusal() {
    use mortar_cli::table::{KindSet, Lands, StateOp};
    for r in rows() {
        let gate = matches!(
            r.lands,
            Lands::Kind(KindSet::Pie) | Lands::DynamicLinker | Lands::State(StateOp::Static(false))
        );
        if gate {
            assert_eq!(r.status, Status::Supported, "{}: a gate", r.name());
        }
        if let Status::Noop(reason) = r.status {
            assert!(!reason.contains("(M2)"), "{}: its reason is the refusal", r.name());
        }
    }
}

// spec: cli.md §3.14 (one registry of written paths: every row whose value
// is a path is classified as written or read, so a new one cannot be
// added without a decision)
#[test]
fn every_path_valued_row_is_classified() {
    use mortar_cli::table::{READS, WRITES};
    let names: HashSet<String> = rows().iter().map(mortar_cli::table::Row::name).collect();
    for (name, role) in WRITES {
        assert!(names.contains(*name), "WRITES names {name}, which is no row");
        assert!(role.starts_with("the "), "{name}: {role}");
    }
    for name in READS {
        assert!(names.contains(*name), "READS names {name}, which is no row");
        assert!(WRITES.iter().all(|(w, _)| w != name), "{name} is in both lists");
    }
    for row in rows() {
        let pathy = ["FILE", "PATH", "DIR"].iter().any(|m| row.meta.contains(m));
        if !pathy {
            continue;
        }
        let name = row.name();
        assert!(
            row.writes().is_some() || READS.contains(&name.as_str()),
            "{name} takes a path ({}): add it to WRITES or READS (cli.md §3.14)",
            row.meta
        );
    }
}
