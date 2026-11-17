//! The per-key refusals of `COPY` and `CANON_PLT` demands (`spec/dyn.md`
//! D-18, D-42, D-43a): checked once per key against its shared
//! definition, at the key's first demanding record in input order.

use std::collections::BTreeMap;

use mortar_core::inputs::{BindsLocally, LocalBinding};
use mortar_core::resolution::Def;
use mortar_core::{SymKey, SymbolId};
use mortar_diag::{Diagnostic, Key, Kind, Place};
use mortar_elf::consts::{STT_FUNC, STT_GNU_IFUNC, STT_TLS, STV_PROTECTED};

use crate::driver::{Sec, Unit};
use crate::view::{Bufs, View};

/// Where a copied definition lies in the shared object that defines it
/// (`spec/dyn.md` D-43a), from its program headers alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lies {
    /// In a `PF_W` `PT_LOAD`, outside `PT_GNU_RELRO`: refused.
    Writable,
    /// In a read-only `PT_LOAD`, or inside `PT_GNU_RELRO`: warned.
    ReadOnly,
    /// In no `PT_LOAD`: refused (mortar cannot tell).
    Nowhere,
    /// A `PT_LOAD` whose `p_vaddr + p_memsz` overflows: refused (mortar
    /// cannot tell where the object's memory is).
    Overflow,
}

/// Where `value` lies among `binding`'s segments (D-43a). The headers'
/// values are unchecked (`LoadSpan`): a `PT_LOAD` whose end overflows
/// makes every answer [`Lies::Overflow`]; a `PT_GNU_RELRO` whose end
/// overflows is taken as absent (which can only turn a warning into a
/// refusal). When `PT_LOAD`s overlap at `value` (a malformed object),
/// any writable one decides.
pub(crate) fn lies(binding: &LocalBinding, value: u64) -> Lies {
    let mut found = None;
    for l in &binding.loads {
        let Some(end) = l.vaddr.checked_add(l.memsz) else { return Lies::Overflow };
        if l.vaddr <= value && value < end {
            found = Some(found.unwrap_or(false) | l.writable);
        }
    }
    let Some(writable) = found else { return Lies::Nowhere };
    let relro = binding.relro.is_some_and(|(at, size)| {
        at.checked_add(size).is_some_and(|end| at <= value && value < end)
    });
    if writable && !relro { Lies::Writable } else { Lies::ReadOnly }
}

/// D-43a: the diagnostic of a copy of a definition its library binds
/// locally (the executable's copy and the library's own instance are two
/// objects), if it is one.
fn locally_bound(
    view: &View<'_, '_>,
    id: SymbolId,
    file: mortar_core::FileId,
    index: u32,
    value: u64,
) -> Option<Diagnostic> {
    let binding = view.inputs.local_binding(file)?;
    if !binding.binds(index) {
        return None;
    }
    let name = view.shown_name(SymKey::global(id));
    let path = view.inputs.files[file].display().path().to_owned();
    let how = match &binding.binds {
        BindsLocally::Symbolic => {
            format!("{path} is linked -Bsymbolic (DT_SYMBOLIC or DF_SYMBOLIC)")
        }
        BindsLocally::Rows(_) => {
            format!("the .note.mason table of {path} marks {name} NONPREEMPTIBLE")
        }
        BindsLocally::Unreadable(why) => format!(
            "the .note.mason table of {path} does not parse ({why}): every definition is taken as bound locally"
        ),
    };
    let kind = Kind::CopyOfLocallyBoundDefinition { name: name.clone(), file: path.clone() };
    let d = match lies(binding, value) {
        Lies::Writable => Diagnostic::error(kind)
            .note(format!("{name} is writable in {path}: the program would write its copy and {path} read its own")),
        Lies::ReadOnly => {
            Diagnostic::warning(kind).note(format!("{name} is read-only in {path}: two equal copies"))
        }
        Lies::Nowhere => Diagnostic::error(kind).note(format!(
            "{name}'s value {value:#x} lies in no PT_LOAD of {path}: mortar cannot tell whether it is writable"
        )),
        Lies::Overflow => Diagnostic::error(kind).note(format!(
            "a PT_LOAD of {path} ends past the address space: mortar cannot tell whether {name} is writable"
        )),
    };
    Some(d.note(how))
}

/// What is wrong with a `COPY`/`CANON_PLT` demand on one key, if anything:
/// an error, or D-43a's warning.
fn refusal(view: &View<'_, '_>, id: SymbolId) -> Option<Diagnostic> {
    let Def::Shared(e) = view.def(id) else { return None };
    let sym = view.sym(e.file, e.index)?;
    let name = || view.shown_name(SymKey::global(id));
    if sym.visibility() == STV_PROTECTED {
        let file = view.inputs.files[e.file].display().path().to_owned();
        return Some(Diagnostic::error(Kind::CopyRelocationAgainstProtected {
            name: name(),
            file,
        }));
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
        return Some(Diagnostic::error(Kind::InternalError {
            what: format!(
                "copy relocation of the thread-local symbol `{}' (spec/dyn.md D-42): \
                 the TLS rule of spec/arch.md §4.3 should have refused it",
                name()
            ),
        }));
    }
    if sym.st_size == 0 {
        return Some(Diagnostic::error(Kind::CopyRelocationOfZeroSize { name: name() }));
    }
    locally_bound(view, id, e.file, e.index.0, sym.st_value)
}

