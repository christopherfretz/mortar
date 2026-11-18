//! `cargo xtask plumb`: the independent verifier on a finished link
//! (`spec/plumb.md` §8).
//!
//!   cargo xtask plumb <output> --map <file> --map-format gnu|lld|mold|mortar
//!                     [--inputs-from <response file>|map]
//!                     [--fix-cortex-a53-843419|--no-fix-cortex-a53-843419]
//!                     [--mutate [--per-class N] [--seed S] [--threads T]]
//!                     [--sweep [--window W] [--threads T] [--no-reseal]]
//!
//!   cargo xtask plumb --compose <D> <D.map> <V> <V.map> <R> <R.map>
//!                     --map-format F [--r-format F] [--line-d|-v|-r <file>]
//!
//! A `-r` output (`ET_REL`) is judged by stage R (spec/plumb.md §14);
//! `--compose` runs P-P1 on a direct link `D`, the link `V` through the
//! `-r` output `R`, and `R`'s own map (§14.5).
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
                         [--mutate [--per-class N] [--seed S] [--threads T]] [--sweep [--window W] [--no-reseal]] [--coverage]\n\
                         (an ET_REL output is judged by stage R, spec/plumb.md §14; P-P1: --compose)\n";

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
    /// `--no-reseal`: the sweep leaves mortar's build id as it was after
    /// a flip (to measure what B1 alone catches; spec/plumb.md §12.3).
    pub no_reseal: bool,
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
        let mut no_reseal = false;
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
                "--no-reseal" => no_reseal = true,
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
            no_reseal,
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

/// The files a link with this response file reads, as paths (relative
/// ones from `cwd`): its file entries, and each `-l` resolved as the
/// linker resolves it (spec/input.md §4.2-4.4): through the `-L` and
/// `--library-path` directories of the whole line in order, `=D` and
/// `$SYSROOT D` under the last `--sysroot`, `libname.so` then `libname.a`
/// per directory (`.a` only where `-static`/`-Bstatic` is in effect),
/// `-l:file` as `D/file`. An `-l` that names no existing file is left
/// out (the verifier then reports the inputs of the map it misses).
/// A file that is a linker script (neither an object nor an archive:
/// glibc's `libm.a` is `GROUP ( libm-2.41.a libmvec.a )`) stands for the
/// files it names, found as the linker finds them ([`script_inputs`]).
///
/// # Errors
/// A linker script with a command [`script_inputs`] does not read.
pub fn response_inputs(text: &str, cwd: &Path) -> Result<Vec<PathBuf>, String> {
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
            if let Some(f) = find(&name, link_static) {
                out.extend(script_inputs(&f, cwd, &dirs, 0)?);
            }
            continue;
        }
        match w {
            "-static" | "-Bstatic" | "-dn" | "-non_shared" => link_static = true,
            "-Bdynamic" | "-dy" | "-call_shared" => link_static = false,
            "--push-state" => states.push(link_static),
            "--pop-state" => link_static = states.pop().unwrap_or(link_static),
            // An option and its value: whether it takes the next word
            // is its row's (mortar_plumb's one reader of the options
            // table, spec/plumb.md §3.2a; WP-131).
            w if w.starts_with('-') && w != "-" => {
                at += mortar_plumb::opts::Table::get().word(&words, at - 1).1 - 1;
            }
            w if !w.starts_with('-') => out.extend(script_inputs(&cwd.join(w), cwd, &dirs, 0)?),
            _ => {}
        }
    }
    Ok(out)
}

