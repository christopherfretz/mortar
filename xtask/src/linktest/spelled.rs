//! Which options mortar accepts that no link test spells
//! (`spec/harness.md` "Every accepted option is spelled", WP-112).
//!
//! The rows of `docs/status/options.md` whose status is `supported`,
//! `partial` or `noop` are the options mortar accepts. A row is
//! *spelled* when some link step of some test under `tests/link` writes
//! one of the row's spellings, in any form the linkers take it:
//!
//! - a long option (`--opt` or `-opt`, a name of two or more characters)
//!   with either one dash or two, alone, as `--opt=value`, or followed
//!   by its value; a `glued` value (`--lto-O2`) as a prefix;
//! - a one-letter option (`-x`), alone, or with its value glued (`-L.`)
//!   when it takes one;
//! - a `-z` keyword (`-z kw`) as `-z kw`, `-zkw`, `-z kw=value` or
//!   `-zkw=value`;
//! - in a driver's link (`link gcc`, `g++`, `gfortran`, `clang`,
//!   `clang++`), the words of `-Wl,a,b` and of `-Xlinker a`, and the
//!   driver options GCC hands the linker in the same spelling (`-static`,
//!   `-pie`, `-no-pie`, `-shared`, `-s`, `-r`, `-e`, `-u`, `-T`, `-L`,
//!   `-l`, `-o`; `-rdynamic` as `-export-dynamic`). What a driver passes
//!   on its own (`--eh-frame-hdr`, `--build-id`, `-plugin`, ...) is not
//!   counted: a test spells only what its file says.
//!
//! A word that is a substitution (`%{dynamic-linker}`) is no option.
//! `cargo xtask spelled` prints the rows no test spells; the test
//! `every_accepted_option_is_spelled_by_a_link_test` (in `cargo test`,
//! and so in the gate) wants none.

use std::path::Path;

use super::format::{self, Driver, StepKind};

/// How a row's option takes its value (the table's "Value" column).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Takes {
    /// No value.
    Nothing,
    /// A value, separate or after `=` (or glued, for one letter).
    Value,
    /// An optional value, after `=` only.
    Optional,
    /// A value glued to the option (`--lto-O2`).
    Glued,
}

/// An accepted row of the options table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The spellings as the table writes them: `-x`, `--discard-all`.
    pub spellings: Vec<String>,
    /// `supported`, `partial` or `noop`.
    pub status: String,
    takes: Takes,
}

impl Row {
    /// `` `-x`, `--discard-all` (supported) ``: the row in a message.
    pub fn describe(&self) -> String {
        let names: Vec<String> = self.spellings.iter().map(|s| format!("`{s}`")).collect();
        format!("{} ({})", names.join(", "), self.status)
    }
}

/// The accepted rows of `docs/status/options.md` (its text).
pub fn accepted_rows(table: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    for line in table.lines() {
        let cells: Vec<&str> = line.split(" | ").collect();
        if !line.starts_with("| `") || cells.len() < 7 {
            continue;
        }
        let status = cells[4].trim();
        let status = status.split(':').next().unwrap_or(status).trim();
        if !matches!(status, "supported" | "partial" | "noop") {
            continue;
        }
        let spellings = cells[0]
            .trim_start_matches('|')
            .split(", ")
            .map(|s| s.trim().trim_matches('`').to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        let value = cells[1].trim();
        let takes = if value.is_empty() {
            Takes::Nothing
        } else if value.starts_with("optional") {
            Takes::Optional
        } else if value.starts_with("glued") {
            Takes::Glued
        } else {
            Takes::Value
        };
        rows.push(Row { spellings, status: status.to_owned(), takes });
    }
    rows
}

/// The words a link step hands the linker, as far as the test file says.
fn linker_words(driver: Driver, args: &[String]) -> Vec<String> {
    if driver == Driver::Ld {
        return args.to_vec();
    }
    let mut words = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if let Some(list) = arg.strip_prefix("-Wl,") {
            words.extend(list.split(',').map(str::to_owned));
        } else if arg == "-Xlinker" {
            words.extend(rest.next().cloned());
        } else if arg == "-rdynamic" {
            words.push("-export-dynamic".to_owned());
        } else if ["-static", "-pie", "-no-pie", "-shared", "-s", "-r"].contains(&arg.as_str()) {
            words.push(arg.clone());
        } else if ["-e", "-u", "-T", "-o"].contains(&arg.as_str()) {
            words.push(arg.clone());
            words.extend(rest.next().cloned());
        } else if arg.starts_with("-L") || arg.starts_with("-l") || arg.starts_with("-o") {
            words.push(arg.clone());
        }
    }
    words
}

