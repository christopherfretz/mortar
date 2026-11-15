//! Running one test on one architecture (`spec/harness.md` "Link tests":
//! "Running a test", "Steps", "The self-check", "Limits").
//!
//! A unit of a run is a test on an architecture. Its inputs are built once,
//! in `<dir>/inputs`; each linker gets a copy of them in `<dir>/<linker>`
//! and runs the link, run and check steps there. The reference linkers go
//! first; mortar is judged only when they establish the test.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt::{Display, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::format::{
    Anomaly, ArchXfail, Check, CheckTool, Driver, Exit, Expectation, Language, Link, Marker,
    MarkerKind, Run, Stdout, Step, StepKind, Template, Test, Vars,
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
    /// Whether L3 runs: the verifier on every link's output.
    pub plumb: bool,
    /// The run-wide default anomalies, `tests/link/anomalies`
    /// (`spec/harness.md` "L3").
    pub defaults: Vec<Anomaly>,
    /// The run-wide mortar `xfail`s by architecture, `tests/link/xfails`
    /// (`spec/harness.md` "Markers").
    pub xfails: Vec<ArchXfail>,
    /// The digest mode of `cargo xtask static-identical` (`spec/harness.md`
    /// "static-identical"): mortar alone, its link steps only, each
    /// recorded as a [`DigestRow`]; compile steps get
    /// `-ffile-prefix-map=<inputs>=.` so that no object names the work
    /// directory.
    pub digest: bool,
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
    pub plumb: Plumb,
    /// The digest mode's rows, in step order.
    pub digests: Vec<DigestRow>,
}

/// One mortar link of the digest mode (`spec/harness.md`
/// "static-identical"): how the linker ended and the SHA-256 of what it
/// wrote, `-` for a file it did not write. The map and the standard error
/// are digested with the work directory's paths replaced (`.` for the
/// link's directory, `<unit>`, `<work>`), so that a baseline does not
/// depend on where it was cut.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DigestRow {
    pub test: String,
    pub arch: Arch,
    /// The link step's line in the test file.
    pub line: usize,
    pub end: String,
    pub output: String,
    pub map: String,
    pub stderr: String,
}

/// The SHA-256 of `bytes`, in hex.
pub fn digest_hex(bytes: &[u8]) -> String {
    mortar_digest::sha256(bytes).iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// `text` with each `(path, name)` of `subs` replaced, in order.
pub fn normalise(text: &[u8], subs: &[(String, &str)]) -> Vec<u8> {
    let mut text = String::from_utf8_lossy(text).into_owned();
    for (path, name) in subs {
        if !path.is_empty() {
            text = text.replace(path.as_str(), name);
        }
    }
    text.into_bytes()
}

/// What L3, the verifier on every link's output, did (`spec/harness.md`
/// "L3").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plumb {
    /// The outputs the verifier judged.
    pub verified: u64,
    /// The findings a test's `anomaly` directive expected.
    pub anomalies: u64,
    /// The outputs it does not judge, by reason.
    pub not_judged: BTreeMap<&'static str, u64>,
    /// The time spent in the verifier, summed over the threads.
    pub time: Duration,
    /// The architectures and linkers of the judged outputs.
    pub judged_by: BTreeSet<(Arch, Linker)>,
    /// The default anomalies (`Matrix::defaults`) found, by architecture
    /// and linker.
    pub defaults_seen: BTreeSet<(Arch, Linker, String)>,
}

impl Plumb {
    pub fn add(&mut self, other: &Plumb) {
        self.verified += other.verified;
        self.anomalies += other.anomalies;
        for (reason, n) in &other.not_judged {
            *self.not_judged.entry(reason).or_default() += n;
        }
        self.time += other.time;
        self.judged_by.extend(other.judged_by.iter().copied());
        self.defaults_seen.extend(other.defaults_seen.iter().cloned());
    }

