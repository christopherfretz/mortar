//! Fill (§5): the jobs of a plan in parallel, each into its worker's
//! reused buffer, hashed (§8.3) and handed to a [`Sink`] window by window
//! (I-33, I-63).

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use mortar_arch::{Arch, FillTables, OutputKind, TlsBlock};
use mortar_core::par::Pool;
use mortar_core::records::RecIndex;
use mortar_core::{Split, SymKey};
use mortar_diag::{Diagnostic, Key, Kind, Place};
use mortar_digest::{Leaf, TreeStyle};
use mortar_merge::order::Order;
use mortar_synth::{EhPlan, EhSource};

use crate::plan::{Plan, Unit};
use crate::reloc::{self, Scratch};
use crate::source::{self, RecordKeys, Source};
use crate::synth::{SynthChunk, SynthCx, SynthError};
use crate::{PHASE_IMAGE, PHASE_OUTPUT};
use crate::{digest, eh, merged};

/// The tables fill reads (`FillTables`), plus what §6.1 needs and
/// `SymValue` does not carry.
///
/// `FillTables` (WP-023b) does not carry the target's section flags, so
/// the image asks for them here; [`crate::link`]'s tables answer from the
/// resolution and the inputs' section headers.
pub trait Tables: FillTables {
    /// Whether the key's definition is in an `SHF_ALLOC` section (I-112).
    fn target_alloc(&self, key: SymKey) -> bool;

    /// Whether input section `section` has `SHF_WRITE` (`spec/dyn.md`
    /// D-46: which record a relocation of it makes, [`crate::dynrel`]).
    /// `None` (the default): the tables cannot say, and the fill leaves
    /// every place as `apply` wrote it, as in a static output.
    fn section_writable(&self, section: mortar_core::SectionId) -> Option<bool> {
        let _ = section;
        None
    }
    /// `S + A` of a reference with addend `addend` to `key` when that
    /// value is not the symbol's plus `A`: a section symbol of a split
    /// `Merge` section (`spec/synth.md` S-17, S-18: the piece holding
    /// byte `st_value + A`) and any key defined in an input `.eh_frame`
    /// (S-50, which follows the target offset). The output address, or
    /// the diagnostic (S-18; S-50 rules 4 and 5). `Ok(None)`: the piece
    /// was collected (`--gc-sections`, `spec/image.md` I-7; a debug
    /// reference to it is the tombstone, I-47). `None` otherwise: the
    /// arch computes `S + A` from [`FillTables::sym`].
    fn split_target(&self, key: SymKey, addend: i64) -> Option<Result<Option<u64>, Diagnostic>> {
        let _ = (key, addend);
        None
    }

    /// [`FillTables::sym`] of a record's key, and whether
    /// [`Tables::split_target`] may give its value. When the flag is true
    /// the value `s` need not be computed (the caller asks `split_target`,
    /// and falls back to `sym` when that is `None`); when it is false,
    /// `split_target` is `None` and the value is `sym(key)` exactly. One
    /// read of the key's symbol where `sym` and `split_target` would make
    /// several (WP-032c).
    fn record_sym(&self, key: SymKey) -> (mortar_arch::SymValue, bool) {
        (self.sym(key), true)
    }

    /// What the scan knows of `key` (`spec/arch.md` §3.4 `SymFacts`) for
    /// a record whose symbol is `mention` (an undefined key takes its
    /// type): fill's checks of a section the scan left to it
    /// (`RecIndex::deferred`, `spec/image.md` I-14). `None`: the tables
    /// cannot say, and such a section's records are faults.
    fn scan_facts(
        &self,
        key: SymKey,
        mention: Option<mortar_core::SymEntry>,
    ) -> Option<mortar_arch::SymFacts> {
        let _ = (key, mention);
        None
    }

    /// [`Tables::record_sym`] and [`Tables::scan_facts`] of one deferred
    /// record (I-14) together, so that tables which read the key's symbol
    /// for both can read it once (WP-032f).
    fn record_sym_at(
        &self,
        key: SymKey,
        mention: Option<mortar_core::SymEntry>,
    ) -> (mortar_arch::SymValue, bool, Option<mortar_arch::SymFacts>) {
        let (value, split) = self.record_sym(key);
        (value, split, self.scan_facts(key, mention))
    }

