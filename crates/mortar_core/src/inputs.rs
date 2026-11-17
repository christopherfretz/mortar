//! `Inputs` (`spec/model.md` §3.2): everything known from reading files,
//! before any symbol is resolved. Written by `mortar_input`; immutable once
//! that phase ends.
//!
//! The rows of `files` are a function of each file's bytes and its own
//! mention (I-file); what depends on several mentions is in
//! [`LinkInputs`] (`spec/input.md` §10.4). The store that holds the bytes
//! is `mortar_input`'s: the tables name it only through [`InputStore`].

use std::fmt;
use std::ops::Range;

use mortar_diag::FileRef;
use mortar_elf::{Class, Machine, Object, SharedObject};

use crate::column::Column;
use crate::flags::flag_set;
use crate::id::{ArchiveId, ComdatId, FileId, IdRange, Span, SymbolId};
use crate::table::{Broken, ensure};

/// How relocations carry their addends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RelocForm {
    /// `SHT_RELA`: explicit addends.
    Rela,
    /// `SHT_REL`: addends implicit in the relocated field.
    Rel,
}

/// The target of the link (§3.2 `target`): machine, class and relocation
/// form. Decided by `-m`, or else by `spec/input.md` §3.1; every ELF file
/// must agree (§3.3), except candidates of a search, which are skipped
/// (§4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LinkTarget {
    pub machine: Machine,
    pub class: Class,
    pub reloc: RelocForm,
}

impl LinkTarget {
    /// The target of a machine, in the one class mortar links it in, with
    /// the relocation form of its psABI.
    #[must_use]
    pub const fn of(machine: Machine) -> Self {
        match machine {
            Machine::AArch64 | Machine::X86_64 => {
                Self { machine, class: Class::Elf64, reloc: RelocForm::Rela }
            }
            Machine::I386 => Self { machine, class: Class::Elf32, reloc: RelocForm::Rel },
        }
    }
}

// -- the store -----------------------------------------------------------

/// The store's handle of one distinct file read (`spec/input.md` §1,
/// §9.3). Only the store gives them out; the tables carry them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Blob(pub u32);

/// An operating-system error number (`errno`), as a probe or a read met it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Errno(pub i32);

/// Why [`InputStore::copy_to`] copied nothing (`spec/input.md` §11.3,
/// §11.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyError {
    /// The range is not inside the file, or `out` is not its length.
    OutOfRange,
    /// The read failed.
    Read(Errno),
    /// The file changed while it was read (`input-changed`).
    Changed,
}

/// What the tables need of `mortar_input`'s store (§1; `spec/input.md`
/// §1, §11): the bytes of a range, as a slice when the range lies inside
/// one resident part, or copied into the caller's buffer. The store is
/// shared and immutable to its users: nothing after `load` takes it
/// mutably.
pub trait InputStore: Sync {
    /// The bytes `range` of `blob`, when the range lies inside one resident
    /// part; `None` otherwise (`spec/input.md` §11.3).
    fn resident(&self, blob: Blob, range: Range<u64>) -> Option<&[u8]>;

    /// Copies the bytes `range` of `blob` into `out`, which is as long as
    /// the range; any range inside the file.
    fn copy_to(&self, blob: Blob, range: Range<u64>, out: &mut [u8]) -> Result<(), CopyError>;
}

/// The store that holds nothing: for tables with no bytes behind them
/// (the internal file alone, tests of the tables).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoStore;

impl InputStore for NoStore {
    fn resident(&self, _blob: Blob, _range: Range<u64>) -> Option<&[u8]> {
        None
    }

    fn copy_to(&self, _blob: Blob, _range: Range<u64>, _out: &mut [u8]) -> Result<(), CopyError> {
        Err(CopyError::OutOfRange)
    }
}

/// A shared reference to the store, as `Inputs` holds it (`&'s Store`).
#[derive(Clone, Copy)]
pub struct StoreRef<'s>(pub &'s dyn InputStore);

impl StoreRef<'_> {
    /// [`NoStore`].
    #[must_use]
    pub fn none() -> StoreRef<'static> {
        StoreRef(&NoStore)
    }
}

impl fmt::Debug for StoreRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StoreRef(..)")
    }
}

/// Where a file's bytes are: its blob, the offset of the file in it (an
/// archive member: its archive's blob and the member's content offset),
/// and its length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileBytes {
    pub blob: Blob,
    pub base: u64,
    pub len: u64,
}

