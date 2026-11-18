//! mortar: the linker of the Mason toolchain.
//!
//! This crate is the facade: one entry point, [`link`], that takes a command
//! line in GNU ld's dialect and produces an output file or diagnostics. The
//! map of the crates behind it is `docs/ARCHITECTURE.md`; the contracts are
//! in `spec/`.
//!
//! The whole pipeline runs: [`front`] (WP-027a) lays a link out, and
//! [`image`] (WP-027b, joined by WP-027c) writes the output file. Until a
//! construct is implemented, mortar refuses it by name (principle 1,
//! "correct, or loudly refused"): it never produces a file it cannot
//! vouch for.

pub mod arch;
pub mod depfile;
pub mod dynamic;
pub mod dynlink;
mod eh_owners;
pub mod floor;
pub mod front;
pub mod image;
pub mod incremental;
pub mod odr_runtime;
mod proof;
mod sites;
pub mod syms;
pub mod timing;
pub mod update;
pub mod written;

use std::ffi::{OsStr, OsString};
use std::path::Path;

pub use mortar_cli::Color;
use mortar_cli::{Exit, Plan};
use mortar_core::par::Pool;
use mortar_core::plan::Threads;
use mortar_input::store::{Hooks, Store, StoreConfig, StorePolicy};

use crate::timing::Timer;
use mortar_diag::{Collector, DEFAULT_ERROR_LIMIT, Diagnostic, Key, Policy, RefusalClass, Report};

/// The version of mortar, as `--version` prints it.
pub const VERSION: &str = mortar_cli::VERSION;

/// How a link ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The output was written, or the request (`--version`, `--help`) was
    /// answered. The text goes to standard output.
    Done(String),
    /// The output file was written. `report` holds the link's warnings
    /// (rendered to standard error; none is an error), `stdout` the map
    /// under `-M` (image.md I-77; empty from [`run`], which prints it
    /// itself, `spec/cli.md` §3.15). The exit status is 0.
    Linked { report: Report, stdout: String },
    /// The link was refused or failed (`spec/diag.md` §7). The report is
    /// rendered to standard error; nothing goes to standard output, the
    /// exit status is 1, and no output file was left behind.
    Failed(Report),
}

/// A link, with what the command line says about rendering its report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// How the link ended.
    pub outcome: Outcome,
    /// `--color-diagnostics[=when]`, `--no-color-diagnostics`
    /// (`spec/cli.md` §3.9): the caller decides `Auto` from its terminal.
    pub color: Color,
    /// `--time-phases` (`spec/image.md` I-88, I-89): the lines, each ending
    /// in a newline, for standard error after the report; empty without it.
    pub phases: String,
    /// `--print-gc-sections` (`spec/image.md` I-10): the lines, each ending
    /// in a newline, for standard error before the report; empty without
    /// it or when the link stopped before liveness.
    pub removed: String,
    /// Incremental mode's lines for standard error (`spec/incr.md` IN-10:
    /// a fall-back, the report when it goes there), each ending in a
    /// newline, after the report; empty in a plain link.
    pub incremental: String,
}

/// The name diagnostics start with (`spec/diag.md` §5.1): the basename
/// mortar was invoked as (`mortar`, `ld`, `ld.mortar`), from `argv[0]`.
#[must_use]
pub fn program_name(argv0: Option<&OsStr>) -> String {
    argv0
        .and_then(|path| Path::new(path).file_name())
        .map_or_else(|| "mortar".to_owned(), |name| name.to_string_lossy().into_owned())
}

/// Runs one link. `args` is the command line without the program name.
#[must_use]
pub fn link(args: &[OsString]) -> Outcome {
    invoke("mortar", args).outcome
}

/// The diagnostic policy a command line asks for (`spec/cli.md` §3.9).
fn policy(plan: &Plan) -> Policy {
    let d = &plan.diagnostics;
    Policy {
        error_limit: d.error_limit.unwrap_or(DEFAULT_ERROR_LIMIT),
        fatal_warnings: d.fatal_warnings,
        no_warnings: d.no_warnings,
    }
}

/// The pool a plan asks for (`--threads`, `spec/cli.md` §3.10), or the
/// error of a malformed `MORTAR_SCHED_SEED` (`spec/model.md` §6).
fn pool_of(plan: &Plan) -> Result<Pool, mortar_core::par::BadSeed> {
    let threads = match plan.bookkeeping.threads {
        None | Some(Threads::All) => Pool::available(),
        Some(Threads::Count(n)) => (n as usize).max(1),
    };
    Pool::try_new(threads)
}

