//! `.eh_frame` cut into records (`spec/synth.md` §4.1, §4.2): S-34 to
//! S-40, the CIE identity of S-43 and the `Split` rows of S-45.
//!
//! [`parse`] takes each input `.eh_frame` with its relocations, already
//! classified and resolved by the caller ([`EhReloc`]), and returns
//! [`EhFrames`]: per section its records, terminator and size; per record
//! its length, relocations, and what it is (a [`Cie`] with its identity
//! class, or an [`Fde`] with its CIE and code section). Everything that
//! depends on liveness (which FDEs and CIEs are kept, their order and
//! output offsets, S-41, S-42, S-44, S-46 to S-49) is `mortar_synth`'s;
//! [`EhFrames::keepers`] and [`crate::refs::eh_ref`] are helpers for it.
//!
//! These types are not in `mortar_core`: `spec/model.md` §3.5 has only
//! the `Split.eh_*` columns, which [`EhFrames::split_rows`] gives.

use mortar_core::hash::HashMap;
use mortar_core::par::Pool;
use mortar_core::split::{EhPiece, SplitRow};
use mortar_core::{FileId, OptId, SectionId, Span, SymbolId};

use crate::problem::{Problem, What};

/// A relocation's class, as far as `.eh_frame` cares (`spec/arch.md`
/// §3.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RelocClass {
    /// `R_*_NONE`: ignored everywhere in `.eh_frame` (S-38).
    None,
    /// `DemandClass::Pc`.
    Pc,
    /// `DemandClass::AbsWord` or `AbsNarrow`: an absolute address of
    /// [`EhReloc::width`] bytes (`udata4` and `sdata4` pointers of
    /// non-PIC x86-64 code use `R_X86_64_32`).
    Abs,
    /// Any other class: accepted at a place that is not a pointer of S-37.
    Other,
}

/// What a relocation's symbol resolves to (the caller follows
/// `sym_map`, `redirect` and `def`, resolve.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    /// Defined in a section of the link: the symbol's `st_value` in it (0
    /// for a section symbol).
    Section { section: SectionId, value: u64 },
    /// `SHN_ABS`, or symbol 0.
    Absolute(u64),
    /// Not defined by any input (a global).
    Undefined { symbol: SymbolId, weak: bool },
    /// A global defined elsewhere than in a section of an input object: a
    /// common, a shared or a linker-defined symbol.
    Elsewhere(SymbolId),
}

/// One relocation of an `.eh_frame` section, classified.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EhReloc {
    /// `r_offset`.
    pub offset: u64,
    pub r_type: u32,
    /// The ABI name (diagnostics).
    pub name: &'static str,
    pub class: RelocClass,
    /// The data field's width in bytes; 0 for an instruction or a marker.
    pub width: u8,
    /// `r_addend`, or for REL the addend read from the field.
    pub addend: i64,
    pub target: Target,
}

/// One input `.eh_frame` section.
#[derive(Debug, Clone, Copy)]
pub struct EhIn<'a> {
    pub section: SectionId,
    pub file: FileId,
    /// Its index in its file.
    pub index: u32,
    pub bytes: &'a [u8],
    /// Its relocations, sorted by `offset` (stably: equal offsets in file
    /// order), `R_*_NONE` included or not.
    pub relocs: &'a [EhReloc],
}

/// A pointer field of a CIE or FDE: its encoding and its offset in the
/// record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pointer {
    pub enc: u8,
    pub at: u32,
}

/// A CIE (S-35).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cie {
    /// The identity class (S-43): equal for CIEs with equal bytes and
    /// equal relocations (offset, type, addend, target); numbered by first
    /// occurrence in `(SectionId, record)` order over every CIE, live or
    /// not. Among the live members of a class the first is kept (S-44).
    pub class: u32,
    pub version: u8,
    /// The `R` encoding (`absptr` when absent).
    pub fde_enc: u8,
    /// The `L` encoding, when present and not `omit`.
    pub lsda_enc: Option<u8>,
    /// The `P` pointer, when present and not `omit`.
    pub personality: Option<Pointer>,
    /// `S` (a signal frame).
    pub signal: bool,
    /// Whether the augmentation starts with `z` (FDEs then have an
    /// augmentation data length).
    pub z: bool,
}