// -- per-file rows ---------------------------------------------------------

/// What a file is (§3.2 `files`: kind).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileKind {
    /// `FileId(0)`: no sections; the symbols the linker defines (§2.1).
    Internal,
    /// A relocatable object, given directly or an archive member.
    Object,
    /// A shared object.
    Shared,
}

/// The parsed reader of a file, over its resident parts
/// (`spec/input.md` §11.3).
#[derive(Debug, Clone, Copy)]
#[allow(clippy::large_enum_variant)] // one row per file, not per symbol; `Copy` matters more
pub enum Reader<'s> {
    /// The internal file has no bytes.
    Internal,
    /// A relocatable object.
    Object(Object<'s>),
    /// A shared object (`spec/elf.md` §6).
    Shared(SharedObject<'s>),
}

flag_set! {
    /// The positional state of a file's own mention (§3.2;
    /// `spec/input.md` §10.3; `spec/cli.md` §3.3). A shared object has its
    /// first mention's; what is merged over its mentions is in
    /// [`SonameUnit`].
    pub struct InputFlags: u8 {
        /// `--as-needed`.
        const AS_NEEDED = 1 << 0;
        /// `--whole-archive`.
        const WHOLE_ARCHIVE = 1 << 1;
        /// A lazy object of a `--start-lib` region. (An archive member is
        /// lazy by being a member: [`File::is_lazy`].)
        const LAZY = 1 << 2;
        /// `-Bstatic` was in force.
        const STATIC = 1 << 3;
        /// `--exclude-libs` names the file's archive.
        const EXCLUDE_LIBS = 1 << 4;
        /// `--copy-dt-needed-entries`.
        const COPY_DT_NEEDED = 1 << 5;
    }
}

/// An archive member's place: which archive mention, its name, its ordinal
/// among the members of the same name, and the offset of its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Member<'s> {
    pub archive: ArchiveId,
    pub name: &'s [u8],
    /// 0 for the first member of this name, 1 for the next, ... (§7.1).
    pub ordinal: u32,
    /// The offset of the member's header in the archive.
    pub offset: u64,
    /// For a member of a thin archive: the member's own found path
    /// (`spec/input.md` §7.2).
    pub thin: Option<&'s [u8]>,
}

/// The mention that brought a file (or an archive, or an opened file) into
/// the link (`spec/input.md` §10.3 `origin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    /// `FileId(0)`.
    Internal,
    /// A `Plan` input: its index in `Plan::inputs` and its argument index;
    /// `entry` is the script entry that named the file, an index in
    /// [`LinkInputs::script_entries`], when a script did.
    Input { input: u32, arg: u32, entry: Option<u32> },
    /// The `DT_NEEDED` closure (`spec/input.md` §8.6): entry `index` of the
    /// dynamic table of `needing`.
    Closure { needing: FileId, index: u32 },
}

/// A content hash, for detecting change (§7.1). Its function is
/// `spec/incr.md`'s; the model fixes only that it is 128 bits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct ContentHash(pub u128);

/// One loaded file (§3.2 `files[FileId]`).
#[derive(Debug, Clone, Copy)]
pub struct File<'s> {
    /// The found path (`spec/input.md` §4.5): of the file, or of its
    /// archive for a member. Empty for the internal file.
    pub path: &'s [u8],
    /// For an archive member: where in which archive.
    pub member: Option<Member<'s>>,
    /// The mention that named it.
    pub origin: Origin,
    /// The positional state of that mention.
    pub state: InputFlags,
    /// The parsed reader. Its variant is the file's [`FileKind`].
    pub reader: Reader<'s>,
    /// Where its bytes are in the store; `None` for the internal file.
    pub bytes: Option<FileBytes>,
    /// `e_shnum`, section 0 included; 0 for the internal file and shared
    /// objects (they get no section rows, §2.2).
    pub sections: u32,
    /// The file's entries in `sym_map`.
    pub syms: Span,
    /// The hash of the file's bytes (`spec/input.md` §11.6), **or
    /// `ContentHash(0)` when it was not computed**: the staged store (the
    /// default) does not hash unless `StoreConfig::hashes` asks it to.
    /// A 0 is never evidence that a file is unchanged; whether the hash was
    /// computed is the store's `hashes()`, not this value, and code that
    /// needs one (an incremental link, M7) asks the store for it.
    pub hash: ContentHash,
}