    /// What [`Tables::split_target`] reads of `key` before it looks at
    /// the addend, read once for many records (WP-032f); `None`: ask
    /// `split_target`.
    fn split_place(&self, key: SymKey) -> Option<SplitPlace> {
        let _ = key;
        None
    }

    /// [`Tables::split_target`] of `key`, whose place is `place`
    /// (`split_place(key)`).
    fn split_at(
        &self,
        place: &SplitPlace,
        key: SymKey,
        addend: i64,
    ) -> Option<Result<Option<u64>, Diagnostic>> {
        let _ = place;
        self.split_target(key, addend)
    }
}

/// Where a split target is defined ([`Tables::split_place`]): the input
/// section, whether it is a `Merge` section (else an `.eh_frame`), the
/// symbol's `st_value` and whether it is the section's symbol, the
/// tables' own row of the section (`u32::MAX`: none) and, for a `Merge`
/// section, the value of offset 0 of its output section's merged chunk
/// when the tables know it (every fragment of the section's pieces is in
/// that chunk: S-13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitPlace {
    pub section: mortar_core::SectionId,
    pub merge: bool,
    pub value: u64,
    pub section_sym: bool,
    pub row: u32,
    pub chunk_base: Option<u64>,
}

/// What merged chunks are written from (I-38): phase 50's fragment order
/// and phase 60's fragments.
#[derive(Debug, Clone, Copy)]
pub struct MergedFrags<'a> {
    pub order: &'a Order,
    pub split: &'a Split,
}

/// What the input `.eh_frame` relocations are applied through (S-52):
/// the plan, its source and the output `.eh_frame`'s address.
#[derive(Clone, Copy)]
pub struct EhFill<'a> {
    pub plan: &'a EhPlan,
    pub src: &'a dyn EhSource,
    pub addr: u64,
}

impl std::fmt::Debug for EhFill<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EhFill").field("addr", &self.addr).finish_non_exhaustive()
    }
}

/// Where windows go (I-63): the output file, or memory in tests.
pub trait Sink: Sync {
    /// Writes `bytes` at file offset `offset`, whole.
    fn write_at(&self, offset: u64, bytes: &[u8]) -> io::Result<()>;
}

impl Sink for std::fs::File {
    fn write_at(&self, offset: u64, bytes: &[u8]) -> io::Result<()> {
        std::os::unix::fs::FileExt::write_all_at(self, bytes, offset)
    }
}

/// Everything fill reads.
#[derive(Clone, Copy)]
pub struct Image<'a> {
    pub plan: &'a Plan,
    pub source: &'a dyn Source,
    pub arch: &'a dyn Arch,
    pub tables: &'a dyn Tables,
    /// The scan's record index (I-29): the order of each relocated
    /// section's records and its `leads` bit.
    pub records: &'a RecIndex,
    /// How records read from the store name their keys.
    pub keys: &'a dyn RecordKeys,
    pub output: OutputKind,
    pub got_base: Option<u64>,
    pub tls: Option<TlsBlock>,
    /// The chunks [`crate::plan::SynthUnit::chunk`] indexes.
    pub chunks: &'a [&'a (dyn SynthChunk + Sync)],
    /// What the chunks' `write` reads (§13).
    pub synth: SynthCx<'a>,
    /// For [`crate::plan::Unit::Merged`] rows.
    pub merged: Option<MergedFrags<'a>>,
    /// For the `.eh_frame` chunk's relocations.
    pub eh: Option<EhFill<'a>>,
    /// The build id's style; `None` when no leaves are computed (I-73).
    /// Leaves are windows, so it needs `plan.window == IMAGE_BLOCK` (I-75).
    pub build_id: Option<TreeStyle>,
    /// Keep the leaves (`spec/incr.md` IN-21: an incremental-mode link
    /// stores them in its state even without a build id), hashed with this
    /// function; it is the build id's tree function when one is computed.
    /// `None`: leaves are kept for nothing (a plain link).
    pub leaf_fn: Option<mortar_digest::TreeFn>,
    /// Words written over the rows' bytes, by file offset, sorted (the
    /// erratum fixes, `crate::link::Link::fixups`).
    pub fixups: &'a [(u64, [u8; 4])],
}

