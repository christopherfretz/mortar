//! The fill plan (I-27, I-28) and the jobs (I-31): data, built once before
//! the windows and read by every worker. The records' order is the scan's
//! `RecIndex` (I-29), which fill reads beside the plan.

use std::collections::BTreeMap;
use std::ops::Range;

use mortar_arch::{ProofRun, Reloc};
use mortar_core::par::Pool;
use mortar_core::{ChunkId, FileId, SectionId};
use mortar_diag::{Diagnostic, FileRef, Kind};
use mortar_elf::Class;

use crate::target::Target;

/// A fill pattern, positioned by file offset modulo its length (I-41).
///
/// The spec positions the pattern by address. Inside one segment, and
/// between two segments, address and file offset are congruent modulo the
/// segment alignment (`p_vaddr ≡ p_offset mod p_align`, a power of two of
/// at least 4 for every target), so modulo a pattern of 1 or 4 bytes they
/// agree; non-alloc gaps are zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pattern {
    bytes: [u8; 4],
    len: u8,
}

impl Pattern {
    /// Zero bytes.
    pub const ZERO: Self = Self { bytes: [0; 4], len: 1 };
    /// AArch64's trap: a zero word (`udf #0`).
    pub const ZERO4: Self = Self { bytes: [0; 4], len: 4 };

    /// A one-byte pattern.
    #[must_use]
    pub const fn byte(b: u8) -> Self {
        Self { bytes: [b; 4], len: 1 }
    }

    /// A four-byte pattern, as it is in the file.
    #[must_use]
    pub const fn word(bytes: [u8; 4]) -> Self {
        Self { bytes, len: 4 }
    }

    /// Whether every byte is zero.
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.bytes == [0; 4]
    }

    /// Fills `out`, whose first byte is at file offset `offset`.
    pub fn fill(&self, offset: u64, out: &mut [u8]) {
        if self.is_zero() {
            out.fill(0);
            return;
        }
        let len = u64::from(self.len);
        let mut phase = (offset % len) as usize;
        for b in out {
            *b = self.bytes[phase];
            phase += 1;
            if phase == usize::from(self.len) {
                phase = 0;
            }
        }
    }
}

/// What one plan row is (I-27).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unit {
    /// [`Plan::inputs`]`[i]`: a placed input section, or a run of merged
    /// fragments copied from one input section.
    Input(u32),
    /// [`Plan::bytes`]`[i]`: a structural unit precomputed by the image
    /// (I-40) or any fixed bytes.
    Bytes(u32),
    /// [`Plan::synth`]`[i]`: a synthetic or patch chunk (§13, I-39).
    Synth(u32),
    /// [`Plan::merged`]`[i]`: a merged chunk's fragments (I-38).
    Merged(u32),
    /// A gap (I-41, I-42).
    Gap(Pattern),
}

/// One row: the file range `[offset, offset + size)` belongs to `unit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Row {
    pub offset: u64,
    pub size: u64,
    pub unit: Unit,
}

impl Row {
    /// One past the row's last byte.
    #[must_use]
    pub fn end(&self) -> u64 {
        self.offset + self.size
    }
}

/// An input unit: bytes of one input section copied at the row's offset,
/// then relocated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputUnit {
    pub file: FileId,
    /// The input section's index in its file.
    pub index: u32,
    pub section: SectionId,
    /// File offset, in the input file, of the input section's byte 0.
    pub src: u64,
    /// The section offset of the unit's first byte (non-zero for a run of
    /// fragments, I-38).
    pub start: u64,
    /// The input section's size (the site's bound).
    pub section_size: u64,
    /// The address of the section's byte 0 (`P` of offset 0); 0 when it is
    /// not `SHF_ALLOC`.
    pub addr: u64,
    pub alloc: bool,
    /// The input section's `SHF_EXECINSTR` (x64.md X-46 decides `DTPOFF`
    /// by it).
    pub exec: bool,
    /// [`Plan::names`] index of the section's name (the tombstone's key,
    /// the diagnostic's place).
    pub name: u32,
    /// [`Plan::relocs`] index, when the section has a relocation section.
    pub relocs: Option<u32>,
}

