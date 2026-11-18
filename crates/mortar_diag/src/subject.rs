//! The subjects of a warning code (`spec/incr.md` IN-75a; `spec/diag.md`'s
//! subjects column, WP-211 §R.7): what a stored warning depends on, so
//! that an update drops and recomputes it when a subject changes. WP-213
//! (U-1): the type [`SubjectKind`] and the entry point [`subjects`];
//! filled by U-9 (WP-221).
//!
//! WP-221 step 1 (PROPOSED, not ruled): the stored form of a warning,
//! [`stored`] and [`from_stored`]. A `DIAGS` row keeps the diagnostic's
//! model (its kind's fields, its place, its context lines), never a
//! rendering: the program name, colour and the demangling hook are the
//! invocation's (`Style`, diag.md §5.1) and are applied when the update
//! renders its report, so a replay equals the full link's standard error
//! under every `Style`.

use crate::model::{Context, Diagnostic, FileRef, Kind, Place, Relation, Severity};

/// One kind of subject (diag.md's closed set: file, key, output section,
/// link).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SubjectKind {
    File,
    /// A key: a symbol (`Subject::Key`) or an ODR key (`Subject::OdrKey`,
    /// odr.md O-4; WP-212 F-12): one kind, since a code's subjects column
    /// names "key" for both.
    Key,
    OutputSection,
    Link,
}

use SubjectKind::{File, Key, Link};

const F: &[SubjectKind] = &[File];
const FK: &[SubjectKind] = &[File, Key];
const FL: &[SubjectKind] = &[File, Link];
const FKL: &[SubjectKind] = &[File, Key, Link];
const K: &[SubjectKind] = &[Key];
const KL: &[SubjectKind] = &[Key, Link];
const L: &[SubjectKind] = &[Link];

/// The subjects of the warning `code` (a `Kind::id`), as diag.md §3.1's
/// subjects column gives them, in the column's order; empty for a code
/// that is never a warning (`—`) or is not a code.
#[must_use]
pub fn subjects(code: &str) -> &'static [SubjectKind] {
    match code {
        "ignored-member" | "cet-report" | "malformed-input" | "unknown-property"
        | "meta-malformed" | "odr-abi-mismatch" | "executable-stack" => F,
        "duplicate-soname" | "needed-not-found" => FL,
        "skipped-incompatible" | "incremental-not-reproducible" | "incremental-unsupported" => L,
        "archive-backref" | "archive-choice" | "versioned-definition-hidden" => FKL,
        "common-alignment"
        | "common-larger-than-definition"
        | "common-overridden"
        | "copy-of-locally-bound-definition"
        | "gnu-warning"
        | "multiple-common"
        | "undefined-symbol"
        | "odr-archive-shadow"
        | "odr-contents-mismatch"
        | "odr-declaration-mismatch"
        | "odr-internal-reference"
        | "odr-isa-mismatch"
        | "odr-size-mismatch"
        | "odr-strong-and-comdat"
        | "odr-symbol-type-mismatch"
        | "odr-type-mismatch"
        | "odr-violation"
        | "odr-vtable-rtti-mismatch" => FK,
        "entry-not-found" => K,
        "version-reassigned" | "version-script-undefined-name" => KL,
        _ => &[],
    }
}

// ---- the stored form (WP-221 step 1, PROPOSED) ------------------------------
//
// A sequence of tokens, each `<length in bytes, decimal>:<bytes>`: valid
// UTF-8 whenever the fields are (they are Rust strings), self-delimiting,
// and free of anything the renderer adds (program name, severity word,
// colour, demangled names, escapes of §5.5). The code is stored beside it
// (`DiagRow.code`); the severity is not stored: a state holds warnings only
// (IN-75a, IN-48).
//
//   form    = fields place *context
//   fields  = the kind's fields in declaration order (numbers in decimal)
//   place   = "-" / form-of-place
//   context = "n" text / "@" relation-index form-of-place
//   form-of-place = "F" file / "S" file section offset / "Y" file name
//           / "A" text / "C" path line column
//           / "R" file opt(name) kept opt(path line)
//   file    = path opt(member);  opt(x) = "-" / "+" x;  kept = "0" / "1"

struct Out(String);