impl std::fmt::Debug for Image<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image").field("file_size", &self.plan.file_size).finish_non_exhaustive()
    }
}

/// What a link's fill produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filled {
    /// The build id, written at the sealed range, when one was computed.
    pub build_id: Option<Vec<u8>>,
    /// The leaves in window order, when `Image::leaf_fn` asked for them.
    pub leaves: Option<Vec<Leaf>>,
}

/// The diagnostics of a failed fill, keyed (I-43, I-64, I-68).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    pub diagnostics: Vec<(Key, Diagnostic)>,
}

/// The result of one job.
#[derive(Debug, Default)]
struct JobOut {
    errors: Vec<(Key, Diagnostic)>,
    leaves: Vec<(u64, Leaf)>,
    /// The lowest window offset whose write failed, with the error.
    write: Option<(u64, io::Error)>,
    fatal: Option<(Key, Diagnostic)>,
    /// Deferred sections with a fault (I-14).
    flagged: Vec<mortar_core::SectionId>,
}

/// Fill's sub-step counters under `MORTAR_STEPS` (WP-032f; nothing they
/// count reaches the output, the map or a diagnostic): CPU time summed
/// over the workers in the jobs' fills, in copies from the store (inputs
/// that are not resident) and in the sink's writes, with their bytes.
#[derive(Debug)]
pub(crate) struct FillSteps {
    fill_ns: AtomicU64,
    read_ns: AtomicU64,
    read_bytes: AtomicU64,
    reads: AtomicU64,
    write_ns: AtomicU64,
    write_bytes: AtomicU64,
}

pub(crate) static FILL_STEPS: FillSteps = FillSteps {
    fill_ns: AtomicU64::new(0),
    read_ns: AtomicU64::new(0),
    read_bytes: AtomicU64::new(0),
    reads: AtomicU64::new(0),
    write_ns: AtomicU64::new(0),
    write_bytes: AtomicU64::new(0),
};

impl FillSteps {
    pub(crate) fn on() -> bool {
        mortar_core::wp032b_steps::enabled()
    }

    fn since(t: Instant) -> u64 {
        u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    pub(crate) fn add_read(&self, t: Instant, bytes: usize) {
        self.read_ns.fetch_add(Self::since(t), Ordering::Relaxed);
        self.read_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
        self.reads.fetch_add(1, Ordering::Relaxed);
    }

    /// Prints the counters and clears them.
    #[allow(clippy::cast_precision_loss)] // printed durations
    fn print(&self) {
        let take = |a: &AtomicU64| a.swap(0, Ordering::Relaxed);
        let ms = |ns: u64| ns as f64 / 1e6;
        let (fill, read, rb, reads, write, wb) = (
            take(&self.fill_ns),
            take(&self.read_ns),
            take(&self.read_bytes),
            take(&self.reads),
            take(&self.write_ns),
            take(&self.write_bytes),
        );
        eprintln!("mortar: steps: image fill jobs (cpu): {:.2} ms", ms(fill));
        eprintln!(
            "mortar: steps: image fill reads (cpu): {:.2} ms, {rb} byte, {reads} read",
            ms(read)
        );
        eprintln!("mortar: steps: image fill writes (cpu): {:.2} ms, {wb} byte", ms(write));
    }
}

/// A worker's state: its window buffer (I-32) and relocation scratch.
#[derive(Debug, Default)]
struct Worker {
    buf: Vec<u8>,
    scratch: Scratch,
}

/// Fills every job of `img.plan` and writes it to `sink` (I-31..I-36,
/// I-63, I-64); then, if a build id is computed, writes it at the sealed
/// range (I-73). `output_path` names the output in a write error (R-7).
///
/// Errors are concatenated in job order; a write error is the one of the
/// lowest failing window (I-64), keyed `(105, 0, 0)` (I-68).
///
/// A section whose checks are fill's (I-14) with a fault is an
/// `internal-error` here: [`write_checked`] hands such sections to the
/// caller, who reports them.
pub fn write(
    img: &Image<'_>,
    sink: &dyn Sink,
    output_path: &str,
    pool: &Pool,
) -> Result<Filled, Failed> {
    let (r, checked) = write_checked(img, sink, output_path, pool);
    if checked.flagged.is_empty() {
        return r;
    }
    let mut diagnostics = r.err().map(|f| f.diagnostics).unwrap_or_default();
    let what = format!(
        "fill: {} deferred section(s) have faults and no report (I-14)",
        checked.flagged.len()
    );
    diagnostics.push((Key::new(PHASE_IMAGE, 0, 0), internal(what)));
    Err(Failed { diagnostics })
}

/// What fill's checks of the sections the scan left to it found
/// (`RecIndex::deferred`, I-14).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checked {
    /// The sections with a fault, sorted: the link has failed, and the
    /// diagnostics are the scan's walk of them.
    pub flagged: Vec<mortar_core::SectionId>,
    /// Every job ran to its end: every clip was checked.
    pub complete: bool,
}

