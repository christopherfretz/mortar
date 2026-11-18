//! Linker-defined keys after the delta layout (`spec/incr.md` IN-71c;
//! IN-70 step U7: `DEFINED` recomputed from `P_U`). WP-213 (U-1): the
//! type [`DefinedChange`] and the entry point [`recompute`]; **filled by
//! U-3** (the function only).

use std::collections::{BTreeMap, BTreeSet};

use mortar_core::incr::LayoutDelta;
use mortar_core::layout::{DefValue, Defined};
use mortar_core::{ChunkId, SymbolId};

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
/// `layout.placement.defined` is the update's whole `DEFINED` set (U-4's
/// `P_U`), keyed by `SYMBOLS` row; it is compared with the stored rows.
/// A definition at an offset in a chunk counts as changed also when the
/// chunk's address moved (the value a referrer reads is the address).
/// Sorted by key. A stored state that cannot be read yields every stored
/// key as changed (the refill is then whole, never short).
#[must_use]
pub fn recompute(view: &StateView<'_>, layout: &LayoutDelta) -> Vec<DefinedChange> {
    let stored = view.defined().unwrap_or_default();
    let chunks = view.chunks().unwrap_or_default();
    let mut old: BTreeMap<u32, Defined> = BTreeMap::new();
    for r in &stored {
        let value = if r.kind == 0 {
            DefValue::Absolute(r.value)
        } else {
            DefValue::At { chunk: ChunkId(r.chunk), offset: r.value }
        };
        old.insert(r.symbol, Defined { symbol: SymbolId(r.symbol), value });
    }
    let mut new: BTreeMap<u32, Defined> = BTreeMap::new();
    for d in &layout.placement.defined {
        new.insert(d.symbol.0, *d);
    }
    let old_addr = |c: ChunkId| chunks.get(c.0 as usize).map(|r| r.addr);
    let new_addr = |c: ChunkId| layout.placement.chunks.get(c).and_then(|c| c.addr);
    let same = |a: &Defined, b: &Defined| match (a.value, b.value) {
        (DefValue::Absolute(x), DefValue::Absolute(y)) => x == y,
        (DefValue::At { chunk: c, offset: o }, DefValue::At { chunk: d, offset: p }) => {
            c == d && o == p && old_addr(c).is_some() && old_addr(c) == new_addr(d)
        }
        _ => false,
    };
    let keys: BTreeSet<u32> = old.keys().chain(new.keys()).copied().collect();
    let mut out = Vec::new();
    for k in keys {
        let (o, n) = (old.get(&k), new.get(&k));
        if let (Some(a), Some(b)) = (o, n)
            && same(a, b)
        {
            continue;
        }
        out.push(DefinedChange { key: SymbolId(k), old: o.copied(), new: n.copied() });
    }
    out
}
