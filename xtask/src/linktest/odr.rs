//! The `diag: <id> <file>...` directive (`spec/harness.md` "Directives",
//! `spec/odr.md` §12.1): the one-definition-rule diagnostics a mortar link
//! step reports, read back from its standard error, and the rule that a
//! mortar link reporting one that no directive names fails.
//!
//! A diagnostic is recognized by its head line (`<prog>: warning: <message>`
//! or `error:`), whose message is matched against the wording of each
//! one-definition-rule kind as `mortar_diag` renders it; its places are the
//! files of the `>>> <relation> <place>` lines under it, in order.

use std::fmt::Write as _;

use mortar_diag::{Diagnostic, Kind, Style};

/// What a `diag` directive expects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expected {
    /// The directive's line.
    pub line: usize,
    pub id: String,
    pub files: Vec<String>,
}

/// A one-definition-rule diagnostic read from a link's standard error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reported {
    pub id: &'static str,
    pub files: Vec<String>,
}

/// The stand-in for a kind's text fields while its wording is rendered.
const HOLE: &str = "@@hole@@";

/// The one-definition-rule kinds the code has (`spec/diag.md` §3.1, the
/// area of that name), each with a value whose text fields are [`HOLE`].
/// The package that adds a kind of the area adds it here (WP-071 the
/// `meta-` kinds, WP-073 the record checks', WP-076 `odr-archive-shadow`).
fn kinds() -> Vec<Kind> {
    let h = || HOLE.to_owned();
    // `odr-type-mismatch` before `odr-violation`: the first wording that
    // fits names the kind, and "type x violates ..." fits both.
    vec![
        Kind::OdrAbiMismatch { option: h(), first: h(), second: h() },
        Kind::OdrArchiveShadow { name: h() },
        Kind::OdrContentsMismatch { name: h() },
        Kind::OdrDeclarationMismatch { name: h() },
        Kind::OdrInternalReference { name: h() },
        Kind::OdrIsaMismatch { name: h() },
        Kind::OdrSizeMismatch { name: h() },
        Kind::OdrStrongAndComdat { name: h() },
        Kind::OdrSymbolTypeMismatch { name: h() },
        Kind::OdrTypeMismatch { name: h() },
        Kind::OdrViolation { name: h() },
        Kind::OdrVtableRttiMismatch { name: h() },
    ]
}

/// The ids a `diag` directive may name.
pub fn ids() -> Vec<&'static str> {
    kinds().iter().map(Kind::id).collect()
}

/// Each kind's id and the fixed pieces of its message, around its fields.
fn wordings() -> Vec<(&'static str, Vec<String>)> {
    kinds()
        .into_iter()
        .map(|kind| {
            let id = kind.id();
            let text = Diagnostic::warning(kind).render(&Style::plain("ld"));
            let message = text.lines().next().unwrap_or_default();
            let message = message.strip_prefix("ld: warning: ").unwrap_or(message);
            (id, message.split(HOLE).map(str::to_owned).collect())
        })
        .collect()
}

/// Whether `message` has the fixed pieces in order, each hole non-empty.
fn fits(message: &str, pieces: &[String]) -> bool {
    let Some((first, rest)) = pieces.split_first() else { return false };
    let Some(mut tail) = message.strip_prefix(first.as_str()) else { return false };
    let Some((last, middle)) = rest.split_last() else { return tail.is_empty() };
    for piece in middle {
        match tail.get(1..).and_then(|t| t.find(piece.as_str())) {
            Some(at) => tail = &tail[1 + at + piece.len()..],
            None => return false,
        }
    }
    tail.len() > last.len() && tail.ends_with(last.as_str())
}

/// The file of a place: `a.o:(sym)` is `a.o`; `libx.a(y.o):(sym)` is
/// `libx.a(y.o)`; a record's `a.o (kept), at h.h:3` is `a.o`; a place
/// without a symbol is the file itself.
fn file_of(place: &str) -> String {
    let place = place.split_once(", at ").map_or(place, |(file, _)| file);
    let place = place.strip_suffix(" (kept)").unwrap_or(place);
    place.find(":(").map_or(place, |at| &place[..at]).to_owned()
}

/// The words of the relations a one-definition-rule diagnostic uses
/// (`in`: the units of `odr-abi-mismatch`; `declared`: kind 5's rows
/// without a definition, WP-151).
const RELATIONS: [&str; 5] =
    ["defined differently in ", "defined in ", "declared differently in ", "declared in ", "in "];

