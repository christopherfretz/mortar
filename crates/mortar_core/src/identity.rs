//! Identity (`spec/model.md` §7.1): what survives between links when ids
//! do not. A link state stores rows in identity terms; a later link
//! translates them to its own ids. The on-disk form is `spec/incr.md`'s
//! (M7); this module has the identity types and the two conversions, id to
//! identity and identity to id, over one link's tables, and the dependees of
//! the dependency index (§7.2) in identity terms, [`Dependee`].
//!
//! Some parts of an identity are read from the input bytes, not from a
//! table: a section's anchor (the symbol defined at its start) and the
//! names of local symbols. The phase that reads them hands them in, per
//! file, as the keys of an [`Ordinals`]; everything else follows from the
//! tables.

use crate::assignment::Assignment;
use crate::column::Column;
use crate::demands::DemandFlags;
use crate::id::{
    ChunkId, ComdatId, FileId, FragId, Id, LocalSym, OutSecId, SectionId, SymEntry, SymKey,
    SymKind, SymbolId,
};
use crate::inputs::{Comdat, ContentHash, Inputs, Probe, ProbeOutcome, SymbolKey};
use crate::layout::{AllocKey, ChunkKind, CopyAlloc, Patch, Placement, SlotKind, Slots};
use crate::resolution::Resolution;
use mortar_elf::{SectionIndex, SymbolIndex};

/// The identity of a file (§7.1): the found path (`spec/input.md` §4.5)
/// and the mention ordinal among files with that path, plus, for an
/// archive member, its name and ordinal among same-named members. Two
/// mentions of one path are two files (`spec/input.md` §9.4). The content
/// hash that detects change is the file's `hash`, not part of the key;
/// the probes that chose the path are re-checked beside it
/// ([`Inputs::probe_checks`]). The internal file's path is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileIdentity<'a> {
    pub path: &'a [u8],
    /// For a file named directly: how many earlier files not archive
    /// members have the same path. For a member: how many earlier archive
    /// mentions have its archive's path.
    pub mention: u32,
    pub member: Option<(&'a [u8], u32)>,
}

/// What an incremental link re-checks of one probe, by one `stat`
/// (§7.1; `spec/input.md` §4.6): that the candidate is still there, or
/// still absent. A probe whose candidate was skipped for its target must
/// still be there and still of another target; one that failed is never
/// recorded in a link state (the link failed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProbeCheck<'a> {
    pub path: &'a [u8],
    /// `true`: a file was there (found or skipped); `false`: absent.
    pub present: bool,
}

impl<'store> Inputs<'store> {
    /// The identity of every file.
    #[must_use]
    pub fn file_identities(&self) -> Column<FileId, FileIdentity<'store>> {
        // Ordinals by sorting, not by counting: links have many files.
        let archives = Ordinals::new(self.archives.as_slice().iter().map(|a| a.path).collect());
        let files = self.files.as_slice();
        let direct =
            Ordinals::new(files.iter().map(|f| f.member.is_none().then_some(f.path)).collect());
        let mut rows = Vec::with_capacity(files.len());
        for (row, f) in files.iter().enumerate() {
            let mention = match f.member {
                Some(m) => archives.ordinal(m.archive.0),
                None => direct.ordinal(row as u32),
            };
            rows.push(FileIdentity {
                path: f.path,
                mention: mention.unwrap_or(0),
                member: f.member.map(|m| (m.name, m.ordinal)),
            });
        }
        Column::from_vec(rows).unwrap_or_default()
    }

    /// The probes of the link, as what an incremental link re-checks
    /// (§7.1): a new candidate earlier in a search changes neither a path
    /// nor a hash, only a probe. In probe order; `None` if a probe failed.
    #[must_use]
    pub fn probe_checks(&self) -> Option<Vec<ProbeCheck<'store>>> {
        self.link
            .probes
            .iter()
            .map(|&Probe { path, outcome, .. }| match outcome {
                ProbeOutcome::Absent => Some(ProbeCheck { path, present: false }),
                ProbeOutcome::Skipped(_) | ProbeOutcome::Found(_) => {
                    Some(ProbeCheck { path, present: true })
                }
                ProbeOutcome::Error(_) => None,
            })
            .collect()
    }
}

