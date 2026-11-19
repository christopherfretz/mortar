//! The scan driver (`spec/image.md` §4, phase 70): runs `Arch::scan` over
//! every relocation of every live section in parallel, merges the demands
//! deterministically, and turns reference events into one diagnostic per
//! group.
//!
//! [`scan`] does, in order (§4.1):
//!
//! 1. the **units**: every live section with a relocation section, in
//!    `(FileId, section index)` order, cut into blocks of
//!    [`SCAN_BLOCK`](mortar_core::par::SCAN_BLOCK) records; a cut that would
//!    split a sequence moves forward past it (step 1);
//! 2. one parallel pass over the units (`Pool::map`, one call), each
//!    walking its records in record order: decode, the key through
//!    `sym_map` and `redirect`, `classify`, `arch.scan`, the events of
//!    §4.4, the window of §3.4's overlap rule and the order check of
//!    I-29; in a live `SHF_ALLOC | SHF_EXECINSTR` section, the records
//!    that may be branch sites ([`BranchRec`], for layout's fixed point);
//! 3. the merge: global flags or-ed into atomics during the pass (I-16:
//!    read before or); local flags, counts, diagnostics and events
//!    concatenated in unit order, the events grouped by [`events::merge`]
//!    (I-20, I-21) and reported per group (I-22).
//!
//! The result is the same for every thread count and schedule: every
//! merge is an or, an integer sum, or a concatenation in unit order
//! followed by a stable sort (`spec/model.md` §6 rules 2 to 4).
//!
//! What the scan reads of an architecture's instruction proof is given by
//! the caller as a [`CodeProof`] (AArch64: its mapping symbols), because
//! `Arch` has no method for it (`spec/image.md` §17 is silent; follow-up).
#![forbid(unsafe_code)]

pub mod events;

mod bound;
mod copies;
mod driver;
pub mod regflow;
mod report;
mod view;
mod walk;
pub mod weak;

pub use driver::scan_file;
pub use view::{Prep, Program, Whole, global_facts, undefined_kind};
pub use walk::{Visit, Walker};

use std::sync::atomic::AtomicU16;

use mortar_arch::{Arch, OutputKind, ProofRun};
use mortar_core::assignment::Assignment;
use mortar_core::column::Column;
use mortar_core::demands::{DemandFlags, DynCounts};
use mortar_core::inputs::Inputs;
use mortar_core::par::Pool;
use mortar_core::resolution::Resolution;
pub use mortar_core::split::EhOwner;
use mortar_core::{
    ComdatId, Demands, FileId, Liveness, LocalSym, SectionId, SymEntry, SymKey, SymbolId,
};
use mortar_diag::{Diagnostic, Key};
use mortar_elf::SectionIndex;

/// The phase ordinal of the scan (`spec/model.md` §8).
pub const PHASE: u16 = 70;

/// A key only dropped copies of a COMDAT group define
/// (`spec/resolve.md` §6.3 (b)): resolution's `ComdatMismatch`, which the
/// caller hands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Mismatch {
    pub key: SymbolId,
    /// The group of the first dropped definition.
    pub group: ComdatId,
    /// The file whose copy of `group` is kept.
    pub keeper: FileId,
    /// The first dropped definition.
    pub dropped: SymEntry,
}

/// A mapping mark of a section: its symbol index, `st_value`, and whether
/// it opens code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mark {
    pub index: u32,
    pub value: u64,
    pub code: bool,
}

/// The instruction proof of an architecture (`spec/arch.md` §3.6 rule 1):
/// which local symbols mark code or data, and the runs a section's marks
/// make. AArch64's is `mortar_a64::mapping` (a64.md §3.4).
pub trait CodeProof: Sync {
    /// `Some(true)` for a code mark, `Some(false)` for a data mark.
    fn mark(&self, name: &[u8], st_info: u8) -> Option<bool>;
    /// The runs of a section of `len` bytes from its marks, in section
    /// offsets, appended to `out`. `exec`: the section has
    /// `SHF_EXECINSTR`.
    fn runs(&self, marks: &mut [Mark], len: u64, exec: bool, out: &mut Vec<ProofRun>);
}

