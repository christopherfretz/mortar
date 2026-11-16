//! Linker scripts: bytes to a checked AST. The contract is
//! `spec/script.md`.
//!
//! Stage 1 (WP-009) reads what a linker meets in every dynamic link: the
//! *implicit* scripts that stand in for libraries (`libc.so` on glibc is
//! the text `GROUP ( /lib/.../libc.so.6 ... AS_NEEDED ( ... ) )`). It
//! parses `INPUT`, `GROUP`, `AS_NEEDED`, `OUTPUT_FORMAT`, `OUTPUT_ARCH`,
//! `TARGET`, `SEARCH_DIR`, `OUTPUT`, `ENTRY`, `EXTERN`, `STARTUP`,
//! `INCLUDE` and `;`, and refuses by name every other command GNU ld
//! accepts (`SECTIONS`, `MEMORY`, assignments, ...): a refusal is an
//! answer, a skipped command would not be (`docs/ARCHITECTURE.md`,
//! principle 1).
//!
//! ```
//! use mortar_script::{CommandKind, FileForm, InputItem, parse};
//!
//! let script = parse(b"GROUP ( libgcc_s.so.1 -lgcc )", "libgcc_s.so").unwrap();
//! let CommandKind::Group(items) = &script.commands[0].kind else { panic!() };
//! let InputItem::File(lib) = &items[1] else { panic!() };
//! assert_eq!((lib.form, &lib.name[..]), (FileForm::Library, &b"gcc"[..]));
//!
//! let refused = parse(b"SECTIONS { }", "x.ld").unwrap_err();
//! assert_eq!(refused.refusal().map(|(_, name)| name), Some("SECTIONS"));
//! ```
//!
//! - [`parse`]: bytes and a path for diagnostics to a [`Script`] or one
//!   located [`mortar_diag::Diagnostic`]. Never panics, whatever the bytes.
//! - [`looks_like_script`]: whether an input file that is no object,
//!   archive or shared object is to be read as a script (§3).
//! - [`lex`]: the modal lexer (§1), for the later stages and for tests.
//! - [`VersionScript`], [`DynamicList`]: the AST of version scripts and
//!   dynamic lists (§9, M4); [`version`] will parse them (WP-125).

#![forbid(unsafe_code)]

mod ast;
pub mod lex;
mod parse;
pub mod version;

pub use ast::{
    Command, CommandKind, DynamicList, FileForm, InputFile, InputItem, Language, Scope, Script,
    Span, VersionEntry, VersionNode, VersionParent, VersionScript, Word,
};
pub use parse::{ASSIGNMENT, MAX_AS_NEEDED_DEPTH};

use mortar_diag::Diagnostic;

/// Parses the bytes of a linker script (`spec/script.md` §2, §4).
///
/// `path` names the script in diagnostics (`Place::Script`), as the user,
/// a search directory or another script gave it. The error is a
/// `malformed-input` diagnostic for a script that is not one, or a
/// `refused` one (class `script-command`) for a command mortar does not
/// implement, placed at the offending token.
pub fn parse(source: &[u8], path: &str) -> Result<Script, Box<Diagnostic>> {
    parse::parse(source, path)
}

/// The line and column of byte `offset` of `source`, both from 1; the
/// column counts bytes (`spec/diag.md` §2.3). An offset past the end is
/// placed at the end. Lines are ended by LF only.
#[must_use]
pub fn position(source: &[u8], offset: usize) -> (u32, u32) {
    let before = &source[..offset.min(source.len())];
    #[allow(clippy::naive_bytecount)] // no dependency for a cold path
    let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
    let line_start = before.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let column = before.len() - line_start + 1;
    let clamp = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    (clamp(line), clamp(column))
}

/// Whether an input file is to be read as a linker script
/// (`spec/script.md` §3).
///
/// False for the formats that have their own readers (an ELF file, a GNU
/// or thin archive, by their magic) and for bytes that cannot be script
/// text: a NUL, or a control character other than tab, line feed and
/// carriage return, anywhere in the file. True otherwise, the empty file
/// included. A true answer does not mean the file parses: [`parse`] then
/// says what is wrong, where.
#[must_use]
pub fn looks_like_script(bytes: &[u8]) -> bool {
    const OTHER_FORMATS: [&[u8]; 3] = [b"\x7fELF", b"!<arch>\n", b"!<thin>\n"];
    if OTHER_FORMATS.iter().any(|magic| bytes.starts_with(magic)) {
        return false;
    }
    !bytes.iter().any(|&b| (b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r')) || b == 0x7f)
}
