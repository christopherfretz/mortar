//! `.eh_frame` (`spec/synth.md` §4.2 to §4.5): which records survive,
//! CIE de-duplication, the output order and offsets, the value of a
//! reference into an input `.eh_frame`, and the bytes.
//!
//! **Plan** ([`EhPlan::new`], phase 80): from the parsed records of every
//! live `.eh_frame` section ([`EhSource`]: `mortar_merge`'s `Phase::eh`)
//! and the liveness of code sections, the kept records in output order
//! (`Slots.eh_records`, S-49), each record's output offset and the size.
//!
//! **Write** ([`EhPlan::write`], image): any byte range of the chunk, in
//! O(range + log records): the length word (raised to a multiple of 4,
//! S-47), the rewritten CIE pointer (S-52), the copied body, the padding,
//! and the one terminator (S-48). Relocated fields are the image phase's
//! (S-52, second bullet): it applies an input relocation at
//! [`EhPlan::place`] of its offset, and skips one in a removed record.

use std::borrow::Cow;

use mortar_core::layout::EhRecord;
use mortar_core::par::Pool;
use mortar_core::resolution::SectionKind;
use mortar_core::{FileId, SectionId};
use mortar_diag::Collector;
use mortar_elf::SectionIndex;
use mortar_elf::consts::SHT_PROGBITS;
use mortar_merge::What;
use mortar_merge::eh::{Code, EhFrames, Kind};
use mortar_merge::phase::Link;
use mortar_merge::refs::eh_ref;
pub use mortar_merge::refs::{EhValue, EhWho};

use crate::error::{Failed, SynthError, fail, internal, malformed, record_key};
use crate::out::{Window, align_up};

/// What the plan needs of an input `.eh_frame` beyond `mortar_merge`'s
/// parse: its file (S-55 orders the internal file's records last), its
/// alignment and its type (S-47, S-54).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EhInfo {
    pub file: FileId,
    /// log2 of `sh_addralign`.
    pub align: u8,
    /// `SHT_PROGBITS`, or `SHT_X86_64_UNWIND` (S-54).
    pub sh_type: u32,
}

/// Where an FDE's initial location points: its code section and the
/// target offset there (`st_value + A`, S-38).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CodeRef {
    pub section: SectionId,
    pub offset: i64,
}

/// The parsed `.eh_frame` input: `mortar_merge`'s [`EhFrames`]
/// (`Phase::eh`, S-34 to S-40, S-43, S-45), which has refused every
/// malformed record, plus per section row its [`EhInfo`] and its bytes.
/// [`MergeEh`] implements it over a link; tests over bare bytes.
pub trait EhSource: Sync {
    /// The records, CIE classes and sections (in `SectionId` order).
    fn frames(&self) -> &EhFrames;
    /// Section row `s`'s file, alignment and type.
    fn info(&self, s: usize) -> EhInfo;
    /// Section row `s`'s bytes.
    fn bytes(&self, s: usize) -> &[u8];
}

/// [`EhSource`] over phase 60's output and the link it read: the bytes
/// are read again through [`Link::bytes`] (borrowed when the store holds
/// them resident, `Inputs::resident`).
pub struct MergeEh<'a> {
    frames: &'a EhFrames,
    info: Box<[EhInfo]>,
    bytes: Box<[Cow<'a, [u8]>]>,
}

impl std::fmt::Debug for MergeEh<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MergeEh").field("info", &self.info).finish_non_exhaustive()
    }
}

