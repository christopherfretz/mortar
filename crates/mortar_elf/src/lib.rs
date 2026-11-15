//! ELF as a linker reads it: constants, and bounds-checked zero-copy
//! readers over `&[u8]` for relocatable objects ([`object`], `spec/elf.md`),
//! archives ([`archive`], `spec/archive.md`) and shared objects ([`dso`],
//! `spec/elf.md` §6).
//!
//! No policy lives here: this crate says what a file contains, never what a
//! linker does with it. Every byte of an input is untrusted: a malformed
//! file is an error value that names what is wrong, never a panic.

pub mod archive;
pub mod consts;
pub mod dso;
pub mod dynamic;
pub mod error;
mod hash;
pub mod header;
pub mod note;
pub mod object;
pub mod raw;
pub mod reloc;
pub mod section;
pub mod segment;
pub mod strtab;
pub mod symbol;
pub mod version;
pub mod view;

pub use dso::{Entry, SharedObject};
pub use dynamic::{DynamicTable, Needed};
pub use error::{Error, NotePart, VersionFault, VersionRecord};
pub use header::{FileHeader, Machine};
pub use note::{Note, Notes, Properties, Property};
pub use object::Object;
pub use raw::{
    Chdr, Class, Dyn, Ehdr, Nhdr, Phdr, Record, Rel, Rela, Shdr, Sym, Table, TableIter, Verdaux,
    Verdef, Vernaux, Verneed, Versym, Word,
};
pub use section::{
    Compression, Group, GroupMembers, Merge, RelSection, RelaSection, RelocSection, Section,
    SectionIndex, SectionIter, SectionTable,
};
pub use segment::{ProgramHeaders, Segment, SegmentIter};
pub use strtab::{DYNAMIC_STRINGS, StrTab};
pub use symbol::{SymbolIndex, SymbolSection, SymbolTable};
pub use version::{
    SymbolVersion, Version, VersionDefinition, VersionDefinitions, VersionNames, VersionNeed,
    VersionNeedEntries, VersionNeedEntry, VersionNeeds, Versions,
};
pub use view::{Part, View};
