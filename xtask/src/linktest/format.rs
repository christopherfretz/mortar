//! The link-test file format (`spec/harness.md` "Link tests": "A test
//! file", "Directives", "Steps", "Expectations", "Substitutions",
//! "Errors").
//!
//! One text file is one test: `#@` directives, then the steps with their
//! indented expectations, then the embedded input files, each after a
//! `#--- <name>` line. [`parse`] turns the text into a [`Test`] or into
//! every error of the file, each with its line.

use std::fmt;
use std::time::Duration;

use super::pattern::{self, Pattern};
use super::target::{Arch, Family, Feature, Linker};

/// An error of a test file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// The line it is on, from 1.
    pub line: usize,
    pub message: String,
}

/// `line 7: unknown step 'compile'`.
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

/// A name `%{...}` stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Var {
    Arch,
    Triple,
    Emulation,
    DynamicLinker,
    Sysroot,
    Libdir,
    ElfClass,
    Machine,
    Linker,
}

impl Var {
    /// Every variable.
    pub const ALL: [Self; 9] = [
        Self::Arch,
        Self::Triple,
        Self::Emulation,
        Self::DynamicLinker,
        Self::Sysroot,
        Self::Libdir,
        Self::ElfClass,
        Self::Machine,
        Self::Linker,
    ];

    /// The name between `%{` and `}`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Arch => "arch",
            Self::Triple => "triple",
            Self::Emulation => "emulation",
            Self::DynamicLinker => "dynamic-linker",
            Self::Sysroot => "sysroot",
            Self::Libdir => "libdir",
            Self::ElfClass => "elf-class",
            Self::Machine => "machine",
            Self::Linker => "linker",
        }
    }
}

/// What the variables stand for in one cell of the matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vars {
    pub arch: Arch,
    /// The machine the harness runs on: it decides the names of the
    /// architecture's directories.
    pub host: Option<Arch>,
    /// The linker of a link, run or check step; `None` in a build step,
    /// which has none.
    pub linker: Option<Linker>,
}

impl Vars {
    fn value(self, var: Var) -> String {
        match var {
            Var::Arch => self.arch.name().to_owned(),
            Var::Triple => self.arch.triple().to_owned(),
            Var::Emulation => self.arch.emulation().to_owned(),
            Var::DynamicLinker => self.arch.dynamic_linker().to_owned(),
            Var::Sysroot => self.arch.sysroot(self.host).display().to_string(),
            Var::Libdir => self.arch.libdir(self.host).display().to_string(),
            Var::ElfClass => self.arch.elf_class().to_owned(),
            Var::Machine => self.arch.machine().to_owned(),
            Var::Linker => self.linker.map_or("", Linker::name).to_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Piece {
    Text(String),
    Var(Var),
}

/// Text with `%{name}` substitutions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pieces: Vec<Piece>,
}

impl Template {
    /// Parses `text`: `%{name}` is a variable, `%%` a percent sign, and
    /// every other character itself.
    ///
    /// # Errors
    /// An unknown variable, or `%{` without `}`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut pieces = Vec::new();
        let mut literal = String::new();
        let mut rest = text;
        while let Some(at) = rest.find('%') {
            literal.push_str(&rest[..at]);
            let after = &rest[at + 1..];
            if let Some(after) = after.strip_prefix('%') {
                literal.push('%');
                rest = after;
            } else if let Some(after) = after.strip_prefix('{') {
                let Some(end) = after.find('}') else {
                    return Err(format!("'%{{' without '}}' in '{text}'"));
                };
                let name = &after[..end];
                let Some(var) = Var::ALL.into_iter().find(|var| var.name() == name) else {
                    let known: Vec<&str> = Var::ALL.iter().map(|var| var.name()).collect();
                    return Err(format!(
                        "unknown substitution '%{{{name}}}' (known: {})",
                        known.join(", ")
                    ));
                };
                if !literal.is_empty() {
                    pieces.push(Piece::Text(std::mem::take(&mut literal)));
                }
                pieces.push(Piece::Var(var));
                rest = &after[end + 1..];
            } else {
                literal.push('%');
                rest = after;
            }
        }
        literal.push_str(rest);
        if !literal.is_empty() {
            pieces.push(Piece::Text(literal));
        }
        Ok(Self { pieces })
    }

    /// The text with every variable replaced.
    pub fn expand(&self, vars: Vars) -> String {
        self.expand_with(vars, |value| value)
    }

    /// The text as a pattern: a variable's value stands for itself,
    /// whatever characters it has.
    pub fn expand_pattern(&self, vars: Vars) -> String {
        self.expand_with(vars, |value| pattern::escape(&value))
    }

    fn expand_with(&self, vars: Vars, value: impl Fn(String) -> String) -> String {
        let mut text = String::new();
        for piece in &self.pieces {
            match piece {
                Piece::Text(literal) => text.push_str(literal),
                Piece::Var(var) => text.push_str(&value(vars.value(*var))),
            }
        }
        text
    }

    /// Whether the text names the linker.
    pub fn uses_linker(&self) -> bool {
        self.pieces.contains(&Piece::Var(Var::Linker))
    }

    /// The text when it has no variable.
    pub fn literal(&self) -> Option<&str> {
        match self.pieces.as_slice() {
            [] => Some(""),
            [Piece::Text(text)] => Some(text),
            _ => None,
        }
    }
}

/// The part of the matrix a step, an expectation or a marker is for.
/// An empty list is "every".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    pub linkers: Vec<Linker>,
    pub arches: Vec<Arch>,
}

impl Scope {
    /// Whether it includes `arch`.
    pub fn has_arch(&self, arch: Arch) -> bool {
        self.arches.is_empty() || self.arches.contains(&arch)
    }

    /// Whether it includes `linker`.
    pub fn has_linker(&self, linker: Linker) -> bool {
        self.linkers.is_empty() || self.linkers.contains(&linker)
    }

    /// Whether it includes `linker` on `arch`.
    pub fn applies(&self, arch: Arch, linker: Linker) -> bool {
        self.has_arch(arch) && self.has_linker(linker)
    }
}

/// `xfail` or `skip`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerKind {
    Xfail,
    Skip,
}

/// An `xfail(...)` or `skip(...)` directive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Marker {
    pub line: usize,
    pub kind: MarkerKind,
    /// At least one linker.
    pub scope: Scope,
    pub reason: String,
}

/// An `anomaly(...)` directive: on the links of these reference linkers,
/// the verifier's findings that `spec/plumb.md` §10.2 records under these
/// ids are expected (L3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anomaly {
    pub line: usize,
    /// At least one linker, never mortar.
    pub scope: Scope,
    /// `A1`, `A9`, ...
    pub ids: Vec<String>,
}

/// The language of a compile step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    C,
    Cxx,
    Fortran,
    Asm,
}

/// What a link step hands the link line to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Driver {
    /// The linker itself.
    Ld,
    Gcc,
    Gxx,
    Gfortran,
    Clang,
    Clangxx,
}

impl Driver {
    const ALL: [Self; 6] =
        [Self::Ld, Self::Gcc, Self::Gxx, Self::Gfortran, Self::Clang, Self::Clangxx];

    /// The word after `link`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Ld => "ld",
            Self::Gcc => "gcc",
            Self::Gxx => "g++",
            Self::Gfortran => "gfortran",
            Self::Clang => "clang",
            Self::Clangxx => "clang++",
        }
    }
}

/// The tool of a check step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckTool {
    Readelf,
    Nm,
    Objdump,
    LlvmReadelf,
    /// The lines of a file the link wrote.
    Text,
}

impl CheckTool {
    const ALL: [Self; 5] = [Self::Readelf, Self::Nm, Self::Objdump, Self::LlvmReadelf, Self::Text];

    /// The word after `check`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Readelf => "readelf",
            Self::Nm => "nm",
            Self::Objdump => "objdump",
            Self::LlvmReadelf => "llvm-readelf",
            Self::Text => "text",
        }
    }
}

