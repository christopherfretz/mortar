//! The text renderer (`spec/diag.md` §5): the only place a character of a
//! diagnostic is chosen. A second renderer (a machine format) would sit
//! beside this file and read the same model through its accessors.

use std::fmt::{self, Write as _};

use crate::model::{Context, Diagnostic, FileRef, Kind, Place, Severity};

/// The demangling hook (`spec/diag.md` §5.4): given a symbol-table name,
/// the name to show instead, or `None` to show it as it is.
pub type Demangle<'a> = &'a (dyn Fn(&str) -> Option<String> + Sync);

/// The hook that demangles nothing.
#[must_use]
pub fn no_demangling(_name: &str) -> Option<String> {
    None
}

/// How to render (`spec/diag.md` §5.1). The renderer probes nothing: the
/// caller decides the program name, whether to color, and how to demangle.
#[derive(Clone, Copy)]
pub struct Style<'a> {
    /// The basename mortar was invoked as: `mortar`, `ld`, `ld.mortar`.
    pub prog: &'a str,
    /// Whether to wrap the severity word in ANSI color sequences.
    pub color: bool,
    /// The demangling hook.
    pub demangle: Demangle<'a>,
}

impl<'a> Style<'a> {
    /// No color, no demangling.
    #[must_use]
    pub fn plain(prog: &'a str) -> Self {
        Self { prog, color: false, demangle: &no_demangling }
    }
}

impl fmt::Debug for Style<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Style")
            .field("prog", &self.prog)
            .field("color", &self.color)
            .finish_non_exhaustive()
    }
}

/// The line that ends a report cut at the error limit (`spec/diag.md`
/// §6.4), without the program name and severity.
pub(crate) const LIMIT_MESSAGE: &str =
    "too many errors emitted, stopping now (use --error-limit=0 to see all errors)";

/// GNU ld's phrase for bytes of a format it cannot read, which build tools
/// match (`spec/diag.md` §8.3).
pub const NOT_RECOGNIZED: &str = "file not recognized: file format not recognized";

const RESET: &str = "\x1b[0m";

const fn color_of(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "\x1b[0;31m",
        Severity::Warning => "\x1b[0;35m",
    }
}

impl Diagnostic {
    /// The diagnostic as text: the head line and its context lines, each
    /// ended by a newline (`spec/diag.md` §5).
    #[must_use]
    pub fn render(&self, style: &Style<'_>) -> String {
        let mut out = String::new();
        self.render_into(style, &mut out);
        out
    }

    pub(crate) fn render_into(&self, style: &Style<'_>, out: &mut String) {
        head(style, self.severity(), out);
        if let Some(place) = self.place() {
            place_into(place, style, out);
            out.push_str(": ");
        }
        let mangled = message(self.kind(), style, out);
        out.push('\n');
        if let Some(mangled) = mangled {
            out.push_str(">>> mangled name: ");
            escaped(mangled, out);
            out.push('\n');
        }
        for line in self.context() {
            out.push_str(">>> ");
            match line {
                Context::At(relation, place) => {
                    out.push_str(relation.phrase());
                    out.push(' ');
                    place_into(place, style, out);
                }
                Context::Note(text) => escaped(text, out),
            }
            out.push('\n');
        }
    }
}

/// `<prog>: <severity>: `, the severity word and its colon colored on
/// request.
fn head(style: &Style<'_>, severity: Severity, out: &mut String) {
    escaped(style.prog, out);
    out.push_str(": ");
    if style.color {
        out.push_str(color_of(severity));
    }
    out.push_str(severity.word());
    out.push(':');
    if style.color {
        out.push_str(RESET);
    }
    out.push(' ');
}

/// The line that says the error limit cut the report.
pub(crate) fn limit_line(style: &Style<'_>, out: &mut String) {
    head(style, Severity::Error, out);
    out.push_str(LIMIT_MESSAGE);
    out.push('\n');
}