/// D-18, D-42, D-43a: one diagnostic per key whose `COPY` or `CANON_PLT`
/// demand cannot be met (or, D-43a, is met with two objects of one
/// constant), at its first demanding record (least scan key).
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
        let Some(mut d) = refusal(view, id) else { continue };
        let u = &units[unit];
        if let Ok(sec) = Sec::open(view, u.file, u.section, u.index, u.rel, &mut bufs) {
            let off = sec.recs.records.raw(record).map_or(0, |r| r.r_offset);
            d = d.at(Place::section(sec.path(), sec.name_str(), off));
        }
        out.push((k, d));
    }
}

#[cfg(test)]
mod tests {
    use mortar_core::FileId;
    use mortar_core::inputs::{BindsLocally, LoadSpan, LocalBinding};

    use super::{Lies, lies};

    const RO: bool = false;
    const RW: bool = true;

    fn binding(loads: &[(u64, u64, bool)], relro: Option<(u64, u64)>) -> LocalBinding {
        LocalBinding {
            file: FileId(1),
            binds: BindsLocally::Symbolic,
            loads: loads
                .iter()
                .map(|&(vaddr, memsz, writable)| LoadSpan { vaddr, memsz, writable })
                .collect(),
            relro,
        }
    }

    /// GNU ld's usual library: text read-only, then one writable segment
    /// whose start is RELRO (`.data.rel.ro`, `.got`), then `.data`/`.bss`.
    fn usual() -> LocalBinding {
        binding(&[(0, 0x1000, RO), (0x1f000, 0x2000, RW)], Some((0x1f000, 0x1000)))
    }

    #[test]
    fn d43a_each_place_kind() {
        let b = usual();
        assert_eq!(lies(&b, 0x800), Lies::ReadOnly, ".rodata");
        assert_eq!(lies(&b, 0x1f010), Lies::ReadOnly, ".data.rel.ro");
        assert_eq!(lies(&b, 0x1ffff), Lies::ReadOnly, "the last RELRO byte");
        assert_eq!(lies(&b, 0x20000), Lies::Writable, "the first byte after RELRO");
        assert_eq!(lies(&b, 0x20ff8), Lies::Writable, ".bss");
        assert_eq!(lies(&b, 0x21000), Lies::Nowhere, "one past the last PT_LOAD");
        assert_eq!(lies(&b, 0x1000), Lies::Nowhere, "between the PT_LOADs");
        assert_eq!(lies(&b, u64::MAX), Lies::Nowhere);
    }

    #[test]
    fn d43a_without_relro_writable_is_refused() {
        let b = binding(&[(0, 0x1000, RO), (0x1f000, 0x2000, RW)], None);
        assert_eq!(lies(&b, 0x1f010), Lies::Writable);
        assert_eq!(lies(&b, 0x10), Lies::ReadOnly);
    }

    #[test]
    fn d43a_no_load_at_all() {
        let b = binding(&[], Some((0, 0x1000)));
        assert_eq!(lies(&b, 0x10), Lies::Nowhere, "RELRO alone is no PT_LOAD");
    }

    #[test]
    fn d43a_empty_load_contains_nothing() {
        let b = binding(&[(0x1000, 0, RW)], None);
        assert_eq!(lies(&b, 0x1000), Lies::Nowhere);
    }

    #[test]
    fn d43a_overflowing_load_is_refused() {
        // `p_vaddr + p_memsz` past 2^64, before, at or after the value.
        let b = binding(&[(0, 0x1000, RO), (u64::MAX - 0xf, 0x20, RO)], None);
        assert_eq!(lies(&b, 0x10), Lies::Overflow);
        let b = binding(&[(u64::MAX, 1, RO), (0, 0x1000, RO)], None);
        assert_eq!(lies(&b, 0x10), Lies::Overflow);
        // Ending exactly at 2^64 - 1 is not an overflow.
        let b = binding(&[(u64::MAX - 0xf, 0xf, RO)], None);
        assert_eq!(lies(&b, u64::MAX - 1), Lies::ReadOnly);
    }

    #[test]
    fn d43a_overflowing_relro_is_absent() {
        let b = binding(&[(0x1000, 0x1000, RW)], Some((0x1000, u64::MAX)));
        assert_eq!(lies(&b, 0x1010), Lies::Writable);
    }

    #[test]
    fn d43a_overlapping_loads_writable_decides() {
        let b = binding(&[(0, 0x2000, RO), (0x1000, 0x1000, RW)], None);
        assert_eq!(lies(&b, 0x1800), Lies::Writable);
        let b = binding(&[(0x1000, 0x1000, RW), (0, 0x2000, RO)], None);
        assert_eq!(lies(&b, 0x1800), Lies::Writable);
        assert_eq!(lies(&b, 0x800), Lies::ReadOnly);
    }

    #[test]
    fn d43a_rows_bind_only_their_indices() {
        let mut b = usual();
        b.binds = BindsLocally::Rows(vec![3, 7].into_boxed_slice());
        assert!(b.binds(3) && b.binds(7));
        assert!(!b.binds(4) && !b.binds(0));
        b.binds = BindsLocally::Unreadable("version 2".into());
        assert!(b.binds(4), "an unreadable table binds every definition");
    }
}
