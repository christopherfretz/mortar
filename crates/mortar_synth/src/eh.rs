//! `.eh_frame` (`spec/synth.md` §4.2 to §4.5): which records survive,
//! CIE de-duplication, the output order and offsets, the value of a
//! reference into an input `.eh_frame`, and the bytes.
//!
//! **Plan** ([`EhPlan::new`], phase 80): from the parsed records of every
//! live `.eh_frame` section ([`EhSource`], `mortar_merge`'s first stage)
//! and the liveness of code sections, the kept records in output order
//! (`Slots.eh_records`, S-49), each record's output offset and the size.
//!
//! **Write** ([`EhPlan::write`], image): any byte range of the chunk, in
//! O(range + log records): the length word (raised to a multiple of 4,
//! S-47), the rewritten CIE pointer (S-52), the copied body, the padding,
//! and the one terminator (S-48). Relocated fields are the image phase's
//! (S-52, second bullet): it applies an input relocation at
//! [`EhPlan::place`] of its offset, and skips one in a removed record.

use std::collections::HashMap;

use mortar_core::layout::EhRecord;
use mortar_core::par::Pool;
use mortar_core::{FileId, SectionId, SymbolId};
use mortar_diag::Collector;
use mortar_elf::consts::SHT_PROGBITS;

use crate::error::{Failed, SynthError, fail, internal, malformed, record_key};
use crate::out::{Window, align_up};

/// A live input `.eh_frame` section (an `EhFrame` section of
/// resolve.md §10 that `Liveness` keeps).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EhSection {
    pub id: SectionId,
    pub file: FileId,
    pub size: u32,
    /// log2 of `sh_addralign`.
    pub align: u8,
    /// `SHT_PROGBITS`, or `SHT_X86_64_UNWIND` (S-54).
    pub sh_type: u32,
}

/// Where an FDE's initial location points: the target section of the
/// relocation at offset 8 and the target offset there (S-38).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CodeRef {
    pub section: SectionId,
    pub offset: u64,
}

/// What a record is (S-34).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    Cie,
    Fde {
        /// The index of its CIE among the section's records (S-45).
        cie: u32,
        /// `None` when the initial location has no relocation and is 0
        /// (S-38: dropped), or targets an undefined weak or absolute
        /// symbol (S-38: dead).
        code: Option<CodeRef>,
        /// The address range field is 0 (S-59: no table entry).
        empty_range: bool,
    },
    /// A zero length word.
    Terminator,
}

/// A record of an input section: its offset, its length word `L` (the
/// bytes after the length word) and its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InRecord {
    pub offset: u32,
    pub len: u32,
    pub kind: RecordKind,
}

/// The resolved target of a relocation in a CIE, for S-43 identity:
/// "targets resolving to the same final symbol or section and offset".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RelocTarget {
    /// A global key (after resolution and redirects).
    Global(SymbolId),
    /// A local or section symbol: its section and `st_value`.
    Section { section: SectionId, offset: u64 },
    /// An absolute symbol's value.
    Absolute(u64),
}

/// A relocation at a CIE (S-43); `R_*_NONE` is omitted (S-38).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CieReloc {
    /// From the start of the record (its length word).
    pub offset: u32,
    pub r_type: u32,
    pub addend: i64,
    pub target: RelocTarget,
}

/// The parsed `.eh_frame` input (S-34 to S-40, `mortar_merge` phase 60):
/// what [`EhPlan`] needs and nothing else. The parser has already refused
/// every malformed record (S-36, S-37, S-39), so records cover each
/// section from 0 to its size, back to back, and an FDE's `cie` names a
/// CIE of its own section.
pub trait EhSource: Sync {
    /// Every live `.eh_frame` section, ascending by `SectionId`.
    fn sections(&self) -> &[EhSection];
    /// The records of `sections()[section]`, ascending by offset.
    fn records(&self, section: usize) -> &[InRecord];
    /// The bytes of `sections()[section]`.
    fn bytes(&self, section: usize) -> &[u8];
    /// Appends the relocations at CIE `record` of `section` to `out`, in
    /// offset order.
    fn cie_relocs(&self, section: usize, record: u32, out: &mut Vec<CieReloc>);
}

