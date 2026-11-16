//! What became of one captured link line (`spec/harness.md` "realbuild",
//! "Outcomes"): the wrapper's record read back, the line's stable id, the
//! kind of output it asks for, and its outcome with mortar; the baseline
//! of those outcomes and its comparison with a run.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

/// A link line as the wrapper (`tests/realbuild/ld`) recorded it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    /// The directory the linker ran in.
    pub cwd: PathBuf,
    /// The linker's arguments, response files not expanded.
    pub argv: Vec<String>,
    /// When the wrapper started (`$EPOCHREALTIME`, microseconds).
    pub start_us: u64,
    /// The control's status: of the line itself (`control` mode) or of its
    /// link to a scratch output (`mortar` mode, the oracle).
    pub control: Option<i32>,
    /// The control's standard error.
    pub control_err: String,
    /// mortar's status (`mortar` mode).
    pub mortar: Option<i32>,
    /// mortar's standard error.
    pub mortar_err: String,
    /// The verifier's report (`realbuild verify`), and its status.
    pub plumb: String,
    pub plumb_status: Option<i32>,
    /// The fallback's status, when mortar did not link the line.
    pub fallback: Option<i32>,
    /// The arguments of the response files the line names, expanded.
    pub expanded: Vec<String>,
    /// The phase of the build (`configure`, `build`, `test`).
    pub phase: String,
}

fn status(dir: &Path, name: &str) -> Option<i32> {
    std::fs::read_to_string(dir.join(name)).ok()?.trim().parse().ok()
}

fn text(dir: &Path, name: &str) -> String {
    std::fs::read(dir.join(name))
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

/// Splits NUL-terminated words.
pub fn nul_words(bytes: &[u8]) -> Vec<String> {
    let body = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    if body.is_empty() {
        return Vec::new();
    }
    body.split(|b| *b == 0).map(|w| String::from_utf8_lossy(w).into_owned()).collect()
}

/// Expands `@file` words (GNU ld's response files, libiberty's quoting),
/// reading each with `read` (a path as the line spells it); at most 16
/// files deep. A file that cannot be read stays a word, as GNU ld keeps it.
pub fn expand(words: &[String], read: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    fn go(
        words: &[String],
        read: &dyn Fn(&str) -> Option<String>,
        depth: u32,
        out: &mut Vec<String>,
    ) {
        for w in words {
            if let Some(path) = w.strip_prefix('@')
                && depth < 16
                && let Some(text) = read(path)
                && let Ok(inner) = crate::bench::response::parse_response(&text)
            {
                go(&inner, read, depth + 1, out);
            } else {
                out.push(w.clone());
            }
        }
    }
    let mut out = Vec::new();
    go(words, read, 0, &mut out);
    out
}

impl Record {
    /// Reads a record directory.
    pub fn read(dir: &Path) -> Result<Self, String> {
        let cwd = std::fs::read(dir.join("cwd")).map_err(|e| format!("{}: {e}", dir.display()))?;
        let cwd = PathBuf::from(nul_words(&cwd).into_iter().next().unwrap_or_default());
        let argv = nul_words(&std::fs::read(dir.join("argv")).unwrap_or_default());
        let start_us = text(dir, "start").trim().replace('.', "").parse().unwrap_or(0);
        // The response files as the wrapper copied them, by their order.
        let list: Vec<String> = text(dir, "rsp.list").lines().map(str::to_owned).collect();
        let copies: BTreeMap<String, String> = list
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                Some((p.clone(), std::fs::read_to_string(dir.join(format!("rsp.{}", i + 1))).ok()?))
            })
            .collect();
        let read =
            |p: &str| copies.get(p).cloned().or_else(|| std::fs::read_to_string(cwd.join(p)).ok());
        let expanded = expand(&argv, &read);
        Ok(Self {
            argv,
            start_us,
            control: status(dir, "control.status"),
            control_err: text(dir, "control.err"),
            mortar: status(dir, "mortar.status"),
            mortar_err: text(dir, "mortar.err"),
            plumb: text(dir, "plumb"),
            plumb_status: status(dir, "plumb.status"),
            fallback: status(dir, "fallback.status"),
            expanded,
            phase: text(dir, "phase").trim().to_owned(),
            cwd,
        })
    }

    /// The output the line names (the last `-o`), else `a.out`.
    pub fn output(&self) -> String {
        let mut found = "a.out".to_owned();
        let mut it = self.expanded.iter();
        while let Some(a) = it.next() {
            if a == "-o" || a == "--output" {
                if let Some(v) = it.next() {
                    found.clone_from(v);
                }
            } else if let Some(v) = a.strip_prefix("--output=") {
                v.clone_into(&mut found);
            } else if let Some(v) = a.strip_prefix("-o").filter(|v| !v.is_empty()) {
                v.clone_into(&mut found);
            }
        }
        found
    }

    /// The kind of output the line asks for.
    pub fn kind(&self) -> OutputKind {
        OutputKind::of(&self.expanded)
    }
}