/// A `has`, `not`, `diag` or `no-diag` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expectation {
    pub line: usize,
    pub scope: Scope,
    /// A line must match (`has`, `diag`), or none may (`not`, `no-diag`).
    pub present: bool,
    pub pattern: Template,
}

/// A compile step: `cc`, `cxx`, `fc` or `as`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compile {
    pub language: Language,
    pub family: Family,
    pub args: Vec<Template>,
}

/// A link step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub driver: Driver,
    pub args: Vec<Template>,
    /// `exit: fail`: the link is expected to fail.
    pub fails: bool,
    /// The `diag` and `no-diag` lines.
    pub diags: Vec<Expectation>,
}

/// How a program is expected to end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Status(i32),
    Signal(i32),
}

/// What a program is expected to print.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stdout {
    /// These lines, each followed by a newline, and nothing else.
    Exact(Vec<Template>),
    /// What the program prints when a reference linker links it.
    Reference,
}

/// A run step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub program: Template,
    pub args: Vec<Template>,
    pub exit: Exit,
    pub stdout: Stdout,
}

/// A check step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub tool: CheckTool,
    pub args: Vec<Template>,
    /// The `has` and `not` lines, at least one.
    pub expectations: Vec<Expectation>,
}

/// What a step does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepKind {
    Compile(Compile),
    Yaml2obj(Vec<Template>),
    Ar(Vec<Template>),
    Link(Link),
    Run(Run),
    Check(Check),
}

/// One step of a test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub line: usize,
    /// The step's first word without its qualifiers: `cc`, `link`.
    pub keyword: &'static str,
    pub scope: Scope,
    pub kind: StepKind,
}

impl Step {
    /// Whether the step builds an input: it runs once per architecture,
    /// before any linker, and its files are shared by all of them.
    pub fn is_build(&self) -> bool {
        matches!(self.kind, StepKind::Compile(_) | StepKind::Yaml2obj(_) | StepKind::Ar(_))
    }

    /// The `requires` word the step's tool needs, if it is not a tool
    /// every test may use.
    pub fn feature(&self) -> Option<Feature> {
        match &self.kind {
            StepKind::Compile(compile) => match (compile.family, compile.language) {
                (Family::Clang, _) => Some(Feature::Clang),
                (Family::Gcc, Language::Cxx) => Some(Feature::Gxx),
                (Family::Gcc, Language::Fortran) => Some(Feature::Gfortran),
                (Family::Gcc, Language::C | Language::Asm) => None,
            },
            StepKind::Yaml2obj(_) => Some(Feature::Yaml2obj),
            StepKind::Link(link) => match link.driver {
                Driver::Gxx => Some(Feature::Gxx),
                Driver::Gfortran => Some(Feature::Gfortran),
                Driver::Clang | Driver::Clangxx => Some(Feature::Clang),
                Driver::Ld | Driver::Gcc => None,
            },
            StepKind::Check(check) if check.tool == CheckTool::LlvmReadelf => {
                Some(Feature::LlvmReadelf)
            }
            StepKind::Ar(_) | StepKind::Run(_) | StepKind::Check(_) => None,
        }
    }

    /// `link (line 12)`: the step in a message.
    pub fn describe(&self) -> String {
        format!("{} (line {})", self.keyword, self.line)
    }
}

/// A file embedded in a test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Embedded {
    /// A relative path.
    pub name: String,
    pub contents: String,
}

/// A parsed test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Test {
    /// The spec sections the test checks, at least one.
    pub spec: Vec<String>,
    /// The architectures the test is for, in [`Arch::ALL`] order.
    pub arches: Vec<Arch>,
    pub requires: Vec<Feature>,
    pub markers: Vec<Marker>,
    pub anomalies: Vec<Anomaly>,
    /// The wall-clock limit of each link and run step, if the test sets
    /// one.
    pub timeout: Option<Duration>,
    /// The address-space limit of each link and run step in MiB, if the
    /// test sets one.
    pub memory_mib: Option<u64>,
    pub steps: Vec<Step>,
    pub files: Vec<Embedded>,
}

impl Test {
    /// The `xfail` or `skip` that applies to `linker` on `arch`. At most
    /// one does.
    pub fn marker(&self, arch: Arch, linker: Linker) -> Option<&Marker> {
        self.markers.iter().find(|marker| marker.scope.applies(arch, linker))
    }

    /// Whether a run step takes its expected output from the references.
    pub fn compares_output(&self) -> bool {
        self.steps
            .iter()
            .any(|step| matches!(&step.kind, StepKind::Run(run) if run.stdout == Stdout::Reference))
    }
}

/// The largest `timeout`, in seconds, and the bounds of `mem`, in MiB.
const MAX_TIMEOUT: u64 = 3600;
const MEMORY_RANGE: std::ops::RangeInclusive<u64> = 16..=65536;

/// The qualifiers in the parentheses after a keyword, sorted by kind.
#[derive(Debug, Default)]
struct Qualifiers {
    families: Vec<Family>,
    scope: Scope,
}

impl Qualifiers {
    fn parse(list: &str) -> Result<Self, String> {
        let mut qualifiers = Self::default();
        for word in list.split(',') {
            let duplicate = || Err(format!("the qualifier '{word}' is given twice"));
            if let Some(family) = Family::parse(word) {
                if qualifiers.families.contains(&family) {
                    return duplicate();
                }
                qualifiers.families.push(family);
            } else if let Some(linker) = Linker::parse(word) {
                if qualifiers.scope.linkers.contains(&linker) {
                    return duplicate();
                }
                qualifiers.scope.linkers.push(linker);
            } else if let Some(arch) = Arch::parse(word) {
                if qualifiers.scope.arches.contains(&arch) {
                    return duplicate();
                }
                qualifiers.scope.arches.push(arch);
            } else if word.is_empty() {
                return Err("an empty qualifier".to_owned());
            } else {
                return Err(format!(
                    "unknown qualifier '{word}': expected a compiler family (gcc, clang), a \
                     linker (bfd, lld, mold, mortar) or an architecture (aarch64, x86_64, i386)"
                ));
            }
        }
        Ok(qualifiers)
    }

    /// For a keyword that takes linkers and architectures only.
    fn no_family(&self, what: &str) -> Result<(), String> {
        match self.families.first() {
            Some(family) => Err(format!("{what} takes no compiler family ('{}')", family.name())),
            None => Ok(()),
        }
    }

    /// For a step that runs before any linker.
    fn no_linker(&self, what: &str) -> Result<(), String> {
        match self.scope.linkers.first() {
            Some(linker) => Err(format!(
                "{what} builds an input, which every linker gets: it cannot be for '{}' only",
                linker.name()
            )),
            None => Ok(()),
        }
    }

    fn is_empty(&self) -> bool {
        self.families.is_empty() && self.scope.linkers.is_empty() && self.scope.arches.is_empty()
    }
}

/// A keyword with its qualifiers, and what follows them.
struct Keyed<'a> {
    key: &'a str,
    /// Whether there were parentheses.
    qualified: bool,
    qualifiers: Qualifiers,
    rest: &'a str,
}

/// Splits `<key>` or `<key>(<qualifiers>)` off the front of `text`. The
/// key is lower-case letters, digits, `+` and `-`.
fn keyed(text: &str) -> Result<Keyed<'_>, String> {
    let end = text
        .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '+'))
        .unwrap_or(text.len());
    let (key, rest) = text.split_at(end);
    if key.is_empty() {
        return Err("expected a keyword of lower-case letters".to_owned());
    }
    let Some(list) = rest.strip_prefix('(') else {
        return Ok(Keyed { key, qualified: false, qualifiers: Qualifiers::default(), rest });
    };
    let Some(close) = list.find(')') else {
        return Err(format!("'{key}(' without ')'"));
    };
    let qualifiers = Qualifiers::parse(&list[..close])?;
    debug_assert!(!qualifiers.is_empty(), "a parsed list has a qualifier");
    Ok(Keyed { key, qualified: true, qualifiers, rest: &list[close + 1..] })
}

