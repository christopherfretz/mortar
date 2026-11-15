//! One-definition-rule checks (`spec/odr.md`): the checks on foreign
//! objects (W1-W4), the record checks on Mason units, and the tables for
//! dynamic checks. Depends on `mortar_core`, `mortar_meta`, and on
//! `mortar_elf` and `mortar_diag` for the symbol tables resolution read
//! and the diagnostics (as `mortar_resolve` does).
//!
//! WP-072: the foreign checks and the switches; the record checks land
//! with WP-073.
#![forbid(unsafe_code)]

pub mod foreign;
pub mod register;

use std::time::Instant;

use mortar_core::par::Pool;
use mortar_core::plan::Odr;
use mortar_core::{Inputs, Resolution};
use mortar_diag::{Diagnostic, Key};

use crate::foreign::{Checks, Counts};
use crate::register::Check;

/// What the checks of a link found.
#[derive(Debug, Default)]
pub struct Outcome {
    /// The diagnostics, keyed (`spec/odr.md` §9.1); warnings.
    pub diagnostics: Vec<(Key, Diagnostic)>,
    /// `--odr-stats`'s line without its newline (O-44), when asked for.
    pub stats: Option<String>,
}

/// The one-definition-rule phase (40, after resolution): runs the checks
/// O-47's register turns on for the link's switches. With `--odr=off`
/// it reads nothing (O-42).
#[must_use]
pub fn check(inputs: &Inputs<'_>, resolution: &Resolution, odr: &Odr, pool: &Pool) -> Outcome {
    let start = Instant::now();
    let checks = Checks {
        w1: Check::W1.runs(odr),
        w2: Check::W2.runs(odr),
        w3: Check::W3.runs(odr),
        w4: Check::W4.runs(odr),
    };
    let (diagnostics, counts) = foreign::check(inputs, resolution, checks, pool);
    let stats = odr.stats.then(|| stats_line(*odr, &counts, start.elapsed().as_micros()));
    Outcome { diagnostics, stats }
}

/// O-44's line. The record counts are zero until the record checks land
/// (WP-073): no container byte is read.
fn stats_line(odr: Odr, c: &Counts, micros: u128) -> String {
    format!(
        "mortar: odr-stats: mode {}; units 0, rows 0, schemes 0, pairs not compared 0, \
         ambiguous anchors 0, unanchored rows 0, system -std differences 0, container bytes \
         read 0; files with COMDAT copies {}, groups {}, copies {}, definitions {}; findings \
         W1 {}, W2 {}, W3 {}, W4 {}; time {}.{:03} ms",
        odr.mode.word(),
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
