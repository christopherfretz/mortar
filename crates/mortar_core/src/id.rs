//! The ids of `spec/model.md` §2: dense, per link, 32 bits.

/// A dense 32-bit id: the index of a row.
pub trait Id: Copy + Ord + std::fmt::Debug + Send + Sync + 'static {
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