/// What a line links.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OutputKind {
    /// `-shared`.
    Shared,
    /// `-r` (a relocatable object).
    Relocatable,
    /// `-static -pie` (gcc's `-static-pie`).
    StaticPie,
    /// `-static`, not PIE.
    Static,
    /// `-pie` against shared libraries.
    Pie,
    /// A dynamic executable, not PIE.
    Exec,
}

impl OutputKind {
    pub const ALL: [Self; 6] =
        [Self::Shared, Self::Relocatable, Self::StaticPie, Self::Static, Self::Pie, Self::Exec];

    /// The kind GNU ld would produce from `words`: the last of `-pie` and
    /// `-no-pie` wins; `-static`, `-Bstatic` before any input, and
    /// `--no-dynamic-linker` with `-pie` make the static kinds.
    pub fn of(words: &[String]) -> Self {
        let mut shared = false;
        let mut reloc = false;
        let mut pie = false;
        for w in words {
            match w.as_str() {
                "-shared" | "-Bshareable" | "--shared" => shared = true,
                "-r" | "-i" | "--relocatable" | "-Ur" => reloc = true,
                "-pie" | "--pic-executable" => pie = true,
                "-no-pie" | "--no-pic-executable" => pie = false,
                _ => {}
            }
        }
        // gcc's static line is `-static`; its static-pie line `-static -pie
        // --no-dynamic-linker`. A `-Bstatic` alone is a search mode, not an
        // output kind: only `-static` counts.
        let static_line = words.iter().any(|w| w == "-static" || w == "--static");
        if reloc {
            Self::Relocatable
        } else if shared {
            Self::Shared
        } else if pie && (static_line || words.iter().any(|w| w == "--no-dynamic-linker")) {
            Self::StaticPie
        } else if static_line {
            Self::Static
        } else if pie {
            Self::Pie
        } else {
            Self::Exec
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Relocatable => "relocatable",
            Self::StaticPie => "static-pie",
            Self::Static => "static",
            Self::Pie => "pie",
            Self::Exec => "exec",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == text)
    }
}

/// The nouns of mortar's refusals (`spec/diag.md` §4.1), longest first so
/// that `option value` is not read as `option`.
const NOUNS: &[&str] = &[
    "linker script command",
    "compression type",
    "relocation type",
    "symbol binding",
    "section flag",
    "section type",
    "option value",
    "object type",
    "file format",
    "symbol type",
    "emulation",
    "feature",
    "machine",
    "option",
];

/// The refusals a standard error names (`<prog>: error: [<place>: ]unsupported
/// <noun>: <name>`, `spec/diag.md` §4): `<noun>: <name>` each, once, in
/// order.
pub fn refusals(stderr: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in stderr.lines() {
        if !line.contains(": error: ") {
            continue;
        }
        let Some(at) = line.find("unsupported ") else { continue };
        let rest = &line[at + "unsupported ".len()..];
        let Some(noun) = NOUNS.iter().find(|n| rest.starts_with(&format!("{n}: "))) else {
            continue;
        };
        let name = rest[noun.len() + 2..].trim();
        let refusal = format!("{noun}: {}", normalise_name(name));
        // `linking` is the consequence of an option's refusal (nothing is
        // linked once one is refused), not a construct of its own.
        if refusal == "feature: linking" {
            continue;
        }
        if !out.contains(&refusal) {
            out.push(refusal);
        }
    }
    out
}

/// A refused name as the ranking counts it: an option's value after `=`
/// kept (`--hash-style=sysv` is its own row) but a path in it is not
/// (`--version-script=<path>` is the option).
fn normalise_name(name: &str) -> String {
    match name.split_once('=') {
        Some((opt, value)) if value.contains('/') || value.contains('.') => format!("{opt}=<file>"),
        _ => name.to_owned(),
    }
}