/// Runs one link as `prog` (the name `--help` shows), and says how its
/// report is to be colored. `args` is the command line without the
/// program name; relative paths are relative to the current directory.
///
/// The command line is parsed (`spec/cli.md`); its errors, then the
/// options mortar refuses, are reported in argument order. `--version` and
/// `--help` are answered; `-v` and `-V` are answered when the line names
/// nothing to link. A line without errors or refused options runs the
/// pipeline: the front ([`front::front`]), then the image phase and the
/// output file ([`image::write`]), then the map (`-Map`, `-M`).
#[must_use]
pub fn invoke(prog: &str, args: &[OsString]) -> Invocation {
    let cwd = std::env::current_dir().unwrap_or_default();
    invoke_in(prog, args, &cwd)
}

/// [`invoke`] with relative paths relative to `cwd`.
#[must_use]
pub fn invoke_in(prog: &str, args: &[OsString], cwd: &Path) -> Invocation {
    invoke_with(prog, args, cwd, None, PrintMap::Capture, hooked_state5())
}

/// WP-244's switch from the test hooks (`MORTAR_INCR_STATE5`, a
/// `testhooks` build only; `None` otherwise).
fn hooked_state5() -> Option<incremental::State5> {
    mortar_incr::testhook::state5_path()
        .map(|path| incremental::State5 { path, collide: mortar_incr::testhook::state5_collide() })
}

/// [`invoke_in`] with WP-244's switch on (tests): an incremental-mode
/// full link also writes its format-5 state to `state5.path`.
#[must_use]
pub fn invoke_state5(
    prog: &str,
    args: &[OsString],
    cwd: &Path,
    state5: incremental::State5,
) -> Invocation {
    invoke_with(prog, args, cwd, None, PrintMap::Capture, Some(state5))
}

/// [`invoke`] as the executable runs it: `-M`'s map is written to standard
/// output (through `mortar_diag::stream`) before the link's verdict, so a
/// map that cannot be written fails the link by name and leaves no output
/// (`spec/cli.md` §3.15, S-2); [`Outcome::Linked`]'s `stdout` is then
/// empty. Everything else is [`invoke`]'s.
#[must_use]
pub fn run(prog: &str, args: &[OsString]) -> Invocation {
    let cwd = std::env::current_dir().unwrap_or_default();
    invoke_with(prog, args, &cwd, None, PrintMap::Stdout, hooked_state5())
}

/// Where `-M`'s map goes (`spec/cli.md` §3.15).
#[derive(Clone, Copy)]
enum PrintMap {
    /// Into [`Outcome::Linked`]'s `stdout`, for the caller.
    Capture,
    /// To the process's standard output, before the verdict.
    Stdout,
}

/// The replay hook of `spec/incr.md` IN-61 (model §7.3), for tests: the
/// link of `args` in `cwd`, with the decision tables of `state`
/// translated to its ids in place of the ones it computes. A row that does
/// not translate fails the link (`internal-error`, `replay: ...`).
#[must_use]
pub fn replay_link(
    prog: &str,
    args: &[OsString],
    cwd: &Path,
    state: &mortar_incr::format::State,
) -> Invocation {
    let hook = |link: &mortar_incr::capture::Link<'_, '_>,
                computed: &mortar_core::layout::Layout| {
        let keys = mortar_incr::capture::Keys::new(link)?;
        mortar_incr::capture::replay(link, &keys, state, computed)
    };
    invoke_with(prog, args, cwd, Some(&hook), PrintMap::Capture, None)
}

