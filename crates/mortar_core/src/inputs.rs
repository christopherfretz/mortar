//! `Inputs` (`spec/model.md` §3.2): everything known from reading files,
//! before any symbol is resolved. Written by `mortar_input`; immutable once
//! that phase ends.

use mortar_diag::FileRef;
use mortar_elf::{Class, Machine, Object};

use crate::column::Column;
use crate::flags::flag_set;
use crate::id::{ArchiveId, ComdatId, FileId, Span, SymbolId};
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
/// form. Decided by `-m`, or else by the first object; every later file
/// must agree or is diagnosed.
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

/// The parsed reader of a file, over the store's bytes.
#[derive(Debug, Clone, Copy)]
pub enum Reader<'store> {
    /// The internal file has no bytes.
    Internal,
    /// A relocatable object.
    Object(Object<'store>),
    /// A shared object: its bytes, until `mortar_elf` has a shared-object
    /// view (`spec/elf.md`).
    Shared(&'store [u8]),
}

flag_set! {
    /// The positional state a file was read under, copied from its `Plan`
    /// input (§3.2; `spec/cli.md` §3.3).
    pub struct InputFlags: u8 {
        /// `--as-needed`.
        const AS_NEEDED = 1 << 0;
        /// `--whole-archive`.
        const WHOLE_ARCHIVE = 1 << 1;
        /// A lazy object: an archive member, or inside `--start-lib`.
        const LAZY = 1 << 2;
        /// `-Bstatic` was in force.
        const STATIC = 1 << 3;
        /// `--exclude-libs` names the file's archive.
        const EXCLUDE_LIBS = 1 << 4;
    }
}

/// An archive member's place: which archive, its name, its ordinal among
/// the members of the same name, and its offset in the archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Member<'store> {
    pub archive: ArchiveId,
    pub name: &'store [u8],
    /// 0 for the first member of this name, 1 for the next, ... (§7.1).
    pub ordinal: u32,
    pub offset: u64,
}

/// A content hash, for detecting change (§7.1). Its function is
/// `spec/incr.md`'s; the model fixes only that it is 128 bits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct ContentHash(pub u128);

/// One loaded file (§3.2 `files[FileId]`).
#[derive(Debug, Clone, Copy)]
pub struct File<'store> {
    /// The path as resolved: of the file, or of its archive for a member.
    /// Empty for the internal file.
    pub path: &'store [u8],
    /// For an archive member: where in which archive.
    pub member: Option<Member<'store>>,
    /// The index in `Plan::inputs` of the input the file came from; `None`
    /// for the internal file.
    pub input: Option<u32>,
    /// The positional state copied from that input.
    pub state: InputFlags,
    /// The parsed reader. Its variant is the file's [`FileKind`].
    pub reader: Reader<'store>,
    /// `e_shnum`, section 0 included; 0 for the internal file and shared
    /// objects (they get no section rows, §2.2).
    pub sections: u32,
    /// The file's entries in `sym_map`.
    pub syms: Span,
    /// The hash of the file's bytes.
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
pub struct SymbolKey<'store> {
    pub name: &'store [u8],
    pub version: Option<&'store [u8]>,
    /// The key's hash, computed once (§9) by the fixed function of §6
    /// rule 5.
    pub hash: u64,
}

/// A COMDAT group signature (§3.2 `comdats[ComdatId]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comdat<'store> {
    pub name: &'store [u8],
}

/// The format of an archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveKind {
    Gnu,
    Thin,
    Bsd,
}

/// An archive opened (§3.2 `archives[ArchiveId]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Archive<'store> {
    pub path: &'store [u8],
    pub kind: ArchiveKind,
}

/// The table (§3.2).
#[derive(Debug, Clone)]
pub struct Inputs<'store> {
    pub target: LinkTarget,
    pub files: Column<FileId, File<'store>>,
    /// The `SymbolId` of the key of each non-local symbol-table entry, per
    /// file in `FileId` order and in table order inside a file.
    pub sym_map: Box<[SymbolId]>,
    pub symbols: Column<SymbolId, SymbolKey<'store>>,
    pub comdats: Column<ComdatId, Comdat<'store>>,
    pub archives: Column<ArchiveId, Archive<'store>>,
}

impl Inputs<'_> {
    /// The ids of the keys file `file` mentions, in its table order (the
    /// non-local entries from `first_global` on).
    #[must_use]
    pub fn sym_map_of(&self, file: FileId) -> &[SymbolId] {
        &self.sym_map[self.files[file].syms.slots()]
    }

    /// The invariants of §2.1 to §3.2 that the table alone shows:
    ///
    /// - `FileId(0)`, and only it, is the internal file, with no sections
    ///   and no symbols; only objects have sections;
    /// - the files' `sym_map` spans are contiguous in `FileId` order and
    ///   cover `sym_map`;
    /// - `SymbolId`s are numbered by first mention (§2.3): walking
    ///   `sym_map`, an id never seen before is the next id, and every row is
    ///   mentioned;
    /// - archive ids in range, and an object's section count is its
    ///   reader's.
    pub fn check(&self) -> Result<(), Broken> {
        let t = "Inputs";
        let mut at = 0u64;
        for (id, file) in self.files.iter() {
            let internal = id == FileId::INTERNAL;
            ensure(internal == (file.kind() == FileKind::Internal), t, || {
                format!("file {}: only file 0 is the internal file", id.0)
            })?;
            if let Reader::Object(object) = file.reader {
                ensure(object.section_count() == file.sections, t, || {
                    format!("file {}: section count is not the reader's", id.0)
                })?;
            } else {
                ensure(file.sections == 0, t, || {
                    format!("file {}: sections without an object", id.0)
                })?;
            }
            if internal {
                ensure(file.syms.len == 0, t, || "the internal file mentions symbols".to_owned())?;
            }
            ensure(u64::from(file.syms.start) == at, t, || {
                format!("file {}: sym_map span is not contiguous", id.0)
            })?;
            at = file.syms.end();
            if let Some(member) = file.member {
                ensure(self.archives.has(member.archive), t, || {
                    format!("file {}: archive {} out of range", id.0, member.archive.0)
                })?;
            }
        }
        ensure(at == self.sym_map.len() as u64, t, || {
            "sym_map is not covered by the files".to_owned()
        })?;
        let mut next = 0u32;
        for (position, id) in self.sym_map.iter().enumerate() {
            ensure(id.0 <= next, t, || {
                format!("sym_map[{position}]: symbol {} is not numbered by first mention", id.0)
            })?;
            if id.0 == next {
                next += 1;
            }
        }
        ensure(next == self.symbols.len(), t, || {
            format!("{} symbol rows, {next} mentioned", self.symbols.len())
        })?;
        Ok(())
    }
}
