//! The per-key refusals of `COPY` and `CANON_PLT` demands (`spec/dyn.md`
//! D-18, D-42): checked once per key against its shared definition, at
//! the key's first demanding record in input order.

use std::collections::BTreeMap;

use mortar_core::resolution::Def;
use mortar_core::{SymKey, SymbolId};
use mortar_diag::{Diagnostic, Key, Kind, Place};
use mortar_elf::consts::{STT_FUNC, STT_GNU_IFUNC, STT_TLS, STV_PROTECTED};

use crate::driver::{Sec, Unit};
use crate::view::{Bufs, View};

/// What is wrong with a `COPY`/`CANON_PLT` demand on one key, if anything.
fn refusal(view: &View<'_, '_>, id: SymbolId) -> Option<Kind> {
    let Def::Shared(e) = view.def(id) else { return None };
    let sym = view.sym(e.file, e.index)?;
    let name = || view.shown_name(SymKey::global(id));
    if sym.visibility() == STV_PROTECTED {
        let file = view.inputs.files[e.file].display().path().to_owned();
        return Some(Kind::CopyRelocationAgainstProtected { name: name(), file });
    }
    let func = matches!(sym.sym_type(), STT_FUNC | STT_GNU_IFUNC);
    if func {
        // `CANON_PLT` (D-44): no size, no copy.
        return None;
    }
    if sym.sym_type() == STT_TLS {
        // Supervisor's ruling (WP-046): the TLS mismatch of D-16 is
        // resolve's (WP-043); arch.md §4.3's TLS rule refuses a non-TLS
        // class against a TLS target before the table, so a copy of one
        // here is a violated invariant, never a copy.
        return Some(Kind::InternalError {
            what: format!(
                "copy relocation of the thread-local symbol `{}' (spec/dyn.md D-42): \
                 the TLS rule of spec/arch.md §4.3 should have refused it",
                name()
            ),
        });
    }
    if sym.st_size == 0 {
        return Some(Kind::CopyRelocationOfZeroSize { name: name() });
    }
    None
}

/// D-18, D-42: one diagnostic per key whose `COPY` or `CANON_PLT` demand
/// cannot be met, at its first demanding record (least scan key).
/// `refs`: `(key, scan key, unit, record)` of every demanding record.
pub(crate) fn refusals(
    view: &View<'_, '_>,
    units: &[Unit],
    refs: Vec<(SymbolId, Key, usize, u32)>,
    out: &mut Vec<(Key, Diagnostic)>,
) {
    if refs.is_empty() {
        return;
    }
    let mut first: BTreeMap<SymbolId, (Key, usize, u32)> = BTreeMap::new();
    for (id, k, unit, record) in refs {
        let e = first.entry(id).or_insert((k, unit, record));
        if k < e.0 {
            *e = (k, unit, record);
        }
    }
    let mut bufs = Bufs::default();
    for (id, (k, unit, record)) in first {
        let Some(kind) = refusal(view, id) else { continue };
        let u = &units[unit];
        let mut d = Diagnostic::error(kind);
        if let Ok(sec) = Sec::open(view, u.file, u.section, u.index, u.rel, &mut bufs) {
            let off = sec.recs.records.raw(record).map_or(0, |r| r.r_offset);
            d = d.at(Place::section(sec.path(), sec.name_str(), off));
        }
        out.push((k, d));
    }
}
