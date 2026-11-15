//! The texts made from the table: `--version`, `-V`, `--help`, and
//! `docs/status/options.md` (`spec/cli.md` §5, §4.5).

use std::fmt::Write as _;

use crate::plan::{Emulation, Target, VersionStyle};
use crate::table::{Arg, Form, Group, Linkers, Row, Spelling, Status, Values, rows};

/// mortar's version (the workspace's).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The BFD target names `--help` lists, for the tools that look for ELF
/// there (libtool: `': supported targets:.* elf'`).
const BFD_TARGETS: &str = "elf64-littleaarch64 elf64-x86-64 elf32-i386";

/// The first line of `--version`, `-v` and `-V` (§5.1). "compatible with
/// GNU ld" is what libtool and meson look for to treat a linker as GNU
/// ld's dialect; mold prints the same words.
#[must_use]
pub fn version_line() -> String {
    format!("mortar {VERSION} (compatible with GNU ld)\n")
}

/// What `--version` prints (§5.1).
#[must_use]
pub fn version_text() -> String {
    version_line()
}

/// What `-v` (`Version`) or `-V` (`Emulations`) prints (§5.1). `-V`
/// lists the emulations as GNU ld does.
#[must_use]
pub fn print_version_text(style: VersionStyle) -> String {
    let mut out = version_line();
    if style == VersionStyle::Emulations {
        out.push_str("  Supported emulations:\n");
        for e in Emulation::ALL {
            let _ = writeln!(out, "   {}", e.name());
        }
    }
    out
}

/// A spelling with its value, as `--help` shows it: `-o FILE`,
/// `--build-id[=STYLE]`, `-z max-page-size=SIZE`.
fn with_meta(sp: &Spelling, row: &Row) -> String {
    let name = sp.display();
    let meta = row.meta;
    match (row.arg, sp.form) {
        (Arg::None, _) => name,
        (Arg::Required(_), Form::Z) => format!("{name}={meta}"),
        (Arg::Required(_), _) => format!("{name} {meta}"),
        (Arg::Optional(_), _) => format!("{name}[={meta}]"),
        (Arg::Glued(_), _) => format!("{name}{meta}"),
        (Arg::Rest, _) => format!("{name} {meta}..."),
    }
}

/// A row's spellings with their values, joined: `-o FILE, --output FILE`.
fn synopsis(row: &Row) -> String {
    row.spellings.iter().map(|sp| with_meta(sp, row)).collect::<Vec<_>>().join(", ")
}

fn targets(row: &Row) -> String {
    if row.targets.is_empty() {
        "all".to_owned()
    } else {
        row.targets.iter().map(|t| t.name()).collect::<Vec<_>>().join(", ")
    }
}

/// What `--help` prints (§5.2): every row by group, with its status.
#[must_use]
pub fn help_text(prog: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Usage: {prog} [options] file...");
    out.push_str(
        "mortar links ELF for AArch64, x86-64 and i386 Linux; its command line is GNU ld's.\n\
         An option marked [refused] is read but not carried out yet: using it fails the\n\
         link with \"unsupported option\". An option marked [noop] needs nothing done.\n\
         An option marked [partial] is carried out except for the values it names.\n",
    );
    for group in Group::ALL {
        let _ = writeln!(out, "\n{}:", group.title());
        for row in rows().iter().filter(|r| r.group == group) {
            let left = synopsis(row);
            let mut right = row.help.to_owned();
            if !row.targets.is_empty() {
                let _ = write!(right, " ({} only)", targets(row));
            }
            match row.status {
                Status::Supported => {}
                Status::Noop(_) => right.push_str(" [noop]"),
                Status::Partial { refused, .. } => {
                    let _ = write!(right, " [partial: refused ={}]", refused.join(", ="));
                }
                Status::Refused => right.push_str(" [refused]"),
            }
            if left.len() < 30 {
                let _ = writeln!(out, "  {left:<28}{right}");
            } else {
                let _ = writeln!(out, "  {left}\n  {:<28}{right}", "");
            }
        }
    }
    let _ = writeln!(out, "  {:<28}Read options from FILE", "@FILE");
    let _ = writeln!(out, "{prog}: supported targets: {BFD_TARGETS}");
    let names: Vec<&str> = Emulation::ALL.iter().map(|e| e.name()).collect();
    let _ = writeln!(out, "{prog}: supported emulations: {}", names.join(" "));
    out
}

fn value_column(row: &Row) -> String {
    let values = match row.arg {
        Arg::None => return String::new(),
        Arg::Rest => return format!("{} (the rest)", row.meta),
        Arg::Required(v) => v,
        Arg::Optional(v) => return format!("optional, `=` only: {}", describe(v, row.meta)),
        Arg::Glued(v) => return format!("glued: {}", describe(v, row.meta)),
    };
    describe(values, row.meta)
}

