//! One-definition-rule checks (`spec/odr.md`): the checks on foreign
//! objects (W1-W4), the record checks on Mason units, and the tables for
//! dynamic checks. Depends on `mortar_core`, `mortar_meta`, and on
//! `mortar_elf` and `mortar_diag` for the symbol tables resolution read
//! and the diagnostics (as `mortar_resolve` does).
//!
//! WP-072: the foreign checks and the switches; WP-073: the record
//! checks; WP-075: the check against shared-object inputs (O-24) and the
//! `.note.mason` table's rows (O-50).
#![forbid(unsafe_code)]

pub mod archives;
pub mod dynamic;
pub mod foreign;
pub mod records;
pub mod register;

use std::time::Instant;

use mortar_core::par::Pool;
use mortar_core::plan::{Odr, OdrMode};
use mortar_core::{Inputs, Resolution};
use mortar_diag::{Diagnostic, Key};

pub use crate::dynamic::{Output, TableRow};
use crate::foreign::{Checks, Counts};
use crate::records::Switches;
use crate::register::Check;
/// O-51's row and O-52's note encoder, for the front that writes the table.
pub use mortar_meta::table::{Row as NoteRow, note as table_note};

/// What the checks of a link found.
#[derive(Debug, Default)]
pub struct Outcome {
    /// The diagnostics, keyed (`spec/odr.md` §9.1); warnings.
    pub diagnostics: Vec<(Key, Diagnostic)>,
    /// `--odr-stats`'s line without its newline (O-44), when asked for.
    pub stats: Option<String>,
    /// The `.note.mason` table's rows (O-50), sorted by symbol, when the
    /// output is dynamic and `--odr-table` asks for one.
    pub table: Option<Vec<TableRow>>,
}

/// The one-definition-rule phase (40, after resolution): runs the checks
/// O-47's register turns on for the link's switches. With `--odr=off`
/// it reads nothing (O-42). `output` is the dynamic output's scope
/// (O-24) and kind; `None` for a static link.
#[must_use]
pub fn check(
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    odr: &Odr,
    output: Option<Output<'_>>,
    pool: &Pool,
) -> Outcome {
    let start = Instant::now();
    let checks = Checks {
        w1: Check::W1.runs(odr),
        w2: Check::W2.runs(odr),
        w3: Check::W3.runs(odr),
        w4: Check::W4.runs(odr),
    };
    let (mut diagnostics, counts) = foreign::check(inputs, resolution, checks, pool);
    let dyn_switches = dynamic_switches(odr, output.is_some());
    let mut s = switches(odr);
    s.keepers = dyn_switches.shared || dyn_switches.table;
    s.declared = dyn_switches.shared;
    let (records, mut r, rows) = records::check(inputs, resolution, s, pool);
    diagnostics.extend(records);
    let mut table = None;
    if let Some(output) = output
        && (dyn_switches.shared || dyn_switches.table)
    {
        let found = dynamic::check(inputs, resolution, output, dyn_switches, &rows);
        r.findings += found.diagnostics.len() as u64;
        r.tables = found.tables;
        r.table_rows = found.rows;
        diagnostics.extend(found.diagnostics);
        diagnostics.sort_by_key(|(k, _)| *k);
        table = found.table;
    }
    let stats = odr.stats.then(|| stats_line(*odr, &counts, &r, start.elapsed().as_micros()));
    Outcome { diagnostics, stats, table }
}

/// The dynamic checks the switches turn on, for a dynamic output
/// (`dynamic`): O-24 and O-50's table (O-47's rows 14 and 15).
#[must_use]
pub fn dynamic_switches(odr: &Odr, dynamic: bool) -> dynamic::Switches {
    dynamic::Switches {
        shared: dynamic && Check::SharedInputs.runs(odr),
        table: dynamic && Check::Table.runs(odr),
    }
}

/// The record checks the switches turn on (O-47's register).
#[must_use]
pub fn switches(odr: &Odr) -> Switches {
    Switches {
        abi: Check::AbiIsa.runs(odr),
        vtable: Check::VtableRtti.runs(odr),
        kind5_cxx: Check::Kind5Cxx.runs(odr),
        kind5_c: Check::Kind5C.runs(odr),
        kind3_layout: Check::Kind3Layout.runs(odr),
        kind3_shape: Check::Kind3Shape.runs(odr),
        kind3_definition: Check::Kind3Definition.runs(odr),
        kind2: Check::Kind2.runs(odr),
        internal: Check::InternalReference.runs(odr),
        // O-32: reported under strict; counted in any mode that compares.
        system_std_reported: Check::SystemStd.runs(odr),
        inlined_only: odr.mode == OdrMode::Strict,
        keepers: false,
        declared: false,
        archives: Check::ArchiveShadow.runs(odr),
        types: false,
    }
}

/// Whether the link reads Mason containers: a record check is on
/// (`spec/meta.md` MT-42, O-42). The store's `containers` switch.
#[must_use]
pub fn reads_containers(odr: &Odr) -> bool {
    switches(odr).any() || Check::Table.runs(odr) || Check::SharedInputs.runs(odr)
}

/// O-44's line.
fn stats_line(odr: Odr, c: &Counts, r: &records::Counts, micros: u128) -> String {
    format!(
        "mortar: odr-stats: mode {}; units {}, rows {} (kind 1 {}, kind 2 {}, kind 3 {}, kind 5 \
         {}), schemes {}, pairs not compared {}, type groups compared across schemes {}, ambiguous anchors {}, unanchored rows {}, \
         system -std differences {}, rows set aside {}, container bytes read {}, shared-object \
         tables {} ({} rows), archive members {} ({} units, {} bytes, {} rows compared), record \
         findings {}; files with COMDAT copies {}, groups {}, copies {}, definitions {}; \
         findings W1 {}, W2 {}, W3 {}, W4 {}; time {}.{:03} ms",
        odr.mode.word(),
        r.units,
        r.rows.iter().sum::<u64>(),
        r.rows[0],
        r.rows[1],
        r.rows[2],
        r.rows[3],
        r.schemes,
        r.pairs_not_compared,
        r.cross_scheme,
        r.ambiguous,
        r.unanchored,
        r.system_std,
        r.ignored,
        r.bytes,
        r.tables,
        r.table_rows,
        r.archive_members,
        r.archive_units,
        r.archive_bytes,
        r.archive_compared,
        r.findings,
        c.files,
        c.groups,
        c.copies,
        c.definitions,
        c.w[0],
        c.w[1],
        c.w[2],
        c.w[3],
        micros / 1000,
        micros % 1000,
    )
}
