//! ELF64 as a linker reads it: constants, and bounds-checked zero-copy
//! readers over `&[u8]` for relocatable objects ([`object`], `spec/elf.md`),
//! archives ([`archive`], `spec/archive.md`) and, later, shared objects.
//!
//! No policy lives here: this crate says what a file contains, never what a
//! linker does with it. Every byte of an input is untrusted: a malformed
//! file is an error value that names what is wrong, never a panic.

pub mod archive;
pub mod object;
