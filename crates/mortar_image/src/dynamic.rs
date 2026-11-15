//! `.dynamic`'s values, `.interp`, `_DYNAMIC` and the GOT header words
//! (`spec/dyn.md` D-30 to D-32; WP-M2-DYNAMIC).
//!
//! `mortar_synth::dynamic` fixes `.dynamic`'s tags before addresses; this
//! module answers each entry's value once they are known
//! ([`DynamicChunk`], a [`SynthChunk`]): the address or size of another
//! chunk or of an output section, a symbol's address, and the `.dynstr`
//! offsets of `Slots.dynstr` (D-24; nothing here builds a string). It does
//! not touch `fill`'s relocation path.
//!
//! D-32: `_DYNAMIC` is `.dynamic`'s address ([`dynamic_symbol`]), and the
//! first word of the GOT the architecture names holds it
//! ([`reserved_word`]): AArch64 `.got[0]`, x86-64 and i386
//! `.got.plt[0]`; the other reserved words are 0. Neither carries a
//! dynamic record.

use std::ops::Range;

use mortar_arch::{Facts, GotBase};
use mortar_core::DynStr;
use mortar_core::id::SymbolId;
use mortar_synth::chunks::SynthKind;
use mortar_synth::dynamic::{ArraySection, DynKind, DynValue, DynamicPlan, RunpathTag};
use mortar_synth::error::internal;
use mortar_synth::image::{Division, FillCx, SizeCx, SynthChunk};
use mortar_synth::{InterpPlan, SynthError, Window};

use crate::map::{self, DynRecords};

/// The addresses `.dynamic` names, as the image knows them after layout.
pub trait DynAddrs: Sync {
    /// A synthetic chunk's `(address, size)`; `None` when it does not
    /// exist.
    fn chunk(&self, kind: SynthKind) -> Option<(u64, u64)>;
    /// An output section's `(address, size)` (layout L-86); `None` when
    /// there is none of that name.
    fn section(&self, section: ArraySection) -> Option<(u64, u64)>;
    /// A symbol's address (its final value); `None` when it has none.
    fn symbol(&self, key: SymbolId) -> Option<u64>;
}

/// The value of `v` (D-30's value column). A reference to something the
/// image does not have is an internal error: the plan named it because
/// the inputs said it exists.
///
/// # Errors
///
/// `internal-error` naming the missing chunk, section, symbol or string.
pub fn value(v: DynValue, dynstr: &DynStr, addrs: &dyn DynAddrs) -> Result<u64, SynthError> {
    let missing = |what: String| internal::<SynthError>(format!(".dynamic names {what}"));
    let chunk = |k: SynthKind| {
        addrs.chunk(k).ok_or_else(|| {
            missing(format!("{} but there is none", String::from_utf8_lossy(k.name(false))))
        })
    };
    let section = |s: ArraySection| {
        addrs.section(s).ok_or_else(|| {
            missing(format!("{} but there is none", String::from_utf8_lossy(s.name())))
        })
    };
    Ok(match v {
        DynValue::Const(c) => c,
        DynValue::NeededName(i) => u64::from(
            *dynstr
                .needed
                .get(i as usize)
                .ok_or_else(|| missing(format!("needed object {i} with no .dynstr name")))?,
        ),
        DynValue::RunpathName => u64::from(
            dynstr.runpath.ok_or_else(|| missing("a run path with no .dynstr string".into()))?,
        ),
        DynValue::DynstrSize => {
            if dynstr.is_empty() {
                return Err(missing("an empty .dynstr".into()));
            }
            u64::from(dynstr.size)
        }
        DynValue::ChunkAddr(k) => chunk(k)?.0,
        DynValue::ChunkSize(k) => chunk(k)?.1,
        DynValue::SectionAddr(s) => section(s)?.0,
        DynValue::SectionSize(s) => section(s)?.1,
        DynValue::Symbol(key) => {
            addrs.symbol(key).ok_or_else(|| missing(format!("symbol {} with no address", key.0)))?
        }
    })
}

/// `.dynamic` as the image drives it: the plan, `.dynstr`'s offsets and
/// the addresses.
#[derive(Clone, Copy)]
pub struct DynamicChunk<'a> {
    pub plan: &'a DynamicPlan,
    pub dynstr: &'a DynStr,
    pub addrs: &'a dyn DynAddrs,
}

impl std::fmt::Debug for DynamicChunk<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynamicChunk").field("plan", self.plan).finish_non_exhaustive()
    }
}

impl SynthChunk for DynamicChunk<'_> {
    fn size(&self, _: &SizeCx<'_>) -> u64 {
        self.plan.size()
    }

    /// Divisible: fixed-size entries, each value one lookup.
    fn division(&self) -> Division {
        Division::Divisible
    }

    fn write(&self, _: &FillCx<'_>, range: Range<u64>, out: &mut [u8]) -> Result<(), SynthError> {
        let size = self.plan.size();
        if range.start > range.end
            || range.end > size
            || out.len() as u64 != range.end - range.start
        {
            return Err(internal(format!(
                ".dynamic of {size} bytes written at {}..{} into {} bytes",
                range.start,
                range.end,
                out.len()
            )));
        }
        let resolve = |v: DynValue| value(v, self.dynstr, self.addrs);
        self.plan.write(&resolve, &mut Window::new(range.start, out))
    }
}

