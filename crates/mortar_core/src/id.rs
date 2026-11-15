//! The ids of `spec/model.md` §2: dense, per link, 32 bits; ranges of ids,
//! the optional id of a column cell, and the keys of symbols.

use std::fmt;
use std::marker::PhantomData;

use mortar_diag::Diagnostic;
use mortar_elf::SymbolIndex;

/// A dense 32-bit id: the index of a row.
pub trait Id: Copy + Ord + fmt::Debug + Send + Sync + 'static {
    /// The id of row `index`.
    fn from_index(index: u32) -> Self;
    /// The row this id names.
    fn index(self) -> u32;
    /// The row this id names, as a slice index.
    fn slot(self) -> usize {
        self.index() as usize
    }
}

macro_rules! ids {
    ($($(#[$doc:meta])* $name:ident;)*) => {$(
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        #[repr(transparent)]
        pub struct $name(pub u32);

        impl Id for $name {
            fn from_index(index: u32) -> Self {
                Self(index)
            }
            fn index(self) -> u32 {
                self.0
            }
        }
    )*};
}

ids! {
    /// A loaded file, in load order (§2.1). `FileId(0)` is the internal file.
    FileId;
    /// An archive opened, in order of first appearance on the command line.
    ArchiveId;
    /// A global symbol row: a name and version key (§2.3).
    SymbolId;
    /// A distinct COMDAT group signature (§2.3).
    ComdatId;
    /// A section-header-table entry of a live relocatable object (§2.2).
    SectionId;
    /// An output section (§3.4).
    OutSecId;
    /// A distinct piece of mergeable content within one output section (§3.5).
    FragId;
    /// A unit of the output that layout places as a whole (§4.4).
    ChunkId;
    /// A program header of the output.
    SegmentId;
}

impl FileId {
    /// The internal file: no sections; owns the symbols the linker defines.
    pub const INTERNAL: Self = Self(0);
}

/// The most rows an id space holds (§1): 2^32 − 1, so that index
/// `u32::MAX` is never an id and [`OptId`] can use it for "none".
pub const MAX_ROWS: u32 = u32::MAX;

/// A table would need more than [`MAX_ROWS`] rows (§1). The phase that
/// builds the table turns it into the refusal of the link
/// ([`IdOverflow::diagnostic`]); ids never wrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdOverflow {
    /// How many rows were asked for.
    pub rows: usize,
}

impl IdOverflow {
    /// `Refused{Feature, "more than 2^32 - 1 <what>"}` (§1). `what` names
    /// the rows in the plural: `"sections"`, `"global symbols"`.
    #[must_use]
    pub fn diagnostic(self, what: &str) -> Diagnostic {
        Diagnostic::refused(
            mortar_diag::RefusalClass::Feature,
            format!("more than 2^32 - 1 {what}"),
        )
    }
}

/// The number of rows `len` as a row count, or the overflow (§1).
pub fn row_count(len: usize) -> Result<u32, IdOverflow> {
    // MAX_ROWS is u32::MAX: every u32 is a row count.
    u32::try_from(len).map_err(|_| IdOverflow { rows: len })
}

/// The half-open range `start..end` of ids: the rows of one unit, such as
/// a file's sections (§2.2) or its entries in `sym_map`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct IdRange<I> {
    /// The first id of the range.
    pub start: I,
    /// One past the last id.
    pub end: I,
}

impl<I: Id> IdRange<I> {
    /// `start..end`. An `end` before `start` is an empty range at `start`.
    #[must_use]
    pub fn new(start: I, end: I) -> Self {
        Self { start, end: end.max(start) }
    }

    /// The empty range at `start`.
    #[must_use]
    pub fn empty(start: I) -> Self {
        Self { start, end: start }
    }

    /// The `len` ids from `start`, or `None` if the last would not be an id.
    #[must_use]
    pub fn from_len(start: I, len: u32) -> Option<Self> {
        let end = start.index().checked_add(len)?;
        Some(Self { start, end: I::from_index(end) })
    }

    /// The number of ids in the range.
    #[must_use]
    pub fn len(self) -> u32 {
        self.end.index().saturating_sub(self.start.index())
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// Whether `id` is in the range.
    #[must_use]
    pub fn contains(self, id: I) -> bool {
        self.start <= id && id < self.end
    }

    /// The id at position `offset` of the range, if the range has one.
    #[must_use]
    pub fn nth(self, offset: u32) -> Option<I> {
        (offset < self.len()).then(|| I::from_index(self.start.index() + offset))
    }

    /// The position of `id` in the range, if it is in it.
    #[must_use]
    pub fn offset_of(self, id: I) -> Option<u32> {
        self.contains(id).then(|| id.index() - self.start.index())
    }

    /// The ids of the range, in order.
    pub fn iter(self) -> impl DoubleEndedIterator<Item = I> + ExactSizeIterator {
        (self.start.index()..self.end.index()).map(I::from_index)
    }

    /// The range as slice indices.
    #[must_use]
    pub fn slots(self) -> std::ops::Range<usize> {
        self.start.slot()..self.end.slot()
    }
}

impl<I: Id> fmt::Debug for IdRange<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}..{:?}", self.start, self.end)
    }
}

