//! The synthetic chunk list (S-3) and `Slots.sizes` (S-108).
//!
//! model §4.4 gives `ChunkId`s to the synthetic chunks after the headers
//! "in the order synth.md lists them"; a chunk that does not exist gets no
//! id. [`chunk_ids`] is that numbering.

use mortar_core::ChunkId;
use mortar_core::layout::Synth;
use mortar_elf::Class;
use mortar_elf::consts::{
    SHF_ALLOC, SHF_EXECINSTR, SHF_INFO_LINK, SHF_MERGE, SHF_STRINGS, SHF_WRITE, SHT_DYNAMIC,
    SHT_DYNSYM, SHT_GNU_HASH, SHT_GNU_VERDEF, SHT_GNU_VERNEED, SHT_GNU_VERSYM, SHT_NOTE,
    SHT_PROGBITS, SHT_REL, SHT_RELA, SHT_STRTAB, SHT_SYMTAB, SHT_SYMTAB_SHNDX,
};

/// A synthetic chunk of S-3, by its number there. The M2 chunks are named
/// so the numbering stays S-3's; M1 never creates them, and a static link
/// never does (`spec/dyn.md` §1: they exist only in dynamic output).
///
/// Each M2 row names the file of the package that plans its content
/// (`spec/landed/dyn/WP-040.md`); after WP-041 nobody else edits this
/// list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum SynthKind {
    /// M2: `mortar_synth/src/interp.rs` (WP-M2-DYNAMIC; D-30, D-31).
    Interp = 1,
    NoteGnuProperty = 2,
    NoteGnuBuildId = 3,
    /// M2: `mortar_synth/src/gnuhash.rs` (WP-M2-DYNSYM; D-26).
    GnuHash = 4,
    /// M2: `mortar_synth/src/dynsym.rs` (WP-M2-DYNSYM; D-21 to D-23).
    Dynsym = 5,
    /// M2: `mortar_synth/src/dynstr.rs` (WP-M2-DYNSYM; D-24).
    Dynstr = 6,
    /// M2: `mortar_synth/src/version.rs` (WP-M2-DYNSYM; D-27).
    GnuVersion = 7,
    /// M4: `mortar_synth/src/version.rs` (WP-125; D-115): a shared object
    /// whose version script names a version node.
    GnuVersionD = 8,
    /// M2: `mortar_synth/src/version.rs` (WP-M2-DYNSYM; D-27).
    GnuVersionR = 9,
    /// M2: `mortar_synth/src/rela.rs` (WP-M2-RELOC; D-40, D-42); the
    /// records' bytes `mortar_image/src/dynrel.rs`.
    RelaDyn = 10,
    /// M4 (`-z pack-relative-relocs`); never created at M2.
    RelrDyn = 11,
    /// M1 (IPLT); M2 `JUMP_SLOT`s: `mortar_synth/src/got.rs` (WP-M2-RELOC;
    /// D-41).
    RelaPlt = 12,
    EhFrameHdr = 13,
    EhFrame = 14,
    Plt = 15,
    Got = 16,
    GotPlt = 17,
    /// M2: `mortar_synth/src/dynamic.rs`, values `mortar_image/src/dynamic.rs`
    /// (WP-M2-DYNAMIC; D-30).
    Dynamic = 18,
    Symtab = 19,
    SymtabShndx = 20,
    Strtab = 21,
    Shstrtab = 22,
    Comment = 23,
    /// M2: `.dynbss`, the copy allocations of [`mortar_core::CopyPlace::DynBss`]
    /// (output section `.bss`, its first chunk): sized by
    /// `mortar_synth/src/rela.rs` (WP-M2-RELOC; D-42), placed by
    /// `mortar_layout` (WP-M2-LAYOUT; D-60). After `.comment` so every M1
    /// chunk keeps its id.
    DynBss = 24,
    /// M2: the copy chunk of `.bss.rel.ro`, the allocations of
    /// [`mortar_core::CopyPlace::RelRo`] (inside RELRO): as [`Self::DynBss`]
    /// (D-42, D-60).
    BssRelRo = 25,
    /// WP-075: `.note.mason`, the one-definition-rule table of a dynamic
    /// output (`spec/odr.md` O-50 to O-52; `--odr-table`), planned by the
    /// front (`mortar/src/front.rs`, from `mortar_odr`'s rows) as
    /// [`crate::NoteMasonPlan`]. After `.bss.rel.ro` so every chunk keeps
    /// its id.
    NoteMason = 26,
    /// WP-076: `.mason.meta`, the live inputs' Mason units kept for tools
    /// (`spec/meta.md` MT-32; `--keep-mason-meta`), non-allocated, planned
    /// by the front from `mortar_odr::kept_units` as
    /// [`crate::MasonMetaPlan`]. After `.note.mason` so every chunk keeps
    /// its id.
    MasonMeta = 27,
    /// WP-097: `.gdb_index` (`spec/synth.md` §17; `--gdb-index`),
    /// non-allocated, planned by the front from `mortar_synth::gdbread` as
    /// [`crate::GdbIndexPlan`]. Last, so every chunk keeps its id.
    GdbIndex = 28,
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
            Self::Comment => b".comment",
            Self::DynBss => b".dynbss",
            Self::BssRelRo => b".bss.rel.ro",
            Self::NoteMason => b".note.mason",
            Self::MasonMeta => b".mason.meta",
            Self::GdbIndex => b".gdb_index",
        }
    }

    /// The header fields of a chunk, for an output of `class` whose
    /// relocations are REL when `rel` (M2 rows: `spec/dyn.md` D-34; their
    /// `sh_link`/`sh_info` are the image's). `None` for the M4 chunks and
    /// for the chunks whose type or alignment comes from their plan
    /// (`.eh_frame`: [`crate::EhPlan::sh_type`] and
    /// [`crate::EhPlan::align`]; `.plt`: the PLT shape's alignment;
    /// `.dynbss` and `.bss.rel.ro`: their allocations' alignment, D-42).
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
            // odr.md O-52: alignment 8 in both classes.
            Self::NoteMason => h(SHT_NOTE, SHF_ALLOC, 8, 0),
            // meta.md MT-32: `SHT_PROGBITS`, no flags, alignment 8 (MT-1's,
            // without `SHF_EXCLUDE`).
            Self::MasonMeta => h(SHT_PROGBITS, 0, 8, 0),
            // S-G1: `SHT_PROGBITS`, no flags, alignment 4.
            Self::GdbIndex => h(SHT_PROGBITS, 0, 4, 0),
            // S-69: SHF_INFO_LINK, sh_info `.got.plt`.
            Self::RelaPlt => {
                let (t, e) = rel_form(class, rel);
                h(t, SHF_ALLOC | SHF_INFO_LINK, word, e)
            }
            // D-34: sh_link `.dynsym`.
            Self::RelaDyn => {
                let (t, e) = rel_form(class, rel);
                h(t, SHF_ALLOC, word, e)
            }
            // D-34.
            Self::Interp => h(SHT_PROGBITS, SHF_ALLOC, 1, 0),
            Self::GnuHash => h(SHT_GNU_HASH, SHF_ALLOC, word, if word == 8 { 0 } else { 4 }),
            Self::Dynsym => h(SHT_DYNSYM, SHF_ALLOC, word, if word == 8 { 24 } else { 16 }),
            Self::Dynstr => h(SHT_STRTAB, SHF_ALLOC, 1, 0),
            Self::GnuVersion => h(SHT_GNU_VERSYM, SHF_ALLOC, 2, 2),
            // D-115: sh_link `.dynstr`, sh_info the `Verdef` count.
            Self::GnuVersionD => h(SHT_GNU_VERDEF, SHF_ALLOC, 4, 0),
            Self::GnuVersionR => h(SHT_GNU_VERNEED, SHF_ALLOC, 4, 0),
            Self::Dynamic => h(SHT_DYNAMIC, SHF_ALLOC | SHF_WRITE, word, 2 * word),
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
            // S-100: the inputs' `.comment` flags.
            Self::Comment => h(SHT_PROGBITS, SHF_MERGE | SHF_STRINGS, 1, 1),
            _ => None,
        }
    }
}