    /// The default anomalies that apply to a judged output of the run and
    /// that no judged output of that architecture and linker has: each one
    /// a stale default (`spec/harness.md` "L3").
    pub fn stale_defaults(&self, defaults: &[Anomaly]) -> Vec<String> {
        let mut stale = Vec::new();
        for &(arch, linker) in &self.judged_by {
            for d in defaults.iter().filter(|d| d.scope.applies(arch, linker)) {
                for id in &d.ids {
                    if !self.defaults_seen.contains(&(arch, linker, id.clone())) {
                        stale.push(format!(
                            "L3: tests/link/anomalies line {} expects {id} of {} on {}, and the \
                             verifier found it on none of the run's outputs",
                            d.line,
                            linker.name(),
                            arch.name()
                        ));
                    }
                }
            }
        }
        stale.dedup();
        stale
    }

    /// The summary's note.
    pub fn note(&self) -> String {
        let mut text = format!(
            "L3: the verifier judged {} link outputs in {:.1} s",
            self.verified,
            self.time.as_secs_f64()
        );
        if self.anomalies > 0 {
            let _ = write!(text, ", {} findings expected as anomalies", self.anomalies);
        }
        if !self.not_judged.is_empty() {
            let parts: Vec<String> =
                self.not_judged.iter().map(|(reason, n)| format!("{reason} {n}")).collect();
            let _ = write!(text, "; not judged: {}", parts.join(", "));
        }
        text
    }
}

/// Why the verifier does not judge an output (`spec/harness.md` "L3"),
/// from its ELF header and program headers; `None` when it does.
pub fn not_judged(bytes: &[u8]) -> Option<&'static str> {
    const ET_REL: u16 = 1;
    const ET_EXEC: u16 = 2;
    const EM_AARCH64: u16 = 183;
    const EM_X86_64: u16 = 62;
    const EM_386: u16 = 3;
    const PT_DYNAMIC: u32 = 2;
    const PT_INTERP: u32 = 3;
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at =
        |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let u64_at = |at: usize| {
        bytes.get(at..at + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
    };
    // Not ELF, or not little-endian: the verifier's own S1 says which.
    if bytes.get(..4) != Some(b"\x7fELF".as_slice()) || bytes.get(5) != Some(&1) {
        return if bytes.get(..4) == Some(b"\x7fELF".as_slice()) { Some("machine") } else { None };
    }
    // AArch64 and x86-64 (ELFCLASS64) and i386 (ELFCLASS32) have tables
    // (spec/plumb.md §4.5).
    let class64 = match (bytes.get(4), u16_at(18)) {
        (Some(2), Some(EM_AARCH64 | EM_X86_64)) => true,
        (Some(1), Some(EM_386)) => false,
        _ => return Some("machine"),
    };
    match u16_at(16) {
        Some(ET_EXEC) => {}
        Some(ET_REL) => return Some("relocatable"),
        _ => return Some("pie-or-shared"),
    }
    let (phoff, entsize, phnum) = if class64 {
        (usize::try_from(u64_at(32)?).ok()?, u16_at(54)?, u16_at(56)?)
    } else {
        (usize::try_from(u32_at(28)?).ok()?, u16_at(42)?, u16_at(44)?)
    };
    let entsize = usize::from(entsize);
    let dynamic = (0..usize::from(phnum)).any(|i| {
        let at = phoff.saturating_add(i.saturating_mul(entsize));
        matches!(u32_at(at), Some(PT_DYNAMIC | PT_INTERP))
    });
    dynamic.then_some("dynamic")
}

/// The map file a link line writes already (`-Map=X`, `-Map X`,
/// `-Wl,-Map=X`, `-Wl,-Map,X`, and the same with `--Map`).
pub fn map_option(args: &[String]) -> Option<String> {
    let mut found = None;
    let mut next_is_map = false;
    for arg in args {
        if next_is_map {
            found = Some(arg.clone());
            next_is_map = false;
            continue;
        }
        let words: Vec<&str> = match arg.strip_prefix("-Wl,") {
            Some(list) => list.split(',').collect(),
            None => vec![arg.as_str()],
        };
        let mut words = words.into_iter();
        while let Some(word) = words.next() {
            let word = word.strip_prefix('-').unwrap_or(word);
            if let Some(file) = word.strip_prefix("-Map=").or_else(|| word.strip_prefix("Map=")) {
                found = Some(file.to_owned());
            } else if word == "-Map" || word == "Map" {
                match words.next() {
                    Some(file) => found = Some(file.to_owned()),
                    None => next_is_map = !arg.starts_with("-Wl,"),
                }
            }
        }
    }
    found
}

