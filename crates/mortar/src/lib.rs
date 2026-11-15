//! mortar: the linker of the Mason toolchain.
//!
//! This crate is the facade: one entry point, [`link`], that takes a command
//! line in GNU ld's dialect and produces an output file or diagnostics. The
//! map of the crates behind it is `docs/ARCHITECTURE.md`; the contracts are
//! in `spec/`.
//!
//! Nothing is implemented yet. Until a construct is implemented, mortar
//! refuses it by name (principle 1, "correct, or loudly refused"): it never
//! produces a file it cannot vouch for.

use std::ffi::OsStr;
use std::path::Path;

use mortar_diag::{Collector, Diagnostic, Key, Policy, RefusalClass, Report};

/// The version of mortar, as `--version` prints it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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
pub fn link(args: &[String]) -> Outcome {
    if args.iter().any(|a| a == "--version" || a == "-v" || a == "-V") {
        // "compatible with GNU ld" is what build systems and compiler
        // drivers grep for when they probe the linker.
        return Outcome::Done(format!("mortar {VERSION} (compatible with GNU ld)\n"));
    }
    let diagnostics = Collector::new(Policy::default());
    diagnostics.emit(
        Key::new(0, 0, 0),
        Diagnostic::refused(RefusalClass::Feature, "linking")
            .note("no link is implemented yet (docs/ROADMAP.md, milestone M1)"),
    );
    Outcome::Failed(diagnostics.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mortar_diag::Style;

    #[test]
    fn version_names_gnu_ld_compatibility() {
        let Outcome::Done(text) = link(&["--version".to_owned()]) else {
            panic!("--version must be answered");
        };
        assert!(text.starts_with("mortar "));
        assert!(text.contains("compatible with GNU ld"));
    }

    // spec: diag.md §7 (a failed link is `Outcome::Failed` with the
    // report), §4 (the refusal names what is refused)
    #[test]
    fn a_link_is_refused_not_faked() {
        let Outcome::Failed(report) =
            link(&["a.o".to_owned(), "-o".to_owned(), "a.out".to_owned()])
        else {
            panic!("no link is implemented");
        };
        assert!(report.failed());
        let [diagnostic] = report.diagnostics() else {
            panic!("one diagnostic, got {:?}", report.diagnostics());
        };
        assert_eq!(diagnostic.id(), "refused");
        assert_eq!(diagnostic.refusal(), Some((RefusalClass::Feature, "linking")));
        assert_eq!(
            report.render(&Style::plain("ld")),
            "ld: error: unsupported feature: linking\n\
             >>> no link is implemented yet (docs/ROADMAP.md, milestone M1)\n"
        );
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
