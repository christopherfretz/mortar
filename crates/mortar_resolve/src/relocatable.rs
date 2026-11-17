//! Resolution at `-r` (`spec/resolve.md` §11). Owner: **RL-2** (front and
//! resolve at `-r`): RV-R1 to RV-R6, RV-R8, RV-R9, and RV-R7's election
//! and dropped copies (no `.eh_frame` work). Then RL-8 (RV-R7 in full,
//! `--force-group-allocation`) and RL-11 (RV-R5's `-d`, RV-R10's
//! `--defsym`), each after the one before has merged.
//!
//! WP-181 (RL-1) wrote this file with [`resolve`]'s signature and its
//! refusal; RL-2 fills [`resolve`] here and may add private items to
//! this file.

use mortar_core::Inputs;
use mortar_core::par::Pool;
use mortar_core::plan::Plan;
use mortar_diag::{Diagnostic, Key};

use crate::Outcome;

/// The resolution of a `-r` link (`Plan.kind == Relocatable`): the table
/// without verdicts (RV-R1), the undefined and common keys kept (RV-R3,
/// RV-R5), hidden keys global (RV-R6). RL-7's driver calls it in place of
/// `resolve_inputs_with` at `-r`.
///
/// # Errors
///
/// The diagnostics that stop the link before a table exists, keyed in
/// phase 30. Until RL-2 lands: `unsupported feature: relocatable output
/// (-r)` (`spec/diag.md` §4.4), never a table.
pub fn resolve(
    inputs: &Inputs<'_>,
    plan: &Plan,
    pool: &Pool,
) -> Result<Outcome, Vec<(Key, Diagnostic)>> {
    let _ = (inputs, plan, pool);
    Err(vec![(
        Key::new(30, 0, 0),
        mortar_diag::relocatable::refused_relocatable_output()
            .note("resolution at -r is RL-2's (spec/resolve.md §11)"),
    )])
}