/// Rows sorted by a key, for the conversion identity to id: a binary
/// search instead of a hash map (§6 rule 5 needs no exception here).
#[derive(Debug, Clone, Default)]
pub struct Lookup<I> {
    sorted: Box<[I]>,
}

impl<I: Id> Lookup<I> {
    /// The rows of `ids` sorted by `key`, then by id.
    pub fn new<K: Ord>(ids: impl Iterator<Item = I>, key: impl Fn(I) -> K) -> Self {
        let mut sorted: Vec<I> = ids.collect();
        sorted.sort_by(|&a, &b| key(a).cmp(&key(b)).then(a.cmp(&b)));
        Self { sorted: sorted.into() }
    }

    /// The row whose key is `want`, if exactly one has it.
    pub fn find<K: Ord>(&self, want: &K, key: impl Fn(I) -> K) -> Option<I> {
        let first = self.sorted.partition_point(|&id| key(id) < *want);
        let found = *self.sorted.get(first)?;
        let unique = self.sorted.get(first + 1).is_none_or(|&next| key(next) != *want);
        (key(found) == *want && unique).then_some(found)
    }
}

/// The rows of one unit (a file's sections, its local symbols, the output
/// sections of a link) by key, with each row's **ordinal** among the rows
/// with the same key: what lets an identity name a row without its index.
#[derive(Debug, Clone, Default)]
pub struct Ordinals<K> {
    keys: Box<[K]>,
    /// Row numbers sorted by `(key, row)`.
    sorted: Box<[u32]>,
}

impl<K: Ord + Copy> Ordinals<K> {
    /// Row `i` has key `keys[i]`.
    #[must_use]
    pub fn new(keys: Vec<K>) -> Self {
        let mut sorted: Vec<u32> = (0..keys.len() as u32).collect();
        sorted.sort_by(|&a, &b| keys[a as usize].cmp(&keys[b as usize]).then(a.cmp(&b)));
        Self { keys: keys.into(), sorted: sorted.into() }
    }

    /// The number of rows.
    #[must_use]
    pub fn len(&self) -> u32 {
        self.keys.len() as u32
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The key of row `row`.
    #[must_use]
    pub fn key(&self, row: u32) -> Option<K> {
        self.keys.get(row as usize).copied()
    }

    fn first(&self, key: &K) -> usize {
        self.sorted.partition_point(|&r| self.keys[r as usize] < *key)
    }

    /// Row `row`'s ordinal among the rows with its key.
    #[must_use]
    pub fn ordinal(&self, row: u32) -> Option<u32> {
        let key = self.key(row)?;
        let first = self.first(&key);
        let at = self.sorted.partition_point(|&r| (self.keys[r as usize], r) < (key, row));
        Some((at - first) as u32)
    }

    /// The row with key `key` and ordinal `ordinal`.
    #[must_use]
    pub fn find(&self, key: &K, ordinal: u32) -> Option<u32> {
        let row = *self.sorted.get(self.first(key) + ordinal as usize)?;
        (self.keys[row as usize] == *key).then_some(row)
    }
}

/// What a section is anchored to (§7.1): its COMDAT signature, else the
/// first named non-local symbol defined at its start, else the first named
/// local one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Anchor<'a> {
    Comdat(&'a [u8]),
    Global(&'a [u8]),
    Local(&'a [u8]),
    None,
}

impl<'a> Anchor<'a> {
    /// The anchor by the order of §7.1.
    #[must_use]
    pub fn choose(
        comdat: Option<&'a [u8]>,
        global: Option<&'a [u8]>,
        local: Option<&'a [u8]>,
    ) -> Self {
        match (comdat, global, local) {
            (Some(name), _, _) => Self::Comdat(name),
            (None, Some(name), _) => Self::Global(name),
            (None, None, Some(name)) => Self::Local(name),
            (None, None, None) => Self::None,
        }
    }
}

/// The part of a section's identity found in its file: anchor, then name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SectionKey<'a> {
    pub anchor: Anchor<'a>,
    pub name: &'a [u8],
}

/// The identity of a section (§7.1). Its content hash is stored beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SectionIdentity<'a> {
    pub file: FileIdentity<'a>,
    pub key: SectionKey<'a>,
    /// Among the file's sections with the same key.
    pub ordinal: u32,
}

/// The identity of a local symbol (§7.1): never its symbol-table index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalIdentity<'a> {
    pub file: FileIdentity<'a>,
    pub name: &'a [u8],
    /// Among the file's locals with the same name.
    pub ordinal: u32,
}