impl<'a> MergeEh<'a> {
    /// The source for `frames` (`Phase::eh` of `mortar_merge::phase::run`
    /// over `link`). Walks the link's sections as phase 60 does, so the
    /// rows match; a mismatch is an internal error.
    pub fn new<'s: 'a>(frames: &'a EhFrames, link: &dyn Link<'s>) -> Result<Self, SynthError> {
        let mut info = Vec::with_capacity(frames.sections.len());
        let mut bytes = Vec::with_capacity(frames.sections.len());
        for f in 0..link.files() {
            let file = FileId(f);
            let Some(object) = link.object(file) else { continue };
            for index in 0..object.section_count() {
                let Some(row) = link.section(file, SectionIndex(index)) else { continue };
                if row.kind != SectionKind::EhFrame {
                    continue;
                }
                let n = info.len();
                if frames.sections.get(n).map(|s| s.section) != Some(row.id) {
                    return Err(internal(format!(
                        ".eh_frame section {} is not phase 60's row {n}",
                        row.id.0
                    )));
                }
                let s = object
                    .section(SectionIndex(index))
                    .map_err(|e| internal::<SynthError>(e.to_string()))?;
                let b = link.bytes(file, &s).map_err(|w| malformed::<SynthError>(w.message()))?;
                let align = s.header().sh_addralign.max(1);
                info.push(EhInfo { file, align: align.ilog2() as u8, sh_type: s.sh_type() });
                bytes.push(b);
            }
        }
        if info.len() != frames.sections.len() {
            return Err(internal("phase 60 has .eh_frame rows the link does not"));
        }
        Ok(Self { frames, info: info.into(), bytes: bytes.into() })
    }
}

impl EhSource for MergeEh<'_> {
    fn frames(&self) -> &EhFrames {
        self.frames
    }
    fn info(&self, s: usize) -> EhInfo {
        self.info[s]
    }
    fn bytes(&self, s: usize) -> &[u8] {
        &self.bytes[s]
    }
}

const NONE: u32 = u32::MAX;

/// The output `.eh_frame` (S-46 to S-49).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EhPlan {
    /// `Slots.eh_records`: the section and the record's index in it.
    records: Box<[EhRecord]>,
    /// Per output record: its index in `EhFrames::records`.
    glob: Box<[u32]>,
    /// Per output record: its section row.
    src: Box<[u32]>,
    /// For an FDE, the output index of its kept CIE; `NONE` for a CIE.
    cie: Box<[u32]>,
    /// The output offsets, one more than the records: the last is the
    /// terminator's.
    out_off: Box<[u64]>,
    /// Per input record (`EhFrames::records`): its output index, or
    /// `NONE` when removed.
    out_index: Box<[u32]>,
    /// The position of each section row in the output order.
    pos: Box<[u32]>,
    /// The first output record of each position, and the count last.
    first: Box<[u32]>,
    ids: Box<[SectionId]>,
    align: u8,
    sh_type: u32,
    fdes: u32,
    /// The padding between records (`spec/incr.md` IN-34a): `(output
    /// offset, size)`, ascending; empty in a plain link.
    pads: Box<[(u64, u64)]>,
    /// The return-address register of a padding CIE (IN-34a: 30 on
    /// AArch64, 16 on x86-64, 8 on i386).
    ra: u8,
}

/// The per-record liveness marks: 0 removed, 1 live FDE, 2 used CIE.
const REMOVED: u8 = 0;
const LIVE_FDE: u8 = 1;
const USED_CIE: u8 = 2;

impl EhPlan {
    /// The plan of S-41 to S-49: [`EhPlan::select`], then
    /// [`EhPlan::from_records`] of its list.
    pub fn new<S, L>(src: &S, code_live: L, pool: &Pool, diag: &Collector) -> Result<Self, Failed>
    where
        S: EhSource + ?Sized,
        L: Fn(SectionId) -> bool + Sync,
    {
        let records = Self::select(src, code_live, pool, diag)?;
        Self::from_records(src, &records, pool, diag)
    }

    /// The decision of S-41 to S-46 (`Slots.eh_records`, S-49): the kept
    /// records in output order. `code_live(s)` is S-41's test for an
    /// FDE's code section: live and not folded away
    /// (`Liveness::fde_live`).
    ///
    /// Cost: a parallel pass marking the records, one walk over the CIEs
    /// for S-44 ([`EhFrames::keepers`]: the identity classes are merge's,
    /// S-43), a parallel pass writing the list.
    pub fn select<S, L>(
        src: &S,
        code_live: L,
        pool: &Pool,
        diag: &Collector,
    ) -> Result<Vec<EhRecord>, Failed>
    where
        S: EhSource + ?Sized,
        L: Fn(SectionId) -> bool + Sync,
    {
        Self::select_in(src, code_live, false, pool, diag)
    }