/// Where one relocation section's records come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecRows {
    /// Records in memory with their keys (tests; records the image makes).
    Decoded(Box<[Reloc]>),
    /// A `SHT_RELA` table of an input file, read at fill time for each
    /// clip (I-35 step 2, I-82 "read twice"): nothing per record lives in
    /// the plan (I-28).
    Rela(RelaTable),
}

/// A `SHT_RELA` table in an input file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelaTable {
    pub file: FileId,
    /// File offset of record 0.
    pub offset: u64,
    pub count: u32,
    /// The file's class: 24-byte (`ELFCLASS64`) or 12-byte records.
    pub class: Class,
    /// `SHT_REL` (WP-030c, i386): 16- or 8-byte records, the addend in the
    /// relocated field (`crate::rel`).
    pub rel: bool,
}

impl RelaTable {
    /// The size of one record.
    #[must_use]
    pub fn entsize(&self) -> u64 {
        match (self.rel, self.class) {
            (false, Class::Elf64) => 24,
            (false, Class::Elf32) => 12,
            (true, Class::Elf64) => 16,
            (true, Class::Elf32) => 8,
        }
    }

    /// The file range of records `[lo, hi)`.
    #[must_use]
    pub fn range(&self, lo: u32, hi: u32) -> Range<u64> {
        let e = self.entsize();
        self.offset + u64::from(lo) * e..self.offset + u64::from(hi) * e
    }
}

/// The relocation records of one relocated section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelocSection {
    /// The relocated section: the key of its order and `leads` bit in the
    /// scan's `RecIndex` (I-29).
    pub section: SectionId,
    pub rows: RecRows,
    /// The instruction proof of the section's bytes, in section offsets
    /// (`spec/arch.md` §3.6 rule 1).
    pub proof: Box<[ProofRun]>,
}

impl RelocSection {
    /// The number of records.
    #[must_use]
    pub fn len(&self) -> u32 {
        match &self.rows {
            RecRows::Decoded(r) => r.len() as u32,
            RecRows::Rela(t) => t.count,
        }
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A `Merged` chunk's unit (I-38): the live fragments of
/// `Order::chunks[chunk]`, each copied from its representative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedUnit {
    /// The index in merge's `Order::chunks`.
    pub chunk: u32,
    /// The output section's name, for diagnostics.
    pub name: u32,
}

/// Where a merge input section's bytes are, for the fragments copied
/// from it (sorted by section in [`Plan::merge_src`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeSource {
    pub section: SectionId,
    pub file: FileId,
    /// File offset of the section's byte 0.
    pub src: u64,
    pub size: u64,
}

/// An input `.eh_frame` section whose relocations the image applies over
/// the output `.eh_frame` (`spec/synth.md` S-52, S-53), sorted by section
/// in [`Plan::eh`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EhSection {
    pub section: SectionId,
    pub file: FileId,
    /// The section's index in its file.
    pub index: u32,
    /// [`Plan::relocs`] index.
    pub relocs: u32,
}

/// A synthetic chunk's row (§13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthUnit {
    /// The index of the chunk in the `chunks` slice given to
    /// [`crate::fill::write`].
    pub chunk: u32,
    pub id: ChunkId,
    /// The output section's name, for diagnostics.
    pub name: u32,
    /// `Division::Whole` (I-104).
    pub whole: bool,
    /// The output `.eh_frame`: after the chunk's bytes, the relocations of
    /// [`Plan::eh`] are applied over them (S-52).
    pub eh: bool,
}

/// The fill plan (I-27) and its jobs (I-31).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub target: Target,
    /// `[0, file_size)` is covered by the rows exactly.
    pub file_size: u64,
    /// `W`: the window ([`crate::IMAGE_BLOCK`] in a link; smaller in tests,
    /// rule 6).
    pub window: u64,
    /// Sorted by offset, disjoint, covering the file: gaps are rows.
    pub rows: Box<[Row]>,
    /// Each a run of whole windows `[lo, hi)`, in file order.
    pub jobs: Box<[Range<u64>]>,
    pub inputs: Box<[InputUnit]>,
    pub relocs: Box<[RelocSection]>,
    pub bytes: Box<[Box<[u8]>]>,
    pub synth: Box<[SynthUnit]>,
    pub merged: Box<[MergedUnit]>,
    /// Sorted by section.
    pub merge_src: Box<[MergeSource]>,
    /// Sorted by section.
    pub eh: Box<[EhSection]>,
    pub names: Box<[Box<[u8]>]>,
    /// The paths of the files the inputs come from, for diagnostics.
    pub paths: BTreeMap<FileId, FileRef>,
    /// The build id's descriptor in the file (I-73), when there is one.
    pub sealed: Option<Range<u64>>,
    /// The order of the records of the sections whose checks are fill's
    /// (`RecIndex::deferred`, I-14) and whose order fill does not find
    /// clip by clip: those with a large table cut into several clips
    /// (`crate::link`). Sorted by section; `None`: sorted.
    pub deferred_order: DeferredOrders,
}

