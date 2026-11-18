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

/// A copy-relocation allocation: one per `(shared FileId, st_value)`
/// (`spec/dyn.md` D-42). `(file, value)` is its identity and its sort key
/// in [`Slots::copies`]; the other fields follow from the shared object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CopyAlloc {
    pub file: FileId,
    pub value: u64,
    /// The chunk the copy lands in (D-42 "place").
    pub place: CopyPlace,
    /// `min(sh_addralign of the defining section, 2^ctz(st_value))`, a
    /// power of two (D-42 "alignment").
    pub align: u64,
    /// The key of the `COPY` record's `r_sym` (D-42 "record"): the least
    /// `SymbolId` among the aliases at `(file, value)` that a live
    /// relocatable object references; a key of [`Slots::copy_of`] that
    /// maps to this allocation.
    pub alias: SymbolId,
}

impl CopyAlloc {
    /// A fixture allocation: `.dynbss`, alignment 1, alias `SymbolId(0)`.
    #[must_use]
    pub fn at(file: FileId, value: u64) -> Self {
        Self { file, value, place: CopyPlace::DynBss, align: 1, alias: SymbolId(0) }
    }

    /// The identity and sort key, `(file, value)`.
    #[must_use]
    pub fn key(&self) -> (FileId, u64) {
        (self.file, self.value)
    }
}

/// Where a copy allocation lands (`spec/dyn.md` D-42; `spec/layout.md`
/// L-80).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CopyPlace {
    /// The chunk `.dynbss` (output section `.bss`, its first chunk): the
    /// defining address is writable and outside the library's RELRO.
    DynBss,
    /// The chunk `.bss.rel.ro` (output section `.bss.rel.ro`, inside
    /// RELRO): the defining address is in a `PT_GNU_RELRO` or a
    /// non-writable `PT_LOAD` of the shared object.
    RelRo,
}

/// A version of a shared object, by the index of its version definition
/// there (`vd_ndx`, `spec/dyn.md` D-14, D-27): what a `Vernaux` of
/// `.gnu.version_r` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VerIndex(pub u16);

/// The `.dynstr` offsets of every string the dynamic sections name
/// (`spec/dyn.md` D-24), fixed by `mortar_synth::dynstr::dynstr_plan`.
/// The writers of `.dynsym`, `.dynamic` and the version sections read
/// offsets here and never build strings. Empty (no `.dynstr`) in a
/// static link.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DynStr {
    /// Per `.dynsym` entry after the null one, in [`Slots::dynsym`]
    /// order: its name.
    pub symbols: Box<[u32]>,
    /// Per [`Slots::needed`] entry: its `DT_NEEDED` name (the soname, else
    /// the name as written).
    pub needed: Box<[u32]>,
    /// Per [`Slots::verneed`] entry, per version: the version's name.
    pub versions: Box<[Box<[u32]>]>,
    /// `DT_RUNPATH`/`DT_RPATH`, when the output has one.
    pub runpath: Option<u32>,
    /// The section's size: byte 0 is NUL, each distinct string once.
    pub size: u32,
}

impl DynStr {
    /// Whether there is no `.dynstr` (a static link).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }
}