    /// [`EhPlan::select`], with S-44's de-duplication per file when
    /// `per_file` (`spec/incr.md` IN-34a, synth.md S-111: an
    /// incremental-mode link, so that no FDE takes another file's CIE).
    pub fn select_in<S, L>(
        src: &S,
        code_live: L,
        per_file: bool,
        pool: &Pool,
        diag: &Collector,
    ) -> Result<Vec<EhRecord>, Failed>
    where
        S: EhSource + ?Sized,
        L: Fn(SectionId) -> bool + Sync,
    {
        let frames = src.frames();
        let sections = &frames.sections;
        let n_sec = sections.len();
        check_rows(frames, diag)?;
        let (internal_sec, order, _) = section_order(src);
        let mut marks = mark_live(src, &code_live, pool, diag)?;
        let files: Option<Vec<FileId>> =
            per_file.then(|| (0..n_sec).map(|s| src.info(s).file).collect());
        Cies::new(frames, &internal_sec, files.as_deref(), &mut marks);
        // S-46: the kept records per position.
        let mut first: Vec<u32> = pool.unit_cost(31).map(n_sec, |p| {
            let r = sections[order[p] as usize].records;
            marks[r.start as usize..(r.start + r.len) as usize]
                .iter()
                .filter(|&&m| m != REMOVED)
                .count() as u32
        });
        first.push(0);
        let total = pool.unit_cost(1).prefix_sum(&mut first) as usize;
        let mut records = vec![EhRecord { section: SectionId(0), piece: 0 }; total];
        let bounds: Vec<usize> = first.iter().map(|&f| f as usize).collect();
        let parts = mortar_core::par::split_offsets(&mut records, &bounds);
        pool.unit_cost(40).for_each_part(parts, |p, recs| {
            let sec = &sections[order[p] as usize];
            let m =
                &marks[sec.records.start as usize..(sec.records.start + sec.records.len) as usize];
            let kept = m.iter().enumerate().filter(|&(_, &m)| m != REMOVED);
            for (slot, (r, _)) in recs.iter_mut().zip(kept) {
                *slot = EhRecord { section: sec.section, piece: r as u32 };
            }
        });
        Ok(records)
    }

    /// The plan of `records`, a layout's `Slots.eh_records` (S-49): each
    /// FDE takes its own input CIE when that is listed, else the nearest
    /// earlier listed CIE of its identity class (S-43); then the output
    /// offsets and the size. A list the rules do not admit fails by name
    /// (S-49's validity rules): a record that is not one, an order that
    /// is not S-46's (a record given twice included), an FDE with no CIE
    /// to take (an internal FDE takes only its own, S-55), a listed CIE no
    /// listed FDE takes (S-42). Which FDEs are listed (S-41) is
    /// `Layout::check`'s.
    ///
    /// Cost: a walk of the list (a lookup per section run), a parallel
    /// pass per section writing the CIEs and sizes, a walk marking the
    /// used CIEs, a prefix sum.
    pub fn from_records<S>(
        src: &S,
        records: &[EhRecord],
        pool: &Pool,
        diag: &Collector,
    ) -> Result<Self, Failed>
    where
        S: EhSource + ?Sized,
    {
        Self::from_records_padded(src, records, &[], 0, pool, diag)
    }