/// [`Plan::deferred_order`]: each deferred section's record order, by
/// section.
pub type DeferredOrders = Box<[(SectionId, Option<Box<[u32]>>)]>;

impl Plan {
    /// The first row ending after `offset` (I-33 step 1).
    #[must_use]
    pub fn first_row(&self, offset: u64) -> usize {
        self.rows.partition_point(|r| r.end() <= offset)
    }

    /// The number of windows (build-id leaves, I-73).
    #[must_use]
    pub fn windows(&self) -> u64 {
        self.file_size.div_ceil(self.window)
    }

    /// The path of `file`, for a diagnostic.
    #[must_use]
    pub fn path(&self, file: FileId) -> FileRef {
        self.paths.get(&file).cloned().unwrap_or_else(|| FileRef::new(format!("<file {}>", file.0)))
    }

    /// Where merge input section `section`'s bytes are.
    #[must_use]
    pub fn merge_source(&self, section: SectionId) -> Option<&MergeSource> {
        let at = self.merge_src.binary_search_by_key(&section, |m| m.section).ok()?;
        self.merge_src.get(at)
    }

    /// The input `.eh_frame` section `section`, if it has relocations.
    #[must_use]
    pub fn eh_section(&self, section: SectionId) -> Option<&EhSection> {
        let at = self.eh.binary_search_by_key(&section, |e| e.section).ok()?;
        self.eh.get(at)
    }

    /// The order of deferred section `section`'s records, when the plan
    /// holds it ([`Plan::deferred_order`]).
    #[must_use]
    pub fn deferred_order(&self, section: SectionId) -> Option<mortar_core::records::Order<'_>> {
        let at = self.deferred_order.binary_search_by_key(&section, |d| d.0).ok()?;
        Some(match &self.deferred_order[at].1 {
            None => mortar_core::records::Order::Sorted,
            Some(p) => mortar_core::records::Order::Permuted(p),
        })
    }

    /// The name of [`Plan::names`]`[i]`.
    #[must_use]
    pub fn name(&self, i: u32) -> &[u8] {
        self.names.get(i as usize).map_or(&[], |n| n)
    }
}

/// Builds a [`Plan`]: rows in any order, checked and completed by
/// [`Builder::finish`].
#[derive(Debug)]
pub struct Builder {
    target: Target,
    file_size: u64,
    rows: Vec<Row>,
    pads: Vec<(Range<u64>, Pattern)>,
    inputs: Vec<InputUnit>,
    relocs: Vec<RelocSection>,
    bytes: Vec<Box<[u8]>>,
    synth: Vec<SynthUnit>,
    merged: Vec<MergedUnit>,
    merge_src: Vec<MergeSource>,
    eh: Vec<EhSection>,
    names: Vec<Box<[u8]>>,
    paths: BTreeMap<FileId, FileRef>,
    sealed: Option<Range<u64>>,
}

/// An `internal-error` of the plan check (I-27, I-44).
fn invalid(what: &str) -> Diagnostic {
    Diagnostic::error(Kind::InternalError { what: format!("invalid fill plan: {what}") })
}

impl Builder {
    /// An empty plan of a file of `file_size` bytes.
    #[must_use]
    pub fn new(target: Target, file_size: u64) -> Self {
        Self {
            target,
            file_size,
            rows: Vec::new(),
            pads: Vec::new(),
            inputs: Vec::new(),
            relocs: Vec::new(),
            bytes: Vec::new(),
            synth: Vec::new(),
            merged: Vec::new(),
            merge_src: Vec::new(),
            eh: Vec::new(),
            names: Vec::new(),
            paths: BTreeMap::new(),
            sealed: None,
        }
    }

