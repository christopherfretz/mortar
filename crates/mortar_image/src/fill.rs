//! Fill (§5): the jobs of a plan in parallel, each into its worker's
//! reused buffer, hashed (§8.3) and handed to a [`Sink`] window by window
//! (I-33, I-63).

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use mortar_arch::{Arch, FillCx, FillTables, OutputKind, TlsBlock};
use mortar_core::SymKey;
use mortar_core::par::Pool;
use mortar_diag::{Diagnostic, Key, Kind, Place};
use mortar_digest::{Leaf, TreeStyle};

use crate::digest;
use crate::plan::{Plan, Unit};
use crate::reloc::{self, Scratch};
use crate::source::{self, Source};
use crate::synth::{SynthChunk, SynthError};
use crate::{PHASE_IMAGE, PHASE_OUTPUT};

/// The tables fill reads (`FillTables`), plus what §6.1 needs and
/// `SymValue` does not carry.
///
/// `FillTables` (WP-023b) does not carry the target's section flags, so
/// the image asks for them here; the driver answers from `Resolution`.
pub trait Tables: FillTables {
    /// Whether the key's definition is in an `SHF_ALLOC` section (I-112).
    fn target_alloc(&self, key: SymKey) -> bool;
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
    pub output: OutputKind,
    pub got_base: Option<u64>,
    pub tls: Option<TlsBlock>,
    /// The chunks [`crate::plan::SynthUnit::chunk`] indexes.
    pub chunks: &'a [&'a dyn SynthChunk],
    /// The build id's style; `None` when no leaves are computed (I-73).
    /// Leaves are windows, so it needs `plan.window == IMAGE_BLOCK` (I-75).
    pub build_id: Option<TreeStyle>,
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
pub fn write(img: &Image<'_>, sink: &dyn Sink, output_path: &str, pool: &Pool) -> Result<Filled, Failed> {
    let plan = img.plan;
    if img.build_id.is_some() && plan.window != crate::IMAGE_BLOCK {
        let what = format!("fill: a build id needs W = IMAGE_BLOCK, the plan has {:#x} (I-75)", plan.window);
        return Err(Failed { diagnostics: vec![(Key::new(PHASE_IMAGE, 0, 0), internal(what))] });
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
    for o in outs {
        diagnostics.extend(o.errors);
        diagnostics.extend(o.fatal);
        leaves.extend(o.leaves);
        if let Some((off, e)) = o.write
            && write_err.as_ref().is_none_or(|(w, _)| off < *w)
        {
            write_err = Some((off, e));
        }
    }
    let mut build_id = None;
    if write_err.is_none()
        && diagnostics.is_empty()
        && let Some(style) = img.build_id
    {
        let id = match digest::build_id(style, plan.file_size, &leaves) {
            Ok(id) => id,
            Err(e) => {
                let what = format!("fill: build id: {e}");
                return Err(Failed { diagnostics: vec![(Key::new(PHASE_IMAGE, 0, 0), internal(what))] });
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
    if diagnostics.is_empty() { Ok(Filled { build_id }) } else { Err(Failed { diagnostics }) }
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
                    reloc::relocate(img, scratch, u, rs, u.start + a, u.start + b, dst, errs).map_err(fail)?;
                }
            }
            Unit::Synth(i) => {
                let su = &plan.synth[i as usize];
                let cx = FillCx { tables: img.tables, output: img.output, got_base: img.got_base, tls: img.tls };
                let Some(chunk) = img.chunks.get(su.chunk as usize) else {
                    let what = format!("fill: no synthetic chunk {} (model §4.4)", su.chunk);
                    errs.push((Key::new(PHASE_IMAGE, 0, u64::from(su.id.0)), internal(what)));
                    continue;
                };
                if let Err(err) = chunk.write(&cx, a..b, dst) {
                    errs.push(synth_error(img, su.id, su.name, row.offset, err));
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
    Ok(())
}

fn internal(what: String) -> Diagnostic {
    Diagnostic::error(Kind::InternalError { what })
}

/// I-106: a synthetic chunk's error at `(100, 0, ChunkId)`.
fn synth_error(img: &Image<'_>, id: mortar_core::ChunkId, name: u32, chunk_off: u64, err: SynthError) -> (Key, Diagnostic) {
    let section = String::from_utf8_lossy(img.plan.name(name)).into_owned();
    match err {
        SynthError::Field { error, offset, what, target } => {
            let output = mortar_diag::FileRef::new("<output>");
            let _ = chunk_off;
            mortar_arch::diag::chunk(error, id, &output, &section, offset, what, &target)
        }
        SynthError::Internal(what) => (
            Key::new(PHASE_IMAGE, 0, u64::from(id.0)),
            internal(format!("{section}: {what}")).at(Place::argument("output")),
        ),
    }
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
    let fill_failed = out.fatal.is_some() || !out.errors.is_empty();
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