/// The files the link reads for `file`: `file` itself when it is an
/// object, an archive, or cannot be read; for a linker script, the files
/// of its `INPUT` and `GROUP` commands (`AS_NEEDED` lists among them),
/// each a plain file name found as GNU ld finds it ("File Commands" of
/// its manual): an absolute name as it is, any other in `cwd`, then in
/// the library directories in order. `OUTPUT_FORMAT` names no file and
/// is passed over. A name found nowhere adds nothing.
///
/// # Errors
/// Any other command, a `-l` name in a script, or scripts nested more
/// than 8 deep: refused by name, not guessed at.
pub fn script_inputs(
    file: &Path,
    cwd: &Path,
    dirs: &[PathBuf],
    depth: usize,
) -> Result<Vec<PathBuf>, String> {
    let Ok(bytes) = std::fs::read(file) else { return Ok(vec![file.to_path_buf()]) };
    if [&b"\x7fELF"[..], b"!<arch>\n", b"!<thin>\n"].iter().any(|m| bytes.starts_with(m)) {
        return Ok(vec![file.to_path_buf()]);
    }
    let what = |m: String| {
        format!(
            "{}: a linker script {m} (--inputs-from reads INPUT, GROUP and AS_NEEDED of file names)",
            file.display()
        )
    };
    if depth >= 8 {
        return Err(what("nested more than 8 deep".into()));
    }
    let text = String::from_utf8_lossy(&bytes);
    // Comments out; parentheses and commas are separators of their own.
    let mut plain = String::new();
    let mut rest = text.as_ref();
    while let Some(i) = rest.find("/*") {
        plain.push_str(&rest[..i]);
        plain.push(' ');
        rest = rest[i + 2..].split_once("*/").map_or("", |(_, after)| after);
    }
    plain.push_str(rest);
    let spaced = plain.replace('(', " ( ").replace(')', " ) ").replace(',', " ");
    let mut words = spaced.split_whitespace().peekable();
    let mut out = Vec::new();
    // The open lists: each is `true` when its words are file names.
    let mut open: Vec<bool> = Vec::new();
    while let Some(w) = words.next() {
        match w {
            "(" => return Err(what("with a '(' that opens no command".into())),
            ")" => {
                if open.pop().is_none() {
                    return Err(what("with a ')' that closes nothing".into()));
                }
            }
            "INPUT" | "GROUP" | "AS_NEEDED" | "OUTPUT_FORMAT" if words.peek() == Some(&"(") => {
                words.next();
                if w == "AS_NEEDED" && open.last() != Some(&true) {
                    return Err(what("with AS_NEEDED outside INPUT or GROUP".into()));
                }
                open.push(w != "OUTPUT_FORMAT");
            }
            _ if open.last() == Some(&false) => {}
            name if open.last() == Some(&true) => {
                if name.starts_with("-l") {
                    return Err(what(format!("with the library name '{name}'")));
                }
                let p = Path::new(name);
                let found = if p.is_absolute() {
                    Some(p.to_path_buf())
                } else {
                    std::iter::once(cwd.to_path_buf())
                        .chain(dirs.iter().cloned())
                        .map(|d| d.join(p))
                        .find(|c| c.is_file())
                };
                if let Some(f) = found {
                    out.extend(script_inputs(&f, cwd, dirs, depth + 1)?);
                }
            }
            other => return Err(what(format!("command '{other}'"))),
        }
    }
    if !open.is_empty() {
        return Err(what("with a command that is not closed".into()));
    }
    Ok(out)
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

/// Whether a linker's words (`line`, the program name not among them)
/// give `--fix-cortex-a53-843419` (spec/a64.md §11, WP-103): the last of
/// it (with or without a value) and `--no-fix-cortex-a53-843419` wins, as
/// the linker reads them. A response file `@<file>` (read from `base` when
/// relative) is its words in place; a driver's `-Wl,<a>,<b>` and
/// `-Xlinker <a>` are the words they pass. Neither word: `false`, every
/// linker's default. Every lane hands the verifier the words the linker
/// received (the `ld` wrappers record them after the driver ran), so a
/// driver's own default (GCC on Debian passes the option to every link,
/// Clang does not) is in them.
pub fn line_fix_843419(line: &[String], base: &Path) -> bool {
    fn scan(words: &[String], base: &Path, depth: u32, fix: &mut bool) {
        let mut it = words.iter();
        while let Some(w) = it.next() {
            let one = |w: &str, fix: &mut bool| {
                if w == "--no-fix-cortex-a53-843419" {
                    *fix = false;
                } else if w == "--fix-cortex-a53-843419"
                    || w.starts_with("--fix-cortex-a53-843419=")
                {
                    *fix = true;
                }
            };
            if let Some(file) = w.strip_prefix('@')
                && depth < 8
                && let Ok(text) = std::fs::read(base.join(file))
            {
                scan(&response_words(&String::from_utf8_lossy(&text)), base, depth + 1, fix);
            } else if let Some(list) = w.strip_prefix("-Wl,") {
                list.split(',').for_each(|a| one(a, fix));
            } else if w == "-Xlinker" {
                if let Some(a) = it.next() {
                    one(a, fix);
                }
            } else {
                one(w, fix);
            }
        }
    }
    let mut fix = false;
    scan(line, base, 0, &mut fix);
    fix
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
    // By position (the input set reads archives' members, spec/plumb.md
    // §3.8, §8.1).
    fn len(&self, path: &str) -> Option<u64> {
        std::fs::metadata(self.0.join(path))
            .ok()
            .filter(std::fs::Metadata::is_file)
            .map(|m| m.len())
    }
    fn head(&self, path: &str, n: usize) -> Option<Vec<u8>> {
        mortar_plumb::line::host_head(&self.0.join(path), n)
    }
    fn read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        mortar_plumb::line::host_read_at(&self.0.join(path), offset, buf)
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
    mortar_plumb::prof::mark("start");
    let output = read(output)?;
    let map_bytes = read(map)?;
    mortar_plumb::prof::mark("read output and map");
    let text = String::from_utf8_lossy(&map_bytes);
    let mut parsed = mortar_plumb::map::parse(format, &text)
        .map_err(|e| format!("{}:{}: {}", map.display(), e.line, e.message))?;
    // A reference's map: the caller's word, else the link's own words
    // (WP-103: every lane passes the linker's line; with neither, the
    // verifier infers it). Mortar's map states its line (`arg`,
    // spec/plumb.md §3.2a), which is all the verifier reads.
    if format != Format::Mortar {
        parsed.fix_843419 =
            fix_843419.or_else(|| line.as_deref().map(|l| line_fix_843419(l, base)));
    }
    mortar_plumb::prof::mark("parse map");
    let files = link_files(&parsed, base, allowed)?;
    mortar_plumb::prof::mark("open inputs");
    // A static output is judged without stage 3 (verify_from's dispatch).
    let dir = BaseFiles(base);
    let ctx = mortar_plumb::Ctx { line, files: &dir };
    let report = mortar_plumb::verify_from(&output, &files, &parsed, &ctx);
    Ok((output, report, files))
}

/// The files `map` names, a relative path read from `base`; with
/// `allowed`, only those of its files. A thin archive's members are files
/// of their own (spec/plumb.md §2.2); an unreadable file is left out.
///
/// # Errors
/// The map names files and not one of them can be read: the verifier is
/// run from the wrong directory (a mortar map records none, §3.2), and
/// every input would be I1 (spec/plumb.md §8).
fn link_files(
    map: &mortar_plumb::map::Map,
    base: &Path,
    allowed: Option<&BTreeSet<PathBuf>>,
) -> Result<FileSource, String> {
    let paths = map.paths();
    let files = FileSource::open(paths.iter().copied(), base, |full| {
        allowed.is_none_or(|a| a.contains(&canonical(full)))
    });
    unresolved(&paths, files.files.len(), base, allowed.is_some())?;
    Ok(files)
}

/// The input files of a link, open, read by position (spec/plumb.md §8.1,
/// WP-109 §6.3): the verifier's [`mortar_plumb::Source`]. A file is
/// named by the path the map gives; its length is taken when it is opened.
#[derive(Debug, Default)]
pub struct FileSource {
    files: Vec<(String, std::fs::File, u64)>,
}

impl FileSource {
    /// The files of `paths`, opened from `base` (those `keep` keeps), each
    /// thin archive's members before it (spec/plumb.md §2.2). A file that
    /// cannot be opened, or is not a regular file, is left out.
    fn open<'p>(
        paths: impl Iterator<Item = &'p str>,
        base: &Path,
        keep: impl Fn(&Path) -> bool,
    ) -> FileSource {
        let mut out = FileSource::default();
        for path in paths {
            let full = base.join(path);
            if !keep(&full) {
                continue;
            }
            let Some((file, len)) = open_file(&full) else { continue };
            // A thin archive holds its members' paths, not their bytes: it
            // is small, and read whole for them. A regular archive is not.
            let mut magic = [0u8; 8];
            if len >= 8
                && std::os::unix::fs::FileExt::read_exact_at(&file, &mut magic, 0).is_ok()
                && &magic == b"!<thin>\n"
                && let Ok(b) = std::fs::read(&full)
            {
                for member in mortar_plumb::thin_members(path, &b) {
                    if let Some((m, n)) = open_file(&base.join(&member)) {
                        out.files.push((member, m, n));
                    }
                }
            }
            out.files.push((path.to_string(), file, len));
        }
        out
    }

    /// Every file read whole (path as the map names it, bytes); a file
    /// that can no longer be read is left out.
    pub fn read_all(&self) -> Vec<(String, Vec<u8>)> {
        self.files
            .iter()
            .filter_map(|(p, f, n)| {
                let mut b = vec![0u8; usize::try_from(*n).ok()?];
                std::os::unix::fs::FileExt::read_exact_at(f, &mut b, 0).ok()?;
                Some((p.clone(), b))
            })
            .collect()
    }
}