/// The type and entry size of a relocation section: REL or RELA, by class.
fn rel_form(class: Class, rel: bool) -> (u32, u64) {
    match (rel, class) {
        (true, Class::Elf32) => (SHT_REL, 8),
        (true, Class::Elf64) => (SHT_REL, 16),
        (false, Class::Elf32) => (SHT_RELA, 12),
        (false, Class::Elf64) => (SHT_RELA, 24),
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

#[cfg(test)]
mod tests {
    use super::*;

    // S-3 and dyn.md D-34: the M2 rows have their names and headers; the
    // copy chunks come after `.comment`, so a static link's chunks keep
    // their ids (a chunk that does not exist gets none).
    #[test]
    fn the_m2_rows() {
        let h = |k: SynthKind, class| k.header(class, false).unwrap();
        assert_eq!(h(SynthKind::Dynsym, Class::Elf64).entsize, 24);
        assert_eq!(h(SynthKind::Dynsym, Class::Elf32).entsize, 16);
        assert_eq!(h(SynthKind::GnuHash, Class::Elf32).entsize, 4);
        assert_eq!(h(SynthKind::GnuVersionR, Class::Elf64).align, 4);
        assert_eq!(h(SynthKind::Dynamic, Class::Elf64).flags, SHF_ALLOC | SHF_WRITE);
        assert_eq!(SynthKind::RelaDyn.header(Class::Elf32, true).unwrap().sh_type, SHT_REL);
        assert_eq!(h(SynthKind::Interp, Class::Elf64).align, 1);
        assert_eq!(h(SynthKind::GnuVersionD, Class::Elf64).align, 4);
        for m4 in [SynthKind::RelrDyn, SynthKind::DynBss] {
            assert!(m4.header(Class::Elf64, false).is_none());
        }
        assert_eq!(SynthKind::BssRelRo.name(false), b".bss.rel.ro");
        // meta.md MT-32 (WP-076): `.mason.meta` is last, non-allocated.
        let m = h(SynthKind::MasonMeta, Class::Elf32);
        assert_eq!(
            (m.name, m.sh_type, m.flags, m.align),
            (&b".mason.meta"[..], SHT_PROGBITS, 0, 8)
        );
        assert!(SynthKind::MasonMeta > SynthKind::NoteMason);
        // synth.md S-G1 (WP-097): `.gdb_index` is row 28, non-allocated.
        let g = h(SynthKind::GdbIndex, Class::Elf64);
        assert_eq!((g.name, g.sh_type, g.flags, g.align), (&b".gdb_index"[..], SHT_PROGBITS, 0, 4));
        assert_eq!(SynthKind::GdbIndex.number(), 28);
        assert!(SynthKind::DynBss > SynthKind::Comment && SynthKind::BssRelRo > SynthKind::DynBss);
        let ids = chunk_ids(ChunkId(4), &[(SynthKind::Comment, 9), (SynthKind::Shstrtab, 3)]);
        assert_eq!(ids.unwrap()[..], [(ChunkId(4), 3), (ChunkId(5), 9)]);
    }
}
