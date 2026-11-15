//! The decision tables (`spec/model.md` §4): `Folds`, `MergeMap`, `Slots`
//! and `Placement`, together [`Layout`]; and what is derived from them
//! (§4.5). Validity, [`Layout::check`], is in `check.rs`.
//!
//! `Slots` and `patches` are written as the model states them today;
//! `spec/arch.md` and `spec/synth.md` confirm or amend them.

use mortar_elf::{SectionIndex, SymbolSection};

use crate::assignment::Assignment;
use crate::column::Column;
use crate::id::{
    ChunkId, FileId, FragId, OptId, OutSecId, SectionId, SegmentId, Span, SymEntry, SymKey,
    SymbolId,
};
use crate::inputs::{Inputs, Reader};
use crate::resolution::{Def, Resolution, SectionKind};
use crate::split::Split;

// ---- §4.1 -----------------------------------------------------------------

/// Identical code folding (§4.1): the section a folded one is replaced by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Folds {
    /// `None` when folding is off (no column at all, §9).
    pub folded: Option<Column<SectionId, OptId<SectionId>>>,
}

impl Folds {
    /// The section `section` is replaced by, if it is folded.
    #[must_use]
    pub fn target(&self, section: SectionId) -> Option<SectionId> {
        self.folded.as_ref()?.get(section)?.get()
    }

    #[must_use]
    pub fn is_folded(&self, section: SectionId) -> bool {
        self.target(section).is_some()
    }
}

// ---- §4.2 -----------------------------------------------------------------

/// The offset of each live fragment in its merged chunk (§4.2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeMap {
    /// [`MergeMap::NONE`] for a dead fragment.
    pub frag_off: Column<FragId, u64>,
}

impl MergeMap {
    /// The offset of a fragment that is not placed.
    pub const NONE: u64 = u64::MAX;

    /// The offset of a placed fragment.
    #[must_use]
    pub fn offset(&self, frag: FragId) -> Option<u64> {
        self.frag_off.get(frag).copied().filter(|&off| off != Self::NONE)
    }
}

// ---- §4.3 -----------------------------------------------------------------

/// A live `.eh_frame` record: a piece of an `EhFrame` section by its
/// ordinal there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EhRecord {
    pub section: SectionId,
    pub piece: u32,
}

/// A copy-relocation allocation: one per `(shared FileId, st_value)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CopyAlloc {
    pub file: FileId,
    pub value: u64,
}

/// The content order of the synthetic tables whose size does not depend
/// on addresses (§4.3). Each list's position is the slot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Slots {
    /// GOT slots (one per `GOT` demand).
    pub got: Box<[SymKey]>,
    /// PLT entries (one per `PLT` or `CANONICAL_PLT` demand).
    pub plt: Box<[SymKey]>,
    /// TLS descriptors.
    pub tls_desc: Box<[SymKey]>,
    /// TLS module/offset pairs.
    pub tls_gd: Box<[SymKey]>,
    /// Thread-pointer offset slots.
    pub tls_ie: Box<[SymKey]>,
    /// `.dynsym`, after the null entry (one per `EXPORT` demand).
    pub dynsym: Box<[SymbolId]>,
    /// The order of the sections whose dynamic relocations are emitted.
    pub dyn_relocs: Box<[SectionId]>,
    /// `DT_NEEDED` entries.
    pub needed: Box<[FileId]>,
    /// The live `.eh_frame` records after deduplication of CIEs, in output
    /// order.
    pub eh_records: Box<[EhRecord]>,
    /// Copy-relocation allocations, sorted.
    pub copies: Box<[CopyAlloc]>,
    /// The allocation of every key that has one, sorted by key.
    pub copy_of: Box<[(SymbolId, u32)]>,
    /// Per file, its locals in `.symtab`: `(first local, count)` (§4.3).
    pub symtab_locals: Column<FileId, Span>,
    /// The size each synthetic chunk sized here has, sorted by chunk.
    pub sizes: Box<[(ChunkId, u64)]>,
}