/// A regular file, open, and its length.
fn open_file(path: &Path) -> Option<(std::fs::File, u64)> {
    let file = std::fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    let len = meta.len();
    meta.is_file().then_some((file, len))
}

impl mortar_plumb::Source for FileSource {
    fn count(&self) -> usize {
        self.files.len()
    }
    fn path(&self, i: usize) -> &str {
        self.files.get(i).map_or("", |f| f.0.as_str())
    }
    fn len(&self, i: usize) -> u64 {
        self.files.get(i).map_or(0, |f| f.2)
    }
    fn read_at(&self, i: usize, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        let Some((path, file, len)) = self.files.get(i) else {
            return Err(format!("no input {i}"));
        };
        if offset.checked_add(buf.len() as u64).is_none_or(|end| end > *len) {
            return Err(format!(
                "{path}: {} bytes at offset {offset:#x} are past its end ({len} bytes when opened)",
                buf.len()
            ));
        }
        std::os::unix::fs::FileExt::read_exact_at(file, buf, offset).map_err(|e| {
            format!("{path}: {} bytes at offset {offset:#x} cannot be read: {e}", buf.len())
        })
    }
}

/// The usage error of a map none of whose files was read (spec/plumb.md
/// §8): it names the first and the directory it was looked for in
/// (`filtered`: `--inputs-from` chose among them).
fn unresolved(paths: &[&str], read: usize, base: &Path, filtered: bool) -> Result<(), String> {
    match paths.first() {
        Some(first) if read == 0 => {
            let dir = if base.as_os_str().is_empty() {
                std::env::current_dir().map_or_else(|_| ".".into(), |d| d.display().to_string())
            } else {
                base.display().to_string()
            };
            Err(format!(
                "not one of the {} files the map names can be read from {dir}{} (the first: {first}); \
                 run the verifier from the directory the link was run in",
                paths.len(),
                if filtered { " among those --inputs-from names" } else { "" }
            ))
        }
        _ => Ok(()),
    }
}