/// What became of a line with mortar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// mortar linked it, the verifier is clean, the control links it too.
    Linked,
    /// mortar refused it by name (each `<noun>: <name>`); the control links it.
    Refused(Vec<String>),
    /// Neither links it (a configure probe of what is not there, mostly).
    BothFail,
    /// A defect: the verifier found something on mortar's output.
    Finding(String),
    /// A defect: the verifier could not judge mortar's output.
    VerifierError(String),
    /// A defect: mortar links a line the control cannot.
    LinksControlCannot,
    /// A defect: mortar crashed (a signal, a panic).
    Crash(String),
    /// A defect: mortar failed with a diagnostic that is not a refusal
    /// while the control links the line.
    Error(String),
    /// The record is incomplete (the wrapper was cut short).
    Incomplete,
}

impl Outcome {
    /// Whether this is a defect row (`spec/harness.md` "realbuild": a
    /// wrong output, a wrong diagnostic, a crash, a verifier finding).
    pub fn is_defect(&self) -> bool {
        matches!(
            self,
            Self::Finding(_)
                | Self::VerifierError(_)
                | Self::LinksControlCannot
                | Self::Crash(_)
                | Self::Error(_)
                | Self::Incomplete
        )
    }

    /// The outcome's word.
    pub fn word(&self) -> &'static str {
        match self {
            Self::Linked => "linked",
            Self::Refused(_) => "refused",
            Self::BothFail => "both-fail",
            Self::Finding(_) => "finding",
            Self::VerifierError(_) => "verifier-error",
            Self::LinksControlCannot => "links-control-cannot",
            Self::Crash(_) => "crash",
            Self::Error(_) => "error",
            Self::Incomplete => "incomplete",
        }
    }

    /// The baseline's text: the word, then its detail.
    pub fn to_text(&self) -> String {
        let detail = match self {
            Self::Refused(names) => names.join("; "),
            Self::Finding(d) | Self::VerifierError(d) | Self::Crash(d) | Self::Error(d) => {
                d.clone()
            }
            _ => String::new(),
        };
        let detail: String =
            detail.chars().map(|c| if c == '\t' || c == '\n' { ' ' } else { c }).collect();
        if detail.is_empty() { self.word().to_owned() } else { format!("{} {detail}", self.word()) }
    }

    /// Reads [`Outcome::to_text`] back.
    pub fn parse(text: &str) -> Option<Self> {
        let (word, detail) = text.split_once(' ').unwrap_or((text, ""));
        let d = detail.to_owned();
        Some(match word {
            "linked" => Self::Linked,
            "refused" => Self::Refused(detail.split("; ").map(str::to_owned).collect()),
            "both-fail" => Self::BothFail,
            "finding" => Self::Finding(d),
            "verifier-error" => Self::VerifierError(d),
            "links-control-cannot" => Self::LinksControlCannot,
            "crash" => Self::Crash(d),
            "error" => Self::Error(d),
            "incomplete" => Self::Incomplete,
            _ => return None,
        })
    }

    /// Of a record of the `mortar` mode.
    pub fn of(r: &Record) -> Self {
        let (Some(mortar), Some(control)) = (r.mortar, r.control) else {
            return Self::Incomplete;
        };
        let control_links = control == 0;
        if mortar == 0 {
            if !control_links {
                return Self::LinksControlCannot;
            }
            return match r.plumb_status {
                Some(0) => Self::Linked,
                Some(1) => Self::Finding(first_line(&r.plumb)),
                _ => Self::VerifierError(first_line(&r.plumb)),
            };
        }
        if mortar >= 128 || r.mortar_err.contains("panicked at") {
            return Self::Crash(format!("status {mortar}: {}", first_line(&r.mortar_err)));
        }
        if !control_links {
            return Self::BothFail;
        }
        let refused = refusals(&r.mortar_err);
        if refused.is_empty() {
            let first = r
                .mortar_err
                .lines()
                .find(|l| l.contains("error"))
                .map_or_else(|| first_line(&r.mortar_err), str::to_owned);
            Self::Error(format!("status {mortar}: {first}"))
        } else {
            Self::Refused(refused)
        }
    }
}

fn first_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    line.chars().take(300).collect()
}