#[allow(clippy::too_many_lines, reason = "the steps of spec/cli.md §4.3 in order")]
fn invoke_with(
    prog: &str,
    args: &[OsString],
    cwd: &Path,
    replay: Option<&front::Replay<'_>>,
    print_to: PrintMap,
    state5: Option<incremental::State5>,
) -> Invocation {
    let parsed = mortar_cli::parse(args);
    let plan = &parsed.plan;
    let color = plan.diagnostics.color;
    let done = |text: String| Invocation {
        outcome: Outcome::Done(text),
        color,
        phases: String::new(),
        removed: String::new(),
        incremental: String::new(),
    };
    match plan.exit {
        Some(Exit::Version) if !parsed.failed() => return done(mortar_cli::version_text()),
        Some(Exit::Help) if !parsed.failed() => return done(mortar_cli::help_text(prog)),
        _ => {}
    }
    // cli.md §5.1: `--verbose` without inputs answers as `-V` does.
    let style = if plan.bookkeeping.verbose {
        Some(mortar_cli::VersionStyle::Emulations)
    } else {
        plan.print_version
    };
    if let Some(style) = style
        && !parsed.failed()
        && plan.inputs.is_empty()
    {
        return done(mortar_cli::print_version_text(style));
    }
    let diagnostics = Collector::new(policy(plan));
    for (key, diagnostic) in parsed.diagnostics.iter().cloned() {
        diagnostics.emit(key, diagnostic);
    }
    // The refusals of options come after the front, whose phases stop at
    // the first error (the report is in key order, whatever the order of
    // emission, `spec/diag.md` §6).
    let refusals = parsed.refusals();
    let failed = |diagnostics: Collector, phases: String| Invocation {
        outcome: Outcome::Failed(diagnostics.finish()),
        color,
        phases,
        removed: String::new(),
        incremental: String::new(),
    };
    let emit_refusals = |diagnostics: &Collector| {
        for (key, diagnostic) in refusals.iter().cloned() {
            diagnostics.emit(key, diagnostic);
        }
    };
    if parsed.failed() {
        emit_refusals(&diagnostics);
        return failed(diagnostics, String::new());
    }
    // An option the pipeline does not honor stops the line here, with the
    // refusal of linking itself (phase 11).
    if !refusals.is_empty() {
        emit_refusals(&diagnostics);
        diagnostics.emit(
            Key::new(mortar_cli::PHASE + 1, 0, 0),
            Diagnostic::refused(RefusalClass::Feature, "linking")
                .note("an option above is refused, so nothing is linked (docs/status/options.md)"),
        );
        return failed(diagnostics, String::new());
    }
    // cli.md §5.1: `-v` and `-V` on a line with inputs print their text
    // on standard output before the link, which then goes on; the exit
    // status is the link's. (Captured, it is in `Linked`'s `stdout`.)
    let mut stdout = String::new();
    if let Some(style) = plan.print_version {
        let text = mortar_cli::print_version_text(style);
        match print_to {
            PrintMap::Capture => stdout.push_str(&text),
            PrintMap::Stdout => {
                let _ = mortar_diag::stream::to_stdout(&text);
            }
        }
    }
    let pool = match pool_of(plan) {
        Ok(p) => p,
        Err(bad) => {
            emit_refusals(&diagnostics);
            diagnostics.emit(mortar_core::seed::SEED_KEY, bad.into());
            return failed(diagnostics, String::new());
        }
    };
    let mut timer = Timer::new(plan.bookkeeping.time_phases);
    timer.phase(10, "command line");
    // The default store is the staged one (`spec/landed/input/WP-021b.md`);
    // `MORTAR_STORE=resident` is the tests' switch to the other.
    // spec/meta.md MT-42, odr.md O-42: containers are kept only when a
    // record check reads them.
    // `--odr-runtime` (WP-198): the plan the line implies (the table, `-z
    // now`). Without the option, one field read and the plan as it is.
    let with_runtime = odr_runtime::effective(plan);
    let plan = with_runtime.as_ref().unwrap_or(plan);
    let config = StoreConfig {
        containers: mortar_odr::reads_containers(&plan.diagnostics.odr),
        ..StoreConfig::default()
    };
    let store = Store::with_config(StorePolicy::from_env(), config, Hooks::default());
    let out_path = cwd.join(plan.output.as_deref().unwrap_or_else(|| OsStr::new("a.out")));
    let map_path = plan.bookkeeping.map.as_deref().map(|m| cwd.join(m));
    let mortar_map_path = plan.bookkeeping.mortar_map.as_deref().map(|m| cwd.join(m));
    let dependency_path = plan.bookkeeping.dependency_file.as_deref().map(|m| cwd.join(m));
    let print_map = plan.bookkeeping.print_map;
    // spec/incr.md §5: nothing at all without --incremental (IN-11).
    let session = match incremental::start(&parsed, cwd, &out_path, &diagnostics) {
        incremental::Start::Plain => None,
        incremental::Start::Full(mut s) => {
            s.set_state5(state5);
            Some(s)
        }
        incremental::Start::Failed => {
            emit_refusals(&diagnostics);
            return failed(diagnostics, timer.finish());
        }
    };
    let env = front::Env {
        cwd,
        pool,
        replay,
        slack: session.is_some(),
        state5: session.as_ref().is_some_and(incremental::Session::state5),
    };
    let mut front = front::front(plan.clone(), env, &store, &diagnostics, &mut timer);
    // The map's `arg` records, in every map (`spec/plumb.md` §3.2a).
    if let Some(f) = &mut front {
        f.args = parsed.args.iter().map(|a| a.as_encoded_bytes().to_vec()).collect();
    }
    if let Some(f) = &mut front {
        f.keep_leaves = session.is_some();
    }
    let mut incremental_lines = String::new();
    let removed = front.as_ref().map_or_else(String::new, |f| removed_lines(prog, f));
    // cli.md §3.14: no file the link writes is a file it read or another
    // file it writes, decided before anything is written.
    let reads = written::Reads::new(&parsed, cwd, store.inodes());
    if let Some(f) = &front
        && written::clear(
            &written::registry(
                &parsed,
                cwd,
                &out_path,
                &session.as_ref().map(incremental::Session::written).unwrap_or_default(),
            ),
            &reads,
            &f.inputs.link.opened,
            cwd,
            &diagnostics,
        )
    {
        let linked = link_image(f, &store, &out_path, &diagnostics, &mut timer);
        if let Some((maps, written)) = linked {
            // I-79, I-120: the maps of a successful link only.
            for (path, text) in [(&map_path, &maps.text), (&mortar_map_path, &maps.machine)] {
                if let Some(m) = path
                    && let Err((k, d)) = mortar_image::output::write_whole(m, text, 0o666)
                {
                    diagnostics.emit(k, d);
                }
            }
            // input.md §10.4: the dependency file, of a successful link.
            if let Some(d) = &dependency_path {
                write_dependency_file(f, d, &diagnostics);
            }
            // cli.md §5.1: `--verbose`'s text (a message, S-1) of a link
            // that succeeded, before -M's map.
            if plan.bookkeeping.verbose && !diagnostics.has_errors() {
                let text = mortar_cli::print_version_text(mortar_cli::VersionStyle::Emulations);
                match print_to {
                    PrintMap::Capture => stdout.push_str(&text),
                    PrintMap::Stdout => {
                        let _ = mortar_diag::stream::to_stdout(&text);
                    }
                }
            }
            // cli.md §3.15 (S-2): -M's map is an output; written last of
            // the outputs that can fail the link, before the state.
            if print_map && !diagnostics.has_errors() {
                let text = String::from_utf8_lossy(&maps.text);
                match print_to {
                    PrintMap::Capture => stdout.push_str(&text),
                    PrintMap::Stdout => {
                        if let Err(e) = mortar_diag::stream::to_stdout(&text) {
                            diagnostics.emit(
                                Key::new(mortar_image::PHASE_OUTPUT, 0, 0),
                                Diagnostic::error(mortar_diag::Kind::CannotWriteOutput {
                                    path: "standard output (-M)".to_owned(),
                                    reason: mortar_diag::os_reason(&e),
                                }),
                            );
                        }
                    }
                }
            }
            // spec/incr.md IN-17, IN-21: the state, of a successful link.
            if let Some(s) = session
                && !diagnostics.has_errors()
            {
                let laps = s.times_handle();
                incremental_lines = s.finish(f, &store, &written, cwd, &out_path, &diagnostics);
                if timer.on() {
                    for (name, ms) in laps.borrow().iter() {
                        timer.line(format!("mortar: incremental: {name}: {ms:.1} ms"));
                    }
                }
                timer.phase(110, "incremental state");
            }
        }
    }
    drop(front);
    emit_refusals(&diagnostics);
    if diagnostics.has_errors() {
        // I-66: a link that fails after the command line leaves no output
        // (GNU ld's choice; never an input, never a non-regular file).
        mortar_image::output::remove_stale_output(&out_path, &reads.identities());
        return Invocation {
            removed,
            incremental: incremental_lines,
            ..failed(diagnostics, timer.finish())
        };
    }
    Invocation {
        outcome: Outcome::Linked { report: diagnostics.finish(), stdout },
        color,
        phases: timer.finish(),
        removed,
        incremental: incremental_lines,
    }
}

