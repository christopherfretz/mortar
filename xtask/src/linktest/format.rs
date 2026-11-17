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
    /// A default for the architectures built with the cross packages only
    /// (the word `cross`): what the reference linker does to the cross C
    /// runtime, which is another build than the host's native one.
    pub cross: bool,
}

impl Anomaly {
    /// Whether it applies to `linker`'s outputs on `arch`, on a machine of
    /// architecture `host`.
    pub fn applies(&self, arch: Arch, linker: Linker, host: Option<Arch>) -> bool {
        self.scope.applies(arch, linker) && (!self.cross || host != Some(arch))
    }
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
    /// `llvm-dwarfdump` (WP-080: `--verify` on split-DWARF outputs).
    LlvmDwarfdump,
    /// The lines of a file the link wrote.
    Text,
    /// Two files the linker's steps wrote, compared byte for byte: one
    /// line, `identical`, or `differ: ...` (WP-112: an option's effect is
    /// the difference between a link with it and one without).
    Cmp,
    /// `eu-elflint --gnu-ld <file>`: its complaints, each allowed by an
    /// `allow` line or a failure.
    Elflint,
    /// The architecture's dynamic loader with `--list <program>` (what
    /// `ldd` runs), under the architecture's runner.
    Ldd,
    /// gdb in batch mode on a program, the arguments after the program
    /// being its commands.
    Gdb,
    /// lldb in batch mode, likewise.
    Lldb,
}

impl CheckTool {
    const ALL: [Self; 11] = [
        Self::Readelf,
        Self::Nm,
        Self::Objdump,
        Self::LlvmReadelf,
        Self::LlvmDwarfdump,
        Self::Text,
        Self::Cmp,
        Self::Elflint,
        Self::Ldd,
        Self::Gdb,
        Self::Lldb,
    ];

    /// The word after `check`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Readelf => "readelf",
            Self::Nm => "nm",
            Self::Objdump => "objdump",
            Self::LlvmReadelf => "llvm-readelf",
            Self::LlvmDwarfdump => "llvm-dwarfdump",
            Self::Text => "text",
            Self::Cmp => "cmp",
            Self::Elflint => "elflint",
            Self::Ldd => "ldd",
            Self::Gdb => "gdb",
            Self::Lldb => "lldb",
        }
    }

    /// Whether the tool runs the linked program (and so is given the run
    /// environment of "Runs").
    pub fn runs_program(self) -> bool {
        matches!(self, Self::Ldd | Self::Gdb | Self::Lldb)
    }
}

/// What an expectation asks of the lines of a text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// `has`, `diag` (some line matches) or, not `present`, `not`,
    /// `no-diag` (no line does).
    Line,
    /// `then`: some line after the line the previous `has` or `then` of
    /// the step matched.
    Then,
    /// `count: <n> <pattern>`: exactly `n` lines match.
    Count(u32),
    /// `allow` (of `check elflint`): a complaint that matches is allowed.
    Allow,
}

/// A `has`, `not`, `then`, `count`, `allow`, `diag` or `no-diag` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expectation {
    pub line: usize,
    pub scope: Scope,
    /// A line must match (`has`, `diag`), or none may (`not`, `no-diag`).
    pub present: bool,
    pub want: Want,
    pub pattern: Template,
}

/// A compile step: `cc`, `cxx`, `fc` or `as`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compile {
    pub language: Language,
    pub family: Family,
    pub args: Vec<Template>,
}

/// A `shlib` step: a fixture shared library, linked by the architecture's
/// compiler driver with `-shared` and the system's own linker (GNU ld), the
/// same for every linker of the matrix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shlib {
    /// `gcc`, `g++` or `gfortran`.
    pub driver: Driver,
    pub args: Vec<Template>,
}

/// A `partial <reference> <args>` step (WP-175): an input made by a
/// reference linker's partial link (`-r`, which the harness puts before the
/// test's arguments), the same for every linker of the matrix: the
/// relocatable outputs old build systems feed a final link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Partial {
    /// `bfd`, `lld` or `mold`.
    pub linker: Linker,
    pub args: Vec<Template>,
}

/// A link step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub driver: Driver,
    pub args: Vec<Template>,
    /// `exit: fail`: the link is expected to fail.
    pub fails: bool,
    /// `exit: info` (WP-112): the link only informs (`--version`,
    /// `--help`): it succeeds and writes no output, so it has no map and
    /// the verifier judges nothing.
    pub informs: bool,
    /// The `diag` and `no-diag` lines.
    pub diags: Vec<Expectation>,
    /// The `print` and `no-print` lines (WP-112): of its standard output.
    pub prints: Vec<Expectation>,
}

/// How a program is expected to end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Status(i32),
    Signal(i32),
    /// `exit: no-start`: the program cannot be started (its interpreter is
    /// not there): natively the shell's exec fails, status 127; under qemu
    /// the emulator cannot load it, status 255.
    NoStart,
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
    /// The `env: NAME=value` lines: the program's environment beyond the
    /// harness's (`LD_LIBRARY_PATH` is the linker's directory unless one
    /// of these sets it).
    pub env: Vec<(String, Template)>,
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
    /// `meta <description> <in.o> -o <out.o>`: the test producer
    /// (`spec/meta.md` §12) adds one container to an object.
    Meta(Vec<Template>),
    Shlib(Shlib),
    Partial(Partial),
    Link(Link),
    /// `host <driver> <args>` (WP-120, `spec/harness.md` LT-1): a program
    /// linked against the libraries of the unit's linkers, once per pair
    /// (library linker, host linker). Never `exit: fail` or `exit: info`.
    Host(Link),
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
        matches!(
            self.kind,
            StepKind::Compile(_)
                | StepKind::Yaml2obj(_)
                | StepKind::Ar(_)
                | StepKind::Meta(_)
                | StepKind::Shlib(_)
                | StepKind::Partial(_)
        )
    }

    /// The `requires` word the step's tool needs, if it is not a tool
    /// every test may use.
    pub fn feature(&self) -> Option<Feature> {
        match &self.kind {
            StepKind::Compile(compile) => match (compile.family, compile.language) {
                (Family::Clang, _) => Some(Feature::Clang),
                (Family::Kiln, _) => Some(Feature::Kiln),
                (Family::Hod, _) => Some(Feature::Hod),
                (Family::Gcc, Language::Cxx) => Some(Feature::Gxx),
                (Family::Gcc, Language::Fortran) => Some(Feature::Gfortran),
                (Family::Gcc, Language::C | Language::Asm) => None,
            },
            StepKind::Yaml2obj(_) => Some(Feature::Yaml2obj),
            StepKind::Shlib(shlib) => match shlib.driver {
                Driver::Gxx => Some(Feature::Gxx),
                Driver::Gfortran => Some(Feature::Gfortran),
                _ => None,
            },
            StepKind::Link(link) | StepKind::Host(link) => match link.driver {
                Driver::Gxx => Some(Feature::Gxx),
                Driver::Gfortran => Some(Feature::Gfortran),
                Driver::Clang | Driver::Clangxx => Some(Feature::Clang),
                Driver::Ld | Driver::Gcc => None,
            },
            StepKind::Check(check) => match check.tool {
                CheckTool::LlvmReadelf => Some(Feature::LlvmReadelf),
                CheckTool::LlvmDwarfdump => Some(Feature::LlvmDwarfdump),
                CheckTool::Gdb => Some(Feature::Gdb),
                CheckTool::Lldb => Some(Feature::Lldb),
                CheckTool::Elflint => Some(Feature::Elflint),
                CheckTool::Readelf
                | CheckTool::Nm
                | CheckTool::Objdump
                | CheckTool::Text
                | CheckTool::Cmp
                | CheckTool::Ldd => None,
            },
            // The linkers need no `requires`.
            StepKind::Ar(_) | StepKind::Meta(_) | StepKind::Partial(_) | StepKind::Run(_) => None,
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
    /// The `diag: <id> <file>...` directives: the one-definition-rule
    /// diagnostics every mortar link step reports (`spec/odr.md` §12.1).
    pub odr: Vec<super::odr::Expected>,
    /// The wall-clock limit of each link and run step, if the test sets
    /// one.
    pub timeout: Option<Duration>,
    /// The address-space limit of each link and run step in MiB, if the
    /// test sets one.
    pub memory_mib: Option<u64>,
    pub steps: Vec<Step>,
    pub files: Vec<Embedded>,
    /// The `fixture` directives: the shared fixtures (`tests/link/fixtures/
    /// <name>.fixture`) whose built files the unit's inputs start with.
    pub fixtures: Vec<String>,
}

/// A shared fixture (`spec/harness.md` "Fixtures"): build steps and
/// embedded files, built once per architecture in a run, whose files every
/// test that names it gets in its inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fixture {
    pub requires: Vec<Feature>,
    pub steps: Vec<Step>,
    pub files: Vec<Embedded>,
}