/// The content order of the synthetic tables whose size does not depend
/// on addresses (§4.3). Each list's position is the slot: an index counts
/// within its kind, and one key can be in several lists. Where the kinds
/// lie relative to each other in the GOT is `spec/synth.md`'s rule, not
/// stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Slots {
    /// GOT address slots (one per `GOT` demand).
    pub got: Box<[SymKey]>,
    /// PLT entries (one per `PLT` or `CANON_PLT` demand).
    pub plt: Box<[SymKey]>,
    /// IPLT entries (one per `IPLT` demand); a key has a PLT entry or an
    /// IPLT entry, never both.
    pub iplt: Box<[SymKey]>,
    /// TLS descriptors (one per `TLS_DESC` demand).
    pub tls_desc: Box<[SymKey]>,
    /// TLS module/offset pairs (one per `TLS_GD` demand).
    pub tls_gd: Box<[SymKey]>,
    /// Thread-pointer offset slots (one per `GOT_TPREL` demand).
    pub tls_ie: Box<[SymKey]>,
    /// The output's local-dynamic module slot, index 0 of its kind: present
    /// exactly when `TLS_LD` is demanded.
    pub tls_ld: bool,
    /// `.dynsym`, after the null entry (`spec/dyn.md` D-21: the imports,
    /// the exports, the copy aliases and the `WeakDyn` keys; an `EXPORT`
    /// demand is an export), in D-22's order: the first
    /// [`Slots::dynsym_unhashed`] entries are the unhashed ones in
    /// `SymbolId` order, the rest hashed, by (bucket, `SymbolId`). Empty in
    /// a static link.
    pub dynsym: Box<[SymbolId]>,
    /// How many entries of [`Slots::dynsym`] are unhashed (D-22: undefined
    /// with `st_value` 0); `.gnu.hash`'s `symoffset` is this plus 1.
    pub dynsym_unhashed: u32,
    /// `.gnu.hash`'s `nbuckets` (D-26: `max(1, ⌊n/4⌋)` for `n` hashed
    /// entries; D-28 lets an incremental link keep it); 0 without a
    /// `.gnu.hash`.
    pub gnu_hash_buckets: u32,
    /// The order of the sections whose dynamic relocations are emitted.
    pub dyn_relocs: Box<[SectionId]>,
    /// `DT_NEEDED` entries: the needed shared objects (`spec/dyn.md`
    /// D-11), in D-19a's order (by the position of each one's keeping
    /// mention). Computed by `mortar_resolve`.
    pub needed: Box<[FileId]>,
    /// `.gnu.version_r` (`spec/dyn.md` D-27): per needed object that
    /// provides a version an import binds to, in [`Slots::needed`] order,
    /// its versions in increasing [`VerIndex`] (the library's own order of
    /// definitions). Empty without `.gnu.version_r`.
    pub verneed: Box<[(FileId, Box<[VerIndex]>)]>,
    /// The `.dynstr` offsets (`spec/dyn.md` D-24).
    pub dynstr: DynStr,
    /// The live `.eh_frame` records after deduplication of CIEs, in output
    /// order.
    pub eh_records: Box<[EhRecord]>,
    /// Copy-relocation allocations (`spec/dyn.md` D-42), sorted by
    /// `(file, value)`. Their order inside each chunk (by the least
    /// `SymbolId` among the aliases) is layout's, not this list's.
    pub copies: Box<[CopyAlloc]>,
    /// The allocation of every key that has one, sorted by key.
    pub copy_of: Box<[(SymbolId, u32)]>,
    /// Per file, its locals in `.symtab`: `(first local, count)` (§4.3).
    pub symtab_locals: Column<FileId, Span>,
    /// The size each synthetic chunk sized here has, sorted by chunk.
    pub sizes: Box<[(ChunkId, u64)]>,
}

/// A kind of slot held per key (§4.3): one list of [`Slots`] each. The
/// output's local-dynamic module slot is [`Slots::tls_ld`], not a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SlotKind {
    Got,
    Plt,
    Iplt,
    TlsDesc,
    TlsGd,
    /// The thread-pointer offset slot (`GOT_TPREL`), list `tls_ie`.
    TlsIe,
}

impl SlotKind {
    /// Every kind, in the order of the fields of [`Slots`].
    pub const ALL: [Self; 6] =
        [Self::Got, Self::Plt, Self::Iplt, Self::TlsDesc, Self::TlsGd, Self::TlsIe];

    /// The demand flags that need a slot of this kind (`spec/synth.md`
    /// S-63, S-64): a key is in the kind's list when its flags intersect
    /// these. The one table: the full link's slot planning
    /// (`mortar::front`), `Layout::check`'s §4.3 and the delta layout's
    /// item 7 (`spec/incr.md` IN-72) read it.
    #[must_use]
    pub const fn demand(self) -> crate::demands::DemandFlags {
        use crate::demands::DemandFlags as D;
        match self {
            Self::Got => D::GOT,
            Self::Plt => D::PLT.union(D::CANON_PLT),
            Self::Iplt => D::IPLT,
            Self::TlsDesc => D::TLS_DESC,
            Self::TlsGd => D::TLS_GD,
            Self::TlsIe => D::GOT_TPREL,
        }
    }
}