/// `path` with `.` and `..` resolved by their spelling.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Whether a path component is a name some tool chose at random:
/// `cc` + six letters or digits (gcc's temporary files), `rustc` + six,
/// `tmp.` + ten (`mktemp`), `TryCompile-` + six and `cmTC_` + five
/// (`CMake`'s checks).
fn random_part(part: &str) -> Option<String> {
    let stem = part.split('.').next().unwrap_or(part);
    let alnum = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_alphanumeric());
    for (prefix, n) in [("cc", 6), ("rustc", 6), ("TryCompile-", 6), ("cmTC_", 5)] {
        if let Some(rest) = stem.strip_prefix(prefix)
            && alnum(rest, n)
        {
            return Some(format!("{prefix}*{}", &part[stem.len()..]));
        }
    }
    if let Some(rest) = part.strip_prefix("tmp.")
        && alnum(rest, 10)
    {
        return Some("tmp.*".to_owned());
    }
    None
}

/// The line's id: the output's path relative to the build's directory
/// (`root`), its random components written `*`; outside `root`, the path as
/// it is with the same rule. A line of the `configure` phase has `configure/`
/// before it.
pub fn line_id(r: &Record, root: &Path) -> String {
    let id = output_id(r, root);
    if r.phase == "configure" { format!("configure/{id}") } else { id }
}

fn output_id(r: &Record, root: &Path) -> String {
    let out = r.output();
    let full = lexical(&r.cwd.join(out));
    let rel = full.strip_prefix(lexical(root)).map_or(full.clone(), Path::to_path_buf);
    let parts: Vec<String> = rel
        .components()
        .map(|c| {
            let s = c.as_os_str().to_string_lossy();
            random_part(&s).unwrap_or_else(|| s.into_owned())
        })
        .collect();
    parts.join("/").replace("//", "/")
}

/// Whether a line is a configure-time probe (autoconf's `conftest`,
/// `CMake`'s checks, a temporary directory).
pub fn is_probe(id: &str) -> bool {
    id.starts_with("configure/")
        || id.contains("conftest")
        || id.contains("CMakeScratch")
        || id.contains("CMakeTmp")
        || id.contains("TryCompile")
        || id.contains("cmTC_")
        || id.starts_with("tmp/")
}

/// The records of a run, in the order their links started, with their ids
/// (a repeated id gets `#2`, `#3`, ...).
pub fn read_records(records: &Path, root: &Path) -> Result<Vec<(String, Record)>, String> {
    let mut all = Vec::new();
    let entries = match std::fs::read_dir(records) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", records.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", records.display()))?;
        let path = entry.path();
        if path.is_dir() && entry.file_name().to_string_lossy().starts_with("link.") {
            let rec = Record::read(&path)?;
            all.push((entry.file_name().to_string_lossy().into_owned(), rec));
        }
    }
    all.sort_by(|a, b| (a.1.start_us, &a.0).cmp(&(b.1.start_us, &b.0)));
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    Ok(all
        .into_iter()
        .map(|(_, r)| {
            let id = line_id(&r, root);
            let n = seen.entry(id.clone()).or_default();
            *n += 1;
            let id = if *n == 1 { id } else { format!("{id}#{n}") };
            (id, r)
        })
        .collect())
}

/// One row of the baseline (`tests/realbuild/baseline.tsv`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub project: String,
    pub row: String,
    pub line: String,
    pub kind: OutputKind,
    pub outcome: Outcome,
}

/// The baseline's first line.
pub const BASELINE_HEADER: &str =
    "# mortar-realbuild-baseline 1\tproject\trow\tline\tkind\toutcome";

pub fn write_rows(rows: &[Row]) -> String {
    let mut s = String::from(BASELINE_HEADER);
    s.push('\n');
    for r in rows {
        let _ = writeln!(
            s,
            "{}\t{}\t{}\t{}\t{}",
            r.project,
            r.row,
            r.line,
            r.kind.name(),
            r.outcome.to_text()
        );
    }
    s
}

pub fn parse_rows(text: &str) -> Result<Vec<Row>, String> {
    let mut out = Vec::new();
    for (at, line) in text.lines().enumerate() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        let [project, row, id, kind, outcome] = f.as_slice() else {
            return Err(format!("line {}: five tab-separated fields expected", at + 1));
        };
        out.push(Row {
            project: (*project).to_owned(),
            row: (*row).to_owned(),
            line: (*id).to_owned(),
            kind: OutputKind::parse(kind).ok_or(format!("line {}: kind `{kind}`", at + 1))?,
            outcome: Outcome::parse(outcome)
                .ok_or(format!("line {}: outcome `{outcome}`", at + 1))?,
        });
    }
    Ok(out)
}