/// An optional id in four bytes: `u32::MAX` (never an id, §1) is "none".
/// The cell type of sparse id columns (`comdat` of a section, `folded`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OptId<I> {
    raw: u32,
    id: PhantomData<fn() -> I>,
}

impl<I: Id> OptId<I> {
    /// No id.
    pub const NONE: Self = Self { raw: u32::MAX, id: PhantomData };

    /// `id`.
    #[must_use]
    pub fn some(id: I) -> Self {
        debug_assert!(id.index() != u32::MAX, "u32::MAX is not an id (spec/model.md §1)");
        Self { raw: id.index(), id: PhantomData }
    }

    /// The id, if there is one.
    #[must_use]
    pub fn get(self) -> Option<I> {
        (self.raw != u32::MAX).then(|| I::from_index(self.raw))
    }

    #[must_use]
    pub fn is_none(self) -> bool {
        self.raw == u32::MAX
    }

    #[must_use]
    pub fn is_some(self) -> bool {
        !self.is_none()
    }
}

impl<I: Id> Default for OptId<I> {
    fn default() -> Self {
        Self::NONE
    }
}

impl<I: Id> From<Option<I>> for OptId<I> {
    fn from(id: Option<I>) -> Self {
        id.map_or(Self::NONE, Self::some)
    }
}

impl<I: Id> fmt::Debug for OptId<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(f)
    }
}

/// A symbol-table entry of a file: `(FileId, st_index)`. The reference
/// that extracted an archive member, a resolved definition (§3.3).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SymEntry {
    pub file: FileId,
    pub index: SymbolIndex,
}

/// A local symbol: it has no id of its own, it is its entry (§2).
pub type LocalSym = SymEntry;

/// The key of a table keyed by "a symbol, local or global" (§2):
/// `Global(SymbolId) | Local(FileId, st_index)`, ordered globals first
/// (by id), then locals by `(FileId, index)`.
///
/// Packed in eight bytes: a global is its id; a local is
/// `(file + 1) << 32 | index`. Since a `FileId` is below `u32::MAX` (§1)
/// the encoding is total and the integer order is the order §2 gives.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymKey(u64);

/// A [`SymKey`] taken apart.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum SymKind {
    Global(SymbolId),
    Local(LocalSym),
}

impl SymKey {
    /// The key of a global symbol row.
    #[must_use]
    pub fn global(id: SymbolId) -> Self {
        Self(u64::from(id.0))
    }

    /// The key of a local symbol.
    #[must_use]
    pub fn local(sym: LocalSym) -> Self {
        debug_assert!(sym.file.0 != u32::MAX, "u32::MAX is not a FileId (spec/model.md §1)");
        Self((u64::from(sym.file.0) + 1) << 32 | u64::from(sym.index.0))
    }

    /// Which symbol the key names.
    #[must_use]
    pub fn kind(self) -> SymKind {
        let high = (self.0 >> 32) as u32;
        if high == 0 {
            SymKind::Global(SymbolId(self.0 as u32))
        } else {
            SymKind::Local(SymEntry { file: FileId(high - 1), index: SymbolIndex(self.0 as u32) })
        }
    }

    /// The global row, for a global.
    #[must_use]
    pub fn as_global(self) -> Option<SymbolId> {
        match self.kind() {
            SymKind::Global(id) => Some(id),
            SymKind::Local(_) => None,
        }
    }
}

impl From<SymbolId> for SymKey {
    fn from(id: SymbolId) -> Self {
        Self::global(id)
    }
}

impl From<LocalSym> for SymKey {
    fn from(sym: LocalSym) -> Self {
        Self::local(sym)
    }
}

impl fmt::Debug for SymKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind() {
            SymKind::Global(id) => write!(f, "Global({})", id.0),
            SymKind::Local(sym) => write!(f, "Local({}, {})", sym.file.0, sym.index.0),
        }
    }
}

/// A run of positions in a column that is not indexed by an id (`sym_map`,
/// the piece columns of `Split`): `start..start + len`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Span {
    pub start: u32,
    pub len: u32,
}

impl Span {
    /// The positions as slice indices.
    #[must_use]
    pub fn slots(self) -> std::ops::Range<usize> {
        self.start as usize..self.end() as usize
    }

    /// One past the last position.
    #[must_use]
    pub fn end(self) -> u64 {
        u64::from(self.start) + u64::from(self.len)
    }
}

const _: () = assert!(size_of::<SymKey>() == 8);
const _: () = assert!(size_of::<OptId<SectionId>>() == 4);
const _: () = assert!(size_of::<IdRange<SectionId>>() == 8);
const _: () = assert!(size_of::<SymEntry>() == 8);
const _: () = assert!(size_of::<Span>() == 8);