    /// [`EhPlan::from_records`] with padding between the records
    /// (`spec/incr.md` IN-34a): each `(offset, size)` of `pads`, ascending
    /// by output offset, lies at a record boundary (or before the
    /// terminator) of the padded section. A pad of 4, 8 or 12 bytes is
    /// `DW_CFA_nop` padding of the record before it; one of 16 bytes or
    /// more is one padding CIE with return-address register `ra`. Any
    /// other size, a pad at no boundary, two pads at one boundary, or a
    /// short pad with no record before it fails by name.
    pub fn from_records_padded<S>(
        src: &S,
        records: &[EhRecord],
        pads: &[(u64, u64)],
        ra: u8,
        pool: &Pool,
        diag: &Collector,
    ) -> Result<Self, Failed>
    where
        S: EhSource + ?Sized,
    {
        let frames = src.frames();
        let sections = &frames.sections;
        let n_sec = sections.len();
        check_rows(frames, diag)?;
        let (internal_sec, order, pos) = section_order(src);
        let ids: Vec<SectionId> = sections.iter().map(|s| s.section).collect();
        let total = records.len();
        let bad = |k: usize, what: String| {
            let r = records[k];
            fail(
                diag,
                record_key(r.section, r.piece),
                internal(format!("EH_RECORDS row {k}: {what}")),
            )
        };
        let ListRuns { src_row, glob, first, cies } =
            match list_runs(frames, records, &ids, &pos, &internal_sec) {
                Ok(l) => l,
                Err((k, what)) => return bad(k, what),
            };
        let out_index = list_index(frames, records, &first, &pos, pool);
        // S-49: each FDE's CIE, and each record's output size (S-47).
        let mut cie = vec![NONE; total];
        let mut out_size = vec![0u64; total + 1];
        let mut errs: Vec<Option<(usize, String)>> = vec![None; n_sec];
        let bounds: Vec<usize> = first.iter().map(|&f| f as usize).collect();
        let parts = mortar_core::par::split_offsets(&mut cie, &bounds)
            .into_iter()
            .zip(mortar_core::par::split_offsets(&mut out_size[..total], &bounds))
            .zip(errs.chunks_mut(1));
        pool.unit_cost(60).for_each_part(parts, |p, ((c, size), err)| {
            let row = order[p] as usize;
            let base = sections[row].records.start;
            let k0 = first[p] as usize;
            for (j, (c, size)) in c.iter_mut().zip(size.iter_mut()).enumerate() {
                let k = k0 + j;
                let rec = &frames.records[glob[k] as usize];
                *size = rec.out_size();
                let Kind::Fde(f) = rec.kind else { continue };
                let own = base + f.cie;
                let taken = match out_index.get(own as usize) {
                    Some(&o) if o != NONE => Some(o).filter(|&o| (o as usize) < k),
                    _ if internal_sec[row] => None,
                    _ => frames.records.get(own as usize).and_then(|r| r.cie()).and_then(|ci| {
                        let list = &cies[ci.class as usize];
                        let at = list.partition_point(|&x| (x as usize) < k);
                        at.checked_sub(1).map(|a| list[a])
                    }),
                };
                let Some(o) = taken else {
                    err[0] = Some((k, "an FDE with no CIE to take (S-49)".to_owned()));
                    return;
                };
                *c = o;
            }
        });
        if let Some((k, what)) = errs.into_iter().flatten().min_by_key(|e| e.0) {
            return bad(k, what);
        }
        // S-42: every listed CIE is taken.
        let mut used = vec![false; total];
        for &c in &cie {
            if c != NONE {
                used[c as usize] = true;
            }
        }
        if let Some(k) = (0..total).find(|&k| cie[k] == NONE && !used[k]) {
            return bad(k, "a CIE no listed FDE takes (S-42)".to_owned());
        }
        let body = pool.unit_cost(1).prefix_sum(&mut out_size);
        out_size[total] = body;
        // IN-34a: the pads shift every record after them.
        let mut shift = 0u64;
        let mut j = 0usize;
        for (k, off) in out_size.iter_mut().enumerate() {
            let at = *off + shift;
            if let Some(&(p, size)) = pads.get(j)
                && p == at
            {
                let short = matches!(size, 4 | 8 | 12);
                if !(short || (size >= 16 && size % 4 == 0)) || (short && k == 0) {
                    let what = format!("an .eh_frame pad of {size:#x} bytes at {p:#x} (IN-34a)");
                    return fail(diag, record_key(SectionId(0), 0), internal(what));
                }
                shift += size;
                j += 1;
            }
            *off += shift;
        }
        if let Some(&(p, size)) = pads.get(j) {
            let what = format!("an .eh_frame pad of {size:#x} bytes at {p:#x} at no boundary");
            return fail(diag, record_key(SectionId(0), 0), internal(what));
        }
        let body = out_size[total];
        if body + 4 > u64::from(u32::MAX) {
            return fail(
                diag,
                record_key(SectionId(0), 0),
                crate::error::feature(".eh_frame larger than 4 GiB"),
            );
        }
        let fdes = cie.iter().filter(|&&c| c != NONE).count() as u32;
        let infos: Vec<EhInfo> = (0..n_sec).map(|s| src.info(s)).collect();
        let (align, sh_type) = out_type(&infos);
        Ok(Self {
            records: records.into(),
            glob: glob.into(),
            src: src_row.into(),
            cie: cie.into(),
            out_off: out_size.into(),
            out_index: out_index.into(),
            pos: pos.into(),
            first: first.into(),
            ids: ids.into(),
            align,
            sh_type,
            fdes,
            pads: pads.into(),
            ra,
        })
    }