    /// Names `file` for diagnostics.
    pub fn path(&mut self, file: FileId, path: FileRef) {
        self.paths.insert(file, path);
    }

    /// Adds a name; returns its index.
    pub fn name(&mut self, name: &[u8]) -> u32 {
        self.names.push(name.into());
        (self.names.len() - 1) as u32
    }

    /// Adds a relocation section; returns its index.
    pub fn relocs(&mut self, relocs: RelocSection) -> u32 {
        self.relocs.push(relocs);
        (self.relocs.len() - 1) as u32
    }

    /// The unit `[offset, offset + size)` of the input bytes `unit`.
    pub fn input(&mut self, offset: u64, size: u64, unit: InputUnit) {
        self.inputs.push(unit);
        let unit = Unit::Input((self.inputs.len() - 1) as u32);
        self.rows.push(Row { offset, size, unit });
    }

    /// Fixed bytes at `offset` (a structural unit, I-40).
    pub fn bytes(&mut self, offset: u64, bytes: Box<[u8]>) {
        let size = bytes.len() as u64;
        self.bytes.push(bytes);
        self.rows.push(Row { offset, size, unit: Unit::Bytes((self.bytes.len() - 1) as u32) });
    }

    /// A synthetic chunk of `size` bytes at `offset` (§13).
    pub fn synth(&mut self, offset: u64, size: u64, unit: SynthUnit) {
        self.synth.push(unit);
        self.rows.push(Row { offset, size, unit: Unit::Synth((self.synth.len() - 1) as u32) });
    }

    /// A merged chunk's fragments at `offset` (I-38).
    pub fn merged(&mut self, offset: u64, size: u64, unit: MergedUnit) {
        self.merged.push(unit);
        self.rows.push(Row { offset, size, unit: Unit::Merged((self.merged.len() - 1) as u32) });
    }

    /// Where a merge input section's bytes are (I-38).
    pub fn merge_source(&mut self, src: MergeSource) {
        self.merge_src.push(src);
    }

    /// An input `.eh_frame` section with relocations (S-52).
    pub fn eh_section(&mut self, eh: EhSection) {
        self.eh.push(eh);
    }

    /// The bytes of `range` no row covers are `pattern` (I-41); elsewhere
    /// gaps are zero. Pad ranges must not overlap.
    pub fn pad(&mut self, range: Range<u64>, pattern: Pattern) {
        if range.start < range.end && pattern != Pattern::ZERO {
            self.pads.push((range, pattern));
        }
    }

    /// The build id's descriptor (I-73, I-107).
    pub fn sealed(&mut self, range: Range<u64>) {
        self.sealed = Some(range);
    }

