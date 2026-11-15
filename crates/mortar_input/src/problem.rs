//! The diagnostics of the phase (`spec/input.md` §13), before they become
//! `mortar_diag` diagnostics.
//!
//! A problem is a diagnostic with its key: its `mortar_diag::Kind` (the
//! text is the kind's, `spec/diag.md` §3.1), place and context lines.
//! Problems are ordered as a `Collector` orders diagnostics: by key, then
//! by text; [`Problem::diagnostic`] and [`Problem::key`] hand them over.

use std::fmt::Write as _;

use mortar_diag::{Context, Diagnostic, Key, Kind, Place, RefusalClass, Relation, Severity};

/// Phase 20 (`spec/model.md` §8).
pub const PHASE: u16 = 20;

/// The `seq` of a key: file ordinal `k` within the unit, and byte offset
/// (§12.2).
#[must_use]
pub const fn seq(k: u32, offset: u64) -> u64 {
    ((k as u64) << 40) | (offset & ((1 << 40) - 1))
}

/// What a problem says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    /// A diagnostic of this phase (§13).
    Own { severity: Severity, kind: Kind },
    /// A refusal by name (§14): `unsupported {noun}: {name}`, and GNU ld's
    /// phrase after it where §13 says so.
    Refused { class: RefusalClass, name: String, gnu_phrase: bool },
    /// The script parser's diagnostic, as it made it (script.md §2.9).
    Parse(Box<Diagnostic>),
}

/// One diagnostic of the phase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The key's unit: the argument index (§12.2).
    pub unit: u32,
    /// The key's position (§12.2).
    pub seq: u64,
    pub body: Body,
    pub place: Option<Place>,
    pub context: Vec<Context>,
}

impl Problem {
    /// A problem of the phase's own kinds.
    #[must_use]
    pub fn new(unit: u32, seq: u64, severity: Severity, kind: Kind) -> Self {
        Self { unit, seq, body: Body::Own { severity, kind }, place: None, context: Vec::new() }
    }

    /// An error.
    #[must_use]
    pub fn error(unit: u32, seq: u64, kind: Kind) -> Self {
        Self::new(unit, seq, Severity::Error, kind)
    }

    /// A warning.
    #[must_use]
    pub fn warning(unit: u32, seq: u64, kind: Kind) -> Self {
        Self::new(unit, seq, Severity::Warning, kind)
    }

    /// A refusal by name.
    #[must_use]
    pub fn refused(
        unit: u32,
        seq: u64,
        class: RefusalClass,
        name: impl Into<String>,
        gnu_phrase: bool,
    ) -> Self {
        Self {
            unit,
            seq,
            body: Body::Refused { class, name: name.into(), gnu_phrase },
            place: None,
            context: Vec::new(),
        }
    }

    /// The parser's diagnostic.
    #[must_use]
    pub fn parse(unit: u32, seq: u64, diag: Box<Diagnostic>) -> Self {
        Self { unit, seq, body: Body::Parse(diag), place: None, context: Vec::new() }
    }

    /// Sets the place.
    #[must_use]
    pub fn at(mut self, place: Place) -> Self {
        self.place = Some(place);
        self
    }

    /// Adds a note.
    #[must_use]
    pub fn note(mut self, text: impl Into<String>) -> Self {
        self.context.push(Context::Note(text.into()));
        self
    }

    /// Adds `>>> named by` lines.
    #[must_use]
    pub fn named_by(mut self, chain: &[Place]) -> Self {
        self.context.extend(chain.iter().cloned().map(|p| Context::At(Relation::NamedBy, p)));
        self
    }

