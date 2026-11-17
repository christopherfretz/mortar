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
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::format::{
    Anomaly, ArchXfail, Check, CheckTool, Driver, Exit, Expectation, Fixture,
    LINKED_FIXTURE_PREFIX, Language, Link, Marker, MarkerKind, Run, Stdout, Step, StepKind,
    Template, Test, Vars, Want,
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
    /// The mortar `xfail`s of `tests/link/xfails`, run-wide by
    /// architecture and by test (`spec/harness.md` "Markers").
    pub xfails: Vec<ArchXfail>,
    /// Whether the verifier has stage 3 ([`stage3_present`]): it judges
    /// dynamic outputs, which are otherwise not judged by the name
    /// [`STAGE3_ABSENT`].
    pub stage3: bool,
    /// The fixtures the tests of the run name, by name: parsed, or why not
    /// (`spec/harness.md` "Fixtures").
    pub fixtures: BTreeMap<String, Result<Fixture, String>>,
    /// Where fixtures are built, `<work>/fixtures/<arch>/<name>`.
    pub fixture_dir: PathBuf,
    /// The fixtures built in this run, each once per architecture.
    pub built: FixtureCache,
    /// The digest mode of `cargo xtask static-identical` (`spec/harness.md`
    /// "static-identical"): mortar alone, its link steps only, each
    /// recorded as a [`DigestRow`]; compile steps get
    /// `-ffile-prefix-map=<inputs>=.` so that no object names the work
    /// directory.
    pub digest: bool,
    /// Who makes the objects of compile and `meta` steps (WP-099).
    pub producer: Producer,
    /// kiln's and hod's builds, when [`Matrix::producer`] needs them.
    pub mason: Option<crate::mason::Mason>,
    /// A kiln that writes records (`requires: kiln`, the kiln producers;
    /// WP-134), or why there is none; `None` when nothing of the run
    /// needs one.
    pub kiln: Option<Result<crate::mason::KilnRecords, String>>,
    /// A hod that writes the `reloc-model` row (`requires: hod`; WP-129),
    /// or why there is none; `None` when nothing of the run needs one.
    pub hod: Option<Result<crate::mason::HodRows, String>>,
    /// The digest mode's option variants (WP-099): when not empty, each
    /// mortar link step runs once per variant, the variant's options after
    /// the test's (`-Wl,` before each for a driver link), each recorded
    /// as a [`DigestRow`] of its index; variant 0 last, and only its link
    /// is judged and leaves its output for the later steps.
    pub variants: Vec<Vec<String>>,
}

/// Who makes a test's objects (WP-099, `spec/landed/meta/WP-099.md`): the
/// compile steps of C and C++ and the `meta` steps that add a container.
/// Assembly, Fortran and every other step are the same for each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Producer {
    /// The architecture's compilers; `meta` by objcopy (the test
    /// producer's ELF form, `spec/meta.md` §12).
    #[default]
    Gcc,
    /// C++ by kiln and C by clang to LLVM IR (`-S -emit-llvm`, the step's
    /// options), then hod; `meta` by objcopy into hod's object.
    HodObjcopy,
    /// As [`Producer::HodObjcopy`], but `meta` writes the container into
    /// the object's IR (`cargo xtask meta write --into-ll`) and hod makes
    /// the object: the container is hod's (`spec/odr.md` §11, H-1).
    Hod,
    /// The architecture's compilers, the IR kept beside each object; the
    /// first, third, ... `meta` step as [`Producer::Hod`] does it, the
    /// others by objcopy into the compiler's object: one unit from each
    /// producer, hod's beside objects without records.
    Mixed,
    /// C and C++ by kiln itself (WP-134), its LLVM route (`opt`, `llc`):
    /// an object a `meta` step takes is compiled with
    /// `-fmason-metadata=<level>` ([`meta_level`]) and the `meta` step
    /// keeps kiln's container (the test producer's rows are not used);
    /// other objects have none, as with GCC. A `meta` step on any other
    /// object (assembly, a shared object's table) is the test producer's.
    Kiln,
    /// As [`Producer::Kiln`], by kiln's hod route (`-fkiln-backend=hod`,
    /// `KILN_HOD_STRICT=1`): hod makes the `-O0` objects, the LLVM route
    /// the others (kiln's own rule, `spec/driver.md` of kiln).
    KilnHod,
}

impl Producer {
    pub const ALL: [Producer; 6] = [
        Producer::Gcc,
        Producer::HodObjcopy,
        Producer::Hod,
        Producer::Mixed,
        Producer::Kiln,
        Producer::KilnHod,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Producer::Gcc => "gcc",
            Producer::HodObjcopy => "hod-objcopy",
            Producer::Hod => "hod",
            Producer::Mixed => "mixed",
            Producer::Kiln => "kiln",
            Producer::KilnHod => "kiln-hod",
        }
    }

    /// Whether kiln writes the containers ([`Producer::Kiln`],
    /// [`Producer::KilnHod`]).
    pub fn is_kiln(self) -> bool {
        matches!(self, Producer::Kiln | Producer::KilnHod)
    }

    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == word)
    }
}

/// The fixtures a run has built or is building, by name, architecture
/// and, for a linked fixture's libraries (`spec/harness.md` LT-4), the
/// linker that linked them: the first unit that needs one builds it, the
/// others wait for it.
pub type FixtureCache =
    Mutex<BTreeMap<(String, Arch, Option<Linker>), Arc<OnceLock<Result<PathBuf, Raw>>>>>;

/// The program that sets a natively run program's environment on its own
/// `exec` (`spec/harness.md` LT-6): the variables of a run step reach the
/// program, never the shell that sets its limits.
pub const ENV_PROGRAM: &str = "/usr/bin/env";

/// A pair of the pair part (`spec/harness.md` LT-3): the linker of the
/// library and the linker of the host linked against it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pair {
    pub library: Linker,
    pub host: Linker,
}

impl Pair {
    /// The pair's directory in the unit: the library linker's own
    /// directory for its own pair, `<library>+<host>` for another (a copy
    /// of the library linker's directory as its library part left it).
    pub fn dir_name(self) -> String {
        if self.library == self.host {
            self.library.name().to_owned()
        } else {
            format!("{}+{}", self.library.name(), self.host.name())
        }
    }

    /// The pairs of the record of `linker`, in the order they run (LT-3):
    /// for mortar, first each reference library of `passed` under a mortar
    /// host; then the linker's own pair and, unless the linker is GNU ld,
    /// its library under a GNU ld host (when `linkers`, the run's, has GNU
    /// ld). The digest mode has mortar's own pair alone.
    pub fn of(linker: Linker, linkers: &[Linker], passed: &[Linker], digest: bool) -> Vec<Pair> {
        let own = Pair { library: linker, host: linker };
        if digest {
            return vec![own];
        }
        let mut pairs = Vec::new();
        if linker == Linker::Mortar {
            pairs.extend(
                passed
                    .iter()
                    .filter(|r| r.is_reference())
                    .map(|&library| Pair { library, host: Linker::Mortar }),
            );
        }
        pairs.push(own);
        if linker != Linker::Bfd && linkers.contains(&Linker::Bfd) {
            pairs.push(Pair { library: linker, host: Linker::Bfd });
        }
        pairs
    }

    /// `[library mortar, host bfd]`: the pair in a result's detail.
    pub fn label(self) -> String {
        format!("[library {}, host {}]", self.library.name(), self.host.name())
    }
}

/// What one run step of a pair did (LT-5): how its program ended and what
/// it printed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ran {
    pub line: usize,
    pub end: End,
    pub stdout: Vec<u8>,
}

/// LT-5: a run of a pair against the same run of the baseline pair, whose
/// library and host `by` linked: the same end and the same standard
/// output. A run the baseline did not make (a step not for that linker) is
/// not compared.
pub fn against_baseline(ran: &Ran, by: Linker, baseline: &[Ran]) -> Result<(), String> {
    let Some(want) = baseline.iter().find(|b| b.line == ran.line) else {
        return Ok(());
    };
    let by = by.name();
    if want.end != ran.end {
        return Err(format!(
            "ended with {}, the {by}/{by} pair's program with {}",
            ran.end, want.end
        ));
    }
    if want.stdout != ran.stdout {
        return Err(format!(
            "printed {}, the {by}/{by} pair printed {}",
            outcome::quoted(&ran.stdout),
            outcome::quoted(&want.stdout)
        ));
    }
    Ok(())
}

/// The pair part's state across the linkers of a unit (LT-3, LT-5).
#[derive(Debug, Default)]
pub struct PairState {
    /// The baseline: the runs of GNU ld's own pair, or, without it, of the
    /// first reference whose record passes; with its linker.
    pub baseline: Option<(Linker, Vec<Ran>)>,
    /// The runs of the current record's own pair: what its other pairs
    /// are compared with while there is no baseline.
    pub own: Option<Vec<Ran>>,
    /// The references whose records passed, in matrix order: mortar hosts
    /// are linked against their libraries.
    pub passed: Vec<Linker>,
}

/// The reason a dynamic output is not judged while the verifier has no
/// stage 3 (`spec/harness.md` "L3").
pub const STAGE3_ABSENT: &str = "stage 3 absent";

/// The reason a reference linker's dynamic output is not judged when the
/// verifier finds its link outside stage 3 (`mortar_plumb::OUTSIDE`,
/// `spec/harness.md` "L3"). A mortar output never gets it.
pub const OUTSIDE_STAGE3: &str = "outside stage 3";

/// Whether the verifier judges dynamic outputs (stage 3, `spec/plumb.md`
/// §12, WP-050): the verifier's own [`mortar_plumb::STAGE`].
pub fn stage3_present() -> bool {
    mortar_plumb::STAGE >= 3
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
            Feature::LlvmDwarfdump => self.common.llvm_dwarfdump.is_some(),
            Feature::Gdb => self.common.gdb.is_some(),
            Feature::Lldb => self.common.lldb.is_some(),
            Feature::Elflint => self.common.elflint.is_some(),
            Feature::Kiln => matches!(self.kiln, Some(Ok(_))),
            // The IR is clang's.
            Feature::Hod => {
                matches!(self.hod, Some(Ok(_)))
                    && self.common.clang.is_some()
                    && self.common.clangxx.is_some()
            }
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
         printf '%s\\n' \"$@\" > \"${STATUS_VAR}.line\"\n\
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
    /// The unit was skipped for a tool that this run does not require
    /// even under `MORTAR_REQUIRE_TOOLS=1` (kiln with records: required
    /// under `MASON_REQUIRE_TOOLS=1` only, WP-134).
    pub optional_skip: bool,
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
    /// The index of [`Matrix::variants`]; 0 without variants.
    pub variant: usize,
    pub end: String,
    pub output: String,
    pub map: String,
    pub stderr: String,
    /// The standard error itself, with the paths replaced as for its
    /// digest.
    pub stderr_text: String,
    /// Whether the output has a section named `.mason.meta`.
    pub meta_section: bool,
    /// Whether the link line asks for `--keep-mason-meta`.
    pub keeps_meta: bool,
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

/// What [`normalise_stderr`] writes for a time.
pub const TIME_NAME: &str = "<time>";

/// A link's standard error with the times mortar measures replaced
/// (`spec/harness.md` "static-identical", WP-112): they change from run to
/// run. In a line holding `: time-phases: ` (`--time-phases`, image.md
/// I-88), all after its first `: wall ` (wall, cpu, rss and peak); in a
/// line holding `: odr-stats: ` (`--odr-stats`), all after its last
/// `; time `; in a line beginning `incremental: ` (`--incremental-report`),
/// all after its last ` time=`; a line beginning `delta time `, all after
/// that. Each is replaced by [`TIME_NAME`]. Any other line is left as it is.
pub fn normalise_stderr(text: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(text);
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let (body, end) = match line.strip_suffix('\n') {
            Some(body) => (body, "\n"),
            None => (line, ""),
        };
        let keep = if body.contains(": time-phases: ") {
            body.find(": wall ").map(|i| i + ": wall ".len())
        } else if body.contains(": odr-stats: ") {
            body.rfind("; time ").map(|i| i + "; time ".len())
        } else if body.starts_with("incremental: ") {
            body.rfind(" time=").map(|i| i + " time=".len())
        } else if body.starts_with("delta time ") {
            Some("delta time ".len())
        } else {
            None
        };
        match keep {
            Some(n) => {
                out.push_str(&body[..n]);
                out.push_str(TIME_NAME);
                out.push_str(end);
            }
            None => out.push_str(line),
        }
    }
    out.into_bytes()
}