/// Who refers to an input `.eh_frame` offset (S-50).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Referrer {
    /// A relocation (its target offset `st_value + A` for a section
    /// symbol).
    Relocation,
    /// A named symbol other than a mapping symbol. `local_unreferenced`:
    /// a local that no relocation refers to, dropped inside a removed
    /// record (rule 4).
    Symbol { local_unreferenced: bool },
    /// An AArch64 mapping symbol (`$d`, `$x`), kept only with its record.
    Mapping,
}

/// The value of a reference into an input `.eh_frame` (S-50).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EhValue {
    /// An offset in the output `.eh_frame`.
    Offset(u64),
    /// The symbol is not written to `.symtab`.
    Dropped,
}

const NONE: u32 = u32::MAX;

/// The output `.eh_frame` (S-46 to S-49).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EhPlan {
    /// `Slots.eh_records`.
    records: Box<[EhRecord]>,
    /// Per output record: its source section's index in `sections()`.
    src: Box<[u32]>,
    in_off: Box<[u32]>,
    len: Box<[u32]>,
    /// For an FDE, the output index of its kept CIE; `NONE` for a CIE.
    cie: Box<[u32]>,
    /// The output offsets, one more than the records: the last is the
    /// terminator's.
    out_off: Box<[u64]>,
    /// Source section index of each position in the output order.
    order: Box<[u32]>,
    /// The position of each source section.
    pos: Box<[u32]>,
    /// The first output record of each position, and the count last.
    first: Box<[u32]>,
    ids: Box<[SectionId]>,
    sizes: Box<[u32]>,
    align: u8,
    sh_type: u32,
    fdes: u32,
}

/// The per-section liveness marks: 0 removed, 1 live FDE, 2 used CIE.
const REMOVED: u8 = 0;
const LIVE_FDE: u8 = 1;
const USED_CIE: u8 = 2;