/// Writes the message of a kind. Returns the symbol-table name when the
/// message shows a demangled name in its place.
#[allow(clippy::too_many_lines)] // one arm per kind, in the spec's order
fn message<'d>(kind: &'d Kind, style: &Style<'_>, out: &mut String) -> Option<&'d str> {
    match kind {
        Kind::ConflictingOptions { first, second } => {
            escaped(first, out);
            out.push_str(" and ");
            escaped(second, out);
            out.push_str(" cannot be used together");
        }
        Kind::InvalidOptionValue { option, value } => {
            escaped(option, out);
            out.push_str(": invalid value '");
            escaped(value, out);
            out.push('\'');
        }
        Kind::MissingArgument { option } => {
            escaped(option, out);
            out.push_str(": missing argument");
        }
        Kind::NoInputFiles => out.push_str("no input files"),
        Kind::ResponseFileLimit { limit } => {
            out.push_str("too many response files: more than ");
            out.push_str(&limit.to_string());
            out.push_str(" expanded");
        }
        Kind::UnbalancedOption { option, counterpart } => {
            escaped(option, out);
            out.push_str(" without a matching ");
            escaped(counterpart, out);
        }
        Kind::UnknownOption { option } => {
            out.push_str("unknown argument '");
            escaped(option, out);
            out.push('\'');
        }
        Kind::AmbiguousSysrootPath { spelling } => {
            out.push_str("ambiguous sysroot path '");
            escaped(spelling, out);
            out.push('\'');
        }
        Kind::CannotOpenFile { path, reason } => {
            out.push_str("cannot open ");
            escaped(path, out);
            out.push_str(": ");
            escaped(reason, out);
        }
        Kind::CannotOpenOutput { path, reason } => {
            out.push_str("cannot open output file ");
            escaped(path, out);
            out.push_str(": ");
            escaped(reason, out);
        }
        Kind::DuplicateSoname { soname } => {
            out.push_str("DT_SONAME '");
            escaped(soname, out);
            out.push_str("' is already loaded; this file is not used");
        }
        Kind::IgnoredMember { what } => {
            out.push_str("archive member is not a relocatable object: ");
            escaped(what, out);
        }
        Kind::IncompatibleFile { expected, found } => {
            out.push_str("incompatible file: ");
            escaped(expected, out);
            out.push_str(" is expected but got ");
            escaped(found, out);
        }
        Kind::InputChanged => out.push_str("file changed while it was read"),
        Kind::LibraryNotFound { name } => {
            out.push_str("cannot find -l");
            escaped(name, out);
            out.push_str(": No such file or directory");
        }
        Kind::NeededNotFound { name, needed_by } => {
            escaped(name, out);
            out.push_str(", needed by ");
            escaped(needed_by, out);
            out.push_str(", not found (try using -rpath or -rpath-link)");
        }
        Kind::NoTarget => {
            out.push_str("target emulation unknown: -m or at least one .o file required");
        }
        Kind::ScriptCycle => out.push_str("linker script names itself"),
        Kind::SkippedIncompatible { path, search } => {
            out.push_str("skipping incompatible ");
            escaped(path, out);
            out.push_str(" when searching for ");
            escaped(search, out);
        }
        Kind::StaticLinkOfSharedObject { path } => {
            out.push_str("attempted static link of dynamic object ");
            escaped(path, out);
        }
        Kind::Malformed { what } => escaped(what, out),
        Kind::UnknownFileType => out.push_str(NOT_RECOGNIZED),
        Kind::ArchiveBackref { name } => {
            out.push_str("backward reference to an archive member: ");
            return symbol_into(name, style, out).then_some(name);
        }
        Kind::ArchiveChoice { name } => {
            out.push_str("GNU ld would choose another definition of ");
            return symbol_into(name, style, out).then_some(name);
        }
        Kind::ComdatMismatch { name, signature } => {
            out.push_str("symbol ");
            let shown = symbol_into(name, style, out);
            out.push_str(" is defined only in a discarded copy of COMDAT group ");
            escaped(signature, out);
            return shown.then_some(name);
        }
        Kind::CommonAlignment { name } => {
            out.push_str("definition of ");
            let shown = symbol_into(name, style, out);
            out.push_str(" is less aligned than its common symbols");
            return shown.then_some(name);
        }
        Kind::CommonLargerThanDefinition { name } => {
            out.push_str("common symbol ");
            let shown = symbol_into(name, style, out);
            out.push_str(" is larger than its definition");
            return shown.then_some(name);
        }
        Kind::CommonOverridden { name } => {
            out.push_str("common symbol ");
            let shown = symbol_into(name, style, out);
            out.push_str(" is overridden by a definition");
            return shown.then_some(name);
        }
        Kind::DiscardedSectionReference { name } => {
            out.push_str("relocation refers to a symbol in a discarded section: ");
            return symbol_into(name, style, out).then_some(name);
        }
        Kind::DuplicateSymbol { name } => {
            // GNU ld's phrase, verbatim, after lld's head (§8.3).
            out.push_str("duplicate symbol: ");
            let shown = symbol_into(name, style, out);
            out.push_str(" (multiple definition of `");
            symbol_into(name, style, out);
            out.push_str("')");
            return shown.then_some(name);
        }
        Kind::EntryNotFound { name } => {
            out.push_str("cannot find entry symbol ");
            return symbol_into(name, style, out).then_some(name);
        }
        Kind::GnuWarning { text } => escaped(text, out),
        Kind::LinkerSymbolRedefined { name } => {
            out.push_str("cannot redefine linker-defined symbol: ");
            return symbol_into(name, style, out).then_some(name);
        }
        Kind::MultipleCommon { name } => {
            out.push_str("multiple common symbols: ");
            return symbol_into(name, style, out).then_some(name);
        }
        Kind::NonExportedSymbol { name } => {
            out.push_str("non-exported symbol referenced by a shared object: ");
            return symbol_into(name, style, out).then_some(name);
        }
        Kind::RequiredSymbol { name } => {
            out.push_str("required symbol not defined: ");
            return symbol_into(name, style, out).then_some(name);
        }
        Kind::TlsMismatch { name } => {
            out.push_str("TLS attribute mismatch: ");
            return symbol_into(name, style, out).then_some(name);
        }
        Kind::UndefinedSymbol { name } => {
            // GNU ld's phrase, verbatim, after lld's head (§8.3).
            out.push_str("undefined symbol: ");
            let shown = symbol_into(name, style, out);
            out.push_str(" (undefined reference to `");
            symbol_into(name, style, out);
            out.push_str("')");
            return shown.then_some(name);
        }
        Kind::UndefinedVersion { name, version } => {
            out.push_str("version ");
            escaped(version, out);
            out.push_str(" of ");
            let shown = symbol_into(name, style, out);
            out.push_str(" is not defined");
            return shown.then_some(name);
        }
        Kind::VersionReassigned { name, first, second } => {
            let shown = symbol_into(name, style, out);
            out.push_str(" is assigned to version ");
            escaped(first, out);
            out.push_str(" and ");
            escaped(second, out);
            out.push_str("; ");
            escaped(first, out);
            out.push_str(" is kept");
            return shown.then_some(name);
        }
        Kind::RelocationIncompatible { r_type, name, reason } => {
            out.push_str("relocation ");
            escaped(r_type, out);
            out.push_str(" cannot be used against symbol '");
            let shown = symbol_into(name, style, out);
            out.push_str("'; ");
            out.push_str(reason.text());
            return shown.then_some(name);
        }
        Kind::RelocationMisaligned { r_type, value, align } => {
            out.push_str("improper alignment for relocation ");
            escaped(r_type, out);
            let _ = write!(out, ": {value} is not aligned to {align} bytes");
        }
        Kind::RelocationOutOfRange { r_type, value, min, max, name } => {
            out.push_str("relocation ");
            escaped(r_type, out);
            let _ = write!(out, " out of range: {value} is not in [{min}, {max}]; references '");
            let shown = symbol_into(name, style, out);
            out.push('\'');
            return shown.then_some(name);
        }
        Kind::Refused { class, name, not_recognized } => {
            out.push_str("unsupported ");
            out.push_str(class.noun());
            out.push_str(": ");
            escaped(name, out);
            if *not_recognized {
                out.push_str("; ");
                out.push_str(NOT_RECOGNIZED);
            }
        }
        Kind::InternalError { what } => {
            out.push_str("internal error in mortar: ");
            escaped(what, out);
        }
    }
    None
}