/// No proof: no word is a proved instruction.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoProof;

impl CodeProof for NoProof {
    fn mark(&self, _: &[u8], _: u8) -> Option<bool> {
        None
    }

    fn runs(&self, _: &mut [Mark], _: u64, _: bool, _: &mut Vec<ProofRun>) {}
}

/// What the scan reads (§4.1, I-13).
#[derive(Debug, Clone, Copy)]
pub struct ScanInputs<'a, 's> {
    pub inputs: &'a Inputs<'s>,
    pub resolution: &'a Resolution,
    pub liveness: &'a Liveness,
    /// For the conditional linker-defined symbols it withdraws
    /// (`Assignment::def`); `None` reads `Resolution.def` as it is.
    pub assignment: Option<&'a Assignment<'a>>,
    /// Resolution's keys only dropped COMDAT copies define.
    pub comdat_mismatch: &'a [Mismatch],
    /// Resolution's `.gnu.warning.<key>` sections of live files.
    pub gnu_warnings: &'a [(SymbolId, FileId, SectionIndex)],
    /// Resolution's keys that only a closure file defines, with that file
    /// (`spec/dyn.md` D-12): their `undefined-symbol` gets the note
    /// `defined in <file>, which is not on the command line`. Sorted by
    /// key; empty in a static link.
    pub closure_only: &'a [(SymbolId, FileId)],
    /// The records of `.eh_frame` and the LSDA starts of
    /// `.gcc_except_table` (WP-177, `spec/resolve.md` §6.3): a reference
    /// there to a dead target, or through a demoted mention nothing live
    /// defines, is judged by the record that owns its field. `None`: the
    /// section's name decides, as before WP-177 (a dead target is no
    /// event there; a `comdat-mismatch` key is one).
    pub eh: Option<&'a dyn EhOwners>,
}

/// The owners of `.eh_frame` and `.gcc_except_table` fields
/// (`spec/resolve.md` §6.3, WP-177), asked only for a reference to a dead
/// target or through a demoted mention nothing live defines.
pub trait EhOwners: Sync + std::fmt::Debug {
    /// The record of `.eh_frame` section `section` whose bytes hold
    /// `offset`.
    fn frame(&self, section: SectionId, offset: u64) -> EhOwner;
    /// The FDE that owns the field at `offset` of the `.gcc_except_table`
    /// section `table`: of every parsed FDE whose LSDA pointer names
    /// `table`, whatever its file, the one whose LSDA start is the
    /// greatest `<= offset`.
    fn lsda(&self, table: SectionId, offset: u64) -> EhOwner;
}

/// The options the scan depends on.
#[derive(Clone, Copy)]
pub struct ScanOptions<'a> {
    pub output: OutputKind,
    /// The output has a dynamic symbol table (`Exec`/`Pie` with
    /// `.dynamic`; `false` in every static link, static PIE included): an
    /// undefined weak key may be `WeakDyn`, per key ([`weak::weak_classes`],
    /// `spec/dyn.md` D-25), and a record in a read-only place is
    /// `text-relocation` (D-47).
    pub dynamic: bool,
    /// The section mapping's writability (D-25's writable place); `None`
    /// in a static link, which has no weak-dynamic key.
    pub sections: Option<&'a dyn weak::SectionMap>,
    pub proof: &'a dyn CodeProof,
}

impl std::fmt::Debug for ScanOptions<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScanOptions")
            .field("output", &self.output)
            .field("dynamic", &self.dynamic)
            .finish_non_exhaustive()
    }
}

/// The record index (I-29) and a section's order: `mortar_core` types
/// (so that the image need not depend on the scan), re-exported here.
pub use mortar_core::records::{Order, RecIndex};

