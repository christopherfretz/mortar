//! mortar: the linker of the Mason toolchain.
//!
//! This crate is the facade: one entry point, [`link`], that takes a command
//! line in GNU ld's dialect and produces an output file or diagnostics. The
//! map of the crates behind it is `docs/ARCHITECTURE.md`; the contracts are
//! in `spec/`.
//!
//! The front of the pipeline runs ([`front`], WP-027a): a link is laid
//! out, then refused at the output file, which WP-027b writes. Until a
//! construct is implemented, mortar refuses it by name (principle 1,
//! "correct, or loudly refused"): it never produces a file it cannot
//! vouch for.

pub mod front;
mod proof;
mod sites;
pub mod syms;
pub mod timing;

use std::ffi::{OsStr, OsString};
use std::path::Path;

pub use mortar_cli::Color;
use mortar_cli::{Exit, Plan};
use mortar_core::par::Pool;
use mortar_core::plan::Threads;
use mortar_input::store::{Store, StorePolicy};

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

/// Whether a refused use stops the line before the pipeline runs: every
/// refused option does, except `-o`, which only the output file reads
/// (`spec/cli.md` §4.3; WP-027b writes it).
fn blocks_the_front(u: &mortar_cli::Use) -> bool {
    !matches!(u.row().lands, mortar_cli::table::Lands::Output)
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
/// nothing to link. A line without errors, whose only refused option is
/// `-o`, runs the front of the pipeline ([`front::front`]) to the final
/// layout, and then ends, until WP-027b, in the refusal of writing the
/// output file.
#[must_use]
pub fn invoke(prog: &str, args: &[OsString]) -> Invocation {
    let cwd = std::env::current_dir().unwrap_or_default();
    invoke_in(prog, args, &cwd)
}

/// [`invoke`] with relative paths relative to `cwd`.
#[must_use]
pub fn invoke_in(prog: &str, args: &[OsString], cwd: &Path) -> Invocation {
    let parsed = mortar_cli::parse(args);
    let plan = &parsed.plan;
    let color = plan.diagnostics.color;
    let done =
        |text: String| Invocation { outcome: Outcome::Done(text), color, phases: String::new() };
    match plan.exit {
        Some(Exit::Version) if !parsed.failed() => return done(mortar_cli::version_text()),
        Some(Exit::Help) if !parsed.failed() => return done(mortar_cli::help_text(prog)),
        _ => {}
    }
    if let Some(style) = plan.print_version
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
    if parsed.refused_uses().into_iter().any(blocks_the_front) {
        emit_refusals(&diagnostics);
        diagnostics.emit(
            Key::new(mortar_cli::PHASE + 1, 0, 0),
            Diagnostic::refused(RefusalClass::Feature, "linking")
                .note("an option above is refused, so nothing is linked (docs/status/options.md)"),
        );
        return failed(diagnostics, String::new());
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
    let store = Store::new(StorePolicy::from_env());
    let env = front::Env { cwd, pool };
    let front = front::front(plan.clone(), env, &store, &diagnostics, &mut timer);
    if front.is_some() {
        diagnostics.emit(
            Key::new(100, 0, 0),
            Diagnostic::refused(RefusalClass::Feature, "writing the output file")
                .note("the link was laid out; the image and the output file are WP-027b"),
        );
    }
    drop(front);
    emit_refusals(&diagnostics);
    failed(diagnostics, timer.finish())
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
    // report), §4 (the refusal names what is refused); cli.md §4.3 (`-o`
    // is refused but does not stop the front: the link runs to the input
    // phase, which reports the missing file)
    #[test]
    fn a_link_is_refused_not_faked() {
        let Outcome::Failed(report) = link(&["a.o".into(), "-o".into(), "a.out".into()]) else {
            panic!("the output file is not written yet");
        };
        assert!(report.failed());
        let ids: Vec<&str> = report.diagnostics().iter().map(Diagnostic::id).collect();
        assert_eq!(ids, ["refused", "cannot-open-file", "no-target"]);
        assert_eq!(report.diagnostics()[0].refusal(), Some((RefusalClass::Option, "-o")));
        assert_eq!(
            report.render(&Style::plain("ld")),
            "ld: error: unsupported option: -o\n\
             ld: error: a.o: cannot open a.o: No such file or directory\n\
             ld: error: target emulation unknown: -m or at least one .o file required\n"
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
            failed(&["--frob", "-o", "x", "a.o", "-o", "y", "--hash-style"]),
            format!(
                "ld: error: unknown argument '--frob'\n\
                 ld: error: unsupported option: -o\n\
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

    // spec: cli.md §5 (--version and --help are answered; -v and -V when
    // nothing is to be linked)
    #[test]
    fn requests_are_answered() {
        let done = |args: &[&str]| {
            let args: Vec<OsString> = args.iter().map(OsString::from).collect();
            match invoke("ld", &args).outcome {
                Outcome::Done(text) => Some(text),
                Outcome::Failed(_) => None,
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
        // `-v` with an input links it (here: the input is missing).
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