/// The identity of a global symbol (§7.1): its key bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolIdentity<'a> {
    pub name: &'a [u8],
    pub version: Option<&'a [u8]>,
}

/// What kind of chunk an identity names (a [`ChunkKind`] without its ids).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChunkTag {
    FileHeader,
    ProgramHeaders,
    SectionHeaders,
    Body,
    Merged,
    Patches,
    Synthetic,
    Group,
    Relocs,
}

impl From<ChunkKind> for ChunkTag {
    fn from(kind: ChunkKind) -> Self {
        match kind {
            ChunkKind::FileHeader => Self::FileHeader,
            ChunkKind::ProgramHeaders => Self::ProgramHeaders,
            ChunkKind::SectionHeaders => Self::SectionHeaders,
            ChunkKind::Body(_) => Self::Body,
            ChunkKind::Merged(_) => Self::Merged,
            ChunkKind::Patches(_) => Self::Patches,
            ChunkKind::Synthetic(_) => Self::Synthetic,
            ChunkKind::Group(_) => Self::Group,
            ChunkKind::Relocs(_) => Self::Relocs,
        }
    }
}

/// The identity of an output section or a chunk (§7.1): kind and name, and
/// an ordinal among those with the same kind and name (a script may give
/// two output sections one name).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChunkIdentity<'a> {
    pub kind: ChunkTag,
    pub name: Option<&'a [u8]>,
    pub ordinal: u32,
}

/// The identity of an output section: its name and ordinal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OutSecIdentity<'a> {
    pub name: &'a [u8],
    pub ordinal: u32,
}

/// The identity of a fragment (§7.1): its output section and the hash of
/// its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FragIdentity<'a> {
    pub out: OutSecIdentity<'a>,
    pub hash: ContentHash,
}

/// The identities of one link's rows, and the way back.
#[derive(Debug, Clone)]
pub struct Identities<'a> {
    files: Column<FileId, FileIdentity<'a>>,
    file_index: Lookup<FileId>,
    /// Per file, its sections' keys by section index (empty for files with
    /// no sections).
    sections: Column<FileId, Ordinals<SectionKey<'a>>>,
    /// Per file, its local symbols' names by symbol index, from 0 to the
    /// first non-local entry.
    locals: Column<FileId, Ordinals<&'a [u8]>>,
}

impl<'a> Identities<'a> {
    /// The identities of a link with these files, whose sections and local
    /// symbols have these keys.
    #[must_use]
    pub fn new(
        files: Column<FileId, FileIdentity<'a>>,
        sections: Column<FileId, Ordinals<SectionKey<'a>>>,
        locals: Column<FileId, Ordinals<&'a [u8]>>,
    ) -> Self {
        let file_index = Lookup::new(files.ids().iter(), |f| files[f]);
        Self { files, file_index, sections, locals }
    }

    #[must_use]
    pub fn file(&self, id: FileId) -> Option<FileIdentity<'a>> {
        self.files.get(id).copied()
    }

    #[must_use]
    pub fn file_id(&self, identity: &FileIdentity<'a>) -> Option<FileId> {
        self.file_index.find(identity, |f| self.files[f])
    }

    /// The identity of a section of this link.
    #[must_use]
    pub fn section(&self, resolution: &Resolution, id: SectionId) -> Option<SectionIdentity<'a>> {
        let (file, index) = resolution.sh_index(id)?;
        let keys = self.sections.get(file)?;
        Some(SectionIdentity {
            file: self.file(file)?,
            key: keys.key(index.0)?,
            ordinal: keys.ordinal(index.0)?,
        })
    }

    /// The section of this link an identity denotes.
    #[must_use]
    pub fn section_id(
        &self,
        resolution: &Resolution,
        identity: &SectionIdentity<'a>,
    ) -> Option<SectionId> {
        let file = self.file_id(&identity.file)?;
        let index = self.sections.get(file)?.find(&identity.key, identity.ordinal)?;
        resolution.section_id(file, SectionIndex(index))
    }

    /// The identity of a local symbol of this link.
    #[must_use]
    pub fn local(&self, sym: LocalSym) -> Option<LocalIdentity<'a>> {
        let names = self.locals.get(sym.file)?;
        Some(LocalIdentity {
            file: self.file(sym.file)?,
            name: names.key(sym.index.0)?,
            ordinal: names.ordinal(sym.index.0)?,
        })
    }

    /// The local symbol of this link an identity denotes.
    #[must_use]
    pub fn local_sym(&self, identity: &LocalIdentity<'a>) -> Option<LocalSym> {
        let file = self.file_id(&identity.file)?;
        let index = self.locals.get(file)?.find(&identity.name, identity.ordinal)?;
        Some(SymEntry { file, index: SymbolIndex(index) })
    }
}

