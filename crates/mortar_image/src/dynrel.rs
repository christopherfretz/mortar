//! The dynamic records of an `Exec` or `Pie` (`spec/dyn.md` D-40, D-46,
//! D-48): which record each relocation of a live section makes, their
//! bytes in `.rela.dyn`, and what the place of each holds.
//!
//! - **Which record** ([`roles`]): `arch.scan` as the scan's walk calls
//!   it, its counted roles captured. The scan's per-section counts
//!   (`Demands.dyn_relocs`) are the same calls' counts, so the records
//!   agree with the plan's sizes by construction (an excess or a shortfall
//!   is an internal error, [`fill_relative`]).
//! - **Records of a section** ([`section_records`]): `RELATIVE` with the
//!   addend `S + A` (the link-time address); a symbolic or TLS record
//!   against the key's `.dynsym` index with the addend `A` (D-46, D-48).
//! - **`.rela.dyn`** ([`fill_relative`], [`RelaDyn`]): the `RELATIVE`
//!   part filled in parallel, each unit (a section, the GOT) its own slice
//!   at the prefix sum of the counts before it in address order (D-40),
//!   so the bytes are the same at any number of threads; then the other
//!   records sorted by (`.dynsym` index, `r_offset`), then the copies'.
//! - **The place** ([`zeroed_place`]): under RELA a symbolic, `GLOB_DAT`,
//!   `COPY` or IE `TPOFF` record's place holds 0 (D-46, [`DynRole::place`]);
//!   fill writes `S + A` (`A` for a `Shlib` or `WeakDyn` key) and then
//!   clears the field of such a record.

use mortar_arch::{
    Arch, DemandOut, DemandSink, DynRole, EventClass, GlobalFlags, PlaceContent, Records, RelForm,
    RelocAt, ScanCx, ScanError, TargetKind,
};
use mortar_core::demands::DemandFlags;
use mortar_core::par::Pool;
use mortar_core::{SymKey, SymbolId};
use mortar_elf::Class;
use mortar_synth::rela::{self, DynRecord, RelaPlan};
use mortar_synth::{SynthError, Window};

/// The roles `arch.scan` counts for one record (at most two: a GD pair).
#[derive(Debug, Default)]
struct Capture {
    roles: Vec<DynRole>,
}

impl DemandOut for Capture {
    fn raise(&mut self, _: SymKey, _: DemandFlags) {}
    fn raise_global(&mut self, _: GlobalFlags) {}
    fn count(&mut self, role: DynRole, n: u32) {
        for _ in 0..n {
            self.roles.push(role);
        }
    }
    fn error(&mut self, _: u32, _: ScanError) {}
    fn reference(&mut self, _: u32, _: EventClass, _: SymKey) {}
}

/// The dynamic roles of the record at `at` (`arch.scan` with `cx`, as the
/// scan's walk calls it), in the order counted.
#[must_use]
pub fn roles(arch: &dyn Arch, cx: &ScanCx<'_>, at: &RelocAt<'_>) -> Vec<DynRole> {
    let mut c = Capture::default();
    arch.scan(cx, at, &mut DemandSink::new(&mut c));
    c.roles
}

/// Whether the fill clears the field of the record at `at` after writing
/// it (D-46: under RELA the place of a symbolic, `GLOB_DAT`, `COPY` or IE
/// `TPOFF` record holds 0). Only a `Shlib` or `WeakDyn` target can make
/// such a record, so a static output never pays for the call.
#[must_use]
pub fn zeroed_place(arch: &dyn Arch, kind: TargetKind, cx: &ScanCx<'_>, at: &RelocAt<'_>) -> bool {
    if !matches!(kind, TargetKind::Shlib { .. } | TargetKind::WeakDyn) {
        return false;
    }
    let form = arch.facts().rel_form;
    roles(arch, cx, at).iter().any(|r| r.place(form) == Some(PlaceContent::Zero))
}

/// The records of one relocated section of an output with `.dynamic`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SectionRecords {
    /// `RELATIVE` records, by `r_offset`.
    pub relative: Vec<DynRecord>,
    /// The others (symbolic, TLS), in record order.
    pub other: Vec<DynRecord>,
}

/// What [`section_records`] reads of the link for one section.
pub struct SectionCx<'a> {
    pub arch: &'a dyn Arch,
    pub scan: ScanCx<'a>,
    /// The section's address (`P` of offset 0).
    pub addr: u64,
    /// `S + A` of a record's key and addend, at link time (a `RELATIVE`'s
    /// addend; the IPLT entry for an IFUNC, S-71).
    pub value: &'a (dyn Fn(SymKey, i64) -> Option<u64> + Sync),
    /// The `.dynsym` index of a key (`DynsymPlan::index`).
    pub dynsym: &'a (dyn Fn(SymbolId) -> Option<u32> + Sync),
}

impl std::fmt::Debug for SectionCx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SectionCx").field("addr", &self.addr).finish_non_exhaustive()
    }
}

/// The dynamic records of a section: for each record (`at(i)` gives the
/// record `i` as the scan sees it, `None` for one the scan skipped), the
/// records its roles make (D-40, D-46, D-48). `IRELATIVE` is never one
/// (D-41). An error names what broke an invariant.
pub fn section_records<'r>(
    cx: &SectionCx<'_>,
    records: &dyn Records,
    at: &dyn Fn(u32) -> Option<RelocAt<'r>>,
) -> Result<SectionRecords, String> {
    let mut out = SectionRecords::default();
    for i in 0..records.len() {
        let Some(at) = at(i) else { continue };
        push_records(cx, &at, &mut out)?;
    }
    out.relative.sort_unstable_by_key(|r| r.r_offset);
    Ok(out)
}

