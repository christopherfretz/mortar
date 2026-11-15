//! The synthetic chunk list (S-3) and `Slots.sizes` (S-108).
//!
//! model §4.4 gives `ChunkId`s to the synthetic chunks after the headers
//! "in the order synth.md lists them"; a chunk that does not exist gets no
//! id. [`chunk_ids`] is that numbering.

use mortar_core::ChunkId;
use mortar_core::layout::Synth;
use mortar_elf::Class;
use mortar_elf::consts::{
    SHF_ALLOC, SHF_EXECINSTR, SHF_INFO_LINK, SHF_WRITE, SHT_NOTE, SHT_PROGBITS, SHT_REL,
    SHT_RELA, SHT_STRTAB, SHT_SYMTAB, SHT_SYMTAB_SHNDX,
};

/// A synthetic chunk of S-3, by its number there. The M2 chunks are named
/// so the numbering stays S-3's; M1 never creates them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum SynthKind {
    Interp = 1,
    NoteGnuProperty = 2,
    NoteGnuBuildId = 3,
    GnuHash = 4,
    Dynsym = 5,
    Dynstr = 6,
    GnuVersion = 7,
    GnuVersionD = 8,
    GnuVersionR = 9,
    RelaDyn = 10,
    RelrDyn = 11,
    RelaPlt = 12,
    EhFrameHdr = 13,
    EhFrame = 14,
    Plt = 15,
    Got = 16,
    GotPlt = 17,
    Dynamic = 18,
    Symtab = 19,
    SymtabShndx = 20,
    Strtab = 21,
    Shstrtab = 22,
}

/// The section header fields of a synthetic chunk that do not depend on
/// its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub name: &'static [u8],
    pub sh_type: u32,
    pub flags: u64,
    /// Bytes, a power of two.
    pub align: u64,
    pub entsize: u64,
}

impl SynthKind {
    /// Its number in S-3.
    #[must_use]
    pub fn number(self) -> u8 {
        self as u8
    }

    /// The model's `Synth` row (model §4.4): only `.relr.dyn` is
    /// address-dependent (S-2).
    #[must_use]
    pub fn synth(self, rel: bool) -> Synth {
        Synth { name: self.name(rel), address_dependent: self == Self::RelrDyn }
    }

    /// The section name; `rel` for an output whose relocations are REL
    /// (i386: `.rel.plt`, S-68).
    #[must_use]
    pub fn name(self, rel: bool) -> &'static [u8] {
        match self {
            Self::Interp => b".interp",
            Self::NoteGnuProperty => b".note.gnu.property",
            Self::NoteGnuBuildId => b".note.gnu.build-id",
            Self::GnuHash => b".gnu.hash",
            Self::Dynsym => b".dynsym",
            Self::Dynstr => b".dynstr",
            Self::GnuVersion => b".gnu.version",
            Self::GnuVersionD => b".gnu.version_d",
            Self::GnuVersionR => b".gnu.version_r",
            Self::RelaDyn if rel => b".rel.dyn",
            Self::RelaDyn => b".rela.dyn",
            Self::RelrDyn => b".relr.dyn",
            Self::RelaPlt if rel => b".rel.plt",
            Self::RelaPlt => b".rela.plt",
            Self::EhFrameHdr => b".eh_frame_hdr",
            Self::EhFrame => b".eh_frame",
            Self::Plt => b".plt",
            Self::Got => b".got",
            Self::GotPlt => b".got.plt",
            Self::Dynamic => b".dynamic",
            Self::Symtab => b".symtab",
            Self::SymtabShndx => b".symtab_shndx",
            Self::Strtab => b".strtab",
            Self::Shstrtab => b".shstrtab",
        }
    }

    /// The header fields of an M1 chunk, for an output of `class` whose
    /// relocations are REL when `rel`. `None` for the M2 chunks, and for
    /// the chunks whose type or alignment comes from their plan
    /// (`.eh_frame`: [`crate::EhPlan::sh_type`] and
    /// [`crate::EhPlan::align`]; `.plt`: the PLT shape's alignment).
    #[must_use]
    pub fn header(self, class: Class, rel: bool) -> Option<Header> {
        let word = class.word_size() as u64;
        let h = |sh_type, flags, align, entsize| {
            Some(Header { name: self.name(rel), sh_type, flags, align, entsize })
        };
        match self {
            // S-96: alignment 8 (ELF64) or 4 (ELF32).
            Self::NoteGnuProperty => h(SHT_NOTE, SHF_ALLOC, word, 0),
            // S-91: alignment 4.
            Self::NoteGnuBuildId => h(SHT_NOTE, SHF_ALLOC, 4, 0),
            // S-69: SHF_INFO_LINK, sh_info `.got.plt`.
            Self::RelaPlt => {
                let (t, e) = match (rel, class) {
                    (true, Class::Elf32) => (SHT_REL, 8),
                    (true, Class::Elf64) => (SHT_REL, 16),
                    (false, Class::Elf32) => (SHT_RELA, 12),
                    (false, Class::Elf64) => (SHT_RELA, 24),
                };
                h(t, SHF_ALLOC | SHF_INFO_LINK, word, e)
            }
            // S-58: alignment 4.
            Self::EhFrameHdr => h(SHT_PROGBITS, SHF_ALLOC, 4, 0),
            Self::Got | Self::GotPlt => h(SHT_PROGBITS, SHF_ALLOC | SHF_WRITE, word, word),
            // S-73.
            Self::Symtab => h(SHT_SYMTAB, 0, word, if word == 8 { 24 } else { 16 }),
            // S-85.
            Self::SymtabShndx => h(SHT_SYMTAB_SHNDX, 0, 4, 4),
            Self::Strtab | Self::Shstrtab => h(SHT_STRTAB, 0, 1, 0),
            Self::Plt => h(SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR, 16, 0),
            Self::EhFrame => h(SHT_PROGBITS, SHF_ALLOC, word, 0),
            _ => None,
        }
    }
}

/// `Slots.sizes` (S-108) for the chunks that exist, `present` in any
/// order: each gets the next `ChunkId` from `first` (the first id after
/// the headers, model §4.4) in S-3 order. Fails (`None`) when a chunk is
/// given twice.
#[must_use]
pub fn chunk_ids(first: ChunkId, present: &[(SynthKind, u64)]) -> Option<Box<[(ChunkId, u64)]>> {
    let mut sorted = present.to_vec();
    sorted.sort_by_key(|row| row.0);
    if sorted.windows(2).any(|w| w[0].0 == w[1].0) {
        return None;
    }
    Some(
        sorted
            .iter()
            .enumerate()
            .map(|(i, &(_, size))| (ChunkId(first.0 + i as u32), size))
            .collect(),
    )
}