/// Splits a step's text into words. Blanks separate words; a part in
/// double quotes is taken as it is, blanks included, with `\"` and `\\`
/// for a quote and a backslash.
fn words(text: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => words.extend(word.take()),
            '"' => {
                let word = word.get_or_insert_default();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(escaped @ ('"' | '\\')) => word.push(escaped),
                            _ => {
                                return Err("in double quotes a backslash goes before '\"' or \
                                            '\\' only"
                                    .to_owned());
                            }
                        },
                        Some(other) => word.push(other),
                        None => return Err("a double quote is not closed".to_owned()),
                    }
                }
            }
            other => word.get_or_insert_default().push(other),
        }
    }
    words.extend(word);
    Ok(words)
}

/// Whether `name` is a relative path of plain components.
fn valid_file_name(name: &str) -> bool {
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-');
    !name.is_empty()
        && name
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && part.chars().all(plain))
}

/// Whether `value` has the form of a spec reference: a file of `spec/`
/// (`resolve.md`), a blank, and the section.
fn valid_spec(value: &str) -> bool {
    let Some((file, section)) = value.split_once([' ', '\t']) else {
        return false;
    };
    let name = file.strip_suffix(".md").unwrap_or("");
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !section.trim().is_empty()
}

/// The parser's state.
#[derive(Default)]
struct Parser {
    errors: Vec<Error>,
    spec: Vec<String>,
    arches: Option<(usize, Vec<Arch>)>,
    requires: Option<(usize, Vec<Feature>)>,
    markers: Vec<Marker>,
    anomalies: Vec<Anomaly>,
    timeout: Option<Duration>,
    memory_mib: Option<u64>,
    steps: Vec<Step>,
    /// `exit` was given for the last step.
    exit_seen: bool,
    files: Vec<Embedded>,
}

impl Parser {
    fn error(&mut self, line: usize, message: impl Into<String>) {
        self.errors.push(Error { line, message: message.into() });
    }

    fn template(&mut self, line: usize, text: &str) -> Option<Template> {
        match Template::parse(text) {
            Ok(template) => Some(template),
            Err(message) => {
                self.error(line, message);
                None
            }
        }
    }

    /// A pattern's template, checked to be a pattern whatever the
    /// variables stand for.
    fn pattern(&mut self, line: usize, text: &str) -> Option<Template> {
        let template = self.template(line, text)?;
        let sample = Vars { arch: Arch::Aarch64, host: None, linker: Some(Linker::Mortar) };
        match Pattern::parse(&template.expand_pattern(sample)) {
            Ok(_) => Some(template),
            Err(message) => {
                self.error(line, message);
                None
            }
        }
    }

    /// A `#@` line; `text` is what follows `#@`.
    #[allow(clippy::too_many_lines)] // One arm per directive.
    fn directive(&mut self, line: usize, text: &str) {
        let Some(text) = text.strip_prefix(' ') else {
            return self.error(line, "a directive is '#@ <key>: <value>'");
        };
        let Keyed { key, qualified, qualifiers, rest } = match keyed(text) {
            Ok(parts) => parts,
            Err(message) => return self.error(line, format!("a directive: {message}")),
        };
        let Some(value) = rest.strip_prefix(':') else {
            return self.error(line, format!("the directive '{key}' has no ':' after its key"));
        };
        let value = value.trim();
        if matches!(key, "xfail" | "skip") {
            let kind = if key == "xfail" { MarkerKind::Xfail } else { MarkerKind::Skip };
            if let Err(message) = qualifiers.no_family(&format!("'{key}'")) {
                return self.error(line, message);
            }
            let scope = qualifiers.scope;
            if scope.linkers.is_empty() {
                return self.error(
                    line,
                    format!("'{key}' names the linkers it is for: '{key}(mortar): <reason>'"),
                );
            }
            if value.is_empty() {
                return self.error(line, format!("'{key}' needs a reason"));
            }
            self.markers.push(Marker { line, kind, scope, reason: value.to_owned() });
            return;
        }
        if key == "anomaly" {
            if let Err(message) = qualifiers.no_family("'anomaly'") {
                return self.error(line, message);
            }
            let scope = qualifiers.scope;
            if scope.linkers.is_empty() || scope.linkers.contains(&Linker::Mortar) {
                return self.error(
                    line,
                    "'anomaly' names the reference linkers it is for: 'anomaly(mold): A9'",
                );
            }
            let ids: Vec<String> = value.split_whitespace().map(str::to_owned).collect();
            let valid = |id: &String| {
                id.strip_prefix('A')
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            };
            if ids.is_empty() || !ids.iter().all(valid) {
                return self.error(
                    line,
                    format!("'anomaly: {value}' is not a list of plumb.md anomaly ids ('A9 A11')"),
                );
            }
            self.anomalies.push(Anomaly { line, scope, ids });
            return;
        }
        if qualified {
            return self.error(line, format!("the directive '{key}' takes no qualifiers"));
        }
        if value.is_empty() {
            return self.error(line, format!("the directive '{key}' has no value"));
        }
        let again = |what: &str| format!("a second '{what}' directive");
        match key {
            "spec" => {
                if valid_spec(value) {
                    self.spec.push(value.to_owned());
                } else {
                    self.error(
                        line,
                        format!(
                            "'spec: {value}' does not name a spec section: expected a file of \
                             spec/ and a section, as in 'resolve.md \"Archives\"'"
                        ),
                    );
                }
            }
            "arch" => {
                let mut arches = Vec::new();
                for word in value.split_whitespace() {
                    match Arch::parse(word) {
                        Some(arch) if arches.contains(&arch) => {
                            self.error(line, format!("the architecture '{word}' is given twice"));
                        }
                        Some(arch) => arches.push(arch),
                        None => self.error(
                            line,
                            format!("unknown architecture '{word}' (aarch64, x86_64 or i386)"),
                        ),
                    }
                }
                arches.sort_unstable();
                if self.arches.replace((line, arches)).is_some() {
                    self.error(line, again("arch"));
                }
            }
            "requires" => {
                let mut features = Vec::new();
                for word in value.split_whitespace() {
                    match Feature::parse(word) {
                        Some(feature) if features.contains(&feature) => {
                            self.error(line, format!("'{word}' is required twice"));
                        }
                        Some(feature) => features.push(feature),
                        None => {
                            let known: Vec<&str> =
                                Feature::ALL.iter().map(|feature| feature.name()).collect();
                            self.error(
                                line,
                                format!(
                                    "unknown requirement '{word}' (known: {})",
                                    known.join(", ")
                                ),
                            );
                        }
                    }
                }
                if self.requires.replace((line, features)).is_some() {
                    self.error(line, again("requires"));
                }
            }
            "timeout" => match value.parse::<u64>() {
                Ok(seconds) if (1..=MAX_TIMEOUT).contains(&seconds) => {
                    if self.timeout.replace(Duration::from_secs(seconds)).is_some() {
                        self.error(line, again("timeout"));
                    }
                }
                _ => self.error(
                    line,
                    format!(
                        "'timeout: {value}' is not a number of seconds from 1 to {MAX_TIMEOUT}"
                    ),
                ),
            },
            "mem" => match value.parse::<u64>() {
                Ok(mib) if MEMORY_RANGE.contains(&mib) => {
                    if self.memory_mib.replace(mib).is_some() {
                        self.error(line, again("mem"));
                    }
                }
                _ => self.error(
                    line,
                    format!(
                        "'mem: {value}' is not a number of MiB from {} to {}",
                        MEMORY_RANGE.start(),
                        MEMORY_RANGE.end()
                    ),
                ),
            },
            _ => self.error(line, format!("unknown directive '{key}'")),
        }
    }

    /// A step line.
    fn step(&mut self, line: usize, text: &str) {
        self.exit_seen = false;
        // A step that does not parse still takes its expectations.
        let before = self.steps.len();
        if let Err(message) = self.try_step(line, text) {
            self.error(line, message);
        }
        if self.steps.len() == before {
            self.steps.push(Step {
                line,
                keyword: "",
                scope: Scope::default(),
                kind: StepKind::Ar(Vec::new()),
            });
        }
    }