/// What the scan did, for `--time-phases` and the benchmarks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Relocated sections scanned.
    pub sections: u32,
    /// Scan units (sections and blocks).
    pub units: u32,
    pub records: u64,
    pub events: u64,
    /// Grouped diagnostics.
    pub groups: u32,
    /// Calls into `Pool` that may run on several threads.
    pub par_calls: u32,
    /// Sections whose blocks' overlap decisions were re-walked together.
    pub rewalked: u32,
}

/// A record of a live `SHF_ALLOC | SHF_EXECINSTR` section for which
/// `need::branch_candidate` is `Some` (`spec/arch.md` §6 step 1), found by
/// the scan's walk so that layout need not walk the code's relocations
/// again (WP-032i). The site is `site.site(flags)` with the key's final
/// `Demands` flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchRec {
    pub section: SectionId,
    pub offset: u64,
    /// The record's index in its relocation section.
    pub record: u32,
    pub site: mortar_arch::need::BranchCandidate,
}

/// What the scan produces (I-15).
#[derive(Debug, Clone, Default)]
pub struct Scanned {
    pub demands: Demands,
    pub order: RecIndex,
    /// The branch candidates of every live code section, one list per
    /// scan unit that has any, in unit order (`(FileId, section index)`,
    /// then record order: concatenated, `SectionId` order; kept apart so
    /// that the scan does not copy them); `need::branch_candidate`
    /// with the scan's output kind and `RelocAt.leads` set (§3.5: the bit
    /// only gates a test that fails without a lead).
    pub branches: Vec<Vec<BranchRec>>,
    /// Every diagnostic of the phase, in unit order and then by group;
    /// the collector orders them by key.
    pub diagnostics: Vec<(Key, Diagnostic)>,
    pub stats: Stats,
}

/// The sort of `Pool` runs sequentially up to this many elements
/// (`mortar_core::par`'s `SORT_RUN`); only above it does a sort count as
/// a parallel call in [`Stats`].
const SORT_RUN: usize = 8192;

/// The scan (§4): `Demands`, the record index and the phase's
/// diagnostics. Cost: one walk of the live relocations (I-16); one
/// parallel call, plus a parallel sort when the link has more than 8192
/// events or local demands, plus one for unsorted sections' permutations.
#[must_use]
pub fn scan(
    arch: &dyn Arch,
    t: &ScanInputs<'_, '_>,
    opts: &ScanOptions<'_>,
    pool: &Pool,
) -> Scanned {
    scan_in(arch, t, opts, pool, false).0
}

/// [`scan`], and each file's own demand (WP-239 S4-5, `spec/incr.md`
/// IN-18e `KEY_REFS.demand`): per file with any, the or of the flags its
/// live units raised on each key (globals and locals), sorted by key.
/// The same walk as [`scan`]'s, with the recording sink: what an
/// incremental-mode full link's capture stores.
#[must_use]
pub fn scan_recording(
    arch: &dyn Arch,
    t: &ScanInputs<'_, '_>,
    opts: &ScanOptions<'_>,
    pool: &Pool,
) -> (Scanned, Vec<mortar_core::incr::FileDemands>) {
    scan_in(arch, t, opts, pool, true)
}