/// What L3 learned on one linker's steps of a unit.
#[derive(Default)]
struct Judged {
    plumb: Plumb,
    /// The anomaly ids seen and expected.
    seen: BTreeSet<String>,
    /// The digest mode's rows.
    digests: Vec<DigestRow>,
}

/// The anomaly ids of `list` that apply to `arch` and `linker`.
fn anomaly_ids(list: &[Anomaly], arch: Arch, linker: Linker) -> Vec<&str> {
    list.iter()
        .filter(|a| a.scope.applies(arch, linker))
        .flat_map(|a| a.ids.iter().map(String::as_str))
        .collect()
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
        if self.matrix.digest {
            self.run_digests(&mut results);
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
            let (raw, printed) = self.run_linker(linker, None, &mut results.plumb);
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
            let record = self.judge_mortar(&verdict, &references, &mut results.plumb);
            results.records.push(record);
        }
        results
    }

    /// The digest mode: mortar's link steps alone, whatever the
    /// references or the markers would say, but a `skip(mortar)` (a test
    /// that mortar cannot run there). A link that fails unexpectedly ends
    /// the unit, as it would a linktest run.
    fn run_digests(&self, results: &mut Results) {
        let mortar = Some(Linker::Mortar);
        let marker = self.test.marker(self.arch(), Linker::Mortar);
        if let Some(skip) = marker.filter(|m| m.kind == MarkerKind::Skip) {
            results.records.push(self.record(mortar, Outcome::Skip, &skip.reason));
            return;
        }
        let mut judged = Judged::default();
        let (raw, _) = self.run_steps(Linker::Mortar, None, &mut judged);
        let (outcome, detail) = outcome::marked(&raw, None);
        results.records.push(self.record(mortar, outcome, detail));
        results.digests = judged.digests;
    }

    /// Mortar's record, once the references have had their say.
    fn judge_mortar(
        &self,
        verdict: &Verdict,
        references: &[Reference],
        plumb: &mut Plumb,
    ) -> Record {
        let mortar = Some(Linker::Mortar);
        let marker = self.test.marker(self.arch(), Linker::Mortar);
        if let Some(skip) = marker.filter(|m| m.kind == MarkerKind::Skip) {
            return self.record(mortar, Outcome::Skip, &skip.reason);
        }
        let xfail = mortar_xfail(marker, &self.matrix.xfails, self.arch());
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
        let (raw, _) = self.run_linker(Linker::Mortar, Some(&expected), plumb);
        let (outcome, detail) = outcome::marked(&raw, xfail);
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
                if self.matrix.digest {
                    let mut map = OsString::from("-ffile-prefix-map=");
                    map.push(self.dir.join("inputs").as_os_str());
                    map.push("=.");
                    before.push(map);
                }
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
    fn run_linker(
        &self,
        linker: Linker,
        expected: Option<&[Printed]>,
        plumb: &mut Plumb,
    ) -> (Raw, Vec<Printed>) {
        let mut judged = Judged::default();
        let result = self.run_steps(linker, expected, &mut judged);
        plumb.add(&judged.plumb);
        let (raw, printed) = result;
        if raw != Raw::Pass || judged.plumb.verified == 0 {
            return (raw, printed);
        }
        // An expected anomaly that no judged output has is stale.
        let stale: Vec<&str> = self
            .test
            .anomalies
            .iter()
            .filter(|a| a.scope.applies(self.arch(), linker))
            .flat_map(|a| a.ids.iter().map(String::as_str))
            .filter(|id| !judged.seen.contains(*id))
            .collect();
        if stale.is_empty() {
            (raw, printed)
        } else {
            let detail = format!(
                "L3: the anomaly {} is declared, and the verifier did not find it",
                stale.join(", ")
            );
            (Raw::Fail(detail), printed)
        }
    }

    fn run_steps(
        &self,
        linker: Linker,
        expected: Option<&[Printed]>,
        judged: &mut Judged,
    ) -> (Raw, Vec<Printed>) {
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
            if self.matrix.digest && !matches!(step.kind, StepKind::Link(_)) {
                continue;
            }
            let result = match &step.kind {
                StepKind::Link(link) => self.link(step, link, linker, &dir, vars, judged),
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

    #[allow(clippy::too_many_lines)] // the driver, the end, the diagnostics, then L3
    fn link(
        &self,
        step: &Step,
        link: &Link,
        linker: Linker,
        dir: &Path,
        vars: Vars,
        judged: &mut Judged,
    ) -> Result<(), Raw> {
        let mut args = expand(&link.args, vars);
        let output_file = dir.join(output_name(&args));
        // L3: a link that is to succeed writes a map, for the verifier:
        // mortar's machine map is `--mortar-map` (image.md I-120), whatever
        // `-Map` the test asks for; a reference's is its `-Map`.
        let map_file = if link.fails || !(self.matrix.plumb || self.matrix.digest) {
            None
        } else if let Some(map) = map_option(&args).filter(|_| linker != Linker::Mortar) {
            Some(dir.join(map))
        } else {
            let name = format!(".linktest-plumb-{}.map", step.line);
            let option = if linker == Linker::Mortar { "--mortar-map" } else { "-Map" };
            args.push(if link.driver == Driver::Ld {
                format!("{option}={name}")
            } else {
                format!("-Wl,{option}={name}")
            });
            Some(dir.join(name))
        };
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
        if self.matrix.digest {
            judged.digests.push(self.digest_row(step, linker_end, &output, &output_file, dir));
        }
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
        match map_file {
            Some(map) if self.matrix.plumb => {
                self.verify(step, linker, &output_file, &map, dir, judged)
            }
            _ => Ok(()),
        }
    }

    /// The digest mode's row of a link step that ran in `dir`.
    fn digest_row(
        &self,
        step: &Step,
        end: End,
        output: &Output,
        output_file: &Path,
        dir: &Path,
    ) -> DigestRow {
        let work = self.matrix.wrappers.parent().unwrap_or(&self.matrix.wrappers);
        let subs = [
            (dir.display().to_string(), "."),
            (self.dir.display().to_string(), "<unit>"),
            (work.display().to_string(), "<work>"),
        ];
        let file = |path: &Path, text: bool| match std::fs::read(path) {
            Ok(bytes) if text => digest_hex(&normalise(&bytes, &subs)),
            Ok(bytes) => digest_hex(&bytes),
            Err(_) => "-".to_owned(),
        };
        let map = dir.join(format!(".linktest-plumb-{}.map", step.line));
        DigestRow {
            test: self.name.to_owned(),
            arch: self.arch(),
            line: step.line,
            end: end.to_string(),
            output: file(output_file, false),
            map: file(&map, true),
            stderr: digest_hex(&normalise(&output.stderr, &subs)),
        }
    }

    /// L3 (`spec/harness.md` "L3"): the verifier, `mortar_plumb`, on the
    /// output of a link that succeeded. A finding fails the step unless it
    /// is a recorded anomaly (`spec/plumb.md` §10.2) that the test expects
    /// of this linker.
    fn verify(
        &self,
        step: &Step,
        linker: Linker,
        output_file: &Path,
        map_file: &Path,
        dir: &Path,
        judged: &mut Judged,
    ) -> Result<(), Raw> {
        let error = |what: &dyn Display| Raw::Error(format!("{}: L3: {what}", step.describe()));
        let bytes = std::fs::read(output_file)
            .map_err(|e| error(&format_args!("{}: {e}", output_file.display())))?;
        if let Some(reason) = not_judged(&bytes) {
            *judged.plumb.not_judged.entry(reason).or_default() += 1;
            return Ok(());
        }
        drop(bytes);
        let format = match linker {
            Linker::Bfd => mortar_plumb::Format::Gnu,
            Linker::Lld => mortar_plumb::Format::Lld,
            Linker::Mold => mortar_plumb::Format::Mold,
            Linker::Mortar => mortar_plumb::Format::Mortar,
        };
        let started = Instant::now();
        let verified = crate::plumb::verify_link(output_file, map_file, format, dir, None);
        judged.plumb.time += started.elapsed();
        let (bytes, report, _) = verified.map_err(|e| error(&e))?;
        judged.plumb.verified += 1;
        judged.plumb.judged_by.insert((self.arch(), linker));
        let expected = anomaly_ids(&self.test.anomalies, self.arch(), linker);
        // mortar never has a default, whatever the file says.
        let defaults = if linker == Linker::Mortar {
            Vec::new()
        } else {
            anomaly_ids(&self.matrix.defaults, self.arch(), linker)
        };
        let mut unexpected = Vec::new();
        for f in &report.findings {
            let anomaly = mortar_plumb::anomaly::classify(format, &bytes, f);
            let id = anomaly.map(mortar_plumb::anomaly::id);
            if let Some(id) = id.filter(|id| expected.contains(id) || defaults.contains(id)) {
                if expected.contains(&id) {
                    judged.seen.insert(id.to_owned());
                }
                if defaults.contains(&id) {
                    judged.plumb.defaults_seen.insert((self.arch(), linker, id.to_owned()));
                }
                judged.plumb.anomalies += 1;
                continue;
            }
            let at = f.addr.map(|a| format!(" at {a:#x}")).unwrap_or_default();
            let mut line = format!("{} {}{at}", f.rule.id(), f.message);
            if let Some(id) = id {
                let _ = write!(
                    line,
                    " (anomaly {id} of plumb.md §10.2; '#@ anomaly({}): {id}' expects it)",
                    linker.name()
                );
            }
            unexpected.push((id, line));
        }
        // A finding that is no recorded anomaly first, then the
        // undeclared anomalies by id.
        unexpected.sort_by_key(|(id, _)| (id.is_some(), id.map(str::to_owned)));
        let mut ids: Vec<&str> = unexpected.iter().filter_map(|(id, _)| *id).collect();
        ids.dedup();
        if unexpected.is_empty() {
            return Ok(());
        }
        let more = match unexpected.len() {
            1 => String::new(),
            n => format!(" (and {} more)", n - 1),
        };
        let undeclared = if ids.is_empty() {
            String::new()
        } else {
            format!("; undeclared anomalies: {}", ids.join(" "))
        };
        Err(Raw::Fail(format!(
            "{}: L3: the verifier finds {}: {}{more}{undeclared}",
            step.describe(),
            output_name_of(output_file),
            unexpected[0].1
        )))
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

/// The reason of the `xfail` that applies to mortar on `arch`: the run-wide
/// one of `tests/link/xfails` (mortar has no back end there), else the
/// test's own. A test's `skip` is taken before this.
fn mortar_xfail<'a>(
    marker: Option<&'a Marker>,
    xfails: &'a [ArchXfail],
    arch: Arch,
) -> Option<&'a str> {
    xfails
        .iter()
        .find(|x| x.arch == arch)
        .map(|x| x.reason.as_str())
        .or_else(|| marker.filter(|m| m.kind == MarkerKind::Xfail).map(|m| m.reason.as_str()))
}

#[cfg(test)]
mod tests {
    // "Markers": a run-wide mortar xfail takes the place of the test's
    // own xfail on its architecture, and nowhere else.
    #[test]
    fn a_run_wide_xfail_applies_to_mortar_on_its_architecture() {
        use super::super::format::Scope;
        use super::{Arch, ArchXfail, Marker, MarkerKind, mortar_xfail};
        let own = Marker {
            line: 1,
            kind: MarkerKind::Xfail,
            scope: Scope::default(),
            reason: "no linking yet (M1)".to_owned(),
        };
        let x = [ArchXfail { line: 3, arch: Arch::X86_64, reason: "no back end".to_owned() }];
        assert_eq!(mortar_xfail(Some(&own), &x, Arch::X86_64), Some("no back end"));
        assert_eq!(mortar_xfail(None, &x, Arch::X86_64), Some("no back end"));
        assert_eq!(mortar_xfail(Some(&own), &x, Arch::Aarch64), Some("no linking yet (M1)"));
        assert_eq!(mortar_xfail(None, &x, Arch::I386), None);
    }

    use super::{Arch, Linker, Plumb, map_option, not_judged, output_name};

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_owned()).collect()
    }

    // spec/harness.md "L3": the map a link line already writes.
    #[test]
    fn the_map_of_a_link_line() {
        let map = |args: &[&str]| map_option(&strings(args));
        assert_eq!(map(&["a.o", "-o", "p"]), None);
        assert_eq!(map(&["-Map=p.map", "a.o"]).as_deref(), Some("p.map"));
        assert_eq!(map(&["-Map", "p.map"]).as_deref(), Some("p.map"));
        assert_eq!(map(&["--Map=q"]).as_deref(), Some("q"));
        assert_eq!(map(&["-Wl,-Map=r", "a.o"]).as_deref(), Some("r"));
        assert_eq!(map(&["-Wl,--gc-sections,-Map,s"]).as_deref(), Some("s"));
        assert_eq!(map(&["-Wl,-Map", "a.o"]), None);
    }

    // spec/harness.md "L3", "The defaults": a default no judged output of
    // its linker and architecture shows is stale; one for a linker the run
    // did not judge is not.
    #[test]
    fn a_default_anomaly_goes_stale() {
        let defaults =
            super::super::format::parse_defaults("mold: A3 A9\nbfd: A10\nlld,x86_64: A1\n")
                .unwrap();
        let mut plumb = Plumb::default();
        plumb.judged_by.insert((Arch::Aarch64, Linker::Mold));
        plumb.judged_by.insert((Arch::Aarch64, Linker::Lld));
        plumb.defaults_seen.insert((Arch::Aarch64, Linker::Mold, "A9".to_owned()));
        let stale = plumb.stale_defaults(&defaults);
        assert_eq!(stale.len(), 1, "{stale:?}");
        assert!(stale[0].contains("line 1 expects A3 of mold on aarch64"), "{stale:?}");
        plumb.defaults_seen.insert((Arch::Aarch64, Linker::Mold, "A3".to_owned()));
        assert!(plumb.stale_defaults(&defaults).is_empty());
    }

    // spec/harness.md "L3": the outputs the verifier does not judge.
    #[test]
    fn what_the_verifier_does_not_judge() {
        // An ELF64 little-endian header, `e_type` and `e_machine` set, and
        // one program header at 64 of type `p_type`.
        let elf = |e_type: u16, machine: u16, p_type: u32| {
            let mut b = vec![0u8; 64 + 56];
            b[..6].copy_from_slice(b"\x7fELF\x02\x01");
            b[16..18].copy_from_slice(&e_type.to_le_bytes());
            b[18..20].copy_from_slice(&machine.to_le_bytes());
            b[32..40].copy_from_slice(&64u64.to_le_bytes());
            b[54..56].copy_from_slice(&56u16.to_le_bytes());
            b[56..58].copy_from_slice(&1u16.to_le_bytes());
            b[64..68].copy_from_slice(&p_type.to_le_bytes());
            b
        };
        assert_eq!(not_judged(&elf(2, 183, 1)), None);
        assert_eq!(not_judged(&elf(2, 183, 3)), Some("dynamic"));
        assert_eq!(not_judged(&elf(2, 183, 2)), Some("dynamic"));
        assert_eq!(not_judged(&elf(3, 183, 1)), Some("pie-or-shared"));
        assert_eq!(not_judged(&elf(1, 183, 1)), Some("relocatable"));
        assert_eq!(not_judged(&elf(2, 62, 1)), None);
        // i386 is judged (ELFCLASS32, EM_386); ELFCLASS32 of another
        // machine, or EM_386 in ELFCLASS64, is not.
        let mut elf32 = vec![0u8; 0x80];
        elf32[..4].copy_from_slice(b"\x7fELF");
        elf32[4] = 1;
        elf32[5] = 1;
        elf32[16] = 2;
        elf32[18] = 3;
        elf32[28] = 0x34;
        elf32[42] = 32;
        elf32[44] = 1;
        elf32[0x34] = 1;
        assert_eq!(not_judged(&elf32), None);
        elf32[0x34] = 3;
        assert_eq!(not_judged(&elf32), Some("dynamic"));
        elf32[18] = 40;
        assert_eq!(not_judged(&elf32), Some("machine"));
        assert_eq!(not_judged(&elf(2, 3, 1)), Some("machine"));
        // Not ELF: the verifier judges it (S1).
        assert_eq!(not_judged(b"#!/bin/sh"), None);
    }

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