/// `--print-gc-sections`' lines (`spec/image.md` I-10), when asked for
/// with `--gc-sections`.
fn removed_lines(prog: &str, f: &front::Front<'_>) -> String {
    if !(f.plan.shaping.gc_sections && f.plan.shaping.print_gc_sections) {
        return String::new();
    }
    mortar_live::print_gc_sections(prog, &f.inputs, &f.resolution, &f.assignment, &f.liveness)
}

/// `--dependency-file` (`spec/input.md` §10.4) of the link `f` to `path`.
fn write_dependency_file(f: &front::Front<'_>, path: &Path, diagnostics: &Collector) {
    let target = f.plan.output.as_deref().unwrap_or_else(|| OsStr::new("a.out"));
    // WP-150: the `--version-script` and `--dynamic-list` files, in
    // command-line order before the inputs (GNU ld's order; lld lists
    // them before the inputs too, mold sorted), each once.
    let scripts = f.plan.scripts.iter().filter(|s| {
        matches!(
            s.role,
            mortar_core::plan::ScriptRole::VersionScript
                | mortar_core::plan::ScriptRole::DynamicList
        )
    });
    let mut seen = std::collections::HashSet::new();
    let paths: Vec<&[u8]> = scripts
        .map(|s| s.path.as_encoded_bytes())
        .chain(mortar_input::dependency_paths(&f.inputs.link))
        // The runtime input (WP-198) is no file: Make has nothing to stat.
        .filter(|p| *p != odr_runtime::NAME.as_bytes())
        .filter(|p| seen.insert(*p))
        .collect();
    match depfile::render(target.as_encoded_bytes(), &paths) {
        Ok(text) => {
            if let Err((k, d)) = mortar_image::output::write_whole(path, &text, 0o666) {
                diagnostics.emit(k, d);
            }
        }
        Err(depfile::Unwritable(p)) => diagnostics.emit(
            Key::new(mortar_cli::PHASE + 1, 0, 0),
            Diagnostic::refused(RefusalClass::Feature, "dependency file path with a newline").note(
                format!("{:?} cannot be written as a Make word", String::from_utf8_lossy(&p)),
            ),
        ),
    }
}