/// The prefix of the map record of gcc's LTO plugin's resolution file
/// (`spec/plumb.md`, the `arg` record): gcc's driver passes
/// `-plugin-opt=-fresolution=<TMPDIR>/ccXXXXXX.res` on every link it makes
/// without `-static`, a new temporary name each time.
const RESOLUTION_ARG: &str = "arg -plugin-opt=-fresolution=";

/// What [`normalise_map`] writes for the resolution file's path.
pub const RESOLUTION_NAME: &str = "<gcc-resolution>.res";

/// A map's text with the one input of a link that changes from run to run
/// and is not mortar's replaced (`spec/harness.md` "static-identical"): a
/// whole line `arg -plugin-opt=-fresolution=<path>` whose `<path>` ends
/// in `/cc` and six ASCII letters or digits and `.res` (libiberty's
/// `make_temp_file`, which gcc's driver names it with) has its `<path>`
/// replaced by [`RESOLUTION_NAME`]. Any other line is left as it is.
pub fn normalise_map(text: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(text);
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let (body, end) = match line.strip_suffix('\n') {
            Some(body) => (body, "\n"),
            None => (line, ""),
        };
        let temporary = body.strip_prefix(RESOLUTION_ARG).is_some_and(|path| {
            path.strip_suffix(".res").and_then(|stem| stem.rsplit_once("/cc")).is_some_and(
                |(dir, x)| {
                    !dir.contains(char::is_whitespace)
                        && x.len() == 6
                        && x.bytes().all(|b| b.is_ascii_alphanumeric())
                },
            )
        });
        if temporary {
            out.push_str(RESOLUTION_ARG);
            out.push_str(RESOLUTION_NAME);
            out.push_str(end);
        } else {
            out.push_str(line);
        }
    }
    out.into_bytes()
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
    /// What the verifier judged of erratum 843419 (WP-103), by linker.
    pub erratum: BTreeMap<Linker, ErratumJudged>,
}

/// The erratum 843419 judgement of the outputs L3 judged (WP-103): the
/// outputs whose link line had the option, those whose line had not (or
/// was not known), the fixed and the not judged sites of the former
/// (`mortar_plumb` coverage `erratum_sites`, `erratum_not_judged`), and
/// the sites of the latter, none of which is judged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ErratumJudged {
    pub with_option: u64,
    pub without: u64,
    pub unknown: u64,
    pub sites: u64,
    pub not_judged: u64,
    pub sites_without: u64,
}

impl ErratumJudged {
    /// Counts one judged output, whose line said `fix` of the option.
    pub fn count(&mut self, fix: Option<bool>, c: &mortar_plumb::report::Coverage) {
        match fix {
            Some(true) => {
                self.with_option += 1;
                self.sites += c.erratum_sites;
                self.not_judged += c.erratum_not_judged;
                return;
            }
            Some(false) => self.without += 1,
            None => self.unknown += 1,
        }
        self.sites_without += c.erratum_sites + c.erratum_not_judged;
    }