impl Out {
    fn tok(&mut self, s: &str) -> &mut Self {
        self.0.push_str(&s.len().to_string());
        self.0.push(':');
        self.0.push_str(s);
        self
    }
    fn num(&mut self, n: impl ToString) -> &mut Self {
        self.tok(&n.to_string())
    }
    fn opt(&mut self, s: Option<&str>) -> &mut Self {
        match s {
            None => self.tok("-"),
            Some(s) => self.tok("+").tok(s),
        }
    }
    fn file(&mut self, f: &FileRef) -> &mut Self {
        self.tok(f.path()).opt(f.member())
    }
    fn place(&mut self, p: &Place) -> Option<()> {
        match p {
            Place::File(f) => self.tok("F").file(f),
            Place::Section { file, section, offset } => {
                self.tok("S").file(file).tok(section).num(offset)
            }
            Place::Symbol { file, name } => self.tok("Y").file(file).tok(name),
            Place::Argument(text) => self.tok("A").tok(text),
            Place::Script { path, line, column } => self.tok("C").tok(path).num(line).num(column),
            Place::Record { file, name, kept, source } => {
                self.tok("R").file(file).opt(name.as_deref()).num(u8::from(*kept));
                match source {
                    None => self.tok("-"),
                    Some((path, line)) => self.tok("+").tok(path).num(line),
                }
            }
        };
        Some(())
    }
}

/// The fields of a kind that can be a warning, in declaration order; `None`
/// for a kind that is never a warning.
fn fields(kind: &Kind) -> Option<Vec<String>> {
    let s = |x: &String| x.clone();
    Some(match kind {
        Kind::DuplicateSoname { soname } => vec![s(soname)],
        Kind::IgnoredMember { what } | Kind::Malformed { what } => vec![s(what)],
        Kind::NeededNotFound { name, needed_by } => vec![s(name), s(needed_by)],
        Kind::SkippedIncompatible { path, search } => vec![s(path), s(search)],
        Kind::CetReport { path, property } => vec![s(path), s(property)],
        Kind::UnknownProperty { path, pr_type } => vec![s(path), pr_type.to_string()],
        Kind::CopyOfLocallyBoundDefinition { name, file } => vec![s(name), s(file)],
        Kind::GnuWarning { text } => vec![s(text)],
        Kind::VersionReassigned { name, first, second } => vec![s(name), s(first), s(second)],
        Kind::VersionScriptUndefinedName { name, version }
        | Kind::VersionedDefinitionHidden { name, version } => vec![s(name), s(version)],
        Kind::MetaMalformed { reason } => vec![s(reason)],
        Kind::OdrAbiMismatch { option, first, second } => vec![s(option), s(first), s(second)],
        Kind::ExecutableStack { file } => vec![s(file)],
        Kind::IncrementalNotReproducible => vec![],
        Kind::IncrementalUnsupported { option } => vec![s(option)],
        Kind::ArchiveBackref { name }
        | Kind::ArchiveChoice { name }
        | Kind::CommonAlignment { name }
        | Kind::CommonLargerThanDefinition { name }
        | Kind::CommonOverridden { name }
        | Kind::EntryNotFound { name }
        | Kind::MultipleCommon { name }
        | Kind::UndefinedSymbol { name }
        | Kind::OdrArchiveShadow { name }
        | Kind::OdrContentsMismatch { name }
        | Kind::OdrDeclarationMismatch { name }
        | Kind::OdrInternalReference { name }
        | Kind::OdrIsaMismatch { name }
        | Kind::OdrSizeMismatch { name }
        | Kind::OdrStrongAndComdat { name }
        | Kind::OdrSymbolTypeMismatch { name }
        | Kind::OdrTypeMismatch { name }
        | Kind::OdrViolation { name }
        | Kind::OdrVtableRttiMismatch { name } => vec![s(name)],
        _ => return None,
    })
}