/// The identity of a global symbol row.
#[must_use]
pub fn symbol_identity<'a>(
    symbols: &Column<SymbolId, SymbolKey<'a>>,
    id: SymbolId,
) -> Option<SymbolIdentity<'a>> {
    symbols.get(id).map(|k| SymbolIdentity { name: k.name, version: k.version })
}

/// The way back from a global symbol's identity.
#[must_use]
pub fn symbol_lookup(symbols: &Column<SymbolId, SymbolKey<'_>>) -> Lookup<SymbolId> {
    Lookup::new(symbols.ids().iter(), |s| (symbols[s].name, symbols[s].version))
}

/// The global symbol row of this link an identity denotes.
#[must_use]
pub fn symbol_id(
    symbols: &Column<SymbolId, SymbolKey<'_>>,
    lookup: &Lookup<SymbolId>,
    identity: &SymbolIdentity<'_>,
) -> Option<SymbolId> {
    lookup.find(&(identity.name, identity.version), |s| (symbols[s].name, symbols[s].version))
}

/// The way back from a COMDAT signature (its identity is its bytes).
#[must_use]
pub fn comdat_lookup(comdats: &Column<ComdatId, Comdat<'_>>) -> Lookup<ComdatId> {
    Lookup::new(comdats.ids().iter(), |c| comdats[c].name)
}

/// The COMDAT group of this link with signature `name`.
#[must_use]
pub fn comdat_id(
    comdats: &Column<ComdatId, Comdat<'_>>,
    lookup: &Lookup<ComdatId>,
    name: &[u8],
) -> Option<ComdatId> {
    lookup.find(&name, |c| comdats[c].name)
}

/// The output sections of a link by name, for their identities.
#[must_use]
pub fn outsec_ordinals<'n>(assignment: &Assignment<'n>) -> Ordinals<&'n [u8]> {
    Ordinals::new(assignment.outsecs.as_slice().iter().map(|o| o.name).collect())
}

/// The identity of an output section, given [`outsec_ordinals`].
#[must_use]
pub fn outsec_identity<'n>(
    ordinals: &Ordinals<&'n [u8]>,
    id: OutSecId,
) -> Option<OutSecIdentity<'n>> {
    Some(OutSecIdentity { name: ordinals.key(id.0)?, ordinal: ordinals.ordinal(id.0)? })
}

/// The output section an identity denotes.
#[must_use]
pub fn outsec_id(ordinals: &Ordinals<&[u8]>, identity: &OutSecIdentity<'_>) -> Option<OutSecId> {
    ordinals.find(&identity.name, identity.ordinal).map(OutSecId)
}

/// The chunks of a link by kind and name, for their identities.
#[must_use]
pub fn chunk_ordinals<'n>(
    placement: &Placement,
    assignment: &Assignment<'n>,
) -> Ordinals<(ChunkTag, Option<&'n [u8]>)> {
    Ordinals::new(
        placement
            .chunks
            .as_slice()
            .iter()
            .map(|c| (ChunkTag::from(c.kind), c.name(assignment)))
            .collect(),
    )
}

/// The identity of a chunk, given [`chunk_ordinals`].
#[must_use]
pub fn chunk_identity<'n>(
    ordinals: &Ordinals<(ChunkTag, Option<&'n [u8]>)>,
    id: ChunkId,
) -> Option<ChunkIdentity<'n>> {
    let (kind, name) = ordinals.key(id.0)?;
    Some(ChunkIdentity { kind, name, ordinal: ordinals.ordinal(id.0)? })
}

