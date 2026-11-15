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
        Kind::LibraryNotFound { name } => {
            out.push_str("unable to find library -l");
            escaped(name, out);
        }
        Kind::Malformed { what } => escaped(what, out),
        Kind::UnknownFileType => out.push_str("unknown file type"),
        Kind::DuplicateSymbol { name } => {
            out.push_str("duplicate symbol: ");
            return symbol_into(name, style, out).then_some(name);
        }
        Kind::UndefinedSymbol { name } => {
            out.push_str("undefined symbol: ");
            return symbol_into(name, style, out).then_some(name);
        }
        Kind::Refused { class, name } => {
            out.push_str("unsupported ");
            out.push_str(class.noun());
            out.push_str(": ");
            escaped(name, out);
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