/// The kind of `code` from its stored fields; `None` when the fields do not
/// fit the code.
fn kind_of(code: &str, f: &mut In<'_>) -> Option<Kind> {
    let mut t = || f.tok().map(str::to_owned);
    Some(match code {
        "duplicate-soname" => Kind::DuplicateSoname { soname: t()? },
        "ignored-member" => Kind::IgnoredMember { what: t()? },
        "malformed-input" => Kind::Malformed { what: t()? },
        "needed-not-found" => Kind::NeededNotFound { name: t()?, needed_by: t()? },
        "skipped-incompatible" => Kind::SkippedIncompatible { path: t()?, search: t()? },
        "cet-report" => Kind::CetReport { path: t()?, property: t()? },
        "unknown-property" => Kind::UnknownProperty { path: t()?, pr_type: t()?.parse().ok()? },
        "copy-of-locally-bound-definition" => {
            Kind::CopyOfLocallyBoundDefinition { name: t()?, file: t()? }
        }
        "gnu-warning" => Kind::GnuWarning { text: t()? },
        "version-reassigned" => {
            Kind::VersionReassigned { name: t()?, first: t()?, second: t()? }
        }
        "version-script-undefined-name" => {
            Kind::VersionScriptUndefinedName { name: t()?, version: t()? }
        }
        "versioned-definition-hidden" => {
            Kind::VersionedDefinitionHidden { name: t()?, version: t()? }
        }
        "meta-malformed" => Kind::MetaMalformed { reason: t()? },
        "odr-abi-mismatch" => Kind::OdrAbiMismatch { option: t()?, first: t()?, second: t()? },
        "executable-stack" => Kind::ExecutableStack { file: t()? },
        "incremental-not-reproducible" => Kind::IncrementalNotReproducible,
        "incremental-unsupported" => Kind::IncrementalUnsupported { option: t()? },
        "archive-backref" => Kind::ArchiveBackref { name: t()? },
        "archive-choice" => Kind::ArchiveChoice { name: t()? },
        "common-alignment" => Kind::CommonAlignment { name: t()? },
        "common-larger-than-definition" => Kind::CommonLargerThanDefinition { name: t()? },
        "common-overridden" => Kind::CommonOverridden { name: t()? },
        "entry-not-found" => Kind::EntryNotFound { name: t()? },
        "multiple-common" => Kind::MultipleCommon { name: t()? },
        "undefined-symbol" => Kind::UndefinedSymbol { name: t()? },
        "odr-archive-shadow" => Kind::OdrArchiveShadow { name: t()? },
        "odr-contents-mismatch" => Kind::OdrContentsMismatch { name: t()? },
        "odr-declaration-mismatch" => Kind::OdrDeclarationMismatch { name: t()? },
        "odr-internal-reference" => Kind::OdrInternalReference { name: t()? },
        "odr-isa-mismatch" => Kind::OdrIsaMismatch { name: t()? },
        "odr-size-mismatch" => Kind::OdrSizeMismatch { name: t()? },
        "odr-strong-and-comdat" => Kind::OdrStrongAndComdat { name: t()? },
        "odr-symbol-type-mismatch" => Kind::OdrSymbolTypeMismatch { name: t()? },
        "odr-type-mismatch" => Kind::OdrTypeMismatch { name: t()? },
        "odr-violation" => Kind::OdrViolation { name: t()? },
        "odr-vtable-rtti-mismatch" => Kind::OdrVtableRttiMismatch { name: t()? },
        _ => return None,
    })
}

/// The stored form of the warning `diag` (WP-221 step 1, PROPOSED):
/// `DIAGS`' `text`, beside `code` = `diag.id()`. `None` for a diagnostic
/// that is not a warning or whose kind is never one: a state holds
/// warnings only (IN-75a).
#[must_use]
pub fn stored(diag: &Diagnostic) -> Option<String> {
    if diag.severity() != Severity::Warning {
        return None;
    }
    let mut out = Out(String::new());
    for f in fields(diag.kind())? {
        out.tok(&f);
    }
    match diag.place() {
        None => {
            out.tok("-");
        }
        Some(p) => out.place(p)?,
    }
    for c in diag.context() {
        match c {
            Context::Note(text) => {
                out.tok("n").tok(text);
            }
            Context::At(relation, place) => {
                let index = Relation::ALL.iter().position(|r| r == relation)?;
                out.tok("@").num(index);
                out.place(place)?;
            }
        }
    }
    Some(out.0)
}

struct In<'a>(&'a str);

