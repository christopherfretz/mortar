//! Linker-defined keys after the delta layout (`spec/incr.md` IN-71c;
//! IN-70 step U7: `DEFINED` recomputed from `P_U`). WP-213 (U-1): the
//! type [`DefinedChange`] and the entry point [`recompute`]; **filled by
//! U-3** (the function only).

use mortar_core::SymbolId;
use mortar_core::incr::LayoutDelta;
use mortar_core::layout::Defined;

use super::view::StateView;

/// A `DEFINED` key whose value, section or existence differs (WP-211 §C
/// `DefinedChange`; IN-71c). Key: `key`. Written by U-3; read by U-3
/// (`refill_set`: the key's referrers), U-5, U-2 (`DEFINED` rewritten).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefinedChange {
    /// The `SYMBOLS` row.
    pub key: SymbolId,
    /// The stored definition, `None` when the update defines it anew.
    pub old: Option<Defined>,
    /// The update's, `None` when it is no longer defined.
    pub new: Option<Defined>,
}

/// The linker-defined keys `layout` changes (WP-211 §C; IN-71c). Called
/// by the join after U-4 (IN-70 U7).
///
/// Until U-3 lands: empty.
#[must_use]
pub fn recompute(view: &StateView<'_>, layout: &LayoutDelta) -> Vec<DefinedChange> {
    let _ = (view, layout);
    Vec::new()
}
