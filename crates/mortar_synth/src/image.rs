//! The chunk interface the image phase drives (`spec/image.md` §13,
//! I-103 to I-108; `spec/synth.md` S-4).
//!
//! Every synthetic chunk of M1 implements [`SynthChunk`] over its plan:
//!
//! | chunk | type | division (S-4) |
//! |---|---|---|
//! | `.eh_frame` | [`EhChunk`] | divisible (binary search over records) |
//! | `.eh_frame_hdr` | [`HdrChunk`] | whole, `12 + 8 × FDEs` |
//! | `.got`, `.got.plt`, `.plt`, `.rela.plt` | [`GotChunk`] | divisible (fixed slots and entries) |
//! | `.symtab`, `.strtab`, `.symtab_shndx` | [`SymtabChunk`] | divisible (checkpoints every 1024 symbols) |
//! | `.shstrtab` | [`ShstrtabPlan`] | whole, its size |
//! | `.note.gnu.build-id` | [`BuildIdPlan`] | whole, `16 + align_up(n, 4)`; sealed = the descriptor |
//! | `.note.gnu.property` | [`PropertyPlan`] | whole, `16 + Σ(8 + align_up(datasz, word))` |
//!
//! `write` fills `out` (exactly `range.len()` bytes) with bytes
//! `range` of the chunk; any split of `[0, size)` gives the same bytes
//! (I-103). A whole chunk writes into a buffer of its size and copies the
//! range out.

use std::marker::PhantomData;
use std::ops::Range;

use mortar_arch::Arch;
use mortar_arch::error::RefName;
use mortar_core::ChunkId;
use mortar_core::par::Pool;
use mortar_diag::FileRef;

use crate::eh::{CodeRef, EhPlan, EhSource};
use crate::error::{SynthError, internal};
use crate::got::{GotAddrs, GotPlan, SlotValues};
use crate::hdr::{HdrAddrs, HdrPlan};
use crate::notes::{BuildIdPlan, PropertyPlan};
use crate::out::Window;
use crate::shstrtab::ShstrtabPlan;
use crate::symtab::{SymAddrs, SymCtx, SymtabPlan};

/// How the image may split a chunk (I-104).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Division {
    /// `write` of any range costs O(range + log chunk + 2^10) and
    /// allocates nothing proportional to the chunk.
    Divisible,
    /// One job writes the whole chunk, at most `max_size` bytes.
    Whole { max_size: u64 },
}

/// What `size` may read. Every M1 chunk's size is fixed by its plan in
/// phase 80 (or, for `.symtab` with patches, its re-plan after layout's
/// fixed point, S-2), so M1 reads nothing here.
#[derive(Debug, Clone, Copy, Default)]
pub struct SizeCx<'a> {
    _tables: PhantomData<&'a ()>,
}

impl SizeCx<'_> {
    /// The context of M1.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// What `write` may read: the image phase's answers once addresses are
/// known.
#[derive(Clone, Copy)]
pub struct FillCx<'a> {
    pub arch: &'a dyn Arch,
    pub pool: &'a Pool,
    /// The output, as diagnostics name it.
    pub output: &'a FileRef,
    /// Slot values for `.got`, `.got.plt` and `.rela.plt`.
    pub slots: &'a dyn SlotValues,
    /// Symbol addresses for `.symtab` and `.symtab_shndx`.
    pub syms: &'a dyn SymAddrs,
    /// The GOT, `.got.plt` and `.plt` addresses (zero when absent).
    pub got: GotAddrs,
    /// `.eh_frame_hdr`'s and `.eh_frame`'s addresses.
    pub hdr: HdrAddrs,
    /// The output address of an FDE's initial location.
    pub code_addr: &'a (dyn Fn(CodeRef) -> Option<u64> + Sync),
}

impl std::fmt::Debug for FillCx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FillCx").field("got", &self.got).field("hdr", &self.hdr).finish_non_exhaustive()
    }
}