#[allow(clippy::too_many_lines)] // the steps, timed one by one
fn scan_in(
    arch: &dyn Arch,
    t: &ScanInputs<'_, '_>,
    opts: &ScanOptions<'_>,
    pool: &Pool,
    record: bool,
) -> (Scanned, Vec<mortar_core::incr::FileDemands>) {
    let mut steps = mortar_core::wp032b_steps::Steps::new("scan");
    let mut view = view::View::new(arch, t, *opts);
    steps.step("view", 0, "");
    // `spec/dyn.md` D-25: after liveness, before the demands of the walk.
    let weak = weak::classes(&view, pool);
    steps.step("weak", weak.weak_dyn.count().into(), "key");
    let mut diagnostics = weak.refused;
    view.whole.weak_dyn = weak.weak_dyn.clone();
    view.weak_refused = weak.refused_keys;
    let (units, deferred) = driver::units(&view, &mut diagnostics);
    steps.step("units", units.len() as u64, "unit");
    let symbols = t.inputs.symbols.len() as usize;
    let global: Vec<AtomicU16> = (0..symbols).map(|_| AtomicU16::new(0)).collect();
    let mut stats = Stats { units: units.len() as u32, par_calls: 1, ..Stats::default() };
    let mut results: Vec<driver::UnitOut> = if record {
        pool.map(units.len(), |i| {
            let g = driver::Recording { cells: Some(&global), log: Vec::new() };
            driver::run_unit(&view, &units[i], g)
        })
    } else {
        pool.map(units.len(), |i| driver::run_unit(&view, &units[i], driver::Shared(&global)))
    };
    if steps.on() {
        let n: u64 = results.iter().map(|o| o.records).sum();
        let nonalloc: u64 = results.iter().map(|o| o.nonalloc).sum();
        steps.step("walk", n, "record");
        steps.step("walk.nonalloc", nonalloc, "record");
    }

    // Per section: counts, order, overlaps across blocks.
    let mut dyn_relocs: Vec<(SectionId, DynCounts)> = Vec::new();
    let mut unsorted: Vec<usize> = Vec::new();
    let mut leads: Vec<SectionId> = Vec::new();
    let mut late: Vec<(Key, Diagnostic)> = Vec::new();
    let mut at = 0;
    while at < units.len() {
        let parts = units[at].parts.max(1) as usize;
        let end = (at + parts).min(units.len());
        let (us, os) = (&units[at..end], &results[at..end]);
        stats.sections += 1;
        let mut counts = [0u32; 4];
        for o in os {
            for (c, n) in counts.iter_mut().zip(o.counts.0) {
                *c = c.saturating_add(n);
            }
            stats.records += o.records;
        }
        if counts.iter().any(|&n| n != 0) {
            dyn_relocs.push((us[0].section, DynCounts(counts)));
        }
        let sorted = os.iter().all(|o| o.sorted)
            && os
                .windows(2)
                .all(|w| w[0].records == 0 || w[1].records == 0 || w[0].last <= w[1].first);
        if !sorted {
            unsorted.push(at);
        }
        if os.iter().any(|o| o.leads) {
            leads.push(us[0].section);
        }
        if parts > 1 {
            let (records, rewalked) = block_overlaps(&view, us, os, &mut late);
            stats.rewalked += u32::from(rewalked);
            for (b, r) in records {
                late.push(driver::overlap_diag(&view, &us[b], r));
            }
        }
        at = end;
    }

    steps.step("sections", u64::from(stats.sections), "section");
    let mut locals: Vec<(LocalSym, DemandFlags)> = Vec::new();
    let mut events = Vec::with_capacity(results.len());
    let mut output = mortar_core::demands::GlobalFlags::empty();
    let mut branches: Vec<Vec<BranchRec>> = Vec::new();
    let mut copy_refs: Vec<(SymbolId, Key, usize, u32)> = Vec::new();
    let mut per_file: Vec<(FileId, SymKey, DemandFlags)> = Vec::new();
    for (k, o) in results.iter_mut().enumerate() {
        let u = &units[k];
        if record {
            per_file.extend(o.demands.drain(..).map(|(id, f)| (u.file, SymKey::global(id), f)));
            per_file.extend(o.locals.iter().map(|&(l, f)| (u.file, SymKey::local(l), f)));
        }
        copy_refs.extend(
            o.copy_refs.drain(..).map(|(id, r)| (id, driver::key_at(u.file, u.index, r), k, r)),
        );
        diagnostics.append(&mut o.diags);
        locals.append(&mut o.locals);
        if !o.branches.is_empty() {
            branches.push(std::mem::take(&mut o.branches));
        }
        events.push(std::mem::take(&mut o.events));
        stats.events += o.n_events;
        output |= o.output;
    }
    diagnostics.append(&mut late);
    drop(results);
    copies::refusals(&view, &units, copy_refs, &mut diagnostics);
    steps.step("copies", 0, "");

    stats.par_calls += u32::from(locals.len() > SORT_RUN);
    pool.unit_cost(10).sort_by_key(&mut locals, |l| l.0);
    let mut local: Vec<(LocalSym, DemandFlags)> = Vec::with_capacity(locals.len());
    for (s, f) in locals {
        match local.last_mut() {
            Some(last) if last.0 == s => last.1 |= f,
            _ if f.is_empty() => {}
            _ => local.push((s, f)),
        }
    }

    steps.step("locals", local.len() as u64, "local");
    let n_groups: usize = events.iter().map(Vec::len).sum();
    stats.par_calls += u32::from(n_groups > SORT_RUN);
    let groups = events::merge(events, pool);
    stats.groups = groups.len() as u32;
    diagnostics.extend(groups.iter().map(|(g, a)| report::group(&view, g, a)));

    steps.step("events", stats.events, "event");
    let permuted: Vec<(SectionId, Box<[u32]>)> = if unsorted.is_empty() {
        Vec::new()
    } else {
        stats.par_calls += 1;
        permutations(&view, &units, &unsorted, pool, &mut diagnostics)
    };

    let global = global
        .into_iter()
        .map(|a| DemandFlags::from_bits(a.into_inner()).unwrap_or_default())
        .collect();
    steps.step("finish", unsorted.len() as u64, "unsorted");
    let demands = Demands {
        global: Column::from_vec(global).unwrap_or_default(),
        local: local.into_boxed_slice(),
        output,
        dyn_relocs: dyn_relocs.into_boxed_slice(),
        // `spec/dyn.md` D-25: `weak::classes` decided it; none in a
        // static link.
        weak_dyn: weak.weak_dyn,
    };
    let order = RecIndex::new(permuted, leads).with_deferred(deferred);
    (Scanned { demands, order, branches, diagnostics, stats }, file_demands(per_file))
}