    fn try_step(&mut self, line: usize, text: &str) -> Result<(), String> {
        let Keyed { key, qualifiers, rest, .. } = keyed(text)?;
        if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
            return Err(format!("unexpected '{rest}' after the step '{key}'"));
        }
        let raw = words(rest)?;
        let mut args = Vec::with_capacity(raw.len());
        for word in &raw {
            args.push(Template::parse(word)?);
        }
        let first = raw.first().map_or("", String::as_str);
        let (keyword, kind) = match key {
            "cc" => ("cc", compile("cc", Language::C, &qualifiers, args)?),
            "cxx" => ("cxx", compile("cxx", Language::Cxx, &qualifiers, args)?),
            "fc" => ("fc", compile("fc", Language::Fortran, &qualifiers, args)?),
            "as" => ("as", compile("as", Language::Asm, &qualifiers, args)?),
            "yaml2obj" | "ar" => {
                let keyword = if key == "ar" { "ar" } else { "yaml2obj" };
                let what = format!("'{keyword}'");
                qualifiers.no_family(&what)?;
                qualifiers.no_linker(&what)?;
                if args.is_empty() {
                    return Err(format!("{what} has no arguments"));
                }
                (keyword, if key == "ar" { StepKind::Ar(args) } else { StepKind::Yaml2obj(args) })
            }
            "link" => {
                qualifiers.no_family("'link'")?;
                let Some(driver) = Driver::ALL.into_iter().find(|driver| driver.name() == first)
                else {
                    let known: Vec<&str> = Driver::ALL.iter().map(|driver| driver.name()).collect();
                    return Err(format!(
                        "'link' is followed by what the link line is handed to ({}), not \
                         '{first}'",
                        known.join(", ")
                    ));
                };
                args.remove(0);
                ("link", StepKind::Link(Link { driver, args, fails: false, diags: Vec::new() }))
            }
            "run" => {
                qualifiers.no_family("'run'")?;
                if args.is_empty() {
                    return Err("'run' is followed by the program".to_owned());
                }
                let program = args.remove(0);
                let stdout = Stdout::Exact(Vec::new());
                ("run", StepKind::Run(Run { program, args, exit: Exit::Status(0), stdout }))
            }
            "check" => {
                qualifiers.no_family("'check'")?;
                let Some(tool) = CheckTool::ALL.into_iter().find(|tool| tool.name() == first)
                else {
                    let known: Vec<&str> = CheckTool::ALL.iter().map(|tool| tool.name()).collect();
                    return Err(format!(
                        "'check' is followed by its tool ({}), not '{first}'",
                        known.join(", ")
                    ));
                };
                args.remove(0);
                if tool == CheckTool::Text && args.len() != 1 {
                    return Err("'check text' takes one file".to_owned());
                }
                if args.is_empty() {
                    return Err(format!("'check {first}' has no arguments"));
                }
                ("check", StepKind::Check(Check { tool, args, expectations: Vec::new() }))
            }
            _ => {
                return Err(format!(
                    "unknown step '{key}' (cc, cxx, fc, as, yaml2obj, ar, link, run, check)"
                ));
            }
        };
        self.steps.push(Step { line, keyword, scope: qualifiers.scope, kind });
        Ok(())
    }

    /// An indented line under a step.
    fn expectation(&mut self, line: usize, text: &str) {
        if let Err(message) = self.try_expectation(line, text) {
            self.error(line, message);
        }
    }

    fn try_expectation(&mut self, line: usize, text: &str) -> Result<(), String> {
        let Keyed { key, qualified, qualifiers, rest } = keyed(text)?;
        let Some(value) = rest.strip_prefix(':') else {
            return Err(format!("the expectation '{key}' has no ':' after its key"));
        };
        qualifiers.no_family(&format!("'{key}'"))?;
        let scope = qualifiers.scope;
        let unqualified = |what: &str| {
            if qualified { Err(format!("'{what}' takes no qualifiers")) } else { Ok(()) }
        };
        if key == "exit" {
            if self.exit_seen {
                return Err("a second 'exit' for this step".to_owned());
            }
            self.exit_seen = true;
        }
        let Some(step) = self.steps.pop() else {
            return Err("an expectation without a step before it".to_owned());
        };
        let Step { line: at, keyword, scope: step_scope, mut kind } = step;
        let result = match (&mut kind, key) {
            _ if keyword.is_empty() => Ok(()),
            (StepKind::Link(linking), "exit") => unqualified("exit").and_then(|()| {
                match value.trim() {
                    "0" => linking.fails = false,
                    "fail" => linking.fails = true,
                    other => {
                        return Err(format!(
                            "'exit: {other}' of a link is not '0' (it succeeds) or 'fail'"
                        ));
                    }
                }
                Ok(())
            }),
            (StepKind::Link(linking), "diag" | "no-diag") => {
                if let Some(pattern) = self.pattern(line, value) {
                    linking.diags.push(Expectation {
                        line,
                        scope,
                        present: key == "diag",
                        pattern,
                    });
                }
                Ok(())
            }
            (StepKind::Check(check), "has" | "not") => {
                if let Some(pattern) = self.pattern(line, value) {
                    let present = key == "has";
                    check.expectations.push(Expectation { line, scope, present, pattern });
                }
                Ok(())
            }
            (StepKind::Run(run), "exit") => unqualified("exit").and_then(|()| {
                run.exit = parse_exit(value.trim())?;
                Ok(())
            }),
            (StepKind::Run(run), "stdout") => unqualified("stdout").and_then(|()| {
                // One blank after the colon is not part of the line.
                let text = value.strip_prefix(' ').unwrap_or(value);
                match &mut run.stdout {
                    Stdout::Exact(lines) => {
                        lines.extend(self.template(line, text));
                        Ok(())
                    }
                    Stdout::Reference => Err(both_outputs()),
                }
            }),
            (StepKind::Run(run), "output") => unqualified("output").and_then(|()| {
                if value.trim() != "reference" {
                    return Err(format!("'output: {}' is not 'reference'", value.trim()));
                }
                match &run.stdout {
                    Stdout::Exact(lines) if lines.is_empty() => {
                        run.stdout = Stdout::Reference;
                        Ok(())
                    }
                    Stdout::Exact(_) => Err(both_outputs()),
                    Stdout::Reference => Err("a second 'output' for this step".to_owned()),
                }
            }),
            (StepKind::Link(_), _) => {
                Err(format!("'{key}' is not an expectation of a link (exit, diag, no-diag)"))
            }
            (StepKind::Run(_), _) => {
                Err(format!("'{key}' is not an expectation of a run (exit, stdout, output)"))
            }
            (StepKind::Check(_), _) => {
                Err(format!("'{key}' is not an expectation of a check (has, not)"))
            }
            (StepKind::Compile(_) | StepKind::Yaml2obj(_) | StepKind::Ar(_), _) => {
                Err(format!("'{keyword}' builds an input and has no expectations"))
            }
        };
        self.steps.push(Step { line: at, keyword, scope: step_scope, kind });
        result
    }

    /// What can only be said of the whole file.
    fn finish(mut self) -> Result<Test, Vec<Error>> {
        if self.spec.is_empty() {
            self.error(1, "no 'spec' directive: a test names the spec section it checks");
        }
        let arches = match self.arches.take() {
            Some((_, arches)) => arches,
            None => Arch::ALL.to_vec(),
        };
        // The placeholders of steps that did not parse have said so.
        self.steps.retain(|step| !step.keyword.is_empty());
        let steps = std::mem::take(&mut self.steps);
        if !steps.iter().any(|step| matches!(step.kind, StepKind::Link(_))) {
            self.error(1, "no 'link' step: a link test links");
        }
        let mut linked = false;
        for step in &steps {
            linked |= !step.is_build();
            if linked && step.is_build() {
                self.error(
                    step.line,
                    format!(
                        "'{}' builds an input after a link, run or check step: the inputs are \
                         built once, before any linker runs",
                        step.keyword
                    ),
                );
            }
        }
        self.check_scopes(&steps, &arches);
        self.check_requires(&steps);
        self.check_steps(&steps, &arches);
        self.check_markers(&arches);
        let mut names: Vec<&str> = Vec::new();
        for file in &self.files {
            if names.contains(&file.name.as_str()) {
                self.errors.push(Error {
                    line: 1,
                    message: format!("the file '{}' is embedded twice", file.name),
                });
            }
            names.push(&file.name);
        }
        if !self.errors.is_empty() {
            self.errors.sort_by_key(|error| error.line);
            return Err(self.errors);
        }
        Ok(Test {
            spec: self.spec,
            arches,
            requires: self.requires.map(|(_, features)| features).unwrap_or_default(),
            markers: self.markers,
            anomalies: self.anomalies,
            timeout: self.timeout,
            memory_mib: self.memory_mib,
            steps,
            files: self.files,
        })
    }

    /// Every scope names only architectures the test is for.
    fn check_scopes(&mut self, steps: &[Step], arches: &[Arch]) {
        let mut scopes: Vec<(usize, &Scope)> = Vec::new();
        for step in steps {
            scopes.push((step.line, &step.scope));
            let expectations = match &step.kind {
                StepKind::Link(link) => link.diags.as_slice(),
                StepKind::Check(check) => check.expectations.as_slice(),
                _ => &[],
            };
            scopes.extend(expectations.iter().map(|expect| (expect.line, &expect.scope)));
        }
        let markers = self.markers.clone();
        scopes.extend(markers.iter().map(|marker| (marker.line, &marker.scope)));
        let anomalies = self.anomalies.clone();
        scopes.extend(anomalies.iter().map(|anomaly| (anomaly.line, &anomaly.scope)));
        for (line, scope) in scopes {
            if let Some(arch) = scope.arches.iter().find(|arch| !arches.contains(arch)) {
                self.error(
                    line,
                    format!(
                        "'{}' is not an architecture of this test (its 'arch' directive)",
                        arch.name()
                    ),
                );
            }
        }
    }

    /// `requires` names exactly the tools the steps need.
    fn check_requires(&mut self, steps: &[Step]) {
        let (at, required) = self.requires.clone().unwrap_or((1, Vec::new()));
        let mut used = Vec::new();
        for step in steps {
            let Some(feature) = step.feature() else { continue };
            used.push(feature);
            if !required.contains(&feature) {
                self.error(
                    step.line,
                    format!(
                        "this step needs '{0}', which the test does not ask for: add \
                         '#@ requires: {0}'",
                        feature.name()
                    ),
                );
            }
        }
        for feature in required {
            if !used.contains(&feature) {
                self.error(at, format!("'requires: {}' but no step uses it", feature.name()));
            }
        }
    }

    /// The rules of single steps that need the whole step.
    fn check_steps(&mut self, steps: &[Step], arches: &[Arch]) {
        for step in steps {
            let uses_linker = |args: &[Template]| args.iter().any(Template::uses_linker);
            match &step.kind {
                StepKind::Compile(Compile { args, .. })
                | StepKind::Yaml2obj(args)
                | StepKind::Ar(args) => {
                    if uses_linker(args) {
                        self.error(
                            step.line,
                            "'%{linker}' in a step that builds an input: it runs before any \
                             linker",
                        );
                    }
                }
                StepKind::Check(check) if check.expectations.is_empty() => {
                    self.error(step.line, "a check without a 'has' or a 'not' checks nothing");
                }
                StepKind::Link(link) if link.fails => {
                    // Wording differs between linkers; mortar's is the
                    // one the test is about.
                    let missing = arches.iter().find(|arch| {
                        step.scope.applies(**arch, Linker::Mortar)
                            && !link.diags.iter().any(|diag| {
                                diag.present && diag.scope.applies(**arch, Linker::Mortar)
                            })
                    });
                    if let Some(arch) = missing {
                        self.error(
                            step.line,
                            format!(
                                "a link that is expected to fail says what mortar must report: \
                                 no 'diag' applies to mortar on {}",
                                arch.name()
                            ),
                        );
                    }
                }
                StepKind::Link(_) | StepKind::Run(_) | StepKind::Check(_) => {}
            }
        }
    }

    /// At most one marker per linker and architecture, and at least one
    /// reference linker without any on each architecture.
    fn check_markers(&mut self, arches: &[Arch]) {
        let markers = self.markers.clone();
        for arch in arches {
            for linker in Linker::ALL {
                let mut applying =
                    markers.iter().filter(|marker| marker.scope.applies(*arch, linker));
                if let (Some(first), Some(second)) = (applying.next(), applying.next()) {
                    self.error(
                        second.line,
                        format!(
                            "a second 'xfail' or 'skip' for {} on {} (the first is on line {})",
                            linker.name(),
                            arch.name(),
                            first.line
                        ),
                    );
                }
            }
            let free = Linker::REFERENCES
                .into_iter()
                .any(|linker| !markers.iter().any(|marker| marker.scope.applies(*arch, linker)));
            if !free && let Some(last) = markers.last() {
                self.error(
                    last.line,
                    format!(
                        "every reference linker is marked 'xfail' or 'skip' on {}: a test must \
                         pass with at least one reference",
                        arch.name()
                    ),
                );
            }
        }
    }
}