impl<'a> In<'a> {
    fn tok(&mut self) -> Option<&'a str> {
        let colon = self.0.find(':')?;
        let len: usize = self.0[..colon].parse().ok()?;
        if self.0[..colon].len() > 1 && self.0.starts_with('0') {
            return None;
        }
        let rest = &self.0[colon + 1..];
        let t = rest.get(..len)?;
        self.0 = &rest[len..];
        Some(t)
    }
    fn num<T: std::str::FromStr>(&mut self) -> Option<T> {
        self.tok()?.parse().ok()
    }
    fn opt(&mut self) -> Option<Option<&'a str>> {
        match self.tok()? {
            "-" => Some(None),
            "+" => Some(Some(self.tok()?)),
            _ => None,
        }
    }
    fn file(&mut self) -> Option<FileRef> {
        let path = self.tok()?;
        Some(match self.opt()? {
            None => FileRef::new(path),
            Some(m) => FileRef::in_archive(path, m),
        })
    }
    fn place(&mut self, tag: &str) -> Option<Place> {
        Some(match tag {
            "F" => Place::File(self.file()?),
            "S" => {
                let file = self.file()?;
                let section = self.tok()?;
                Place::section(file, section, self.num()?)
            }
            "Y" => {
                let file = self.file()?;
                Place::symbol(file, self.tok()?)
            }
            "A" => Place::argument(self.tok()?),
            "C" => {
                let path = self.tok()?;
                let line = self.num()?;
                Place::script(path, line, self.num()?)
            }
            "R" => {
                let file = self.file()?;
                let name = self.opt()?.map(str::to_owned);
                let kept = match self.tok()? {
                    "0" => false,
                    "1" => true,
                    _ => return None,
                };
                let source = match self.tok()? {
                    "-" => None,
                    "+" => {
                        let path = self.tok()?.to_owned();
                        Some((path, self.num()?))
                    }
                    _ => return None,
                };
                Place::record(file, name, kept, source)
            }
            _ => return None,
        })
    }
}