impl EhPlan {
    /// The plan of S-41 to S-49. `code_live(s)` is S-41's test for an
    /// FDE's code section: live and not folded away (`Liveness::fde_live`).
    ///
    /// Cost: two parallel passes over the records, a sequential intern of
    /// the live CIEs (S-45: few), two prefix sums.
    pub fn new<S, L>(src: &S, code_live: L, pool: &Pool, diag: &Collector) -> Result<Self, Failed>
    where
        S: EhSource + ?Sized,
        L: Fn(SectionId) -> bool + Sync,
    {
        let sections = src.sections();
        let n_sec = sections.len();
        if sections.windows(2).any(|w| w[0].id >= w[1].id) {
            return fail(
                diag,
                record_key(SectionId(0), 0),
                internal(".eh_frame sections not sorted"),
            );
        }
        // S-55, review finding 13: the internal file's records come last.
        let order: Vec<u32> = (0..n_sec as u32)
            .filter(|&i| sections[i as usize].file != FileId::INTERNAL)
            .chain((0..n_sec as u32).filter(|&i| sections[i as usize].file == FileId::INTERNAL))
            .collect();
        let mut pos = vec![0u32; n_sec];
        for (p, &i) in order.iter().enumerate() {
            pos[i as usize] = p as u32;
        }

        let mut live = mark_live(src, &code_live, pool, diag)?;
        let rep_of = dedup_cies(src, &order, &mut live, pool, diag)?;

        // S-46: per position, the kept records and their local indices.
        let local: Vec<(Vec<u32>, u32)> = pool.map(n_sec, |p| {
            let i = order[p] as usize;
            let mut idx = vec![NONE; live[i].len()];
            let mut n = 0u32;
            for (r, &m) in live[i].iter().enumerate() {
                if m != REMOVED {
                    idx[r] = n;
                    n += 1;
                }
            }
            (idx, n)
        });
        let mut first: Vec<u32> = local.iter().map(|l| l.1).collect();
        first.push(0);
        let total = pool.prefix_sum(&mut first) as usize;
        first[n_sec] = total as u32;

        let mut records = vec![EhRecord { section: SectionId(0), piece: 0 }; total];
        let mut srcs = vec![0u32; total];
        let mut in_off = vec![0u32; total];
        let mut len = vec![0u32; total];
        let mut cie = vec![NONE; total];
        let mut out_size = vec![0u64; total + 1];
        let bounds: Vec<usize> = first.iter().map(|&f| f as usize).collect();
        let parts = mortar_core::par::split_offsets(&mut records, &bounds)
            .into_iter()
            .zip(mortar_core::par::split_offsets(&mut srcs, &bounds))
            .zip(mortar_core::par::split_offsets(&mut in_off, &bounds))
            .zip(mortar_core::par::split_offsets(&mut len, &bounds))
            .zip(mortar_core::par::split_offsets(&mut cie, &bounds))
            .zip(mortar_core::par::split_offsets(&mut out_size[..total], &bounds));
        let kept_cie = |i: u32, r: u32| -> u32 {
            let (ki, kr) =
                rep_of.binary_search_by_key(&(i, r), |row| row.0).map_or((i, r), |at| rep_of[at].1);
            first[pos[ki as usize] as usize] + local[pos[ki as usize] as usize].0[kr as usize]
        };
        pool.for_each_part(parts, |p, (((((recs, s), off), l), c), size)| {
            let i = order[p];
            let all = src.records(i as usize);
            let id = sections[i as usize].id;
            let mut k = 0;
            for (r, &m) in live[i as usize].iter().enumerate() {
                if m == REMOVED {
                    continue;
                }
                let rec = all[r];
                recs[k] = EhRecord { section: id, piece: r as u32 };
                s[k] = i;
                off[k] = rec.offset;
                l[k] = rec.len;
                if let RecordKind::Fde { cie: ci, .. } = rec.kind {
                    c[k] = kept_cie(i, ci);
                }
                // S-47.
                size[k] = 4 + align_up(u64::from(rec.len), 4);
                k += 1;
            }
        });
        let body = pool.prefix_sum(&mut out_size);
        out_size[total] = body;
        if body + 4 > u64::from(u32::MAX) {
            return fail(
                diag,
                record_key(SectionId(0), 0),
                crate::error::feature(".eh_frame larger than 4 GiB"),
            );
        }
        let fdes = cie.iter().filter(|&&c| c != NONE).count() as u32;
        let (align, sh_type) = out_type(sections);
        Ok(Self {
            records: records.into(),
            src: srcs.into(),
            in_off: in_off.into(),
            len: len.into(),
            cie: cie.into(),
            out_off: out_size.into(),
            order: order.into(),
            pos: pos.into(),
            first: first.into(),
            ids: sections.iter().map(|s| s.id).collect(),
            sizes: sections.iter().map(|s| s.size).collect(),
            align,
            sh_type,
            fdes,
        })
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

    /// The source section index and record index of output record `n`.
    #[must_use]
    pub fn source(&self, n: usize) -> (usize, u32) {
        (self.src[n] as usize, self.records[n].piece)
    }

    fn section_index(&self, section: SectionId) -> Option<usize> {
        self.ids.binary_search(&section).ok()
    }

    /// The output records of source section `i`.
    fn range_of(&self, i: usize) -> std::ops::Range<usize> {
        let p = self.pos[i] as usize;
        self.first[p] as usize..self.first[p + 1] as usize
    }

    /// The kept output record of source section `i` that starts at input
    /// offset `start`.
    fn kept_at(&self, i: usize, start: u32) -> Option<usize> {
        let range = self.range_of(i);
        let offs = &self.in_off[range.clone()];
        offs.binary_search(&start).ok().map(|k| range.start + k)
    }

    /// S-50 rule 3: the output offset of the next kept record of `i` at
    /// or after input offset `r`, else of the next kept record after `i`'s
    /// records, else the terminator.
    fn next_kept(&self, i: usize, r: u64) -> u64 {
        let range = self.range_of(i);
        let offs = &self.in_off[range.clone()];
        let k = offs.partition_point(|&o| u64::from(o) < r);
        self.out_off[range.start + k]
    }

    /// The output offset of input byte `offset` of `section` when it lies
    /// in a kept record: where the image phase applies a relocation of the
    /// input section (S-52). `None` for a byte of a removed record.
    #[must_use]
    pub fn place(&self, section: SectionId, offset: u64) -> Option<u64> {
        let i = self.section_index(section)?;
        let range = self.range_of(i);
        let offs = &self.in_off[range.clone()];
        let k = offs.partition_point(|&o| u64::from(o) <= offset).checked_sub(1)?;
        let n = range.start + k;
        let rel = offset - u64::from(self.in_off[n]);
        (rel < 4 + u64::from(self.len[n])).then(|| self.out_off[n] + rel)
    }

    /// S-50: the value of a reference at offset `r` of input `.eh_frame`
    /// section `section` (`st_value + A` for a section symbol, `st_value`
    /// for a symbol). `src` is the source the plan was made from. The
    /// error is the diagnostic, without place or key.
    pub fn value<S>(
        &self,
        src: &S,
        section: SectionId,
        r: i64,
        who: Referrer,
    ) -> Result<EhValue, SynthError>
    where
        S: EhSource + ?Sized,
    {
        let Some(i) = self.section_index(section) else {
            return Err(internal(format!("section {} is not a live .eh_frame", section.0)));
        };
        let size = u64::from(self.sizes[i]);
        // Rule 5.
        let Ok(r) = u64::try_from(r) else { return Err(outside(who)) };
        if r > size {
            return Err(outside(who));
        }
        let recs = src.records(i);
        let at = recs.partition_point(|rec| u64::from(rec.offset) <= r).checked_sub(1);
        let inside =
            at.map(|a| recs[a]).filter(|rec| r < u64::from(rec.offset) + 4 + u64::from(rec.len));
        if who == Referrer::Mapping {
            // S-50, S-76: a mapping symbol is kept exactly with its record.
            return Ok(match inside.and_then(|rec| self.kept_at(i, rec.offset)) {
                Some(n) => EhValue::Offset(self.out_off[n] + r - u64::from(self.in_off[n])),
                None => EhValue::Dropped,
            });
        }
        // Rule 1.
        if r == 0 {
            return Ok(EhValue::Offset(0));
        }
        let Some(rec) = inside else {
            // r == sh_size (rule 3).
            return Ok(EhValue::Offset(self.next_kept(i, r)));
        };
        if let Some(n) = self.kept_at(i, rec.offset) {
            // Rule 2.
            return Ok(EhValue::Offset(self.out_off[n] + r - u64::from(self.in_off[n])));
        }
        if r == u64::from(rec.offset) {
            // Rule 3.
            return Ok(EhValue::Offset(self.next_kept(i, r)));
        }
        // Rule 4.
        match who {
            Referrer::Relocation => Err(malformed("reference into a removed .eh_frame record")),
            Referrer::Symbol { local_unreferenced: true } => Ok(EhValue::Dropped),
            _ => Ok(EhValue::Offset(self.next_kept(i, u64::from(rec.offset)))),
        }
    }

    /// Fills `out` (S-47, S-48, S-52) for the window's range of the chunk.
    /// The relocated fields hold the input's bytes; the image phase
    /// applies the relocations over them.
    pub fn write<S>(&self, src: &S, out: &mut Window<'_>) -> Result<(), SynthError>
    where
        S: EhSource + ?Sized,
    {
        let n = self.records.len();
        let first = self.out_off[1..=n].partition_point(|&end| end <= out.start());
        for k in first..n {
            let at = self.out_off[k];
            if at >= out.end() {
                return Ok(());
            }
            let (i, _) = self.source(k);
            let bytes = src.bytes(i);
            let start = self.in_off[k] as usize;
            let l = self.len[k] as usize;
            let Some(body) = bytes.get(start + 4..start + 4 + l) else {
                return Err(internal("an .eh_frame record runs past its section"));
            };
            let padded = align_up(l as u64, 4);
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
        out.zero(self.out_off[n], 4);
        Ok(())
    }
}

fn outside(who: Referrer) -> SynthError {
    match who {
        Referrer::Relocation => {
            malformed("offset is outside the section; refer to it through a named symbol")
        }
        _ => malformed("offset is outside the section"),
    }
}

/// S-41, S-42: per section, per record, [`REMOVED`], [`LIVE_FDE`] or
/// [`USED_CIE`].
fn mark_live<S, L>(
    src: &S,
    code_live: &L,
    pool: &Pool,
    diag: &Collector,
) -> Result<Vec<Vec<u8>>, Failed>
where
    S: EhSource + ?Sized,
    L: Fn(SectionId) -> bool + Sync,
{
    let sections = src.sections();
    let n_sec = sections.len();
    let marks: Vec<Result<Vec<u8>, SynthError>> = pool.map(n_sec, |i| {
        let recs = src.records(i);
        let mut m = vec![REMOVED; recs.len()];
        for (r, rec) in recs.iter().enumerate() {
            if let RecordKind::Fde { cie, code, .. } = rec.kind {
                if !code.is_some_and(|c| code_live(c.section)) {
                    continue;
                }
                if !matches!(recs.get(cie as usize).map(|c| c.kind), Some(RecordKind::Cie)) {
                    return Err(internal(format!(
                        ".eh_frame record {r} of section {} names record {cie} as its CIE",
                        sections[i].id.0
                    )));
                }
                m[r] = LIVE_FDE;
                m[cie as usize] = USED_CIE;
            }
        }
        Ok(m)
    });
    let mut live = Vec::with_capacity(n_sec);
    for (i, m) in marks.into_iter().enumerate() {
        match m {
            Ok(m) => live.push(m),
            Err(d) => return fail(diag, record_key(sections[i].id, 0), d.into_diagnostic()),
        }
    }
    Ok(live)
}

/// `(section, record)` of a removed duplicate CIE and of its kept
/// representative, sorted by the first.
type RepOf = Vec<((u32, u32), (u32, u32))>;

/// S-47: the largest input alignment; L-49 / S-54: the shared type, else
/// `SHT_PROGBITS`.
fn out_type(sections: &[EhSection]) -> (u8, u32) {
    let align = sections.iter().map(|s| s.align).max().unwrap_or(2);
    let sh_type = match sections.first() {
        Some(s) if sections.iter().all(|t| t.sh_type == s.sh_type) => s.sh_type,
        _ => SHT_PROGBITS,
    };
    (align, sh_type)
}

/// The identity of a CIE (S-43): its bytes and its resolved relocations.
type CieIdentity<'a> = (&'a [u8], &'a [CieReloc]);

/// S-43, S-44: marks every used CIE equal to an earlier one removed and
/// returns, sorted, `(section, record) -> representative`.
fn dedup_cies<S>(
    src: &S,
    order: &[u32],
    live: &mut [Vec<u8>],
    pool: &Pool,
    diag: &Collector,
) -> Result<RepOf, Failed>
where
    S: EhSource + ?Sized,
{
    let sections = src.sections();
    // S-43, S-44: CIE identity over the used CIEs, first in
    // (FileId, sh_index, record) order kept. Internal records take no
    // part (S-44).
    let used: Vec<(u32, u32)> = order
        .iter()
        .filter(|&&i| sections[i as usize].file != FileId::INTERNAL)
        .flat_map(|&i| {
            live[i as usize]
                .iter()
                .enumerate()
                .filter(|m| *m.1 == USED_CIE)
                .map(move |(r, _)| (i, r as u32))
        })
        .collect();
    let relocs: Vec<Vec<CieReloc>> = pool.map(used.len(), |u| {
        let (i, r) = used[u];
        let mut v = Vec::new();
        src.cie_relocs(i as usize, r, &mut v);
        v
    });
    // (section, record) of a de-duplicated CIE -> its representative,
    // sorted by the first.
    let mut rep_of: RepOf = Vec::new();
    {
        let mut seen: HashMap<CieIdentity<'_>, (u32, u32)> = HashMap::new();
        for (u, &(i, r)) in used.iter().enumerate() {
            let rec = src.records(i as usize)[r as usize];
            let bytes = src.bytes(i as usize);
            let start = rec.offset as usize;
            let Some(body) = bytes.get(start..start + 4 + rec.len as usize) else {
                return fail(
                    diag,
                    record_key(sections[i as usize].id, r),
                    internal("a CIE runs past its section"),
                );
            };
            match seen.get(&(body, relocs[u].as_slice())) {
                Some(&first) => {
                    rep_of.push(((i, r), first));
                    live[i as usize][r as usize] = REMOVED;
                }
                None => {
                    seen.insert((body, relocs[u].as_slice()), (i, r));
                }
            }
        }
    }
    rep_of.sort_unstable();
    Ok(rep_of)
}