/// [`write`], with fill's checks of the deferred sections (I-14) handed
/// back: a fault fails the fill (nothing more is written) with no
/// diagnostic of its own.
pub fn write_checked(
    img: &Image<'_>,
    sink: &dyn Sink,
    output_path: &str,
    pool: &Pool,
) -> (Result<Filled, Failed>, Checked) {
    let plan = img.plan;
    if img.build_id.is_some() && plan.window != crate::IMAGE_BLOCK {
        let what = format!(
            "fill: a build id needs W = IMAGE_BLOCK, the plan has {:#x} (I-75)",
            plan.window
        );
        let d = vec![(Key::new(PHASE_IMAGE, 0, 0), internal(what))];
        return (Err(Failed { diagnostics: d }), Checked::default());
    }
    let mut outs: Vec<JobOut> = plan.jobs.iter().map(|_| JobOut::default()).collect();
    let stop = AtomicBool::new(false);
    pool.for_each_part_init(outs.iter_mut(), Worker::default, |w, j, out| {
        let job = plan.jobs[j].clone();
        run_job(img, w, job, sink, &stop, out);
    });
    if FillSteps::on() {
        FILL_STEPS.print();
    }
    let mut diagnostics = Vec::new();
    let mut write_err: Option<(u64, io::Error)> = None;
    let mut leaves = Vec::new();
    let mut checked = Checked { flagged: Vec::new(), complete: true };
    for o in outs {
        diagnostics.extend(o.errors);
        checked.complete &= o.fatal.is_none();
        diagnostics.extend(o.fatal);
        checked.flagged.extend(o.flagged);
        leaves.extend(o.leaves);
        if let Some((off, e)) = o.write
            && write_err.as_ref().is_none_or(|(w, _)| off < *w)
        {
            write_err = Some((off, e));
        }
    }
    checked.flagged.sort_unstable();
    checked.flagged.dedup();
    let mut build_id = None;
    if write_err.is_none()
        && diagnostics.is_empty()
        && checked.flagged.is_empty()
        && let Some(style) = img.build_id
    {
        let id = match digest::build_id(style, plan.file_size, &leaves) {
            Ok(id) => id,
            Err(e) => {
                let what = format!("fill: build id: {e}");
                let d = vec![(Key::new(PHASE_IMAGE, 0, 0), internal(what))];
                return (Err(Failed { diagnostics: d }), checked);
            }
        };
        if let Some(sealed) = &plan.sealed {
            let n = (sealed.end - sealed.start) as usize;
            let mut bytes = id.clone();
            bytes.resize(n, 0);
            if let Err(e) = sink.write_at(sealed.start, &bytes) {
                write_err = Some((sealed.start, e));
            }
        }
        build_id = Some(id);
    }
    if let Some((_, e)) = write_err {
        diagnostics.push(write_error(output_path, &e));
    }
    let r = if diagnostics.is_empty() && checked.flagged.is_empty() {
        let leaves = img.leaf_fn.map(|_| {
            leaves.sort_unstable_by_key(|l| l.0);
            leaves.into_iter().map(|l| l.1).collect()
        });
        Ok(Filled { build_id, leaves })
    } else {
        Err(Failed { diagnostics })
    };
    (r, checked)
}