/// The warning a `DIAGS` row stores (WP-221 step 1, PROPOSED): the inverse
/// of [`stored`]; `None` when `text` is not a stored form of `code` (the
/// state is corrupt).
#[must_use]
pub fn from_stored(code: &str, text: &str) -> Option<Diagnostic> {
    let mut i = In(text);
    let mut d = Diagnostic::warning(kind_of(code, &mut i)?);
    match i.tok()? {
        "-" => {}
        tag => d = d.at(i.place(tag)?),
    }
    while !i.0.is_empty() {
        match i.tok()? {
            "n" => d = d.note(i.tok()?),
            "@" => {
                let relation = *Relation::ALL.get(i.num::<usize>()?)?;
                let tag = i.tok()?;
                d = d.with(relation, i.place(tag)?);
            }
            _ => return None,
        }
    }
    Some(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Style;

    /// A warning of every kind that can be one, with every place form and
    /// both context forms, and hostile text (control characters, `:`,
    /// digits, multibyte, empty).
    fn samples() -> Vec<Diagnostic> {
        let s = |x: &str| x.to_owned();
        let kinds = [
            Kind::DuplicateSoname { soname: s("libx.so.1") },
            Kind::IgnoredMember { what: s("3:a\n") },
            Kind::NeededNotFound { name: s("liba.so"), needed_by: s("libb.so") },
            Kind::SkippedIncompatible { path: s("/l/libc.a"), search: s("-lc") },
            Kind::CetReport { path: s("a.o"), property: s("IBT") },
            Kind::Malformed { what: s("") },
            Kind::UnknownProperty { path: s("a.o"), pr_type: 0xc000_0002 },
            Kind::ArchiveBackref { name: s("f") },
            Kind::ArchiveChoice { name: s("f") },
            Kind::CommonAlignment { name: s("c") },
            Kind::CommonLargerThanDefinition { name: s("c") },
            Kind::CommonOverridden { name: s("c") },
            Kind::CopyOfLocallyBoundDefinition { name: s("v"), file: s("libv.so") },
            Kind::EntryNotFound { name: s("_start") },
            Kind::GnuWarning { text: s("the use of `mktemp' is dangerous \u{1b}[31m é") },
            Kind::MultipleCommon { name: s("c") },
            Kind::UndefinedSymbol { name: s("_ZN2ns1fEic") },
            Kind::VersionReassigned { name: s("f"), first: s("V1"), second: s("V2") },
            Kind::VersionScriptUndefinedName { name: s("f"), version: s("V1") },
            Kind::VersionedDefinitionHidden { name: s("f"), version: s("V1") },
            Kind::MetaMalformed { reason: s("12:") },
            Kind::OdrAbiMismatch { option: s("-std"), first: s("c++17"), second: s("c++20") },
            Kind::OdrArchiveShadow { name: s("n") },
            Kind::OdrContentsMismatch { name: s("n") },
            Kind::OdrDeclarationMismatch { name: s("n") },
            Kind::OdrInternalReference { name: s("n") },
            Kind::OdrIsaMismatch { name: s("n") },
            Kind::OdrSizeMismatch { name: s("n") },
            Kind::OdrStrongAndComdat { name: s("n") },
            Kind::OdrSymbolTypeMismatch { name: s("n") },
            Kind::OdrTypeMismatch { name: s("n") },
            Kind::OdrViolation { name: s("_ZN1A1fEv") },
            Kind::OdrVtableRttiMismatch { name: s("n") },
            Kind::ExecutableStack { file: s("lib.a(x.o)") },
            Kind::IncrementalNotReproducible,
            Kind::IncrementalUnsupported { option: s("-r") },
        ];
        let member = FileRef::in_archive("lib/libc.a", "mktemp.o");
        let places = [
            Place::File(FileRef::new("a.o")),
            Place::File(member.clone()),
            Place::section(member.clone(), ".text", 0),
            Place::section(FileRef::new("1:x"), ".text.x", u64::MAX),
            Place::symbol(member.clone(), "_ZN2ns1fEic"),
            Place::argument("--entry=main"),
            Place::script("v.map", 5, 9),
            Place::record(member.clone(), Some(s("f")), true, Some((s("h.h"), 3))),
            Place::record(FileRef::new("b.o"), None, false, None),
        ];
        let mut out = Vec::new();
        for (i, kind) in kinds.into_iter().enumerate() {
            let mut d = Diagnostic::warning(kind);
            if i % 2 == 0 {
                d = d.at(places[i % places.len()].clone());
            }
            for (j, p) in places.iter().enumerate() {
                let relation = Relation::ALL[(i + j) % Relation::ALL.len()];
                d = d.with(relation, p.clone());
            }
            out.push(d.note("referenced 4 more times").note(""));
        }
        out
    }

    // spec: WP-221 step 1 (PROPOSED): every kind that can be a warning has
    // a stored form, read back as the same model value, which renders as
    // the original under every style (program name, colour, demangling)
    #[test]
    fn stored_form_reads_back_under_every_style() {
        let demangle = |n: &str| Some(format!("<{n}>"));
        let styles = [
            Style::plain("mortar"),
            Style::plain("aarch64-linux-gnu-ld"),
            Style { color: true, ..Style::plain("ld") },
            Style { demangle: &demangle, ..Style::plain("ld.mortar") },
        ];
        let all = samples();
        let mut ids: Vec<&str> = all.iter().map(Diagnostic::id).collect();
        ids.sort_unstable();
        let mut want: Vec<&str> =
            Kind::IDS.iter().copied().filter(|id| !subjects(id).is_empty()).collect();
        want.sort_unstable();
        assert_eq!(ids, want, "a sample for every code with subjects");
        for d in &all {
            let text = stored(d).expect("a stored form");
            assert!(!text.contains("mortar") && !text.contains('\u{1b}') || d.id() == "gnu-warning");
            let back = from_stored(d.id(), &text).expect("read back");
            assert_eq!(&back, d);
            for style in &styles {
                assert_eq!(back.render(style), d.render(style), "{}", d.id());
            }
            // Corrupt forms are refused, never read as something else.
            assert!(from_stored(d.id(), &text[..text.len() - 1]).is_none());
            assert!(from_stored("undefined-symbolx", &text).is_none());
        }
    }

    // spec: WP-221 step 1: an error, or a kind that is never a warning, has
    // no stored form (a state holds warnings only, IN-75a)
    #[test]
    fn errors_have_no_stored_form() {
        let undefined = || Kind::UndefinedSymbol { name: "f".to_owned() };
        assert!(stored(&Diagnostic::error(undefined())).is_none());
        assert!(stored(&Diagnostic::warning(undefined())).is_some());
        assert!(stored(&Diagnostic::warning(Kind::NoInputFiles)).is_none());
        assert!(from_stored("no-input-files", "1:-").is_none());
        assert!(from_stored("undefined-symbol", "01:f1:-").is_none());
    }
}
