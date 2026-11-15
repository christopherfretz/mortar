//! What resolution reads (stage A of WP-022): a read-only view of the
//! inputs, independent of `mortar_core::Inputs`, so that the algorithms
//! compile and are tested before `Inputs` carries every column they need
//! (WP-020d). Stage B implements [`View`] over `Inputs`.
//!
//! Everything here is a fact of one file (I-file, `spec/model.md` §3.2)
//! except the key and signature ids, which are dense per link. The view is
//! filled into caller-owned buffers, so that a worker reuses one buffer
//! for every file it visits and resolution allocates nothing per file.

use mortar_core::{ArchiveId, ComdatId, FileId, OptId, SymbolId};
use mortar_elf::{Machine, SectionIndex, SymbolIndex, SymbolSection};

/// What a file is, for resolution (`spec/resolve.md` §2, §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    /// `FileId(0)`: no sections; the linker's own symbols (§8).
    Internal,
    /// A relocatable object named on the command line (or by a script):
    /// live from the start.
    Object,
    /// A member of an archive read under `--whole-archive`: live from the
    /// start (§4.2).
    WholeMember,
    /// A member of an archive read without `--whole-archive`: lazy.
    LazyMember,
    /// An object in a `--start-lib` region (M4): lazy, an archive of its
    /// own (§4.8).
    LazyObject,
    /// A shared object (M2, `spec/dyn.md` D-10-D-13): what it is to the
    /// link is [`View::shared`].
    Shared,
}

impl FileKind {
    /// Whether the file is lazy (§2): live only if §4 reaches it.
    #[must_use]
    pub const fn is_lazy(self) -> bool {
        matches!(self, Self::LazyMember | Self::LazyObject)
    }

    /// Whether the file is a relocatable object (named, member or lazy).
    #[must_use]
    pub const fn is_object(self) -> bool {
        matches!(self, Self::Object | Self::WholeMember | Self::LazyMember | Self::LazyObject)
    }
}

/// One file: its kind and the positional facts §4.6 needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileInfo {
    pub kind: FileKind,
    /// The archive mention a member came from (one `ArchiveId` per mention,
    /// `spec/input.md` §7.1). Members of one archive mention have
    /// consecutive `FileId`s in member order.
    pub archive: OptId<ArchiveId>,
    /// The `--start-group` region (a script `GROUP` included) the file was
    /// read in: its ordinal (`spec/input.md` §10.2), or none.
    pub group: Option<u32>,
    /// `e_shnum`, section 0 included; 0 for files without sections.
    pub sections: u32,
    /// The file's identity without its mention ordinal
    /// (`spec/model.md` §7.1): equal for two files exactly when they are
    /// the same found path and, for members, the same member name and
    /// ordinal, as when one archive is mentioned twice. Dense and
    /// arbitrary otherwise; §4.6 compares it.
    pub source: u32,
}

/// A non-local symbol-table entry of a file (§2 "mention").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mention {
    /// Its index in the file's symbol table.
    pub index: SymbolIndex,
    /// The key of its name (`spec/model.md` §2.3).
    pub key: SymbolId,
    /// For a definition `foo@@V` (a default version): the row `foo@V`, if
    /// the link has one, so that a reference requiring `V` binds to it
    /// (§9.2's second lookup, §9.4). `NONE` otherwise.
    pub also: OptId<SymbolId>,
    /// `STB_*`.
    pub binding: u8,
    /// `STT_*`.
    pub sym_type: u8,
    /// `st_other & 3`: `STV_*`.
    pub visibility: u8,
    /// `st_shndx`, decoded.
    pub place: SymbolSection,
    /// `st_value`: the alignment for a common, the value for an absolute,
    /// the offset in its section otherwise.
    pub value: u64,
    /// `st_size`.
    pub size: u64,
}

impl Mention {
    /// The entry as a `(FileId, st_index)` word: the order `≺` (§2) as an
    /// integer (§12.1).
    #[must_use]
    pub fn word(&self, file: FileId) -> u64 {
        entry_word(file, self.index)
    }
}

/// `(FileId << 32) | st_index` (§12.1).
#[must_use]
pub const fn entry_word(file: FileId, index: SymbolIndex) -> u64 {
    ((file.0 as u64) << 32) | index.0 as u64
}

/// The entry of a word made by [`entry_word`].
#[must_use]
pub const fn word_entry(word: u64) -> mortar_core::SymEntry {
    mortar_core::SymEntry { file: FileId((word >> 32) as u32), index: SymbolIndex(word as u32) }
}

/// What a shared object is to the link (`spec/dyn.md` D-11, D-12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SharedInfo {
    /// A closure file (W): loaded by the `DT_NEEDED` walk, never by the
    /// command line or a script. It never gives `def` and is never needed.
    pub closure: bool,
    /// For a member of C: its merged `as_needed` (on only if on at every
    /// mention, `spec/input.md` §8.2). Its undefined references are edges
    /// of §4.1 only when this is off (`spec/dyn.md` D-12, Q1).
    pub as_needed: bool,
}