/// A run against the baseline, for the (project, row) pairs the run has.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Comparison {
    /// Linked in the baseline, not now; or a defect now that was not one.
    pub regressed: Vec<String>,
    /// Not linked in the baseline, linked now.
    pub fixed: Vec<String>,
    /// Another outcome that is neither (another refusal name, say).
    pub changed: Vec<String>,
    /// In the run, not in the baseline.
    pub new: Vec<String>,
    /// In the baseline, not in the run.
    pub lost: Vec<String>,
}

pub fn compare(baseline: &[Row], run: &[Row]) -> Comparison {
    let pairs: std::collections::BTreeSet<(&str, &str)> =
        run.iter().map(|r| (r.project.as_str(), r.row.as_str())).collect();
    let key = |r: &Row| format!("{}/{}/{}", r.project, r.row, r.line);
    let old: BTreeMap<String, &Row> = baseline
        .iter()
        .filter(|r| pairs.contains(&(r.project.as_str(), r.row.as_str())))
        .map(|r| (key(r), r))
        .collect();
    let now: BTreeMap<String, &Row> = run.iter().map(|r| (key(r), r)).collect();
    let mut c = Comparison::default();
    for (k, r) in &now {
        let Some(o) = old.get(k) else {
            c.new.push(format!("{k}: {}", r.outcome.to_text()));
            if r.outcome.is_defect() {
                c.regressed.push(format!("{k}: (new) {}", r.outcome.to_text()));
            }
            continue;
        };
        if o.outcome == r.outcome {
            continue;
        }
        let what = format!("{k}: {} -> {}", o.outcome.to_text(), r.outcome.to_text());
        if (o.outcome == Outcome::Linked && r.outcome != Outcome::Linked)
            || (r.outcome.is_defect() && !o.outcome.is_defect())
        {
            c.regressed.push(what);
        } else if r.outcome == Outcome::Linked {
            c.fixed.push(what);
        } else {
            c.changed.push(what);
        }
    }
    for (k, o) in &old {
        if !now.contains_key(k) {
            c.lost.push(format!("{k}: {}", o.outcome.to_text()));
        }
    }
    c
}