/// Writes a symbol name as the hook wants it shown. True when what was
/// written is not the name itself.
fn symbol_into(name: &str, style: &Style<'_>, out: &mut String) -> bool {
    match (style.demangle)(name) {
        Some(shown) if shown != name => {
            escaped(&shown, out);
            true
        }
        _ => {
            escaped(name, out);
            false
        }
    }
}

fn file_into(file: &FileRef, out: &mut String) {
    escaped(file.path(), out);
    if let Some(member) = file.member() {
        out.push('(');
        escaped(member, out);
        out.push(')');
    }
}

fn place_into(place: &Place, style: &Style<'_>, out: &mut String) {
    match place {
        Place::File(file) => file_into(file, out),
        Place::Section { file, section, offset } => {
            file_into(file, out);
            out.push_str(":(");
            escaped(section, out);
            // Writing to a `String` cannot fail.
            let _ = write!(out, "+{offset:#x})");
        }
        Place::Symbol { file, name } => {
            file_into(file, out);
            out.push_str(":(");
            symbol_into(name, style, out);
            out.push(')');
        }
        Place::Argument(text) => {
            out.push_str("argument '");
            escaped(text, out);
            out.push('\'');
        }
        Place::Script { path, line, column } => {
            escaped(path, out);
            let _ = write!(out, ":{line}:{column}");
        }
    }
}

/// Writes text that came from outside (a path, a name from a symbol table,
/// an argument) with its control characters made visible
/// (`spec/diag.md` §5.5): no input can start a line or move the cursor.
fn escaped(text: &str, out: &mut String) {
    if !text.chars().any(char::is_control) {
        out.push_str(text);
        return;
    }
    for c in text.chars() {
        if c.is_control() {
            let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
        } else {
            out.push(c);
        }
    }
}

/// The text of an operating-system error as the diagnostics show it
/// (`spec/diag.md` §3.2): the system's message, without the
/// ` (os error N)` that Rust's `Display` appends.
#[must_use]
pub fn os_reason(error: &std::io::Error) -> String {
    let mut text = error.to_string();
    if let Some(code) = error.raw_os_error() {
        let suffix = format!(" (os error {code})");
        if text.ends_with(&suffix) {
            text.truncate(text.len() - suffix.len());
        }
    }
    text
}
