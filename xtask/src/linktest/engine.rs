//! Running one test on one architecture (`spec/harness.md` "Link tests":
//! "Running a test", "Steps", "The self-check", "Limits").
//!
//! A unit of a run is a test on an architecture. Its inputs are built once,
//! in `<dir>/inputs`; each linker gets a copy of them in `<dir>/<linker>`
//! and runs the link, run and check steps there. The reference linkers go
//! first; mortar is judged only when they establish the test.

use std::ffi::OsString;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::format::{
    Check, CheckTool, Driver, Exit, Expectation, Language, Link, MarkerKind, Run, Stdout, Step,
    StepKind, Template, Test, Vars,
};
use super::outcome::{self, Outcome, Printed, Raw, Reference, Verdict};
use super::pattern::Pattern;
use super::report::Record;
use super::target::{Arch, ArchTools, CommonTools, Family, Feature, Linker, Runner};
use crate::process::{self, End, Limits, Output};

/// The limits of a step that builds an input and of a check's tool.
pub const TOOL_LIMITS: Limits = Limits { time: Duration::from_mins(2), memory_kib: 4 << 20 };
/// The default limits of a link step (the whole driver, when there is one).
pub const LINK_LIMITS: Limits = Limits { time: Duration::from_mins(1), memory_kib: 4 << 20 };
/// The default limits of a program run natively.
pub const RUN_LIMITS: Limits = Limits { time: Duration::from_secs(10), memory_kib: 1 << 20 };
/// How much longer an emulated program may run.
pub const EMULATED_TIME_FACTOR: u32 = 6;
/// What `qemu-i386` reserves for its guest beyond the program's own limit:
/// the whole 32-bit address space, which counts against `ulimit -v`.
pub const I386_GUEST_KIB: u64 = 4 << 20;

/// The environment variable the linker wrapper of a driver link appends
/// the linker's exit status to.
pub const STATUS_VAR: &str = "MORTAR_LINKTEST_STATUS";

/// What every unit of a run shares.
#[derive(Debug)]
pub struct Matrix {
    /// The machine the harness runs on.
    pub host: Option<Arch>,
    pub common: CommonTools,
    /// The linkers of the run, in [`Linker::ALL`] order.
    pub linkers: Vec<Linker>,
    /// The mortar binary, when mortar is in the run.
    pub mortar: Option<PathBuf>,
    /// The directory of the linker wrappers, `<dir>/<arch>/<linker>/ld`.
    pub wrappers: PathBuf,
}

impl Matrix {
    /// The program of `linker` for an architecture, if there is one.
    pub fn linker_program<'a>(&'a self, tools: &'a ArchTools, linker: Linker) -> Option<&'a Path> {
        match linker {
            Linker::Bfd => tools.bfd.as_deref(),
            Linker::Lld => self.common.lld.as_deref(),
            Linker::Mold => self.common.mold.as_deref(),
            Linker::Mortar => self.mortar.as_deref(),
        }
    }

    /// The linkers of the run that `tools`' architecture has.
    pub fn linkers_of(&self, tools: &ArchTools) -> Vec<Linker> {
        self.linkers
            .iter()
            .copied()
            .filter(|linker| self.linker_program(tools, *linker).is_some())
            .collect()
    }

    /// The directory whose `ld` is the wrapper of `linker` on `arch`.
    pub fn wrapper_dir(&self, arch: Arch, linker: Linker) -> PathBuf {
        self.wrappers.join(arch.name()).join(linker.name())
    }

    /// Writes the wrapper of every linker of every architecture in
    /// `arches`. Called before any child is started: a script this
    /// process has open for writing must not be inherited by a fork.
    pub fn write_wrappers(&self, arches: &[ArchTools]) -> Result<(), String> {
        for tools in arches {
            for linker in self.linkers_of(tools) {
                let Some(program) = self.linker_program(tools, linker) else { continue };
                let dir = self.wrapper_dir(tools.arch, linker);
                write_wrapper(&dir.join("ld"), program)?;
            }
        }
        Ok(())
    }