/// An inverse of one slot list: key to slot.
#[derive(Debug, Clone, Default)]
pub struct SlotIndex {
    sorted: Box<[(SymKey, u32)]>,
}

impl SlotIndex {
    /// The index of `slots` (a list of `Slots`).
    #[must_use]
    pub fn new(slots: &[SymKey]) -> Self {
        let mut sorted: Vec<(SymKey, u32)> =
            slots.iter().enumerate().map(|(i, &key)| (key, i as u32)).collect();
        sorted.sort_unstable();
        Self { sorted: sorted.into() }
    }

    /// The slot of `key`.
    #[must_use]
    pub fn find(&self, key: SymKey) -> Option<u32> {
        let at = self.sorted.binary_search_by_key(&key, |row| row.0).ok()?;
        Some(self.sorted[at].1)
    }
}

// ---- §4.4 -----------------------------------------------------------------

/// A synthetic chunk (`spec/synth.md` lists them and their names).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Synth {
    /// The section name: `.got`, `.dynsym`, `.relr.dyn`, ...
    pub name: &'static [u8],
    /// Its content depends on addresses, so its size is in `sized`
    /// (`.relr.dyn`).
    pub address_dependent: bool,
}

/// What a chunk is (§4.4 `chunks`: kind).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChunkKind {
    FileHeader,
    ProgramHeaders,
    SectionHeaders,
    /// The body of an output section: its placed input sections.
    Body(OutSecId),
    /// The merged fragments of an output section.
    Merged(OutSecId),
    /// The patches (thunks, erratum patches) the linker adds to an output
    /// section.
    Patches(OutSecId),
    Synthetic(Synth),
    /// `-r`: the group of an output section.
    Group(OutSecId),
    /// `-r`: the relocations of an output section.
    Relocs(OutSecId),
}

impl ChunkKind {
    /// The output section the chunk is part of or about, if any.
    #[must_use]
    pub fn out(self) -> Option<OutSecId> {
        match self {
            Self::Body(o)
            | Self::Merged(o)
            | Self::Patches(o)
            | Self::Group(o)
            | Self::Relocs(o) => Some(o),
            Self::FileHeader | Self::ProgramHeaders | Self::SectionHeaders | Self::Synthetic(_) => {
                None
            }
        }
    }
}

/// One chunk (§4.4 `chunks[ChunkId]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk {
    pub kind: ChunkKind,
    pub sh_type: u32,
    pub flags: u64,
    /// log2 of the alignment.
    pub align: u8,
    pub size: u64,
    /// `≥ size`; the rest is slack.
    pub capacity: u64,
    /// The address; `None` exactly for a non-alloc chunk.
    pub addr: Option<u64>,
    /// The file offset.
    pub offset: u64,
    /// The index of the output section header the chunk is part of; `None`
    /// for the headers.
    pub shndx: Option<u32>,
    /// Part of the RELRO run.
    pub relro: bool,
}

impl Chunk {
    /// Whether the chunk is `SHF_ALLOC`.
    #[must_use]
    pub fn is_alloc(&self) -> bool {
        self.flags & mortar_elf::consts::SHF_ALLOC != 0
    }

    /// Whether the chunk occupies bytes of the file (not `SHT_NOBITS`).
    #[must_use]
    pub fn in_file(&self) -> bool {
        self.sh_type != mortar_elf::consts::SHT_NOBITS
    }

    /// Whether the chunk takes address space of the image: alloc, not
    /// empty, and not TLS `NOBITS` (`.tbss` has addresses in the TLS
    /// template only, and may overlap what follows it).
    #[must_use]
    pub fn takes_memory(&self) -> bool {
        self.is_alloc() && self.capacity > 0 && (self.in_file() || !self.is_tls())
    }

    /// Whether the chunk is part of the TLS template.
    #[must_use]
    pub fn is_tls(&self) -> bool {
        self.flags & mortar_elf::consts::SHF_TLS != 0
    }