    fn add(&mut self, o: &ErratumJudged) {
        self.with_option += o.with_option;
        self.without += o.without;
        self.unknown += o.unknown;
        self.sites += o.sites;
        self.not_judged += o.not_judged;
        self.sites_without += o.sites_without;
    }
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
        for (linker, e) in &other.erratum {
            self.erratum.entry(*linker).or_default().add(e);
        }
    }

    /// The default anomalies that apply to a judged output of the run and
    /// that no judged output of that architecture and linker has: each one
    /// a stale default (`spec/harness.md` "L3").
    pub fn stale_defaults(&self, defaults: &[Anomaly], host: Option<Arch>) -> Vec<String> {
        let mut stale = Vec::new();
        for &(arch, linker) in &self.judged_by {
            for d in defaults.iter().filter(|d| d.applies(arch, linker, host)) {
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
        // WP-103: how many outputs were judged for erratum 843419 sites.
        let parts: Vec<String> = self
            .erratum
            .iter()
            .map(|(linker, e)| {
                let mut p = format!(
                    "{} {} with the option ({} sites fixed, {} not judged), {} without \
                     ({} sites)",
                    linker.name(),
                    e.with_option,
                    e.sites,
                    e.not_judged,
                    e.without,
                    e.sites_without
                );
                if e.unknown > 0 {
                    let _ = write!(p, ", {} with no line", e.unknown);
                }
                p
            })
            .collect();
        if !parts.is_empty() {
            let _ = write!(text, "; erratum 843419 outputs: {}", parts.join("; "));
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
    map_option_named(args, "Map")
}

/// The file a link line gives the map option `name` (`Map`, or mortar's
/// `mortar-map`), in the forms of [`map_option`] (WP-112: a test that
/// writes `--mortar-map` keeps its file, and L3 reads that one).
pub fn map_option_named(args: &[String], name: &str) -> Option<String> {
    let (dashed, eq_dashed, eq) = (format!("-{name}"), format!("-{name}="), format!("{name}="));
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
            if let Some(file) =
                word.strip_prefix(eq_dashed.as_str()).or_else(|| word.strip_prefix(eq.as_str()))
            {
                found = Some(file.to_owned());
            } else if word == dashed || word == name {
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

/// How a program that cannot be started ends (`exit: no-start`): the
/// shell's status for an exec that fails, natively; qemu's for a program
/// it cannot load.
fn no_start_status(runner: &Runner) -> i32 {
    match runner {
        Runner::Native => 127,
        Runner::Emulated { .. } => 255,
    }
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
        // The fixtures' tools are the test's too.
        let mut requires = self.test.requires.clone();
        for name in &self.test.fixtures {
            match self.matrix.fixtures.get(name) {
                Some(Ok(fixture))
                    if fixture.is_linked() && !name.starts_with(LINKED_FIXTURE_PREFIX) =>
                {
                    let detail = format!(
                        "fixture {name}: it links libraries, and a linked fixture's name begins \
                         '{LINKED_FIXTURE_PREFIX}' (spec/harness.md LT-4)"
                    );
                    results.records.push(self.record(None, Outcome::Error, detail));
                    return results;
                }
                Some(Ok(fixture)) => requires.extend(fixture.requires.iter().copied()),
                Some(Err(why)) => {
                    let detail = format!("fixture {name}: {why}");
                    results.records.push(self.record(None, Outcome::Error, detail));
                    return results;
                }
                None => {
                    let detail = format!("fixture {name}: not loaded");
                    results.records.push(self.record(None, Outcome::Error, detail));
                    return results;
                }
            }
        }
        requires.sort_unstable();
        requires.dedup();
        let absent: Vec<Feature> =
            requires.iter().copied().filter(|f| !self.matrix.has(self.tools, *f)).collect();
        let missing: Vec<String> = absent
            .iter()
            .map(|feature| match (feature, &self.matrix.kiln, &self.matrix.hod) {
                (Feature::Kiln, Some(Err(why)), _) | (Feature::Hod, _, Some(Err(why))) => {
                    why.clone()
                }
                _ => super::target::feature_tool_name(*feature, self.arch(), self.matrix.host),
            })
            .collect();
        if !missing.is_empty() {
            let detail = format!("requires {}: not found", missing.join(", "));
            results.records.push(self.record(None, Outcome::Skip, detail));
            // A kiln with records and a hod with the row are the Mason
            // lane's tools: required under MASON_REQUIRE_TOOLS=1 only
            // (WP-134, WP-129), so that a gate with an older kiln or hod
            // skips these tests by name.
            let needed = |f: &Feature| {
                if f.is_mason() {
                    crate::mason::required()
                } else {
                    mortar_testutil::tools_required()
                }
            };
            if absent.iter().any(needed) {
                results.missing = missing;
            } else {
                results.optional_skip = true;
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
        let mut pairs = PairState::default();
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
            let (raw, printed) = self.run_linker(linker, None, &mut results.plumb, &mut pairs);
            let (outcome, detail) = outcome::marked(&raw, marker.map(|m| m.reason.as_str()));
            // LT-3, LT-5: a reference that passed lends its library to a
            // mortar host, and the first one's own pair is the baseline.
            if matches!(outcome, Outcome::Pass | Outcome::Xpass) {
                pairs.passed.push(linker);
                if pairs.baseline.is_none() {
                    pairs.baseline = pairs.own.take().map(|runs| (linker, runs));
                }
            }
            results.records.push(self.record(Some(linker), outcome, detail.clone()));
            references.push(Reference { linker, outcome: Some(outcome), detail, printed });
        }
        let verdict = outcome::self_check(&references);
        if let Verdict::Refuted(why) = &verdict {
            results.records.push(self.record(None, Outcome::Oracle, why.clone()));
            return results;
        }
        if linkers.contains(&Linker::Mortar) {
            let record = self.judge_mortar(&verdict, &references, &mut results.plumb, &mut pairs);
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
        let mut pairs = PairState::default();
        let (raw, _) = self.run_steps(Linker::Mortar, None, &mut judged, &mut pairs);
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
        pairs: &mut PairState,
    ) -> Record {
        let mortar = Some(Linker::Mortar);
        let marker = self.test.marker(self.arch(), Linker::Mortar);
        if let Some(skip) = marker.filter(|m| m.kind == MarkerKind::Skip) {
            return self.record(mortar, Outcome::Skip, &skip.reason);
        }
        let xfail = mortar_xfail(marker, &self.matrix.xfails, self.arch(), self.name);
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
        let (raw, _) = self.run_linker(Linker::Mortar, Some(&expected), plumb, pairs);
        let (outcome, detail) = outcome::marked(&raw, xfail);
        self.record(mortar, outcome, detail)
    }

    /// Copies the built files of the test's fixtures to `<dir>/inputs`,
    /// writes the embedded files there (a test's file replaces a fixture's
    /// of the same name) and runs the steps that build inputs there.
    fn build_inputs(&self) -> Result<(), Raw> {
        let inputs = self.dir.join("inputs");
        std::fs::create_dir_all(&inputs)
            .map_err(|e| Raw::Error(format!("cannot write '{}': {e}", inputs.display())))?;
        for name in &self.test.fixtures {
            let built =
                self.fixture(name).map_err(|e| Raw::Error(format!("fixture {name}: {e}")))?;
            copy_dir(&built, &inputs).map_err(Raw::Error)?;
        }
        self.build(&self.test.files, &self.test.steps, &inputs)
    }

    /// The directory of the fixture `name` built for the unit's
    /// architecture: built by the first unit of the run that asks, once.
    fn fixture(&self, name: &str) -> Result<PathBuf, String> {
        let cell = {
            let mut built =
                self.matrix.built.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(built.entry((name.to_owned(), self.arch(), None)).or_default())
        };
        let built = cell.get_or_init(|| {
            let Some(Ok(fixture)) = self.matrix.fixtures.get(name) else {
                return Err(Raw::Error("not loaded".to_owned()));
            };
            let dir = self.matrix.fixture_dir.join(self.arch().name()).join(name);
            self.build(&fixture.files, &fixture.steps, &dir).map(|()| dir)
        });
        match built {
            Ok(dir) => Ok(dir.clone()),
            Err(Raw::Error(e) | Raw::Fail(e) | Raw::Killed(e) | Raw::Skip(e)) => Err(e.clone()),
            Err(Raw::Pass) => Err(String::new()),
        }
    }

    /// The directory of the libraries `linker` linked from the linked
    /// fixture `name` (`spec/harness.md` LT-4), on the unit's
    /// architecture: its built inputs, copied, then its `link` and
    /// `check` steps run there with `linker`, each link judged by L3 with
    /// the run-wide defaults and nothing of a test's. Linked by the first
    /// unit of the run that asks, once; its L3 counts are that unit's.
    fn linked_fixture(
        &self,
        name: &str,
        fixture: &Fixture,
        linker: Linker,
        judged: &mut Judged,
    ) -> Result<PathBuf, Raw> {
        let cell = {
            let mut built =
                self.matrix.built.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(built.entry((name.to_owned(), self.arch(), Some(linker))).or_default())
        };
        let prefix = |raw: Raw| prefixed(raw, &format!("fixture {name}:"));
        cell.get_or_init(|| {
            let built =
                self.fixture(name).map_err(|e| Raw::Error(format!("fixture {name}: {e}")))?;
            let dir = self
                .matrix
                .fixture_dir
                .join(self.arch().name())
                .join(format!("{name}.{}", linker.name()));
            copy_dir(&built, &dir).map_err(Raw::Error)?;
            let test = Test {
                spec: Vec::new(),
                arches: vec![self.arch()],
                requires: fixture.requires.clone(),
                markers: Vec::new(),
                anomalies: Vec::new(),
                odr: Vec::new(),
                timeout: None,
                memory_mib: None,
                steps: fixture.steps.clone(),
                files: Vec::new(),
                fixtures: Vec::new(),
            };
            let label = format!("fixture {name}");
            let unit = Unit {
                matrix: self.matrix,
                tools: self.tools,
                test: &test,
                name: &label,
                dir: dir.clone(),
            };
            let vars = Vars { arch: self.arch(), host: self.matrix.host, linker: Some(linker) };
            // A fixture's links have no digest rows; their L3 counts go to
            // the unit that linked them.
            let mut own = Judged::default();
            for step in &test.steps {
                if step.is_build() || !step.scope.applies(self.arch(), linker) {
                    continue;
                }
                let result = match &step.kind {
                    StepKind::Link(link) => unit.link(step, link, linker, &dir, vars, &mut own),
                    StepKind::Check(check) => unit.check(step, check, linker, &dir, vars),
                    _ => Err(Raw::Error(format!(
                        "{}: not a step of a fixture (LT-4)",
                        step.describe()
                    ))),
                };
                if let Err(raw) = result {
                    judged.plumb.add(&own.plumb);
                    return Err(prefix(raw));
                }
            }
            judged.plumb.add(&own.plumb);
            Ok(dir)
        })
        .clone()
    }

    /// A linker's directory before its steps: the libraries it linked from
    /// the test's linked fixtures (LT-4; their L3 files left behind), then
    /// the unit's inputs over them, so that a test's file replaces a
    /// fixture's of the same name.
    fn prepare(&self, linker: Linker, dir: &Path, judged: &mut Judged) -> Result<(), Raw> {
        for name in &self.test.fixtures {
            let Some(Ok(fixture)) = self.matrix.fixtures.get(name) else { continue };
            if !fixture.is_linked() {
                continue;
            }
            let libraries = self.linked_fixture(name, fixture, linker, judged)?;
            copy_dir_skipping(&libraries, dir, LINKTEST_FILES).map_err(Raw::Error)?;
        }
        copy_dir(&self.dir.join("inputs"), dir).map_err(Raw::Error)
    }

    /// Writes `files` to `dir` and runs the build steps of `steps` there.
    #[allow(clippy::too_many_lines)] // one step loop, a branch per producer
    fn build(
        &self,
        files: &[super::format::Embedded],
        steps: &[Step],
        dir: &Path,
    ) -> Result<(), Raw> {
        let error = |e: std::io::Error, path: &Path| {
            Raw::Error(format!("cannot write '{}': {e}", path.display()))
        };
        std::fs::create_dir_all(dir).map_err(|e| error(e, dir))?;
        for file in files {
            let path = dir.join(&file.name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| error(e, parent))?;
            }
            std::fs::write(&path, &file.contents).map_err(|e| error(e, &path))?;
        }
        let vars = Vars { arch: self.arch(), host: self.matrix.host, linker: None };
        let mut meta_steps = 0usize;
        // The kiln producers: the objects `meta` steps take, with the level
        // each description implies, and those kiln made with records.
        let levels = if self.matrix.producer.is_kiln() {
            meta_levels(steps, self.arch(), vars, dir)
        } else {
            BTreeMap::new()
        };
        let mut by_kiln: BTreeSet<String> = BTreeSet::new();
        for step in steps {
            if !step.is_build() || !step.scope.has_arch(self.arch()) {
                continue;
            }
            let args = expand(build_args(step), vars);
            // A `(hod)` step is hod's, whatever the producer (WP-129).
            if let StepKind::Compile(compile) = &step.kind
                && compile.family == Family::Hod
            {
                self.compile_hod(step, compile, &args, dir)?;
                continue;
            }
            if let StepKind::Meta(_) = step.kind
                && self.matrix.producer.is_kiln()
            {
                if let [_, input, _, out] = args.as_slice()
                    && by_kiln.contains(input)
                {
                    // kiln's own container, not the description's rows.
                    std::fs::copy(dir.join(input), dir.join(out)).map_err(|e| {
                        Raw::Error(format!("{}: cannot copy '{input}': {e}", step.describe()))
                    })?;
                    by_kiln.insert(out.clone());
                } else {
                    let (program, _) = self.build_tool(step)?;
                    meta_step(step, &program, &args, dir)?;
                }
                continue;
            }
            // A `(kiln)` step stays the test's own kiln run, any producer.
            if let StepKind::Compile(compile) = &step.kind
                && self.matrix.producer.is_kiln()
                && compile.family != Family::Kiln
                && matches!(compile.language, Language::C | Language::Cxx)
            {
                let out = output_of(&args);
                let level = out.and_then(|o| levels.get(o)).copied();
                self.compile_kiln(step, compile, &args, dir, level)?;
                if let (Some(o), Some(_)) = (out, level) {
                    by_kiln.insert(o.to_owned());
                }
                continue;
            }
            if let StepKind::Meta(_) = step.kind {
                let (program, _) = self.build_tool(step)?;
                let through_ir = match self.matrix.producer {
                    Producer::Gcc | Producer::HodObjcopy | Producer::Kiln | Producer::KilnHod => {
                        false
                    }
                    Producer::Hod => true,
                    Producer::Mixed => meta_steps.is_multiple_of(2),
                };
                meta_steps += 1;
                if through_ir {
                    self.meta_step_ir(step, &args, dir)?;
                } else {
                    meta_step(step, &program, &args, dir)?;
                }
                continue;
            }
            if let StepKind::Compile(compile) = &step.kind
                && self.matrix.producer != Producer::Gcc
                && compile.family != Family::Kiln
                && matches!(compile.language, Language::C | Language::Cxx)
            {
                self.compile_ir(step, compile, &args, dir)?;
                if self.matrix.producer != Producer::Mixed {
                    continue;
                }
            }
            let (program, mut full) = self.build_tool(step)?;
            full.extend(args.into_iter().map(OsString::from));
            // A `(kiln)` step's hod route uses the lane's hod, and a hod
            // failure is an error, not a fallback (WP-134).
            let mut env: Vec<(&str, &std::ffi::OsStr)> = Vec::new();
            if let StepKind::Compile(c) = &step.kind
                && c.family == Family::Kiln
                && let Some(Ok(kiln)) = &self.matrix.kiln
            {
                env.push(("KILN_HOD", kiln.hod.as_os_str()));
                env.push(("KILN_HOD_STRICT", std::ffi::OsStr::new("1")));
            }
            let output = spawn(step, &program, &full, dir, &env, TOOL_LIMITS)?;
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

    /// kiln's and hod's builds, which a [`Producer`] other than GCC needs.
    fn mason(&self, step: &Step) -> Result<&crate::mason::Mason, Raw> {
        self.matrix.mason.as_ref().ok_or_else(|| {
            Raw::Error(format!(
                "{}: producer {} without kiln and hod",
                step.describe(),
                self.matrix.producer.name()
            ))
        })
    }

    /// A C or C++ compile step for a producer other than GCC (WP-099): the
    /// step's source to LLVM IR beside its object (`<out>.ll`; C++ by kiln,
    /// C by clang, the step's options), the relocation model it asks for in
    /// `<out>.model`, and (except for [`Producer::Mixed`]) the object by
    /// hod at `-O0` as kiln's own hod route makes it.
    fn compile_ir(
        &self,
        step: &Step,
        compile: &super::format::Compile,
        args: &[String],
        dir: &Path,
    ) -> Result<(), Raw> {
        let fail = |e: String| Raw::Error(format!("{}: {e}", step.describe()));
        let mason = self.mason(step)?;
        let program = match compile.language {
            Language::Cxx => mason.kiln.clone(),
            _ => self
                .matrix
                .common
                .clang
                .clone()
                .ok_or_else(|| fail("no clang for C to IR".to_owned()))?,
        };
        let Some(at) = args.iter().position(|a| a == "-o") else {
            return Err(fail("a compile step for hod names its output with -o".to_owned()));
        };
        let Some(out) = args.get(at + 1).cloned() else {
            return Err(fail("-o without a file".to_owned()));
        };
        let ll = format!("{out}.ll");
        let mut full: Vec<OsString> = vec![
            format!("--target={}", self.arch().triple()).into(),
            "-S".into(),
            "-emit-llvm".into(),
        ];
        if self.matrix.digest {
            let mut map = OsString::from("-ffile-prefix-map=");
            map.push(self.dir.join("inputs").as_os_str());
            map.push("=.");
            full.push(map);
        }
        for (i, a) in args.iter().enumerate() {
            full.push(if i == at + 1 { ll.clone().into() } else { a.into() });
        }
        let output = spawn(step, &program, &full, dir, &[], TOOL_LIMITS)?;
        if !output.end.success() {
            return Err(fail(format!("to IR: {}: {}", output.end, first_error(&output))));
        }
        // The last of the options that choose it, as GCC reads them.
        let model = args
            .iter()
            .rev()
            .find_map(|a| match a.as_str() {
                "-fno-pic" | "-fno-PIC" | "-fno-pie" | "-fno-PIE" => Some("static"),
                "-fpic" | "-fPIC" | "-fpie" | "-fPIE" => Some("pic"),
                _ => None,
            })
            .unwrap_or("pic");
        let model_file = dir.join(format!("{out}.model"));
        std::fs::write(&model_file, model)
            .map_err(|e| fail(format!("cannot write '{}': {e}", model_file.display())))?;
        if self.matrix.producer == Producer::Mixed {
            return Ok(());
        }
        self.hod(step, &ll, &out, dir)
    }

    /// A C or C++ compile step for a kiln producer (WP-134): the object by
    /// kiln (`kiln` for C, `kiln++` for C++, the step's options), with
    /// `-fmason-metadata=<level>` before them when a `meta` step takes it;
    /// [`Producer::KilnHod`] asks for kiln's hod route.
    fn compile_kiln(
        &self,
        step: &Step,
        compile: &super::format::Compile,
        args: &[String],
        dir: &Path,
        level: Option<&str>,
    ) -> Result<(), Raw> {
        let fail = |e: String| Raw::Error(format!("{}: {e}", step.describe()));
        let kiln = match &self.matrix.kiln {
            Some(Ok(kiln)) => kiln,
            Some(Err(why)) => return Err(fail(why.clone())),
            None => return Err(fail("no kiln for this run".to_owned())),
        };
        let program = if compile.language == Language::Cxx { &kiln.cxx } else { &kiln.c };
        let mut full: Vec<OsString> = vec![format!("--target={}", self.arch().triple()).into()];
        full.push("-c".into());
        if self.matrix.digest {
            let mut map = OsString::from("-ffile-prefix-map=");
            map.push(self.dir.join("inputs").as_os_str());
            map.push("=.");
            full.push(map);
        }
        let mut env: Vec<(&str, &std::ffi::OsStr)> = Vec::new();
        if self.matrix.producer == Producer::KilnHod {
            let mason = self.mason(step)?;
            full.push("-fkiln-backend=hod".into());
            let mut hod = OsString::from("--hod=");
            hod.push(mason.hod.as_os_str());
            full.push(hod);
            env.push(("KILN_HOD_STRICT", std::ffi::OsStr::new("1")));
        }
        if let Some(level) = level {
            full.push(format!("-fmason-metadata={level}").into());
        }
        full.extend(args.iter().map(OsString::from));
        let output = spawn(step, program, &full, dir, &env, TOOL_LIMITS)?;
        if !output.end.success() {
            return Err(fail(format!("kiln: {}: {}", output.end, first_error(&output))));
        }
        Ok(())
    }

    /// A `cc(hod)` or `cxx(hod)` step (WP-129): the source to `<out>.ll` by
    /// clang or clang++ (the step's options), then `<out>` by the probed
    /// hod ([`crate::mason::HodRows`]) with the relocation model those
    /// options choose, as GCC reads them (`-fno-pic` `static`; `-fpic`,
    /// `-fPIE` and the default `pic`: hod writes `pie` for IR with a PIE
    /// level) and the step's last `-O` level (`-O0` without one).
    fn compile_hod(
        &self,
        step: &Step,
        compile: &super::format::Compile,
        args: &[String],
        dir: &Path,
    ) -> Result<(), Raw> {
        let fail = |e: String| Raw::Error(format!("{}: {e}", step.describe()));
        let hod = match &self.matrix.hod {
            Some(Ok(rows)) => &rows.hod,
            Some(Err(why)) => return Err(fail(why.clone())),
            None => return Err(fail("no hod for this run".to_owned())),
        };
        let common = &self.matrix.common;
        let clang = if compile.language == Language::Cxx { &common.clangxx } else { &common.clang };
        let clang = clang.as_ref().ok_or_else(|| fail("no clang for the IR".to_owned()))?;
        let Some(out) = output_of(args).map(str::to_owned) else {
            return Err(fail("a compile step for hod names its output with -o".to_owned()));
        };
        let at = args.iter().position(|a| a == "-o").unwrap_or_default();
        let ll = format!("{out}.ll");
        let mut full: Vec<OsString> = vec![
            format!("--target={}", self.arch().triple()).into(),
            "-S".into(),
            "-emit-llvm".into(),
        ];
        if self.matrix.digest {
            let mut map = OsString::from("-ffile-prefix-map=");
            map.push(self.dir.join("inputs").as_os_str());
            map.push("=.");
            full.push(map);
        }
        for (i, a) in args.iter().enumerate() {
            full.push(if i == at + 1 { ll.clone().into() } else { a.into() });
        }
        let output = spawn(step, clang, &full, dir, &[], TOOL_LIMITS)?;
        if !output.end.success() {
            return Err(fail(format!("to IR: {}: {}", output.end, first_error(&output))));
        }
        let model = args
            .iter()
            .rev()
            .find_map(|a| match a.as_str() {
                "-fno-pic" | "-fno-PIC" | "-fno-pie" | "-fno-PIE" => Some("static"),
                "-fpic" | "-fPIC" | "-fpie" | "-fPIE" => Some("pic"),
                _ => None,
            })
            .unwrap_or("pic");
        let level = args
            .iter()
            .rev()
            .find(|a| matches!(a.as_str(), "-O0" | "-O1" | "-O2" | "-O3"))
            .map_or("-O0", String::as_str);
        let full: Vec<OsString> = vec![
            "--strict".into(),
            level.into(),
            format!("-relocation-model={model}").into(),
            "-filetype=obj".into(),
            format!("-mtriple={}", hod_triple(self.arch())).into(),
            ll.into(),
            "-o".into(),
            out.into(),
        ];
        let output = spawn(step, hod, &full, dir, &[], TOOL_LIMITS)?;
        if !output.end.success() {
            return Err(fail(format!("hod: {}: {}", output.end, first_error(&output))));
        }
        Ok(())
    }

    /// `<out>` by hod from `<ll>`, with the relocation model of the object
    /// the IR was made for.
    fn hod(&self, step: &Step, ll: &str, out: &str, dir: &Path) -> Result<(), Raw> {
        let fail = |e: String| Raw::Error(format!("{}: {e}", step.describe()));
        let mason = self.mason(step)?;
        let base = ll.strip_suffix(".ll").unwrap_or(ll);
        let model = std::fs::read_to_string(dir.join(format!("{base}.model")))
            .unwrap_or_else(|_| "pic".to_owned());
        let full: Vec<OsString> = vec![
            "--strict".into(),
            "-O0".into(),
            format!("-relocation-model={}", model.trim()).into(),
            "-filetype=obj".into(),
            format!("-mtriple={}", hod_triple(self.arch())).into(),
            ll.into(),
            "-o".into(),
            out.into(),
        ];
        let output = spawn(step, &mason.hod, &full, dir, &[], TOOL_LIMITS)?;
        if !output.end.success() {
            return Err(fail(format!("hod: {}: {}", output.end, first_error(&output))));
        }
        Ok(())
    }

    /// A `meta <description> <in.o> -o <out.o>` step through hod (WP-099):
    /// the description's section written into `<in.o>.ll` as IR
    /// (`cargo xtask meta write --into-ll`), and `<out.o>` made by hod from
    /// it; the container is hod's.
    fn meta_step_ir(&self, step: &Step, args: &[String], dir: &Path) -> Result<(), Raw> {
        let fail = |e: String| Raw::Error(format!("{}: {e}", step.describe()));
        let [description, input, _, out] = args else {
            return Err(fail("not 'meta <description> <in.o> -o <out.o>'".to_owned()));
        };
        if Path::new(input).extension().is_some_and(|e| e == "so") {
            let (program, _) = self.build_tool(step)?;
            return meta_step(step, &program, args, dir);
        }
        let text = std::fs::read_to_string(dir.join(description))
            .map_err(|e| fail(format!("cannot read '{description}': {e}")))?;
        let sections = crate::meta::parse(&text).map_err(fail)?;
        let in_ll = dir.join(format!("{input}.ll"));
        let module = std::fs::read_to_string(&in_ll).map_err(|e| {
            fail(format!("no IR for '{input}' (made by a compile step of C or C++?): {e}"))
        })?;
        let with = crate::meta::into_ir(&module, &sections).map_err(fail)?;
        let out_ll = format!("{out}.ll");
        std::fs::write(dir.join(&out_ll), with)
            .map_err(|e| fail(format!("cannot write '{out_ll}': {e}")))?;
        if let Ok(model) = std::fs::read(dir.join(format!("{input}.model"))) {
            let _ = std::fs::write(dir.join(format!("{out}.model")), model);
        }
        self.hod(step, &out_ll, out, dir)
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
                    // Never run here: `compile_hod` makes the object.
                    (Family::Hod, _) => None,
                    (Family::Kiln, language) => match &self.matrix.kiln {
                        Some(Ok(k)) if language == Language::Cxx => Some(&k.cxx),
                        Some(Ok(k)) => Some(&k.c),
                        _ => None,
                    },
                };
                let mut before = Vec::new();
                if matches!(compile.family, Family::Clang | Family::Kiln) {
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
            StepKind::Meta(_) => (self.tools.objcopy.clone(), Vec::new()),
            // The system's own GNU ld links every fixture library, whatever
            // linkers the run has.
            StepKind::Shlib(shlib) => {
                let program = match shlib.driver {
                    Driver::Gxx => self.tools.gxx.as_ref(),
                    Driver::Gfortran => self.tools.gfortran.as_ref(),
                    _ => Some(&self.tools.gcc),
                };
                let before = vec!["-shared".into(), "-fuse-ld=bfd".into()];
                (program.ok_or_else(missing)?.clone(), before)
            }
            StepKind::Link(_) | StepKind::Host(_) | StepKind::Run(_) | StepKind::Check(_) => {
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
        pairs: &mut PairState,
    ) -> (Raw, Vec<Printed>) {
        let mut judged = Judged::default();
        let result = self.run_steps(linker, expected, &mut judged, pairs);
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

    /// A linker's record: its library part in its directory, then the
    /// pair part once per pair (`spec/harness.md` LT-2, LT-3); a test
    /// without a `host` step is all library part. It stops at the first
    /// step that does not pass.
    fn run_steps(
        &self,
        linker: Linker,
        expected: Option<&[Printed]>,
        judged: &mut Judged,
        pairs: &mut PairState,
    ) -> (Raw, Vec<Printed>) {
        let dir = self.dir.join(linker.name());
        let mut printed = Vec::new();
        pairs.own = None;
        let split = self.test.pair_start().unwrap_or(self.test.steps.len());
        let (library, pair_part) = self.test.steps.split_at(split);
        let linkers = self.matrix.linkers_of(self.tools);
        let all = Pair::of(linker, &linkers, &pairs.passed, self.matrix.digest);
        // LT-3: mortar hosts the references' libraries first, whatever
        // becomes of its own library.
        if !pair_part.is_empty() {
            for pair in all.iter().filter(|pair| pair.library != linker) {
                if let Err(raw) = self.run_pair(*pair, linker, pair_part, judged, pairs) {
                    return (raw, printed);
                }
            }
        }
        if let Err(raw) = self.prepare(linker, &dir, judged) {
            return (raw, printed);
        }
        if let Err(raw) = self.run_library(linker, library, &dir, expected, judged, &mut printed) {
            return (raw, printed);
        }
        if pair_part.is_empty() {
            return (Raw::Pass, printed);
        }
        // The library as this linker's library part left it, for the
        // pairs whose host another linker links: this record's, and a
        // reference's under a mortar host (mortar's record, later).
        let mut copies: Vec<Pair> = all
            .iter()
            .filter(|pair| pair.library == linker && pair.host != linker)
            .copied()
            .collect();
        if linker.is_reference() && !self.matrix.digest && linkers.contains(&Linker::Mortar) {
            copies.push(Pair { library: linker, host: Linker::Mortar });
        }
        for pair in copies {
            if let Err(e) = copy_dir(&dir, &self.dir.join(pair.dir_name())) {
                return (Raw::Error(e), printed);
            }
        }
        for pair in all.iter().filter(|pair| pair.library == linker) {
            if let Err(raw) = self.run_pair(*pair, linker, pair_part, judged, pairs) {
                return (raw, printed);
            }
        }
        (Raw::Pass, printed)
    }

    /// The pair part's steps for one pair of `linker`'s record, in the
    /// pair's directory (LT-3): each `host` linked by the pair's host
    /// linker, each run compared with the baseline pair's (LT-5), each
    /// check as the record's. A result of another pair than the record's
    /// own names the pair.
    fn run_pair(
        &self,
        pair: Pair,
        linker: Linker,
        steps: &[Step],
        judged: &mut Judged,
        pairs: &mut PairState,
    ) -> Result<(), Raw> {
        let dir = self.dir.join(pair.dir_name());
        let vars = Vars { arch: self.arch(), host: self.matrix.host, linker: Some(linker) };
        let own = pair == Pair { library: linker, host: linker };
        let label = |raw: Raw| if own { raw } else { prefixed(raw, &pair.label()) };
        let mut ran = Vec::new();
        // The references' output of a pair is the baseline's, not the
        // self-check's: what a pair's run prints is compared below.
        let mut unused = Vec::new();
        for step in steps {
            if step.is_build() || !step.scope.applies(self.arch(), linker) {
                continue;
            }
            if self.matrix.digest && !matches!(step.kind, StepKind::Host(_)) {
                continue;
            }
            let result = match &step.kind {
                StepKind::Host(link) => self.link(step, link, pair.host, &dir, vars, judged),
                StepKind::Run(run) => self
                    .run_program(step, run, &dir, vars, None, &mut unused)
                    .and_then(|(end, stdout)| {
                        let this = Ran { line: step.line, end, stdout };
                        let baseline = match (&pairs.baseline, &pairs.own) {
                            (Some((by, runs)), _) => Some((*by, runs.as_slice())),
                            (None, Some(runs)) if !own => Some((linker, runs.as_slice())),
                            _ => None,
                        };
                        if let Some((by, runs)) = baseline {
                            against_baseline(&this, by, runs).map_err(|what| {
                                Raw::Fail(format!("{}: {what}", step.describe()))
                            })?;
                        }
                        ran.push(this);
                        Ok(())
                    }),
                StepKind::Check(check) => self.check(step, check, linker, &dir, vars),
                StepKind::Link(_)
                | StepKind::Compile(_)
                | StepKind::Yaml2obj(_)
                | StepKind::Ar(_)
                | StepKind::Meta(_)
                | StepKind::Shlib(_) => {
                    unreachable!("the pair part links no library (LT-2); builds ran before")
                }
            };
            result.map_err(label)?;
        }
        if own {
            pairs.own = Some(ran);
        }
        Ok(())
    }

    /// The library part of a record (all of a test without a `host`
    /// step), in the linker's directory.
    fn run_library(
        &self,
        linker: Linker,
        steps: &[Step],
        dir: &Path,
        expected: Option<&[Printed]>,
        judged: &mut Judged,
        printed: &mut Vec<Printed>,
    ) -> Result<(), Raw> {
        let dir = dir.to_path_buf();
        let vars = Vars { arch: self.arch(), host: self.matrix.host, linker: Some(linker) };
        for step in steps {
            if step.is_build() || !step.scope.applies(self.arch(), linker) {
                continue;
            }
            if self.matrix.digest && !matches!(step.kind, StepKind::Link(_)) {
                continue;
            }
            let result = match &step.kind {
                StepKind::Link(link) => self.link(step, link, linker, &dir, vars, judged),
                StepKind::Run(run) => {
                    self.run_program(step, run, &dir, vars, expected, printed).map(|_| ())
                }
                StepKind::Check(check) => self.check(step, check, linker, &dir, vars),
                StepKind::Host(_) => unreachable!("the library part ends at the first host"),
                StepKind::Compile(_)
                | StepKind::Yaml2obj(_)
                | StepKind::Ar(_)
                | StepKind::Meta(_)
                | StepKind::Shlib(_) => {
                    unreachable!("build steps are run once, before the linkers")
                }
            };
            result?;
        }
        Ok(())
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
        let map_file = if link.fails || link.informs || !(self.matrix.plumb || self.matrix.digest) {
            None
        } else if let Some(map) = match linker {
            Linker::Mortar => map_option_named(&args, "mortar-map"),
            _ => map_option(&args),
        } {
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
        if (link.fails || link.informs) && output_file.symlink_metadata().is_ok() {
            std::fs::remove_file(&output_file).map_err(|e| {
                Raw::Error(format!("cannot remove '{}': {e}", output_file.display()))
            })?;
        }
        // WP-099: the option variants of the digest mode, variant 0 last.
        let variants = &self.matrix.variants;
        if self.matrix.digest && linker == Linker::Mortar && variants.len() > 1 {
            for (k, extra) in variants.iter().enumerate().skip(1) {
                let mut words = args.clone();
                words.extend(extra.iter().map(|o| variant_word(link.driver, o)));
                if output_file.symlink_metadata().is_ok() {
                    let _ = std::fs::remove_file(&output_file);
                }
                let keeps = keeps_meta(&words);
                let (output, end) = self.run_link_line(step, link, linker, dir, words)?;
                let mut row = self.digest_row(step, k, end, &output, &output_file, dir);
                row.keeps_meta = keeps;
                judged.digests.push(row);
            }
        }
        if let Some(extra) = variants.first().filter(|_| linker == Linker::Mortar) {
            args.extend(extra.iter().map(|o| variant_word(link.driver, o)));
        }
        let keeps = keeps_meta(&args);
        let (output, linker_end) = self.run_link_line(step, link, linker, dir, args.clone())?;
        let ld_words = (link.driver == Driver::Ld).then_some(args);
        if self.matrix.digest {
            let mut row = self.digest_row(step, 0, linker_end, &output, &output_file, dir);
            row.keeps_meta = keeps;
            judged.digests.push(row);
        }
        self.judge_link(
            step,
            link,
            linker,
            dir,
            &output_file,
            map_file,
            ld_words,
            &output,
            linker_end,
            judged,
        )
    }

    /// Runs one link line of a link step: the driver (or the linker), and
    /// how the linker itself ended.
    fn run_link_line(
        &self,
        step: &Step,
        link: &Link,
        linker: Linker,
        dir: &Path,
        args: Vec<String>,
    ) -> Result<(Output, End), Raw> {
        let status_file = dir.join(format!(".linktest-ld-status-{}", step.line));
        let _ = std::fs::remove_file(&status_file);
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
        Ok((output, linker_end))
    }

    /// The expectations of a link step on its link's end and standard
    /// error, then L3. `ld_words` is stage 3's line (spec/plumb.md
    /// §12.1): a reference link's words as its linker saw them; a mortar
    /// link gets none (its map's `arg` records are the line, and a map
    /// without them is P-D2).
    #[allow(clippy::too_many_arguments)] // the step, its link, and what it did
    fn judge_link(
        &self,
        step: &Step,
        link: &Link,
        linker: Linker,
        dir: &Path,
        output_file: &Path,
        map_file: Option<PathBuf>,
        ld_words: Option<Vec<String>>,
        output: &Output,
        linker_end: End,
        judged: &mut Judged,
    ) -> Result<(), Raw> {
        let status_file = dir.join(format!(".linktest-ld-status-{}", step.line));
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
                    output_name_of(output_file)
                )));
            }
        } else if !linker_end.success() || !output.end.success() {
            let end = if linker_end.success() { output.end } else { linker_end };
            return Err(fail(&format_args!("{end}: {}", first_error(output))));
        }
        if link.informs && output_file.symlink_metadata().is_ok() {
            return Err(fail(&format_args!(
                "the link wrote '{}'; the test expects it only to inform",
                output_name_of(output_file)
            )));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        self.expect(&link.diags, linker, &stderr, "the link's standard error")
            .map_err(|what| Raw::Fail(format!("{}: {what}", step.describe())))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        self.expect(&link.prints, linker, &stdout, "the link's standard output")
            .map_err(|what| Raw::Fail(format!("{}: {what}", step.describe())))?;
        // spec/harness.md "Directives" (`diag`): the one-definition-rule
        // diagnostics of a mortar link are exactly those the test names.
        if linker == Linker::Mortar {
            super::odr::judge(&self.test.odr, &stderr)
                .map_err(|what| Raw::Fail(format!("{}: {what}", step.describe())))?;
        }
        match map_file {
            Some(map) if self.matrix.plumb => {
                let words: Option<Vec<String>> = ld_words.or_else(|| {
                    let mut recorded = status_file.clone().into_os_string();
                    recorded.push(".line");
                    std::fs::read_to_string(&recorded)
                        .ok()
                        .map(|text| text.lines().map(str::to_owned).collect())
                });
                // A reference's words state `--fix-cortex-a53-843419`
                // (WP-103); a mortar output is judged with its map's `arg`
                // records only, the option and stage 3 alike ("L3").
                let (fix, words) = if linker == Linker::Mortar {
                    (None, None)
                } else {
                    (words.as_deref().map(|w| crate::plumb::line_fix_843419(w, dir)), words)
                };
                self.verify(step, linker, output_file, &map, dir, words, fix, judged)
            }
            _ => Ok(()),
        }
    }

    /// The digest mode's row of a link step that ran in `dir`.
    fn digest_row(
        &self,
        step: &Step,
        variant: usize,
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
        let file = |path: &Path, map: bool| match std::fs::read(path) {
            Ok(bytes) if map => digest_hex(&normalise(&normalise_map(&bytes), &subs)),
            Ok(bytes) => digest_hex(&bytes),
            Err(_) => "-".to_owned(),
        };
        let map = dir.join(format!(".linktest-plumb-{}.map", step.line));
        let stderr = normalise(&normalise_stderr(&output.stderr), &subs);
        let meta_section = std::fs::read(output_file).is_ok_and(|bytes| has_meta_section(&bytes));
        DigestRow {
            test: self.name.to_owned(),
            arch: self.arch(),
            line: step.line,
            variant,
            end: end.to_string(),
            output: file(output_file, false),
            map: file(&map, true),
            stderr: digest_hex(&stderr),
            stderr_text: String::from_utf8_lossy(&stderr).into_owned(),
            meta_section,
            keeps_meta: false,
        }
    }

    /// L3 (`spec/harness.md` "L3"): the verifier, `mortar_plumb`, on the
    /// output of a link that succeeded. A finding fails the step unless it
    /// is a recorded anomaly (`spec/plumb.md` §10.2) that the test expects
    /// of this linker.
    #[allow(clippy::too_many_arguments)] // the step's facts and its line
    #[allow(clippy::too_many_lines, reason = "one output, read to verdict")]
    fn verify(
        &self,
        step: &Step,
        linker: Linker,
        output_file: &Path,
        map_file: &Path,
        dir: &Path,
        line: Option<Vec<String>>,
        fix_843419: Option<bool>,
        judged: &mut Judged,
    ) -> Result<(), Raw> {
        let error = |what: &dyn Display| Raw::Error(format!("{}: L3: {what}", step.describe()));
        let bytes = std::fs::read(output_file)
            .map_err(|e| error(&format_args!("{}: {e}", output_file.display())))?;
        match not_judged(&bytes) {
            // Stage 3 (WP-050) judges dynamic outputs; until it lands they
            // are not judged, by that name.
            Some("dynamic" | "pie-or-shared") if self.matrix.stage3 => {}
            Some("dynamic" | "pie-or-shared") => {
                *judged.plumb.not_judged.entry(STAGE3_ABSENT).or_default() += 1;
                return Ok(());
            }
            Some(reason) => {
                *judged.plumb.not_judged.entry(reason).or_default() += 1;
                return Ok(());
            }
            None => {}
        }
        drop(bytes);
        let format = match linker {
            Linker::Bfd => mortar_plumb::Format::Gnu,
            Linker::Lld => mortar_plumb::Format::Lld,
            Linker::Mold => mortar_plumb::Format::Mold,
            Linker::Mortar => mortar_plumb::Format::Mortar,
        };
        let started = Instant::now();
        let verified = crate::plumb::verify_link_line(
            output_file,
            map_file,
            format,
            dir,
            None,
            fix_843419,
            line,
        );
        judged.plumb.time += started.elapsed();
        let (bytes, report, _) = verified.map_err(|e| error(&e))?;
        // Counted under the option the verifier judged with: mortar's is
        // its map's line (`Coverage::erratum_option`), a reference's the
        // caller's.
        let fix =
            if linker == Linker::Mortar { report.coverage.erratum_option } else { fix_843419 };
        judged.plumb.erratum.entry(linker).or_default().count(fix, &report.coverage);
        // A reference's link that stage 3 does not model (P-D1 "outside
        // stage 3": an option dyn.md refuses, an i386 dynamic output,
        // version definitions, a closure object no search finds; a
        // static-pie is judged, plumb.md §12.4) is counted as not
        // judged by stage 3, by that name; what the earlier stages found
        // is still judged below. mortar's never is: its finding fails.
        let outside =
            linker != Linker::Mortar && report.findings.iter().any(mortar_plumb::Finding::outside);
        if outside {
            *judged.plumb.not_judged.entry(OUTSIDE_STAGE3).or_default() += 1;
        } else {
            judged.plumb.verified += 1;
        }
        judged.plumb.judged_by.insert((self.arch(), linker));
        let expected = anomaly_ids(&self.test.anomalies, self.arch(), linker);
        // mortar never has a default, whatever the file says.
        let defaults = if linker == Linker::Mortar {
            Vec::new()
        } else {
            self.matrix
                .defaults
                .iter()
                .filter(|a| a.applies(self.arch(), linker, self.matrix.host))
                .flat_map(|a| a.ids.iter().map(String::as_str))
                .collect()
        };
        let mut unexpected = Vec::new();
        for f in report.findings.iter().filter(|f| !(outside && f.outside())) {
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
    ) -> Result<(End, Vec<u8>), Raw> {
        let fail = |what: &dyn Display| Raw::Fail(format!("{}: {what}", step.describe()));
        let name = run.program.expand(vars);
        let path = dir.join(&name);
        if !path.is_file() {
            return Err(fail(&format_args!("there is no program '{name}'")));
        }
        let args = expand(&run.args, vars);
        let mut limits = self.limits(RUN_LIMITS);
        if let Runner::Native = &self.tools.runner {
            // `/bin/sh` runs a file the kernel refuses as a script.
            let mut magic = [0u8; 4];
            let is_elf = std::fs::File::open(&path)
                .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut magic))
                .is_ok()
                && magic == *b"\x7fELF";
            if !is_elf {
                return Err(fail(&format_args!("'{name}' is not an ELF file")));
            }
        } else {
            limits.time *= EMULATED_TIME_FACTOR;
            if self.arch() == Arch::I386 {
                limits.memory_kib += I386_GUEST_KIB;
            }
        }
        let env = program_env(dir, &run.env, vars);
        let (program, full, host_env) = self.under_runner(&path, &env, args);
        let host_env: Vec<(&str, &std::ffi::OsStr)> =
            host_env.iter().map(|(n, v)| (n.as_str(), v.as_os_str())).collect();
        let output = spawn(step, &program, &full, dir, &host_env, limits)?;
        let met = match (run.exit, output.end) {
            (Exit::Status(want), End::Exit(got)) | (Exit::Signal(want), End::Signal(got)) => {
                want == got
            }
            (Exit::NoStart, End::Exit(got)) => got == no_start_status(&self.tools.runner),
            _ => false,
        };
        if !met {
            let want = match run.exit {
                Exit::Status(status) => End::Exit(status),
                Exit::Signal(signal) => End::Signal(signal),
                Exit::NoStart => End::Exit(no_start_status(&self.tools.runner)),
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
                None => printed.push(Printed { line: step.line, stdout: output.stdout.clone() }),
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
        Ok((output.end, output.stdout))
    }

    /// The program, arguments and host environment that run `program` of
    /// the unit's architecture with `env` and the arguments `args`:
    /// natively with `env` in its environment, or under qemu with `-L
    /// <system root>` and each variable as `-E NAME=value` (the guest's,
    /// not qemu's: an `LD_PRELOAD` of a guest library must not reach the
    /// host's loader).
    fn under_runner(
        &self,
        program: &Path,
        env: &[(String, String)],
        args: Vec<String>,
    ) -> (PathBuf, Vec<OsString>, Vec<(String, OsString)>) {
        match &self.tools.runner {
            Runner::Native => {
                let (program, args) = native_command(program, env, args);
                (program, args, Vec::new())
            }
            Runner::Emulated { qemu, sysroot } => {
                let mut full: Vec<OsString> = vec!["-L".into(), sysroot.clone().into_os_string()];
                for (name, value) in env {
                    full.push("-E".into());
                    full.push(format!("{name}={value}").into());
                }
                full.push(program.as_os_str().to_owned());
                full.extend(args.into_iter().map(OsString::from));
                (qemu.clone(), full, Vec::new())
            }
        }
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
        let common = &self.matrix.common;
        let env = program_env(dir, &[], vars);
        let text = match check.tool {
            CheckTool::Text => {
                let path = dir.join(&args[0]);
                match std::fs::read(&path) {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                    Err(e) => return Err(fail(&format_args!("cannot read '{}': {e}", args[0]))),
                }
            }
            CheckTool::Cmp => {
                let read = |name: &String| {
                    std::fs::read(dir.join(name))
                        .map_err(|e| fail(&format_args!("cannot read '{name}': {e}")))
                };
                compare(&read(&args[0])?, &read(&args[1])?)
            }
            CheckTool::Gdb | CheckTool::Lldb if self.tools.runner != Runner::Native => {
                return Err(Raw::Skip(format!(
                    "{}: {} runs only programs of the host here, and {} programs run under qemu",
                    step.describe(),
                    check.tool.name(),
                    self.arch().name()
                )));
            }
            tool => {
                let (program, full, host_env): (PathBuf, Vec<OsString>, Vec<(String, OsString)>) =
                    match tool {
                        CheckTool::Ldd => {
                            let loader = self.arch().loader(self.matrix.host);
                            // A name without a slash would be searched for
                            // as a library.
                            let mut list = vec!["--list".to_owned()];
                            list.extend(args.iter().map(|arg| {
                                if arg.contains('/') { arg.clone() } else { format!("./{arg}") }
                            }));
                            self.under_runner(&loader, &env, list)
                        }
                        CheckTool::Gdb | CheckTool::Lldb => {
                            let (program, full) =
                                debugger(tool, common, &args).ok_or_else(|| {
                                    Raw::Error(format!(
                                        "{}: its tool is not there",
                                        step.describe()
                                    ))
                                })?;
                            let host = env.iter().map(|(n, v)| (n.clone(), v.into())).collect();
                            (program, full, host)
                        }
                        _ => {
                            let (program, before): (Option<&PathBuf>, &[&str]) = match tool {
                                CheckTool::Readelf => (Some(&self.tools.readelf), &[]),
                                CheckTool::Nm => (Some(&self.tools.nm), &[]),
                                CheckTool::Objdump => (Some(&self.tools.objdump), &[]),
                                CheckTool::LlvmReadelf => (common.llvm_readelf.as_ref(), &[]),
                                CheckTool::LlvmDwarfdump => (common.llvm_dwarfdump.as_ref(), &[]),
                                _ => (common.elflint.as_ref(), &["--gnu-ld"]),
                            };
                            let Some(program) = program else {
                                return Err(Raw::Error(format!(
                                    "{}: its tool is not there",
                                    step.describe()
                                )));
                            };
                            let mut full: Vec<OsString> =
                                before.iter().map(OsString::from).collect();
                            full.extend(args.iter().map(OsString::from));
                            (program.clone(), full, Vec::new())
                        }
                    };
                let host_env: Vec<(&str, &std::ffi::OsStr)> =
                    host_env.iter().map(|(n, v)| (n.as_str(), v.as_os_str())).collect();
                let mut limits = TOOL_LIMITS;
                if tool == CheckTool::Ldd && self.tools.runner != Runner::Native {
                    limits.time *= EMULATED_TIME_FACTOR;
                    if self.arch() == Arch::I386 {
                        limits.memory_kib += I386_GUEST_KIB;
                    }
                }
                let output = spawn(step, &program, &full, dir, &host_env, limits)?;
                let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
                if tool == CheckTool::Elflint {
                    // Its complaints are judged by the `allow` lines, not
                    // by its exit status.
                    text.push_str(&String::from_utf8_lossy(&output.stderr));
                    self.allowed(&check.expectations, linker, &text, &shown)
                        .map_err(|w| fail(&w))?;
                } else if !output.end.success() {
                    return Err(fail(&format_args!(
                        "`{shown}`: {}: {}",
                        output.end,
                        first_error(&output)
                    )));
                }
                text
            }
        };
        self.expect(&check.expectations, linker, &text, &format!("`{shown}`"))
            .map_err(|what| fail(&what))
    }

    /// `check elflint`: every line it prints is `No errors` or matches an
    /// `allow` line that applies to `linker` (`spec/harness.md` "Checks").
    fn allowed(
        &self,
        expectations: &[Expectation],
        linker: Linker,
        text: &str,
        shown: &str,
    ) -> Result<(), String> {
        let vars = Vars { arch: self.arch(), host: self.matrix.host, linker: Some(linker) };
        let mut allow = Vec::new();
        for e in expectations.iter().filter(|e| e.want == Want::Allow) {
            if e.scope.applies(self.arch(), linker) {
                let source = e.pattern.expand_pattern(vars);
                allow.push(
                    Pattern::parse(&source).map_err(|err| format!("line {}: {err}", e.line))?,
                );
            }
        }
        let complaints: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && *line != "No errors")
            .filter(|line| !allow.iter().any(|p| p.matches_line(line)))
            .collect();
        match complaints.as_slice() {
            [] => Ok(()),
            [first, rest @ ..] => {
                let more = if rest.is_empty() {
                    String::new()
                } else {
                    format!(" (and {} more)", rest.len())
                };
                Err(format!("`{shown}` complains, and no 'allow' line allows it: {first}{more}"))
            }
        }
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
        let lines = || text.lines().map(|line| line.strip_suffix('\r').unwrap_or(line));
        // The line after the one the last `has` or `then` matched.
        let mut next = 0;
        for expectation in expectations.iter().filter(|e| e.scope.applies(self.arch(), linker)) {
            let source = expectation.pattern.expand_pattern(vars);
            let pattern =
                Pattern::parse(&source).map_err(|e| format!("line {}: {e}", expectation.line))?;
            match expectation.want {
                Want::Allow => continue,
                Want::Count(want) => {
                    let got = lines().filter(|line| pattern.matches_line(line)).count();
                    if u32::try_from(got).ok() != Some(want) {
                        return Err(format!(
                            "line {}: {got} lines of {what} match '{pattern}', not {want}{}",
                            expectation.line,
                            first_lines(text)
                        ));
                    }
                    continue;
                }
                Want::Then => {
                    let hit = lines().enumerate().skip(next).find(|(_, l)| pattern.matches_line(l));
                    match hit {
                        Some((at, _)) => next = at + 1,
                        None => {
                            return Err(format!(
                                "line {}: no line of {what} after the one the expectation before \
                                 matched matches '{pattern}'{}",
                                expectation.line,
                                first_lines(text)
                            ));
                        }
                    }
                    continue;
                }
                Want::Line => {}
            }
            let hit = lines().enumerate().find(|(_, line)| pattern.matches_line(line));
            if let (true, Some((at, _))) = (expectation.present, hit) {
                next = at + 1;
            }
            let hit = hit.map(|(_, line)| line);
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

/// The `-o` file of a compile step's arguments.
fn output_of(args: &[String]) -> Option<&str> {
    let at = args.iter().position(|a| a == "-o")?;
    args.get(at + 1).map(String::as_str)
}

/// For the kiln producers (WP-134): the object each `meta` step of `steps`
/// takes (not a shared object), with the `-fmason-metadata` level its
/// description implies ([`meta_level`]).
fn meta_levels(
    steps: &[Step],
    arch: Arch,
    vars: Vars,
    dir: &Path,
) -> BTreeMap<String, &'static str> {
    let mut levels = BTreeMap::new();
    for step in steps {
        if !matches!(step.kind, StepKind::Meta(_)) || !step.scope.has_arch(arch) {
            continue;
        }
        let args = expand(build_args(step), vars);
        let [description, input, _, _] = args.as_slice() else { continue };
        if Path::new(input).extension().is_some_and(|e| e == "so") {
            continue;
        }
        let text = std::fs::read_to_string(dir.join(description)).unwrap_or_default();
        levels.insert(input.clone(), meta_level(&text));
    }
    levels
}

/// The `-fmason-metadata` level a test producer's description implies
/// (WP-134): `detail` when it says `level detail` (kind 4, O-19) or when a
/// kind 5 row of a C++-linkage function carries its type (`spec/odr.md`
/// O-35: only `detail` writes those), else `default` (kinds 1, 3 and 5).
pub fn meta_level(description: &str) -> &'static str {
    for line in description.lines() {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("declaration") => {}
            Some("level") if words.next() == Some("detail") => return "detail",
            _ => continue,
        }
        let mut flags = 0u16;
        let mut typed = false;
        for word in words {
            if let Some(v) = word.strip_prefix("flags=") {
                flags = v
                    .strip_prefix("0x")
                    .map_or_else(|| v.parse().ok(), |h| u16::from_str_radix(h, 16).ok())
                    .unwrap_or(0);
            } else if let Some(v) = word.strip_prefix("type_name=") {
                typed = !v.is_empty() && v != "\"\"";
            }
        }
        let function = flags & 2 == 0;
        let cxx = flags & 4 == 0;
        if function && cxx && typed {
            return "detail";
        }
    }
    "default"
}

/// A `meta <description> <in.o> -o <out.o>` step: the description's one
/// section, written by the test producer and added by `objcopy`.
fn meta_step(step: &Step, objcopy: &Path, args: &[String], dir: &Path) -> Result<(), Raw> {
    let fail = |e: String| Raw::Error(format!("{}: {e}", step.describe()));
    let [description, input, _, out] = args else {
        return Err(fail("not 'meta <description> <in.o> -o <out.o>'".to_owned()));
    };
    let text = std::fs::read_to_string(dir.join(description))
        .map_err(|e| fail(format!("cannot read '{description}': {e}")))?;
    // A shared object: its `.note.mason` table (`spec/odr.md` O-50 to
    // O-52), from `row` lines (`cargo xtask meta table`).
    if Path::new(input).extension().is_some_and(|e| e == "so") {
        return crate::meta::fill_table(&text, &dir.join(input), &dir.join(out)).map_err(fail);
    }
    let sections = crate::meta::parse(&text).map_err(fail)?;
    let [items] = sections.as_slice() else {
        return Err(fail("the description of a 'meta' step has one section".to_owned()));
    };
    crate::meta::add_with(objcopy, items, &dir.join(input), &dir.join(out)).map_err(fail)
}

/// The triple hod is given for an architecture.
fn hod_triple(arch: Arch) -> &'static str {
    match arch {
        Arch::Aarch64 => "aarch64-unknown-linux-gnu",
        Arch::X86_64 => "x86_64-unknown-linux-gnu",
        Arch::I386 => "i686-unknown-linux-gnu",
    }
}

/// Whether a link line asks for `--keep-mason-meta`.
fn keeps_meta(words: &[String]) -> bool {
    words.iter().any(|w| w.split(',').any(|o| o == "--keep-mason-meta"))
}

/// One option of a digest variant on a link line: bare for `link ld`,
/// `-Wl,` before it for a driver.
fn variant_word(driver: Driver, option: &str) -> String {
    match driver {
        Driver::Ld => option.to_owned(),
        _ => format!("-Wl,{option}"),
    }
}

/// Whether an ELF file (64-bit, little-endian: the targets hod has) has a
/// section named `.mason.meta`; anything it cannot read has none.
pub fn has_meta_section(bytes: &[u8]) -> bool {
    let word = |at: usize, n: usize| -> Option<u64> {
        let b = bytes.get(at..at.checked_add(n)?)?;
        Some(b.iter().rev().fold(0u64, |v, &x| (v << 8) | u64::from(x)))
    };
    let names = || -> Option<bool> {
        if bytes.get(..6)? != b"\x7fELF\x02\x01" {
            return Some(false);
        }
        let shoff = usize::try_from(word(0x28, 8)?).ok()?;
        let shentsize = usize::try_from(word(0x3a, 2)?).ok()?;
        let shnum = usize::try_from(word(0x3c, 2)?).ok()?;
        let shstrndx = usize::try_from(word(0x3e, 2)?).ok()?;
        let header = |i: usize| shoff.checked_add(i.checked_mul(shentsize)?);
        let strtab = usize::try_from(word(header(shstrndx)? + 0x18, 8)?).ok()?;
        for i in 0..shnum {
            let name = usize::try_from(word(header(i)?, 4)?).ok()?;
            let at = strtab.checked_add(name)?;
            let text = bytes.get(at..)?;
            let end = text.iter().position(|&b| b == 0)?;
            if &text[..end] == mortar_meta::SECTION_NAME {
                return Some(true);
            }
        }
        Some(false)
    };
    names().unwrap_or(false)
}

/// The arguments of a build step.
fn build_args(step: &Step) -> &[Template] {
    match &step.kind {
        StepKind::Compile(compile) => &compile.args,
        StepKind::Yaml2obj(args) | StepKind::Ar(args) | StepKind::Meta(args) => args,
        StepKind::Shlib(shlib) => &shlib.args,
        StepKind::Link(_) | StepKind::Host(_) | StepKind::Run(_) | StepKind::Check(_) => &[],
    }
}

fn expand(templates: &[Template], vars: Vars) -> Vec<String> {
    templates.iter().map(|template| template.expand(vars)).collect()
}

/// The environment a linked program gets beyond the harness's
/// (`spec/harness.md` "Runs"): `LD_LIBRARY_PATH` is the linker's
/// directory, where the fixture libraries are, unless an `env` line sets
/// it (to nothing, for none); then the `env` lines in order.
pub fn program_env(dir: &Path, env: &[(String, Template)], vars: Vars) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if !env.iter().any(|(name, _)| name == "LD_LIBRARY_PATH") {
        let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
        out.push(("LD_LIBRARY_PATH".to_owned(), dir.display().to_string()));
    }
    out.extend(env.iter().map(|(name, value)| (name.clone(), value.expand(vars))));
    out
}

/// The command of `check gdb` or `check lldb`: the debugger in batch mode
/// on the program (the first argument), each further argument one of its
/// commands. gdb: no init files, no pager, no debuginfod; lldb: ASLR as
/// the program would have it (the container refuses `personality`,
/// `spec/dyn.md` D-70).
fn debugger(
    tool: CheckTool,
    common: &CommonTools,
    args: &[String],
) -> Option<(PathBuf, Vec<OsString>)> {
    let (program, commands) = args.split_first()?;
    let mut full: Vec<OsString> = Vec::new();
    let path = if tool == CheckTool::Gdb {
        for arg in
            ["-nx", "-batch", "-ex", "set pagination off", "-ex", "set debuginfod enabled off"]
        {
            full.push(arg.into());
        }
        for command in commands {
            full.push("-ex".into());
            full.push(command.into());
        }
        full.push(program.into());
        common.gdb.clone()?
    } else {
        for arg in ["--no-lldbinit", "-b", "-o", "settings set target.disable-aslr false"] {
            full.push(arg.into());
        }
        for command in commands {
            full.push("-o".into());
            full.push(command.into());
        }
        full.push(program.into());
        common.lldb.clone()?
    };
    Some((path, full))
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
/// A natively run program's command (`spec/harness.md` LT-6): with an
/// environment, `env NAME=value... program args`, so that the variables
/// (an `LD_PRELOAD`) are set on the program's own `exec` and reach nothing
/// else, not the shell that sets the limits; without one, the program.
pub fn native_command(
    program: &Path,
    env: &[(String, String)],
    args: Vec<String>,
) -> (PathBuf, Vec<OsString>) {
    if env.is_empty() {
        return (program.to_path_buf(), args.into_iter().map(OsString::from).collect());
    }
    let mut full: Vec<OsString> =
        env.iter().map(|(name, value)| format!("{name}={value}").into()).collect();
    full.push(program.as_os_str().to_owned());
    full.extend(args.into_iter().map(OsString::from));
    (PathBuf::from(ENV_PROGRAM), full)
}

/// The prefix of the files the harness writes beside a link (its maps,
/// the wrapper's status and line files): not copied out of a linked
/// fixture (LT-4), so that none stands in for a test's own.
pub const LINKTEST_FILES: &str = ".linktest-";

/// `raw` with `what` before its detail: the place of a fixture's step, a
/// pair's label.
fn prefixed(raw: Raw, what: &str) -> Raw {
    match raw {
        Raw::Pass => Raw::Pass,
        Raw::Fail(d) => Raw::Fail(format!("{what} {d}")),
        Raw::Killed(d) => Raw::Killed(format!("{what} {d}")),
        Raw::Error(d) => Raw::Error(format!("{what} {d}")),
        Raw::Skip(d) => Raw::Skip(format!("{what} {d}")),
    }
}

/// [`copy_dir`] without the files of the top directory whose names begin
/// with `skip`.
fn copy_dir_skipping(from: &Path, to: &Path, skip: &str) -> Result<(), String> {
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
    for name in names.into_iter().filter(|n| !n.to_string_lossy().starts_with(skip)) {
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
    test: &str,
) -> Option<&'a str> {
    let line =
        |test: Option<&str>| xfails.iter().find(|x| x.arch == arch && x.test.as_deref() == test);
    line(None)
        .or_else(|| line(Some(test)))
        .map(|x| x.reason.as_str())
        .or_else(|| marker.filter(|m| m.kind == MarkerKind::Xfail).map(|m| m.reason.as_str()))
}

/// `check cmp`: `identical`, or `differ: ` with the sizes and the first
/// byte that differs (`spec/harness.md` "Checks").
pub fn compare(a: &[u8], b: &[u8]) -> String {
    match a.iter().zip(b).position(|(x, y)| x != y) {
        None if a.len() == b.len() => "identical".to_owned(),
        None => format!("differ: sizes {} and {}, equal up to the shorter", a.len(), b.len()),
        Some(at) => format!(
            "differ: sizes {} and {}, first at byte {at:#x} ({:#04x}, {:#04x})",
            a.len(),
            b.len(),
            a[at],
            b[at]
        ),
    }
}

#[cfg(test)]
mod tests {
    // spec/harness.md "Checks" (WP-112): `check cmp` says whether two
    // files are the same bytes, and where they first differ.
    #[test]
    fn two_files_compared() {
        assert_eq!(super::compare(b"abc", b"abc"), "identical");
        assert_eq!(super::compare(b"", b""), "identical");
        assert_eq!(
            super::compare(b"abc", b"abd"),
            "differ: sizes 3 and 3, first at byte 0x2 (0x63, 0x64)"
        );
        assert_eq!(super::compare(b"ab", b"abc"), "differ: sizes 2 and 3, equal up to the shorter");
    }

    // WP-103: the sites of an output whose line has the option are counted
    // apart from those of an output whose line has not.
    #[test]
    fn wp103_erratum_counts() {
        let cov = |sites, not| mortar_plumb::report::Coverage {
            erratum_sites: sites,
            erratum_not_judged: not,
            ..Default::default()
        };
        let mut e = super::ErratumJudged::default();
        e.count(Some(true), &cov(3, 1));
        e.count(Some(false), &cov(0, 2));
        e.count(None, &cov(0, 4));
        let want = super::ErratumJudged {
            with_option: 1,
            without: 1,
            unknown: 1,
            sites: 3,
            not_judged: 1,
            sites_without: 6,
        };
        assert_eq!(e, want);
        let mut sum = super::ErratumJudged::default();
        sum.add(&e);
        sum.add(&e);
        assert_eq!(sum.sites_without, 12);
        assert_eq!(sum.with_option, 2);
    }

    // spec/harness.md "static-identical": gcc's resolution file is the
    // one word of a map replaced, and only in its own form.
    #[test]
    fn a_map_loses_only_gccs_resolution_file_name() {
        use super::{RESOLUTION_NAME, normalise_map};
        let map = "arg -plugin\narg -plugin-opt=-fresolution=/tmp/ccIJMg1f.res\narg a.o";
        let want = format!("arg -plugin\narg -plugin-opt=-fresolution={RESOLUTION_NAME}\narg a.o");
        assert_eq!(String::from_utf8(normalise_map(map.as_bytes())).unwrap(), want);
        let other = "arg -plugin-opt=-fresolution=/tmp/x/ccABC123.res\n";
        assert_ne!(normalise_map(other.as_bytes()), other.as_bytes(), "any directory");
        for kept in [
            "arg -plugin-opt=-fresolution=/tmp/ccABC12.res\n",
            "arg -plugin-opt=-fresolution=/tmp/ccABC1234.res\n",
            "arg -plugin-opt=-fresolution=/tmp/ccAB_123.res\n",
            "arg -plugin-opt=-fresolution=/tmp/ccABC123.o\n",
            "arg -plugin-opt=-fresolution=ccABC123.res\n",
            "arg x -plugin-opt=-fresolution=/tmp/ccABC123.res\n",
            "sym -plugin-opt=-fresolution=/tmp/ccABC123.res\n",
        ] {
            assert_eq!(normalise_map(kept.as_bytes()), kept.as_bytes(), "{kept}");
        }
    }

    // spec/harness.md "static-identical": the times mortar measures are
    // replaced in its standard error, and nothing else (WP-112).
    #[test]
    fn a_standard_error_loses_only_measured_times() {
        use super::{TIME_NAME, normalise_stderr};
        let t = TIME_NAME;
        let cases = [
            (
                "mortar: time-phases: 10 command line: wall 0.1 ms, cpu 0 ms, rss 3.1 MiB, peak 3.2 MiB\n",
                format!("mortar: time-phases: 10 command line: wall {t}\n"),
            ),
            (
                "mortar: time-phases: total: wall 4.0 ms, cpu 1 ms, peak 9.0 MiB",
                format!("mortar: time-phases: total: wall {t}"),
            ),
            (
                "mortar: odr-stats: mode warn; units 0; findings W1 0; time 0.016 ms\n",
                format!("mortar: odr-stats: mode warn; units 0; findings W1 0; time {t}\n"),
            ),
            (
                "incremental: full (no-state) changed=1 refilled=0 bytes=600 time=0.412\n",
                format!("incremental: full (no-state) changed=1 refilled=0 bytes=600 time={t}\n"),
            ),
            ("delta time detect=0.010 delta=0.020\n", format!("delta time {t}\n")),
        ];
        for (line, want) in cases {
            assert_eq!(String::from_utf8(normalise_stderr(line.as_bytes())).unwrap(), want);
        }
        for kept in [
            "mortar: error: undefined symbol: time\n",
            "mortar: warning: wall 0.1 ms\n",
            "a.o: incremental: full time=1\n",
            "mortar: odr-stats: mode off\n",
        ] {
            assert_eq!(normalise_stderr(kept.as_bytes()), kept.as_bytes(), "{kept}");
        }
    }

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
        let x = [
            ArchXfail { line: 3, arch: Arch::X86_64, test: None, reason: "no back end".to_owned() },
            ArchXfail {
                line: 4,
                arch: Arch::Aarch64,
                test: Some("dyn/a.test".to_owned()),
                reason: "M2: WP-053".to_owned(),
            },
        ];
        let t = "t.test";
        assert_eq!(mortar_xfail(Some(&own), &x, Arch::X86_64, t), Some("no back end"));
        assert_eq!(mortar_xfail(None, &x, Arch::X86_64, t), Some("no back end"));
        assert_eq!(mortar_xfail(Some(&own), &x, Arch::Aarch64, t), Some("no linking yet (M1)"));
        assert_eq!(mortar_xfail(None, &x, Arch::I386, t), None);
        // A test's line, on its architecture only; the run-wide line first.
        assert_eq!(mortar_xfail(None, &x, Arch::Aarch64, "dyn/a.test"), Some("M2: WP-053"));
        assert_eq!(mortar_xfail(None, &x, Arch::X86_64, "dyn/a.test"), Some("no back end"));
        assert_eq!(mortar_xfail(None, &x, Arch::I386, "dyn/a.test"), None);
    }

    // spec/harness.md "Runs": a program finds the fixture libraries in its
    // linker's directory; an `env` line adds a variable or replaces that.
    #[test]
    fn the_environment_of_a_run() {
        use super::super::format::{Template, Vars};
        use super::program_env;
        let vars =
            Vars { arch: Arch::X86_64, host: Some(Arch::Aarch64), linker: Some(Linker::Bfd) };
        let dir = std::path::Path::new("/w/u/x86_64/bfd");
        let none = program_env(dir, &[], vars);
        assert_eq!(none, [("LD_LIBRARY_PATH".to_owned(), "/w/u/x86_64/bfd".to_owned())]);
        let env = [
            ("LD_PRELOAD".to_owned(), Template::parse("./libhook.so").unwrap()),
            ("LD_BIND_NOW".to_owned(), Template::parse("1").unwrap()),
        ];
        let names: Vec<String> = program_env(dir, &env, vars).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["LD_LIBRARY_PATH", "LD_PRELOAD", "LD_BIND_NOW"]);
        let own = [("LD_LIBRARY_PATH".to_owned(), Template::parse("%{arch}").unwrap())];
        assert_eq!(
            program_env(dir, &own, vars),
            [("LD_LIBRARY_PATH".to_owned(), "x86_64".to_owned())]
        );
    }

    // LT-6: a native run's environment is set by `env` on the program's
    // own exec, never in the environment of the shell before it.
    #[test]
    fn a_native_runs_environment_is_the_programs_alone() {
        use std::ffi::OsString;
        let program = std::path::Path::new("/w/u/aarch64/bfd/prog");
        let (bare, args) = super::native_command(program, &[], vec!["a".to_owned()]);
        assert_eq!((bare.as_path(), args), (program, vec![OsString::from("a")]));
        let env = [
            ("LD_LIBRARY_PATH".to_owned(), "/w/u/aarch64/bfd".to_owned()),
            ("LD_PRELOAD".to_owned(), "./libpre.so".to_owned()),
        ];
        let (env_program, args) = super::native_command(program, &env, vec!["a".to_owned()]);
        assert_eq!(env_program, std::path::Path::new(super::ENV_PROGRAM));
        let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            args,
            [
                "LD_LIBRARY_PATH=/w/u/aarch64/bfd",
                "LD_PRELOAD=./libpre.so",
                "/w/u/aarch64/bfd/prog",
                "a"
            ]
        );
    }

    // LT-3: the pairs of each record, in order, and their directories.
    #[test]
    fn the_pairs_of_a_record() {
        use super::Pair;
        let all = [Linker::Bfd, Linker::Lld, Linker::Mold, Linker::Mortar];
        let p = |library, host| Pair { library, host };
        assert_eq!(Pair::of(Linker::Bfd, &all, &[], false), [p(Linker::Bfd, Linker::Bfd)]);
        assert_eq!(
            Pair::of(Linker::Lld, &all, &[Linker::Bfd], false),
            [p(Linker::Lld, Linker::Lld), p(Linker::Lld, Linker::Bfd)]
        );
        // mortar hosts the libraries of the references that passed first.
        assert_eq!(
            Pair::of(Linker::Mortar, &all, &[Linker::Bfd, Linker::Mold], false),
            [
                p(Linker::Bfd, Linker::Mortar),
                p(Linker::Mold, Linker::Mortar),
                p(Linker::Mortar, Linker::Mortar),
                p(Linker::Mortar, Linker::Bfd),
            ]
        );
        // Without GNU ld in the run, no library goes under its host; the
        // digest mode has mortar's own pair alone.
        assert_eq!(
            Pair::of(Linker::Mortar, &[Linker::Mortar], &[], false),
            [p(Linker::Mortar, Linker::Mortar)]
        );
        assert_eq!(
            Pair::of(Linker::Mortar, &all, &[Linker::Bfd], true),
            [p(Linker::Mortar, Linker::Mortar)]
        );
        assert_eq!(p(Linker::Mold, Linker::Mold).dir_name(), "mold");
        assert_eq!(p(Linker::Lld, Linker::Mortar).dir_name(), "lld+mortar");
        assert_eq!(p(Linker::Mortar, Linker::Bfd).label(), "[library mortar, host bfd]");
    }

    // LT-5: a pair's run against the baseline pair's: the same end and the
    // same output, a run the baseline did not make not compared.
    #[test]
    fn a_pairs_run_against_the_baseline() {
        use super::{Ran, against_baseline};
        use crate::process::End;
        let ran = |line, end, out: &str| Ran { line, end, stdout: out.as_bytes().to_vec() };
        let baseline = [ran(10, End::Exit(0), "7 7\n"), ran(12, End::Exit(3), "")];
        assert_eq!(
            against_baseline(&ran(10, End::Exit(0), "7 7\n"), Linker::Bfd, &baseline),
            Ok(())
        );
        let differs = against_baseline(&ran(10, End::Exit(0), "7 1\n"), Linker::Bfd, &baseline);
        assert_eq!(
            differs,
            Err("printed \"7 1\\n\", the bfd/bfd pair printed \"7 7\\n\"".to_owned())
        );
        let ends =
            against_baseline(&ran(12, End::Exit(0), ""), Linker::Lld, &baseline).unwrap_err();
        assert!(ends.contains("the lld/lld pair's program"), "{ends}");
        assert_eq!(against_baseline(&ran(14, End::Exit(1), "x"), Linker::Bfd, &baseline), Ok(()));
    }

    // "L3", "Stage 3": the verifier has stage 3 since WP-050.
    #[test]
    fn stage_3_is_present() {
        assert!(super::stage3_present());
    }

    use super::{Arch, Linker, Plumb, map_option, not_judged, output_name};

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_owned()).collect()
    }

    // spec/harness.md "L3": the map a link line already writes.
    #[test]
    fn the_map_of_a_link_line() {
        let map = |args: &[&str]| map_option(&strings(args));
        let mortar = |args: &[&str]| super::map_option_named(&strings(args), "mortar-map");
        assert_eq!(mortar(&["a.o", "--mortar-map=m.map"]), Some("m.map".to_owned()));
        assert_eq!(mortar(&["-Wl,--mortar-map,m.map"]), Some("m.map".to_owned()));
        assert_eq!(mortar(&["--mortar-map", "m.map", "-Map=x.map"]), Some("m.map".to_owned()));
        assert_eq!(mortar(&["-Map=x.map"]), None);
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
        let stale = plumb.stale_defaults(&defaults, Some(Arch::Aarch64));
        assert_eq!(stale.len(), 1, "{stale:?}");
        assert!(stale[0].contains("line 1 expects A3 of mold on aarch64"), "{stale:?}");
        plumb.defaults_seen.insert((Arch::Aarch64, Linker::Mold, "A3".to_owned()));
        assert!(plumb.stale_defaults(&defaults, Some(Arch::Aarch64)).is_empty());
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