/// The byte sweep (`mortar_plumb::sweep`, spec/plumb.md §12.3) of the
/// dynamic `output` a link in `base` wrote with `map` (in `format`); the
/// map's `arg` records are its line (a mortar map). WP-088's lane
/// (`xtask sweep`).
///
/// # Errors
/// The output or the map cannot be read, the map does not parse, or the
/// output is not dynamic.
pub fn sweep_link(
    output: &Path,
    map: &Path,
    format: Format,
    base: &Path,
    threads: usize,
    window: u64,
    sections: &[&str],
) -> Result<mortar_plumb::sweep::Sweep, String> {
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let bytes = read(output)?;
    let map_bytes = read(map)?;
    let parsed = mortar_plumb::map::parse(format, &String::from_utf8_lossy(&map_bytes))
        .map_err(|e| format!("{}:{}: {}", map.display(), e.line, e.message))?;
    let files = link_files(&parsed, base, None)?.read_all();
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let dir = BaseFiles(base);
    let ctx = mortar_plumb::Ctx { line: None, files: &dir };
    mortar_plumb::sweep::run_sections(&bytes, &inputs, &parsed, threads, window, &ctx, sections)
}

/// [`verify_link`]'s answer: the output's bytes, the report, and the
/// inputs (open, as the map names them; [`FileSource::read_all`] reads
/// them whole).
pub type Verified = (Vec<u8>, mortar_plumb::Report, FileSource);

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
                    .map_err(|e| format!("{}: {e}", rsp.display()))?
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
        let files = files.read_all();
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
        let files = files.read_all();
        let inputs: Vec<Input<'_>> =
            files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        // Stage 3 judges the mutants of a dynamic output (spec/plumb.md §12).
        let files = BaseFiles(Path::new(""));
        let ctx = mortar_plumb::Ctx { line: line.clone(), files: &files };
        if cfg.sweep {
            let opts = mortar_plumb::sweep::Options {
                threads: cfg.threads,
                window: cfg.window,
                reseal: !args.no_reseal,
                ..mortar_plumb::sweep::Options::default()
            };
            let sweep = mortar_plumb::sweep::run_with(&output, &inputs, &parsed, &ctx, &opts)?;
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
    print_profile();
    Ok(report.clean())
}