/// What an FDE's initial location designates (S-38).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Code {
    /// `offset` (`st_value + A`) in `section`; the FDE is live when the
    /// section is live and not folded (S-41).
    Section { section: SectionId, offset: i64 },
    /// No relocation and a zero field: dropped (S-38).
    Dropped,
    /// An absolute target: dead (S-38).
    Absolute,
    /// An undefined target, weak or not: dead (S-38 for weak; a strong
    /// undefined fails the link elsewhere, resolve.md).
    Undefined,
}

/// An FDE.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fde {
    /// Its CIE: the record's index among its section's records.
    pub cie: u32,
    pub code: Code,
    /// The address range (the field after the initial location).
    pub range: u64,
    /// The LSDA pointer, when the CIE has `L` (not `omit`).
    pub lsda: Option<Pointer>,
}

/// A record's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Cie(Cie),
    Fde(Fde),
}

/// One CIE or FDE (terminators are not records, S-48).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Record {
    /// Its offset in its section.
    pub offset: u32,
    /// `L`: the body length after the length word. The output size is
    /// `4 + align_up(L, 4)` (S-47): [`Record::out_size`].
    pub len: u32,
    /// Its relocations (`R_*_NONE` among them, which nothing applies): a
    /// range of the section's [`EhIn::relocs`].
    pub relocs: Span,
    pub kind: Kind,
}

impl Record {
    /// S-47: `4 + align_up(L, 4)`.
    #[must_use]
    pub fn out_size(&self) -> u64 {
        4 + u64::from(self.len).next_multiple_of(4)
    }

    /// The input end: `offset + 4 + L`.
    #[must_use]
    pub fn end(&self) -> u64 {
        u64::from(self.offset) + 4 + u64::from(self.len)
    }

    /// The FDE, for an FDE.
    #[must_use]
    pub fn fde(&self) -> Option<&Fde> {
        match &self.kind {
            Kind::Fde(f) => Some(f),
            Kind::Cie(_) => None,
        }
    }

    /// The CIE, for a CIE.
    #[must_use]
    pub fn cie(&self) -> Option<&Cie> {
        match &self.kind {
            Kind::Cie(c) => Some(c),
            Kind::Fde(_) => None,
        }
    }
}

/// The `Split.eh_of`, `eh_off` and `eh_piece` columns.
pub type EhRows = (Box<[SplitRow]>, Box<[u32]>, Box<[EhPiece]>);

/// One input `.eh_frame`, parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EhSection {
    pub section: SectionId,
    /// `sh_size`.
    pub size: u32,
    /// Its records: a range of [`EhFrames::records`].
    pub records: Span,
    /// The offset of its terminator, if it has one (the last 4 bytes,
    /// S-39).
    pub terminator: Option<u32>,
}

/// Every input `.eh_frame`, parsed (S-34 to S-40, S-43, S-45).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EhFrames {
    /// Every input section given to [`parse`], in its order (`SectionId`
    /// order), with or without records.
    pub sections: Box<[EhSection]>,
    pub records: Box<[Record]>,
    /// The number of CIE identity classes.
    pub classes: u32,
}

impl EhFrames {
    /// The `Split.eh_of`, `eh_off` and `eh_piece` columns (S-45): a row
    /// per section with at least one record. An FDE's `code` is its code
    /// section when [`Code::Section`], else none (dead).
    #[must_use]
    pub fn split_rows(&self) -> EhRows {
        let rows = self
            .sections
            .iter()
            .filter(|s| s.records.len > 0)
            .map(|s| SplitRow { section: s.section, pieces: s.records })
            .collect();
        let off = self.records.iter().map(|r| r.offset).collect();
        let piece = self
            .records
            .iter()
            .map(|r| match r.kind {
                Kind::Cie(_) => EhPiece::CIE,
                Kind::Fde(f) => EhPiece {
                    cie: f.cie,
                    code: match f.code {
                        Code::Section { section, .. } => OptId::some(section),
                        _ => OptId::NONE,
                    },
                },
            })
            .collect();
        (rows, off, piece)
    }

