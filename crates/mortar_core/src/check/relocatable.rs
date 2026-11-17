//! `Layout::check` of a relocatable placement (`-r`; `spec/model.md` §4.6
//! MD-R1, `spec/layout.md` L-R10) [M4-r].
//!
//! Owner: RL-3 (layout at `-r`, record `spec/landed/layout/WP-154.md` §F),
//! which fills [`validate`] with L-R10's readings of §4.4's rules (rule 1
//! as written; rule 2 on file offsets only; rule 3 with no segments; rule 4
//! vacuous; rule 5 with no `defined`, no `sym_place` other than `-d`'s
//! commons, and no `entry`) and its added rules (one `SHT_GROUP` chunk per
//! kept group, its members `SHF_GROUP` output sections; one relocation
//! chunk exactly per output section with a relocated piece; no relocation
//! chunk targets a group or a relocation chunk). RL-3 may add private items
//! here and edits no other file of `mortar_core` (WP-181).

use crate::check::{CheckContext, Rule, Violation};
use crate::layout::Layout;

/// Whether a relocatable layout is valid (L-R10): `Ok`, or the first
/// violation, as `Layout::validate` answers.
///
/// # Errors
///
/// Until RL-3 lands, every relocatable layout: no `-r` layout is computed
/// or loaded before then (`-r` is refused at the command line), and a
/// check that is not written passes nothing.
pub fn validate(layout: &Layout, cx: &CheckContext<'_, '_>) -> Result<(), Violation> {
    let _ = (layout, cx);
    Err(Violation {
        rule: Rule::Relocatable,
        what: "the readings of spec/layout.md L-R10 are not written yet (RL-3)".to_owned(),
        rows: Vec::new(),
    })
}