    /// The chunk's name: its output section's, a synthetic chunk's own, none
    /// for the headers. (For `Group` and `Relocs`, the name of the output
    /// section they are about; the section header's name is
    /// `spec/layout.md`'s.)
    #[must_use]
    pub fn name<'n>(&self, assignment: &Assignment<'n>) -> Option<&'n [u8]> {
        match self.kind {
            ChunkKind::Synthetic(synth) => Some(synth.name),
            kind => kind.out().and_then(|o| assignment.outsecs.get(o)).map(|o| o.name),
        }
    }
}

/// One program header (§4.4 `segments[SegmentId]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub p_type: u32,
    pub flags: u32,
    pub offset: u64,
    pub addr: u64,
    pub filesz: u64,
    pub memsz: u64,
    /// log2 of the alignment.
    pub align: u8,
}

/// Where a section is (§4.4 `place`): `(ChunkId, offset in chunk)` in 12
/// bytes (§9), or none.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Place {
    chunk: u32,
    lo: u32,
    hi: u32,
}

impl Place {
    /// No place.
    pub const NONE: Self = Self { chunk: u32::MAX, lo: 0, hi: 0 };

    /// `offset` bytes into `chunk`.
    #[must_use]
    pub fn at(chunk: ChunkId, offset: u64) -> Self {
        debug_assert!(chunk.0 != u32::MAX);
        Self { chunk: chunk.0, lo: offset as u32, hi: (offset >> 32) as u32 }
    }

    /// The chunk and offset, if placed.
    #[must_use]
    pub fn get(self) -> Option<(ChunkId, u64)> {
        (self.chunk != u32::MAX)
            .then(|| (ChunkId(self.chunk), u64::from(self.hi) << 32 | u64::from(self.lo)))
    }
}

impl std::fmt::Debug for Place {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.get().fmt(f)
    }
}

/// What a symbol-anchored allocation is for (§4.4 `sym_place`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AllocKey {
    /// A common symbol.
    Common(SymbolId),
    /// A copy-relocation allocation: its position in `Slots::copies`.
    Copy(u32),
}

/// A symbol-anchored allocation (§4.4 `sym_place`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymPlace {
    pub key: AllocKey,
    pub chunk: ChunkId,
    pub offset: u64,
    pub size: u64,
    /// log2 of the alignment: a common's, or the shared object symbol's.
    pub align: u8,
}

/// The value of a linker-defined symbol (§4.4 `defined`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefValue {
    Absolute(u64),
    /// An offset in a chunk; `offset == size` is the chunk's end.
    At {
        chunk: ChunkId,
        offset: u64,
    },
}

/// A linker-defined symbol and its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Defined {
    pub symbol: SymbolId,
    pub value: DefValue,
}

/// Where a range-extension thunk goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThunkTarget {
    Sym { key: SymKey, addend: i64 },
    Plt(SymKey),
}

/// What a patch is (§4.4 `patches`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchKind {
    Thunk(ThunkTarget),
    /// The AArch64 erratum 843419 patch of the instruction that redirects
    /// to it.
    Erratum843419,
}

/// Code the linker adds because of addresses (§4.4 `patches`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Patch {
    /// A `Patches` chunk.
    pub chunk: ChunkId,
    pub offset: u64,
    pub kind: PatchKind,
}

/// A site that goes through a patch: `(SectionId, offset in section) →
/// patch` (the patch's position in `patches`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Redirect {
    pub section: SectionId,
    pub offset: u64,
    pub patch: u32,
}

/// The size of a chunk whose content depends on addresses (§4.4 `sized`).
/// The encoding inputs `spec/synth.md` will name join this row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sized {
    pub chunk: ChunkId,
    pub size: u64,
}

/// The TLS template (§4.4 `tls`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tls {
    pub start: u64,
    pub size: u64,
    /// log2 of the alignment.
    pub align: u8,
}