/// The verifier's phase marks (`MORTAR_PLUMB_PROFILE`, WP-109), on
/// standard error.
fn print_profile() {
    for l in mortar_plumb::prof::take() {
        eprintln!("{l}");
    }
}

/// The command's entry point: exit 0 when clean, 1 on any finding, 2 on
/// a usage or read error.
pub fn main(args: &[String]) -> std::process::ExitCode {
    use std::process::ExitCode;
    if args.first().is_some_and(|a| a == "--compose") {
        return match ComposeArgs::parse(&args[1..]).and_then(|a| run_compose(&a)) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(message) => {
                eprint!("xtask plumb --compose: {message}\n{COMPOSE_USAGE}");
                ExitCode::from(2)
            }
        };
    }
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

pub const COMPOSE_USAGE: &str = "usage: cargo xtask plumb --compose <D> <D.map> <V> <V.map> \
                                 <R> <R.map> --map-format gnu|lld|mold|mortar \
                                 [--r-format gnu|lld|mold|mortar] [--line-d <file>] \
                                 [--line-v <file>] [--line-r <file>]\n";

/// The command line of `xtask plumb --compose` (spec/plumb.md §14.5 P-P1;
/// spec/harness.md LT-R2): the direct link `D`, the link `V` through the
/// `-r` output `R`, each with its map. `D` and `V` are one linker's
/// (`--map-format`); `R` may be another's (`--r-format`, LT-R3's mixed
/// toolchain). A reference's line is a file of words, one per line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeArgs {
    /// (output, map) of `D`, `V` and `R`.
    pub links: [(PathBuf, PathBuf); 3],
    pub format: Format,
    pub r_format: Format,
    pub lines: [Option<PathBuf>; 3],
}

impl ComposeArgs {
    /// Parses the arguments after `--compose`.
    pub fn parse(args: &[String]) -> Result<ComposeArgs, String> {
        let mut paths = Vec::new();
        let mut format = None;
        let mut r_format = None;
        let mut lines: [Option<PathBuf>; 3] = [None, None, None];
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let mut value =
                |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
            let fmt = |v: String| {
                Format::from_name(&v).ok_or_else(|| format!("unknown map format '{v}'"))
            };
            match a.as_str() {
                "--map-format" => format = Some(fmt(value(a)?)?),
                "--r-format" => r_format = Some(fmt(value(a)?)?),
                "--line-d" => lines[0] = Some(PathBuf::from(value(a)?)),
                "--line-v" => lines[1] = Some(PathBuf::from(value(a)?)),
                "--line-r" => lines[2] = Some(PathBuf::from(value(a)?)),
                s if s.starts_with('-') => return Err(format!("unknown option '{s}'")),
                s => paths.push(PathBuf::from(s)),
            }
        }
        let [d, dm, v, vm, r, rm]: [PathBuf; 6] = paths
            .try_into()
            .map_err(|p: Vec<PathBuf>| format!("six paths are needed, not {}", p.len()))?;
        let format = format.ok_or("no --map-format")?;
        Ok(ComposeArgs {
            links: [(d, dm), (v, vm), (r, rm)],
            format,
            r_format: r_format.unwrap_or(format),
            lines,
        })
    }
}