/// `cannot-write-output` at `(105, 0, 0)` (I-64, I-68).
pub(crate) fn write_error(path: &str, e: &io::Error) -> (Key, Diagnostic) {
    let kind = Kind::CannotWriteOutput { path: path.to_owned(), reason: mortar_diag::os_reason(e) };
    (Key::new(PHASE_OUTPUT, 0, 0), Diagnostic::error(kind))
}

/// Fills the bytes of `[lo, hi)` of the file into `out`
/// (`out.len() == hi − lo`): every row overlapping the range (I-33 steps
/// 1-3). Relocation errors go to `errs`; the first failed read is returned.
/// A fault in a section deferred to fill (I-14) has no report here (that
/// is the scan's walk, `link::write`): it is an internal error in `errs`,
/// so that no fault passes unseen.
pub fn fill_range(
    img: &Image<'_>,
    lo: u64,
    out: &mut [u8],
    errs: &mut Vec<(Key, Diagnostic)>,
) -> Result<(), (Key, Diagnostic)> {
    let mut scratch = Scratch::default();
    let r = fill_into(img, &mut scratch, lo, out, errs);
    if !scratch.flagged.is_empty() {
        let what = format!(
            "fill: {} deferred section(s) have faults and no report (I-14)",
            scratch.flagged.len()
        );
        errs.push((Key::new(PHASE_IMAGE, 0, 0), internal(what)));
    }
    r
}

fn fill_into(
    img: &Image<'_>,
    scratch: &mut Scratch,
    lo: u64,
    out: &mut [u8],
    errs: &mut Vec<(Key, Diagnostic)>,
) -> Result<(), (Key, Diagnostic)> {
    let plan = img.plan;
    let hi = lo + out.len() as u64;
    let mut k = plan.first_row(lo);
    while let Some(row) = plan.rows.get(k) {
        if row.offset >= hi {
            break;
        }
        k += 1;
        let (s, e) = (row.offset.max(lo), row.end().min(hi));
        let dst = &mut out[(s - lo) as usize..(e - lo) as usize];
        // The clip in the unit's own offsets.
        let (a, b) = (s - row.offset, e - row.offset);
        match row.unit {
            Unit::Gap(p) => p.fill(s, dst),
            Unit::Bytes(i) => dst.copy_from_slice(&plan.bytes[i as usize][a as usize..b as usize]),
            Unit::Input(i) => {
                let u = &plan.inputs[i as usize];
                let range = u.src + a..u.src + b;
                let fail = |d| (Key::new(PHASE_IMAGE, u.file.0, 0), d);
                source::read(img.source, u.file, range, dst)
                    .map_err(|err| fail(source::copy_error(err, &plan.path(u.file))))?;
                if let Some(r) = u.relocs {
                    let rs = &plan.relocs[r as usize];
                    let clip = reloc::Clip::of_input(plan, u, rs, u.start + a, u.start + b);
                    reloc::relocate(img, scratch, &clip, None, dst, errs).map_err(fail)?;
                }
            }
            Unit::Merged(i) => {
                let mu = &plan.merged[i as usize];
                merged::fill(img, mu, a, b, dst)?;
            }
            Unit::Synth(i) => {
                let su = &plan.synth[i as usize];
                let Some(chunk) = img.chunks.get(su.chunk as usize) else {
                    let what = format!("fill: no synthetic chunk {} (model §4.4)", su.chunk);
                    errs.push((Key::new(PHASE_IMAGE, 0, u64::from(su.id.0)), internal(what)));
                    continue;
                };
                if let Err(err) = chunk.write(&img.synth, a..b, dst) {
                    errs.push(synth_error(img, su.id, su.name, err));
                }
                if su.eh {
                    eh::relocate(img, scratch, a, b, dst, errs)?;
                }
                if let Some(sealed) = chunk.sealed() {
                    // The descriptor's zeros (I-73): whatever `write` put there.
                    let (zs, ze) = (sealed.start.max(a), sealed.end.min(b));
                    if zs < ze {
                        dst[(zs - a) as usize..(ze - a) as usize].fill(0);
                    }
                }
            }
        }
    }
    // The fixed words over the rows (a word may straddle the range).
    let first = img.fixups.partition_point(|f| f.0 + 4 <= lo);
    for &(off, word) in img.fixups[first..].iter().take_while(|f| f.0 < hi) {
        for (i, b) in word.iter().enumerate() {
            let at = off + i as u64;
            if (lo..hi).contains(&at) {
                out[(at - lo) as usize] = *b;
            }
        }
    }
    Ok(())
}