/// A compile step's kind: `keyword` with its family and arguments.
fn compile(
    keyword: &str,
    language: Language,
    qualifiers: &Qualifiers,
    args: Vec<Template>,
) -> Result<StepKind, String> {
    let family = match qualifiers.families.as_slice() {
        [] => Family::Gcc,
        [family] => *family,
        _ => return Err(format!("'{keyword}' takes one compiler family")),
    };
    if language == Language::Fortran && family == Family::Clang {
        return Err("'fc' has no clang family: flang is not a pinned tool".to_owned());
    }
    qualifiers.no_linker(&format!("'{keyword}'"))?;
    if args.is_empty() {
        return Err(format!("'{keyword}' has no arguments"));
    }
    Ok(StepKind::Compile(Compile { language, family, args }))
}

fn both_outputs() -> String {
    "a run has 'stdout' lines or 'output: reference', not both".to_owned()
}

/// `N` (0 to 255) or `signal N` (1 to 64).
fn parse_exit(value: &str) -> Result<Exit, String> {
    let number = |text: &str| text.parse::<i32>().ok().filter(|_| !text.starts_with('+'));
    let exit = match value.strip_prefix("signal") {
        Some(signal) if signal.starts_with([' ', '\t']) => {
            number(signal.trim()).filter(|n| (1..=64).contains(n)).map(Exit::Signal)
        }
        Some(_) => None,
        None => number(value).filter(|n| (0..=255).contains(n)).map(Exit::Status),
    };
    exit.ok_or_else(|| {
        format!("'exit: {value}' of a run is not a status from 0 to 255 or 'signal N' (1 to 64)")
    })
}