/// `_DYNAMIC`'s value (D-32; layout's table, resolve §8.1): the address
/// of `.dynamic`, in `Exec`, `Pie` and `StaticPie`; `None` without one.
#[must_use]
pub fn dynamic_symbol(addrs: &dyn DynAddrs) -> Option<u64> {
    addrs.chunk(SynthKind::Dynamic).map(|(at, _)| at)
}

/// The GOT that holds the reserved words (D-32).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GotTable {
    /// `.got` (its header words, `Facts.got_header`).
    Got,
    /// `.got.plt` (its reserved words, `Facts.got_plt_reserved`).
    GotPlt,
}

/// The table whose word 0 holds `_DYNAMIC` in an output with `.dynamic`
/// (D-32): `.got` where the GOT base is `.got`'s start (AArch64, a64
/// §10.1), `.got.plt` where it is `.got.plt`'s (x86-64 and i386, the
/// psABIs).
#[must_use]
pub fn dynamic_table(facts: &Facts) -> GotTable {
    match facts.got_base {
        GotBase::GotStart => GotTable::Got,
        GotBase::GotPltStart => GotTable::GotPlt,
    }
}

/// Reserved word `index` of `table` in an output with `.dynamic` whose
/// `.dynamic` is at `dynamic` (D-32): `_DYNAMIC`'s link-time address for
/// word 0 of [`dynamic_table`], 0 for every other reserved word (the
/// loader fills `.got.plt[1]` and `[2]`). No word carries a record; in a
/// PIE the loader reads the link-time value as written. A static output
/// writes 0 in every reserved word (synth S-65, S-67) and does not ask.
#[must_use]
pub fn reserved_word(facts: &Facts, table: GotTable, index: u8, dynamic: u64) -> u64 {
    if index == 0 && table == dynamic_table(facts) { dynamic } else { 0 }
}

/// The map's dynamic records this package owns (`spec/plumb.md` §3.2,
/// P-D3): `output`, `interp`, `runpath`/`rpath`, `bind`, `relro`. The
/// run path is the string `mortar_synth::dynamic::join_rpath` gives.
pub fn fill_records(
    records: &mut DynRecords,
    kind: DynKind,
    interp: Option<&InterpPlan>,
    runpath: Option<(RunpathTag, &[u8])>,
    bind_now: bool,
    relro: bool,
) {
    records.kind = Some(match kind {
        DynKind::Exec => map::OutputKind::Exec,
        DynKind::Pie => map::OutputKind::Pie,
        DynKind::StaticPie => map::OutputKind::StaticPie,
    });
    records.interp = interp.map(|i| i.path().to_vec());
    records.runpath = runpath.map(|(tag, paths)| {
        let t = match tag {
            RunpathTag::Runpath => map::RunpathTag::Runpath,
            RunpathTag::Rpath => map::RunpathTag::Rpath,
        };
        (t, paths.to_vec())
    });
    records.now = Some(bind_now);
    records.relro = Some(relro);
}

#[cfg(test)]
mod tests {
    use super::*;
    use mortar_arch::{Arch, RelForm};
    use mortar_elf::Class;
    use mortar_elf::consts::{DT_INIT, DT_NEEDED, DT_RUNPATH, DT_STRSZ, DT_STRTAB};
    use mortar_synth::dynamic::DynamicInputs;

    struct Addrs;
    impl DynAddrs for Addrs {
        fn chunk(&self, kind: SynthKind) -> Option<(u64, u64)> {
            match kind {
                SynthKind::Dynstr => Some((0x400, 0x80)),
                SynthKind::Dynsym => Some((0x300, 0x48)),
                SynthKind::GnuHash => Some((0x2a0, 0x1c)),
                SynthKind::Dynamic => Some((0x1fdb8, 0x1f0)),
                _ => None,
            }
        }
        fn section(&self, _: ArraySection) -> Option<(u64, u64)> {
            None
        }
        fn symbol(&self, key: SymbolId) -> Option<u64> {
            (key == SymbolId(9)).then_some(0x5a8)
        }
    }

    fn inputs() -> DynamicInputs {
        DynamicInputs {
            kind: DynKind::Pie,
            class: Class::Elf64,
            rel_form: RelForm::Rela,
            needed: 1,
            runpath: Some(RunpathTag::Runpath),
            init: Some(SymbolId(9)),
            fini: None,
            preinit_array: false,
            init_array: false,
            fini_array: false,
            got_plt: false,
            rela_plt: false,
            rela_dyn: false,
            relative: 0,
            verneed: 0,
            bind_now: false,
            arch_tags: &[],
            bti_property: false,
            variant_pcs_jump_slot: false,
        }
    }

    fn dynstr() -> DynStr {
        DynStr {
            symbols: Box::new([]),
            needed: Box::new([1]),
            versions: Box::new([]),
            runpath: Some(11),
            size: 0x80,
        }
    }