/// The chunk an identity denotes.
#[must_use]
pub fn chunk_id(
    ordinals: &Ordinals<(ChunkTag, Option<&[u8]>)>,
    identity: &ChunkIdentity<'_>,
) -> Option<ChunkId> {
    ordinals.find(&(identity.kind, identity.name), identity.ordinal).map(ChunkId)
}

/// The identity of a fragment, given its content hash (computed from its
/// bytes by `spec/incr.md`'s function).
#[must_use]
pub fn frag_identity<'n>(
    split: &crate::split::Split,
    assignment: &Assignment<'n>,
    outsecs: &Ordinals<&'n [u8]>,
    frag: FragId,
    hash: ContentHash,
) -> Option<FragIdentity<'n>> {
    let out = assignment.out(split.frags.get(frag)?.section)?;
    Some(FragIdentity { out: outsec_identity(outsecs, out)?, hash })
}

/// The fragments of a link by identity: sorted `(identity, FragId)` for
/// the way back. `hashes` is per fragment.
#[must_use]
pub fn frag_lookup<'n>(
    split: &crate::split::Split,
    assignment: &Assignment<'n>,
    outsecs: &Ordinals<&'n [u8]>,
    hashes: &Column<FragId, ContentHash>,
) -> Box<[(FragIdentity<'n>, FragId)]> {
    let mut rows: Vec<_> = split
        .frags
        .ids()
        .iter()
        .filter_map(|f| Some((frag_identity(split, assignment, outsecs, f, *hashes.get(f)?)?, f)))
        .collect();
    rows.sort_unstable();
    rows.into()
}

/// The fragment of this link an identity denotes, in a [`frag_lookup`].
#[must_use]
pub fn frag_id(
    lookup: &[(FragIdentity<'_>, FragId)],
    identity: &FragIdentity<'_>,
) -> Option<FragId> {
    let at = lookup.binary_search_by(|row| row.0.cmp(identity)).ok()?;
    Some(lookup[at].1)
}

// ---- §7.2: the dependees of the dependency index ---------------------------

/// The identity of a [`SymKey`](crate::id::SymKey): a global's key bytes or
/// a local's file, name and ordinal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KeyIdentity<'a> {
    Global(SymbolIdentity<'a>),
    Local(LocalIdentity<'a>),
}

/// The identity of a site: a section and an offset in it (a redirected
/// branch, an erratum's key instruction).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SiteIdentity<'a> {
    pub section: SectionIdentity<'a>,
    pub offset: u64,
}

/// The identity of a patch: its chunk and its offset there (two patches are
/// never at one place, §4.4 rule 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PatchIdentity<'a> {
    pub chunk: ChunkIdentity<'a>,
    pub offset: u64,
}

/// The identity of a symbol-anchored allocation (§4.4 `sym_place`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AllocIdentity<'a> {
    Common(SymbolIdentity<'a>),
    /// A copy-relocation allocation: the shared object and the `st_value`.
    Copy {
        file: FileIdentity<'a>,
        value: u64,
    },
}

/// A thing whose address or content can change, in identity terms: a key
/// of the dependency index (§7.2). The index maps each to the units whose
/// bytes depend on it; its form on disk is `spec/incr.md`'s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dependee<'a> {
    Section(SectionIdentity<'a>),
    Frag(FragIdentity<'a>),
    /// A slot of a key.
    Slot {
        kind: SlotKind,
        key: KeyIdentity<'a>,
    },
    /// The output's local-dynamic module slot.
    ModuleSlot,
    Alloc(AllocIdentity<'a>),
    /// A global symbol's definition.
    Definition(SymbolIdentity<'a>),
    /// A patch: its redirected sites depend on it.
    Patch(PatchIdentity<'a>),
    /// A redirected site: its patch's chunk depends on it.
    Redirect(SiteIdentity<'a>),
    /// One flag of [`DemandFlags::CROSS_FILE`] of a key: the units of other
    /// files whose bytes it changes depend on it (M-8).
    Flag {
        key: KeyIdentity<'a>,
        flag: DemandFlags,
    },
}

/// The [`Dependee::Flag`] rows of a key, one per flag of
/// [`DemandFlags::CROSS_FILE`] among `flags`, in declaration order.
pub fn flag_dependees(
    key: KeyIdentity<'_>,
    flags: DemandFlags,
) -> impl Iterator<Item = Dependee<'_>> {
    DemandFlags::NAMED
        .iter()
        .map(|row| row.1)
        .filter(move |&f| DemandFlags::CROSS_FILE.contains(f) && flags.contains(f))
        .map(move |flag| Dependee::Flag { key, flag })
}

impl<'a> Identities<'a> {
    /// The identity of a key of this link.
    #[must_use]
    pub fn key(
        &self,
        symbols: &Column<SymbolId, SymbolKey<'a>>,
        key: SymKey,
    ) -> Option<KeyIdentity<'a>> {
        match key.kind() {
            SymKind::Global(id) => symbol_identity(symbols, id).map(KeyIdentity::Global),
            SymKind::Local(sym) => self.local(sym).map(KeyIdentity::Local),
        }
    }

    /// The key of this link an identity denotes.
    #[must_use]
    pub fn key_id(
        &self,
        symbols: &Column<SymbolId, SymbolKey<'_>>,
        lookup: &Lookup<SymbolId>,
        identity: &KeyIdentity<'a>,
    ) -> Option<SymKey> {
        match identity {
            KeyIdentity::Global(g) => symbol_id(symbols, lookup, g).map(SymKey::global),
            KeyIdentity::Local(l) => self.local_sym(l).map(SymKey::local),
        }
    }

    /// The identity of a site of this link.
    #[must_use]
    pub fn site(
        &self,
        resolution: &Resolution,
        section: SectionId,
        offset: u64,
    ) -> Option<SiteIdentity<'a>> {
        Some(SiteIdentity { section: self.section(resolution, section)?, offset })
    }

    /// The site of this link an identity denotes.
    #[must_use]
    pub fn site_id(
        &self,
        resolution: &Resolution,
        identity: &SiteIdentity<'a>,
    ) -> Option<(SectionId, u64)> {
        Some((self.section_id(resolution, &identity.section)?, identity.offset))
    }

    /// The identity of a symbol-anchored allocation of this link.
    #[must_use]
    pub fn alloc(
        &self,
        symbols: &Column<SymbolId, SymbolKey<'a>>,
        slots: &Slots,
        key: AllocKey,
    ) -> Option<AllocIdentity<'a>> {
        match key {
            AllocKey::Common(id) => symbol_identity(symbols, id).map(AllocIdentity::Common),
            AllocKey::Copy(at) => {
                let row = slots.copies.get(at as usize)?;
                Some(AllocIdentity::Copy { file: self.file(row.file)?, value: row.value })
            }
        }
    }

    /// The allocation of this link an identity denotes.
    #[must_use]
    pub fn alloc_key(
        &self,
        symbols: &Column<SymbolId, SymbolKey<'_>>,
        lookup: &Lookup<SymbolId>,
        slots: &Slots,
        identity: &AllocIdentity<'a>,
    ) -> Option<AllocKey> {
        match identity {
            AllocIdentity::Common(g) => symbol_id(symbols, lookup, g).map(AllocKey::Common),
            AllocIdentity::Copy { file, value } => {
                let want = CopyAlloc { file: self.file_id(file)?, value: *value };
                let at = slots.copies.binary_search(&want).ok()?;
                Some(AllocKey::Copy(at as u32))
            }
        }
    }
}

/// The identity of a patch, given [`chunk_ordinals`].
#[must_use]
pub fn patch_identity<'n>(
    ordinals: &Ordinals<(ChunkTag, Option<&'n [u8]>)>,
    patch: &Patch,
) -> Option<PatchIdentity<'n>> {
    Some(PatchIdentity { chunk: chunk_identity(ordinals, patch.chunk)?, offset: patch.offset })
}

/// The patch of this link an identity denotes: its position in
/// `Placement::patches`, which is sorted by `(chunk, offset)`.
#[must_use]
pub fn patch_at(
    ordinals: &Ordinals<(ChunkTag, Option<&[u8]>)>,
    placement: &Placement,
    identity: &PatchIdentity<'_>,
) -> Option<u32> {
    let chunk = chunk_id(ordinals, &identity.chunk)?;
    let at = placement
        .patches
        .binary_search_by(|p| (p.chunk, p.offset).cmp(&(chunk, identity.offset)))
        .ok()?;
    Some(at as u32)
}