/// Runs P-P1 on the three links: prints the findings, the composition's
/// coverage and the exclusions by letter; `Ok(true)` when it is clean.
///
/// # Errors
/// A file cannot be read, or a map does not parse.
pub fn run_compose(args: &ComposeArgs) -> Result<bool, String> {
    let (clean, text) = compose_report(args)?;
    print!("{text}");
    Ok(clean)
}

/// [`run_compose`]'s verdict and text, unprinted (the census's `-r` lines,
/// RL-7).
///
/// # Errors
/// A file cannot be read, or a map does not parse.
pub fn compose_report(args: &ComposeArgs) -> Result<(bool, String), String> {
    use std::fmt::Write as _;
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let mut outputs = Vec::new();
    let mut maps = Vec::new();
    let mut lines = Vec::new();
    for (k, (out, map)) in args.links.iter().enumerate() {
        let format = if k == 2 { args.r_format } else { args.format };
        outputs.push(read(out)?);
        let text = String::from_utf8_lossy(&read(map)?).into_owned();
        maps.push(
            mortar_plumb::map::parse(format, &text)
                .map_err(|e| format!("{}:{}: {}", map.display(), e.line, e.message))?,
        );
        lines.push(match &args.lines[k] {
            Some(l) => {
                Some(String::from_utf8_lossy(&read(l)?).lines().map(str::to_string).collect())
            }
            None => None,
        });
    }
    let mut files = Vec::new();
    for map in &maps {
        files.push(link_files(map, Path::new(""), None)?);
    }
    let mut lines = lines.into_iter();
    let mut link = |k: usize| mortar_plumb::compose::Link {
        output: &outputs[k],
        src: &files[k],
        map: &maps[k],
        line: lines.next().flatten(),
    };
    let (d, v, r) = (link(0), link(1), link(2));
    let path = args.links[2].0.display().to_string();
    let report = mortar_plumb::compose::compose(
        &d,
        &v,
        &mortar_plumb::compose::Partial { link: r, path: &path },
    );
    let mut text = report.render(&args.links[1].0.display().to_string());
    for n in mortar_plumb::compose::not_judged(&report) {
        let _ = writeln!(text, "composition not judged: {n}");
    }
    text.push_str(&report.summary());
    Ok((report.clean(), text))
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
    let text = std::str::from_utf8(&b)
        .map_err(|_| format!("{}: not a mortar map (not text)", p.display()))?;
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
            return Err(format!(
                "{}: not an ELF file (are the arguments in the order X.map F.map X F?)",
                p.display()
            ));
        }
    }
    let mut paths: BTreeSet<String> = BTreeSet::new();
    paths.extend(xm.paths().into_iter().map(str::to_string));
    paths.extend(fm.paths().into_iter().map(str::to_string));
    let files = FileSource::open(paths.iter().map(String::as_str), Path::new(""), |_| true);
    unresolved(
        &paths.iter().map(String::as_str).collect::<Vec<_>>(),
        files.files.len(),
        Path::new(""),
        false,
    )?;
    mortar_plumb::prof::mark("read outputs, maps; open inputs");
    let eq = mortar_plumb::equiv::check_from(&x, &xm, &f, &fm, &files);
    let (xn, fname) = (a.x.display().to_string(), a.f.display().to_string());
    for line in eq.x.render(&xn).lines() {
        println!("X: {line}");
    }
    for line in eq.f.render(&fname).lines() {
        println!("F: {line}");
    }
    let pe = mortar_plumb::Report {
        findings: eq.findings.clone(),
        coverage: mortar_plumb::Coverage::default(),
    };
    print!("{}", pe.render(&xn));
    println!("X: {} findings, {} records", eq.x.findings.len(), eq.x.coverage.records);
    println!("F: {} findings, {} records", eq.f.findings.len(), eq.f.coverage.records);
    println!("{}", eq.summary());
    print_profile();
    Ok(eq.clean())
}

#[cfg(test)]
mod tests {
    use super::*;