    fn entry(bytes: &[u8], k: usize) -> (i64, u64) {
        let w = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        (w(16 * k) as i64, w(16 * k + 8))
    }

    // D-30's values: string offsets from the DynStr, chunk addresses and
    // sizes, a symbol's address.
    #[test]
    fn values_from_the_tables() {
        let plan = DynamicPlan::new(&inputs()).unwrap();
        let ds = dynstr();
        let c = DynamicChunk { plan: &plan, dynstr: &ds, addrs: &Addrs };
        let arch = mortar_a64::A64::M1;
        let pool = mortar_core::par::Pool::sequential();
        let output = mortar_diag::FileRef::new("out");
        let cx = crate::synth::no_answers(&arch as &dyn Arch, &pool, &output);
        let size = c.size(&SizeCx::new());
        let mut out = vec![0xee; size as usize];
        c.write(&cx, 0..size, &mut out).unwrap();
        let got: Vec<(i64, u64)> = (0..plan.entries().len()).map(|k| entry(&out, k)).collect();
        assert_eq!(got[0], (DT_NEEDED, 1));
        assert_eq!(got[1], (DT_RUNPATH, 11));
        assert_eq!(got[2], (DT_INIT, 0x5a8));
        assert!(got.contains(&(DT_STRTAB, 0x400)));
        assert!(got.contains(&(DT_STRSZ, 0x80)));
        assert!(out[16 * got.len()..].iter().all(|&b| b == 0));
    }

    // A value the image cannot answer is an internal error, never 0.
    #[test]
    fn missing_values_are_errors() {
        let mut i = inputs();
        i.init_array = true;
        let plan = DynamicPlan::new(&i).unwrap();
        let ds = dynstr();
        let mut buf = vec![0; plan.size() as usize];
        let r = plan.write(&|v| value(v, &ds, &Addrs), &mut Window::new(0, &mut buf));
        assert!(r.is_err());
        let empty = DynStr::default();
        assert!(value(DynValue::DynstrSize, &empty, &Addrs).is_err());
        assert!(value(DynValue::NeededName(0), &empty, &Addrs).is_err());
        assert!(value(DynValue::RunpathName, &empty, &Addrs).is_err());
        assert!(value(DynValue::Symbol(SymbolId(1)), &ds, &Addrs).is_err());
        assert!(value(DynValue::ChunkAddr(SynthKind::RelaDyn), &ds, &Addrs).is_err());
    }

    // D-32: `_DYNAMIC` is `.dynamic`'s address; AArch64 puts it in
    // `.got[0]`, x86-64 in `.got.plt[0]`; every other reserved word is 0.
    #[test]
    fn dynamic_and_the_got_header() {
        assert_eq!(dynamic_symbol(&Addrs), Some(0x1fdb8));
        let f = &mortar_a64::FACTS;
        assert_eq!(dynamic_table(f), GotTable::Got);
        assert_eq!(reserved_word(f, GotTable::Got, 0, 0x1fdb8), 0x1fdb8);
        for k in 0..3 {
            assert_eq!(reserved_word(f, GotTable::GotPlt, k, 0x1fdb8), 0);
        }
        let x64 = mortar_x64_facts();
        assert_eq!(dynamic_table(&x64), GotTable::GotPlt);
        assert_eq!(reserved_word(&x64, GotTable::GotPlt, 0, 0x3e10), 0x3e10);
        assert_eq!(reserved_word(&x64, GotTable::GotPlt, 1, 0x3e10), 0);
        assert_eq!(reserved_word(&x64, GotTable::GotPlt, 2, 0x3e10), 0);
        assert_eq!(reserved_word(&x64, GotTable::Got, 0, 0x3e10), 0);
    }

    // x86-64's facts without a dependency on `mortar_x64`: only the GOT
    // base differs from AArch64's for this rule.
    fn mortar_x64_facts() -> Facts {
        Facts { got_base: GotBase::GotPltStart, ..mortar_a64::FACTS }
    }

    // P-D3: the records of the command line's facts.
    #[test]
    fn map_records() {
        let mut r = DynRecords::default();
        let interp = InterpPlan::new(b"/lib/ld-linux-aarch64.so.1").unwrap();
        fill_records(
            &mut r,
            DynKind::Pie,
            Some(&interp),
            Some((RunpathTag::Rpath, b"/x:$ORIGIN")),
            true,
            false,
        );
        assert_eq!(r.kind, Some(map::OutputKind::Pie));
        assert_eq!(r.interp.as_deref(), Some(&b"/lib/ld-linux-aarch64.so.1"[..]));
        assert_eq!(r.runpath, Some((map::RunpathTag::Rpath, b"/x:$ORIGIN".to_vec())));
        assert_eq!((r.now, r.relro), (Some(true), Some(false)));
        let mut s = DynRecords::default();
        fill_records(&mut s, DynKind::StaticPie, None, None, false, true);
        assert_eq!(s.kind, Some(map::OutputKind::StaticPie));
        assert_eq!((s.interp, s.runpath, s.now, s.relro), (None, None, Some(false), Some(true)));
    }
}