    /// The padding between records (`spec/incr.md` IN-34a): `(output
    /// offset, size)`, ascending.
    #[must_use]
    pub fn pads(&self) -> &[(u64, u64)] {
        &self.pads
    }

    /// `Slots.eh_records` (S-49): the kept records in output order.
    #[must_use]
    pub fn records(&self) -> &[EhRecord] {
        &self.records
    }

    /// Whether the chunk exists (S-3: some live record).
    #[must_use]
    pub fn exists(&self) -> bool {
        !self.records.is_empty()
    }

    /// The chunk's size (S-49): the records' output sizes plus the
    /// terminator.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.out_off[self.records.len()] + 4
    }

    /// log2 of the chunk's alignment (S-47).
    #[must_use]
    pub fn align(&self) -> u8 {
        self.align
    }

    /// The output `sh_type` (layout L-49, S-54).
    #[must_use]
    pub fn sh_type(&self) -> u32 {
        self.sh_type
    }

    /// The number of output FDEs.
    #[must_use]
    pub fn fde_count(&self) -> u32 {
        self.fdes
    }

    /// The output offset of output record `n`; `n == records().len()` is
    /// the terminator.
    #[must_use]
    pub fn out_offset(&self, n: usize) -> u64 {
        self.out_off[n]
    }

    /// For output record `n`, the output index of its CIE if it is an FDE.
    #[must_use]
    pub fn cie_of(&self, n: usize) -> Option<usize> {
        let c = self.cie[n];
        (c != NONE).then_some(c as usize)
    }

    /// The section row and the `EhFrames::records` index of output record
    /// `n`.
    #[must_use]
    pub fn source(&self, n: usize) -> (usize, u32) {
        (self.src[n] as usize, self.glob[n])
    }

    /// The output index of input record `g` (`EhFrames::records`), if kept.
    #[must_use]
    pub fn output_index(&self, g: u32) -> Option<usize> {
        let k = *self.out_index.get(g as usize)?;
        (k != NONE).then_some(k as usize)
    }

    fn section_index(&self, section: SectionId) -> Option<usize> {
        self.ids.binary_search(&section).ok()
    }

    /// The output offset of input byte `offset` of `section` when it lies
    /// in a kept record: where the image phase applies a relocation of the
    /// input section (S-52). `None` for a byte of a removed record.
    #[must_use]
    pub fn place<S>(&self, src: &S, section: SectionId, offset: u64) -> Option<u64>
    where
        S: EhSource + ?Sized,
    {
        let s = self.section_index(section)?;
        let frames = src.frames();
        let recs = frames.records_of(s);
        let j = recs.partition_point(|r| u64::from(r.offset) <= offset).checked_sub(1)?;
        let rec = &recs[j];
        let n = self.output_index(frames.sections[s].records.start + j as u32)?;
        (offset < rec.end()).then(|| self.out_off[n] + offset - u64::from(rec.offset))
    }

    /// S-50: the value of a reference at offset `r` of input `.eh_frame`
    /// section `section` (`st_value + A` for a section symbol, `st_value`
    /// for a symbol), by `mortar_merge::refs::eh_ref`. `src` is the source
    /// the plan was made from. The error is the diagnostic, without place
    /// or key.
    pub fn value<S>(
        &self,
        src: &S,
        section: SectionId,
        r: i64,
        who: EhWho,
    ) -> Result<EhValue, SynthError>
    where
        S: EhSource + ?Sized,
    {
        let Some(s) = self.section_index(section) else {
            return Err(internal(format!("section {} is not a live .eh_frame", section.0)));
        };
        // The next kept record after the section's, in output order.
        let after = self.out_off[self.first[self.pos[s] as usize + 1] as usize];
        let kept = |g: u32| self.output_index(g).map(|n| self.out_off[n]);
        eh_ref(src.frames(), s, r, who, kept, after).map_err(|w| match w {
            What::OutsideSection => outside(who),
            w @ What::IntoRemovedRecord => malformed(w.message()),
            w => internal(w.message()),
        })
    }

    /// Fills `out` (S-47, S-48, S-52) for the window's range of the chunk.
    /// The relocated fields hold the input's bytes; the image phase
    /// applies the relocations over them.
    pub fn write<S>(&self, src: &S, out: &mut Window<'_>) -> Result<(), SynthError>
    where
        S: EhSource + ?Sized,
    {
        let frames = src.frames();
        let count = self.records.len();
        // IN-34a: a pad of 16 bytes or more is a padding CIE.
        for &(p, size) in self.pads.iter().filter(|p| p.1 >= 16) {
            if p < out.end() && p + size > out.start() {
                out.word(p, 4, size - 4);
                out.zero(p + 4, size - 4);
                out.put(p + 8, &[1, 0, 1, 0x7c, self.ra]);
            }
        }
        let first = self.out_off[1..=count].partition_point(|&end| end <= out.start());
        for k in first..count {
            let at = self.out_off[k];
            if at >= out.end() {
                return Ok(());
            }
            let (row, gi) = self.source(k);
            let rec = &frames.records[gi as usize];
            let bytes = src.bytes(row);
            let start = rec.offset as usize;
            let l = rec.len as usize;
            let Some(body) = bytes.get(start + 4..start + 4 + l) else {
                return Err(internal("an .eh_frame record runs past its section"));
            };
            let padded = align_up(l as u64, 4);
            // IN-34a: 4, 8 or 12 bytes of padding after the record are its
            // own `DW_CFA_nop`s.
            let gap = self.out_off[k + 1] - (at + 4 + padded);
            let padded = if matches!(gap, 4 | 8 | 12) { padded + gap } else { padded };
            out.word(at, 4, padded);
            match self.cie_of(k) {
                Some(c) => {
                    // S-52: the distance back from the pointer to its CIE.
                    let ptr = at + 4 - self.out_off[c];
                    out.word(at + 4, 4, ptr);
                    out.put(at + 8, body.get(4..).unwrap_or(&[]));
                }
                None => out.put(at + 4, body),
            }
            out.zero(at + 4 + l as u64, padded - l as u64);
        }
        // S-48.
        out.zero(self.out_off[count], 4);
        Ok(())
    }

    /// The code reference and whether the address range is 0 (S-59) of
    /// output record `n`, when it is an FDE.
    #[must_use]
    pub fn code<S>(&self, src: &S, n: usize) -> Option<(CodeRef, bool)>
    where
        S: EhSource + ?Sized,
    {
        let f = *src.frames().records.get(self.glob[n] as usize)?.fde()?;
        match f.code {
            Code::Section { section, offset } => Some((CodeRef { section, offset }, f.range == 0)),
            _ => None,
        }
    }
}