/// The table (§4.4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Placement {
    pub chunks: Column<ChunkId, Chunk>,
    /// Every chunk once, in file order.
    pub order: Box<[ChunkId]>,
    pub segments: Column<SegmentId, Segment>,
    /// Per section; a folded section has the place of its target.
    pub place: Column<SectionId, Place>,
    /// Sorted by key.
    pub sym_place: Box<[SymPlace]>,
    /// Sorted by symbol.
    pub defined: Box<[Defined]>,
    /// Sorted by `(chunk, offset)`.
    pub patches: Box<[Patch]>,
    /// Sorted by `(section, offset)`.
    pub redirects: Box<[Redirect]>,
    /// Sorted by chunk.
    pub sized: Box<[Sized]>,
    pub tls: Option<Tls>,
    pub entry: Option<u64>,
}

impl Placement {
    /// The address of `offset` bytes into a section (§4.5): its chunk's
    /// address plus its offset.
    #[must_use]
    pub fn section_addr(&self, section: SectionId, offset: u64) -> Option<u64> {
        let (chunk, at) = self.place.get(section)?.get()?;
        self.chunks.get(chunk)?.addr?.checked_add(at)?.checked_add(offset)
    }

    /// The allocation of a key, if it has one.
    #[must_use]
    pub fn sym_place_of(&self, key: AllocKey) -> Option<&SymPlace> {
        let at = self.sym_place.binary_search_by_key(&key, |row| row.key).ok()?;
        Some(&self.sym_place[at])
    }

    /// The value of a linker-defined symbol.
    #[must_use]
    pub fn defined_of(&self, symbol: SymbolId) -> Option<u64> {
        let at = self.defined.binary_search_by_key(&symbol, |row| row.symbol).ok()?;
        match self.defined[at].value {
            DefValue::Absolute(value) => Some(value),
            DefValue::At { chunk, offset } => self.chunks.get(chunk)?.addr?.checked_add(offset),
        }
    }

    /// The patch a site goes through, if any.
    #[must_use]
    pub fn redirect_of(&self, section: SectionId, offset: u64) -> Option<&Patch> {
        let at = self
            .redirects
            .binary_search_by_key(&(section, offset), |r| (r.section, r.offset))
            .ok()?;
        self.patches.get(self.redirects[at].patch as usize)
    }
}

/// The layout (§4): every decision of a link.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    pub folds: Folds,
    pub merge: MergeMap,
    pub slots: Slots,
    pub placement: Placement,
}

// ---- §4.5 -----------------------------------------------------------------

/// How a symbol-table entry is defined, as the reader gives it: what
/// [`Addresses::symbol_addr`] needs of a `Defined` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryDef {
    /// `SHN_ABS`.
    Absolute(u64),
    /// `st_value` in section `st_shndx` of the entry's file.
    Section(SectionIndex, u64),
}

impl Inputs<'_> {
    /// How entry `entry` is defined, read from its file (§3.2: entries are
    /// not copied into tables). `None` if it is not defined in a section or
    /// absolute, or cannot be read.
    #[must_use]
    pub fn entry_def(&self, entry: SymEntry) -> Option<EntryDef> {
        let Reader::Object(object) = self.files.get(entry.file)?.reader else { return None };
        let symbols = object.symbols().ok()??;
        let sym = symbols.get(entry.index).ok()?;
        match symbols.section_of(entry.index, &sym).ok()? {
            SymbolSection::Absolute => Some(EntryDef::Absolute(sym.st_value)),
            SymbolSection::Section(index) => Some(EntryDef::Section(index, sym.st_value)),
            SymbolSection::Undefined | SymbolSection::Common | SymbolSection::Reserved(_) => None,
        }
    }
}

/// The addresses derived from a layout (§4.5): computed, never stored.
/// Holds the tables they follow from and the one index they need (the
/// merged chunk of each output section).
#[derive(Debug)]
pub struct Addresses<'t, 'n> {
    pub resolution: &'t Resolution,
    pub assignment: &'t Assignment<'n>,
    pub split: &'t Split,
    pub layout: &'t Layout,
    merged: Column<OutSecId, OptId<ChunkId>>,
}