    /// Sorts and checks the rows (I-27), makes every gap a row (I-42) and
    /// forms the jobs for window `window` (I-31).
    #[allow(clippy::too_many_lines)]
    pub fn finish(self, window: u64, pool: &Pool) -> Result<Plan, Diagnostic> {
        let Self {
            target,
            file_size,
            mut rows,
            mut pads,
            inputs,
            relocs,
            bytes,
            synth,
            merged,
            mut merge_src,
            mut eh,
            names,
            paths,
            sealed,
        } = self;
        if window == 0 {
            return Err(invalid("window 0"));
        }
        rows.retain(|r| r.size > 0);
        pool.sort_by_key(&mut rows, |r| r.offset);
        pads.sort_by_key(|p| p.0.start);
        for w in pads.windows(2) {
            if w[0].0.end > w[1].0.start {
                return Err(invalid(&format!("pad ranges {:?} and {:?} overlap", w[0].0, w[1].0)));
            }
        }
        let mut out = Vec::with_capacity(rows.len() * 2 + 1);
        let mut at = 0u64;
        let mut pad = 0usize;
        let mut gap = |from: u64, to: u64, out: &mut Vec<Row>| {
            let mut from = from;
            while from < to {
                while pad < pads.len() && pads[pad].0.end <= from {
                    pad += 1;
                }
                let (end, pattern) = match pads.get(pad) {
                    Some((r, p)) if r.start <= from => (r.end.min(to), *p),
                    Some((r, _)) => (r.start.min(to), Pattern::ZERO),
                    None => (to, Pattern::ZERO),
                };
                out.push(Row { offset: from, size: end - from, unit: Unit::Gap(pattern) });
                from = end;
            }
        };
        for row in &rows {
            let Some(end) = row.offset.checked_add(row.size) else {
                return Err(invalid(&format!("row at {:#x} overflows", row.offset)));
            };
            if row.offset < at {
                return Err(invalid(&format!(
                    "row at {:#x} overlaps the row before (ends {at:#x})",
                    row.offset
                )));
            }
            if end > file_size {
                return Err(invalid(&format!(
                    "row at {:#x} ends past the file ({file_size:#x})",
                    row.offset
                )));
            }
            gap(at, row.offset, &mut out);
            out.push(*row);
            at = end;
        }
        gap(at, file_size, &mut out);
        if let Some(s) = &sealed
            && (s.start > s.end || s.end > file_size)
        {
            return Err(invalid(&format!("sealed range {s:?} is not in the file")));
        }
        for row in &out {
            let ok = match row.unit {
                Unit::Input(i) => inputs.get(i as usize).is_some_and(|u| {
                    u.relocs.is_none_or(|r| (r as usize) < relocs.len())
                        && u.start.checked_add(row.size).is_some_and(|e| e <= u.section_size)
                }),
                Unit::Bytes(i) => bytes.get(i as usize).is_some_and(|b| b.len() as u64 == row.size),
                Unit::Synth(i) => (i as usize) < synth.len(),
                Unit::Merged(i) => (i as usize) < merged.len(),
                Unit::Gap(_) => true,
            };
            if !ok {
                return Err(invalid(&format!(
                    "row at {:#x} names no unit of its size",
                    row.offset
                )));
            }
        }
        merge_src.sort_by_key(|m| m.section);
        eh.sort_by_key(|e| e.section);
        if merge_src.windows(2).any(|w| w[0].section == w[1].section)
            || eh.windows(2).any(|w| w[0].section == w[1].section)
        {
            return Err(invalid("a merge or .eh_frame source given twice"));
        }
        if eh.iter().any(|e| e.relocs as usize >= relocs.len()) {
            return Err(invalid("an .eh_frame section names no relocation section"));
        }
        let jobs = jobs(&out, &synth, file_size, window);
        Ok(Plan {
            target,
            file_size,
            window,
            rows: out.into_boxed_slice(),
            jobs,
            inputs: inputs.into_boxed_slice(),
            relocs: relocs.into_boxed_slice(),
            bytes: bytes.into_boxed_slice(),
            synth: synth.into_boxed_slice(),
            merged: merged.into_boxed_slice(),
            merge_src: merge_src.into_boxed_slice(),
            eh: eh.into_boxed_slice(),
            names: names.into_boxed_slice(),
            paths,
            sealed,
            deferred_order: Box::new([]),
        })
    }
}

/// I-31: one job per window, except that a whole synthetic chunk that
/// touches more than one window joins every window it touches, with the windows already in the
/// job; one sequential pass, a function of the plan and `W` only.
fn jobs(rows: &[Row], synth: &[SynthUnit], file_size: u64, w: u64) -> Box<[Range<u64>]> {
    let mut jobs: Vec<Range<u64>> = Vec::new();
    let mut next = 0u64; // the first window not yet in a job
    let n = file_size.div_ceil(w);
    for row in rows {
        let Unit::Synth(i) = row.unit else { continue };
        let first = row.offset / w;
        let last = (row.end() - 1) / w;
        // A whole chunk is written by one call (I-104): every chunk that
        // touches more than one window joins them, whatever its size (the
        // spec's "larger than W" misses a smaller chunk that straddles a
        // window boundary; spec/landed/image/WP-026.md).
        if !synth[i as usize].whole || first == last {
            continue;
        }
        // Windows before the chunk's first are single jobs.
        while next < first {
            jobs.push(next * w..((next + 1) * w).min(file_size));
            next += 1;
        }
        let end = ((last + 1) * w).min(file_size);
        if next > first {
            // The first window is already in the previous (multi-window)
            // job: extend it.
            if let Some(j) = jobs.last_mut() {
                j.end = j.end.max(end);
            }
        } else {
            jobs.push(first * w..end);
        }
        next = next.max(last + 1);
    }
    while next < n {
        jobs.push(next * w..((next + 1) * w).min(file_size));
        next += 1;
    }
    jobs.into_boxed_slice()
}