impl Slots {
    /// The list of one kind of slot.
    #[must_use]
    pub fn list(&self, kind: SlotKind) -> &[SymKey] {
        match kind {
            SlotKind::Got => &self.got,
            SlotKind::Plt => &self.plt,
            SlotKind::Iplt => &self.iplt,
            SlotKind::TlsDesc => &self.tls_desc,
            SlotKind::TlsGd => &self.tls_gd,
            SlotKind::TlsIe => &self.tls_ie,
        }
    }
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

impl Synth {
    /// The RELRO padding (`spec/layout.md` L-64): a `SHT_NOBITS`,
    /// `SHF_ALLOC|SHF_WRITE` chunk of alignment 1 that ends the RELRO run
    /// and makes its end a multiple of the maximum page size. Layout's
    /// address stage writes its size; it is not in `sized` (its size is a
    /// function of the addresses before it, not of encoding inputs).
    pub const RELRO_PADDING: Self = Self { name: b".pad.relro", address_dependent: false };
}

/// One part of an output section (`spec/layout.md` L-110): the body of an
/// exec output section is cut into parts in input order, each followed by
/// its island; every other output section has the one part 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Part {
    pub out: OutSecId,
    pub part: u32,
}

impl Part {
    /// Part 0 of `out`, the only part of a section that is not cut.
    #[must_use]
    pub const fn first(out: OutSecId) -> Self {
        Self { out, part: 0 }
    }
}

/// What a chunk is (§4.4 `chunks`: kind).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChunkKind {
    FileHeader,
    ProgramHeaders,
    SectionHeaders,
    /// The body of one part of an output section: its placed input
    /// sections.
    Body(Part),
    /// The merged fragments of an output section.
    Merged(OutSecId),
    /// The island of one body part: the patches (thunks, erratum patches)
    /// the linker adds after it.
    Patches(Part),
    Synthetic(Synth),
    /// `-r`: one `SHT_GROUP` output section per kept group (§4.6 MD-R1,
    /// `spec/layout.md` L-R3), named by the input `SHT_GROUP` section it
    /// is written from: the kept copy of a COMDAT group, or one copy of a
    /// non-COMDAT group (`spec/resolve.md` RV-R7). Its members are the
    /// output sections `out_of` gives the input group's members (L-R2).
    Group(SectionId),
    /// `-r`: the relocation section of an output section with a relocated
    /// piece (§4.6 MD-R1, `spec/layout.md` L-R10, `spec/image.md` I-R5).
    Relocs(OutSecId),
}

impl ChunkKind {
    /// The output section the chunk is part of or about, if any.
    #[must_use]
    pub fn out(self) -> Option<OutSecId> {
        match self {
            Self::Body(p) | Self::Patches(p) => Some(p.out),
            Self::Merged(o) | Self::Relocs(o) => Some(o),
            Self::FileHeader
            | Self::ProgramHeaders
            | Self::SectionHeaders
            | Self::Synthetic(_)
            | Self::Group(_) => None,
        }
    }

