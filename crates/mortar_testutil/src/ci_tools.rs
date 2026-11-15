//! The tool guard (`spec/harness.md` "Hosted CI", "The tool guard"): the
//! names a test may look a tool up by are the ones `scripts/ci-tools.sh`
//! installs and checks on the hosted lanes.
//!
//! The script is compiled in, and its lists are read here: `TOOLS` (on
//! `PATH` by that name), `TRIPLE_TOOLS` (`<triple>-<tool>` for each of
//! `TRIPLES`), `LLVM_TOOLS` (bare or `-<major>`), `SETUP_TOOLS` (given by
//! the job's Rust set-up) and `FALLBACK_TOOLS` (looked up only when a
//! listed tool is missing). [`crate::tool`] and [`crate::find_listed_tool`]
//! fail on a name no list covers, on every machine: a tool the development
//! machine has and the hosted lanes would lack is caught by the first test
//! that asks for it, not by a red hosted run.

/// `scripts/ci-tools.sh`, as built.
const SCRIPT: &str = include_str!("../../../scripts/ci-tools.sh");

/// The words of the script's one-line array `name=(...)`.
///
/// # Panics
/// Panics when the script has no such line: the guard cannot work
/// without it.
fn list(script: &'static str, name: &str) -> Vec<&'static str> {
    let prefix = format!("{name}=(");
    let Some((words, _)) = script
        .lines()
        .find_map(|line| line.trim().strip_prefix(prefix.as_str()))
        .and_then(|rest| rest.split_once(')'))
    else {
        panic!("scripts/ci-tools.sh has no one-line list `{name}=(...)`")
    };
    words.split_whitespace().collect()
}

/// The script's lists.
#[derive(Debug)]
pub struct Lists {
    pub tools: Vec<&'static str>,
    pub triples: Vec<&'static str>,
    pub triple_tools: Vec<&'static str>,
    pub llvm_tools: Vec<&'static str>,
    pub setup_tools: Vec<&'static str>,
    pub fallback_tools: Vec<&'static str>,
}

impl Lists {
    /// The lists of `script`.
    fn of(script: &'static str) -> Self {
        Self {
            tools: list(script, "TOOLS"),
            triples: list(script, "TRIPLES"),
            triple_tools: list(script, "TRIPLE_TOOLS"),
            llvm_tools: list(script, "LLVM_TOOLS"),
            setup_tools: list(script, "SETUP_TOOLS"),
            fallback_tools: list(script, "FALLBACK_TOOLS"),
        }
    }

    /// The lists of the `scripts/ci-tools.sh` this crate was built with.
    pub fn get() -> &'static Self {
        static LISTS: std::sync::OnceLock<Lists> = std::sync::OnceLock::new();
        LISTS.get_or_init(|| Self::of(SCRIPT))
    }

    /// Whether `name` is covered.
    pub fn covers(&self, name: &str) -> bool {
        if [&self.tools, &self.setup_tools, &self.fallback_tools]
            .iter()
            .any(|list| list.contains(&name))
        {
            return true;
        }
        let triple_tool = self.triples.iter().any(|triple| {
            name.strip_prefix(triple)
                .and_then(|rest| rest.strip_prefix('-'))
                .is_some_and(|tool| self.triple_tools.contains(&tool))
        });
        if triple_tool {
            return true;
        }
        let bare = match name.rsplit_once('-') {
            Some((bare, major)) if crate::LLVM_MAJORS.iter().any(|m| m.to_string() == major) => {
                bare
            }
            _ => name,
        };
        self.llvm_tools.contains(&bare)
    }
}

/// Whether `scripts/ci-tools.sh` covers the tool name `name`.
pub fn covers(name: &str) -> bool {
    Lists::get().covers(name)
}

/// Fails when `scripts/ci-tools.sh` does not cover `name`.
///
/// # Panics
/// Panics with what to do when it does not.
pub fn assert_listed(name: &str) {
    assert!(
        covers(name),
        "tool `{name}` is not in scripts/ci-tools.sh: add it to one of its lists \
         (TOOLS, TRIPLE_TOOLS, LLVM_TOOLS, SETUP_TOOLS, FALLBACK_TOOLS) and its package to packages(), \
         or the hosted lanes will not have it (spec/harness.md \"Hosted CI\", \"The tool guard\")"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec/harness.md "Hosted CI", "The tool guard": each list's form,
    // and what is covered.
    #[test]
    fn the_lists_cover_what_the_tests_ask_for() {
        let lists = Lists::get();
        assert_eq!(lists.triples, ["aarch64-linux-gnu", "x86_64-linux-gnu", "i686-linux-gnu"]);
        for name in [
            "gcc",
            "readelf",
            "gdb",
            "eu-elflint",
            "qemu-aarch64",
            "x86_64-linux-gnu-gcc",
            "aarch64-linux-gnu-ld.bfd",
            "aarch64-linux-gnu-ld",
            "sha256sum",
            "mkfifo",
            "ps",
            "clang-19",
            "clang++-19",
            "i686-linux-gnu-gfortran",
            "clang",
            "clang-22",
            "clang++",
            "ld.lld-22",
            "lldb",
            "llvm-dwarfdump",
            "rustc",
            "ld.mold",
        ] {
            assert!(covers(name), "{name}");
        }
        for name in [
            "gccx",
            "x86_64-linux-gnu-clang",
            "arm-linux-gnueabi-gcc",
            "clang-18",
            "llvm-strip",
            "x86_64-linux-gnu-",
            "",
        ] {
            assert!(!covers(name), "{name}");
        }
    }

    #[test]
    fn a_list_is_one_line() {
        let script = "A=(x y)\n  B=(z)  # note\nC=(\n";
        assert_eq!(list(script, "A"), ["x", "y"]);
        assert_eq!(list(script, "B"), ["z"]);
        let missing = std::panic::catch_unwind(|| list(script, "C"));
        assert!(missing.is_err(), "a list over several lines is not read");
    }

    #[test]
    #[should_panic(expected = "tool `strace` is not in scripts/ci-tools.sh")]
    fn an_unlisted_tool_fails() {
        assert_listed("strace");
    }
}
