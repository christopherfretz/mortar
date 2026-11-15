//! ELF as a linker reads it: constants, and bounds-checked zero-copy
//! readers over `&[u8]` for relocatable objects ([`object`], `spec/elf.md`),
//! archives ([`archive`], `spec/archive.md`) and, later, shared objects.
//!
//! No policy lives here: this crate says what a file contains, never what a
//! linker does with it. Every byte of an input is untrusted: a malformed
//! file is an error value that names what is wrong, never a panic.

pub mod archive;
pub mod consts;
pub mod dso;
pub mod error;
pub mod header;
pub mod note;
pub mod object;
pub mod raw;
pub mod reloc;
pub mod section;
pub mod strtab;
pub mod symbol;

pub use error::{Error, NotePart};
pub use header::{FileHeader, Machine};
pub use note::{Note, Notes, Properties, Property};
pub use object::Object;
pub use raw::{Chdr, Class, Ehdr, Nhdr, Record, Rel, Rela, Shdr, Sym, Table, TableIter, Word};
pub use section::{
    Compression, Group, GroupMembers, Merge, RelSection, RelaSection, RelocSection, Section,
    SectionIndex, SectionIter, SectionTable,
};
pub use strtab::StrTab;
pub use symbol::{SymbolIndex, SymbolSection, SymbolTable};