/// A synthetic chunk as the image drives it (`spec/image.md` §13).
pub trait SynthChunk {
    /// The size (S-108).
    fn size(&self, cx: &SizeCx<'_>) -> u64;
    /// The division (S-4, I-104).
    fn division(&self) -> Division;
    /// Bytes `range` of the chunk into `out` (`out.len() == range.len()`).
    ///
    /// # Errors
    ///
    /// `relocation-out-of-range` or `relocation-misaligned` of a field,
    /// or `internal-error` (I-106); the image keys it `(100, 0, ChunkId)`.
    fn write(&self, cx: &FillCx<'_>, range: Range<u64>, out: &mut [u8]) -> Result<(), SynthError>;
    /// The range the sealing pass writes (I-107): the build id's
    /// descriptor.
    fn sealed(&self) -> Option<Range<u64>> {
        None
    }
}

fn check(range: &Range<u64>, out: &[u8], size: u64) -> Result<(), SynthError> {
    if range.start > range.end || range.end > size || out.len() as u64 != range.end - range.start {
        return Err(internal(format!(
            "a synthetic chunk of {size} bytes written at {}..{} into {} bytes",
            range.start,
            range.end,
            out.len()
        )));
    }
    Ok(())
}

/// A whole chunk: `fill` writes all of it into a buffer of `size`
/// bytes, and the range is copied out (no buffer when the range is the
/// chunk).
fn whole(
    range: &Range<u64>,
    out: &mut [u8],
    size: u64,
    fill: impl FnOnce(&mut [u8]) -> Result<(), SynthError>,
) -> Result<(), SynthError> {
    check(range, out, size)?;
    if range.start == 0 && range.end == size {
        return fill(out);
    }
    let mut buf = vec![0u8; size as usize];
    fill(&mut buf)?;
    out.copy_from_slice(&buf[range.start as usize..range.end as usize]);
    Ok(())
}

/// `.eh_frame` (S-46 to S-48) over its plan and the parsed input.
#[derive(Clone, Copy)]
pub struct EhChunk<'a> {
    pub plan: &'a EhPlan,
    pub src: &'a dyn EhSource,
}

impl std::fmt::Debug for EhChunk<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EhChunk").field("plan", self.plan).finish_non_exhaustive()
    }
}

impl SynthChunk for EhChunk<'_> {
    fn size(&self, _: &SizeCx<'_>) -> u64 {
        self.plan.size()
    }

    fn division(&self) -> Division {
        Division::Divisible
    }

    fn write(&self, _: &FillCx<'_>, range: Range<u64>, out: &mut [u8]) -> Result<(), SynthError> {
        check(&range, out, self.plan.size())?;
        self.plan.write(self.src, &mut Window::new(range.start, out))
    }
}

/// `.eh_frame_hdr` (S-57 to S-62).
#[derive(Clone, Copy)]
pub struct HdrChunk<'a> {
    pub plan: &'a HdrPlan,
    pub eh: &'a EhPlan,
    pub src: &'a dyn EhSource,
}

impl std::fmt::Debug for HdrChunk<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HdrChunk").field("plan", self.plan).finish_non_exhaustive()
    }
}

impl SynthChunk for HdrChunk<'_> {
    fn size(&self, _: &SizeCx<'_>) -> u64 {
        self.plan.size()
    }

    /// Whole: the table is sorted by address, which only the whole
    /// table knows (I-105's "global order"), bounded by 8 bytes per live
    /// FDE.
    fn division(&self) -> Division {
        Division::Whole { max_size: 12 + 8 * u64::from(self.eh.fde_count()) }
    }

    fn write(&self, cx: &FillCx<'_>, range: Range<u64>, out: &mut [u8]) -> Result<(), SynthError> {
        whole(&range, out, self.plan.size(), |buf| {
            self.plan.write(self.src, self.eh, cx.hdr, cx.code_addr, cx.pool, buf)
        })
    }
}

/// Which of the GOT plan's chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GotPart {
    Got,
    GotPlt,
    /// The IPLT entries of `.plt`.
    Plt,
    RelaPlt,
}

/// `.got`, `.got.plt`, `.plt` or `.rela.plt` (S-63 to S-72).
#[derive(Debug, Clone, Copy)]
pub struct GotChunk<'a> {
    pub plan: &'a GotPlan,
    pub part: GotPart,
}