fn outside(who: EhWho) -> SynthError {
    match who {
        EhWho::Reloc => malformed(What::OutsideSection.message()),
        _ => malformed("offset is outside the section"),
    }
}

/// The rows phase 60 gives: sections ascending, records in section order.
fn check_rows(frames: &EhFrames, diag: &Collector) -> Result<(), Failed> {
    let sections = &frames.sections;
    if sections.windows(2).any(|w| w[0].section >= w[1].section) {
        return fail(diag, record_key(SectionId(0), 0), internal(".eh_frame sections not sorted"));
    }
    let mut at = 0u32;
    for s in sections {
        if s.records.start != at {
            return fail(
                diag,
                record_key(s.section, 0),
                internal(".eh_frame records not in section order"),
            );
        }
        at += s.records.len;
    }
    Ok(())
}

/// S-43, S-44: per identity class (merge's), the kept CIE; per file and
/// class with `files` (IN-34a).
struct Cies {
    keep: Vec<u32>,
    /// The internal file's records, as ranges of `EhFrames::records`
    /// (S-44: they take no part in de-duplication).
    internal: Vec<(u32, u32)>,
    /// Per file and class, the kept CIE (`files` given).
    per_file: std::collections::HashMap<(FileId, u32), u32>,
    /// Per section row its file and record range (`files` given).
    rows: Vec<(u32, u32, FileId)>,
}