/// The per-file demand rows (or-ed per (file, key), empty flags dropped)
/// as `FileDemands`, by file, each sorted by key.
fn file_demands(
    mut rows: Vec<(FileId, SymKey, DemandFlags)>,
) -> Vec<mortar_core::incr::FileDemands> {
    use mortar_core::incr::{FileDemands, SlotDemand};
    rows.sort_unstable_by_key(|r| (r.0, r.1));
    let mut out: Vec<FileDemands> = Vec::new();
    for (file, key, flags) in rows {
        if flags.is_empty() {
            continue;
        }
        if out.last().is_none_or(|f| f.file != file) {
            out.push(FileDemands { file, demands: Vec::new() });
        }
        let Some(f) = out.last_mut() else { continue };
        match f.demands.last_mut() {
            Some(last) if last.key == key => last.flags |= flags,
            _ => f.demands.push(SlotDemand { key, flags }),
        }
    }
    out
}

/// The diagnostics of the sections whose checks are fill's (I-14,
/// `RecIndex::deferred`): each section walked whole as the scan walks a
/// section (§4.1 to §4.4), its faults and events keyed at fill's phase
/// (`spec/image.md` I-14: after every scan diagnostic). Empty when they
/// have none.
fn deferred_report(
    view: &view::View<'_, '_>,
    sections: &[SectionId],
    pool: &Pool,
) -> Vec<(Key, Diagnostic)> {
    let r = view.whole.resolution;
    let mut diagnostics = Vec::new();
    let mut units = Vec::new();
    let mut bufs = view::Bufs::default();
    for &sid in sections {
        let Some((file, index)) = r.sh_index(sid) else { continue };
        let Some(row) = r.sections.get(sid) else { continue };
        let rel = row.reloc.0;
        if rel == 0 {
            continue;
        }
        match driver::Sec::open(view, file, sid, index.0, rel, &mut bufs) {
            Ok(sec) => {
                let hi = u32::try_from(sec.recs.records.len()).unwrap_or(u32::MAX);
                units.push(driver::Unit {
                    file,
                    section: sid,
                    index: index.0,
                    rel,
                    lo: 0,
                    hi,
                    parts: 1,
                    types_only: false,
                });
            }
            Err(d) => diagnostics.push(d),
        }
    }
    let symbols = view.whole.inputs.symbols.len() as usize;
    let global: Vec<AtomicU16> = (0..symbols).map(|_| AtomicU16::new(0)).collect();
    let results: Vec<driver::UnitOut> =
        pool.map(units.len(), |i| driver::run_unit(view, &units[i], driver::Shared(&global)));
    let mut events = Vec::with_capacity(results.len());
    for mut o in results {
        diagnostics.append(&mut o.diags);
        events.push(o.events);
    }
    let groups = events::merge(events, pool);
    diagnostics.extend(groups.iter().map(|(g, a)| report::group(view, g, a)));
    for (k, _) in &mut diagnostics {
        k.phase = DEFERRED_PHASE;
    }
    diagnostics
}