impl SharedInfo {
    /// Whether its undefined references extract archive members (Q1).
    #[must_use]
    pub const fn extracts(self) -> bool {
        !self.closure && !self.as_needed
    }
}

/// One `SHT_GROUP` section of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupInfo {
    /// The group section's index.
    pub section: SectionIndex,
    /// The flag word.
    pub flags: u32,
    /// The signature, interned by its effective name (§6.1: the section's
    /// name for an `STT_SECTION` signature symbol).
    pub signature: ComdatId,
    /// Its members: `members[start..start + len]` of the [`Groups`] buffer.
    pub start: u32,
    pub len: u32,
}

/// A file's groups and their member section indices, as filled by
/// [`View::groups`].
#[derive(Debug, Clone, Default)]
pub struct Groups {
    pub groups: Vec<GroupInfo>,
    pub members: Vec<u32>,
}

impl Groups {
    /// Empties the buffer, keeping its capacity.
    pub fn clear(&mut self) {
        self.groups.clear();
        self.members.clear();
    }

    /// Appends a group with `members`.
    pub fn push(
        &mut self,
        section: SectionIndex,
        flags: u32,
        signature: ComdatId,
        members: &[u32],
    ) {
        let start = self.members.len() as u32;
        self.members.extend_from_slice(members);
        self.groups.push(GroupInfo { section, flags, signature, start, len: members.len() as u32 });
    }

    /// The member section indices of `group`.
    #[must_use]
    pub fn members_of(&self, group: &GroupInfo) -> &[u32] {
        let start = group.start as usize;
        &self.members[start..start + group.len as usize]
    }
}

/// One section header, as §10 classifies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionInfo<'v> {
    pub name: &'v [u8],
    pub sh_type: u32,
    pub flags: u64,
    pub entsize: u64,
    pub addralign: u64,
    pub size: u64,
    /// `sh_link`.
    pub link: u32,
    /// `sh_info`.
    pub info: u32,
}

/// The read-only view resolution works on.
///
/// Files are `FileId(0)` (the internal file) to `FileId(files() - 1)`, in
/// load order. Keys are `SymbolId(0)` to `SymbolId(keys() - 1)`; the
/// view may have rows that no file mentions (names that only an option
/// gives, such as a `-u` symbol or a `--wrap` target), so that every
/// column of resolution has a row for them. Every method is a function of
/// the inputs: called twice it gives the same answer.
pub trait View: Sync {
    /// The target machine (§8.1's table depends on it).
    fn machine(&self) -> Machine;

    /// The number of files, the internal file included.
    fn files(&self) -> u32;

    /// The number of key rows.
    fn keys(&self) -> u32;

    /// The number of distinct COMDAT signatures.
    fn comdats(&self) -> u32;

    /// File `file`.
    fn file(&self, file: FileId) -> FileInfo;

    /// Appends the non-local entries of `file`, in symbol-table order, to
    /// `out` (which the caller has emptied).
    fn mentions(&self, file: FileId, out: &mut Vec<Mention>);

    /// The non-local entry `index` of `file`, if there is one.
    fn mention(&self, file: FileId, index: SymbolIndex) -> Option<Mention>;

    /// Fills `out` (emptied by the caller) with the `SHT_GROUP` sections
    /// of `file`, in section-index order.
    fn groups(&self, file: FileId, out: &mut Groups);

    /// Section `index` of `file`, for an object.
    fn section(&self, file: FileId, index: SectionIndex) -> Option<SectionInfo<'_>>;

    /// The name of key `key` before any `@`, and its version for a
    /// non-default one.
    fn key_name(&self, key: SymbolId) -> (&[u8], Option<&[u8]>);

    /// The unversioned key named `name`, if the link has a row for it.
    fn find_key(&self, name: &[u8]) -> Option<SymbolId>;

    /// For a shared object, what it is to the link; `None` for every other
    /// file. A shared object's [`View::mentions`] are its `.dynsym` entries
    /// past `sh_info` that matter to resolution (`spec/resolve.md` §9.1):
    /// its references and its visible definitions (hidden and internal
    /// definitions are left out).
    fn shared(&self, file: FileId) -> Option<SharedInfo> {
        let _ = file;
        None
    }

    /// The `DT_NEEDED` edges of a shared object, in table order: the
    /// loaded object each leads to, or `None` when it was not found
    /// (`spec/input.md` §8.6). Appended to `out`.
    fn needed(&self, file: FileId, out: &mut Vec<Option<FileId>>) {
        let _ = (file, out);
    }

    /// The version index (`versym & 0x7fff`) of entry `index` of a shared
    /// object's `.dynsym`; 0 without `.gnu.version` (`spec/dyn.md` D-27).
    fn version(&self, file: FileId, index: SymbolIndex) -> u16 {
        let _ = (file, index);
        0
    }
}