impl Cies {
    /// The kept CIEs; marks every used CIE that is not kept removed.
    /// `files` is each section row's file, for S-44 per file.
    fn new(
        frames: &EhFrames,
        internal_sec: &[bool],
        files: Option<&[FileId]>,
        marks: &mut [u8],
    ) -> Self {
        let internal = frames
            .sections
            .iter()
            .zip(internal_sec)
            .filter(|(_, i)| **i)
            .map(|(s, _)| (s.records.start, s.records.start + s.records.len))
            .collect();
        let mut cies = Self {
            keep: Vec::new(),
            internal,
            per_file: std::collections::HashMap::new(),
            rows: Vec::new(),
        };
        if let Some(files) = files {
            for (s, &f) in frames.sections.iter().zip(files) {
                let r = s.records;
                cies.rows.push((r.start, r.start + r.len, f));
                for g in r.start..r.start + r.len {
                    if let Kind::Cie(c) = frames.records[g as usize].kind
                        && marks[g as usize] == USED_CIE
                        && !cies.is_internal(g)
                    {
                        cies.per_file.entry((f, c.class)).or_insert(g);
                    }
                }
            }
        } else {
            cies.keep = frames.keepers(|g| marks[g as usize] == USED_CIE && !cies.is_internal(g));
        }
        for (g, m) in marks.iter_mut().enumerate() {
            if *m == USED_CIE && cies.rep(frames, g as u32) != g as u32 {
                *m = REMOVED;
            }
        }
        cies
    }

    fn is_internal(&self, g: u32) -> bool {
        self.internal.iter().any(|&(a, b)| (a..b).contains(&g))
    }

    /// The kept CIE for CIE `g`.
    fn rep(&self, frames: &EhFrames, g: u32) -> u32 {
        match frames.records[g as usize].kind {
            Kind::Cie(c) if !self.is_internal(g) && self.rows.is_empty() => {
                self.keep[c.class as usize]
            }
            Kind::Cie(c) if !self.is_internal(g) => {
                let at = self.rows.partition_point(|r| r.1 <= g);
                self.rows
                    .get(at)
                    .and_then(|r| self.per_file.get(&(r.2, c.class)))
                    .copied()
                    .unwrap_or(g)
            }
            _ => g,
        }
    }
}

/// The list's section runs (`EhPlan::from_records`), in S-46's order: per
/// record its row in `EhFrames::sections` and its index in
/// `EhFrames::records`; per position the first record; per class the
/// listed CIEs.
struct ListRuns {
    src_row: Vec<u32>,
    glob: Vec<u32>,
    first: Vec<u32>,
    cies: Vec<Vec<u32>>,
}

/// Walks the list (a lookup per section run); a row out of S-46's order,
/// or naming no live record, is the error `(row, what)`.
fn list_runs(
    frames: &EhFrames,
    records: &[EhRecord],
    ids: &[SectionId],
    pos: &[u32],
    internal_sec: &[bool],
) -> Result<ListRuns, (usize, String)> {
    let sections = &frames.sections;
    let total = records.len();
    let mut src_row = vec![0u32; total];
    let mut glob = vec![0u32; total];
    let mut first = vec![0u32; sections.len() + 1];
    let mut cies: Vec<Vec<u32>> = vec![Vec::new(); frames.classes as usize];
    let mut next_pos = 0usize;
    let mut k = 0usize;
    while k < total {
        let section = records[k].section;
        let Ok(row) = ids.binary_search(&section) else {
            return Err((k, format!("section {} is no live .eh_frame", section.0)));
        };
        let p = pos[row] as usize;
        if p < next_pos {
            return Err((k, "not in S-46's order (sections)".to_owned()));
        }
        first[next_pos..=p].fill(k as u32);
        next_pos = p + 1;
        let sec = &sections[row];
        let mut prev: Option<u32> = None;
        while k < total && records[k].section == section {
            let piece = records[k].piece;
            if piece >= sec.records.len {
                return Err((k, format!("section {} has no record {piece}", section.0)));
            }
            if prev.is_some_and(|q| q >= piece) {
                return Err((k, "not in S-46's order (records), or given twice".to_owned()));
            }
            prev = Some(piece);
            let g = sec.records.start + piece;
            src_row[k] = row as u32;
            glob[k] = g;
            if let Kind::Cie(c) = frames.records[g as usize].kind
                && !internal_sec[row]
            {
                cies[c.class as usize].push(k as u32);
            }
            k += 1;
        }
    }
    first[next_pos..].fill(total as u32);
    Ok(ListRuns { src_row, glob, first, cies })
}