/// Parses a test file.
///
/// # Errors
/// Every error of the file, in line order.
pub fn parse(text: &str) -> Result<Test, Vec<Error>> {
    let mut parser = Parser::default();
    // The file being read: its name, the line of its marker, its text.
    let mut file: Option<(String, usize, String)> = None;
    let mut in_files = false;
    let mut stepped = false;
    for (index, full) in text.split_inclusive('\n').enumerate() {
        let number = index + 1;
        let line = full.strip_suffix('\n').unwrap_or(full);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(name) = line.strip_prefix("#---") {
            if let Some((name, _, contents)) = file.take() {
                parser.files.push(Embedded { name, contents });
            }
            in_files = true;
            let name = name.strip_prefix(' ').unwrap_or("").trim_end();
            if valid_file_name(name) {
                file = Some((name.to_owned(), number, String::new()));
            } else {
                parser.error(
                    number,
                    format!(
                        "'{line}' does not start a file: expected '#--- <name>', the name a \
                         relative path of letters, digits, '.', '_', '+' and '-'"
                    ),
                );
                // What follows is still a file's text, not steps.
                file = Some((String::new(), number, String::new()));
            }
        } else if in_files {
            if let Some((_, _, contents)) = &mut file {
                contents.push_str(full);
            }
        } else if let Some(directive) = line.strip_prefix("#@") {
            if stepped {
                parser.error(number, "the directives come before the first step");
            } else {
                parser.directive(number, directive);
            }
        } else if line.starts_with('#') || line.trim().is_empty() {
            // A comment or an empty line.
        } else if line.starts_with([' ', '\t']) {
            parser.expectation(number, line.trim_start());
        } else {
            stepped = true;
            parser.step(number, line.trim_end());
        }
    }
    if let Some((name, _, contents)) = file.take() {
        parser.files.push(Embedded { name, contents });
    }
    // A marker that named no file has said so.
    parser.files.retain(|file| !file.name.is_empty());
    parser.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The errors of `text`, as `line: message` strings.
    fn errors(text: &str) -> Vec<String> {
        match parse(text) {
            Ok(parsed) => panic!("parsed: {parsed:?}"),
            Err(errors) => errors.iter().map(ToString::to_string).collect(),
        }
    }

    /// Whether some error of `text` is on `line` and contains `part`.
    fn has_error(text: &str, line: usize, part: &str) -> bool {
        let all = errors(text);
        let found =
            all.iter().any(|e| e.starts_with(&format!("line {line}: ")) && e.contains(part));
        assert!(found, "no error on line {line} with '{part}' in {all:#?}");
        found
    }

    const MINIMAL: &str = "#@ spec: layout.md \"Entry point\"\nlink ld a.o\n";

    // spec/harness.md "Link tests", "A test file": directives, steps with
    // their indented expectations, embedded files.
    #[test]
    fn a_whole_test() {
        let text = "\
# A comment.
#@ spec: resolve.md \"Archives\"
#@ spec: layout.md \"Entry point\"
#@ arch: i386 aarch64
#@ requires: g++ yaml2obj
#@ xfail(mortar): no linking yet (M1)
#@ skip(mold,i386): mold differs here
#@ timeout: 30
#@ mem: 2048

cc -O2 main.c -o main.o
cxx(i386) -m32 \"a b.cc\" -o ab.o
yaml2obj odd.yaml -o odd.o
ar rcs libx.a main.o
link g++ main.o -L. -lx -o prog
  exit: 0
  no-diag: warning
  diag(bfd): ^ld*: note$
run prog arg%%1 %{arch}
  exit: signal 6
  stdout: hello %{arch}
  stdout:
check(lld) nm prog
  has: T main
  not(aarch64): U foo
#--- main.c
int main(void) { return 0; }
#--- sub/odd.yaml
--- !ELF
FileHeader:
#--- last.txt
";
        let parsed = parse(text).unwrap();
        assert_eq!(parsed.spec, ["resolve.md \"Archives\"", "layout.md \"Entry point\""]);
        assert_eq!(parsed.arches, [Arch::Aarch64, Arch::I386]);
        assert_eq!(parsed.requires, [Feature::Gxx, Feature::Yaml2obj]);
        assert_eq!(parsed.timeout, Some(Duration::from_secs(30)));
        assert_eq!(parsed.memory_mib, Some(2048));
        assert_eq!(parsed.markers.len(), 2);
        let xfail = parsed.marker(Arch::I386, Linker::Mortar).unwrap();
        assert_eq!((xfail.kind, xfail.reason.as_str()), (MarkerKind::Xfail, "no linking yet (M1)"));
        assert_eq!(parsed.marker(Arch::I386, Linker::Mold).unwrap().kind, MarkerKind::Skip);
        assert!(parsed.marker(Arch::Aarch64, Linker::Mold).is_none());
        assert!(parsed.marker(Arch::Aarch64, Linker::Bfd).is_none());

        let keywords: Vec<&str> = parsed.steps.iter().map(|step| step.keyword).collect();
        assert_eq!(keywords, ["cc", "cxx", "yaml2obj", "ar", "link", "run", "check"]);
        let lines: Vec<usize> = parsed.steps.iter().map(|step| step.line).collect();
        assert_eq!(lines, [11, 12, 13, 14, 15, 19, 23]);
        let vars = Vars { arch: Arch::I386, host: Some(Arch::Aarch64), linker: Some(Linker::Lld) };
        let StepKind::Compile(cxx) = &parsed.steps[1].kind else { panic!() };
        assert_eq!((cxx.language, cxx.family), (Language::Cxx, Family::Gcc));
        assert_eq!(parsed.steps[1].scope.arches, [Arch::I386]);
        let args: Vec<String> = cxx.args.iter().map(|a| a.expand(vars)).collect();
        assert_eq!(args, ["-m32", "a b.cc", "-o", "ab.o"]);
        let StepKind::Link(link) = &parsed.steps[4].kind else { panic!() };
        assert_eq!(link.driver, Driver::Gxx);
        assert!(!link.fails);
        assert_eq!(link.diags.len(), 2);
        assert!(!link.diags[0].present && link.diags[1].present);
        assert_eq!(link.diags[1].scope.linkers, [Linker::Bfd]);
        let StepKind::Run(run) = &parsed.steps[5].kind else { panic!() };
        assert_eq!(run.exit, Exit::Signal(6));
        let args: Vec<String> = run.args.iter().map(|a| a.expand(vars)).collect();
        assert_eq!(args, ["arg%1", "i386"]);
        let Stdout::Exact(stdout) = &run.stdout else { panic!() };
        let stdout: Vec<String> = stdout.iter().map(|line| line.expand(vars)).collect();
        assert_eq!(stdout, ["hello i386", ""]);
        let StepKind::Check(check) = &parsed.steps[6].kind else { panic!() };
        assert_eq!(check.tool, CheckTool::Nm);
        assert_eq!(parsed.steps[6].scope.linkers, [Linker::Lld]);
        assert_eq!(check.expectations[1].scope.arches, [Arch::Aarch64]);

        let names: Vec<&str> = parsed.files.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(names, ["main.c", "sub/odd.yaml", "last.txt"]);
        assert_eq!(parsed.files[0].contents, "int main(void) { return 0; }\n");
        assert_eq!(parsed.files[1].contents, "--- !ELF\nFileHeader:\n", "'---' is a file's text");
        assert_eq!(parsed.files[2].contents, "");
        assert!(!parsed.compares_output());
    }

    // "Directives": the defaults, and the forms of each directive.
    #[test]
    fn defaults_and_directive_forms() {
        let test = parse(MINIMAL).unwrap();
        assert_eq!(test.arches, Arch::ALL);
        assert!(test.requires.is_empty() && test.markers.is_empty() && test.files.is_empty());
        assert_eq!((test.timeout, test.memory_mib), (None, None));

        let with =
            |directive: &str| format!("#@ spec: layout.md \"x\"\n{directive}\nlink ld a.o\n");
        assert!(has_error(&with("#@spec: a.md b"), 2, "'#@ <key>: <value>'"));
        assert!(has_error(&with("#@ arch aarch64"), 2, "no ':'"));
        assert!(has_error(&with("#@ arch:"), 2, "has no value"));
        assert!(has_error(&with("#@ color: red"), 2, "unknown directive 'color'"));
        assert!(has_error(&with("#@ arch: arm64"), 2, "unknown architecture 'arm64'"));
        assert!(has_error(&with("#@ arch: i386 i386"), 2, "given twice"));
        assert!(has_error(&with("#@ arch: i386\n#@ arch: i386"), 3, "a second 'arch'"));
        assert!(has_error(&with("#@ requires: flang"), 2, "unknown requirement 'flang'"));
        assert!(has_error(&with("#@ timeout: 0"), 2, "from 1 to 3600"));
        assert!(has_error(&with("#@ timeout: 3601"), 2, "from 1 to 3600"));
        assert!(has_error(&with("#@ mem: 8"), 2, "from 16 to 65536"));
        assert!(has_error(&with("#@ timeout(bfd): 3"), 2, "takes no qualifiers"));
        assert!(has_error(&with("#@ spec: layout"), 2, "does not name a spec section"));
        assert!(has_error(&with("#@ spec: Layout.md x"), 2, "does not name a spec section"));
        assert!(has_error(&with("#@ spec: layout.md"), 2, "does not name a spec section"));
        // A directive after a step.
        let late = "#@ spec: layout.md \"x\"\nlink ld a.o\n#@ arch: i386\n";
        assert!(has_error(late, 3, "before the first step"));
        // A file needs a spec and a link.
        assert!(has_error("link ld a.o\n", 1, "no 'spec' directive"));
        assert!(has_error("#@ spec: layout.md \"x\"\n", 1, "no 'link' step"));
    }

    // "Markers": `xfail` and `skip` name linkers and a reason; at most one
    // applies per linker and architecture; a reference stays unmarked on
    // every architecture.
    #[test]
    fn markers() {
        let with = |directives: &str| {
            format!("#@ spec: layout.md \"x\"\n#@ arch: aarch64 i386\n{directives}link ld a.o\n")
        };
        assert!(has_error(&with("#@ xfail: why\n"), 3, "names the linkers it is for"));
        assert!(has_error(&with("#@ xfail(i386): why\n"), 3, "names the linkers it is for"));
        assert!(has_error(&with("#@ xfail(mortar):\n"), 3, "needs a reason"));
        assert!(has_error(&with("#@ skip(gcc,bfd): why\n"), 3, "takes no compiler family"));
        assert!(has_error(&with("#@ xfail(gold): why\n"), 3, "unknown qualifier 'gold'"));
        assert!(has_error(&with("#@ xfail(bfd,bfd): why\n"), 3, "given twice"));
        assert!(has_error(&with("#@ xfail(bfd,): why\n"), 3, "an empty qualifier"));
        assert!(has_error(&with("#@ xfail(bfd: why\n"), 3, "without ')'"));
        assert!(has_error(&with("#@ xfail(x86_64,lld): why\n"), 3, "not an architecture of this"));
        let twice = "#@ xfail(mortar): a\n#@ skip(mortar,i386): b\n";
        assert!(has_error(&with(twice), 4, "a second 'xfail' or 'skip' for mortar on i386"));
        let all = "#@ xfail(bfd): a\n#@ skip(lld): b\n#@ xfail(mold,i386): c\n";
        assert!(has_error(&with(all), 5, "every reference linker is marked"));
        // Marked on different architectures: each keeps a reference.
        let split = "#@ xfail(bfd,lld,i386): a\n#@ xfail(mold,aarch64): c\n";
        assert!(parse(&with(split)).is_ok());
    }

    // "Directives", "L3": `anomaly` names reference linkers and anomaly
    // ids of plumb.md §10.2.
    #[test]
    fn anomaly_declarations() {
        let with = |directives: &str| {
            format!("#@ spec: layout.md \"x\"\n#@ arch: aarch64 i386\n{directives}link ld a.o\n")
        };
        let test =
            parse(&with("#@ anomaly(mold): A9 A11\n#@ anomaly(bfd,aarch64): A10\n")).unwrap();
        assert_eq!(test.anomalies.len(), 2);
        assert_eq!(test.anomalies[0].ids, ["A9", "A11"]);
        assert_eq!(test.anomalies[1].scope.arches, [Arch::Aarch64]);
        assert!(has_error(&with("#@ anomaly: A9\n"), 3, "names the reference linkers"));
        assert!(has_error(&with("#@ anomaly(mortar): A9\n"), 3, "names the reference linkers"));
        assert!(has_error(&with("#@ anomaly(mold): 9\n"), 3, "not a list of plumb.md anomaly"));
        assert!(has_error(&with("#@ anomaly(mold): A\n"), 3, "not a list of plumb.md anomaly"));
        assert!(has_error(&with("#@ anomaly(mold,x86_64): A9\n"), 3, "not an architecture of"));
    }

    // "Steps": the keywords, their qualifiers and their arguments.
    #[test]
    fn steps() {
        let with = |steps: &str| format!("#@ spec: layout.md \"x\"\n{steps}");
        let ok = |steps: &str| parse(&with(steps)).unwrap();
        let clang = ok("#@ requires: clang\ncc(clang) a.c\nlink clang++ a.o\n");
        let StepKind::Compile(compile) = &clang.steps[0].kind else { panic!() };
        assert_eq!(compile.family, Family::Clang);
        let StepKind::Link(link) = &clang.steps[1].kind else { panic!() };
        assert_eq!(link.driver, Driver::Clangxx);
        let fortran = ok("#@ requires: gfortran\nfc a.f90\nlink gfortran a.o\n");
        assert_eq!(fortran.requires, [Feature::Gfortran]);
        let asm = ok("as a.S -o a.o\nlink gcc -static a.o\n");
        let StepKind::Compile(compile) = &asm.steps[0].kind else { panic!() };
        assert_eq!(compile.language, Language::Asm);
        let checked = ok("link ld a.o\ncheck text a.map\n  has: .text\n");
        let StepKind::Check(check) = &checked.steps[1].kind else { panic!() };
        assert_eq!(check.tool, CheckTool::Text);

        assert!(has_error(&with("compile a.c\nlink ld a.o\n"), 2, "unknown step 'compile'"));
        assert!(has_error(&with("cc\nlink ld a.o\n"), 2, "'cc' has no arguments"));
        assert!(has_error(&with("cc(bfd) a.c\nlink ld a.o\n"), 2, "cannot be for 'bfd' only"));
        assert!(has_error(&with("cc(gcc,clang) a.c\nlink ld a.o\n"), 2, "one compiler family"));
        assert!(has_error(&with("fc(clang) a.f\nlink ld a.o\n"), 2, "flang"));
        assert!(has_error(&with("link(gcc) ld a.o\n"), 2, "takes no compiler family"));
        assert!(has_error(&with("link cc a.o\n"), 2, "not 'cc'"));
        assert!(has_error(&with("link ld a.o\nrun\n"), 3, "followed by the program"));
        assert!(has_error(&with("link ld a.o\ncheck ldd a\n  has: x\n"), 3, "not 'ldd'"));
        assert!(has_error(&with("link ld a.o\ncheck nm\n  has: x\n"), 3, "has no arguments"));
        assert!(has_error(&with("link ld a.o\ncheck text a b\n  has: x\n"), 3, "one file"));
        assert!(has_error(&with("link ld a.o\ncheck nm a\n"), 3, "checks nothing"));
        assert!(has_error(&with("linkx ld a.o\n"), 2, "unknown step 'linkx'"));
        assert!(has_error(&with("link: ld a.o\n"), 2, "unexpected ': ld a.o'"));
        assert!(has_error(&with("link ld \"a.o\n"), 2, "not closed"));
        assert!(has_error(&with("link ld \"a\\n\"\n"), 2, "a backslash goes before"));
        // Inputs are built before any linker runs.
        assert!(has_error(&with("link ld a.o\ncc a.c\n"), 3, "after a link"));
        // `%{linker}` is not known while inputs are built.
        assert!(has_error(&with("cc %{linker}.c\nlink ld a.o\n"), 2, "'%{linker}' in a step"));
        assert!(has_error(&with("cc %{os}.c\nlink ld a.o\n"), 2, "unknown substitution '%{os}'"));
        assert!(has_error(&with("cc %{arch.c\nlink ld a.o\n"), 2, "without '}'"));
    }

    // "Directives", `requires`: exactly the tools the steps use beyond
    // gcc, binutils and the linkers.
    #[test]
    fn requires_names_exactly_the_tools_used() {
        let with = |text: &str| format!("#@ spec: layout.md \"x\"\n{text}");
        assert!(has_error(&with("cxx a.cc\nlink ld a.o\n"), 2, "add '#@ requires: g++'"));
        assert!(has_error(&with("link g++ a.o\n"), 2, "needs 'g++'"));
        assert!(has_error(&with("cc(clang) a.c\nlink ld a.o\n"), 2, "needs 'clang'"));
        assert!(has_error(&with("yaml2obj a.yaml\nlink ld a.o\n"), 2, "needs 'yaml2obj'"));
        let readelf = "link ld a.o\ncheck llvm-readelf -h a.out\n  has: ELF\n";
        assert!(has_error(&with(readelf), 3, "needs 'llvm-readelf'"));
        assert!(has_error(&with("#@ requires: g++\nlink ld a.o\n"), 2, "no step uses it"));
        assert!(has_error(&with("#@ requires: g++ g++\nlink g++ a.o\n"), 2, "required twice"));
        assert!(
            parse(
                &with("#@ requires: llvm-readelf\n".to_owned().as_str())
                    .replace("llvm-readelf\n", &format!("llvm-readelf\n{readelf}"))
            )
            .is_ok()
        );
    }

    // "Expectations": which a step takes, and their values.
    #[test]
    fn expectations() {
        let with = |text: &str| format!("#@ spec: layout.md \"x\"\n{text}");
        let test = parse(&with(
            "link ld a.o -o p\n  exit: fail\n  diag: undefined*foo\nrun p\n  exit: 255\n",
        ))
        .unwrap();
        let StepKind::Link(link) = &test.steps[0].kind else { panic!() };
        assert!(link.fails);
        let StepKind::Run(run) = &test.steps[1].kind else { panic!() };
        assert_eq!(run.exit, Exit::Status(255));
        let reference = parse(&with("link ld a.o\nrun a.out\n  output: reference\n")).unwrap();
        assert!(reference.compares_output());

        assert!(has_error(&with("  exit: 0\nlink ld a.o\n"), 2, "without a step"));
        assert!(has_error(&with("link ld a.o\n  exit: 1\n"), 3, "not '0'"));
        assert!(has_error(&with("link ld a.o\n  exit: 0\n  exit: 0\n"), 4, "a second 'exit'"));
        assert!(has_error(&with("link ld a.o\n  exit(bfd): 0\n"), 3, "takes no qualifiers"));
        assert!(has_error(&with("link ld a.o\n  has: x\n"), 3, "not an expectation of a link"));
        assert!(has_error(&with("link ld a.o\n  diag: a\\b\n"), 3, "a backslash goes before"));
        assert!(has_error(&with("link ld a.o\n  diag:\n"), 3, "an empty pattern"));
        assert!(has_error(&with("link ld a.o\n  diag x\n"), 3, "no ':'"));
        assert!(has_error(&with("link ld a.o\nrun p\n  exit: 256\n"), 4, "from 0 to 255"));
        assert!(has_error(&with("link ld a.o\nrun p\n  exit: signal 0\n"), 4, "1 to 64"));
        assert!(has_error(&with("link ld a.o\nrun p\n  exit: +1\n"), 4, "from 0 to 255"));
        assert!(has_error(&with("link ld a.o\nrun p\n  exit: signals 6\n"), 4, "from 0 to 255"));
        assert!(has_error(
            &with("link ld a.o\nrun p\n  diag: x\n"),
            4,
            "not an expectation of a run"
        ));
        assert!(has_error(&with("link ld a.o\nrun p\n  output: mine\n"), 4, "is not 'reference'"));
        let both = "link ld a.o\nrun p\n  stdout: a\n  output: reference\n";
        assert!(has_error(&with(both), 5, "not both"));
        let both = "link ld a.o\nrun p\n  output: reference\n  stdout: a\n";
        assert!(has_error(&with(both), 5, "not both"));
        assert!(has_error(&with("cc a.c\n  exit: 0\nlink ld a.o\n"), 3, "has no expectations"));
        assert!(has_error(&with("link ld a.o\ncheck nm a\n  diag: x\n"), 4, "of a check"));
        // An expectation of a step that did not parse is not reported again.
        assert_eq!(errors(&with("lnk ld a.o\n  exit: 0\nlink ld a.o\n")).len(), 1);
    }

    // "Expectations": a link expected to fail says what mortar reports, on
    // every architecture.
    #[test]
    fn a_failing_link_names_mortars_diagnostic() {
        let with = |text: &str| format!("#@ spec: layout.md \"x\"\n#@ arch: aarch64 i386\n{text}");
        assert!(has_error(&with("link ld a.o\n  exit: fail\n"), 3, "no 'diag' applies to mortar"));
        let others = "link ld a.o\n  exit: fail\n  diag(bfd): undefined\n  no-diag: x\n";
        assert!(has_error(&with(others), 3, "mortar on aarch64"));
        let one_arch = "link ld a.o\n  exit: fail\n  diag(mortar,aarch64): undefined\n";
        assert!(has_error(&with(one_arch), 3, "mortar on i386"));
        assert!(parse(&with("link ld a.o\n  exit: fail\n  diag(mortar): undefined\n")).is_ok());
        assert!(parse(&with("link ld a.o\n  exit: fail\n  diag: undefined\n")).is_ok());
        // A link that mortar does not run needs no diagnostic of mortar's.
        assert!(parse(&with("link(bfd) ld a.o\n  exit: fail\n")).is_ok());
    }

    // "Embedded files": names are relative paths of plain components; a
    // name is embedded once.
    #[test]
    fn embedded_files() {
        let with = |files: &str| format!("{MINIMAL}{files}");
        assert!(has_error(&with("#--- ../a.c\nx\n"), 3, "does not start a file"));
        assert!(has_error(&with("#--- /a.c\n"), 3, "does not start a file"));
        assert!(has_error(&with("#---a.c\n"), 3, "does not start a file"));
        assert!(has_error(&with("#--- a b.c\n"), 3, "does not start a file"));
        assert!(has_error(&with("#--- a.c\n#--- a.c\n"), 1, "embedded twice"));
        // After the first file, nothing is a step or a directive.
        let test = parse(&with("#--- a.c\n#@ spec: x\nlink ld b.o\n  exit: 0\n")).unwrap();
        assert_eq!(test.files[0].contents, "#@ spec: x\nlink ld b.o\n  exit: 0\n");
        assert_eq!(test.steps.len(), 1);
        // Line ends: a carriage return ends a marker line, and is kept in
        // a file's text.
        let test = parse(&with("#--- a.c\r\nx\r\n")).unwrap();
        assert_eq!(
            (test.files[0].name.as_str(), test.files[0].contents.as_str()),
            ("a.c", "x\r\n")
        );
    }

    // "Substitutions": every variable, `%%`, and a value inside a pattern
    // stands for itself.
    #[test]
    fn substitutions() {
        let host = Some(Arch::Aarch64);
        let vars = Vars { arch: Arch::X86_64, host, linker: Some(Linker::Mold) };
        let all: Vec<String> = Var::ALL.iter().map(|var| format!("%{{{}}}", var.name())).collect();
        let template = Template::parse(&all.join("|")).unwrap();
        assert_eq!(
            template.expand(vars),
            "x86_64|x86_64-linux-gnu|elf_x86_64|/lib64/ld-linux-x86-64.so.2|/usr/x86_64-linux-gnu|\
             /usr/x86_64-linux-gnu/lib|ELF64|Advanced Micro Devices X86-64|mold"
        );
        assert!(template.uses_linker());
        let native = Vars { arch: Arch::Aarch64, host, linker: None };
        assert_eq!(
            Template::parse("%{sysroot} %{libdir}").unwrap().expand(native),
            "/ /usr/lib/aarch64-linux-gnu"
        );
        assert_eq!(Template::parse("100%% %x %").unwrap().expand(native), "100% %x %");
        assert_eq!(Template::parse("plain").unwrap().literal(), Some("plain"));
        assert_eq!(Template::parse("%{arch}").unwrap().literal(), None);
        // A substituted value is literal in a pattern.
        let star = Template::parse("^%{arch}$").unwrap();
        assert_eq!(star.expand_pattern(native), "^aarch64$");
        assert_eq!(pattern::escape("a*b$"), "a\\*b\\$");
    }

    // "Words": blanks separate words, double quotes keep them.
    #[test]
    fn words_of_a_step() {
        assert_eq!(words("  a  b\tc ").unwrap(), ["a", "b", "c"]);
        assert_eq!(words("-Wl,\"x y\"z \"\" \"q\\\"\\\\\"").unwrap(), ["-Wl,x yz", "", "q\"\\"]);
        assert!(words("\"open").is_err());
    }
}