    /// The part of a body or an island.
    #[must_use]
    pub fn part(self) -> Option<Part> {
        match self {
            Self::Body(p) | Self::Patches(p) => Some(p),
            _ => None,
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
    /// The address; `None` exactly for a non-alloc chunk, and for every
    /// chunk of a [`PlacementKind::Relocatable`] placement (§4.6 MD-R1).
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

    /// Whether the chunk is the RELRO padding (`spec/layout.md` L-64).
    #[must_use]
    pub fn is_relro_padding(&self) -> bool {
        self.kind == ChunkKind::Synthetic(Synth::RELRO_PADDING)
    }

    /// Whether the chunk is part of an x86-64 large output section
    /// (`SHF_X86_64_LARGE`, `spec/layout.md` L-36, L-36a). Layout keeps
    /// the flag only on x86-64, so on another machine it is never set.
    #[must_use]
    pub fn is_large(&self) -> bool {
        self.flags & mortar_elf::consts::SHF_X86_64_LARGE != 0
    }

    /// The `PT_LOAD` class of an alloc chunk (`spec/layout.md` L-65,
    /// L-36a), `None` for a non-alloc one.
    #[must_use]
    pub fn load_class(&self) -> Option<LoadClass> {
        use mortar_elf::consts::{SHF_EXECINSTR, SHF_WRITE};
        if !self.is_alloc() {
            return None;
        }
        if self.is_large() {
            return Some(if self.flags & SHF_WRITE != 0 {
                LoadClass::LargeRw
            } else if self.flags & SHF_EXECINSTR != 0 {
                LoadClass::LargeRx
            } else {
                LoadClass::LargeR
            });
        }
        Some(if self.relro {
            LoadClass::Relro
        } else if self.flags & SHF_WRITE != 0 {
            LoadClass::Rw
        } else if self.flags & SHF_EXECINSTR != 0 {
            LoadClass::Rx
        } else {
            LoadClass::R
        })
    }

    /// The class of the `PT_LOAD` run the chunk joins, after a run of
    /// class `last` (`spec/layout.md` L-65, L-36a): its [`load_class`],
    /// except that a large `NOBITS` chunk (`.lbss`) right after the small
    /// RW run joins it, as `.lbss` without `.ldata` does in GNU ld's, lld's
    /// and mold's outputs, rather than make a `PT_LOAD` without file
    /// bytes. `.lbss` is the last alloc output section (L-36a), so the
    /// small RW run's addresses are unchanged. Every place that cuts the
    /// alloc chunks into runs uses this.
    ///
    /// [`load_class`]: Chunk::load_class
    #[must_use]
    pub fn run_class(&self, last: Option<LoadClass>) -> Option<LoadClass> {
        let class = self.load_class()?;
        if class == LoadClass::LargeRw && !self.in_file() && last == Some(LoadClass::Rw) {
            return Some(LoadClass::Rw);
        }
        Some(class)
    }

    /// The chunk's name: its output section's, a synthetic chunk's own, none
    /// for the headers. (A `Group` chunk is `.group`, `spec/layout.md`
    /// L-R3; for `Relocs`, the name of the output section it is about; the
    /// relocation section header's name is `spec/image.md` I-R5's.)
    #[must_use]
    pub fn name<'n>(&self, assignment: &Assignment<'n>) -> Option<&'n [u8]> {
        match self.kind {
            ChunkKind::Synthetic(synth) => Some(synth.name),
            ChunkKind::Group(_) => Some(b".group"),
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

/// The `PT_LOAD` a chunk belongs to (`spec/layout.md` L-65): consecutive
/// alloc chunks of one class in `order` form one `PT_LOAD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LoadClass {
    /// Read-only: the headers and ranks 0 to 5.
    R,
    /// Read and execute: rank 6.
    Rx,
    /// Writable, made read-only after relocation: rank 7.
    Relro,
    /// Writable: rank 8 (and rank 7 under `-z norelro`).
    Rw,
    /// x86-64 `.lrodata` (L-36a): its own read-only `PT_LOAD` after every
    /// small section.
    LargeR,
    /// x86-64 `.ltext` (L-36a): its own read-and-execute `PT_LOAD`.
    LargeRx,
    /// x86-64 `.ldata` then `.lbss` (L-36a): one writable `PT_LOAD`.
    LargeRw,
}

impl LoadClass {
    /// The `p_flags` of the class's `PT_LOAD`.
    #[must_use]
    pub const fn p_flags(self) -> u32 {
        use mortar_elf::consts::{PF_R, PF_W, PF_X};
        match self {
            Self::R | Self::LargeR => PF_R,
            Self::Rx | Self::LargeRx => PF_R | PF_X,
            Self::Relro | Self::Rw | Self::LargeRw => PF_R | PF_W,
        }
    }
}

/// What [`derive_segments`] needs beyond the chunks (`spec/layout.md`
/// L-125: the facts of the target and the options of the `Plan`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentRules {
    /// The ELF class: the word size of `PT_GNU_PROPERTY`'s alignment.
    pub class: mortar_elf::Class,
    /// The maximum page size (`-z max-page-size`, or the target's): the
    /// least `p_align` of a `PT_LOAD` (L-67).
    pub max_page: u64,
    /// The `p_flags` of `PT_GNU_STACK` (L-84): `PF_R|PF_W`, or with `PF_X`.
    pub stack: u32,
    /// The output is position-independent (`ET_DYN`, `spec/dyn.md` D-1):
    /// its image base is 0 (`spec/layout.md` L-68), so the file header is
    /// at address 0; otherwise it is at a non-zero base. [`Layout::check`]
    /// requires it (rule 3).
    pub pie: bool,
}

/// `PT_PHDR` and `PT_INTERP` (exactly when an interpreter is named), and
/// `PT_DYNAMIC` (`spec/dyn.md` D-33, D-61): each over its chunk.
fn derive_dynamic(ordered: &[&Chunk], word: u8) -> (Vec<Segment>, Option<Segment>) {
    use mortar_elf::consts::{PF_R, PF_W, PT_DYNAMIC, PT_INTERP, PT_PHDR};
    let synthetic = |name: &[u8]| {
        ordered
            .iter()
            .copied()
            .find(|c| c.is_alloc() && matches!(c.kind, ChunkKind::Synthetic(s) if s.name == name))
    };
    let covering = |c: &Chunk, p_type, flags, align| Segment {
        p_type,
        flags,
        offset: c.offset,
        addr: c.addr.unwrap_or(0),
        filesz: c.size,
        memsz: c.size,
        align,
    };
    let mut head = Vec::new();
    if let Some(interp) = synthetic(b".interp") {
        if let Some(ph) = ordered.iter().find(|c| c.kind == ChunkKind::ProgramHeaders) {
            head.push(covering(ph, PT_PHDR, PF_R, word));
        }
        head.push(covering(interp, PT_INTERP, PF_R, 0));
    }
    let dynamic = synthetic(b".dynamic").map(|d| covering(d, PT_DYNAMIC, PF_R | PF_W, word));
    (head, dynamic)
}

/// The program headers of a layout (`spec/layout.md` §11, L-125): a pure
/// function of the chunks, their order and the rules, shared by
/// `mortar_layout` (which writes them) and [`Layout::check`] (which
/// requires the stored ones to equal it).
///
/// In this order (L-67, `spec/dyn.md` D-61): `PT_PHDR` over the program
/// header table and `PT_INTERP` over the synthetic chunk `.interp`, both
/// exactly when that chunk exists (an interpreter is named); one `PT_LOAD`
/// per run of consecutive alloc chunks of one [`LoadClass`] (non-alloc
/// chunks do not break a run); `PT_DYNAMIC` over the synthetic chunk
/// `.dynamic` when it exists; one `PT_NOTE` per run of consecutive alloc
/// `SHT_NOTE` chunks of one alignment; `PT_TLS` over the TLS chunks;
/// `PT_GNU_PROPERTY` and `PT_GNU_EH_FRAME` over the synthetic chunks of
/// those names; one `PT_GNU_STACK`; `PT_GNU_RELRO` over the RELRO chunks.
///
/// - `PT_PHDR` is `PF_R`, `p_align` the word; `PT_INTERP` `PF_R`,
///   `p_align` 1; `PT_DYNAMIC` `PF_R|PF_W`, `p_align` the word (D-33).
///   Each covers its chunk's `size` exactly (file and memory).
///
/// - A `PT_LOAD` starts at its first chunk; its `p_offset` is the least
///   offset at or after that chunk's offset congruent to its address modulo
///   `p_align` (the chunk's own offset for a chunk in the file; for a
///   `NOBITS` first chunk, L-70's offset, of which the chunk's own is the
///   undisplaced file end, L-69). `p_filesz` runs to the end (capacity) of
///   its last chunk in the file, `p_memsz` to the end of its last chunk
///   that takes memory; `p_align` is `max(max_page, the largest chunk
///   alignment in it)`.
/// - `PT_TLS` runs from the first TLS chunk to the end (`size`) of the
///   last, `p_filesz` to the end of the last TLS `PROGBITS` chunk;
///   `p_align` is the largest TLS alignment. `PT_GNU_RELRO` runs from the
///   first RELRO chunk to the end of the last (the padding), `p_filesz` to
///   the end of the last RELRO chunk in the file; `p_align` 1.
///
/// The function never fails and never panics: a layout loaded from a link
/// state may be anything, and then simply differs from its derivation.
#[must_use]
pub fn derive_segments(
    chunks: &Column<ChunkId, Chunk>,
    order: &[ChunkId],
    rules: &SegmentRules,
) -> Vec<Segment> {
    use mortar_elf::consts::{
        PF_R, PT_GNU_EH_FRAME, PT_GNU_PROPERTY, PT_GNU_RELRO, PT_GNU_STACK, PT_TLS,
    };
    let word: u8 = match rules.class {
        mortar_elf::Class::Elf64 => 3,
        mortar_elf::Class::Elf32 => 2,
    };
    let max_page = if rules.max_page.is_power_of_two() { rules.max_page } else { 1 };
    let page_log = max_page.trailing_zeros() as u8;
    let ordered: Vec<&Chunk> = order.iter().filter_map(|&c| chunks.get(c)).collect();
    let mut out = Vec::new();

    let (head, dynamic) = derive_dynamic(&ordered, word);
    out.extend(head);

    let loads = derive_loads(&ordered, page_log);
    out.extend(loads.iter().map(|l| l.0));

    out.extend(dynamic);
    let load_page = |addr: u64| -> u64 {
        loads
            .iter()
            .find(|(s, _)| s.addr <= addr && addr <= s.addr.saturating_add(s.memsz))
            .map_or(max_page, |l| l.1)
    };

    out.extend(derive_notes(&ordered));

    // PT_TLS.
    let tls: Vec<&Chunk> = ordered.iter().copied().filter(|c| c.is_alloc() && c.is_tls()).collect();
    if let (Some(first), Some(last)) = (tls.first(), tls.last()) {
        let start = first.addr.unwrap_or(0);
        let init_end = tls
            .iter()
            .filter(|c| c.in_file())
            .filter_map(|c| c.addr.map(|a| a.saturating_add(c.size)))
            .max();
        let memsz = last.addr.unwrap_or(0).saturating_add(last.size).saturating_sub(start);
        out.push(Segment {
            p_type: PT_TLS,
            flags: PF_R,
            offset: congruent_offset(first, load_page(start)),
            addr: start,
            filesz: init_end.map_or(0, |e| e.saturating_sub(start)),
            memsz,
            align: tls.iter().map(|c| c.align).max().unwrap_or(0),
        });
    }

    // PT_GNU_PROPERTY and PT_GNU_EH_FRAME.
    for (name, p_type, align) in [
        (&b".note.gnu.property"[..], PT_GNU_PROPERTY, word),
        (&b".eh_frame_hdr"[..], PT_GNU_EH_FRAME, 2),
    ] {
        let found = ordered
            .iter()
            .find(|c| c.is_alloc() && matches!(c.kind, ChunkKind::Synthetic(s) if s.name == name));
        if let Some(c) = found {
            out.push(Segment {
                p_type,
                flags: PF_R,
                offset: c.offset,
                addr: c.addr.unwrap_or(0),
                filesz: c.size,
                memsz: c.size,
                align,
            });
        }
    }

    out.push(Segment {
        p_type: PT_GNU_STACK,
        flags: rules.stack,
        offset: 0,
        addr: 0,
        filesz: 0,
        memsz: 0,
        align: 4,
    });

    // PT_GNU_RELRO.
    let relro: Vec<&Chunk> = ordered.iter().copied().filter(|c| c.relro && c.is_alloc()).collect();
    if let Some(first) = relro.first() {
        let start = first.addr.unwrap_or(0);
        let offset = congruent_offset(first, load_page(start));
        let mem_end = relro
            .iter()
            .filter(|c| c.takes_memory() || c.is_relro_padding())
            .filter_map(|c| c.addr.map(|a| a.saturating_add(c.capacity)))
            .max()
            .unwrap_or(start);
        let file_end = relro
            .iter()
            .filter(|c| c.in_file())
            .map(|c| c.offset.saturating_add(c.capacity))
            .max()
            .unwrap_or(offset);
        out.push(Segment {
            p_type: PT_GNU_RELRO,
            flags: PF_R,
            offset,
            addr: start,
            filesz: file_end.saturating_sub(offset),
            memsz: mem_end.saturating_sub(start),
            align: 0,
        });
    }
    out
}

/// The `PT_LOAD`s of [`derive_segments`], each with its `p_align` in bytes.
fn derive_loads(ordered: &[&Chunk], page_log: u8) -> Vec<(Segment, u64)> {
    use mortar_elf::consts::PT_LOAD;
    // PT_LOAD runs.
    let mut runs: Vec<(LoadClass, Vec<&Chunk>)> = Vec::new();
    for &c in ordered {
        let Some(class) = c.run_class(runs.last().map(|r| r.0)) else { continue };
        match runs.last_mut() {
            Some((last, members)) if *last == class => members.push(c),
            _ => runs.push((class, vec![c])),
        }
    }
    let mut loads: Vec<(Segment, u64)> = Vec::new();
    for (class, members) in &runs {
        let align = members.iter().map(|c| c.align).max().unwrap_or(0).max(page_log).min(63);
        let p = 1u64 << align;
        let first = members[0];
        let vaddr = first.addr.unwrap_or(0);
        let offset = congruent_offset(first, p);
        let file_end = members
            .iter()
            .filter(|c| c.in_file())
            .map(|c| c.offset.saturating_add(c.capacity))
            .max()
            .unwrap_or(offset);
        let mem_end = members
            .iter()
            .filter(|c| c.takes_memory())
            .filter_map(|c| c.addr.map(|a| a.saturating_add(c.capacity)))
            .max()
            .unwrap_or(vaddr);
        let filesz = file_end.saturating_sub(offset);
        let memsz = mem_end.saturating_sub(vaddr).max(filesz);
        let seg = Segment {
            p_type: PT_LOAD,
            flags: class.p_flags(),
            offset,
            addr: vaddr,
            filesz,
            memsz,
            align,
        };
        loads.push((seg, p));
    }
    loads
}

/// The `PT_NOTE`s of [`derive_segments`]: one per run of consecutive alloc
/// notes of one alignment.
fn derive_notes(ordered: &[&Chunk]) -> Vec<Segment> {
    use mortar_elf::consts::{PF_R, PT_NOTE, SHT_NOTE};
    // PT_NOTE per run of consecutive alloc notes of one alignment.
    let mut note: Option<(u8, &Chunk, &Chunk)> = None;
    let mut notes = Vec::new();
    for &c in ordered {
        let is_note = c.is_alloc() && c.sh_type == SHT_NOTE;
        match note {
            Some((a, first, _)) if is_note && c.align == a => note = Some((a, first, c)),
            _ => {
                if let Some(run) = note.take() {
                    notes.push(run);
                }
                if is_note {
                    note = Some((c.align, c, c));
                }
            }
        }
    }
    notes.extend(note);
    let mut out = Vec::new();
    for (align, first, last) in notes {
        let size = last.offset.saturating_add(last.size).saturating_sub(first.offset);
        out.push(Segment {
            p_type: PT_NOTE,
            flags: PF_R,
            offset: first.offset,
            addr: first.addr.unwrap_or(0),
            filesz: size,
            memsz: size,
            align,
        });
    }
    out
}

/// The least offset at or after `c.offset` congruent to `c.addr` modulo
/// `p` (a power of two): `c.offset` itself for a chunk whose offset is
/// already congruent.
fn congruent_offset(c: &Chunk, p: u64) -> u64 {
    let addr = c.addr.unwrap_or(c.offset);
    let delta = addr.wrapping_sub(c.offset) & p.wrapping_sub(1);
    c.offset.saturating_add(delta)
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

/// A processor erratum the linker patches around (`spec/arch.md` §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Erratum {
    /// Arm Cortex-A53 erratum 843419 (`spec/a64.md` §11).
    E843419,
}

/// What a patch is (§4.4 `patches`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchKind {
    Thunk(ThunkTarget),
    /// An erratum patch, keyed by the site of the instruction it replaces
    /// (M-7): the one site that may redirect to it.
    Erratum {
        erratum: Erratum,
        section: SectionId,
        offset: u64,
    },
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

/// What a [`Hole`] holds (`spec/incr.md` IN-3, IN-3a, IN-34a, IN-51b).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HoleKind {
    /// A region's slack in an allocated L-143 section (IN-30): the
    /// chunk's fill (trap in code, zero in data, nothing in `NOBITS`).
    Fill,
    /// The unused tail of an `.eh_frame` per-file range (IN-34a): 4, 8 or
    /// 12 bytes absorbed as `DW_CFA_nop` padding of the record before
    /// it, or one padding CIE of at least 16 bytes.
    EhPad,
    /// A debug contribution's allotment beyond its size (IN-50): the
    /// section's neutral form (IN-51b).
    Debug,
    /// The one neutral abbreviation table (IN-51: four abbreviations
    /// and the table's end, 38 bytes) at the end of `.debug_abbrev`,
    /// which every neutral `.debug_info` and `.debug_types` unit names.
    AbbrevTable,
    /// A dead range an earlier link's debug contribution held (IN-3a):
    /// the section's neutral form.
    DeadDebug,
    /// A dead merged string piece (IN-3a, L-147): its bytes are left as
    /// written (the map's `dead … strings`). A dead constant piece takes
    /// the fill instead: it is a [`HoleKind::Fill`] hole.
    DeadFragment,
    /// An unused CIE in `.eh_frame` (IN-3a): a padding CIE.
    DeadCie,
    /// A retired GOT, IPLT or TLS slot (IN-3a; IN-72 item 7, S-113): the
    /// slot's words written 0, its `SLOTS` row kept with `refs` 0 (a
    /// revival takes it back). The map lists it as a `hole` line (zero
    /// bytes are a data chunk's fill).
    DeadSlot,
}