/// Per input record its row in the list, or `NONE` (a parallel pass per
/// section).
fn list_index(
    frames: &EhFrames,
    records: &[EhRecord],
    first: &[u32],
    pos: &[u32],
    pool: &Pool,
) -> Vec<u32> {
    let mut out_index = vec![NONE; frames.records.len()];
    let bounds: Vec<usize> = frames
        .sections
        .iter()
        .map(|s| s.records.start as usize)
        .chain(std::iter::once(frames.records.len()))
        .collect();
    let parts = mortar_core::par::split_offsets(&mut out_index, &bounds);
    pool.unit_cost(40).for_each_part(parts, |s, part| {
        let p = pos[s] as usize;
        for k in first[p]..first[p + 1] {
            part[records[k as usize].piece as usize] = k;
        }
    });
    out_index
}

/// S-46, S-55: per section row whether it is the internal file's, the
/// section rows in output order (the internal file's last), and each
/// row's position in it.
fn section_order<S: EhSource + ?Sized>(src: &S) -> (Vec<bool>, Vec<u32>, Vec<u32>) {
    let n_sec = src.frames().sections.len();
    let internal_sec: Vec<bool> =
        (0..n_sec).map(|s| src.info(s).file == FileId::INTERNAL).collect();
    // S-55, review finding 13: the internal file's records come last.
    let order: Vec<u32> = (0..n_sec as u32)
        .filter(|&i| !internal_sec[i as usize])
        .chain((0..n_sec as u32).filter(|&i| internal_sec[i as usize]))
        .collect();
    let mut pos = vec![0u32; n_sec];
    for (p, &i) in order.iter().enumerate() {
        pos[i as usize] = p as u32;
    }
    (internal_sec, order, pos)
}

/// S-41, S-42: per input record (`EhFrames::records`), [`REMOVED`],
/// [`LIVE_FDE`] or [`USED_CIE`].
fn mark_live<S, L>(src: &S, code_live: &L, pool: &Pool, diag: &Collector) -> Result<Vec<u8>, Failed>
where
    S: EhSource + ?Sized,
    L: Fn(SectionId) -> bool + Sync,
{
    let frames = src.frames();
    let sections = &frames.sections;
    let marks: Vec<Result<Vec<u8>, SynthError>> = pool.unit_cost(2000).map(sections.len(), |i| {
        let recs = frames.records_of(i);
        let mut m = vec![REMOVED; recs.len()];
        for (r, rec) in recs.iter().enumerate() {
            let Kind::Fde(f) = rec.kind else { continue };
            let Code::Section { section, .. } = f.code else { continue };
            if !code_live(section) {
                continue;
            }
            if recs.get(f.cie as usize).and_then(|c| c.cie()).is_none() {
                return Err(internal(format!(
                    ".eh_frame record {r} of section {} names record {} as its CIE",
                    sections[i].section.0, f.cie
                )));
            }
            m[r] = LIVE_FDE;
            m[f.cie as usize] = USED_CIE;
        }
        Ok(m)
    });
    let mut live = Vec::with_capacity(frames.records.len());
    for (i, m) in marks.into_iter().enumerate() {
        match m {
            Ok(m) => live.extend(m),
            Err(d) => return fail(diag, record_key(sections[i].section, 0), d.into_diagnostic()),
        }
    }
    Ok(live)
}

/// S-47: the largest input alignment; L-49 / S-54: the shared type, else
/// `SHT_PROGBITS`.
fn out_type(sections: &[EhInfo]) -> (u8, u32) {
    let align = sections.iter().map(|s| s.align).max().unwrap_or(2);
    let sh_type = match sections.first() {
        Some(s) if sections.iter().all(|t| t.sh_type == s.sh_type) => s.sh_type,
        _ => SHT_PROGBITS,
    };
    (align, sh_type)
}