    // WP-103: the option as the line gives it, the last word winning.
    #[test]
    fn wp103_line_fix_843419() {
        let base = std::env::temp_dir().join(format!("mortar-wp103-line-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let f = |w: &[&str]| line_fix_843419(&s(w), &base);
        assert!(!f(&["-o", "a", "a.o"]));
        assert!(f(&["--fix-cortex-a53-843419", "a.o"]));
        assert!(f(&["--fix-cortex-a53-843419=full"]));
        assert!(!f(&["--fix-cortex-a53-843419", "--no-fix-cortex-a53-843419"]));
        assert!(f(&["--no-fix-cortex-a53-843419", "--fix-cortex-a53-843419"]));
        assert!(f(&["-Wl,-z,now,--fix-cortex-a53-843419", "a.o"]));
        assert!(f(&["-Xlinker", "--fix-cortex-a53-843419"]));
        // `-Xlinker` takes its word: the word after it is not read twice.
        assert!(!f(&["-Xlinker", "--no-fix-cortex-a53-843419"]));
        std::fs::write(base.join("rsp"), "a.o '--fix-cortex-a53-843419'\n").unwrap();
        assert!(f(&["@rsp"]));
        assert!(!f(&["@rsp", "--no-fix-cortex-a53-843419"]));
        assert!(!f(&["@missing"]));
        std::fs::remove_dir_all(&base).unwrap();
    }

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
        std::fs::write(&plain, "mortar-map 2\narg x.o\nend\n").unwrap();
        let elf = dir.join("x");
        std::fs::write(&elf, b"\x7fELF\x02\x01").unwrap();
        let arg = |p: &Path| p.display().to_string();
        // A plain map as X, a missing file, swapped arguments: exit 2.
        let r = run_equiv(
            &EquivArgs::parse(&s(&["--equiv", &arg(&plain), &arg(&plain), &arg(&elf), &arg(&elf)]))
                .unwrap(),
        );
        assert!(r.is_err(), "{r:?}");
        let r = run_equiv(
            &EquivArgs::parse(&s(&["--equiv", &arg(&elf), &arg(&plain), &arg(&plain), &arg(&elf)]))
                .unwrap(),
        );
        assert!(r.is_err(), "{r:?}");
        let r = run_equiv(
            &EquivArgs::parse(&s(&[
                "--equiv",
                "/nonexistent/x.map",
                &arg(&plain),
                &arg(&elf),
                &arg(&elf),
            ]))
            .unwrap(),
        );
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
            std::fs::write(root.join(f), b"!<arch>\n").unwrap();
        }
        let got = |text: &str| -> Vec<String> {
            response_inputs(text, root)
                .unwrap()
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
        // WP-131: whether an option takes the next word is its row's in
        // the options table, for every row: a refused option's value, an
        // option no hand list named, and an abbreviation are not inputs
        // (before, `alignment`, `x.o` and `y.o` were read as files).
        assert_eq!(
            got("-Ld1 --sort-section alignment --retain-symbols-file x.o --outp y.o -lf a.o"),
            ["d1/libf.a", "a.o"]
        );
    }

    // spec/plumb.md §8: a linker script among the inputs (glibc's
    // `libm.a`) stands for the files of its INPUT and GROUP commands;
    // a command not read is refused by name.
    #[test]
    fn s8_linker_scripts_are_followed() {
        let scratch = mortar_testutil::Scratch::new("plumb-scripts");
        let root = scratch.path();
        std::fs::create_dir_all(root.join("d")).unwrap();
        for f in ["d/libm-2.41.a", "d/libmvec.a", "d/libz.so.1", "a.o"] {
            std::fs::write(root.join(f), b"!<arch>\n").unwrap();
        }
        let abs = root.join("d/libm-2.41.a");
        let script = |name: &str, text: &str| std::fs::write(root.join(name), text).unwrap();
        script(
            "d/libm.a",
            &format!(
                "/* GNU ld script\n*/\nOUTPUT_FORMAT(elf64-littleaarch64)\nGROUP ( {} libmvec.a )\n",
                abs.display()
            ),
        );
        script("d/libz.so", "INPUT(AS_NEEDED(libz.so.1),a.o)");
        script("d/libbad.a", "SEARCH_DIR(/x) GROUP(libmvec.a)");
        script("d/libl.a", "GROUP(-lz)");
        script("d/libopen.a", "GROUP(libmvec.a");
        let got = |text: &str| -> Result<Vec<String>, String> {
            response_inputs(text, root).map(|v| {
                v.iter().map(|p| p.strip_prefix(root).unwrap().display().to_string()).collect()
            })
        };
        // An absolute name as it is; a relative one in the working
        // directory first (a.o), then the library directories.
        assert_eq!(got("-static -Ld -lm").unwrap(), ["d/libm-2.41.a", "d/libmvec.a"]);
        assert_eq!(got("-Ld -lz").unwrap(), ["d/libz.so.1", "a.o"]);
        assert_eq!(got("-Ld d/libm.a").unwrap(), ["d/libm-2.41.a", "d/libmvec.a"]);
        for (lib, why) in [
            ("bad", "command 'SEARCH_DIR'"),
            ("l", "the library name '-lz'"),
            ("open", "a command that is not closed"),
        ] {
            let e = got(&format!("-static -Ld -l{lib}")).unwrap_err();
            assert!(e.contains(&format!("lib{lib}.a")) && e.contains(why), "{e}");
        }
    }

    // spec/plumb.md §8.1: the command's source reads by position; a read
    // past the length the file had when it was opened, or of a file that
    // shrank since, is an error naming the file, not a panic. A thin
    // archive's members are files of their own, before it; a directory is
    // not an input.
    #[test]
    fn s8_1_file_source() {
        use mortar_plumb::Source;
        let scratch = mortar_testutil::Scratch::new("plumb-file-source");
        let root = scratch.path();
        std::fs::write(root.join("a.o"), b"0123456789").unwrap();
        std::fs::write(root.join("m.o"), b"member").unwrap();
        let mut thin = b"!<thin>\n".to_vec();
        thin.extend_from_slice(format!("{:<16}{:<32}{:<10}`\n", "m.o/", "", 6).as_bytes());
        std::fs::write(root.join("libt.a"), &thin).unwrap();
        std::fs::create_dir(root.join("d.o")).unwrap();
        let src = FileSource::open(["a.o", "libt.a", "d.o", "none.o"].into_iter(), root, |_| true);
        let paths: Vec<&str> = (0..src.count()).map(|i| src.path(i)).collect();
        assert_eq!(paths, ["a.o", "m.o", "libt.a"]);
        assert_eq!(Source::len(&src, 0), 10);
        let mut b = [0u8; 4];
        src.read_at(0, 6, &mut b).unwrap();
        assert_eq!(&b, b"6789");
        let e = src.read_at(0, 7, &mut b).unwrap_err();
        assert!(e.contains("a.o") && e.contains("past its end"), "{e}");
        std::fs::write(root.join("a.o"), b"01").unwrap();
        let e = src.read_at(0, 2, &mut b).unwrap_err();
        assert!(e.contains("a.o") && e.contains("cannot be read"), "{e}");
        assert!(src.read_at(9, 0, &mut b).is_err());
        assert_eq!(src.read_all().len(), 2, "a.o shrank: left out");
    }

    // spec/plumb.md §8: a map none of whose files can be read (the
    // verifier run from another directory) is a usage error naming the
    // first file and the directory, not a report of every input I1.
    #[test]
    fn s8_no_input_resolved_is_a_usage_error() {
        let scratch = mortar_testutil::Scratch::new("plumb-unresolved");
        let root = scratch.path();
        std::fs::write(root.join("a.o"), b"\x7fELF").unwrap();
        let paths = ["a.o", "b.o"];
        let read = |base: &Path| FileSource::open(paths.iter().copied(), base, |_| true);
        let found = read(root);
        assert_eq!(found.files.len(), 1);
        assert!(unresolved(&paths, found.files.len(), root, false).is_ok());
        let elsewhere = root.join("elsewhere");
        let e = unresolved(&paths, read(&elsewhere).files.len(), &elsewhere, false).unwrap_err();
        assert!(
            e.contains("the first: a.o") && e.contains("elsewhere") && e.contains("2 files"),
            "{e}"
        );
        let e = unresolved(&paths, 0, root, true).unwrap_err();
        assert!(e.contains("--inputs-from"), "{e}");
        assert!(unresolved(&[], 0, root, false).is_ok());
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