impl<'t, 'n> Addresses<'t, 'n> {
    #[must_use]
    pub fn new(
        resolution: &'t Resolution,
        assignment: &'t Assignment<'n>,
        split: &'t Split,
        layout: &'t Layout,
    ) -> Self {
        let mut merged = Column::filled(assignment.outsecs.len(), OptId::NONE);
        for (id, chunk) in layout.placement.chunks.iter() {
            if let ChunkKind::Merged(out) = chunk.kind
                && let Some(cell) = merged.get_mut(out)
                && cell.is_none()
            {
                *cell = OptId::some(id);
            }
        }
        Self { resolution, assignment, split, layout, merged }
    }

    /// The merged chunk of an output section.
    #[must_use]
    pub fn merged_chunk(&self, out: OutSecId) -> Option<ChunkId> {
        self.merged.get(out)?.get()
    }

    /// The address of a live fragment: its merged chunk's address plus its
    /// offset there.
    #[must_use]
    pub fn frag_addr(&self, frag: FragId) -> Option<u64> {
        let row = self.split.frags.get(frag)?;
        let chunk = self.merged_chunk(self.assignment.out(row.section)?)?;
        let offset = self.layout.merge.offset(frag)?;
        self.layout.placement.chunks.get(chunk)?.addr?.checked_add(offset)
    }

    /// The address of `offset` bytes into a section: through `place`, or for
    /// a `Merge` section through the piece it lands in and its fragment.
    #[must_use]
    pub fn addr_in(&self, section: SectionId, offset: u64) -> Option<u64> {
        if self.resolution.sections.get(section)?.kind == SectionKind::Merge {
            let piece = self.split.merge_piece_at(section, u32::try_from(offset).ok()?)?;
            let frag = *self.split.piece_frag.get(piece.piece as usize)?;
            return self.frag_addr(frag)?.checked_add(u64::from(piece.within));
        }
        self.layout.placement.section_addr(section, offset)
    }

    /// The address of a global symbol (§4.5): `redirect` applied once, then
    /// by its definition: a defined entry through [`Self::addr_in`]
    /// (`entry` reads the entry, [`Inputs::entry_def`]); a common through
    /// `sym_place`; a shared definition through its copy allocation, if it
    /// has one; a linker-defined one through `defined`; a weak undefined is
    /// 0. Otherwise `None`: the symbol has no address in this output.
    #[must_use]
    pub fn symbol_addr(
        &self,
        symbol: SymbolId,
        entry: impl Fn(SymEntry) -> Option<EntryDef>,
    ) -> Option<u64> {
        let symbol = *self.resolution.redirect.get(symbol)?;
        let placement = &self.layout.placement;
        match *self.resolution.def.get(symbol)? {
            Def::Undefined { weak: true } => Some(0),
            Def::Undefined { weak: false } => None,
            Def::Defined(e) => match entry(e)? {
                EntryDef::Absolute(value) => Some(value),
                EntryDef::Section(index, value) => {
                    self.addr_in(self.resolution.section_id(e.file, index)?, value)
                }
            },
            Def::Common(_) => self.sym_place_addr(AllocKey::Common(symbol)),
            Def::Shared(_) => {
                let copies = &self.layout.slots.copy_of;
                let at = copies.binary_search_by_key(&symbol, |row| row.0).ok()?;
                self.sym_place_addr(AllocKey::Copy(copies[at].1))
            }
            Def::Internal => placement.defined_of(symbol),
        }
    }

    fn sym_place_addr(&self, key: AllocKey) -> Option<u64> {
        let row = self.layout.placement.sym_place_of(key)?;
        self.layout.placement.chunks.get(row.chunk)?.addr?.checked_add(row.offset)
    }
}

const _: () = assert!(size_of::<Place>() == 12);