    /// The records of section row `s`.
    #[must_use]
    pub fn records_of(&self, s: usize) -> &[Record] {
        let r = self.sections[s].records;
        &self.records[r.start as usize..(r.start + r.len) as usize]
    }

    /// S-44: per CIE class, the kept CIE (a global record index): the
    /// least live member in `(SectionId, record)` order, `u32::MAX` when
    /// no member is live. `live(record)` says whether a CIE is live (a
    /// live FDE uses it, S-42).
    #[must_use]
    pub fn keepers(&self, live: impl Fn(u32) -> bool) -> Vec<u32> {
        let mut kept = vec![u32::MAX; self.classes as usize];
        for (i, r) in self.records.iter().enumerate() {
            if let Kind::Cie(c) = r.kind {
                let k = &mut kept[c.class as usize];
                if *k == u32::MAX && live(i as u32) {
                    *k = i as u32;
                }
            }
        }
        kept
    }
}

/// The fields an S-37 pointer can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    /// The FDE initial location (`R`).
    Location,
    /// `P`.
    Personality,
    /// `L`.
    Lsda,
}

/// `DW_EH_PE_omit`.
const OMIT: u8 = 0xff;
/// `DW_EH_PE_pcrel`.
const PCREL: u8 = 0x10;
/// `DW_EH_PE_indirect`.
const INDIRECT: u8 = 0x80;

/// S-37: whether `enc` is accepted for `field`, and its width in bytes
/// (`word` for `absptr`). `uleb128`/`sleb128` are refused: S-37 refuses
/// them in a relocated pointer, and the `R`, `P` and `L` pointers are the
/// relocated ones in every producer.
fn pointer_width(enc: u8, field: Field, word: u8) -> Result<u8, What> {
    let refused = What::Encoding(enc);
    if enc & 0x70 > PCREL || (enc & INDIRECT != 0 && field == Field::Location) {
        return Err(refused);
    }
    match enc & 0x0f {
        0x00 => Ok(word),
        0x02 | 0x0a => Ok(2),
        0x03 | 0x0b => Ok(4),
        0x04 | 0x0c => Ok(8),
        _ => Err(refused),
    }
}

/// A cursor over a record's body, every read bounded by the record.
struct Body<'a> {
    bytes: &'a [u8],
    at: usize,
    end: usize,
}

impl Body<'_> {
    fn u8(&mut self) -> Result<u8, What> {
        let b = *self.bytes[..self.end].get(self.at).ok_or(What::CorruptedRecord)?;
        self.at += 1;
        Ok(b)
    }

    fn uint(&mut self, width: u8) -> Result<u64, What> {
        let w = usize::from(width);
        let s = self.bytes[..self.end].get(self.at..self.at + w).ok_or(What::CorruptedRecord)?;
        self.at += w;
        let mut v = [0u8; 8];
        v[..w].copy_from_slice(s);
        Ok(u64::from_le_bytes(v))
    }

    /// A LEB128 number (its value is not needed, only its extent; values
    /// over 64 bits are corrupt).
    fn leb(&mut self) -> Result<u64, What> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.u8()?;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(What::CorruptedRecord)
    }

    fn skip(&mut self, n: u64) -> Result<(), What> {
        let n = usize::try_from(n).map_err(|_| What::CorruptedRecord)?;
        if self.end - self.at < n {
            return Err(What::CorruptedRecord);
        }
        self.at += n;
        Ok(())
    }
}

/// The relocations of a section, other than `R_*_NONE`, whose place is in
/// `from..to`, as a range of indices of `relocs`.
fn relocs_in(relocs: &[EhReloc], from: u64, to: u64) -> std::ops::Range<usize> {
    let a = relocs.partition_point(|r| r.offset < from);
    let b = relocs.partition_point(|r| r.offset < to);
    a..b
}

