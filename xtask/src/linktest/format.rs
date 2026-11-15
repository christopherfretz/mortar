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
    fn value(&self, var: Var) -> String {
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
    pub fn expand(&self, vars: &Vars) -> String {
        self.expand_with(vars, |value| value)
    }

    /// The text as a pattern: a variable's value stands for itself,
    /// whatever characters it has.
    pub fn expand_pattern(&self, vars: &Vars) -> String {
        self.expand_with(vars, |value| pattern::escape(&value))
    }

    fn expand_with(&self, vars: &Vars, value: impl Fn(String) -> String) -> String {
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
    const ALL: [Self; 5] =
        [Self::Readelf, Self::Nm, Self::Objdump, Self::LlvmReadelf, Self::Text];

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
        self.steps.iter().any(
            |step| matches!(&step.kind, StepKind::Run(run) if run.stdout == Stdout::Reference),
        )
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
        && name.split('/').all(|part| {
            !part.is_empty() && part != "." && part != ".." && part.chars().all(plain)
        })
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
        match Pattern::parse(&template.expand_pattern(&sample)) {
            Ok(_) => Some(template),
            Err(message) => {
                self.error(line, message);
                None
            }
        }
    }

    /// A `#@` line; `text` is what follows `#@`.
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
                            format!(
                                "unknown architecture '{word}' (aarch64, x86_64 or i386)"
                            ),
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
                    format!("'timeout: {value}' is not a number of seconds from 1 to {MAX_TIMEOUT}"),
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
            (StepKind::Link(link), "exit") => unqualified("exit").and_then(|()| {
                match value.trim() {
                    "0" => link.fails = false,
                    "fail" => link.fails = true,
                    other => {
                        return Err(format!(
                            "'exit: {other}' of a link is not '0' (it succeeds) or 'fail'"
                        ));
                    }
                }
                Ok(())
            }),
            (StepKind::Link(link), "diag" | "no-diag") => {
                if let Some(pattern) = self.pattern(line, value) {
                    link.diags.push(Expectation { line, scope, present: key == "diag", pattern });
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
            (StepKind::Link(_), _) => Err(format!(
                "'{key}' is not an expectation of a link (exit, diag, no-diag)"
            )),
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
                self.error(
                    at,
                    format!("'requires: {}' but no step uses it", feature.name()),
                );
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