impl File<'_> {
    /// The kind, from the reader.
    #[must_use]
    pub fn kind(&self) -> FileKind {
        match self.reader {
            Reader::Internal => FileKind::Internal,
            Reader::Object(_) => FileKind::Object,
            Reader::Shared(_) => FileKind::Shared,
        }
    }

    /// A lazy object: an archive member, or an object of a `--start-lib`
    /// region (`spec/resolve.md` §4.1).
    #[must_use]
    pub fn is_lazy(&self) -> bool {
        self.member.is_some() || self.state.contains(InputFlags::LAZY)
    }

    /// The display name (`spec/diag.md` §2.3). Bytes that are not UTF-8
    /// become U+FFFD; only rendering looks at it.
    #[must_use]
    pub fn display(&self) -> FileRef {
        let path = String::from_utf8_lossy(self.path);
        match self.member {
            Some(member) => FileRef::in_archive(path, String::from_utf8_lossy(member.name)),
            None => FileRef::new(path),
        }
    }
}

/// A global symbol key (§2.3; §3.2 `symbols[SymbolId]`): the name before
/// any `@`, and the version for a non-default version (`foo@V1`); an
/// unversioned and a default-version mention (`foo`, `foo@@V2`) share the
/// row with `version: None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolKey<'s> {
    pub name: &'s [u8],
    pub version: Option<&'s [u8]>,
    /// The key's hash, computed once (§9) by the fixed function of §6
    /// rule 5.
    pub hash: u64,
}

/// A COMDAT group signature (§3.2 `comdats[ComdatId]`): the effective
/// signature, the name of its section for an `STT_SECTION` signature
/// symbol (§2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comdat<'s> {
    pub name: &'s [u8],
}

/// The format of an archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveKind {
    Gnu,
    Thin,
    Bsd,
}

/// An archive mention (§3.2 `archives[ArchiveId]`; `spec/input.md` §10.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Archive<'s> {
    /// The found path.
    pub path: &'s [u8],
    pub kind: ArchiveKind,
    /// The mention.
    pub origin: Origin,
    /// Its relocatable members, in archive order: contiguous `FileId`s.
    pub members: IdRange<FileId>,
}

// -- the per-link part ----------------------------------------------------

/// A `--start-group` region or script `GROUP`, numbered from 1 in load
/// order (`spec/input.md` §10.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GroupOrdinal(pub u32);

/// One loaded shared object and every mention of its soname
/// (`spec/input.md` §8.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SonameUnit {
    pub file: FileId,
    /// Every mention of the soname, in load order; the first is the file's
    /// own.
    pub mentions: Box<[Origin]>,
    /// On only if on at every mention.
    pub as_needed: bool,
    /// On if on at any mention.
    pub copy_dt_needed: bool,
}

/// One `PT_LOAD` of a shared object, as `spec/dyn.md` D-43a reads it: the
/// header's `p_vaddr` and `p_memsz`, unchecked (`vaddr + memsz` may
/// overflow in a hostile file: the reader checks it), and `PF_W`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LoadSpan {
    pub vaddr: u64,
    pub memsz: u64,
    pub writable: bool,
}

/// Which definitions of a shared object the object binds to itself
/// (`spec/dyn.md` D-43a, "binds locally").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindsLocally {
    /// `DT_SYMBOLIC`, or `DF_SYMBOLIC` in `DT_FLAGS`: every definition.
    Symbolic,
    /// The `.dynsym` indices that a row of its `.note.mason` table marks
    /// `NONPREEMPTIBLE` (`spec/odr.md` O-51, O-60), sorted, without
    /// repeats; never empty.
    Rows(Box<[u32]>),
    /// Its `.note.mason` table does not parse (the reason, O-51's): taken
    /// as binding every definition, since mortar cannot tell which.
    Unreadable(Box<str>),
}

/// What `spec/dyn.md` D-43a reads of a loaded shared object that binds
/// some of its definitions locally: from its program headers and
/// `.dynamic` only. A shared object that binds nothing locally has no
/// row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalBinding {
    pub file: FileId,
    pub binds: BindsLocally,
    /// Every `PT_LOAD`, in table order.
    pub loads: Box<[LoadSpan]>,
    /// The last `PT_GNU_RELRO`'s `(p_vaddr, p_memsz)` (glibc's reading),
    /// unchecked like `loads`.
    pub relro: Option<(u64, u64)>,
}