/// The relocation of a pointer field at `at..at + width` (S-37, S-38):
/// none, or one at its start whose class fits the encoding (`pcrel`:
/// `Pc`; otherwise `Abs`) and whose width is the field's.
fn pointer_reloc(
    relocs: &[EhReloc],
    at: u64,
    width: u8,
    enc: u8,
) -> Result<Option<&EhReloc>, What> {
    let mut found = None;
    for r in &relocs[relocs_in(relocs, at, at + u64::from(width))] {
        if r.class == RelocClass::None {
            continue;
        }
        let class = if enc & 0x70 == PCREL { RelocClass::Pc } else { RelocClass::Abs };
        if r.offset != at || r.class != class || r.width != width {
            return Err(What::PointerReloc(r.name.to_owned()));
        }
        if found.replace(r).is_some() {
            return Err(What::TwoRelocs);
        }
    }
    Ok(found)
}

/// A section's records, before CIE classes (step 1 of [`parse`]).
struct Parsed {
    records: Vec<Record>,
    terminator: Option<u32>,
}

/// S-34 to S-40 for one section. The problem's `piece` is the record's
/// ordinal.
#[allow(clippy::too_many_lines)] // the record format, field by field
fn parse_section(s: &EhIn<'_>, word: u8) -> Result<Parsed, Problem> {
    let problem = |piece: usize, offset: u64, what: What| Problem {
        file: s.file,
        section: s.index,
        piece: piece as u32,
        offset,
        what,
    };
    let bytes = s.bytes;
    let size = bytes.len();
    if u32::try_from(size).is_err() {
        return Err(problem(0, 0, What::SectionTooLarge));
    }
    let relocs = s.relocs;
    let real = |r: &&EhReloc| r.class != RelocClass::None;
    let mut records: Vec<Record> = Vec::new();
    let mut at = 0usize;
    let mut terminator = None;
    let mut cursor = 0usize;
    while at < size {
        let n = records.len();
        let Some(head) = bytes.get(at..at + 4) else {
            return Err(problem(n, at as u64, What::PastEnd));
        };
        let len = u32::from_le_bytes(head.try_into().unwrap_or_default());
        if len == 0 {
            // S-39: nothing may follow a terminator.
            terminator = Some(at as u32);
            let after = at + 4;
            if let Some(r) = relocs[relocs_in(relocs, at as u64, u64::MAX)].iter().find(real) {
                return Err(problem(n, r.offset, What::RelocNotInPiece));
            }
            if after < size {
                return Err(problem(n, after as u64, What::AfterTerminator));
            }
            at = after;
            break;
        }
        if len == u32::MAX {
            return Err(problem(n, at as u64, What::Dwarf64));
        }
        let end = at + 4 + len as usize;
        if end > size {
            return Err(problem(n, at as u64, What::PastEnd));
        }
        let fail = |what: What| problem(n, at as u64, what);
        // The records and the relocations are both in offset order: the
        // record's span continues from the previous one's end (a search
        // over the whole section per record is two cache-missing binary
        // searches).
        let lo = cursor + relocs[cursor..].partition_point(|r| r.offset < at as u64);
        let hi = lo + relocs[lo..].iter().take_while(|r| r.offset < end as u64).count();
        cursor = hi;
        let span = lo..hi;
        let mine = &relocs[span.clone()];
        // The length and the CIE id are the linker's (S-52).
        if let Some(r) = mine.iter().filter(real).find(|r| r.offset < at as u64 + 8) {
            return Err(problem(n, r.offset, What::RelocInHeader));
        }
        let mut body = Body { bytes, at: at + 4, end };
        let id = body.uint(4).map_err(fail)?;
        let kind = if id == 0 {
            Kind::Cie(parse_cie(&mut body, mine, at, word).map_err(fail)?)
        } else {
            // S-39: the CIE pointer is the distance back from its own word.
            let cie_at = (at as u64 + 4).checked_sub(id);
            // The records are in offset order: the one at `c`, if any, by
            // binary search (a scan from the front is quadratic in the
            // records of a section whose CIEs are not all first).
            let cie = cie_at.and_then(|c| {
                let k = records.partition_point(|r| u64::from(r.offset) < c);
                records.get(k).filter(|r| u64::from(r.offset) == c && r.cie().is_some()).map(|_| k)
            });
            let Some(cie) = cie else {
                return Err(problem(n, at as u64 + 4, What::BadCieReference));
            };
            let Kind::Cie(c) = records[cie].kind else { unreachable!("found a CIE") };
            Kind::Fde(parse_fde(&mut body, mine, at, cie as u32, &c, word).map_err(fail)?)
        };
        let first = span.start as u32;
        records.push(Record {
            offset: at as u32,
            len,
            relocs: Span { start: first, len: (span.end - span.start) as u32 },
            kind,
        });
        at = end;
    }
    // S-39: a relocation past the records (and not after a terminator,
    // handled above) is in no piece.
    if let Some(r) = relocs[relocs_in(relocs, at as u64, u64::MAX)].iter().find(real) {
        return Err(problem(records.len(), r.offset, What::RelocNotInPiece));
    }
    Ok(Parsed { records, terminator })
}

