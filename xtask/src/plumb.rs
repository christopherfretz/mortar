//! `cargo xtask plumb`: the independent verifier on a finished link
//! (`spec/plumb.md` §8).
//!
//!   cargo xtask plumb <output> --map <file> --map-format gnu|lld|mold|mortar
//!                     [--inputs-from <response file>|map]
//!                     [--fix-cortex-a53-843419|--no-fix-cortex-a53-843419]
//!                     [--mutate [--per-class N] [--seed S] [--threads T]]
//!
//! With `--mutate`, the link is not judged but mutated (§9.5): the table
//! of mutants applied, caught and missed per class, then every missed one.
//!
//! The inputs are the files the map names, read from the paths it gives
//! (relative ones from the current directory). With a response file, only
//! the map's files that name the same file as one of its entries are read;
//! the others are missing inputs (rule I1).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use mortar_plumb::{Format, Input};

pub const USAGE: &str = "usage: cargo xtask plumb <output> --map <file> --map-format gnu|lld|mold|mortar \
                         [--inputs-from <response file>|map] [--line <file>] \
                         [--fix-cortex-a53-843419|--no-fix-cortex-a53-843419] \
                         [--mutate [--per-class N] [--seed S] [--threads T]]\n";

/// The command line of `xtask plumb`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub output: PathBuf,
    pub map: PathBuf,
    pub format: Format,
    /// `None`: the map itself.
    pub inputs_from: Option<PathBuf>,
    /// Whether the link had `--fix-cortex-a53-843419` (spec/plumb.md
    /// §5.10); `None`: from the response file, else inferred.
    pub fix_843419: Option<bool>,
    /// `--mutate`: the mutation run's configuration.
    pub mutate: Option<mortar_plumb::mutate::Config>,
    /// `--line`: the linker's words, one per line (stage 3, a dynamic
    /// output); else the response file's words, else the map's `arg`s.
    pub line: Option<PathBuf>,
}

impl Args {
    /// Parses the arguments after `plumb`.
    pub fn parse(args: &[String]) -> Result<Args, String> {
        let mut output = None;
        let mut map = None;
        let mut format = None;
        let mut inputs_from = None;
        let mut fix_843419 = None;
        let mut line = None;
        let mut mutate: Option<mortar_plumb::mutate::Config> = None;
        let mut cfg = mortar_plumb::mutate::Config::default();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let mut value =
                |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
            match a.as_str() {
                "--map" => map = Some(PathBuf::from(value("--map")?)),
                "--map-format" => {
                    let v = value("--map-format")?;
                    format = Some(
                        Format::from_name(&v).ok_or_else(|| format!("unknown map format '{v}'"))?,
                    );
                }
                "--inputs-from" => {
                    let v = value("--inputs-from")?;
                    inputs_from = (v != "map").then(|| PathBuf::from(v));
                }
                "--mutate" => mutate = Some(cfg),
                "--line" => line = Some(PathBuf::from(value("--line")?)),
                "--no-fix-cortex-a53-843419" => fix_843419 = Some(false),
                s if s == "--fix-cortex-a53-843419"
                    || s.starts_with("--fix-cortex-a53-843419=") =>
                {
                    fix_843419 = Some(true);
                }
                o @ ("--per-class" | "--seed" | "--threads") => {
                    let v = value(o)?;
                    let n: u64 = v.parse().map_err(|_| format!("{o}: not a number '{v}'"))?;
                    match o {
                        "--per-class" => cfg.per_class = n as usize,
                        "--seed" => cfg.seed = n,
                        _ => cfg.threads = n as usize,
                    }
                }
                s if s.starts_with('-') => return Err(format!("unknown option '{s}'")),
                s if output.is_none() => output = Some(PathBuf::from(s)),
                s => return Err(format!("a second output '{s}'")),
            }
        }
        Ok(Args {
            output: output.ok_or("no output file")?,
            map: map.ok_or("no --map")?,
            format: format.ok_or("no --map-format")?,
            inputs_from,
            fix_843419,
            mutate: mutate.map(|_| cfg),
            line,
        })
    }
}

/// The entries of a response file: words separated by white space, with
/// single and double quotes and backslash escapes, as GNU ld reads `@file`.
/// Options (words starting with `-`) are skipped, and so is the value of
/// `-o`.
pub fn response_entries(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for w in response_words(text) {
        if skip {
            skip = false;
        } else if w == "-o" {
            skip = true;
        } else if !w.starts_with('-') {
            out.push(w);
        }
    }
    out
}