fn describe(values: Values, meta: &str) -> String {
    match values {
        Values::Any => meta.to_owned(),
        Values::Count => format!("{meta} (number)"),
        Values::BuildId => format!("{meta} (style or 0xHEX)"),
        Values::Choice(words) => words.join(" \\| "),
    }
}

fn count(f: impl Fn(&Row) -> bool) -> usize {
    rows().iter().filter(|r| f(r)).count()
}

/// `docs/status/options.md` (§4.5): the table, generated. A golden test
/// keeps the file equal to this.
#[must_use]
pub fn status_markdown() -> String {
    let mut out = String::new();
    out.push_str(
        "# Options\n\n\
         Generated from `crates/mortar_cli/src/rows.rs` by `mortar_cli::status_markdown`;\n\
         do not edit. `MORTAR_BLESS=1 cargo test -p mortar_cli --test table` rewrites it.\n\
         The rules are `spec/cli.md`.\n\n",
    );
    let supported = count(|r| r.status == Status::Supported);
    let noop = count(|r| matches!(r.status, Status::Noop(_)));
    let partial = count(|r| matches!(r.status, Status::Partial { .. }));
    let refused = count(|r| r.status == Status::Refused);
    let _ = writeln!(
        out,
        "{} rows: {supported} supported, {partial} partial, {noop} noop, {refused} refused.\n",
        rows().len()
    );
    let gnu = count(|r| r.linkers.gnu());
    let lld_only = count(|r| r.linkers == Linkers::L);
    let mold_only = count(|r| r.linkers == Linkers::M);
    let lld_mold = count(|r| r.linkers == Linkers::LM);
    let mortar = count(|r| r.linkers == Linkers::MORTAR);
    let _ = writeln!(
        out,
        "Origin: {gnu} GNU ld, {lld_only} lld only, {mold_only} mold only, {lld_mold} lld and mold \
         only, {mortar} mortar's own.\n"
    );
    for target in Target::ALL {
        let n = count(|r| r.targets.contains(&target));
        let _ = writeln!(out, "- {}: {n} rows of that target only", target.name());
    }
    out.push('\n');
    for group in Group::ALL {
        let _ = writeln!(out, "## {}\n", group.title());
        out.push_str("| Option | Value | Linkers | Targets | Status | Lands in | Help |\n");
        out.push_str("|---|---|---|---|---|---|---|\n");
        for row in rows().iter().filter(|r| r.group == group) {
            let spellings: Vec<String> =
                row.spellings.iter().map(|s| format!("`{}`", s.display())).collect();
            let status = status_cell(row.status);
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | `{}` | {} |",
                spellings.join(", "),
                value_column(row),
                row.linkers.names(),
                targets(row),
                status,
                row.lands.field(),
                row.help.replace('|', "\\|"),
            );
        }
        out.push('\n');
    }
    join_markdown(&mut out);
    out
}

/// The status column of a row's status.
fn status_cell(status: Status) -> String {
    match status {
        Status::Noop(reason) => format!("noop: {reason}"),
        Status::Partial { refused, note } => {
            format!("partial: `={}` refused ({note})", refused.join("`, `="))
        }
        other => other.word().to_owned(),
    }
}

/// The rows the M2 join changes (`spec/dyn.md` R-C1;
/// [`crate::table::join_changes`]).
fn join_markdown(out: &mut String) {
    let changes = crate::table::join_changes();
    let after =
        |f: &dyn Fn(Status) -> bool| rows().iter().filter(|r| f(r.status_at_join())).count();
    let _ = writeln!(
        out,
        "## At the M2 join\n\n\
         Dynamic output is refused by name until the M2 join (`crates/mortar/src/dynamic.rs`).\n\
         These {} rows take their M2 status (`spec/dyn.md` R-C1) in the change that lifts it;\n\
         the table then counts {} supported, {} partial, {} noop, {} refused.\n",
        changes.len(),
        after(&|s| s == Status::Supported),
        after(&|s| matches!(s, Status::Partial { .. })),
        after(&|s| matches!(s, Status::Noop(_))),
        after(&|s| s == Status::Refused),
    );
    out.push_str("| Option | Status today | Status at the join |\n");
    out.push_str("|---|---|---|\n");
    for (name, status) in changes {
        let Some(row) = rows().iter().find(|r| r.name() == *name) else { continue };
        let spellings: Vec<String> =
            row.spellings.iter().map(|s| format!("`{}`", s.display())).collect();
        let _ = writeln!(
            out,
            "| {} | {} | {} |",
            spellings.join(", "),
            status_cell(row.status),
            status_cell(*status)
        );
    }
}