/// S-35 to S-37: a CIE's body after its id.
fn parse_cie(body: &mut Body<'_>, relocs: &[EhReloc], at: usize, word: u8) -> Result<Cie, What> {
    let version = body.u8()?;
    if version != 1 && version != 3 {
        return Err(What::CieVersion(version));
    }
    let aug_start = body.at;
    while body.u8()? != 0 {}
    let aug = &body.bytes[aug_start..body.at - 1];
    let shown = || String::from_utf8_lossy(aug).into_owned();
    let z = aug.first() == Some(&b'z');
    if !aug.is_empty() && !z {
        return Err(What::Augmentation(shown()));
    }
    let mut seen = 0u8;
    for &c in aug.iter().skip(1) {
        let bit = match c {
            b'R' => 1,
            b'P' => 2,
            b'L' => 4,
            b'S' => 8,
            b'B' => 16,
            b'G' => 32,
            _ => return Err(What::Augmentation(shown())),
        };
        if seen & bit != 0 {
            return Err(What::Augmentation(shown()));
        }
        seen |= bit;
    }
    body.leb()?; // code alignment
    body.leb()?; // data alignment
    if version == 1 {
        body.u8()?;
    } else {
        body.leb()?;
    }
    let mut cie =
        Cie { class: 0, version, fde_enc: 0, lsda_enc: None, personality: None, signal: false, z };
    if !z {
        return Ok(cie);
    }
    let data_len = body.leb()?;
    let data_end =
        body.at.checked_add(usize::try_from(data_len).map_err(|_| What::CorruptedRecord)?);
    if data_end.is_none_or(|e| e > body.end) {
        return Err(What::CorruptedRecord);
    }
    for &c in aug.iter().skip(1) {
        match c {
            b'R' => {
                cie.fde_enc = body.u8()?;
                pointer_width(cie.fde_enc, Field::Location, word)?;
            }
            b'P' => {
                let enc = body.u8()?;
                if enc != OMIT {
                    let w = pointer_width(enc, Field::Personality, word)?;
                    let p = Pointer { enc, at: (body.at - at) as u32 };
                    pointer_reloc(relocs, body.at as u64, w, enc)?;
                    body.skip(u64::from(w))?;
                    cie.personality = Some(p);
                }
            }
            b'L' => {
                let enc = body.u8()?;
                if enc != OMIT {
                    pointer_width(enc, Field::Lsda, word)?;
                    cie.lsda_enc = Some(enc);
                }
            }
            b'S' => cie.signal = true,
            _ => {} // B, G: no data
        }
    }
    if data_end.is_some_and(|e| body.at > e) {
        return Err(What::CorruptedRecord);
    }
    Ok(cie)
}

