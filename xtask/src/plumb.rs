//! `cargo xtask plumb`: the independent verifier on a finished link
//! (`spec/plumb.md` §8).
//!
//!   cargo xtask plumb <output> --map <file> --map-format gnu|lld|mold|mortar
//!                     [--inputs-from <response file>|map]
//!
//! The inputs are the files the map names, read from the paths it gives
//! (relative ones from the current directory). With a response file, only
//! the map's files that name the same file as one of its entries are read;
//! the others are missing inputs (rule I1).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use mortar_plumb::{Format, Input};

pub const USAGE: &str = "usage: cargo xtask plumb <output> --map <file> --map-format gnu|lld|mold|mortar \
                         [--inputs-from <response file>|map]\n";

/// The command line of `xtask plumb`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub output: PathBuf,
    pub map: PathBuf,
    pub format: Format,
    /// `None`: the map itself.
    pub inputs_from: Option<PathBuf>,
}

impl Args {
    /// Parses the arguments after `plumb`.
    pub fn parse(args: &[String]) -> Result<Args, String> {
        let mut output = None;
        let mut map = None;
        let mut format = None;
        let mut inputs_from = None;
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
        })
    }
}

/// The entries of a response file: words separated by white space, with
/// single and double quotes and backslash escapes, as GNU ld reads `@file`.
/// Options (words starting with `-`) are skipped, and so is the value of
/// `-o`.
pub fn response_entries(text: &str) -> Vec<String> {
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
    let mut out = Vec::new();
    let mut skip = false;
    for w in words {
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

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Runs the command; prints the findings and the coverage. `Ok(clean)`.
pub fn run(args: &Args) -> Result<bool, String> {
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let output = read(&args.output)?;
    let map_bytes = read(&args.map)?;
    let text = String::from_utf8_lossy(&map_bytes);
    let map = mortar_plumb::map::parse(args.format, &text)
        .map_err(|e| format!("{}:{}: {}", args.map.display(), e.line, e.message))?;
    let allowed: Option<BTreeSet<PathBuf>> = match &args.inputs_from {
        None => None,
        Some(rsp) => {
            let t = read(rsp)?;
            Some(
                response_entries(&String::from_utf8_lossy(&t))
                    .iter()
                    .map(|e| canonical(Path::new(e)))
                    .collect(),
            )
        }
    };
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for path in map.paths() {
        if allowed.as_ref().is_some_and(|a| !a.contains(&canonical(Path::new(path)))) {
            continue;
        }
        // An unreadable file is a missing input (I1) in the report.
        if let Ok(b) = std::fs::read(path) {
            files.push((path.to_string(), b));
        }
    }
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let report = mortar_plumb::verify(&output, &inputs, &map);
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