/// The maps a link writes: the text of `-Map`/`-M` (I-121) and
/// `mortar-map 2` for `--mortar-map` (I-77); each empty when not asked for.
#[derive(Default)]
struct Maps {
    text: Vec<u8>,
    machine: Vec<u8>,
}

/// The image phase of a laid-out link (100, 105): the output file at
/// `path`, and the maps when the link succeeded (rendered from the
/// tables before fill, I-79, I-122).
fn link_image(
    f: &front::Front<'_>,
    store: &Store,
    path: &Path,
    diagnostics: &Collector,
    timer: &mut Timer,
) -> Option<(Maps, mortar_image::link::Written)> {
    let b = &f.plan.bookkeeping;
    let mut maps = Maps::default();
    let failed = |e: mortar_synth::error::SynthError| {
        diagnostics.emit(mortar_synth::error::key(0), e.into_diagnostic());
    };
    if b.map.is_some() || b.print_map {
        let output = f.plan.output.as_deref().unwrap_or_else(|| OsStr::new("a.out"));
        match image::text_map_of(f, store, output.as_encoded_bytes()) {
            Ok(m) => maps.text = mortar_image::textmap::render(&m, &f.pool),
            Err(e) => {
                failed(e);
                return None;
            }
        }
    }
    if b.mortar_map.is_some() {
        match image::map_of(f, store) {
            Ok(m) => maps.machine = mortar_image::map::render(&m, &f.pool),
            Err(e) => {
                failed(e);
                return None;
            }
        }
    }
    match image::write(f, store, path, &|_| {}, &mut |n, name| timer.phase(n, name)) {
        Ok(written) => Some((maps, written)),
        Err(e) => {
            for (k, d) in e.diagnostics {
                diagnostics.emit(k, d);
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mortar_diag::Style;

    #[test]
    fn version_names_gnu_ld_compatibility() {
        let Outcome::Done(text) = link(&["--version".into()]) else {
            panic!("--version must be answered");
        };
        assert!(text.starts_with("mortar "));
        assert!(text.contains("compatible with GNU ld"));
    }

    // spec: diag.md §7 (a failed link is `Outcome::Failed` with the
    // report); input.md §3.1 rule 4 (a missing file is `cannot-open-file`
    // alone, not also `no-target`)
    #[test]
    fn a_missing_input_is_cannot_open_alone() {
        let Outcome::Failed(report) = link(&["a.o".into(), "-o".into(), "a.out".into()]) else {
            panic!("a missing input fails the link");
        };
        assert!(report.failed());
        let ids: Vec<&str> = report.diagnostics().iter().map(Diagnostic::id).collect();
        assert_eq!(ids, ["cannot-open-file"]);
        assert_eq!(
            report.render(&Style::plain("ld")),
            "ld: error: a.o: cannot open a.o: No such file or directory\n"
        );
    }

    fn failed(args: &[&str]) -> String {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let Outcome::Failed(report) = link(&args) else { panic!("{args:?} must fail") };
        report.render(&Style::plain("ld"))
    }

    // spec: cli.md §2.1 (errors in argument order, refusals among them),
    // §4.4 (each refused option once), §4.3 (a line with errors is not
    // linked)
    #[test]
    fn errors_come_in_argument_order() {
        assert_eq!(
            failed(&["--frob", "-o", "x", "a.o", "-z", "nodump", "-z", "nodump", "--hash-style"]),
            format!(
                "ld: error: unknown argument '--frob'\n\
                 ld: error: unsupported option: -z nodump\n\
                 ld: error: --hash-style: missing argument\n"
            )
        );
    }

    // spec: cli.md §3.9 (the diagnostic switches are applied to the
    // report)
    #[test]
    fn the_diagnostic_switches_apply() {
        let report = failed(&["--error-limit=1", "--frob", "--frab", "a.o"]);
        assert!(report.starts_with("ld: error: unknown argument '--frob'\n"), "{report}");
        assert!(!report.contains("--frab"), "{report}");
    }

    // spec: cli.md §5 (--version and --help are answered; -v and -V alone
    // when nothing is to be linked)
    #[test]
    fn requests_are_answered() {
        let done = |args: &[&str]| {
            let args: Vec<OsString> = args.iter().map(OsString::from).collect();
            match invoke("ld", &args).outcome {
                Outcome::Done(text) => Some(text),
                Outcome::Failed(_) | Outcome::Linked { .. } => None,
            }
        };
        assert_eq!(done(&["--version", "a.o"]), Some(mortar_cli::version_text()));
        assert_eq!(done(&["-v"]), Some(mortar_cli::version_text()));
        assert_eq!(
            done(&["-V"]),
            Some(mortar_cli::print_version_text(mortar_cli::VersionStyle::Emulations))
        );
        assert_eq!(done(&["--help"]), Some(mortar_cli::help_text("ld")));
        assert_eq!(done(&["-v", "a.o"]), None);
        assert_eq!(done(&["--frob", "--version"]), None);
        // `-v` with an input prints the line and links it (here: the
        // input is missing, so the link fails; spec/cli.md §5.1).
        assert!(failed(&["-v", "a.o"]).contains("ld: error: a.o: cannot open a.o"));
    }

    // spec: cli.md §3.9 (the color choice is the caller's to apply)
    #[test]
    fn the_color_choice_is_returned() {
        let color = |args: &[&str]| {
            invoke("ld", &args.iter().map(OsString::from).collect::<Vec<_>>()).color
        };
        assert_eq!(color(&["a.o"]), Color::Auto);
        assert_eq!(color(&["--color-diagnostics", "a.o"]), Color::Always);
        assert_eq!(color(&["--color-diagnostics=never", "a.o"]), Color::Never);
        assert_eq!(color(&["--color-diagnostics", "--no-color-diagnostics"]), Color::Never);
    }

    // spec: diag.md §5.1 (the program name is the basename of `argv[0]`)
    #[test]
    fn program_name_is_the_basename() {
        let name = |argv0: &str| program_name(Some(OsStr::new(argv0)));
        assert_eq!(name("mortar"), "mortar");
        assert_eq!(name("/usr/bin/ld"), "ld");
        assert_eq!(name("./build/ld.mortar"), "ld.mortar");
        assert_eq!(name("../x/aarch64-linux-gnu-ld"), "aarch64-linux-gnu-ld");
        assert_eq!(program_name(None), "mortar");
        assert_eq!(name(""), "mortar");
        assert_eq!(name("/"), "mortar");
    }
}