/// Whether `words` spell `spelling` of an option that takes `takes`.
fn spells(words: &[String], spelling: &str, takes: Takes) -> bool {
    if let Some(keyword) = spelling.strip_prefix("-z ") {
        let is_keyword = |word: &str| {
            word == keyword || word.strip_prefix(keyword).is_some_and(|v| v.starts_with('='))
        };
        return words.iter().enumerate().any(|(at, word)| {
            (word == "-z" && words.get(at + 1).is_some_and(|next| is_keyword(next)))
                || word
                    .strip_prefix("-z")
                    .is_some_and(|glued| !glued.is_empty() && is_keyword(glued))
        });
    }
    let name = spelling.trim_start_matches('-');
    if name.chars().count() == 1 {
        return words.iter().any(|word| {
            word == spelling
                || (takes != Takes::Nothing
                    && word.strip_prefix(spelling).is_some_and(|value| !value.is_empty()))
        });
    }
    words.iter().any(|word| {
        let Some(bare) = word.strip_prefix("--").or_else(|| word.strip_prefix('-')) else {
            return false;
        };
        match takes {
            Takes::Glued => bare.starts_with(name),
            Takes::Nothing => bare == name,
            Takes::Value | Takes::Optional => {
                bare == name || bare.strip_prefix(name).is_some_and(|v| v.starts_with('='))
            }
        }
    })
}

/// The link steps' words of every test file under `dir` (recursively).
///
/// # Errors
/// A test that cannot be read or does not parse.
pub fn link_lines(dir: &Path) -> Result<Vec<Vec<String>>, String> {
    let files = super::collect(&[dir.to_path_buf()], &[])?;
    let mut lines = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file.path)
            .map_err(|e| format!("cannot read '{}': {e}", file.path.display()))?;
        let parsed = format::parse(&text).map_err(|errors| {
            format!("'{}' does not parse ({} errors)", file.name, errors.len())
        })?;
        for step in &parsed.steps {
            if let StepKind::Link(link) | StepKind::Host(link) = &step.kind {
                let args: Vec<String> = link
                    .args
                    .iter()
                    .map(|arg| arg.literal().map_or_else(|| "%".to_owned(), str::to_owned))
                    .collect();
                lines.push(linker_words(link.driver, &args));
            }
        }
    }
    Ok(lines)
}

/// The accepted rows of `table` that none of `lines` spells.
pub fn unspelled(table: &str, lines: &[Vec<String>]) -> Vec<Row> {
    accepted_rows(table)
        .into_iter()
        .filter(|row| {
            !row.spellings
                .iter()
                .any(|spelling| lines.iter().any(|words| spells(words, spelling, row.takes)))
        })
        .collect()
}

/// The workspace's unspelled rows.
///
/// # Errors
/// The table or a test cannot be read.
pub fn workspace_unspelled() -> Result<(usize, Vec<Row>), String> {
    let root = super::workspace_root();
    let table_path = root.join("docs").join("status").join("options.md");
    let table = std::fs::read_to_string(&table_path)
        .map_err(|e| format!("cannot read '{}': {e}", table_path.display()))?;
    let lines = link_lines(&root.join("tests").join("link"))?;
    Ok((accepted_rows(&table).len(), unspelled(&table, &lines)))
}