fn internal(what: String) -> Diagnostic {
    Diagnostic::error(Kind::InternalError { what })
}

/// I-106: a synthetic chunk's error at `(100, 0, ChunkId)`; an
/// `internal-error` names the output section.
fn synth_error(
    img: &Image<'_>,
    id: mortar_core::ChunkId,
    name: u32,
    err: SynthError,
) -> (Key, Diagnostic) {
    let d = err.into_diagnostic();
    let d = match d.kind() {
        Kind::InternalError { what } => {
            let section = String::from_utf8_lossy(img.plan.name(name));
            internal(format!("{section}: {what}")).at(Place::argument("output"))
        }
        _ => d,
    };
    (Key::new(PHASE_IMAGE, 0, u64::from(id.0)), d)
}

/// One job (I-33): fill, hash each window, write each window.
fn run_job(
    img: &Image<'_>,
    w: &mut Worker,
    job: std::ops::Range<u64>,
    sink: &dyn Sink,
    stop: &AtomicBool,
    out: &mut JobOut,
) {
    let plan = img.plan;
    let len = (job.end - job.start) as usize;
    w.buf.resize(len, 0);
    let Worker { buf, scratch } = w;
    let steps = FillSteps::on().then(Instant::now);
    if let Err(fatal) = fill_into(img, scratch, job.start, &mut buf[..len], &mut out.errors) {
        out.fatal = Some(fatal);
        stop.store(true, Ordering::Relaxed);
    }
    if let Some(t) = steps {
        FILL_STEPS.fill_ns.fetch_add(FillSteps::since(t), Ordering::Relaxed);
    }
    out.flagged = std::mem::take(&mut scratch.flagged);
    let fill_failed = out.fatal.is_some() || !out.errors.is_empty() || !out.flagged.is_empty();
    let win = plan.window as usize;
    for (k, window) in buf[..len].chunks(win).enumerate() {
        let offset = job.start + (k * win) as u64;
        if let Some(f) = img.build_id.map(TreeStyle::tree_fn).or(img.leaf_fn) {
            out.leaves.push((offset / plan.window, mortar_digest::leaf(f, window)));
        }
        // A failed link writes nothing more (I-64, I-65): the temporary is
        // removed anyway.
        if fill_failed || stop.load(Ordering::Relaxed) {
            continue;
        }
        let steps = FillSteps::on().then(Instant::now);
        if let Err(e) = sink.write_at(offset, window) {
            out.write = Some((offset, e));
            stop.store(true, Ordering::Relaxed);
        }
        if let Some(t) = steps {
            FILL_STEPS.write_ns.fetch_add(FillSteps::since(t), Ordering::Relaxed);
            FILL_STEPS.write_bytes.fetch_add(window.len() as u64, Ordering::Relaxed);
        }
    }
    // A multi-window job grew the buffer: shrink it back to `W` (I-32).
    if buf.len() > win {
        buf.truncate(win);
        buf.shrink_to(win);
    }
}