// spec/harness.md "realbuild", "Outcomes".
#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &[&str]) -> Vec<String> {
        s.iter().map(|w| (*w).to_owned()).collect()
    }

    #[test]
    fn refusals_are_read_by_noun() {
        let err = "mortar: error: unsupported option: -shared\n\
                   mortar: error: unsupported option value: --hash-style=sysv\n\
                   mortar: error: a.o: unsupported section type: SHT_GNU_verdef\n\
                   mortar: error: unsupported option: -shared\n\
                   mortar: error: unsupported option: --version-script=libz.map\n\
                   mortar: error: unsupported feature: linking\n\
                   mortar: error: no input files\n";
        assert_eq!(
            refusals(err),
            [
                "option: -shared",
                "option value: --hash-style=sysv",
                "section type: SHT_GNU_verdef",
                "option: --version-script=<file>"
            ]
        );
        assert!(refusals("mortar: error: undefined symbol: foo\n").is_empty());
    }

    #[test]
    fn kinds_follow_the_line() {
        let k = |s: &[&str]| OutputKind::of(&words(s));
        assert_eq!(k(&["-shared", "-o", "x.so"]), OutputKind::Shared);
        assert_eq!(k(&["-r", "a.o"]), OutputKind::Relocatable);
        assert_eq!(k(&["-static", "-pie", "--no-dynamic-linker"]), OutputKind::StaticPie);
        assert_eq!(k(&["-static", "a.o"]), OutputKind::Static);
        assert_eq!(k(&["-pie", "-Bstatic", "-lfoo", "-Bdynamic"]), OutputKind::Pie);
        assert_eq!(k(&["-pie", "-no-pie"]), OutputKind::Exec);
        for kind in OutputKind::ALL {
            assert_eq!(OutputKind::parse(kind.name()), Some(kind));
        }
    }

    fn record(mortar: i32, control: i32, plumb: Option<i32>, err: &str) -> Record {
        Record {
            mortar: Some(mortar),
            control: Some(control),
            plumb_status: plumb,
            plumb: "plumb: 1 findings: R1 x1".into(),
            mortar_err: err.into(),
            ..Record::default()
        }
    }

    #[test]
    fn outcomes_follow_mortar_the_control_and_the_verifier() {
        assert_eq!(Outcome::of(&record(0, 0, Some(0), "")), Outcome::Linked);
        assert!(matches!(Outcome::of(&record(0, 0, Some(1), "")), Outcome::Finding(_)));
        assert!(matches!(Outcome::of(&record(0, 0, Some(2), "")), Outcome::VerifierError(_)));
        assert_eq!(Outcome::of(&record(0, 1, Some(0), "")), Outcome::LinksControlCannot);
        assert_eq!(
            Outcome::of(&record(1, 0, None, "mortar: error: unsupported option: -r\n")),
            Outcome::Refused(vec!["option: -r".into()])
        );
        assert_eq!(
            Outcome::of(&record(1, 1, None, "mortar: error: undefined symbol: f\n")),
            Outcome::BothFail
        );
        assert!(matches!(
            Outcome::of(&record(1, 0, None, "mortar: error: undefined symbol: f\n")),
            Outcome::Error(_)
        ));
        assert!(matches!(Outcome::of(&record(134, 1, None, "")), Outcome::Crash(_)));
        assert_eq!(Outcome::of(&Record::default()), Outcome::Incomplete);
        for o in [
            Outcome::Linked,
            Outcome::Refused(vec!["option: -shared".into(), "feature: x y".into()]),
            Outcome::BothFail,
            Outcome::Finding("plumb: 2 findings".into()),
            Outcome::Crash("status 134: x".into()),
        ] {
            assert_eq!(Outcome::parse(&o.to_text()), Some(o));
        }
    }

    #[test]
    fn ids_are_relative_and_random_names_starred() {
        let r = Record {
            cwd: "/w/b/zlib-gcc-mortar/CMakeFiles/CMakeScratch/TryCompile-AbC123".into(),
            expanded: words(&["-o", "cmTC_1a2b3", "x.o"]),
            ..Record::default()
        };
        let id = line_id(&r, Path::new("/w/b/zlib-gcc-mortar"));
        assert_eq!(id, "CMakeFiles/CMakeScratch/TryCompile-*/cmTC_*");
        assert!(is_probe(&id));
        let r = Record {
            cwd: "/w/b/x/src".into(),
            expanded: words(&["-o", "../bin/lua"]),
            ..Record::default()
        };
        assert_eq!(line_id(&r, Path::new("/w/b/x")), "bin/lua");
        assert!(!is_probe("bin/lua"));
    }

    #[test]
    fn response_files_are_expanded() {
        let read = |p: &str| match p {
            "a.rsp" => Some("x.o \"y z.o\" @b.rsp".to_owned()),
            "b.rsp" => Some("-lm".to_owned()),
            _ => None,
        };
        assert_eq!(
            expand(&words(&["-o", "p", "@a.rsp", "@missing"]), &read),
            ["-o", "p", "x.o", "y z.o", "-lm", "@missing"]
        );
    }

    #[test]
    fn the_baseline_round_trips_and_compares() {
        let row = |line: &str, o: Outcome| Row {
            project: "zlib".into(),
            row: "gcc".into(),
            line: line.into(),
            kind: OutputKind::Pie,
            outcome: o,
        };
        let base = vec![
            row("a", Outcome::Linked),
            row("b", Outcome::Refused(vec!["option: -shared".into()])),
            row("c", Outcome::Linked),
            row("gone", Outcome::Linked),
        ];
        assert_eq!(parse_rows(&write_rows(&base)).unwrap(), base);
        let run = vec![
            row("a", Outcome::Linked),
            row("b", Outcome::Linked),
            row("c", Outcome::Refused(vec!["option: -r".into()])),
            row("new", Outcome::BothFail),
        ];
        let c = compare(&base, &run);
        assert_eq!(c.regressed.len(), 1);
        assert_eq!(c.fixed.len(), 1);
        assert_eq!(c.new.len(), 1);
        assert_eq!(c.lost.len(), 1);
        // Another project's rows are not compared.
        let other = vec![Row { project: "lua".into(), ..row("a", Outcome::Linked) }];
        assert_eq!(compare(&base, &other).lost.len(), 0);
    }
}