impl HoleKind {
    /// Whether it is a dead range (IN-3a): what an earlier link's live
    /// unit held, not slack.
    #[must_use]
    pub fn is_dead(self) -> bool {
        matches!(self, Self::DeadDebug | Self::DeadFragment | Self::DeadCie | Self::DeadSlot)
    }
}

/// A range of a chunk's content no live unit owns, made by an
/// incremental-mode link (`spec/incr.md` IN-3, IN-3a): a region's slack,
/// an `.eh_frame` range's padding, a debug contribution's allotment, or a
/// dead range. A plain link has none. The bytes are a function of the
/// kind, the chunk and the target (the image writes them, layout.md
/// L-141).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hole {
    pub chunk: ChunkId,
    /// From the chunk's start.
    pub offset: u64,
    pub size: u64,
    pub kind: HoleKind,
}

/// What a placement places its chunks in (§4.4; §4.6 MD-R1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum PlacementKind {
    /// A loadable output (an executable or a shared object): alloc chunks
    /// have addresses, `segments` are derived from the chunks, and
    /// `Layout::check` reads its rules 1-5 as written.
    #[default]
    Loadable,
    /// A relocatable output (`-r`, §4.6 MD-R1, `spec/layout.md` L-R7): file
    /// offsets only. No chunk has an address (alloc or not), `segments`,
    /// `defined`, `patches`, `sized` and `tls` are empty, `entry` is none
    /// and `sym_place` holds only `-d`'s commons; the chunks are the file
    /// header, the section headers, `Body(o, 0)`, `Group(g)`, `Relocs(o)`
    /// and the synthetic `.symtab`, `.symtab_shndx`, `.strtab`,
    /// `.shstrtab`, `.eh_frame` and build-id note. `Layout::check` reads
    /// its rules as `spec/layout.md` L-R10 says (RL-3).
    Relocatable,
}

