//! `cargo xtask plumb`: the independent verifier on a finished link
//! (`spec/plumb.md` §8).
//!
//!   cargo xtask plumb <output> --map <file> --map-format gnu|lld|mold|mortar
//!                     [--inputs-from <response file>|map]
//!                     [--fix-cortex-a53-843419|--no-fix-cortex-a53-843419]
//!                     [--mutate [--per-class N] [--seed S] [--threads T]]
//!                     [--sweep [--window W] [--threads T]]
//!
//! With `--mutate`, the link is not judged but mutated (§9.5): the table
//! of mutants applied, caught and missed per class, then every missed one.
//!
//! The inputs are the files the map names, read from the paths it gives
//! (relative ones from the current directory). With a response file, only
//! the map's files that name the same file as one of its entries, or as
//! one of its `-l` resolved through its `-L` directories and sysroot, are
//! read; the others are missing inputs (rule I1).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use mortar_plumb::{Format, Input};

pub const USAGE: &str = "usage: cargo xtask plumb <output> --map <file> --map-format gnu|lld|mold|mortar \
                         [--inputs-from <response file>|map] [--line <file>] \
                         [--fix-cortex-a53-843419|--no-fix-cortex-a53-843419] \
                         [--mutate [--per-class N] [--seed S] [--threads T]] [--sweep [--window W]] [--coverage]\n";

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
    /// `--coverage`: print IN-60's coverage of every allocated section
    /// and what a map claiming it would be told (spec/plumb.md §6.8).
    pub coverage: bool,
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
        let mut coverage = false;
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
                "--coverage" => coverage = true,
                // The byte sweep of a dynamic output's synthetic sections
                // (mortar_plumb::sweep, WP-082).
                "--sweep" => {
                    cfg.sweep = true;
                    mutate = Some(cfg);
                }
                "--line" => line = Some(PathBuf::from(value("--line")?)),
                "--no-fix-cortex-a53-843419" => fix_843419 = Some(false),
                s if s == "--fix-cortex-a53-843419"
                    || s.starts_with("--fix-cortex-a53-843419=") =>
                {
                    fix_843419 = Some(true);
                }
                o @ ("--per-class" | "--seed" | "--threads" | "--window") => {
                    let v = value(o)?;
                    let n: u64 = v.parse().map_err(|_| format!("{o}: not a number '{v}'"))?;
                    match o {
                        "--per-class" => cfg.per_class = n as usize,
                        "--seed" => cfg.seed = n,
                        "--window" => cfg.window = n,
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
            coverage,
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

/// Options whose value is the next word (GNU ld's spelling and the long
/// forms the drivers write): the value is not an input.
const SEPARATE_VALUE: [&str; 24] = [
    "-o",
    "--output",
    "-m",
    "-e",
    "--entry",
    "-T",
    "--script",
    "-z",
    "-h",
    "-soname",
    "-u",
    "--undefined",
    "-y",
    "-plugin",
    "-plugin-opt",
    "-dynamic-linker",
    "-I",
    "-rpath",
    "-rpath-link",
    "-Map",
    "--version-script",
    "--dynamic-list",
    "--sysroot",
    "--library-path",
];

/// The files a link with this response file reads, as paths (relative
/// ones from `cwd`): its file entries, and each `-l` resolved as the
/// linker resolves it (spec/input.md §4.2-4.4): through the `-L` and
/// `--library-path` directories of the whole line in order, `=D` and
/// `$SYSROOT D` under the last `--sysroot`, `libname.so` then `libname.a`
/// per directory (`.a` only where `-static`/`-Bstatic` is in effect),
/// `-l:file` as `D/file`. An `-l` that names no existing file is left
/// out (the verifier then reports the inputs of the map it misses).
pub fn response_inputs(text: &str, cwd: &Path) -> Vec<PathBuf> {
    let words = response_words(text);
    let mut dirs = Vec::new();
    let mut sysroot: Option<String> = None;
    let mut at = 0;
    while at < words.len() {
        let w = words[at].as_str();
        let mut value = |prefix: &str, long: bool| -> Option<String> {
            match w.strip_prefix(prefix) {
                Some("") => {
                    at += 1;
                    words.get(at).cloned()
                }
                Some(v) if !long => Some(v.to_owned()),
                Some(v) => v.strip_prefix('=').map(str::to_owned),
                None => None,
            }
        };
        if let Some(d) = value("-L", false).or_else(|| value("--library-path", true)) {
            dirs.push(d);
        } else if let Some(s) = value("--sysroot", true) {
            sysroot = Some(s).filter(|s| !s.is_empty());
        }
        at += 1;
    }
    let dirs: Vec<PathBuf> = dirs
        .iter()
        .map(|d| {
            let in_sysroot = d.strip_prefix('=').or_else(|| d.strip_prefix("$SYSROOT"));
            let dir = match (in_sysroot, &sysroot) {
                (Some(d), Some(s)) => format!("{}{d}", s.trim_end_matches('/')),
                (Some(d), None) => d.to_owned(),
                (None, _) => d.clone(),
            };
            cwd.join(dir)
        })
        .collect();
    let find = |name: &str, link_static: bool| -> Option<PathBuf> {
        let candidates: Vec<String> = match name.strip_prefix(':') {
            Some(file) => vec![file.to_owned()],
            None if link_static => vec![format!("lib{name}.a")],
            None => vec![format!("lib{name}.so"), format!("lib{name}.a")],
        };
        dirs.iter().find_map(|d| candidates.iter().map(|c| d.join(c)).find(|p| p.is_file()))
    };
    let mut out = Vec::new();
    let mut link_static = false;
    let mut states = Vec::new();
    let mut at = 0;
    while at < words.len() {
        let w = words[at].as_str();
        at += 1;
        let library = if w == "-l" || w == "--library" {
            at += 1;
            words.get(at - 1).cloned()
        } else {
            w.strip_prefix("--library=").or_else(|| w.strip_prefix("-l")).map(str::to_owned)
        };
        if let Some(name) = library {
            out.extend(find(&name, link_static));
            continue;
        }
        match w {
            "-static" | "-Bstatic" | "-dn" | "-non_shared" => link_static = true,
            "-Bdynamic" | "-dy" | "-call_shared" => link_static = false,
            "--push-state" => states.push(link_static),
            "--pop-state" => link_static = states.pop().unwrap_or(link_static),
            w if SEPARATE_VALUE.contains(&w) || w == "-L" => at += 1,
            w if !w.starts_with('-') => out.push(cwd.join(w)),
            _ => {}
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
        (Some(l), _) => {
            Some(String::from_utf8_lossy(&read(l)?).lines().map(str::to_string).collect())
        }
        (None, Some(rsp)) => Some(response_words(&String::from_utf8_lossy(&read(rsp)?))),
        (None, None) => None,
    };
    let allowed: Option<BTreeSet<PathBuf>> = match &args.inputs_from {
        None => None,
        Some(rsp) => {
            let t = read(rsp)?;
            fix = fix.or_else(|| response_fix_843419(&String::from_utf8_lossy(&t)));
            Some(
                response_inputs(&String::from_utf8_lossy(&t), Path::new(""))
                    .iter()
                    .map(|e| canonical(e))
                    .collect(),
            )
        }
    };
    if args.coverage {
        let (output, _, files) = verify_link_with(
            &args.output,
            &args.map,
            args.format,
            Path::new(""),
            allowed.as_ref(),
            fix,
        )?;
        let map_bytes = read(&args.map)?;
        let parsed = mortar_plumb::map::parse(args.format, &String::from_utf8_lossy(&map_bytes))
            .map_err(|e| format!("{}:{}: {}", args.map.display(), e.line, e.message))?;
        let inputs: Vec<Input<'_>> =
            files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        let (cover, findings) = mortar_plumb::dead::coverage_table(&output, &inputs, &parsed)?;
        print!("{}", cover.render());
        for f in &findings {
            println!("finding {}: {}", f.rule.id(), f.message);
        }
        return Ok(findings.is_empty());
    }
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
        // Stage 3 judges the mutants of a dynamic output (spec/plumb.md §12).
        let files = BaseFiles(Path::new(""));
        let ctx = mortar_plumb::Ctx { line: line.clone(), files: &files };
        if cfg.sweep {
            let sweep =
                mortar_plumb::sweep::run(&output, &inputs, &parsed, cfg.threads, cfg.window, &ctx)?;
            println!("{}: baseline findings {}", args.output.display(), sweep.baseline);
            print!("{}", sweep.render());
            return Ok(true);
        }
        let outcome = mortar_plumb::mutate::run_with(&output, &inputs, &parsed, cfg, Some(&ctx))?;
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
    if args.first().is_some_and(|a| a == "--equiv") {
        return match EquivArgs::parse(args).and_then(|a| run_equiv(&a)) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(message) => {
                eprint!("xtask plumb --equiv: {message}\n{EQUIV_USAGE}");
                ExitCode::from(2)
            }
        };
    }
    match Args::parse(args).and_then(|a| run(&a)) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprint!("xtask plumb: {message}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

pub const EQUIV_USAGE: &str = "usage: cargo xtask plumb --equiv <X.map> <F.map> <X> <F>\n";

/// The command line of `xtask plumb --equiv` (§8; `spec/incr.md` IN-62).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EquivArgs {
    pub x_map: PathBuf,
    pub f_map: PathBuf,
    pub x: PathBuf,
    pub f: PathBuf,
}

impl EquivArgs {
    pub fn parse(args: &[String]) -> Result<EquivArgs, String> {
        let rest: Vec<&String> = args.iter().skip(1).collect();
        if let Some(o) = rest.iter().find(|a| a.starts_with('-')) {
            return Err(format!("unknown option '{o}'"));
        }
        match rest.as_slice() {
            [xm, fm, x, f] => Ok(EquivArgs {
                x_map: PathBuf::from(xm),
                f_map: PathBuf::from(fm),
                x: PathBuf::from(x),
                f: PathBuf::from(f),
            }),
            _ => Err(format!("--equiv takes four files, {} given", rest.len())),
        }
    }
}

/// A mortar map, read and parsed; a usage error otherwise (§8).
fn read_mortar_map(p: &Path) -> Result<mortar_plumb::Map, String> {
    let b = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let text = std::str::from_utf8(&b).map_err(|_| format!("{}: not a mortar map (not text)", p.display()))?;
    mortar_plumb::map::parse(Format::Mortar, text)
        .map_err(|e| format!("{}:{}: not a mortar map: {}", p.display(), e.line, e.message))
}

/// Runs `--equiv`: prints each side's findings, the P-E findings and the
/// summaries. `Ok(clean)`; `Err` is a usage error (exit 2).
pub fn run_equiv(a: &EquivArgs) -> Result<bool, String> {
    let xm = read_mortar_map(&a.x_map)?;
    let fm = read_mortar_map(&a.f_map)?;
    if !xm.claims_coverage() {
        return Err(format!(
            "{}: X's map claims no coverage (no `incremental` and no hole or dead line): not an update's map",
            a.x_map.display()
        ));
    }
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let (x, f) = (read(&a.x)?, read(&a.f)?);
    for (p, b) in [(&a.x, &x), (&a.f, &f)] {
        if !b.starts_with(b"\x7fELF") {
            return Err(format!("{}: not an ELF file (are the arguments in the order X.map F.map X F?)", p.display()));
        }
    }
    let mut paths: BTreeSet<String> = BTreeSet::new();
    paths.extend(xm.paths().into_iter().map(str::to_string));
    paths.extend(fm.paths().into_iter().map(str::to_string));
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for path in &paths {
        if let Ok(b) = std::fs::read(path) {
            for member in mortar_plumb::thin_members(path, &b) {
                if let Ok(m) = std::fs::read(&member) {
                    files.push((member, m));
                }
            }
            files.push((path.clone(), b));
        }
    }
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let eq = mortar_plumb::equiv::check(&x, &xm, &f, &fm, &inputs);
    let (xn, fname) = (a.x.display().to_string(), a.f.display().to_string());
    for line in eq.x.render(&xn).lines() {
        println!("X: {line}");
    }
    for line in eq.f.render(&fname).lines() {
        println!("F: {line}");
    }
    let pe = mortar_plumb::Report { findings: eq.findings.clone(), coverage: Default::default() };
    print!("{}", pe.render(&xn));
    println!("X: {} findings, {} records", eq.x.findings.len(), eq.x.coverage.records);
    println!("F: {} findings, {} records", eq.f.findings.len(), eq.f.coverage.records);
    println!("{}", eq.summary());
    Ok(eq.clean())
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec/plumb.md §8: `--equiv`.
    #[test]
    fn s8_equiv_arguments() {
        let a = EquivArgs::parse(&s(&["--equiv", "x.map", "f.map", "x", "f"])).unwrap();
        assert_eq!(a.x_map, PathBuf::from("x.map"));
        assert_eq!(a.f, PathBuf::from("f"));
        assert!(EquivArgs::parse(&s(&["--equiv", "x.map", "f.map", "x"])).is_err());
        assert!(EquivArgs::parse(&s(&["--equiv", "x.map", "f.map", "x", "f", "g"])).is_err());
        assert!(EquivArgs::parse(&s(&["--equiv", "--map", "f.map", "x", "f"])).is_err());
        let dir = std::env::temp_dir().join(format!("mortar-wp092-args-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plain = dir.join("plain.map");
        std::fs::write(&plain, "mortar-map 1\nend\n").unwrap();
        let elf = dir.join("x");
        std::fs::write(&elf, b"\x7fELF\x02\x01").unwrap();
        let arg = |p: &Path| p.display().to_string();
        // A plain map as X, a missing file, swapped arguments: exit 2.
        let r = run_equiv(&EquivArgs::parse(&s(&["--equiv", &arg(&plain), &arg(&plain), &arg(&elf), &arg(&elf)])).unwrap());
        assert!(r.is_err(), "{r:?}");
        let r = run_equiv(&EquivArgs::parse(&s(&["--equiv", &arg(&elf), &arg(&plain), &arg(&plain), &arg(&elf)])).unwrap());
        assert!(r.is_err(), "{r:?}");
        let r = run_equiv(&EquivArgs::parse(&s(&["--equiv", "/nonexistent/x.map", &arg(&plain), &arg(&elf), &arg(&elf)])).unwrap());
        assert!(r.is_err(), "{r:?}");
        std::fs::remove_file(&plain).unwrap();
        std::fs::remove_file(&elf).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

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

    // spec/plumb.md §8, spec/input.md §4.2-4.4: `-l` is resolved through
    // the `-L` directories of the whole line and the sysroot, shared
    // before static per directory unless `-static`/`-Bstatic`.
    #[test]
    fn s8_libraries_are_resolved() {
        let scratch = mortar_testutil::Scratch::new("plumb-inputs");
        let root = scratch.path();
        for f in ["d1/libf.a", "d2/libf.so", "d2/libf.a", "d2/libg.a", "s/lib/libq.a", "d1/crt.o"] {
            std::fs::create_dir_all(root.join(f).parent().unwrap()).unwrap();
            std::fs::write(root.join(f), b"x").unwrap();
        }
        let got = |text: &str| -> Vec<String> {
            response_inputs(text, root)
                .iter()
                .map(|p| p.strip_prefix(root).unwrap().display().to_string())
                .collect()
        };
        // A `-L` after the `-l` applies; the first directory that has a
        // candidate wins; `.so` before `.a` in one directory.
        assert_eq!(got("-lf -Ld1 -L d2 a.o"), ["d1/libf.a", "a.o"]);
        assert_eq!(got("-L d2 -Ld1 -lf -l g"), ["d2/libf.so", "d2/libg.a"]);
        // `-static`/`-Bstatic` take only archives; `-Bdynamic` and the
        // state stack give the shared object back.
        assert_eq!(got("-static -Ld2 -lf"), ["d2/libf.a"]);
        assert_eq!(
            got("-Ld2 -Bstatic --library=f -Bdynamic --library f"),
            ["d2/libf.a", "d2/libf.so"]
        );
        assert_eq!(
            got("-Ld2 --push-state -Bstatic -lf --pop-state -lf"),
            ["d2/libf.a", "d2/libf.so"]
        );
        // `-l:file`; a library found nowhere is left out.
        assert_eq!(
            got("-L d1 -l:crt.o -lnone --start-group -lf --end-group"),
            ["d1/crt.o", "d1/libf.a"]
        );
        // The sysroot: `=D` and `$SYSROOT D` under the last `--sysroot`; an
        // unmarked directory is not prefixed.
        assert_eq!(got("--sysroot=/nowhere --sysroot s/ -L=/lib -lq"), ["s/lib/libq.a"]);
        assert_eq!(got("--sysroot=s -L$SYSROOT/lib -lq"), ["s/lib/libq.a"]);
        assert!(got("--sysroot=s -L/lib -lq").is_empty());
        // `--library-path`, and option values that are not inputs.
        assert_eq!(
            got("--library-path=d1 -plugin p.so -m aarch64linux -o out -lf -dynamic-linker ld.so"),
            ["d1/libf.a"]
        );
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