/// What a response file says of `--fix-cortex-a53-843419`: the last of it
/// (with or without a value) and `--no-fix-cortex-a53-843419` wins.
pub fn response_fix_843419(text: &str) -> Option<bool> {
    response_words(text).iter().rev().find_map(|w| {
        if w == "--no-fix-cortex-a53-843419" {
            Some(false)
        } else if w == "--fix-cortex-a53-843419" || w.starts_with("--fix-cortex-a53-843419=") {
            Some(true)
        } else {
            None
        }
    })
}

/// The words of a response file (see [`response_entries`]).
fn response_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (_, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            (_, c) => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    words
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Verifies the link that wrote `output` and `map` (in `format`). The
/// inputs are the files the map names, a relative path read from `base`;
/// with `allowed`, only those of its files. An unreadable input is a
/// missing input (I1) in the report.
///
/// # Errors
/// The output or the map cannot be read, or the map does not parse.
pub fn verify_link(
    output: &Path,
    map: &Path,
    format: Format,
    base: &Path,
    allowed: Option<&BTreeSet<PathBuf>>,
) -> Result<Verified, String> {
    verify_link_with(output, map, format, base, allowed, None)
}

/// [`verify_link`], with what the caller knows of the link's
/// `--fix-cortex-a53-843419` (spec/plumb.md §5.10).
///
/// # Errors
/// As [`verify_link`].
pub fn verify_link_with(
    output: &Path,
    map: &Path,
    format: Format,
    base: &Path,
    allowed: Option<&BTreeSet<PathBuf>>,
    fix_843419: Option<bool>,
) -> Result<Verified, String> {
    verify_link_line(output, map, format, base, allowed, fix_843419, None)
}

/// The files of a link, read from the directory it ran in.
struct BaseFiles<'a>(&'a Path);

impl mortar_plumb::Files for BaseFiles<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

/// [`verify_link_with`], with the linker's words for stage 3 (a dynamic
/// output, spec/plumb.md §12).
///
/// # Errors
/// As [`verify_link`].
pub fn verify_link_line(
    output: &Path,
    map: &Path,
    format: Format,
    base: &Path,
    allowed: Option<&BTreeSet<PathBuf>>,
    fix_843419: Option<bool>,
    line: Option<Vec<String>>,
) -> Result<Verified, String> {
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let output = read(output)?;
    let map_bytes = read(map)?;
    let text = String::from_utf8_lossy(&map_bytes);
    let mut parsed = mortar_plumb::map::parse(format, &text)
        .map_err(|e| format!("{}:{}: {}", map.display(), e.line, e.message))?;
    parsed.fix_843419 = fix_843419;
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for path in parsed.paths() {
        let full = base.join(path);
        if allowed.is_some_and(|a| !a.contains(&canonical(&full))) {
            continue;
        }
        if let Ok(b) = std::fs::read(&full) {
            // A thin archive's members are files of their own (§2.2).
            for member in mortar_plumb::thin_members(path, &b) {
                if let Ok(m) = std::fs::read(base.join(&member)) {
                    files.push((member, m));
                }
            }
            files.push((path.to_string(), b));
        }
    }
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let report = if mortar_plumb::is_dynamic(&output) {
        let files = BaseFiles(base);
        let ctx = mortar_plumb::Ctx { line, files: &files };
        mortar_plumb::verify_with(&output, &inputs, &parsed, &ctx)
    } else {
        mortar_plumb::verify(&output, &inputs, &parsed)
    };
    drop(inputs);
    Ok((output, report, files))
}

/// [`verify_link`]'s answer: the output's bytes, the report, and the
/// inputs read (path as the map names it, bytes).
pub type Verified = (Vec<u8>, mortar_plumb::Report, Vec<(String, Vec<u8>)>);