    /// Whether the tool of `feature` is there for `tools`' architecture.
    pub fn has(&self, tools: &ArchTools, feature: Feature) -> bool {
        match feature {
            Feature::Gxx => tools.gxx.is_some(),
            Feature::Gfortran => tools.gfortran.is_some(),
            Feature::Clang => self.common.clang.is_some() && self.common.clangxx.is_some(),
            Feature::Yaml2obj => self.common.yaml2obj.is_some(),
            Feature::LlvmReadelf => self.common.llvm_readelf.is_some(),
        }
    }
}

/// The wrapper a compiler driver runs as its `ld`: it runs the linker with
/// the driver's arguments, appends the linker's exit status to the file
/// named by [`STATUS_VAR`], and exits with it.
fn write_wrapper(path: &Path, linker: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    let shown = linker.display().to_string();
    if shown.contains('\'') {
        return Err(format!("the linker path '{shown}' has a quote"));
    }
    let body = format!(
        "#!/bin/sh\n\
         # The linker of a link-test driver link (spec/harness.md \"Link tests\").\n\
         printf '%s\\n' \"$*\" >> \"${STATUS_VAR}.args\"\n\
         '{shown}' \"$@\"\n\
         status=$?\n\
         echo \"$status\" >> \"${STATUS_VAR}\"\n\
         exit \"$status\"\n"
    );
    let cannot = |e: std::io::Error| format!("cannot write '{}': {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(cannot)?;
    }
    std::fs::write(path, body).map_err(cannot)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(cannot)
}

/// What one unit produced.
#[derive(Debug, Default)]
pub struct Results {
    pub records: Vec<Record>,
    /// The tools the unit needed that are missing.
    pub missing: Vec<String>,
}

/// A test on one architecture.
#[derive(Debug)]
pub struct Unit<'a> {
    pub matrix: &'a Matrix,
    pub tools: &'a ArchTools,
    pub test: &'a Test,
    /// The test's name in a report.
    pub name: &'a str,
    /// The unit's work directory; it is created.
    pub dir: PathBuf,
}