/// The records of the one relocation at `at` ([`section_records`]'s
/// step), pushed onto `out` (`relative` unsorted). An error names what
/// broke an invariant.
pub fn push_records(
    cx: &SectionCx<'_>,
    at: &RelocAt<'_>,
    out: &mut SectionRecords,
) -> Result<(), String> {
    let i = at.index;
    let rs = roles(cx.arch, &cx.scan, at);
    if rs.is_empty() {
        return Ok(());
    }
    let addend = at.addend.unwrap_or(0);
    let r_offset = cx.addr.wrapping_add(at.r_offset);
    for role in rs {
        let r_type = cx
            .arch
            .dyn_type(role)
            .ok_or_else(|| format!("record {i}: no type for the role {role:?}"))?;
        match role {
            DynRole::Relative => {
                let v = (cx.value)(at.key, addend)
                    .ok_or_else(|| format!("record {i}: a RELATIVE record without S + A"))?;
                out.relative.push(DynRecord { r_offset, r_sym: 0, r_type, addend: v as i64 });
            }
            DynRole::IRelative => {
                return Err(format!("record {i}: an IRELATIVE record in .rela.dyn (D-41)"));
            }
            _ => {
                let id = at.key.as_global().ok_or_else(|| {
                    format!("record {i}: a {role:?} record against a local symbol")
                })?;
                let r_sym = (cx.dynsym)(id)
                    .ok_or_else(|| format!("record {i}: symbol {} not in .dynsym", id.0))?;
                let a = if matches!(role, DynRole::Symbolic) { addend } else { 0 };
                out.other.push(DynRecord { r_offset, r_sym, r_type, addend: a });
            }
        }
    }
    Ok(())
}

/// D-40 step 1: the `RELATIVE` part of `.rela.dyn`, its units (in address
/// order) filled on `pool`, each into its own slice at the prefix sum of
/// the counts before it. `counts[u]` is unit `u`'s count by the plan;
/// `unit(u)` makes its records (sorted by `r_offset`). The bytes do not
/// depend on the number of threads: no unit reads another's output.
pub fn fill_relative<F>(
    pool: &Pool,
    class: Class,
    form: RelForm,
    counts: &[u32],
    unit: F,
) -> Result<Vec<u8>, String>
where
    F: Fn(usize) -> Result<Vec<DynRecord>, String> + Sync,
{
    let e = rela::entsize(class, form) as usize;
    let total: usize = counts.iter().map(|&n| n as usize).sum();
    let mut bytes = vec![0u8; total * e];
    let mut parts: Vec<&mut [u8]> = Vec::with_capacity(counts.len());
    let mut rest = &mut bytes[..];
    for &n in counts {
        let (head, tail) = rest.split_at_mut(n as usize * e);
        parts.push(head);
        rest = tail;
    }
    let errors = std::sync::Mutex::new(Vec::<(usize, String)>::new());
    pool.for_each_part(parts, |u, slice| {
        let result = unit(u).and_then(|recs| {
            if recs.len() * e != slice.len() {
                return Err(format!(
                    "unit {u}: {} RELATIVE records, the plan counted {}",
                    recs.len(),
                    slice.len() / e
                ));
            }
            rela::encode(class, form, &recs, slice);
            Ok(())
        });
        if let Err(m) = result
            && let Ok(mut v) = errors.lock()
        {
            v.push((u, m));
        }
    });
    let mut errors = errors.into_inner().unwrap_or_default();
    errors.sort();
    match errors.into_iter().next() {
        Some((_, m)) => Err(m),
        None => Ok(bytes),
    }
}

/// `.rela.dyn`'s bytes once its records are known: the `RELATIVE` part
/// ([`fill_relative`]), then every other record sorted (D-40 step 2).
#[derive(Debug, Clone)]
pub struct RelaDyn {
    class: Class,
    form: RelForm,
    relative: Vec<u8>,
    other: Vec<DynRecord>,
}

impl RelaDyn {
    /// From the `RELATIVE` part's bytes and the other records (any
    /// order), checked against `plan`'s counts.
    pub fn new(
        plan: &RelaPlan,
        class: Class,
        form: RelForm,
        relative: Vec<u8>,
        mut other: Vec<DynRecord>,
    ) -> Result<Self, SynthError> {
        let e = rela::entsize(class, form);
        if relative.len() as u64 != plan.relative_count() * e
            || other.len() as u64 != plan.other_count()
        {
            return Err(mortar_synth::error::internal(format!(
                ".rela.dyn: {} RELATIVE and {} other records, the plan {} and {}",
                relative.len() as u64 / e,
                other.len(),
                plan.relative_count(),
                plan.other_count()
            )));
        }
        rela::sort_symbolic(&mut other);
        Ok(Self { class, form, relative, other })
    }

    /// The size in bytes.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.relative.len() as u64 + self.other.len() as u64 * rela::entsize(self.class, self.form)
    }

    /// Fills the window's range.
    pub fn write(&self, out: &mut Window<'_>) {
        let n = self.relative.len() as u64;
        if out.start() < n {
            let a = out.start() as usize;
            let b = out.end().min(n) as usize;
            out.put(a as u64, &self.relative[a..b]);
        }
        // The window's records only: the others are sorted, so a range.
        let e = rela::entsize(self.class, self.form);
        let first = n / e;
        let lo = (out.start().saturating_sub(n) / e) as usize;
        let hi = (out.end().saturating_sub(n).div_ceil(e) as usize).min(self.other.len());
        for k in lo..hi {
            rela::put(self.class, self.form, out, first + k as u64, &self.other[k]);
        }
    }
}