/// The one-definition-rule diagnostics in a link's standard error.
pub fn reported(stderr: &str) -> Vec<Reported> {
    let wordings = wordings();
    let mut out: Vec<Reported> = Vec::new();
    let mut open = false;
    for line in stderr.lines() {
        if let Some(context) = line.strip_prefix(">>> ") {
            if open
                && let Some(last) = out.last_mut()
                && let Some(place) = RELATIONS.iter().find_map(|r| context.strip_prefix(r))
            {
                last.files.push(file_of(place));
            }
            continue;
        }
        open = false;
        let message = ["warning: ", "error: "]
            .iter()
            .find_map(|severity| line.find(severity).map(|at| &line[at + severity.len()..]));
        let Some(message) = message else { continue };
        if let Some((id, _)) = wordings.iter().find(|(_, pieces)| fits(message, pieces)) {
            out.push(Reported { id, files: Vec::new() });
            open = true;
        }
    }
    out
}

/// Judges one mortar link's standard error against the test's `diag`
/// directives: each is reported, and nothing else of the area is.
pub fn judge(expected: &[Expected], stderr: &str) -> Result<(), String> {
    let reported = reported(stderr);
    let mut problems = String::new();
    for e in expected {
        if !reported.iter().any(|r| r.id == e.id && r.files == e.files) {
            let _ = write!(
                problems,
                "{}the diagnostic {} with places {} of 'diag' (line {}) is not reported",
                if problems.is_empty() { "" } else { "; " },
                e.id,
                e.files.join(" "),
                e.line
            );
        }
    }
    for r in &reported {
        if !expected.iter().any(|e| e.id == r.id && e.files == r.files) {
            let _ = write!(
                problems,
                "{}mortar reports {} with places {}, which no 'diag' directive names",
                if problems.is_empty() { "" } else { "; " },
                r.id,
                r.files.join(" ")
            );
        }
    }
    if problems.is_empty() { Ok(()) } else { Err(problems) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W1: &str = "ld: warning: _ZTV1VIiE has different sizes in different files\n\
        >>> defined in k5a.o:(_ZTV1VIiE)\n\
        >>> defined differently in libk.a(k5b.o):(_ZTV1VIiE)\n\
        >>> st_size 48 and 40\n";

    // spec: harness.md "Directives" (`diag`: the id from the message, the
    // places in order)
    #[test]
    fn reads_the_diagnostics_back() {
        let got = reported(W1);
        assert_eq!(
            got,
            [Reported {
                id: "odr-size-mismatch",
                files: vec!["k5a.o".into(), "libk.a(k5b.o)".into()]
            }]
        );
        let other = "ld: error: undefined symbol: f\n>>> referenced by a.o:(.text+0x0)\n";
        assert!(reported(other).is_empty());
        assert_eq!(ids().len(), 12);
        let record = "ld: error: type 1S violates the one-definition rule\n\
            >>> defined in a.o (kept), at h.h:1\n\
            >>> defined differently in b.o, at h.h:1\n";
        assert_eq!(
            reported(record),
            [Reported { id: "odr-type-mismatch", files: vec!["a.o".into(), "b.o".into()] }]
        );
        let declared = "ld: error: _Z1fv is declared with a type that does not match its \
            definition\n>>> declared in a.o:(_Z1fv)\n>>> declared differently in b.o:(_Z1fv)\n";
        assert_eq!(
            reported(declared),
            [Reported { id: "odr-declaration-mismatch", files: vec!["a.o".into(), "b.o".into()] }]
        );
    }

    // spec: harness.md "Directives" (a reported diagnostic no directive
    // names fails; a directive's must be reported)
    #[test]
    fn judges_both_ways() {
        let want = |files: &[&str]| Expected {
            line: 3,
            id: "odr-size-mismatch".into(),
            files: files.iter().map(|f| (*f).to_owned()).collect(),
        };
        assert!(judge(&[want(&["k5a.o", "libk.a(k5b.o)"])], W1).is_ok());
        let unnamed = judge(&[], W1).unwrap_err();
        assert!(unnamed.contains("no 'diag' directive names"), "{unnamed}");
        let order = judge(&[want(&["libk.a(k5b.o)", "k5a.o"])], W1).unwrap_err();
        assert!(order.contains("is not reported"), "{order}");
        assert!(judge(&[want(&["a.o"])], "").unwrap_err().contains("line 3"));
        assert!(judge(&[], "ld: warning: something else\n").is_ok());
    }
}