/// The table (§4.4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Placement {
    /// What the chunks are placed in: [`PlacementKind::Loadable`] for every
    /// output but `-r`.
    pub kind: PlacementKind,
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
    /// Sorted by `(chunk, offset)`, never overlapping (`spec/incr.md`
    /// IN-3): empty in a plain link.
    pub holes: Box<[Hole]>,
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

/// The layout of a `-r` link (§4.6 MD-R1, `spec/layout.md` §24): what
/// `mortar_layout::relocatable::layout` (RL-3) gives
/// `mortar_image::relocatable::write` (RL-4). WP-181 fixed it so that the
/// two packages, which run at the same time, share this type and no file.
#[derive(Debug, Clone, Default)]
pub struct RelocatableLayout<'n> {
    /// `out_of` by L-R1/L-R2 (an output section is an index, never a name
    /// alone).
    pub assignment: Assignment<'n>,
    /// `placement.kind` is [`PlacementKind::Relocatable`]: MD-R1's chunks
    /// (the file header, the section headers, `Body(o, 0)` per output
    /// section, `Group(g)` per kept group, `Relocs(o)` per output section
    /// with a relocated piece, the synthetic chunks), in L-R8's order,
    /// with file offsets and no address; `sym_place` holds only `-d`'s
    /// commons (L-R4). `folds`, `merge` and `slots` are empty.
    pub layout: Layout,
    /// L-R6 as data: the `.eh_frame` records removed, as (input
    /// `.eh_frame` section, offset of the record in it), sorted. A record
    /// not listed is kept, in input order.
    pub eh_removed: Box<[(SectionId, u64)]>,
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
    /// 0, and so is a withdrawn conditional symbol unless a reference to it
    /// is strong (`Assignment::def`). Otherwise `None`: the symbol has no address in this output.
    #[must_use]
    pub fn symbol_addr(
        &self,
        symbol: SymbolId,
        entry: impl Fn(SymEntry) -> Option<EntryDef>,
    ) -> Option<u64> {
        let symbol = *self.resolution.redirect.get(symbol)?;
        let placement = &self.layout.placement;
        match self.assignment.def(self.resolution, symbol)? {
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
            Def::Internal(_) => placement.defined_of(symbol),
        }
    }

    fn sym_place_addr(&self, key: AllocKey) -> Option<u64> {
        let row = self.layout.placement.sym_place_of(key)?;
        self.layout.placement.chunks.get(row.chunk)?.addr?.checked_add(row.offset)
    }
}

const _: () = assert!(size_of::<Place>() == 12);
