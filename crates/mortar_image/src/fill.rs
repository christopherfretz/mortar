//! Fill (§5): the jobs of a plan in parallel, each into its worker's
//! reused buffer, hashed (§8.3) and handed to a [`Sink`] window by window
//! (I-33, I-63).

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

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
    /// `S + A` of a reference with addend `addend` to `key` when that
    /// value is not the symbol's plus `A`: a section symbol of a split
    /// `Merge` section (`spec/synth.md` S-17, S-18: the piece holding
    /// byte `st_value + A`) and any key defined in an input `.eh_frame`
    /// (S-50, which follows the target offset). The output address, or
    /// the diagnostic (S-18; S-50 rules 4 and 5). `None` otherwise: the
    /// arch computes `S + A` from [`FillTables::sym`].
    fn split_target(&self, key: SymKey, addend: i64) -> Option<Result<u64, Diagnostic>> {
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
        Ok(Filled { build_id })
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
pub fn fill_range(
    img: &Image<'_>,
    lo: u64,
    out: &mut [u8],
    errs: &mut Vec<(Key, Diagnostic)>,
) -> Result<(), (Key, Diagnostic)> {
    let mut scratch = Scratch::default();
    fill_into(img, &mut scratch, lo, out, errs)
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
    if let Err(fatal) = fill_into(img, scratch, job.start, &mut buf[..len], &mut out.errors) {
        out.fatal = Some(fatal);
        stop.store(true, Ordering::Relaxed);
    }
    out.flagged = std::mem::take(&mut scratch.flagged);
    let fill_failed = out.fatal.is_some() || !out.errors.is_empty() || !out.flagged.is_empty();
    let win = plan.window as usize;
    for (k, window) in buf[..len].chunks(win).enumerate() {
        let offset = job.start + (k * win) as u64;
        if let Some(style) = img.build_id {
            out.leaves.push((offset / plan.window, mortar_digest::leaf(style.tree_fn(), window)));
        }
        // A failed link writes nothing more (I-64, I-65): the temporary is
        // removed anyway.
        if fill_failed || stop.load(Ordering::Relaxed) {
            continue;
        }
        if let Err(e) = sink.write_at(offset, window) {
            out.write = Some((offset, e));
            stop.store(true, Ordering::Relaxed);
        }
    }
    // A multi-window job grew the buffer: shrink it back to `W` (I-32).
    if buf.len() > win {
        buf.truncate(win);
        buf.shrink_to(win);
    }
}