impl Unit<'_> {
    fn arch(&self) -> Arch {
        self.tools.arch
    }

    fn record(
        &self,
        linker: Option<Linker>,
        outcome: Outcome,
        detail: impl Into<String>,
    ) -> Record {
        Record {
            test: self.name.to_owned(),
            arch: Some(self.arch()),
            linker,
            outcome,
            detail: detail.into(),
            spec: self.test.spec.join("; "),
        }
    }

    /// Runs the unit: every linker of the run that the architecture has,
    /// the references first.
    pub fn run(&self) -> Results {
        let mut results = Results::default();
        let missing: Vec<String> = self
            .test
            .requires
            .iter()
            .filter(|feature| !self.matrix.has(self.tools, **feature))
            .map(|feature| {
                super::target::feature_tool_name(*feature, self.arch(), self.matrix.host)
            })
            .collect();
        if !missing.is_empty() {
            let detail = format!("requires {}: not found", missing.join(", "));
            results.records.push(self.record(None, Outcome::Skip, detail));
            if mortar_testutil::tools_required() {
                results.missing = missing;
            }
            return results;
        }
        if let Err(raw) = self.build_inputs() {
            let (outcome, detail) = outcome::marked(&raw, None);
            results.records.push(self.record(None, outcome, format!("inputs: {detail}")));
            return results;
        }
        let linkers = self.matrix.linkers_of(self.tools);
        let mut references = Vec::new();
        for linker in linkers.iter().copied().filter(|linker| linker.is_reference()) {
            let marker = self.test.marker(self.arch(), linker);
            if let Some(skip) = marker.filter(|m| m.kind == MarkerKind::Skip) {
                results.records.push(self.record(Some(linker), Outcome::Skip, &skip.reason));
                references.push(Reference {
                    linker,
                    outcome: None,
                    detail: String::new(),
                    printed: Vec::new(),
                });
                continue;
            }
            let (raw, printed) = self.run_linker(linker, None);
            let (outcome, detail) = outcome::marked(&raw, marker.map(|m| m.reason.as_str()));
            results.records.push(self.record(Some(linker), outcome, detail.clone()));
            references.push(Reference { linker, outcome: Some(outcome), detail, printed });
        }
        let verdict = outcome::self_check(&references);
        if let Verdict::Refuted(why) = &verdict {
            results.records.push(self.record(None, Outcome::Oracle, why.clone()));
            return results;
        }
        if linkers.contains(&Linker::Mortar) {
            results.records.push(self.judge_mortar(&verdict, &references));
        }
        results
    }

    /// Mortar's record, once the references have had their say.
    fn judge_mortar(&self, verdict: &Verdict, references: &[Reference]) -> Record {
        let mortar = Some(Linker::Mortar);
        let marker = self.test.marker(self.arch(), Linker::Mortar);
        if let Some(skip) = marker.filter(|m| m.kind == MarkerKind::Skip) {
            return self.record(mortar, Outcome::Skip, &skip.reason);
        }
        let expected = match verdict {
            Verdict::Unproven => {
                return self.record(
                    mortar,
                    Outcome::Skip,
                    "not established: no reference linker of the run passes the test without a \
                     marker",
                );
            }
            Verdict::Unchecked if self.test.compares_output() => {
                return self.record(
                    mortar,
                    Outcome::Skip,
                    "the expected output is the references' and no reference linker is in the run",
                );
            }
            Verdict::Unchecked | Verdict::Refuted(_) => Vec::new(),
            Verdict::Established => references
                .iter()
                .find(|r| matches!(r.outcome, Some(Outcome::Pass | Outcome::Xpass)))
                .map(|r| r.printed.clone())
                .unwrap_or_default(),
        };
        let (raw, _) = self.run_linker(Linker::Mortar, Some(&expected));
        let (outcome, detail) = outcome::marked(&raw, marker.map(|m| m.reason.as_str()));
        self.record(mortar, outcome, detail)
    }

    /// Writes the embedded files to `<dir>/inputs` and runs the steps that
    /// build inputs there.
    fn build_inputs(&self) -> Result<(), Raw> {
        let inputs = self.dir.join("inputs");
        let error = |e: std::io::Error, path: &Path| {
            Raw::Error(format!("cannot write '{}': {e}", path.display()))
        };
        std::fs::create_dir_all(&inputs).map_err(|e| error(e, &inputs))?;
        for file in &self.test.files {
            let path = inputs.join(&file.name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| error(e, parent))?;
            }
            std::fs::write(&path, &file.contents).map_err(|e| error(e, &path))?;
        }
        let vars = Vars { arch: self.arch(), host: self.matrix.host, linker: None };
        for step in &self.test.steps {
            if !step.is_build() || !step.scope.has_arch(self.arch()) {
                continue;
            }
            let args = expand(build_args(step), vars);
            let (program, mut full) = self.build_tool(step)?;
            full.extend(args.into_iter().map(OsString::from));
            let output = spawn(step, &program, &full, &inputs, &[], TOOL_LIMITS)?;
            if !output.end.success() {
                return Err(Raw::Error(format!(
                    "{}: {}: {}",
                    step.describe(),
                    output.end,
                    first_error(&output)
                )));
            }
        }
        Ok(())
    }

    /// The program of a build step and the arguments the harness puts
    /// before the test's.
    fn build_tool(&self, step: &Step) -> Result<(PathBuf, Vec<OsString>), Raw> {
        let missing = || Raw::Error(format!("{}: its tool is not there", step.describe()));
        let common = &self.matrix.common;
        Ok(match &step.kind {
            StepKind::Compile(compile) => {
                let program = match (compile.family, compile.language) {
                    (Family::Gcc, Language::C | Language::Asm) => Some(&self.tools.gcc),
                    (Family::Gcc, Language::Cxx) => self.tools.gxx.as_ref(),
                    (Family::Gcc, Language::Fortran) => self.tools.gfortran.as_ref(),
                    (Family::Clang, Language::Cxx) => common.clangxx.as_ref(),
                    (Family::Clang, _) => common.clang.as_ref(),
                };
                let mut before = Vec::new();
                if compile.family == Family::Clang {
                    before.push(format!("--target={}", self.arch().triple()).into());
                }
                before.push("-c".into());
                (program.ok_or_else(missing)?.clone(), before)
            }
            StepKind::Yaml2obj(_) => (common.yaml2obj.clone().ok_or_else(missing)?, Vec::new()),
            StepKind::Ar(_) => (self.tools.ar.clone(), Vec::new()),
            StepKind::Link(_) | StepKind::Run(_) | StepKind::Check(_) => {
                unreachable!("{} does not build an input", step.describe())
            }
        })
    }

    /// Runs the link, run and check steps for `linker` in a copy of the
    /// inputs. `expected` is what the programs must print where the test
    /// leaves that to the references (mortar's run); without it, what they
    /// print is returned.
    fn run_linker(&self, linker: Linker, expected: Option<&[Printed]>) -> (Raw, Vec<Printed>) {
        let dir = self.dir.join(linker.name());
        let mut printed = Vec::new();
        if let Err(e) = copy_dir(&self.dir.join("inputs"), &dir) {
            return (Raw::Error(e), printed);
        }
        let vars = Vars { arch: self.arch(), host: self.matrix.host, linker: Some(linker) };
        for step in &self.test.steps {
            if step.is_build() || !step.scope.applies(self.arch(), linker) {
                continue;
            }
            let result = match &step.kind {
                StepKind::Link(link) => self.link(step, link, linker, &dir, vars),
                StepKind::Run(run) => {
                    self.run_program(step, run, &dir, vars, expected, &mut printed)
                }
                StepKind::Check(check) => self.check(step, check, linker, &dir, vars),
                StepKind::Compile(_) | StepKind::Yaml2obj(_) | StepKind::Ar(_) => {
                    unreachable!("build steps are run once, before the linkers")
                }
            };
            if let Err(raw) = result {
                return (raw, printed);
            }
        }
        (Raw::Pass, printed)
    }

    fn link(
        &self,
        step: &Step,
        link: &Link,
        linker: Linker,
        dir: &Path,
        vars: Vars,
    ) -> Result<(), Raw> {
        let args = expand(&link.args, vars);
        let output_file = dir.join(output_name(&args));
        if link.fails && output_file.symlink_metadata().is_ok() {
            std::fs::remove_file(&output_file).map_err(|e| {
                Raw::Error(format!("cannot remove '{}': {e}", output_file.display()))
            })?;
        }
        let status_file = dir.join(format!(".linktest-ld-status-{}", step.line));
        let wrapper = self.matrix.wrapper_dir(self.arch(), linker);
        let common = &self.matrix.common;
        let (program, mut full): (Option<&PathBuf>, Vec<OsString>) = match link.driver {
            Driver::Ld => (None, Vec::new()),
            Driver::Gcc | Driver::Gxx | Driver::Gfortran => {
                let program = match link.driver {
                    Driver::Gcc => Some(&self.tools.gcc),
                    Driver::Gxx => self.tools.gxx.as_ref(),
                    _ => self.tools.gfortran.as_ref(),
                };
                let mut b = OsString::from("-B");
                b.push(wrapper.as_os_str());
                b.push("/");
                (program, vec![b])
            }
            Driver::Clang | Driver::Clangxx => {
                let program = if link.driver == Driver::Clang {
                    common.clang.as_ref()
                } else {
                    common.clangxx.as_ref()
                };
                let mut ld_path = OsString::from("--ld-path=");
                ld_path.push(wrapper.join("ld").as_os_str());
                (program, vec![format!("--target={}", self.arch().triple()).into(), ld_path])
            }
        };
        let program: PathBuf = match (link.driver, program) {
            (Driver::Ld, _) => match self.matrix.linker_program(self.tools, linker) {
                Some(path) => path.to_path_buf(),
                None => {
                    return Err(Raw::Error(format!("{}: no {}", step.describe(), linker.name())));
                }
            },
            (_, Some(path)) => path.clone(),
            (driver, None) => {
                return Err(Raw::Error(format!("{}: no {}", step.describe(), driver.name())));
            }
        };
        full.extend(args.into_iter().map(OsString::from));
        let limits = self.limits(LINK_LIMITS);
        let output =
            spawn(step, &program, &full, dir, &[(STATUS_VAR, status_file.as_os_str())], limits)?;
        // The linker's own end: the driver's wrapper recorded it.
        let linker_end = if link.driver == Driver::Ld {
            output.end
        } else {
            let recorded = std::fs::read_to_string(&status_file).unwrap_or_default();
            match recorded.lines().last().and_then(|line| line.trim().parse::<i32>().ok()) {
                Some(status) if status > 128 => End::Signal(status - 128),
                Some(status) => End::Exit(status),
                None => {
                    return Err(Raw::Error(format!(
                        "{}: the driver did not run the linker: {}: {}",
                        step.describe(),
                        output.end,
                        first_error(&output)
                    )));
                }
            }
        };
        let fail = |what: &dyn Display| Raw::Fail(format!("{}: {what}", step.describe()));
        if link.fails {
            match linker_end {
                End::Exit(0) => {
                    return Err(fail(&"the link succeeded; the test expects it to fail"));
                }
                End::Signal(signal) => {
                    return Err(fail(&format_args!(
                        "the linker ended by signal {signal}; a failed link exits with a status"
                    )));
                }
                End::Exit(_) | End::Killed(_) => {}
            }
            if output_file.symlink_metadata().is_ok() {
                return Err(fail(&format_args!(
                    "the failed link left its output file '{}'",
                    output_name_of(&output_file)
                )));
            }
        } else if !linker_end.success() || !output.end.success() {
            let end = if linker_end.success() { output.end } else { linker_end };
            return Err(fail(&format_args!("{end}: {}", first_error(&output))));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        self.expect(&link.diags, linker, &stderr, "the link's standard error")
            .map_err(|what| Raw::Fail(format!("{}: {what}", step.describe())))?;
        // L3 (WP-028): the verifier, mortar_plumb, checks every output a
        // link step produced, by any linker, here.
        Ok(())
    }

    fn run_program(
        &self,
        step: &Step,
        run: &Run,
        dir: &Path,
        vars: Vars,
        expected: Option<&[Printed]>,
        printed: &mut Vec<Printed>,
    ) -> Result<(), Raw> {
        let fail = |what: &dyn Display| Raw::Fail(format!("{}: {what}", step.describe()));
        let name = run.program.expand(vars);
        let path = dir.join(&name);
        if !path.is_file() {
            return Err(fail(&format_args!("there is no program '{name}'")));
        }
        let args = expand(&run.args, vars);
        let mut limits = self.limits(RUN_LIMITS);
        let (program, mut full) = match &self.tools.runner {
            Runner::Native => {
                // `/bin/sh` runs a file the kernel refuses as a script.
                let mut magic = [0u8; 4];
                let is_elf = std::fs::File::open(&path)
                    .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut magic))
                    .is_ok()
                    && magic == *b"\x7fELF";
                if !is_elf {
                    return Err(fail(&format_args!("'{name}' is not an ELF file")));
                }
                (path.clone(), Vec::new())
            }
            Runner::Emulated { qemu, sysroot } => {
                limits.time *= EMULATED_TIME_FACTOR;
                if self.arch() == Arch::I386 {
                    limits.memory_kib += I386_GUEST_KIB;
                }
                (qemu.clone(), vec!["-L".into(), sysroot.clone().into_os_string(), path.into()])
            }
        };
        full.extend(args.into_iter().map(OsString::from));
        let output = spawn(step, &program, &full, dir, &[], limits)?;
        let met = match (run.exit, output.end) {
            (Exit::Status(want), End::Exit(got)) | (Exit::Signal(want), End::Signal(got)) => {
                want == got
            }
            _ => false,
        };
        if !met {
            let want = match run.exit {
                Exit::Status(status) => End::Exit(status),
                Exit::Signal(signal) => End::Signal(signal),
            };
            return Err(fail(&format_args!(
                "{}, expected {want}{}",
                output.end,
                stderr_note(&output)
            )));
        }
        match &run.stdout {
            Stdout::Exact(lines) => {
                let want: String = lines.iter().map(|line| line.expand(vars) + "\n").collect();
                if output.stdout != want.as_bytes() {
                    return Err(fail(&format_args!(
                        "printed {}, expected {}",
                        outcome::quoted(&output.stdout),
                        outcome::quoted(want.as_bytes())
                    )));
                }
            }
            Stdout::Reference => match expected {
                None => printed.push(Printed { line: step.line, stdout: output.stdout }),
                Some(expected) => {
                    let Some(want) = expected.iter().find(|p| p.line == step.line) else {
                        return Err(Raw::Error(format!(
                            "{}: no reference output to compare with",
                            step.describe()
                        )));
                    };
                    if output.stdout != want.stdout {
                        return Err(fail(&format_args!(
                            "printed {}, the references printed {}",
                            outcome::quoted(&output.stdout),
                            outcome::quoted(&want.stdout)
                        )));
                    }
                }
            },
        }
        Ok(())
    }

    fn check(
        &self,
        step: &Step,
        check: &Check,
        linker: Linker,
        dir: &Path,
        vars: Vars,
    ) -> Result<(), Raw> {
        let fail = |what: &dyn Display| Raw::Fail(format!("{}: {what}", step.describe()));
        let args = expand(&check.args, vars);
        let shown = format!("{} {}", check.tool.name(), args.join(" "));
        let text = if check.tool == CheckTool::Text {
            let path = dir.join(&args[0]);
            match std::fs::read(&path) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(e) => return Err(fail(&format_args!("cannot read '{}': {e}", args[0]))),
            }
        } else {
            let program = match check.tool {
                CheckTool::Readelf => Some(&self.tools.readelf),
                CheckTool::Nm => Some(&self.tools.nm),
                CheckTool::Objdump => Some(&self.tools.objdump),
                CheckTool::LlvmReadelf => self.matrix.common.llvm_readelf.as_ref(),
                CheckTool::Text => unreachable!("read above"),
            };
            let Some(program) = program else {
                return Err(Raw::Error(format!("{}: its tool is not there", step.describe())));
            };
            let full: Vec<OsString> = args.iter().map(OsString::from).collect();
            let output = spawn(step, program, &full, dir, &[], TOOL_LIMITS)?;
            if !output.end.success() {
                return Err(fail(&format_args!(
                    "`{shown}`: {}: {}",
                    output.end,
                    first_error(&output)
                )));
            }
            String::from_utf8_lossy(&output.stdout).into_owned()
        };
        self.expect(&check.expectations, linker, &text, &format!("`{shown}`"))
            .map_err(|what| fail(&what))
    }

    /// Checks the expectations that apply to `linker` against the lines
    /// of `text` (`what` names it in a message).
    fn expect(
        &self,
        expectations: &[Expectation],
        linker: Linker,
        text: &str,
        what: &str,
    ) -> Result<(), String> {
        let vars = Vars { arch: self.arch(), host: self.matrix.host, linker: Some(linker) };
        for expectation in expectations.iter().filter(|e| e.scope.applies(self.arch(), linker)) {
            let source = expectation.pattern.expand_pattern(vars);
            let pattern =
                Pattern::parse(&source).map_err(|e| format!("line {}: {e}", expectation.line))?;
            let lines = || text.lines().map(|line| line.strip_suffix('\r').unwrap_or(line));
            let hit = lines().find(|line| pattern.matches_line(line));
            match (expectation.present, hit) {
                (true, None) => {
                    return Err(format!(
                        "line {}: no line of {what} matches '{pattern}'{}",
                        expectation.line,
                        first_lines(text)
                    ));
                }
                (false, Some(line)) => {
                    return Err(format!(
                        "line {}: {what} has '{}', which matches '{pattern}'",
                        expectation.line,
                        line.trim()
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The limits of a link or run step: `base`, or the test's own.
    fn limits(&self, base: Limits) -> Limits {
        Limits {
            time: self.test.timeout.unwrap_or(base.time),
            memory_kib: self.test.memory_mib.map_or(base.memory_kib, |mib| mib << 10),
        }
    }
}

/// The arguments of a build step.
fn build_args(step: &Step) -> &[Template] {
    match &step.kind {
        StepKind::Compile(compile) => &compile.args,
        StepKind::Yaml2obj(args) | StepKind::Ar(args) => args,
        StepKind::Link(_) | StepKind::Run(_) | StepKind::Check(_) => &[],
    }
}

fn expand(templates: &[Template], vars: Vars) -> Vec<String> {
    templates.iter().map(|template| template.expand(vars)).collect()
}

/// Runs a step's program in `dir`, in the C locale (diagnostics are
/// matched as the tools print them there). A limit is [`Raw::Killed`],
/// trouble of the harness [`Raw::Error`].
fn spawn(
    step: &Step,
    program: &Path,
    args: &[OsString],
    dir: &Path,
    env: &[(&str, &std::ffi::OsStr)],
    limits: Limits,
) -> Result<Output, Raw> {
    let output = process::run(
        program,
        |command| {
            command.args(args).current_dir(dir).env("LC_ALL", "C");
            for (name, value) in env {
                command.env(name, value);
            }
        },
        b"",
        limits,
    )
    .map_err(|e| Raw::Error(format!("{}: {e}", step.describe())))?;
    if output.end.killed().is_some() {
        return Err(Raw::Killed(format!("{}: {}", step.describe(), output.end)));
    }
    Ok(output)
}

/// The output file a link line names: the last `-o <file>`, `-o<file>`,
/// `--output <file>` or `--output=<file>`, else `a.out`.
pub fn output_name(args: &[String]) -> String {
    let mut name = "a.out".to_owned();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "-o" || arg == "--output" {
            if let Some(next) = rest.next() {
                name.clone_from(next);
            }
        } else if let Some(value) = arg.strip_prefix("--output=") {
            value.clone_into(&mut name);
        } else if let Some(value) = arg.strip_prefix("-o").filter(|v| !v.is_empty()) {
            value.clone_into(&mut name);
        }
    }
    name
}

fn output_name_of(path: &Path) -> String {
    path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

/// The line of a child's standard error that says what went wrong: the
/// first that has `error`, else the first that is not empty.
fn first_error(output: &Output) -> String {
    let text = String::from_utf8_lossy(&output.stderr);
    text.lines()
        .map(str::trim)
        .find(|line| line.to_ascii_lowercase().contains("error"))
        .map_or_else(|| output.first_error_line(), str::to_owned)
}

/// `: <first error line>` when the child said anything on standard error.
fn stderr_note(output: &Output) -> String {
    let line = first_error(output);
    if line.is_empty() { String::new() } else { format!(": {line}") }
}

/// The first lines of a text, for a message about what it did not have.
fn first_lines(text: &str) -> String {
    const SHOWN: usize = 4;
    let lines: Vec<&str> =
        text.lines().map(str::trim).filter(|line| !line.is_empty()).take(SHOWN).collect();
    if lines.is_empty() {
        " (it is empty)".to_owned()
    } else {
        format!(" (it starts: {})", lines.join(" | "))
    }
}

/// Copies the directory `from` to `to`, recursively.
fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    let cannot = |what: &str, path: &Path, e: std::io::Error| {
        format!("cannot {what} '{}': {e}", path.display())
    };
    std::fs::create_dir_all(to).map_err(|e| cannot("create", to, e))?;
    let entries = std::fs::read_dir(from).map_err(|e| cannot("read", from, e))?;
    let mut names = Vec::new();
    for entry in entries {
        names.push(entry.map_err(|e| cannot("read", from, e))?.file_name());
    }
    names.sort();
    for name in names {
        let (source, target) = (from.join(&name), to.join(&name));
        let kind = source.symlink_metadata().map_err(|e| cannot("read", &source, e))?;
        if kind.is_dir() {
            copy_dir(&source, &target)?;
        } else {
            std::fs::copy(&source, &target).map_err(|e| cannot("copy", &source, e))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::output_name;

    // spec/harness.md "Link tests", "Steps": the output file of a link.
    #[test]
    fn the_output_file_of_a_link_line() {
        let name =
            |args: &[&str]| output_name(&args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>());
        assert_eq!(name(&["a.o"]), "a.out");
        assert_eq!(name(&["a.o", "-o", "prog"]), "prog");
        assert_eq!(name(&["-oprog", "a.o"]), "prog");
        assert_eq!(name(&["--output", "x", "a.o", "--output=y"]), "y");
        assert_eq!(name(&["-o", "x", "-Wl,-Map,m", "-o", "z"]), "z");
    }
}
