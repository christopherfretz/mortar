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

/// The version of mortar, as `--version` prints it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How a link ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The output was written, or the request (`--version`, `--help`) was
    /// answered. The text goes to standard output.
    Done(String),
    /// The link was refused or failed. Each line is one diagnostic for
    /// standard error. No output file was left behind.
    Failed(Vec<String>),
}

/// Runs one link. `args` is the command line without the program name.
#[must_use]
pub fn link(args: &[String]) -> Outcome {
    if args.iter().any(|a| a == "--version" || a == "-v" || a == "-V") {
        // "compatible with GNU ld" is what build systems and compiler
        // drivers grep for when they probe the linker.
        return Outcome::Done(format!("mortar {VERSION} (compatible with GNU ld)\n"));
    }
    Outcome::Failed(vec![
        "mortar: error: linking is not implemented yet (docs/ROADMAP.md, milestone M1)".to_owned(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_names_gnu_ld_compatibility() {
        let Outcome::Done(text) = link(&["--version".to_owned()]) else {
            panic!("--version must be answered");
        };
        assert!(text.starts_with("mortar "));
        assert!(text.contains("compatible with GNU ld"));
    }

    #[test]
    fn a_link_is_refused_not_faked() {
        let out = link(&["a.o".to_owned(), "-o".to_owned(), "a.out".to_owned()]);
        assert!(matches!(out, Outcome::Failed(lines) if lines[0].contains("not implemented")));
    }
}
