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
//!    I-29;
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

mod driver;
mod report;
mod view;
mod walk;

pub use walk::{Visit, Walker};

use std::sync::atomic::AtomicU16;

use mortar_arch::{Arch, OutputKind, ProofRun};
use mortar_core::assignment::Assignment;
use mortar_core::column::Column;
use mortar_core::demands::{DemandFlags, DynCounts};
use mortar_core::inputs::Inputs;
use mortar_core::par::Pool;
use mortar_core::resolution::Resolution;
use mortar_core::{ComdatId, Demands, FileId, Liveness, LocalSym, SectionId, SymEntry, SymbolId};
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
}

/// The options the scan depends on.
#[derive(Clone, Copy)]
pub struct ScanOptions<'a> {
    pub output: OutputKind,
    /// The output has a dynamic symbol table: an undefined weak key is
    /// `WeakDyn` rather than `Weak0` (M2).
    pub dynamic: bool,
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

/// The order of a relocation section's records by `r_offset` (I-29).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order<'a> {
    /// `r_offset` never decreases.
    Sorted,
    /// The record indexes stably sorted by `r_offset`.
    Permuted(&'a [u32]),
}

/// The record index (I-29): per relocated section, whether its records
/// are in `r_offset` order, and the permutation of those that are not.
/// Kept sparse: a section not listed is `Sorted`. (The store is resident
/// in M1, so `REC_SAMPLE` samples are not kept.)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecIndex {
    permuted: Box<[(SectionId, Box<[u32]>)]>,
    /// The sections with a record whose type is in `Facts.seq_leads`,
    /// sorted (`spec/arch.md` §3.5: fill's `RelocAt.leads`).
    leads: Box<[SectionId]>,
}

impl RecIndex {
    /// The order of the records of `section`'s relocation section.
    #[must_use]
    pub fn get(&self, section: SectionId) -> Order<'_> {
        match self.permuted.binary_search_by_key(&section, |p| p.0) {
            Ok(at) => Order::Permuted(&self.permuted[at].1),
            Err(_) => Order::Sorted,
        }
    }

    /// `RelocAt.leads` for `section`'s records (`spec/arch.md` §3.5):
    /// whether one has a type in `Facts.seq_leads`. Exact for every
    /// section the scan read: the live ones, which are all fill reads.
    #[must_use]
    pub fn leads(&self, section: SectionId) -> bool {
        self.leads.binary_search(&section).is_ok()
    }

    /// The sections whose records are not sorted, in id order.
    pub fn unsorted(&self) -> impl ExactSizeIterator<Item = SectionId> + '_ {
        self.permuted.iter().map(|p| p.0)
    }
}

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

/// What the scan produces (I-15).
#[derive(Debug, Clone, Default)]
pub struct Scanned {
    pub demands: Demands,
    pub order: RecIndex,
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
    let view = view::View::new(arch, t, *opts);
    let mut diagnostics = Vec::new();
    let units = driver::units(&view, &mut diagnostics);
    let symbols = t.inputs.symbols.len() as usize;
    let global: Vec<AtomicU16> = (0..symbols).map(|_| AtomicU16::new(0)).collect();
    let mut stats = Stats { units: units.len() as u32, par_calls: 1, ..Stats::default() };
    let mut results: Vec<driver::UnitOut> =
        pool.map(units.len(), |i| driver::run_unit(&view, &units[i], &global));

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

    let mut locals: Vec<(LocalSym, DemandFlags)> = Vec::new();
    let mut events = Vec::with_capacity(results.len());
    let mut output = mortar_core::demands::GlobalFlags::empty();
    for o in &mut results {
        diagnostics.append(&mut o.diags);
        locals.append(&mut o.locals);
        events.push(std::mem::take(&mut o.events));
        stats.events += o.n_events;
        output |= o.output;
    }
    diagnostics.append(&mut late);
    drop(results);

    stats.par_calls += u32::from(locals.len() > SORT_RUN);
    pool.sort_by_key(&mut locals, |l| l.0);
    let mut local: Vec<(LocalSym, DemandFlags)> = Vec::with_capacity(locals.len());
    for (s, f) in locals {
        match local.last_mut() {
            Some(last) if last.0 == s => last.1 |= f,
            _ if f.is_empty() => {}
            _ => local.push((s, f)),
        }
    }

    let n_groups: usize = events.iter().map(Vec::len).sum();
    stats.par_calls += u32::from(n_groups > SORT_RUN);
    let groups = events::merge(events, pool);
    stats.groups = groups.len() as u32;
    diagnostics.extend(groups.iter().map(|(g, a)| report::group(&view, g, a)));

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
    let demands = Demands {
        global: Column::from_vec(global).unwrap_or_default(),
        local: local.into_boxed_slice(),
        output,
        dyn_relocs: dyn_relocs.into_boxed_slice(),
    };
    Scanned {
        demands,
        order: RecIndex { permuted: permuted.into_boxed_slice(), leads: leads.into_boxed_slice() },
        diagnostics,
        stats,
    }
}

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