/// The phase the diagnostics of the sections fill checks are keyed at
/// (`spec/image.md` I-14): fill's.
pub const DEFERRED_PHASE: u16 = 100;

/// The permutations of the unsorted sections (`unsorted`: their first
/// units), one parallel call; a section that cannot be read again is its
/// read's diagnostic.
fn permutations(
    view: &view::View<'_, '_>,
    units: &[driver::Unit],
    unsorted: &[usize],
    pool: &Pool,
    diagnostics: &mut Vec<(Key, Diagnostic)>,
) -> Vec<(SectionId, Box<[u32]>)> {
    let all = pool.map(unsorted.len(), |k| {
        let u = &units[unsorted[k]];
        (u.section, driver::permutation(view, u))
    });
    let mut v = Vec::with_capacity(all.len());
    for (s, p) in all {
        match p {
            Ok(p) => v.push((s, p)),
            Err(d) => diagnostics.push(d),
        }
    }
    v
}

/// The overlapping records of a section of several blocks, as `(block,
/// record)` in block order: each block's own result, except for the
/// blocks whose windows' extents intersect, which are decided together
/// (`driver::rewalk`). Whether any were re-walked.
fn block_overlaps(
    view: &view::View<'_, '_>,
    us: &[driver::Unit],
    os: &[driver::UnitOut],
    diags: &mut Vec<(Key, Diagnostic)>,
) -> (Vec<(usize, u32)>, bool) {
    let mut ext: Vec<(u64, u64, usize)> =
        os.iter().enumerate().filter_map(|(b, o)| o.extent.map(|(s, e)| (s, e, b))).collect();
    ext.sort_unstable();
    // Components of intersecting extents.
    let mut comp = vec![usize::MAX; os.len()];
    let mut c = 0;
    let mut end = 0u64;
    for (k, &(s, e, b)) in ext.iter().enumerate() {
        if k == 0 || s >= end {
            c = b;
            end = e;
        } else {
            end = end.max(e);
        }
        comp[b] = c;
    }
    let mut out = Vec::new();
    let mut rewalked = false;
    let mut done = vec![false; os.len()];
    for b in 0..os.len() {
        if done[b] {
            continue;
        }
        let members: Vec<usize> =
            (b..os.len()).filter(|&m| comp[m] != usize::MAX && comp[m] == comp[b]).collect();
        if members.len() > 1 {
            rewalked = true;
            let blocks: Vec<(&driver::Unit, &driver::UnitOut)> =
                members.iter().map(|&m| (&us[m], &os[m])).collect();
            let records = driver::rewalk(view, &blocks).unwrap_or_else(|d| {
                diags.push(d);
                Vec::new()
            });
            for r in records {
                let m =
                    members.iter().copied().find(|&m| us[m].lo <= r && r < us[m].hi).unwrap_or(b);
                out.push((m, r));
            }
            for m in members {
                done[m] = true;
            }
        } else {
            out.extend(os[b].overlaps.iter().map(|&r| (b, r)));
            done[b] = true;
        }
    }
    out.sort_unstable();
    (out, rewalked)
}