    /// The kind id (§13; `refused` for a refusal).
    #[must_use]
    pub fn id(&self) -> &'static str {
        match &self.body {
            Body::Own { kind, .. } => kind.id(),
            Body::Refused { .. } => "refused",
            Body::Parse(d) => d.id(),
        }
    }

    /// The severity.
    #[must_use]
    pub fn severity(&self) -> Severity {
        match &self.body {
            Body::Own { severity, .. } => *severity,
            Body::Refused { .. } => Severity::Error,
            Body::Parse(d) => d.severity(),
        }
    }

    /// Whether it is an error.
    #[must_use]
    pub fn is_error(&self) -> bool {
        self.severity() == Severity::Error
    }

    /// The message of the head line, without the place.
    #[must_use]
    pub fn message(&self) -> String {
        match &self.body {
            Body::Own { severity, kind } => head_message(&Diagnostic::new(*severity, kind.clone())),
            Body::Refused { class, name, gnu_phrase } => {
                let mut s = format!("unsupported {}: {name}", class.noun());
                if *gnu_phrase {
                    let _ = write!(s, "; {}", mortar_diag::NOT_RECOGNIZED);
                }
                s
            }
            Body::Parse(d) => d.render(&mortar_diag::Style::plain("mortar")).trim_end().to_owned(),
        }
    }

    /// A plain rendering, one line per context line: what the tests and
    /// the determinism comparison read.
    #[must_use]
    pub fn render(&self) -> String {
        if let Body::Parse(d) = &self.body {
            return d.render(&mortar_diag::Style::plain("mortar")).trim_end().to_owned();
        }
        let mut out = String::new();
        let severity = if self.is_error() { "error" } else { "warning" };
        let _ = write!(out, "{severity}: ");
        if let Some(place) = &self.place {
            let _ = write!(out, "{}: ", place_text(place));
        }
        out.push_str(&self.message());
        for c in &self.context {
            match c {
                Context::At(r, p) => {
                    let _ = write!(out, "\n>>> {} {}", r.phrase(), place_text(p));
                }
                Context::Note(n) => {
                    let _ = write!(out, "\n>>> {n}");
                }
            }
        }
        out
    }
}

/// The message of a diagnostic's head line, as `mortar_diag` renders it.
fn head_message(d: &Diagnostic) -> String {
    let line = d.render(&mortar_diag::Style::plain("p"));
    let line = line.lines().next().unwrap_or_default();
    let rest = line.split_once(": ").map_or(line, |(_, r)| r);
    rest.split_once(": ").map_or(rest, |(_, r)| r).to_owned()
}

impl Problem {
    /// The key (§12.2): phase 20, the argument index, the position.
    #[must_use]
    pub fn key(&self) -> Key {
        Key::new(PHASE, self.unit, self.seq)
    }

    /// The `mortar_diag` diagnostic (§13).
    #[must_use]
    pub fn diagnostic(&self) -> Diagnostic {
        let mut d = match &self.body {
            Body::Own { severity, kind } => Diagnostic::new(*severity, kind.clone()),
            Body::Refused { class, name, gnu_phrase } => Diagnostic::error(Kind::Refused {
                class: *class,
                name: name.clone(),
                not_recognized: *gnu_phrase,
            }),
            Body::Parse(d) => return (**d).clone(),
        };
        if let Some(place) = &self.place {
            d = d.at(place.clone());
        }
        for c in &self.context {
            d = match c {
                Context::At(r, p) => d.with(*r, p.clone()),
                Context::Note(n) => d.note(n.clone()),
            };
        }
        d
    }
}

/// A place as text.
#[must_use]
pub fn place_text(place: &Place) -> String {
    match place {
        Place::File(f) => file_text(f),
        Place::Argument(a) => format!("argument '{a}'"),
        Place::Script { path, line, column } => format!("{path}:{line}:{column}"),
        Place::Section { file, section, offset } => {
            format!("{}:({section}+{offset:#x})", file_text(file))
        }
        Place::Symbol { file, name } => format!("{}:({name})", file_text(file)),
        other => format!("{other:?}"),
    }
}

fn file_text(f: &mortar_diag::FileRef) -> String {
    match f.member() {
        Some(m) => format!("{}({m})", f.path()),
        None => f.path().to_owned(),
    }
}

/// Sorts problems as a `Collector` does: by key, then by rendered text.
pub fn sort(problems: &mut [Problem]) {
    problems.sort_by_cached_key(|p| (p.unit, p.seq, p.render()));
}