impl SynthChunk for GotChunk<'_> {
    fn size(&self, _: &SizeCx<'_>) -> u64 {
        match self.part {
            GotPart::Got => self.plan.got_size(),
            GotPart::GotPlt => self.plan.got_plt_size(),
            GotPart::Plt => self.plan.plt_size(),
            GotPart::RelaPlt => self.plan.rela_plt_size(),
        }
    }

    fn division(&self) -> Division {
        Division::Divisible
    }

    fn write(&self, cx: &FillCx<'_>, range: Range<u64>, out: &mut [u8]) -> Result<(), SynthError> {
        check(&range, out, self.size(&SizeCx::new()))?;
        let start = range.start;
        let mut w = Window::new(start, out);
        match self.part {
            GotPart::Got => self.plan.write_got(&cx.got, cx.slots, &mut w),
            GotPart::GotPlt => self.plan.write_got_plt(cx.slots, &mut w),
            GotPart::RelaPlt => self.plan.write_rela_plt(&cx.got, cx.slots, &mut w),
            GotPart::Plt => self.plan.write_plt(cx.arch, &cx.got, &mut w).map_err(|(n, e)| {
                let (_, d) = mortar_arch::diag::chunk(
                    e,
                    ChunkId(0),
                    cx.output,
                    ".plt",
                    self.plan.iplt_entry(n),
                    RefName::Named("IPLT entry"),
                    "",
                );
                SynthError::from(d)
            }),
        }
    }
}

/// Which of the symbol tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymTable {
    Symtab,
    Strtab,
    SymtabShndx,
}

/// `.symtab`, `.strtab` or `.symtab_shndx` (S-73 to S-89).
#[derive(Debug, Clone, Copy)]
pub struct SymtabChunk<'a> {
    pub plan: &'a SymtabPlan,
    pub cx: SymCtx<'a>,
    pub table: SymTable,
}

impl SynthChunk for SymtabChunk<'_> {
    fn size(&self, _: &SizeCx<'_>) -> u64 {
        match self.table {
            SymTable::Symtab => self.plan.symtab_size(),
            SymTable::Strtab => self.plan.strtab_size(),
            SymTable::SymtabShndx => self.plan.shndx_size(),
        }
    }

    fn division(&self) -> Division {
        Division::Divisible
    }

    fn write(&self, cx: &FillCx<'_>, range: Range<u64>, out: &mut [u8]) -> Result<(), SynthError> {
        check(&range, out, self.size(&SizeCx::new()))?;
        let mut w = Window::new(range.start, out);
        match self.table {
            SymTable::Symtab => self.plan.write_symtab(&self.cx, cx.syms, &mut w),
            SymTable::Strtab => self.plan.write_strtab(&self.cx, &mut w),
            SymTable::SymtabShndx => self.plan.write_shndx(&self.cx, cx.syms, &mut w),
        }
    }
}

impl SynthChunk for ShstrtabPlan {
    fn size(&self, _: &SizeCx<'_>) -> u64 {
        ShstrtabPlan::size(self)
    }

    fn division(&self) -> Division {
        Division::Whole { max_size: ShstrtabPlan::size(self) }
    }

    fn write(&self, _: &FillCx<'_>, range: Range<u64>, out: &mut [u8]) -> Result<(), SynthError> {
        check(&range, out, ShstrtabPlan::size(self))?;
        out.copy_from_slice(&self.bytes()[range.start as usize..range.end as usize]);
        Ok(())
    }
}

impl SynthChunk for BuildIdPlan {
    fn size(&self, _: &SizeCx<'_>) -> u64 {
        BuildIdPlan::size(self)
    }

    fn division(&self) -> Division {
        Division::Whole { max_size: BuildIdPlan::size(self) }
    }

    fn write(&self, _: &FillCx<'_>, range: Range<u64>, out: &mut [u8]) -> Result<(), SynthError> {
        check(&range, out, BuildIdPlan::size(self))?;
        BuildIdPlan::write(self, &mut Window::new(range.start, out));
        Ok(())
    }

    /// I-107: the descriptor, unless `0xHEX` gave it.
    fn sealed(&self) -> Option<Range<u64>> {
        self.hashed().then(|| self.desc_range())
    }
}

impl SynthChunk for PropertyPlan {
    fn size(&self, _: &SizeCx<'_>) -> u64 {
        PropertyPlan::size(self)
    }

    fn division(&self) -> Division {
        Division::Whole { max_size: PropertyPlan::size(self) }
    }

    fn write(&self, _: &FillCx<'_>, range: Range<u64>, out: &mut [u8]) -> Result<(), SynthError> {
        check(&range, out, PropertyPlan::size(self))?;
        PropertyPlan::write(self, &mut Window::new(range.start, out));
        Ok(())
    }
}