impl LocalBinding {
    /// Whether the object binds its definition at `.dynsym` index `index`
    /// to itself.
    #[must_use]
    pub fn binds(&self, index: u32) -> bool {
        match &self.binds {
            BindsLocally::Symbolic | BindsLocally::Unreadable(_) => true,
            BindsLocally::Rows(rows) => rows.binary_search(&index).is_ok(),
        }
    }
}

/// What a `DT_NEEDED` edge of the closure led to (`spec/input.md` §8.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeOutcome {
    /// A loaded shared object: the closure file found, or one already
    /// loaded with that soname.
    Satisfied(FileId),
    /// `needed-not-found`.
    NotFound,
}

/// One `DT_NEEDED` entry the closure processed: entry `index` of the
/// dynamic table of `needing`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClosureEdge {
    pub needing: FileId,
    pub index: u32,
    pub outcome: EdgeOutcome,
}

/// What an opened file turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpenedKind {
    /// A relocatable object that got a row (a thin archive's member file
    /// too).
    Object(FileId),
    /// An archive mention.
    Archive(ArchiveId),
    /// A shared object that got a row.
    Shared(FileId),
    /// A shared object whose soname was already loaded, as this file.
    SharedNotLoaded(FileId),
    /// A linker script.
    Script,
    /// An empty file.
    Empty,
    /// A file opened that contributes nothing (a thin member that is not a
    /// relocatable object).
    Ignored,
}

/// One file opened, in load order (`spec/input.md` §10.4 `opened`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opened<'s> {
    /// The found path.
    pub path: &'s [u8],
    pub kind: OpenedKind,
    pub origin: Origin,
}

/// The search a probe belongs to (`spec/input.md` §4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SearchRef {
    /// The `Plan` input of that index (`-l`, `-l:`).
    Input(u32),
    /// The script entry of that index in [`LinkInputs::script_entries`].
    Entry(u32),
    /// The closure edge of that index in [`LinkInputs::closure`].
    Edge(u32),
}

/// The target a skipped candidate was of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetName<'s> {
    /// An ELF file: its `e_machine` and `EI_CLASS`.
    Elf { machine: u16, class: u8 },
    /// A script whose `OUTPUT_FORMAT` names another target: the name.
    Format(&'s [u8]),
}

/// What a probe found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProbeOutcome<'s> {
    /// No such file.
    Absent,
    /// A candidate of another target (`skipped-incompatible`).
    Skipped(TargetName<'s>),
    /// The candidate the search chose: an index in [`LinkInputs::opened`]
    /// (a script or an archive has no `FileId`).
    Found(u32),
    /// The candidate could not be examined.
    Error(Errno),
}

/// One `stat` of a candidate (`spec/input.md` §4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Probe<'s> {
    /// The candidate as spelled (§4.5).
    pub path: &'s [u8],
    pub search: SearchRef,
    pub outcome: ProbeOutcome<'s>,
}

/// An entry of a script's `INPUT`, `GROUP`, `AS_NEEDED` or `INCLUDE`
/// (`spec/input.md` §10.3): the script (an index in
/// [`LinkInputs::opened`]), the entry's byte offset in it, the entry
/// that named the script, if a script did, and the state of the entry's
/// own mention (`spec/dyn.md` D-19a: a shared object mentioned again
/// inside a script is placed by it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScriptEntry {
    pub script: u32,
    pub offset: u64,
    pub parent: Option<u32>,
    /// The as-needed state in force where the script was named,
    /// `AS_NEEDED(…)` turning it on (`--push-state`/`--pop-state` already
    /// applied).
    pub as_needed: bool,
    /// The outermost group the entry is in (the numbering of
    /// [`LinkInputs::group`]), or `None`: in none, or in one that holds no
    /// loaded file.
    pub group: Option<GroupOrdinal>,
}

/// Which script command a directive is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DirectiveKind {
    Output,
    Entry,
    Extern,
}

/// `OUTPUT`, `ENTRY` or `EXTERN` of an input script, with its position
/// (`spec/input.md` §5.1): the script (an index in [`LinkInputs::opened`])
/// and the byte offset of the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Directive<'s> {
    pub kind: DirectiveKind,
    pub value: &'s [u8],
    pub script: u32,
    pub offset: u64,
}