/// Runs the command; prints the findings and the coverage. `Ok(clean)`.
pub fn run(args: &Args) -> Result<bool, String> {
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let mut fix = args.fix_843419;
    let line: Option<Vec<String>> = match (&args.line, &args.inputs_from) {
        (Some(l), _) => Some(
            String::from_utf8_lossy(&read(l)?).lines().map(str::to_string).collect(),
        ),
        (None, Some(rsp)) => Some(response_words(&String::from_utf8_lossy(&read(rsp)?))),
        (None, None) => None,
    };
    let allowed: Option<BTreeSet<PathBuf>> = match &args.inputs_from {
        None => None,
        Some(rsp) => {
            let t = read(rsp)?;
            fix = fix.or_else(|| response_fix_843419(&String::from_utf8_lossy(&t)));
            Some(
                response_entries(&String::from_utf8_lossy(&t))
                    .iter()
                    .map(|e| canonical(Path::new(e)))
                    .collect(),
            )
        }
    };
    if let Some(cfg) = &args.mutate {
        let (output, _, files) = verify_link_with(
            &args.output,
            &args.map,
            args.format,
            Path::new(""),
            allowed.as_ref(),
            fix,
        )?;
        let map_bytes = read(&args.map)?;
        let mut parsed =
            mortar_plumb::map::parse(args.format, &String::from_utf8_lossy(&map_bytes))
                .map_err(|e| format!("{}:{}: {}", args.map.display(), e.line, e.message))?;
        parsed.fix_843419 = fix;
        let inputs: Vec<Input<'_>> =
            files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        let outcome = mortar_plumb::mutate::run(&output, &inputs, &parsed, cfg)?;
        println!("{}: baseline findings {}", args.output.display(), outcome.baseline);
        print!("{}", outcome.render());
        return Ok(true);
    }
    let (_, report, _) = verify_link_line(
        &args.output,
        &args.map,
        args.format,
        Path::new(""),
        allowed.as_ref(),
        fix,
        line,
    )?;
    print!("{}", report.render(&args.output.display().to_string()));
    print!("{}", report.summary());
    Ok(report.clean())
}

/// The command's entry point: exit 0 when clean, 1 on any finding, 2 on
/// a usage or read error.
pub fn main(args: &[String]) -> std::process::ExitCode {
    use std::process::ExitCode;
    match Args::parse(args).and_then(|a| run(&a)) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprint!("xtask plumb: {message}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_string()).collect()
    }

    // spec/plumb.md §8.
    #[test]
    fn s8_arguments() {
        let a = Args::parse(&s(&["a.out", "--map", "m", "--map-format", "lld"])).unwrap();
        assert_eq!(a.format, Format::Lld);
        assert_eq!(a.inputs_from, None);
        let a = Args::parse(&s(&[
            "a.out",
            "--map",
            "m",
            "--map-format",
            "gnu",
            "--inputs-from",
            "r.rsp",
        ]))
        .unwrap();
        assert_eq!(a.inputs_from, Some(PathBuf::from("r.rsp")));
        assert!(Args::parse(&s(&["a.out", "--map", "m"])).is_err());
        let a = Args::parse(&s(&[
            "a.out",
            "--map",
            "m",
            "--map-format",
            "mortar",
            "--mutate",
            "--seed",
            "7",
        ]))
        .unwrap();
        assert_eq!(a.mutate.map(|c| c.seed), Some(7));
        assert_eq!(a.fix_843419, None);
        let a = Args::parse(&s(&[
            "a.out",
            "--map",
            "m",
            "--map-format",
            "mortar",
            "--fix-cortex-a53-843419",
        ]))
        .unwrap();
        assert_eq!(a.fix_843419, Some(true));
        // spec/plumb.md §5.10: the last of the two options wins.
        assert_eq!(response_fix_843419("-X --fix-cortex-a53-843419 -pie a.o"), Some(true));
        assert_eq!(
            response_fix_843419("--fix-cortex-a53-843419=full --no-fix-cortex-a53-843419"),
            Some(false)
        );
        assert_eq!(response_fix_843419("a.o -o out"), None);
        assert!(Args::parse(&s(&["a.out", "--map", "m", "--map-format", "x"])).is_err());
        assert!(Args::parse(&s(&["a.out", "b.out", "--map", "m", "--map-format", "gnu"])).is_err());
    }

    // spec/plumb.md §8.
    #[test]
    fn s8_response_files() {
        assert_eq!(
            response_entries("-o out 'a b.o' \"c.o\" -static d\\ e.o\n--gc-sections lib.a"),
            ["a b.o", "c.o", "d e.o", "lib.a"]
        );
    }
}
