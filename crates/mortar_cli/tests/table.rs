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
            // The front of the pipeline honors these (WP-027a).
            "-m",
            "-e",
            "-s",
            "-S",
            "-x",
            "-X",
            "--discard-none",
            "--build-id",
            "--eh-frame-hdr",
            "--no-eh-frame-hdr",
            "--gc-sections",
            "--no-gc-sections",
            "-l",
            "-(",
            "-)",
            "--as-needed",
            "--no-as-needed",
            "-Bstatic",
            "-L",
            "--hash-style",
            "--error-limit",
            "--fatal-warnings",
            "--no-fatal-warnings",
            "-w",
            "--color-diagnostics",
            "--no-color-diagnostics",
            "--version",
            "-v",
            "-V",
            "--help",
            "-Map",
            "-M",
            "--threads",
            "--time-phases",
            "--fix-cortex-a53-843419",
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
        // is mortar's own and implemented (image.md §11).
        if !row.linkers.gnu()
            && !row.linkers.lld()
            && !row.linkers.mold()
            && row.name() != "--time-phases"
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