/// What the input phase produces that depends on more than one mention
/// (`spec/input.md` §10.4). I-file does not cover it: an incremental link
/// recomputes it whole.
#[derive(Debug, Clone, Default)]
pub struct LinkInputs<'s> {
    /// The outermost group of each file's mention.
    pub group: Column<FileId, Option<GroupOrdinal>>,
    /// One per loaded shared object, in `FileId` order.
    pub sonames: Box<[SonameUnit]>,
    /// Every `DT_NEEDED` edge processed, sorted by `(needing, index)`.
    pub closure: Box<[ClosureEdge]>,
    /// Every file opened, in load order.
    pub opened: Box<[Opened<'s>]>,
    /// Every probe made, in load order.
    pub probes: Box<[Probe<'s>]>,
    pub script_entries: Box<[ScriptEntry]>,
    pub directives: Box<[Directive<'s>]>,
    /// The directories `-R` named (`spec/input.md` §4.8): argument index
    /// and directory.
    pub rpath_from_r: Box<[(u32, &'s [u8])]>,
    /// The side table of `spec/dyn.md` D-43a: one row per loaded shared
    /// object that binds a definition locally, in `FileId` order. A row
    /// depends on its file alone; it is here, not in the `Copy` file row,
    /// so that a link allocates nothing for it unless a library binds
    /// locally.
    pub local_binding: Box<[LocalBinding]>,
}

impl LinkInputs<'_> {
    /// The per-link part of `files` file rows with nothing recorded: no
    /// groups, no shared objects, nothing opened. For tables built without
    /// the input phase.
    #[must_use]
    pub fn of_files(files: u32) -> Self {
        Self { group: Column::filled(files, None), ..Self::default() }
    }
}

/// The table (§3.2).
#[derive(Debug, Clone)]
pub struct Inputs<'s> {
    pub target: LinkTarget,
    pub files: Column<FileId, File<'s>>,
    /// The `SymbolId` of the key of each non-local symbol-table entry, per
    /// file in `FileId` order and in table order inside a file.
    pub sym_map: Box<[SymbolId]>,
    pub symbols: Column<SymbolId, SymbolKey<'s>>,
    pub comdats: Column<ComdatId, Comdat<'s>>,
    pub archives: Column<ArchiveId, Archive<'s>>,
    /// The part that depends on more than one mention.
    pub link: LinkInputs<'s>,
    /// The store the files' bytes are in.
    pub store: StoreRef<'s>,
}

impl<'s> Inputs<'s> {
    /// The ids of the keys file `file` mentions, in its table order (the
    /// non-local entries from `first_global` on).
    #[must_use]
    pub fn sym_map_of(&self, file: FileId) -> &[SymbolId] {
        &self.sym_map[self.files[file].syms.slots()]
    }

    /// The bytes `range` of file `file` (offsets in the file), when the
    /// range lies inside one resident part (§1).
    #[must_use]
    pub fn resident(&self, file: FileId, range: Range<u64>) -> Option<&'s [u8]> {
        let (blob, range) = self.in_store(file, range)?;
        self.store.0.resident(blob, range)
    }

    /// Copies the bytes `range` of file `file` into `out` (§1).
    pub fn copy_to(
        &self,
        file: FileId,
        range: Range<u64>,
        out: &mut [u8],
    ) -> Result<(), CopyError> {
        let (blob, range) = self.in_store(file, range).ok_or(CopyError::OutOfRange)?;
        self.store.0.copy_to(blob, range, out)
    }

    fn in_store(&self, file: FileId, range: Range<u64>) -> Option<(Blob, Range<u64>)> {
        let bytes = self.files.get(file)?.bytes?;
        if range.start > range.end || range.end > bytes.len {
            return None;
        }
        Some((bytes.blob, bytes.base + range.start..bytes.base + range.end))
    }

    /// The first file of the `DT_NEEDED` closure; one past the last file
    /// when there is none (`spec/input.md` §10.1).
    #[must_use]
    pub fn first_closure_file(&self) -> FileId {
        let first = self.files.iter().find(|(_, f)| matches!(f.origin, Origin::Closure { .. }));
        first.map_or(FileId(self.files.len()), |(id, _)| id)
    }

    /// The `--start-group` region of a file's mention.
    #[must_use]
    pub fn group_of(&self, file: FileId) -> Option<GroupOrdinal> {
        self.link.group.get(file).copied().flatten()
    }

    /// The soname unit of a loaded shared object.
    #[must_use]
    pub fn soname_unit(&self, file: FileId) -> Option<&SonameUnit> {
        let at = self.link.sonames.binary_search_by_key(&file, |unit| unit.file).ok()?;
        Some(&self.link.sonames[at])
    }

    /// The D-43a row of a shared object, when it binds a definition
    /// locally (`spec/dyn.md` D-43a).
    #[must_use]
    pub fn local_binding(&self, file: FileId) -> Option<&LocalBinding> {
        let rows = &self.link.local_binding;
        let at = rows.binary_search_by_key(&file, |row| row.file).ok()?;
        Some(&rows[at])
    }

    /// The invariants of §2.1 to §3.2 and `spec/input.md` §10 that the
    /// table alone shows. Per file:
    ///
    /// 1. `FileId(0)`, and only it, is the internal file, with origin
    ///    `Internal`, no bytes, no sections and no symbols; only objects
    ///    have sections, as many as their reader;
    /// 2. the files' `sym_map` spans are contiguous in `FileId` order and
    ///    cover `sym_map`;
    /// 3. an archive member is an object of an archive in range, with that
    ///    archive's path; the members of one archive are its `members`
    ///    range, in increasing header offset; archives follow one another
    ///    in `FileId` order;
    /// 4. `LAZY` is for an object that is not a member;
    /// 5. files named by `Plan` inputs come in input order; closure files
    ///    come after every other file, are shared objects, and each is
    ///    needed by an earlier shared object;
    ///
    /// for the symbols:
    ///
    /// 6. `SymbolId`s are numbered by first mention (§2.3): walking
    ///    `sym_map`, an id never seen before is the next id, and every row
    ///    is mentioned but the last ones, which are distinct unversioned
    ///    names only options give (`--wrap`);
    ///
    /// and for `link`:
    ///
    /// 7. `group` has a row per file; ordinals start at 1 and first appear
    ///    in increasing order;
    /// 8. `sonames` has one unit per shared object, in `FileId` order,
    ///    each with at least one mention;
    /// 9. `closure` is sorted by `(needing, index)`, needed by and
    ///    satisfied with shared objects;
    /// 10. every index into `opened`, `script_entries` and `closure` is in
    ///     range, and names the right kind of row;
    /// 11. `local_binding` is in `FileId` order, one row per file at most,
    ///     each of a shared object.
    pub fn check(&self) -> Result<(), Broken> {
        self.check_files()?;
        self.check_symbols()?;
        self.check_link()
    }

    fn check_files(&self) -> Result<(), Broken> {
        let t = "Inputs";
        let mut at = 0u64;
        let mut last_input = 0u32;
        let mut closure = false;
        let mut last_member: Option<(ArchiveId, u64)> = None;
        for (id, file) in self.files.iter() {
            let n = id.0;
            let internal = id == FileId::INTERNAL;
            ensure(internal == (file.kind() == FileKind::Internal), t, || {
                format!("rule 1: file {n}: only file 0 is the internal file")
            })?;
            ensure(internal == (file.origin == Origin::Internal), t, || {
                format!("rule 1: file {n}: only file 0 has origin Internal")
            })?;
            ensure(internal == file.bytes.is_none(), t, || {
                format!("rule 1: file {n}: only the internal file has no bytes")
            })?;
            if let Reader::Object(object) = file.reader {
                ensure(object.section_count() == file.sections, t, || {
                    format!("rule 1: file {n}: section count is not the reader's")
                })?;
            } else {
                ensure(file.sections == 0, t, || {
                    format!("rule 1: file {n}: sections without an object")
                })?;
            }
            if internal {
                ensure(file.syms.len == 0, t, || {
                    "rule 1: the internal file mentions symbols".to_owned()
                })?;
            }
            ensure(u64::from(file.syms.start) == at, t, || {
                format!("rule 2: file {n}: sym_map span is not contiguous")
            })?;
            at = file.syms.end();
            if let Some(member) = file.member {
                let archive = self.archives.get(member.archive);
                let archive = archive.ok_or_else(|| Broken {
                    table: t,
                    what: format!("rule 3: file {n}: archive {} out of range", member.archive.0),
                })?;
                ensure(file.kind() == FileKind::Object, t, || {
                    format!("rule 3: file {n}: an archive member that is not an object")
                })?;
                ensure(archive.path == file.path, t, || {
                    format!("rule 3: file {n}: a member's path is not its archive's")
                })?;
                ensure(archive.members.contains(id), t, || {
                    format!("rule 3: file {n}: not in the members of archive {}", member.archive.0)
                })?;
                if let Some((previous, offset)) = last_member {
                    ensure(previous <= member.archive, t, || {
                        format!("rule 3: file {n}: archives out of load order")
                    })?;
                    ensure(previous != member.archive || offset < member.offset, t, || {
                        format!("rule 3: file {n}: members out of archive order")
                    })?;
                }
                last_member = Some((member.archive, member.offset));
            }
            if file.state.contains(InputFlags::LAZY) {
                ensure(file.member.is_none() && file.kind() == FileKind::Object, t, || {
                    format!("rule 4: file {n}: LAZY on a member or a file that is not an object")
                })?;
            }
            match file.origin {
                Origin::Internal => {}
                Origin::Input { input, .. } => {
                    ensure(!closure, t, || {
                        format!("rule 5: file {n}: a file of an input after the closure")
                    })?;
                    ensure(input >= last_input, t, || {
                        format!("rule 5: file {n}: inputs out of command-line order")
                    })?;
                    last_input = input;
                }
                Origin::Closure { needing, .. } => {
                    closure = true;
                    ensure(file.kind() == FileKind::Shared, t, || {
                        format!("rule 5: file {n}: a closure file that is not a shared object")
                    })?;
                    let needer = self.files.get(needing).filter(|_| needing < id);
                    ensure(needer.is_some_and(|f| f.kind() == FileKind::Shared), t, || {
                        format!("rule 5: file {n}: not needed by an earlier shared object")
                    })?;
                }
            }
        }
        ensure(at == self.sym_map.len() as u64, t, || {
            "rule 2: sym_map is not covered by the files".to_owned()
        })?;
        for (a, archive) in self.archives.iter() {
            let members = self.files.range(archive.members);
            let all = members.is_some_and(|files| {
                files.iter().all(|f| f.member.is_some_and(|m| m.archive == a))
            });
            ensure(all, t, || format!("rule 3: archive {}: members are not its files", a.0))?;
        }
        Ok(())
    }

    fn check_symbols(&self) -> Result<(), Broken> {
        let t = "Inputs";
        let mut next = 0u32;
        for (position, id) in self.sym_map.iter().enumerate() {
            ensure(id.0 <= next, t, || {
                format!(
                    "rule 6: sym_map[{position}]: symbol {} is not numbered by first mention",
                    id.0
                )
            })?;
            if id.0 == next {
                next += 1;
            }
        }
        // `spec/model.md` §2.3: the rows past the mentioned ones are names
        // only options give (`--wrap`'s), each distinct and unversioned.
        ensure(next <= self.symbols.len(), t, || {
            format!("rule 6: {} symbol rows, {next} mentioned", self.symbols.len())
        })?;
        let extra = &self.symbols.as_slice()[next as usize..];
        for (i, k) in extra.iter().enumerate() {
            let unique = !self.symbols.as_slice()[..next as usize + i]
                .iter()
                .any(|o| o.name == k.name && o.version.is_none());
            ensure(k.version.is_none() && unique, t, || {
                format!("rule 6: option key row {} is versioned or not distinct", next as usize + i)
            })?;
        }
        Ok(())
    }

    fn check_link(&self) -> Result<(), Broken> {
        let t = "Inputs";
        let link = &self.link;
        let shared = |f: FileId| self.files.get(f).is_some_and(|f| f.kind() == FileKind::Shared);
        ensure(link.group.len() == self.files.len(), t, || {
            format!("rule 7: {} group rows for {} files", link.group.len(), self.files.len())
        })?;
        let mut highest = 0u32;
        for (f, group) in link.group.iter() {
            if let Some(GroupOrdinal(g)) = *group {
                ensure(g >= 1 && g <= highest + 1, t, || {
                    format!("rule 7: file {}: group {g} is not numbered in load order", f.0)
                })?;
                highest = highest.max(g);
            }
        }
        let loaded: Vec<FileId> = self
            .files
            .iter()
            .filter(|(_, f)| f.kind() == FileKind::Shared)
            .map(|(i, _)| i)
            .collect();
        let units: Vec<FileId> = link.sonames.iter().map(|unit| unit.file).collect();
        ensure(units == loaded, t, || {
            "rule 8: the soname units are not the shared objects in FileId order".to_owned()
        })?;
        ensure(link.sonames.iter().all(|unit| !unit.mentions.is_empty()), t, || {
            "rule 8: a soname unit without a mention".to_owned()
        })?;
        if let Some(at) = crate::table::unsorted_at(&link.closure, |e| (e.needing, e.index)) {
            return Err(Broken {
                table: t,
                what: format!("rule 9: closure[{at}] is out of order"),
            });
        }
        let rows = &link.local_binding;
        ensure(rows.windows(2).all(|w| w[0].file < w[1].file), t, || {
            "rule 11: local_binding is not in FileId order".to_owned()
        })?;
        ensure(rows.iter().all(|row| shared(row.file)), t, || {
            "rule 11: a local_binding row of a file that is not a shared object".to_owned()
        })?;
        for (i, edge) in link.closure.iter().enumerate() {
            ensure(shared(edge.needing), t, || {
                format!("rule 9: closure[{i}]: needed by a file that is not a shared object")
            })?;
            if let EdgeOutcome::Satisfied(f) = edge.outcome {
                ensure(shared(f), t, || {
                    format!("rule 9: closure[{i}]: satisfied by a file that is not a shared object")
                })?;
            }
        }
        self.check_indices()
    }

    fn check_indices(&self) -> Result<(), Broken> {
        let t = "Inputs";
        let link = &self.link;
        let opened = link.opened.len();
        let entries = link.script_entries.len();
        let is_script =
            |i: u32| link.opened.get(i as usize).is_some_and(|o| o.kind == OpenedKind::Script);
        let origin_ok = |origin: Origin| match origin {
            Origin::Input { entry: Some(e), .. } => (e as usize) < entries,
            Origin::Closure { needing, .. } => self.files.has(needing),
            _ => true,
        };
        for (id, file) in self.files.iter() {
            ensure(origin_ok(file.origin), t, || {
                format!("rule 10: file {}: origin out of range", id.0)
            })?;
        }
        for (a, archive) in self.archives.iter() {
            ensure(origin_ok(archive.origin), t, || {
                format!("rule 10: archive {}: origin out of range", a.0)
            })?;
        }
        for (i, o) in link.opened.iter().enumerate() {
            let kind_ok = match o.kind {
                OpenedKind::Object(f) => {
                    self.files.get(f).is_some_and(|f| f.kind() == FileKind::Object)
                }
                OpenedKind::Shared(f) | OpenedKind::SharedNotLoaded(f) => {
                    self.files.get(f).is_some_and(|f| f.kind() == FileKind::Shared)
                }
                OpenedKind::Archive(a) => self.archives.has(a),
                OpenedKind::Script | OpenedKind::Empty | OpenedKind::Ignored => true,
            };
            ensure(kind_ok && origin_ok(o.origin), t, || {
                format!("rule 10: opened[{i}]: names a row out of range or of another kind")
            })?;
        }
        for (i, entry) in link.script_entries.iter().enumerate() {
            ensure(is_script(entry.script), t, || {
                format!("rule 10: script_entries[{i}]: not in an opened script")
            })?;
            ensure(entry.parent.is_none_or(|p| (p as usize) < i), t, || {
                format!("rule 10: script_entries[{i}]: parent is not an earlier entry")
            })?;
        }
        for (i, d) in link.directives.iter().enumerate() {
            ensure(is_script(d.script), t, || {
                format!("rule 10: directives[{i}]: not in an opened script")
            })?;
        }
        for (i, probe) in link.probes.iter().enumerate() {
            let search_ok = match probe.search {
                SearchRef::Input(_) => true,
                SearchRef::Entry(e) => (e as usize) < entries,
                SearchRef::Edge(e) => (e as usize) < link.closure.len(),
            };
            let outcome_ok = match probe.outcome {
                ProbeOutcome::Found(o) => (o as usize) < opened,
                _ => true,
            };
            ensure(search_ok && outcome_ok, t, || {
                format!("rule 10: probes[{i}]: an index out of range")
            })?;
        }
        Ok(())
    }
}