/// `cargo xtask spelled`: prints the accepted rows no link test spells,
/// one per line, and a count; exit status 1 when there is one.
pub fn main(args: &[String]) -> u8 {
    if !args.is_empty() {
        eprintln!("usage: cargo xtask spelled");
        return 2;
    }
    match workspace_unspelled() {
        Ok((accepted, rows)) => {
            for row in &rows {
                println!("{}", row.describe());
            }
            println!("spelled: {} of {accepted} accepted rows spelled by no link test", rows.len());
            u8::from(!rows.is_empty())
        }
        Err(message) => {
            eprintln!("xtask spelled: {message}");
            3
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    // spec/harness.md "Every accepted option is spelled": the forms of a
    // spelling.
    #[test]
    fn the_forms_of_a_spelling() {
        let table = "\
| `-x`, `--discard-all` |  | GNU | all | supported | `a` | h |
| `-L`, `--library-path` | DIR | GNU | all | supported | `a` | h |
| `--build-id` | optional, `=` only: STYLE | GNU | all | partial: `=md5` refused | `a` | h |
| `-z max-page-size` | SIZE | GNU | all | supported | `a` | h |
| `-z defs` |  | GNU | all | noop: why | `a` | h |
| `--lto-O` | glued: LEVEL (number) | lld | all | noop: x | `a` | h |
| `-r` |  | GNU | all | refused | `a` | h |
";
        let rows = accepted_rows(table);
        assert_eq!(rows.len(), 6, "a refused row is not accepted");
        assert_eq!(rows[0].spellings, ["-x", "--discard-all"]);
        assert_eq!(rows[2].status, "partial");
        let left = |line: &str| -> Vec<String> {
            unspelled(table, &[words(line)]).iter().map(|row| row.spellings[0].clone()).collect()
        };
        let all = ["-x", "-L", "--build-id", "-z max-page-size", "-z defs", "--lto-O"];
        assert_eq!(left("a.o"), all);
        assert!(!left("-discard-all").contains(&"-x".to_owned()), "one dash");
        assert!(!left("--discard-all").contains(&"-x".to_owned()));
        assert!(left("-xy").contains(&"-x".to_owned()), "a flag is not glued");
        assert!(!left("-L.").contains(&"-L".to_owned()), "a value glued to one letter");
        assert!(!left("--library-path=d").contains(&"-L".to_owned()));
        assert!(!left("-build-id=sha1").contains(&"--build-id".to_owned()));
        assert!(left("--build-idx").contains(&"--build-id".to_owned()));
        assert!(!left("-z max-page-size=4096").contains(&"-z max-page-size".to_owned()));
        assert!(!left("-zmax-page-size=4096").contains(&"-z max-page-size".to_owned()));
        assert!(!left("-zdefs").contains(&"-z defs".to_owned()));
        assert!(left("-z undefs").contains(&"-z defs".to_owned()));
        assert!(left("-z").contains(&"-z defs".to_owned()));
        assert!(!left("--lto-O2").contains(&"--lto-O".to_owned()));
    }

    // spec/harness.md "Every accepted option is spelled": a driver's link
    // spells what the file hands the linker, not what the driver adds.
    #[test]
    fn a_drivers_words() {
        let line =
            words("-static -Wl,-z,now,--gc-sections -Xlinker -E -rdynamic -O2 -g -e main a.o");
        assert_eq!(
            linker_words(Driver::Gcc, &line),
            ["-static", "-z", "now", "--gc-sections", "-E", "-export-dynamic", "-e", "main"]
        );
        assert_eq!(linker_words(Driver::Ld, &line), line, "the linker's own words");
    }

    // spec/harness.md "Every accepted option is spelled" (WP-112): every
    // row of docs/status/options.md that mortar accepts is spelled by a
    // link test. A row this lists needs a test under tests/link that
    // shows its effect (or, for a noop, that it has none and why).
    #[test]
    fn every_accepted_option_is_spelled_by_a_link_test() {
        let (accepted, rows) = workspace_unspelled().unwrap();
        assert!(accepted > 100, "the table has {accepted} accepted rows");
        let list: Vec<String> = rows.iter().map(Row::describe).collect();
        assert!(
            rows.is_empty(),
            "{} of {accepted} accepted options are spelled by no link test:\n  {}",
            rows.len(),
            list.join("\n  ")
        );
    }
}