/// S-37, S-38: an FDE's body after its CIE pointer.
fn parse_fde(
    body: &mut Body<'_>,
    relocs: &[EhReloc],
    at: usize,
    cie: u32,
    c: &Cie,
    word: u8,
) -> Result<Fde, What> {
    let w = pointer_width(c.fde_enc, Field::Location, word)?;
    let loc_at = body.at;
    let field = body.uint(w)?;
    let range = body.uint(w)?;
    let code = match pointer_reloc(relocs, loc_at as u64, w, c.fde_enc)? {
        None if field == 0 => Code::Dropped,
        None => return Err(What::FdeWithoutReloc),
        Some(r) => match r.target {
            Target::Section { section, value } => {
                Code::Section { section, offset: (value as i64).wrapping_add(r.addend) }
            }
            Target::Absolute(_) => Code::Absolute,
            Target::Undefined { .. } => Code::Undefined,
            Target::Elsewhere(_) => return Err(What::FdeCodeElsewhere),
        },
    };
    let mut lsda = None;
    if c.z {
        let data_len = body.leb()?;
        let data_at = body.at;
        if let Some(enc) = c.lsda_enc {
            let lw = pointer_width(enc, Field::Lsda, word)?;
            pointer_reloc(relocs, body.at as u64, lw, enc)?;
            lsda = Some(Pointer { enc, at: (body.at - at) as u32 });
            body.skip(u64::from(lw))?;
        }
        if ((body.at - data_at) as u64) > data_len {
            return Err(What::CorruptedRecord);
        }
        body.at = data_at;
        body.skip(data_len)?;
    }
    Ok(Fde { cie, code, range, lsda })
}

/// S-43: the identity of a CIE: its bytes and its relocations, each as
/// (offset in the record, type, addend, target).
type CieKey<'a> = (&'a [u8], Vec<(u32, u32, i64, Target)>);

/// S-34 to S-40, S-43, S-45: parses every input `.eh_frame` (`sections`
/// in `SectionId` order; `word` the address size, 4 or 8). Parsing is
/// parallel by section; the CIE classes are numbered by a sequential walk
/// in key order (there are few CIEs, S-45). The problems, one per broken
/// section, are returned sorted, and then nothing else is.
pub fn parse(pool: &Pool, word: u8, sections: &[EhIn<'_>]) -> Result<EhFrames, Vec<Problem>> {
    debug_assert!(sections.windows(2).all(|w| w[0].section < w[1].section));
    let parsed = pool.unit_cost(2200).map(sections.len(), |i| parse_section(&sections[i], word));
    let mut problems: Vec<Problem> =
        parsed.iter().filter_map(|p| p.as_ref().err().cloned()).collect();
    if !problems.is_empty() {
        problems.sort();
        return Err(problems);
    }
    let mut out_sections = Vec::with_capacity(sections.len());
    let mut records: Vec<Record> = Vec::new();
    for (s, p) in sections.iter().zip(parsed) {
        let Ok(p) = p else { unreachable!("problems returned above") };
        let start = u32::try_from(records.len()).ok();
        let len = p.records.len() as u32;
        let Some(start) = start.filter(|s| s.checked_add(len).is_some()) else {
            return Err(vec![Problem {
                file: s.file,
                section: s.index,
                piece: 0,
                offset: 0,
                what: What::TooManyPiecesInLink,
            }]);
        };
        out_sections.push(EhSection {
            section: s.section,
            size: s.bytes.len() as u32,
            records: Span { start, len },
            terminator: p.terminator,
        });
        records.extend(p.records);
    }
    // S-43 classes by first occurrence.
    let mut classes: HashMap<CieKey<'_>, u32> = HashMap::default();
    for (s, sec) in sections.iter().zip(&out_sections) {
        let r = sec.records;
        for rec in &mut records[r.start as usize..(r.start + r.len) as usize] {
            let (base, end, rs) = (u64::from(rec.offset), rec.end(), rec.relocs);
            let Kind::Cie(cie) = &mut rec.kind else { continue };
            let rel = &s.relocs[rs.start as usize..(rs.start + rs.len) as usize];
            let key: CieKey<'_> = (
                &s.bytes[base as usize..end as usize],
                rel.iter()
                    .filter(|r| r.class != RelocClass::None)
                    .map(|r| ((r.offset - base) as u32, r.r_type, r.addend, r.target))
                    .collect(),
            );
            let next = classes.len() as u32;
            cie.class = *classes.entry(key).or_insert(next);
        }
    }
    let classes = classes.len() as u32;
    Ok(EhFrames { sections: out_sections.into(), records: records.into(), classes })
}