impl Fixture {
    /// Whether the fixture has `link` steps: a linked fixture
    /// (`spec/harness.md` LT-4), whose libraries each linker of the run
    /// links for itself; its name begins `shared-`.
    pub fn is_linked(&self) -> bool {
        self.steps.iter().any(|step| matches!(step.kind, StepKind::Link(_)))
    }
}

/// The prefix of a linked fixture's name (`spec/harness.md` LT-4).
pub const LINKED_FIXTURE_PREFIX: &str = "shared-";

impl Test {
    /// The `xfail` or `skip` that applies to `linker` on `arch`. At most
    /// one does.
    pub fn marker(&self, arch: Arch, linker: Linker) -> Option<&Marker> {
        self.markers.iter().find(|marker| marker.scope.applies(arch, linker))
    }

    /// The index in [`Test::steps`] of the first `host` step: where the
    /// pair part begins (`spec/harness.md` LT-2). `None`: the test links
    /// no host, and every linker runs its steps once, in its directory.
    pub fn pair_start(&self) -> Option<usize> {
        self.steps.iter().position(|step| matches!(step.kind, StepKind::Host(_)))
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
    odr: Vec<super::odr::Expected>,
    timeout: Option<Duration>,
    memory_mib: Option<u64>,
    steps: Vec<Step>,
    /// `exit` was given for the last step.
    exit_seen: bool,
    files: Vec<Embedded>,
    fixtures: Vec<String>,
    /// Parsing a fixture: build steps and `requires` only.
    fixture: bool,
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
            if ids.is_empty() || !ids.iter().all(|id| anomaly_id(id)) {
                return self.error(
                    line,
                    format!("'anomaly: {value}' is not a list of plumb.md anomaly ids ('A9 A11')"),
                );
            }
            self.anomalies.push(Anomaly { line, scope, ids, cross: false });
            return;
        }
        if qualified {
            return self.error(line, format!("the directive '{key}' takes no qualifiers"));
        }
        if value.is_empty() {
            return self.error(line, format!("the directive '{key}' has no value"));
        }
        if self.fixture && key != "requires" {
            return self
                .error(line, format!("a fixture has no '{key}' directive (only 'requires')"));
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
            "fixture" => {
                for name in value.split_whitespace() {
                    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-');
                    if !name.chars().all(plain) {
                        self.error(
                            line,
                            format!(
                                "'{name}' is not a fixture name (letters, digits, '_', '-'): the \
                                 fixture is tests/link/fixtures/<name>.fixture"
                            ),
                        );
                    } else if self.fixtures.iter().any(|f| f == name) {
                        self.error(line, format!("the fixture '{name}' is named twice"));
                    } else {
                        self.fixtures.push(name.to_owned());
                    }
                }
            }
            "diag" => {
                let mut words = value.split_whitespace();
                let id = words.next().unwrap_or_default();
                let files: Vec<String> = words.map(str::to_owned).collect();
                if !super::odr::ids().contains(&id) {
                    self.error(
                        line,
                        format!(
                            "'diag: {value}': '{id}' is not a one-definition-rule diagnostic of \
                             diag.md §3.1"
                        ),
                    );
                } else if files.is_empty() {
                    self.error(
                        line,
                        format!("'diag: {value}' names no file: 'diag: <id> <file>...'"),
                    );
                } else {
                    self.odr.push(super::odr::Expected { line, id: id.to_owned(), files });
                }
            }
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

    #[allow(clippy::too_many_lines)] // One arm per step keyword.
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
            "meta" => {
                qualifiers.no_family("'meta'")?;
                qualifiers.no_linker("'meta'")?;
                let shape = raw.len() == 4 && raw[2] == "-o";
                if !shape {
                    return Err(
                        "'meta' is 'meta <description> <in.o> -o <out.o>' (or <in.so> -o <out.so>, \
                         its .note.mason table)"
                            .to_owned(),
                    );
                }
                ("meta", StepKind::Meta(args))
            }
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
            "shlib" => {
                qualifiers.no_family("'shlib'")?;
                qualifiers.no_linker("'shlib'")?;
                let driver = [Driver::Gcc, Driver::Gxx, Driver::Gfortran]
                    .into_iter()
                    .find(|driver| driver.name() == first)
                    .ok_or_else(|| {
                        format!(
                            "'shlib' is followed by the compiler driver that links the shared \
                             library (gcc, g++, gfortran), not '{first}'"
                        )
                    })?;
                args.remove(0);
                if args.is_empty() {
                    return Err("'shlib' has no arguments".to_owned());
                }
                ("shlib", StepKind::Shlib(Shlib { driver, args }))
            }
            "partial" => {
                qualifiers.no_family("'partial'")?;
                qualifiers.no_linker("'partial'")?;
                let linker = Linker::parse(first).filter(|l| l.is_reference()).ok_or_else(|| {
                    format!(
                        "'partial' is followed by the reference linker that makes the \
                         relocatable output (bfd, lld, mold), not '{first}'"
                    )
                })?;
                args.remove(0);
                if args.is_empty() {
                    return Err("'partial' has no arguments".to_owned());
                }
                ("partial", StepKind::Partial(Partial { linker, args }))
            }
            "link" | "host" => {
                let keyword = if key == "host" { "host" } else { "link" };
                qualifiers.no_family(&format!("'{keyword}'"))?;
                if keyword == "host" && !qualifiers.scope.linkers.is_empty() {
                    // LT-1: a host is linked by every host linker of its
                    // pairs; a linker qualifier would say nothing.
                    return Err("'host' takes no linker qualifier: every pair links its host \
                                (LT-1)"
                        .to_owned());
                }
                let Some(driver) = Driver::ALL.into_iter().find(|driver| driver.name() == first)
                else {
                    let known: Vec<&str> = Driver::ALL.iter().map(|driver| driver.name()).collect();
                    return Err(format!(
                        "'{keyword}' is followed by what the link line is handed to ({}), not \
                         '{first}'",
                        known.join(", ")
                    ));
                };
                args.remove(0);
                let line = Link {
                    driver,
                    args,
                    fails: false,
                    informs: false,
                    diags: Vec::new(),
                    prints: Vec::new(),
                };
                if keyword == "host" {
                    ("host", StepKind::Host(line))
                } else {
                    ("link", StepKind::Link(line))
                }
            }
            "run" => {
                qualifiers.no_family("'run'")?;
                if args.is_empty() {
                    return Err("'run' is followed by the program".to_owned());
                }
                let program = args.remove(0);
                let stdout = Stdout::Exact(Vec::new());
                let exit = Exit::Status(0);
                ("run", StepKind::Run(Run { program, args, exit, stdout, env: Vec::new() }))
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
                if matches!(tool, CheckTool::Text | CheckTool::Elflint | CheckTool::Ldd)
                    && args.len() != 1
                {
                    return Err(format!("'check {first}' takes one file"));
                }
                if tool == CheckTool::Cmp && args.len() != 2 {
                    return Err(format!("'check {first}' takes two files"));
                }
                if args.is_empty() {
                    return Err(format!("'check {first}' has no arguments"));
                }
                ("check", StepKind::Check(Check { tool, args, expectations: Vec::new() }))
            }
            _ => {
                return Err(format!(
                    "unknown step '{key}' (cc, cxx, fc, as, yaml2obj, ar, meta, shlib, link, host, \
                     run, check)"
                ));
            }
        };
        // LT-4: a fixture builds inputs, and a linked fixture also links
        // its libraries (and checks them) with each linker.
        if self.fixture
            && !matches!(
                keyword,
                "cc" | "cxx"
                    | "fc"
                    | "as"
                    | "yaml2obj"
                    | "ar"
                    | "meta"
                    | "shlib"
                    | "partial"
                    | "link"
                    | "check"
            )
        {
            return Err(format!(
                "a fixture builds inputs and links libraries only: '{keyword}' is a step of a test"
            ));
        }
        self.steps.push(Step { line, keyword, scope: qualifiers.scope, kind });
        Ok(())
    }

    /// An indented line under a step.
    fn expectation(&mut self, line: usize, text: &str) {
        if let Err(message) = self.try_expectation(line, text) {
            self.error(line, message);
        }
    }

    #[allow(clippy::too_many_lines)] // One arm per (step, expectation).
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
            (StepKind::Host(_), "exit") => Err(
                "a host is to link: 'host' takes no 'exit' (LT-1; its library's 'link' may fail)"
                    .to_owned(),
            ),
            (StepKind::Link(_), "exit") if self.fixture => {
                Err("a linked fixture's library is to link: no 'exit' (LT-4)".to_owned())
            }
            (StepKind::Link(linking), "exit") => unqualified("exit").and_then(|()| {
                match value.trim() {
                    "0" => linking.fails = false,
                    "fail" => linking.fails = true,
                    "info" => linking.informs = true,
                    other => {
                        return Err(format!(
                            "'exit: {other}' of a link is not '0' (it succeeds), 'fail' or \
                             'info' (it succeeds and writes no output)"
                        ));
                    }
                }
                Ok(())
            }),
            (
                StepKind::Link(linking) | StepKind::Host(linking),
                "diag" | "no-diag" | "print" | "no-print",
            ) => {
                if let Some(pattern) = self.pattern(line, value) {
                    let expectation = Expectation {
                        line,
                        scope,
                        present: matches!(key, "diag" | "print"),
                        want: Want::Line,
                        pattern,
                    };
                    if key.ends_with("diag") {
                        linking.diags.push(expectation);
                    } else {
                        linking.prints.push(expectation);
                    }
                }
                Ok(())
            }
            (StepKind::Check(check), "has" | "not" | "then" | "count" | "allow") => {
                let (want, value) = match key {
                    "then" => (Want::Then, value),
                    "count" => {
                        let value = value.trim_start();
                        let (n, rest) = value.split_once([' ', '\t']).unwrap_or((value, ""));
                        match n.parse::<u32>() {
                            Ok(n) if !rest.trim().is_empty() => (Want::Count(n), rest),
                            _ => {
                                return Err(format!(
                                    "'count: {value}' is not 'count: <number> <pattern>'"
                                ));
                            }
                        }
                    }
                    "allow" if check.tool != CheckTool::Elflint => {
                        return Err("'allow' is an expectation of 'check elflint' only".to_owned());
                    }
                    "allow" => (Want::Allow, value),
                    _ => (Want::Line, value),
                };
                if let Some(pattern) = self.pattern(line, value) {
                    let present = key != "not";
                    check.expectations.push(Expectation { line, scope, present, want, pattern });
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
            (StepKind::Run(run), "env") => unqualified("env").and_then(|()| {
                let value = value.trim();
                let Some((name, text)) = value.split_once('=') else {
                    return Err(format!("'env: {value}' is not 'env: NAME=value'"));
                };
                let valid = !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
                if !valid {
                    return Err(format!("'{name}' is not an environment variable's name"));
                }
                if run.env.iter().any(|(other, _)| other == name) {
                    return Err(format!("a second 'env: {name}=' for this step"));
                }
                run.env.push((name.to_owned(), Template::parse(text)?));
                Ok(())
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
                "'{key}' is not an expectation of a link (exit, diag, no-diag, print, \
                     no-print)"
            )),
            (StepKind::Host(_), _) => Err(format!(
                "'{key}' is not an expectation of a host (diag, no-diag, print, no-print)"
            )),
            (StepKind::Run(_), _) => {
                Err(format!("'{key}' is not an expectation of a run (exit, stdout, output, env)"))
            }
            (StepKind::Check(_), _) => Err(format!(
                "'{key}' is not an expectation of a check (has, not, then, count, allow)"
            )),
            (
                StepKind::Compile(_)
                | StepKind::Yaml2obj(_)
                | StepKind::Ar(_)
                | StepKind::Meta(_)
                | StepKind::Shlib(_)
                | StepKind::Partial(_),
                _,
            ) => Err(format!("'{keyword}' builds an input and has no expectations")),
        };
        self.steps.push(Step { line: at, keyword, scope: step_scope, kind });
        result
    }

    /// What can only be said of a whole fixture file.
    fn finish_fixture(mut self) -> Result<Fixture, Vec<Error>> {
        self.steps.retain(|step| !step.keyword.is_empty());
        let steps = std::mem::take(&mut self.steps);
        // LT-4: the inputs are built once, before any linker links the
        // libraries; a check checks what a linker linked.
        let mut linked = false;
        for step in &steps {
            linked |= !step.is_build();
            if linked && step.is_build() {
                self.error(
                    step.line,
                    format!(
                        "'{}' builds an input after a 'link' or 'check': a fixture's inputs are \
                         built once, before any linker links its libraries",
                        step.keyword
                    ),
                );
            }
        }
        let links = steps.iter().any(|step| matches!(step.kind, StepKind::Link(_)));
        if let Some(check) = steps.iter().find(|step| matches!(step.kind, StepKind::Check(_)))
            && !links
        {
            self.error(check.line, "a fixture's 'check' checks a library it links: no 'link' step");
        }
        self.check_requires(&steps);
        self.check_steps(&steps, &Arch::ALL);
        self.check_file_names();
        if !self.errors.is_empty() {
            self.errors.sort_by_key(|error| error.line);
            return Err(self.errors);
        }
        Ok(Fixture {
            requires: self.requires.map(|(_, features)| features).unwrap_or_default(),
            steps,
            files: self.files,
        })
    }

    fn check_file_names(&mut self) {
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
        // A host linked against a linked fixture's library is the link
        // of a test without a library part (LT-2, LT-4).
        if !steps.iter().any(|step| matches!(step.kind, StepKind::Link(_) | StepKind::Host(_))) {
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
        self.check_pairs(&steps);
        self.check_scopes(&steps, &arches);
        self.check_requires(&steps);
        self.check_kiln_arches(&steps, &arches);
        self.check_steps(&steps, &arches);
        self.check_markers(&arches);
        self.check_file_names();
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
            odr: self.odr,
            timeout: self.timeout,
            memory_mib: self.memory_mib,
            steps,
            files: self.files,
            fixtures: self.fixtures,
        })
    }

    /// LT-2: the library part (to the first `host`) has a `link`; the pair
    /// part (from it) links no library.
    fn check_pairs(&mut self, steps: &[Step]) {
        let Some(start) = steps.iter().position(|step| matches!(step.kind, StepKind::Host(_)))
        else {
            return;
        };
        let linked_fixture = self.fixtures.iter().any(|f| f.starts_with(LINKED_FIXTURE_PREFIX));
        if !linked_fixture
            && !steps[..start].iter().any(|step| matches!(step.kind, StepKind::Link(_)))
        {
            self.error(
                steps[start].line,
                "a 'host' with no 'link' before it and no linked fixture: the library part, or a \
                 'shared-' fixture, links the library its hosts are linked against (LT-2)",
            );
        }
        for step in &steps[start..] {
            if matches!(step.kind, StepKind::Link(_)) {
                self.error(
                    step.line,
                    "a 'link' after the first 'host': from there on the steps run once per pair \
                     (LT-2); link the library before it",
                );
            }
        }
    }

    /// Every scope names only architectures the test is for.
    fn check_scopes(&mut self, steps: &[Step], arches: &[Arch]) {
        let mut scopes: Vec<(usize, &Scope)> = Vec::new();
        for step in steps {
            scopes.push((step.line, &step.scope));
            let expectations = match &step.kind {
                StepKind::Link(link) | StepKind::Host(link) => link.diags.as_slice(),
                StepKind::Check(check) => check.expectations.as_slice(),
                _ => &[],
            };
            scopes.extend(expectations.iter().map(|expect| (expect.line, &expect.scope)));
            if let StepKind::Link(link) | StepKind::Host(link) = &step.kind {
                scopes.extend(link.prints.iter().map(|expect| (expect.line, &expect.scope)));
            }
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

    /// kiln targets AArch64 and x86-64 only: a test with a `(kiln)` step
    /// says so with `arch` (WP-134).
    fn check_kiln_arches(&mut self, steps: &[Step], arches: &[Arch]) {
        let Some(step) = steps.iter().find(|s| s.feature() == Some(Feature::Kiln)) else {
            return;
        };
        if arches.contains(&Arch::I386) {
            self.error(
                step.line,
                "a kiln step in a test that runs on i386: kiln does not target it (add \
                 '#@ arch: aarch64 x86_64')",
            );
        }
    }

    /// The rules of single steps that need the whole step.
    fn check_steps(&mut self, steps: &[Step], arches: &[Arch]) {
        for step in steps {
            let uses_linker = |args: &[Template]| args.iter().any(Template::uses_linker);
            match &step.kind {
                StepKind::Compile(Compile { args, .. })
                | StepKind::Yaml2obj(args)
                | StepKind::Ar(args)
                | StepKind::Meta(args)
                | StepKind::Shlib(Shlib { args, .. })
                | StepKind::Partial(Partial { args, .. }) => {
                    if uses_linker(args) {
                        self.error(
                            step.line,
                            "'%{linker}' in a step that builds an input: it runs before any \
                             linker",
                        );
                    }
                }
                // `check elflint` checks its complaints even without a
                // `has` or `not`.
                StepKind::Check(check)
                    if check.tool != CheckTool::Elflint
                        && !check.expectations.iter().any(|e| e.want != Want::Allow) =>
                {
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
                StepKind::Link(_) | StepKind::Host(_) | StepKind::Run(_) | StepKind::Check(_) => {}
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
    if family == Family::Kiln && !matches!(language, Language::C | Language::Cxx) {
        return Err(format!("'{keyword}' has no kiln family: kiln compiles C and C++"));
    }
    if family == Family::Hod && !matches!(language, Language::C | Language::Cxx) {
        return Err(format!(
            "'{keyword}' has no hod family: hod compiles the IR clang makes of C and C++"
        ));
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

/// `N` (0 to 255), `signal N` (1 to 64) or `no-start`.
fn parse_exit(value: &str) -> Result<Exit, String> {
    if value == "no-start" {
        return Ok(Exit::NoStart);
    }
    let number = |text: &str| text.parse::<i32>().ok().filter(|_| !text.starts_with('+'));
    let exit = match value.strip_prefix("signal") {
        Some(signal) if signal.starts_with([' ', '\t']) => {
            number(signal.trim()).filter(|n| (1..=64).contains(n)).map(Exit::Signal)
        }
        Some(_) => None,
        None => number(value).filter(|n| (0..=255).contains(n)).map(Exit::Status),
    };
    exit.ok_or_else(|| {
        format!(
            "'exit: {value}' of a run is not a status from 0 to 255, 'signal N' (1 to 64) or \
             'no-start'"
        )
    })
}

/// Parses a test file.
///
/// # Errors
/// Every error of the file, in line order.
/// Whether `id` has the form of a `spec/plumb.md` §10.2 anomaly id: `A`
/// and a number.
fn anomaly_id(id: &str) -> bool {
    id.strip_prefix('A').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The run-wide default anomalies, `tests/link/anomalies`
/// (`spec/harness.md` "L3"): one `<linker>[,<more linkers or
/// architectures, or `cross`>]: <id>...` per line, `#` comments and blank
/// lines aside.
/// Every line names at least one reference linker and never mortar.
///
/// # Errors
/// Every line that does not have that form, with its number.
pub fn parse_defaults(text: &str) -> Result<Vec<Anomaly>, Vec<Error>> {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let text = raw.trim();
        if text.is_empty() || text.starts_with('#') {
            continue;
        }
        let mut error = |message: String| errors.push(Error { line, message });
        let Some((who, ids)) = text.split_once(':') else {
            error(format!("'{text}' is not '<linker>[,<arch>...]: <id>...'"));
            continue;
        };
        let mut scope = Scope::default();
        let mut cross = false;
        let mut bad = false;
        for word in who.split(',').map(str::trim) {
            if word == "cross" {
                cross = true;
            } else if let Some(linker) = Linker::parse(word) {
                scope.linkers.push(linker);
            } else if let Some(arch) = Arch::parse(word) {
                scope.arches.push(arch);
            } else {
                error(format!("unknown linker or architecture '{word}'"));
                bad = true;
            }
        }
        if bad {
            continue;
        }
        if scope.linkers.is_empty() || scope.linkers.contains(&Linker::Mortar) {
            error("a default names the reference linkers it is for, never mortar".to_owned());
            continue;
        }
        let ids: Vec<String> = ids.split_whitespace().map(str::to_owned).collect();
        if ids.is_empty() || !ids.iter().all(|id| anomaly_id(id)) {
            error(format!("'{}' is not a list of plumb.md anomaly ids ('A9 A11')", ids.join(" ")));
            continue;
        }
        out.push(Anomaly { line, scope, ids, cross });
    }
    if errors.is_empty() { Ok(out) } else { Err(errors) }
}

/// A mortar `xfail` of `tests/link/xfails` (`spec/harness.md` "Markers"):
/// run-wide on one architecture (mortar has no back end there yet, so
/// every mortar link of that architecture is expected to fail), or of one
/// test on one architecture (the package that makes it pass has not
/// landed: "M2: WP-053").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchXfail {
    pub line: usize,
    pub arch: Arch,
    /// The test's name (`dyn/kinds/output-kind-table.test`), or `None` for
    /// every test of the architecture.
    pub test: Option<String>,
    pub reason: String,
}

/// The mortar `xfail`s, `#` comments and blank lines aside: one
/// `mortar,<arch>: <reason>` per line (run-wide, at most one per
/// architecture), or `mortar[,<arch>...] <test>: <reason>` (one test; on
/// every architecture when none is named; at most one per test and
/// architecture).
///
/// # Errors
/// Every line that does not have that form, with its number.
pub fn parse_xfails(text: &str) -> Result<Vec<ArchXfail>, Vec<Error>> {
    let mut out: Vec<ArchXfail> = Vec::new();
    let mut errors = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let entry = raw.trim();
        if entry.is_empty() || entry.starts_with('#') {
            continue;
        }
        let mut error = |message: String| errors.push(Error { line, message });
        let Some((who, reason)) = entry.split_once(':') else {
            error(format!(
                "'{entry}' is not 'mortar,<arch>: <reason>' or 'mortar[,<arch>...] <test>: <reason>'"
            ));
            continue;
        };
        // The test's name is the last word when it ends with `.test`, or
        // when there are words after the qualifiers' list.
        let who = who.trim();
        let (who, named) = match who.rsplit_once([' ', '\t']) {
            Some((rest, last))
                if last.strip_suffix(".test").is_some() || !rest.trim_end().ends_with(',') =>
            {
                (rest.trim_end(), Some(last))
            }
            _ => (who, None),
        };
        if let Some(test) = named {
            let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-');
            let valid = test.strip_suffix(".test").is_some()
                && test.split('/').all(|part| !part.is_empty() && part.chars().all(plain));
            if !valid {
                error(format!("'{test}' is not a test's name ('dyn/kinds/x.test')"));
                continue;
            }
        }
        let words: Vec<&str> = who.split(',').map(str::trim).collect();
        let arches: Option<Vec<Arch>> = match (words.split_first(), named) {
            (Some((&"mortar", [arch])), None) => Arch::parse(arch).map(|arch| vec![arch]),
            (Some((&"mortar", [])), Some(_)) => Some(Arch::ALL.to_vec()),
            (Some((&"mortar", rest)), Some(_)) => {
                rest.iter().map(|word| Arch::parse(word)).collect()
            }
            _ => None,
        };
        let Some(arches) = arches else {
            error(format!(
                "'{}' is not 'mortar,<arch>': a run-wide xfail is for mortar on one \
                 architecture, a test's for mortar on the architectures named",
                who.trim()
            ));
            continue;
        };
        let reason = reason.trim();
        if reason.is_empty() {
            error("an xfail needs a reason".to_owned());
            continue;
        }
        let name = named.map(str::to_owned);
        for arch in arches {
            if let Some(first) = out.iter().find(|x| x.arch == arch && x.test == name) {
                let what = name.as_deref().map_or_else(String::new, |t| format!(" of {t}"));
                error(format!(
                    "a second xfail{what} for mortar on {} (the first is on line {})",
                    arch.name(),
                    first.line
                ));
                break;
            }
            out.push(ArchXfail { line, arch, test: name.clone(), reason: reason.to_owned() });
        }
    }
    if errors.is_empty() { Ok(out) } else { Err(errors) }
}

pub fn parse(text: &str) -> Result<Test, Vec<Error>> {
    parse_with(text, Parser::default()).finish()
}

/// Parses a fixture file (`spec/harness.md` "Fixtures"): `requires` and
/// build steps, then embedded files.
///
/// # Errors
/// Every error of the file, in line order.
pub fn parse_fixture(text: &str) -> Result<Fixture, Vec<Error>> {
    parse_with(text, Parser { fixture: true, ..Parser::default() }).finish_fixture()
}

fn parse_with(text: &str, mut parser: Parser) -> Parser {
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
    parser
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

    // "Directives": `diag: <id> <file>...` names a one-definition-rule
    // diagnostic and its places; repeatable.
    #[test]
    fn odr_diag_directives() {
        let with = |directives: &str| {
            format!("#@ spec: odr.md \"x\"\n#@ arch: aarch64\n{directives}link ld a.o b.o\n")
        };
        let test = parse(&with(
            "#@ diag: odr-size-mismatch a.o b.o\n#@ diag: odr-symbol-type-mismatch b.o a.o\n",
        ))
        .unwrap();
        assert_eq!(test.odr.len(), 2);
        assert_eq!(test.odr[0].id, "odr-size-mismatch");
        assert_eq!(test.odr[1].files, ["b.o", "a.o"]);
        assert_eq!(test.odr[1].line, 4);
        assert!(has_error(&with("#@ diag: undefined-symbol a.o\n"), 3, "not a one-definition"));
        assert!(has_error(&with("#@ diag: odr-size-mismatch\n"), 3, "names no file"));
        assert!(has_error(&with("#@ diag(mortar): odr-size-mismatch a.o\n"), 3, "no qualifiers"));
    }

    // "L3": the run-wide default anomalies.
    #[test]
    fn default_anomalies() {
        let d = parse_defaults("# c\n\nmold: A1 A9\nbfd,aarch64: A10\n").unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(
            (d[0].line, d[0].scope.linkers.as_slice(), d[0].ids.as_slice()),
            (3, &[Linker::Mold][..], &["A1".to_owned(), "A9".to_owned()][..])
        );
        assert_eq!(d[1].scope.arches, [Arch::Aarch64]);
        assert!(!d[0].cross && !d[1].cross);
        // `cross`: on an architecture that is not the host's only.
        let c = parse_defaults("mold,x86_64,cross: A13\n").unwrap();
        assert!(c[0].cross);
        assert!(c[0].applies(Arch::X86_64, Linker::Mold, Some(Arch::Aarch64)));
        assert!(!c[0].applies(Arch::X86_64, Linker::Mold, Some(Arch::X86_64)));
        assert!(!c[0].applies(Arch::Aarch64, Linker::Mold, Some(Arch::X86_64)));
        assert!(d[0].applies(Arch::Aarch64, Linker::Mold, Some(Arch::Aarch64)));
        let error = |text: &str| parse_defaults(text).unwrap_err()[0].message.clone();
        assert!(error("mold A9\n").contains("is not '<linker>"));
        assert!(error("gold: A9\n").contains("unknown linker or architecture 'gold'"));
        assert!(error("aarch64: A9\n").contains("never mortar"));
        assert!(error("mortar: A9\n").contains("never mortar"));
        assert!(error("mold: 9\n").contains("not a list of plumb.md anomaly ids"));
        assert!(error("mold:\n").contains("not a list of plumb.md anomaly ids"));
    }

    // "Markers": the run-wide mortar xfails, `tests/link/xfails`.
    #[test]
    fn arch_xfails() {
        let x = parse_xfails("# c\n\nmortar,x86_64: no back end\nmortar, i386 : later\n").unwrap();
        assert_eq!(x.len(), 2);
        assert_eq!((x[0].line, x[0].arch, x[0].reason.as_str()), (3, Arch::X86_64, "no back end"));
        assert_eq!((x[1].arch, x[1].reason.as_str()), (Arch::I386, "later"));
        let error = |text: &str| parse_xfails(text).unwrap_err()[0].message.clone();
        assert!(error("mortar x86_64 why\n").contains("is not 'mortar,<arch>: <reason>'"));
        assert!(error("mold,x86_64: why\n").contains("for mortar on one architecture"));
        assert!(error("mortar: why\n").contains("for mortar on one architecture"));
        assert!(error("mortar,x86_64,i386: why\n").contains("for mortar on one architecture"));
        assert!(error("mortar,sparc: why\n").contains("for mortar on one architecture"));
        assert!(error("mortar,i386:\n").contains("needs a reason"));
        assert!(error("mortar,i386: a\nmortar,i386: b\n").contains("a second xfail for"));
        // A test's xfails, on the architectures named or on every one.
        let t =
            parse_xfails("mortar,aarch64 dyn/a.test: M2: WP-053\nmortar dyn/b.test: M2\n").unwrap();
        assert_eq!(t.len(), 4);
        assert_eq!((t[0].arch, t[0].test.as_deref()), (Arch::Aarch64, Some("dyn/a.test")));
        assert_eq!(t[0].reason, "M2: WP-053");
        assert_eq!(t[1..].iter().map(|x| x.arch).collect::<Vec<_>>(), Arch::ALL);
        assert!(error("mortar dyn/a: M2\n").contains("is not a test's name"));
        assert!(error("mold dyn/a.test: M2\n").contains("for mortar on one architecture"));
        assert!(error("mortar,arm dyn/a.test: M2\n").contains("for mortar on one architecture"));
        assert!(
            error("mortar dyn/a.test: x\nmortar,i386 dyn/a.test: y\n")
                .contains("a second xfail of dyn/a.test for mortar on i386")
        );
        // A run-wide line and a test's line on one architecture are both
        // allowed: the run-wide one is taken.
        assert_eq!(parse_xfails("mortar,i386: a\nmortar dyn/a.test: b\n").unwrap().len(), 4);
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
        // WP-129: a `(hod)` step needs `requires: hod`; hod takes C and C++.
        let hod = ok("#@ requires: hod\ncc(hod) -fPIC a.c -o a.o\nlink gcc -shared a.o\n");
        let StepKind::Compile(compile) = &hod.steps[0].kind else { panic!() };
        assert_eq!((compile.family, hod.requires.as_slice()), (Family::Hod, &[Feature::Hod][..]));
        assert!(has_error(&with("cc(hod) a.c -o a.o\nlink gcc a.o\n"), 2, "requires: hod"));
        assert!(has_error(&with("#@ requires: hod\nas(hod) a.s\n"), 3, "no hod family"));
        let fortran = ok("#@ requires: gfortran\nfc a.f90\nlink gfortran a.o\n");
        assert_eq!(fortran.requires, [Feature::Gfortran]);
        let asm = ok("as a.S -o a.o\nlink gcc -static a.o\n");
        let StepKind::Compile(compile) = &asm.steps[0].kind else { panic!() };
        assert_eq!(compile.language, Language::Asm);
        let checked = ok("link ld a.o\ncheck text a.map\n  has: .text\n");
        let StepKind::Check(check) = &checked.steps[1].kind else { panic!() };
        assert_eq!(check.tool, CheckTool::Text);
        let compared =
            ok("link ld a.o -o a\nlink ld b.o -o b\ncheck cmp a b\n  has: ^identical$\n");
        let StepKind::Check(check) = &compared.steps[2].kind else { panic!() };
        assert_eq!((check.tool, check.args.len()), (CheckTool::Cmp, 2));
        assert!(has_error(&with("link ld a.o\ncheck cmp a\n  has: x\n"), 3, "takes two files"));

        assert!(has_error(&with("compile a.c\nlink ld a.o\n"), 2, "unknown step 'compile'"));
        assert!(has_error(&with("cc\nlink ld a.o\n"), 2, "'cc' has no arguments"));
        assert!(has_error(&with("cc(bfd) a.c\nlink ld a.o\n"), 2, "cannot be for 'bfd' only"));
        assert!(has_error(&with("cc(gcc,clang) a.c\nlink ld a.o\n"), 2, "one compiler family"));
        assert!(has_error(&with("fc(clang) a.f\nlink ld a.o\n"), 2, "flang"));
        assert!(has_error(&with("link(gcc) ld a.o\n"), 2, "takes no compiler family"));
        assert!(has_error(&with("link cc a.o\n"), 2, "not 'cc'"));
        assert!(has_error(&with("link ld a.o\nrun\n"), 3, "followed by the program"));
        assert!(has_error(&with("link ld a.o\ncheck lddx a\n  has: x\n"), 3, "not 'lddx'"));
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
        // WP-080: `llvm-dwarfdump` (`--verify` on split-DWARF outputs).
        let dump = "link ld a.o\ncheck llvm-dwarfdump --verify a.out\n  has: No errors.\n";
        assert!(has_error(&with(dump), 3, "needs 'llvm-dwarfdump'"));
        let required = format!("#@ requires: llvm-dwarfdump\n{dump}");
        let test = parse(&with(&required)).unwrap();
        let tools: Vec<CheckTool> = test
            .steps
            .iter()
            .filter_map(|s| match &s.kind {
                StepKind::Check(c) => Some(c.tool),
                _ => None,
            })
            .collect();
        assert_eq!(tools, [CheckTool::LlvmDwarfdump]);
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
        let no_start = parse(&with("link ld a.o -o p\nrun p\n  exit: no-start\n")).unwrap();
        let StepKind::Run(run) = &no_start.steps[1].kind else { panic!() };
        assert_eq!(run.exit, Exit::NoStart);
        let reference = parse(&with("link ld a.o\nrun a.out\n  output: reference\n")).unwrap();
        assert!(reference.compares_output());
        // WP-112: a link that only informs, and its standard output.
        let info =
            parse(&with("link ld --version\n  exit: info\n  print: ^GNU ld\n  no-print: x\n"))
                .unwrap();
        let StepKind::Link(link) = &info.steps[0].kind else { panic!() };
        assert!(link.informs && !link.fails && link.diags.is_empty());
        assert_eq!(link.prints.len(), 2);
        assert!(link.prints[0].present && !link.prints[1].present);

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

    // "Steps": `partial <reference> <args>` (WP-175), a build step.
    #[test]
    fn partial_step() {
        let with = |text: &str| format!("#@ spec: arch.md \"3.4\"\n{text}");
        let test = parse(&with("partial lld a.o b.o -o r.o\nlink gcc r.o\n")).unwrap();
        let StepKind::Partial(partial) = &test.steps[0].kind else { panic!() };
        assert_eq!(partial.linker, Linker::Lld);
        let vars = Vars { arch: Arch::X86_64, host: Some(Arch::Aarch64), linker: None };
        let args: Vec<String> = partial.args.iter().map(|a| a.expand(vars)).collect();
        assert_eq!(args, ["a.o", "b.o", "-o", "r.o"]);
        assert!(test.steps[0].is_build() && test.steps[0].feature().is_none());
        assert_eq!(test.steps[0].keyword, "partial");
        let bad = with("partial mortar a.o -o r.o\nlink gcc r.o\n");
        assert!(has_error(&bad, 2, "reference linker"));
        assert!(has_error(&with("partial bfd\nlink gcc r.o\n"), 2, "has no arguments"));
        assert!(has_error(&with("partial(mold) bfd a.o\nlink gcc r.o\n"), 2, "'partial'"));
        let in_build = with("partial bfd a.o -o %{linker}.o\nlink gcc r.o\n");
        assert!(has_error(&in_build, 2, "'%{linker}' in a step that builds an input"));
        assert!(has_error(&with("partial bfd a.o -o r.o\n  exit: 0\nlink gcc r.o\n"), 3, "has no"));
    }

    // "Steps", "Expectations", "Checks": the forms dynamic tests use.
    #[test]
    fn dynamic_steps_and_expectations() {
        let with = |text: &str| format!("#@ spec: dyn.md \"D-1\"\n{text}");
        let test = parse(&with(
            "#@ requires: g++ gdb eu-elflint\n#@ fixture: greet hook-lib\n\
             shlib g++ a.o -o liba.so\nlink gcc a.o\n\
             run a.out\n  env: LD_PRELOAD=./libhook.so\n  env: LD_BIND_NOW=1\n\
             check readelf -dW a.out\n  has: (NEEDED)\n  then: (NEEDED)\n  count: 2 (NEEDED)\n\
             check elflint a.out\n  allow(mortar): has wrong flags\n\
             check ldd a.out\n  not: not found\n\
             check gdb a.out \"break main\" run\n  has: Breakpoint 1\n",
        ))
        .unwrap();
        assert_eq!(test.fixtures, ["greet", "hook-lib"]);
        let StepKind::Shlib(shlib) = &test.steps[0].kind else { panic!() };
        assert_eq!(shlib.driver, Driver::Gxx);
        assert!(test.steps[0].is_build());
        let StepKind::Run(run) = &test.steps[2].kind else { panic!() };
        let vars = Vars { arch: Arch::Aarch64, host: Some(Arch::Aarch64), linker: None };
        let env: Vec<(&str, String)> =
            run.env.iter().map(|(n, v)| (n.as_str(), v.expand(vars))).collect();
        assert_eq!(
            env,
            [("LD_PRELOAD", "./libhook.so".to_owned()), ("LD_BIND_NOW", "1".to_owned())]
        );
        let StepKind::Check(readelf) = &test.steps[3].kind else { panic!() };
        let wants: Vec<Want> = readelf.expectations.iter().map(|e| e.want).collect();
        assert_eq!(wants, [Want::Line, Want::Then, Want::Count(2)]);
        let tools: Vec<CheckTool> = test.steps[3..]
            .iter()
            .map(|step| match &step.kind {
                StepKind::Check(check) => check.tool,
                _ => panic!(),
            })
            .collect();
        assert_eq!(tools, [CheckTool::Readelf, CheckTool::Elflint, CheckTool::Ldd, CheckTool::Gdb]);
        let StepKind::Check(elflint) = &test.steps[4].kind else { panic!() };
        assert_eq!(elflint.expectations[0].want, Want::Allow);
        assert_eq!(elflint.expectations[0].scope.linkers, [Linker::Mortar]);
        // An elflint check with nothing but its allow-list.
        assert!(
            parse(&with("#@ requires: eu-elflint\nlink gcc a.o\ncheck elflint a.out\n")).is_ok()
        );

        let run = |e: &str| with(&format!("link gcc a.o\nrun a.out\n  {e}\n"));
        assert!(has_error(&run("env: LD_PRELOAD"), 4, "is not 'env: NAME=value'"));
        assert!(has_error(&run("env: ld_preload=x"), 4, "not an environment variable's name"));
        assert!(has_error(&run("env: A=1\n  env: A=2"), 5, "a second 'env: A='"));
        let check = |e: &str| with(&format!("link gcc a.o\ncheck readelf -d a.out\n  {e}\n"));
        assert!(has_error(&check("count: x (NEEDED)"), 4, "is not 'count: <number> <pattern>'"));
        assert!(has_error(&check("count: 2"), 4, "is not 'count: <number> <pattern>'"));
        assert!(has_error(&check("allow: x"), 4, "of 'check elflint' only"));
        assert!(has_error(&with("link gcc a.o\ncheck ldd a b\n  has: x\n"), 3, "one file"));
        assert!(has_error(&with("link gcc a.o\ncheck gdb a\n  has: x\n"), 3, "needs 'gdb'"));
        assert!(has_error(&with("shlib g++ a.o\nlink gcc a.o\n"), 2, "g++"));
        assert!(has_error(&with("shlib ld a.o\nlink gcc a.o\n"), 2, "ld"));
        assert!(has_error(&with("#@ fixture: ../x\nlink gcc a.o\n"), 2, "not a fixture name"));
        assert!(has_error(&with("#@ fixture: x x\nlink gcc a.o\n"), 2, "named twice"));
    }

    // "Fixtures": `requires`, build steps and files only.
    #[test]
    fn fixtures() {
        let fixture = parse_fixture(
            "# libgreet\n#@ requires: g++\ncxx -fPIC g.cc -o g.o\nshlib g++ g.o -o libg.so\n\
             #--- g.cc\nint g() { return 1; }\n",
        )
        .unwrap();
        assert_eq!(fixture.requires, [Feature::Gxx]);
        assert_eq!(fixture.steps.len(), 2);
        assert_eq!(fixture.files[0].name, "g.cc");
        let error = |text: &str| -> Vec<String> {
            parse_fixture(text).unwrap_err().iter().map(ToString::to_string).collect()
        };
        let has = |text: &str, part: &str| {
            let all = error(text);
            assert!(all.iter().any(|e| e.contains(part)), "no '{part}' in {all:#?}");
        };
        has("#@ spec: dyn.md \"D-1\"\ncc a.c\n", "a fixture has no 'spec' directive");
        has("cc a.c\nrun prog\n", "a fixture builds inputs and links libraries only: 'run'");
        has("cc a.c\nhost gcc a.o\n", "a fixture builds inputs and links libraries only: 'host'");
        has("cxx a.cc\n", "g++");
        has("cc a.c\n#--- a.c\n#--- a.c\n", "embedded twice");
        // No spec and no link are needed.
        let plain = parse_fixture("cc a.c\n#--- a.c\nint a;\n").unwrap();
        assert!(!plain.is_linked());
    }

    // LT-4: a linked fixture links (and checks) its libraries with each
    // linker, after its inputs are built; its library is to link.
    #[test]
    fn linked_fixtures() {
        let linked = parse_fixture(
            "cc -fPIC a.c -o a.o\nlink gcc -shared a.o -o liba.so\n\
             check readelf -dW liba.so\n  has: SONAME\n#--- a.c\nint a;\n",
        )
        .unwrap();
        assert!(linked.is_linked());
        assert_eq!(linked.steps.len(), 3);
        let error = |text: &str| -> Vec<String> {
            parse_fixture(text).unwrap_err().iter().map(ToString::to_string).collect()
        };
        let has = |text: &str, line: usize, part: &str| {
            let all = error(text);
            let want = format!("line {line}: ");
            assert!(
                all.iter().any(|e| e.starts_with(&want) && e.contains(part)),
                "no '{part}' at line {line} in {all:#?}"
            );
        };
        has("link gcc a.o -o l.so\ncc a.c\n", 2, "builds an input after a 'link'");
        has("cc a.c\nlink gcc a.o\n  exit: fail\n  diag: x\n", 3, "no 'exit' (LT-4)");
        has("cc a.c\ncheck nm a.o\n  has: a\n", 2, "no 'link' step");
    }

    // LT-1, LT-2: `host` links a program in each pair; it needs a library
    // linked before it and is never expected to fail; from the first
    // `host` on, no library is linked.
    #[test]
    fn host_steps_and_the_pair_part() {
        let head = "#@ spec: harness.md \"x\"\ncc a.c\n";
        let test = parse(&format!(
            "{head}link gcc -shared a.o -o liba.so\nhost gcc m.o liba.so -o m\n  \
             no-diag: warning\nrun m\n  exit: 3\nhost gcc d.o -o d\nrun d\n"
        ))
        .unwrap();
        assert_eq!(test.pair_start(), Some(2));
        let StepKind::Host(host) = &test.steps[2].kind else { panic!("{:?}", test.steps[2]) };
        assert_eq!((host.driver, host.diags.len()), (Driver::Gcc, 1));
        assert_eq!(test.steps[2].feature(), None);
        assert!(!test.steps[2].is_build());
        let plain = parse(&format!("{head}link gcc a.o -o a\nrun a\n")).unwrap();
        assert_eq!(plain.pair_start(), None);
        let at = |text: &str, line: usize, part: &str| {
            assert!(has_error(&format!("{head}{text}"), line, part), "{text}");
        };
        at("host gcc m.o -o m\n", 3, "no 'link' before it");
        at("link gcc a.o\nhost gcc m.o\nlink gcc b.o\n", 5, "a 'link' after the first 'host'");
        at("link gcc a.o\nhost gcc m.o\n  exit: fail\n", 5, "'host' takes no 'exit'");
        at("link gcc a.o\nhost(mortar) gcc m.o\n", 4, "'host' takes no linker");
        at("link gcc a.o\nhost(clang) gcc m.o\n", 4, "'host' takes no");
        at("link gcc a.o\nhost cc m.o\n", 4, "'host' is followed by what the link line");
        at("link gcc a.o\nhost gcc m.o\n  has: x\n", 5, "not an expectation of a host");
        assert!(has_error(
            &format!("{head}link gcc a.o\nhost g++ m.o\n"),
            4,
            "this step needs 'g++'"
        ));
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
