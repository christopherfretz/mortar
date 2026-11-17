//! The command line of a dynamic link, read by the verifier itself
//! (stage 3, `spec/plumb.md` §12, P-D2): the output kind
//! (D-1), the interpreter, `-z now`/`-z norelro`, the run paths, and the
//! shared-object mentions in order with their `--as-needed` state (D-11,
//! D-19a), linker scripts expanded inline. Files are read through
//! [`Files`], so tests and `xtask` decide where bytes come from.

use crate::map::OutputKind;
use crate::opts::Word;

/// Where the verifier reads files from.
///
/// Only [`Files::read`] is required; the positional reads have defaults
/// that read the whole file (a caller over real files overrides them, so
/// that the input set of §3.8 reads an archive's members by position and
/// the line's files are sniffed by their first bytes, §8.1).
pub trait Files: Sync {
    /// The bytes of `path`, or `None` when it cannot be read.
    fn read(&self, path: &str) -> Option<Vec<u8>>;

    /// The length of `path` in bytes, or `None` when it cannot be read.
    fn len(&self, path: &str) -> Option<u64> {
        self.read(path).map(|b| b.len() as u64)
    }

    /// The first `n` bytes of `path` (fewer for a shorter file).
    fn head(&self, path: &str, n: usize) -> Option<Vec<u8>> {
        self.read(path).map(|mut b| {
            b.truncate(n);
            b
        })
    }

    /// Exactly `buf.len()` bytes of `path` from `offset`.
    ///
    /// # Errors
    /// The message names the file and what failed.
    fn read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        let b = self.read(path).ok_or_else(|| format!("{path}: cannot be read"))?;
        let got = usize::try_from(offset)
            .ok()
            .and_then(|at| b.get(at..at.checked_add(buf.len())?))
            .ok_or_else(|| {
                format!("{path}: {} bytes at {offset:#x} are past its end", buf.len())
            })?;
        buf.copy_from_slice(got);
        Ok(())
    }
}

/// The host's file system.
#[derive(Debug)]
pub struct HostFiles;

impl Files for HostFiles {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(path).ok()
    }
    fn len(&self, path: &str) -> Option<u64> {
        std::fs::metadata(path).ok().filter(std::fs::Metadata::is_file).map(|m| m.len())
    }
    fn head(&self, path: &str, n: usize) -> Option<Vec<u8>> {
        host_head(std::path::Path::new(path), n)
    }
    fn read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        host_read_at(std::path::Path::new(path), offset, buf)
    }
}

/// The first `n` bytes of a regular file of the host.
pub fn host_head(path: &std::path::Path, n: usize) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let f = std::fs::File::open(path).ok()?;
    if !f.metadata().ok()?.is_file() {
        return None;
    }
    let mut b = Vec::with_capacity(n);
    f.take(n as u64).read_to_end(&mut b).ok()?;
    Some(b)
}

/// `buf.len()` bytes of a file of the host from `offset`.
///
/// # Errors
/// The message names the file and what failed.
pub fn host_read_at(path: &std::path::Path, offset: u64, buf: &mut [u8]) -> Result<(), String> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.seek(SeekFrom::Start(offset)).map_err(|e| format!("{}: {e}", path.display()))?;
    f.read_exact(buf)
        .map_err(|e| format!("{}: {} bytes at {offset:#x}: {e}", path.display(), buf.len()))
}

/// An input of the line, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A relocatable object or an archive, by its path as named, with the
    /// `--whole-archive` state (an archive's members are all live) and the
    /// `--start-lib` state (an object is lazy) in force where it is named
    /// (input §5.2, §7.5, §7.6; the input set, §3.8).
    Relocatable { path: String, group: Option<usize>, whole: bool, lib: bool },
    /// A shared object mention (D-19a): the file found, the state in
    /// force, the `--start-group`/`GROUP` it is in.
    /// `named` is the name the linker records in `DT_NEEDED` when the
    /// object has no `DT_SONAME` (input §8.3): the file name of a `-l`
    /// search, the path as written otherwise.
    Shared { path: String, named: String, as_needed: bool, group: Option<usize> },
}

/// The line, read.
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)] // independent options of the line
pub struct Line {
    pub kind: Option<OutputKind>,
    pub interp: Option<String>,
    /// `-z now` (or `--now`, `-z lazy` last wins).
    pub now: bool,
    /// `-z relro` unless `-z norelro` (the default on GNU ld, lld, mold).
    pub relro: bool,
    /// The last of `-z execstack` (true) and `-z noexecstack` (false).
    pub execstack: Option<bool>,
    pub export_dynamic: bool,
    /// `--export-dynamic-symbol` or `--export-dynamic-symbol-list` (D-21;
    /// in a static-pie D-1 gives them no effect, §12.4).
    pub export_symbols: bool,
    pub allow_shlib_undefined: bool,
    /// `--enable-new-dtags` (the default): `DT_RUNPATH`, else `DT_RPATH`.
    pub new_dtags: bool,
    pub rpath: Vec<String>,
    pub rpath_link: Vec<String>,
    pub lib_dirs: Vec<String>,
    pub items: Vec<Item>,
    /// `-u` names (file 0 references, D-11 item 2).
    pub undefined: Vec<String>,
    pub entry: Option<String>,
    /// `-init`/`-fini` (`DT_INIT`/`DT_FINI` name these functions, D-30).
    pub init: Option<String>,
    pub fini: Option<String>,
    /// `-shared` was given.
    pub shared: bool,
    /// `-r` (`--relocatable`) was given.
    pub relocatable: bool,
    /// `--require-defined` names (`spec/resolve.md` §4.2).
    pub require_defined: Vec<String>,
    /// `--defsym` operands as written (`A=EXPR`), each with the number of
    /// items named before it (GNU ld reads it in place, resolve §4.2).
    pub defsym: Vec<(String, usize)>,
    /// `--wrap` names (resolve §7.1).
    pub wrap: Vec<String>,
    /// The `EXTERN` names of the implicit scripts, each with the number
    /// of items named before the script (input §5.1; resolve §4.2).
    pub externs: Vec<(String, usize)>,
    /// The last `ENTRY` of an implicit script.
    pub script_entry: Option<String>,
    /// `-T`/`--script`/`--default-script` files (not inputs: their
    /// commands are read by the input set, §3.8).
    pub scripts: Vec<String>,
    /// Options whose effect on the input set this reader does not model
    /// (`--undefined-glob`, `--mri-script`), by name.
    pub unmodelled: Vec<String>,
    /// The last of the `-Bsymbolic` family, by its row (`-Bsymbolic`,
    /// `-Bsymbolic-functions`, `--Bsymbolic-non-weak`,
    /// `--Bsymbolic-non-weak-functions`); `None` with none or after
    /// `-Bno-symbolic` (V2, A44).
    pub symbolic: Option<&'static str>,
    /// What the verifier could not read (each a finding of P-D2).
    pub problems: Vec<String>,
    /// What puts the link outside stage 3 (each a P-D1 finding,
    /// [`crate::report::OUTSIDE`]): an option `spec/dyn.md` refuses, or a
    /// kind stage 3 does not judge.
    pub outside: Vec<String>,
}

/// The rows of the options table (`opts.rs`, `docs/status/options.md`)
/// whose option this reader interprets, by the row's first spelling: every
/// spelling of such a row, and every form of its value, reaches the same
/// state, and every other row leaves the state alone (WP-131, V1 and V6;
/// `tests::s12_1_every_spelling_of_a_row_reads_alike`). Whether an option
/// takes the next word is the table's too, for every row.
#[cfg_attr(not(test), allow(dead_code))] // the tests' list; the match below is the reader
pub(crate) const INTERPRETED: &[&str] = &[
    "-pie",
    "-no-pie",
    "-shared",
    "-Bstatic",
    "-Bdynamic",
    "--as-needed",
    "--no-as-needed",
    "--push-state",
    "--pop-state",
    "--no-dynamic-linker",
    "-I",
    "-E",
    "--no-export-dynamic",
    "--export-dynamic-symbol",
    "--export-dynamic-symbol-list",
    "--allow-shlib-undefined",
    "--no-allow-shlib-undefined",
    "--enable-new-dtags",
    "--disable-new-dtags",
    "-(",
    "-)",
    "-rpath",
    "-R",
    "-rpath-link",
    "-u",
    "-e",
    "-init",
    "-fini",
    "-l",
    "-L",
    "--copy-dt-needed-entries",
    "--hash-style",
    "-Bsymbolic",
    "-Bsymbolic-functions",
    "--Bsymbolic-non-weak",
    "--Bsymbolic-non-weak-functions",
    "-Bno-symbolic",
    "--whole-archive",
    "--no-whole-archive",
    "--start-lib",
    "--end-lib",
    "--require-defined",
    "--defsym",
    "--wrap",
    "-r",
    "-T",
    "--default-script",
    "--undefined-glob",
    "-c",
];

/// The `-z` keywords this reader interprets.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const INTERPRETED_Z: &[&str] =
    &["now", "lazy", "relro", "norelro", "execstack", "noexecstack"];

#[derive(Clone, Copy)]
#[allow(clippy::struct_excessive_bools)] // the positional state of cli.md §3.3
struct State {
    as_needed: bool,
    bstatic: bool,
    whole: bool,
    lib: bool,
}

impl Line {
    /// Reads `args` (the linker's words, without the program name). Each
    /// word is placed by the options table (`opts::Table::word`, cli.md
    /// §2): its row, its value, or an input file.
    #[allow(clippy::too_many_lines, clippy::many_single_char_names)] // one arm per option family
    pub fn parse(args: &[String], files: &dyn Files) -> Line {
        let table = crate::opts::Table::get();
        let mut l = Line { relro: true, new_dtags: true, ..Line::default() };
        let mut pie: Option<bool> = None;
        let mut static_pie = false;
        let mut no_interp = false;
        let mut shared = false;
        // The library directories come first: `-L` applies to every `-l`
        // whatever its position (GNU ld, lld, mold).
        let mut i = 0;
        while i < args.len() {
            let (w, used) = table.word(args, i);
            if let Word::Opt { id: "-L", value: Some(d), .. } = w {
                l.lib_dirs.push(d);
            }
            i += used;
        }
        let mut st = State { as_needed: false, bstatic: false, whole: false, lib: false };
        let mut stack: Vec<State> = Vec::new();
        let mut group: Option<usize> = None;
        let mut groups = 0usize;
        let mut i = 0;
        while i < args.len() {
            let (w, used) = table.word(args, i);
            let a = args[i].as_str();
            i += used;
            let (id, value) = match w {
                Word::Input => {
                    l.file(a, a, st, group, files, 0);
                    continue;
                }
                Word::Z { kw, .. } => {
                    l.z(kw);
                    continue;
                }
                Word::Unknown => {
                    // No row of the table: no linker of cli.md has it.
                    // Two words some callers' lines carry (the drivers'
                    // spellings) are still read.
                    match a {
                        "-static-pie" | "--static-pie" => static_pie = true,
                        "--now" => l.now = true,
                        _ => {}
                    }
                    continue;
                }
                Word::Opt { id, value, .. } => (id, value),
            };
            let v = value.unwrap_or_default();
            match id {
                "-pie" => pie = Some(true),
                "-no-pie" => pie = Some(false),
                "-shared" => shared = true,
                // cli.md §3.2: `-static` is `-Bstatic`; neither decides the
                // kind (every spelling of the row alike, WP-131).
                "-Bstatic" => st.bstatic = true,
                "-Bdynamic" => st.bstatic = false,
                "--as-needed" => st.as_needed = true,
                "--no-as-needed" => st.as_needed = false,
                "--push-state" => stack.push(st),
                "--pop-state" => {
                    if let Some(s) = stack.pop() {
                        st.as_needed = s.as_needed;
                        st.bstatic = s.bstatic;
                        st.whole = s.whole;
                    } else {
                        l.problems.push("--pop-state without --push-state".into());
                    }
                }
                "--no-dynamic-linker" => no_interp = true,
                "-I" => l.interp = Some(v),
                "-E" => l.export_dynamic = true,
                "--no-export-dynamic" => l.export_dynamic = false,
                "--export-dynamic-symbol" | "--export-dynamic-symbol-list" => {
                    l.export_symbols = true;
                }
                "--allow-shlib-undefined" => l.allow_shlib_undefined = true,
                "--no-allow-shlib-undefined" => l.allow_shlib_undefined = false,
                "--enable-new-dtags" => l.new_dtags = true,
                "--disable-new-dtags" => l.new_dtags = false,
                "-(" => {
                    group = Some(groups);
                    groups += 1;
                }
                "-)" => group = None,
                "-rpath" => l.rpath.extend(v.split(':').map(str::to_string)),
                // GNU ld: `-R` (`--just-symbols`) naming a directory is
                // `-rpath`; naming a file it takes only its symbols, which
                // stage 3 does not model.
                "-R" => {
                    if files.len(&v).is_some() {
                        l.outside.push(format!("--just-symbols {v} (cli.md: refused)"));
                    } else {
                        l.rpath.extend(v.split(':').map(str::to_string));
                    }
                }
                "-rpath-link" => l.rpath_link.extend(v.split(':').map(str::to_string)),
                "-u" => l.undefined.push(v),
                "-e" => l.entry = Some(v),
                "-init" => l.init = Some(v),
                "-fini" => l.fini = Some(v),
                "--whole-archive" => st.whole = true,
                "--no-whole-archive" => st.whole = false,
                "--start-lib" => st.lib = true,
                "--end-lib" => st.lib = false,
                "--require-defined" => l.require_defined.push(v),
                "--defsym" => l.defsym.push((v, l.items.len())),
                "--wrap" => l.wrap.push(v),
                "-r" => l.relocatable = true,
                "-T" | "--default-script" => l.scripts.push(v),
                "--undefined-glob" | "-c" => l.unmodelled.push(format!("{id} {v}")),
                "-l" => l.library(&v, st, group, files),
                "--copy-dt-needed-entries" => {
                    l.outside.push("--copy-dt-needed-entries (dyn.md D-15)".into());
                }
                "--hash-style" => l.hash_style(&v),
                "-Bsymbolic"
                | "-Bsymbolic-functions"
                | "--Bsymbolic-non-weak"
                | "--Bsymbolic-non-weak-functions" => l.symbolic = Some(id),
                "-Bno-symbolic" => l.symbolic = None,
                // Every other option leaves the inputs alone.
                _ => {}
            }
        }
        // D-1 and cli.md §3.2: the kind is the line's (`-pie`, the
        // interpreter) and the inputs' (a shared object loaded makes the
        // executable dynamic); never the output's (WP-131, V5).
        let loads_shared = l.items.iter().any(|i| matches!(i, Item::Shared { .. }));
        l.shared = shared;
        if no_interp {
            l.interp = None;
        }
        l.kind = Some(if shared {
            l.outside.push("-shared (M4)".into());
            OutputKind::Pie
        } else if static_pie {
            OutputKind::StaticPie
        } else if pie == Some(true) {
            if l.interp.is_none() && (no_interp || !loads_shared) {
                OutputKind::StaticPie
            } else {
                OutputKind::Pie
            }
        } else if l.interp.is_some() || loads_shared {
            OutputKind::Exec
        } else {
            OutputKind::Static
        });
        l
    }

    /// `--hash-style=<v>`: `gnu` is D-20's; every other value is refused
    /// by `spec/cli.md` §4.4.
    fn hash_style(&mut self, v: &str) {
        if v != "gnu" {
            self.outside.push(format!("--hash-style={v} (dyn.md D-20; cli.md §4.4)"));
        }
    }

    fn z(&mut self, z: &str) {
        match z {
            "now" => self.now = true,
            "lazy" => self.now = false,
            "relro" => self.relro = true,
            "norelro" => self.relro = false,
            "execstack" => self.execstack = Some(true),
            "noexecstack" => self.execstack = Some(false),
            _ => {}
        }
    }

    fn library(&mut self, name: &str, st: State, group: Option<usize>, files: &dyn Files) {
        let found = if let Some(exact) = name.strip_prefix(':') {
            self.lib_dirs.iter().map(|d| format!("{d}/{exact}")).find(|p| files.len(p).is_some())
        } else {
            let mut found = None;
            for d in &self.lib_dirs {
                if !st.bstatic {
                    let so = format!("{d}/lib{name}.so");
                    if files.len(&so).is_some() {
                        found = Some(so);
                        break;
                    }
                }
                let a = format!("{d}/lib{name}.a");
                if files.len(&a).is_some() {
                    found = Some(a);
                    break;
                }
            }
            found
        };
        match found {
            Some(p) => {
                let named = p.rsplit_once('/').map_or(p.as_str(), |(_, f)| f).to_string();
                self.file(&p, &named, st, group, files, 0);
            }
            None => self.problems.push(format!("-l{name}: not found in the -L directories")),
        }
    }

    /// An input file: by its first bytes, an object, an archive, a shared
    /// object or a script.
    fn file(
        &mut self,
        path: &str,
        named: &str,
        st: State,
        group: Option<usize>,
        files: &dyn Files,
        depth: u32,
    ) {
        let Some(bytes) = files.head(path, 64) else {
            self.problems.push(format!("{path}: cannot be read"));
            return;
        };
        if bytes.starts_with(b"\x7fELF") {
            let e_type = bytes.get(16..18).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
            if e_type == mortar_elf::consts::ET_DYN {
                if st.bstatic {
                    self.problems
                        .push(format!("{path}: a shared object after -Bstatic (dyn.md D-19)"));
                }
                self.items.push(Item::Shared {
                    path: path.into(),
                    named: named.into(),
                    as_needed: st.as_needed,
                    group,
                });
            } else {
                self.items.push(Item::Relocatable {
                    path: path.into(),
                    group,
                    whole: false,
                    lib: st.lib,
                });
            }
        } else if bytes.starts_with(b"!<arch>\n") || bytes.starts_with(b"!<thin>\n") {
            self.items.push(Item::Relocatable {
                path: path.into(),
                group,
                whole: st.whole,
                lib: false,
            });
        } else if bytes.is_empty() {
            // input §2.3: an empty file is ignored.
        } else if depth > 8 {
            self.problems.push(format!("{path}: scripts nested too deeply"));
        } else {
            let Some(text) = files.read(path) else {
                self.problems.push(format!("{path}: cannot be read"));
                return;
            };
            self.script(path, &String::from_utf8_lossy(&text), st, group, files, depth);
        }
    }

    /// The subset of the script language a shared-library script uses:
    /// `INPUT`, `GROUP`, `AS_NEEDED`, `OUTPUT_FORMAT`, comments. Anything
    /// else is a problem (named).
    #[allow(clippy::too_many_lines)] // one arm per script command
    fn script(
        &mut self,
        path: &str,
        text: &str,
        st: State,
        group: Option<usize>,
        files: &dyn Files,
        depth: u32,
    ) {
        // A quoted word is never a keyword or a parenthesis (script.md
        // §1.6) and a comma separates words (§2.1, §5.1): `words` is what
        // the command match sees (`QUOTED` for a string), `text_of` the word.
        let toks: Vec<(String, bool)> =
            tokens(text, &['(', ')', ',']).into_iter().filter(|(t, q)| *q || t != ",").collect();
        let words: Vec<&str> =
            toks.iter().map(|(t, q)| if *q { QUOTED } else { t.as_str() }).collect();
        let text_of = |k: usize| toks[k].0.as_str();
        let mut i = 0;
        let mut depth_paren = 0;
        let mut as_needed_at: Option<usize> = None;
        let mut in_input = false;
        let mut grp = group;
        while i < words.len() {
            let w = words[i];
            match w {
                "OUTPUT_FORMAT" | "OUTPUT_ARCH" | "SEARCH_DIR" | "TARGET" => {
                    // Skip its parenthesised argument.
                    let mut d = 0;
                    i += 1;
                    while i < words.len() {
                        match words[i] {
                            "(" => d += 1,
                            ")" => {
                                d -= 1;
                                if d == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                }
                "INPUT" | "GROUP" => {
                    in_input = true;
                    if w == "GROUP" && grp.is_none() {
                        grp = Some(1_000_000 + self.items.len());
                    }
                }
                "AS_NEEDED" => as_needed_at = Some(depth_paren),
                // input §5.1: recorded for the phases that use them (the
                // input set's roots, §3.8; resolve §4.2).
                "EXTERN" | "ENTRY" if !in_input => {
                    let at = self.items.len();
                    let mut names = Vec::new();
                    let mut j = i + 1;
                    if words.get(j) != Some(&"(") {
                        self.problems.push(format!("{path}: {w} without '('"));
                        return;
                    }
                    j += 1;
                    while let Some(&x) = words.get(j) {
                        if x == ")" {
                            break;
                        }
                        names.push(text_of(j).to_string());
                        j += 1;
                    }
                    if j >= words.len() {
                        self.problems.push(format!("{path}: {w} without ')'"));
                        return;
                    }
                    if w == "ENTRY" {
                        self.script_entry = names.pop();
                    } else {
                        self.externs.extend(names.into_iter().map(|n| (n, at)));
                    }
                    i = j;
                }
                "(" => depth_paren += 1,
                ")" => {
                    depth_paren -= 1;
                    if as_needed_at == Some(depth_paren) {
                        as_needed_at = None;
                    }
                    if depth_paren == 0 {
                        in_input = false;
                        grp = group;
                    }
                }
                _ if in_input => {
                    let sub = State { as_needed: st.as_needed || as_needed_at.is_some(), ..st };
                    // A quoted `-l...` is a file name for GNU ld (script.md
                    // §2.4); mortar refuses it, so no link of it is judged.
                    let w = text_of(i);
                    if let Some(n) = words[i].strip_prefix("-l") {
                        self.library(n, sub, grp, files);
                    } else if w.starts_with('/') || files.len(w).is_some() {
                        self.file(w, w, sub, grp, files, depth + 1);
                    } else {
                        // GNU ld: a relative name is tried in the current
                        // directory, the script's, then the -L path.
                        let dir = path.rsplit_once('/').map(|(d, _)| d.to_string());
                        let found = dir
                            .iter()
                            .chain(self.lib_dirs.iter())
                            .map(|d| format!("{d}/{w}"))
                            .find(|p| files.len(p).is_some());
                        match found {
                            Some(p) => self.file(&p, w, sub, grp, files, depth + 1),
                            None => self.problems.push(format!("{w} (in {path}): not found")),
                        }
                    }
                }
                _ => {
                    let w = text_of(i);
                    self.problems.push(format!("{path}: script word '{w}' is not modelled"));
                    return;
                }
            }
            i += 1;
        }
    }

    /// The position of the first relocatable item named `path` (D-19a).
    pub fn position_of(&self, path: &str) -> Option<usize> {
        self.items.iter().position(|i| matches!(i, Item::Relocatable { path: p, .. } if p == path))
    }
}

/// The command match's key for a quoted word: no keyword, parenthesis or
/// `-l` prefix can equal it (script.md §1.6).
const QUOTED: &str = "\"";

/// A script's tokens, `(text, quoted)`: comments removed, each byte of
/// `punct` a token of its own, a quoted string (`"` to the next `"`, no
/// escapes, script.md §1.6) one token without its quotes, quoted. A string
/// directly followed by a name is two tokens; an unterminated string runs
/// to the end.
pub(crate) fn tokens(text: &str, punct: &[char]) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    let flush = |cur: &mut String, out: &mut Vec<(String, bool)>| {
        if !cur.is_empty() {
            out.push((std::mem::take(cur), false));
        }
    };
    while let Some(c) = chars.next() {
        if c == '/' && chars.peek() == Some(&'*') {
            flush(&mut cur, &mut out);
            chars.next();
            let mut prev = ' ';
            for d in chars.by_ref() {
                if prev == '*' && d == '/' {
                    break;
                }
                prev = d;
            }
        } else if c == '"' {
            flush(&mut cur, &mut out);
            let s: String = chars.by_ref().take_while(|&d| d != '"').collect();
            out.push((s, true));
        } else if c.is_whitespace() {
            flush(&mut cur, &mut out);
        } else if punct.contains(&c) {
            flush(&mut cur, &mut out);
            out.push((c.to_string(), false));
        } else {
            cur.push(c);
        }
    }
    flush(&mut cur, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Mem(HashMap<String, Vec<u8>>);
    impl Files for Mem {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.0.get(path).cloned()
        }
    }

    fn elf(e_type: u16) -> Vec<u8> {
        let mut v = vec![0u8; 64];
        v[..4].copy_from_slice(b"\x7fELF");
        v[16..18].copy_from_slice(&e_type.to_le_bytes());
        v
    }

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_default_gcc_line_and_the_libc_script() {
        let mut m = HashMap::new();
        m.insert("/l/libc.so".to_string(), b"/* GNU ld script */\nOUTPUT_FORMAT(elf64-littleaarch64)\nGROUP ( /l/libc.so.6 /l/libc_nonshared.a  AS_NEEDED ( /l/ld.so.1 ) )\n".to_vec());
        m.insert("/l/libc.so.6".into(), elf(3));
        m.insert("/l/ld.so.1".into(), elf(3));
        m.insert("/l/libc_nonshared.a".into(), b"!<arch>\n".to_vec());
        m.insert("/l/libgcc_s.so".into(), elf(3));
        m.insert("/l/libgcc.a".into(), b"!<arch>\n".to_vec());
        m.insert("h.o".into(), elf(1));
        let l = Line::parse(
            &words(
                "--as-needed -dynamic-linker /lib/ld.so.1 -pie -z now -o h -L/l h.o -lgcc --push-state --no-as-needed -lgcc_s --pop-state -lc",
            ),
            &Mem(m),
        );
        assert!(l.problems.is_empty(), "{:?}", l.problems);
        assert_eq!(l.kind, Some(OutputKind::Pie));
        assert!(l.now && l.relro);
        assert_eq!(l.interp.as_deref(), Some("/lib/ld.so.1"));
        let shared: Vec<(String, bool)> = l
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Shared { path, as_needed, .. } => Some((path.clone(), *as_needed)),
                Item::Relocatable { .. } => None,
            })
            .collect();
        assert_eq!(
            shared,
            vec![
                ("/l/libgcc_s.so".into(), false),
                ("/l/libc.so.6".into(), true),
                ("/l/ld.so.1".into(), true)
            ]
        );
        assert_eq!(l.position_of("h.o"), Some(0));
    }

    #[test]
    fn the_output_kinds() {
        let m = Mem(HashMap::new());
        let k = |s: &str| Line::parse(&words(s), &m).kind;
        assert_eq!(k("-o a"), Some(OutputKind::Static));
        assert_eq!(k("-dynamic-linker /x -o a"), Some(OutputKind::Exec));
        assert_eq!(k("-pie -dynamic-linker /x"), Some(OutputKind::Pie));
        assert_eq!(k("-static-pie"), Some(OutputKind::StaticPie));
        assert_eq!(k("-pie --no-dynamic-linker"), Some(OutputKind::StaticPie));
    }

    // spec: plumb.md §12.1 — a link stage 3 does not model is named
    // (P-D1, "outside stage 3"), never judged by D rules it is outside of.
    #[test]
    fn s12_1_links_outside_stage_3_are_named() {
        let m = Mem(HashMap::new());
        let out = |s: &str| Line::parse(&words(s), &m).outside;
        assert!(out("-pie -dynamic-linker /x --hash-style=gnu").is_empty());
        assert_eq!(
            out("-pie -dynamic-linker /x --hash-style=sysv"),
            ["--hash-style=sysv (dyn.md D-20; cli.md §4.4)"]
        );
        assert_eq!(
            out("-pie -dynamic-linker /x --hash-style both"),
            ["--hash-style=both (dyn.md D-20; cli.md §4.4)"]
        );
        assert!(out("-static-pie -o a").is_empty());
        assert!(out("-pie --no-dynamic-linker -o a").is_empty());
        // A static-pie with exports is judged (§12.4, WP-088).
        assert!(out("-static-pie -E -o a").is_empty());
        assert!(out("-pie --no-dynamic-linker --export-dynamic -o a").is_empty());
        assert_eq!(
            out("-dynamic-linker /x --copy-dt-needed-entries"),
            ["--copy-dt-needed-entries (dyn.md D-15)"]
        );
        assert_eq!(out("-shared -o a.so"), ["-shared (M4)"]);
    }

    // spec: plumb.md §12.4 — `--export-dynamic-symbol` in either form is
    // read as an option with its value (never as an input file) and
    // recorded: in a static-pie it allows a reference's exports.
    #[test]
    fn s12_4_export_dynamic_symbol_is_an_option_with_a_value() {
        let m = Mem(HashMap::new());
        for s in [
            "-static-pie --export-dynamic-symbol foo -o a",
            "-static-pie --export-dynamic-symbol=f* -o a",
            "-static-pie -export-dynamic-symbol-list list -o a",
        ] {
            let l = Line::parse(&words(s), &m);
            assert!(l.export_symbols, "{s}");
            assert!(!l.export_dynamic, "{s}");
            assert!(l.items.is_empty() && l.problems.is_empty(), "{s}: {:?}", l.problems);
        }
        assert!(!Line::parse(&words("-static-pie -E -o a"), &m).export_symbols);
    }

    // spec: dyn.md D-30 (input §8.6) — a shared object without a soname is
    // recorded in DT_NEEDED by the name the line gives: the file name of
    // an `-l` search, the path as written otherwise.
    #[test]
    fn s12_1_a_soname_less_library_is_named_as_the_line_names_it() {
        let mut m = HashMap::new();
        m.insert("./d/libv.so".to_string(), elf(3));
        m.insert("sub/libw.so".to_string(), elf(3));
        let l = Line::parse(&words("-pie -dynamic-linker /x -L./d -lv sub/libw.so"), &Mem(m));
        let named: Vec<(String, String)> = l
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Shared { path, named, .. } => Some((path.clone(), named.clone())),
                Item::Relocatable { .. } => None,
            })
            .collect();
        assert_eq!(
            named,
            vec![
                ("./d/libv.so".into(), "libv.so".into()),
                ("sub/libw.so".into(), "sub/libw.so".into())
            ]
        );
    }

    /// Every way the table lets `name` (a spelling without its dashes) be
    /// written with a value `v` (cli.md §2.2, §2.3): one or two dashes as
    /// §2.2 allows, a required value joined by `=` (a short option's by
    /// nothing) or in the next word, an optional one by `=` only.
    fn forms(name: &str, takes: Option<bool>, v: &str) -> Vec<Vec<String>> {
        let mut out = Vec::new();
        for dashes in [1, 2] {
            if !crate::opts::dashes_allow(name, dashes) {
                continue;
            }
            let o = format!("{}{name}", "-".repeat(dashes));
            let short = name.chars().count() == 1;
            match takes {
                None => out.push(vec![o]),
                Some(true) => {
                    out.push(vec![o.clone(), v.to_string()]);
                    out.push(vec![if short { format!("{o}{v}") } else { format!("{o}={v}") }]);
                }
                Some(false) => {
                    out.push(vec![o.clone()]);
                    out.push(vec![format!("{o}={v}")]);
                }
            }
        }
        out
    }

    /// The state of a line, everything [`Line`] holds.
    fn state(words: &[String], m: &Mem) -> String {
        format!("{:?}", Line::parse(words, m))
    }

    // spec: plumb.md §3.2a, §12.1 (WP-131, V1 and V6) — the reader takes
    // its spellings and values from the options table: for EVERY row of
    // the table (accepted or refused), every spelling in every form
    // reaches the state its first spelling reaches; a row the reader does
    // not interpret leaves the state as it was (its value, in either form,
    // is consumed, never read as an input file); a row it interprets
    // changes it. A test over the table, not over a hand list.
    #[test]
    fn s12_1_every_spelling_of_a_row_reads_alike() {
        let mut files = HashMap::new();
        files.insert("/l/libg.so".to_string(), elf(3));
        files.insert("/l/libg.a".to_string(), b"!<arch>\n".to_vec());
        files.insert("/l/libh.so".to_string(), elf(3));
        // An object, so that `--start-lib` shows on it.
        files.insert("h.o".to_string(), elf(1));
        let m = Mem(files);
        let t = crate::opts::Table::get();
        // The line each option is put in: `h.o -lg` after the context
        // that lets an interpreted option show (an undoing option after
        // what it undoes).
        let context = |id: &str| -> (&str, &str) {
            match id {
                "-no-pie" => ("-pie", ""),
                "-Bdynamic" => ("-Bstatic", ""),
                "--no-as-needed" => ("--as-needed", ""),
                "--push-state" => ("", "--as-needed --pop-state"),
                "--pop-state" => ("--push-state --as-needed", ""),
                "-)" => ("-(", ""),
                "--no-dynamic-linker" => ("-dynamic-linker /x", ""),
                "--no-export-dynamic" => ("-E", ""),
                "--no-allow-shlib-undefined" => ("--allow-shlib-undefined", ""),
                "--enable-new-dtags" => ("--disable-new-dtags", ""),
                "-Bno-symbolic" => ("-Bsymbolic", ""),
                // The positional states show on an object named after.
                "--whole-archive" => ("", "/l/libg.a"),
                "--no-whole-archive" => ("--whole-archive", "/l/libg.a"),
                "--start-lib" => ("", "h.o"),
                "--end-lib" => ("--start-lib", "h.o"),
                _ => ("", ""),
            }
        };
        // The value given to an option: a word that would be an input
        // file (`zzval: cannot be read`) if it were not consumed.
        let value = |id: &str| match id {
            "-l" => "h",
            "-L" | "-rpath" | "-rpath-link" | "-R" => "/l",
            "--hash-style" => "sysv",
            _ => "zzval",
        };
        let line = |id: &str, opt: &[String]| -> Vec<String> {
            let (before, after) = context(id);
            let mut w = words(&format!("-L/l h.o {before}"));
            w.extend(opt.iter().cloned());
            w.extend(words(&format!("{after} -lg")));
            w
        };
        let mut interpreted = std::collections::BTreeSet::new();
        let rows = t.rows();
        assert!(rows.len() + t.keywords().len() > 480, "the whole table: {}", rows.len());
        for (id, names, takes, _) in &rows {
            let v = value(id);
            let mut want: Option<String> = None;
            for name in names {
                // An abbreviation of a GNU long name (§2.4) is a spelling
                // too, where it names this row alone.
                let mut spellings = vec![(*name).to_string()];
                if name.len() > 3 {
                    let ab = &name[..name.len() - 1];
                    if matches!(
                        t.word(&[format!("--{ab}")], 0).0,
                        Word::Opt { id: x, .. } if x == *id
                    ) {
                        spellings.push(ab.to_string());
                    }
                }
                for sp in spellings {
                    for f in forms(&sp, *takes, v) {
                        let got = state(&line(id, &f), &m);
                        match &want {
                            None => want = Some(got),
                            Some(w) => assert_eq!(&got, w, "row {id}: {f:?}"),
                        }
                    }
                }
            }
            let got = want.unwrap_or_default();
            let plain = state(&line(id, &[]), &m);
            if INTERPRETED.contains(id) {
                assert_ne!(got, plain, "row {id} is interpreted and changes nothing");
                interpreted.insert(*id);
            } else {
                assert_eq!(got, plain, "row {id} is not interpreted and changes the state");
            }
        }
        let listed: std::collections::BTreeSet<&str> = INTERPRETED.iter().copied().collect();
        assert_eq!(interpreted, listed, "INTERPRETED names rows the table does not have");
        for (kw, takes, _) in t.keywords() {
            let mut want: Option<String> = None;
            let k = if *takes { format!("{kw}=0x1000") } else { (*kw).to_string() };
            for f in [vec!["-z".to_string(), k.clone()], vec![format!("-z{k}")]] {
                let got = state(&line("", &f), &m);
                match &want {
                    None => want = Some(got),
                    Some(w) => assert_eq!(&got, w, "-z {kw}: {f:?}"),
                }
            }
            let got = want.unwrap_or_default();
            let plain = state(&line("", &[]), &m);
            let lazy = *kw == "lazy" || *kw == "relro"; // the defaults
            if INTERPRETED_Z.contains(kw) && !lazy {
                assert_ne!(got, plain, "-z {kw} is interpreted and changes nothing");
            } else {
                assert_eq!(got, plain, "-z {kw} is not interpreted and changes the state");
            }
        }
    }

    // spec: plumb.md §12.1 (WP-112 V1) — two dashes: `--dy` and
    // `--call_shared` undo `-Bstatic` as `-dy` does, `--dn`,
    // `--non_shared` and `--static` set it as `-dn` does.
    #[test]
    fn s12_1_v1_two_dashes() {
        let mut files = HashMap::new();
        files.insert("/l/libg.so".to_string(), elf(3));
        files.insert("/l/libg.a".to_string(), b"!<arch>\n".to_vec());
        let m = Mem(files);
        let shared = |s: &str| {
            let l = Line::parse(&words(s), &m);
            assert!(l.problems.is_empty(), "{s}: {:?}", l.problems);
            l.items.iter().any(|i| matches!(i, Item::Shared { .. }))
        };
        for on in ["-dy", "--dy", "-call_shared", "--call_shared", "-Bdynamic", "--Bdynamic"] {
            assert!(shared(&format!("-L/l -Bstatic {on} -lg")), "{on}");
        }
        for off in
            ["-dn", "--dn", "-non_shared", "--non_shared", "-static", "--static", "--Bstatic"]
        {
            assert!(!shared(&format!("-L/l {off} -lg")), "{off}");
        }
    }

    // spec: plumb.md §12.1 (WP-111 V6) — `--sort-section` takes its value
    // in the next word (BusyBox's `trylink` writes `--sort-section
    // alignment`): the value is never an input file.
    #[test]
    fn s12_1_v6_sort_section_takes_the_next_word() {
        let m = Mem(HashMap::new());
        for s in [
            "-pie -dynamic-linker /x --sort-section alignment",
            "-pie -dynamic-linker /x -sort-section alignment",
            "-pie -dynamic-linker /x --sort-section=name",
            "-pie -dynamic-linker /x --sort-sec alignment",
        ] {
            let l = Line::parse(&words(s), &m);
            assert!(l.items.is_empty() && l.problems.is_empty(), "{s}: {:?}", l.problems);
        }
    }

    // spec: script.md §1.6, §2.1, §2.4; resolve.md §4.2 (WP-152) — an
    // implicit script's quoted words are names and files, never keywords
    // or punctuation; several EXTERN names, with or without commas, are
    // roots each; `INPUT("a,b")` is one file, `"a.o"b.o` two.
    #[test]
    fn s1_6_quoted_words_of_an_implicit_script() {
        let mut m = HashMap::new();
        m.insert(
            "/l/x.ld".to_string(),
            br#"/* "not a file" */ EXTERN("k_fn") EXTERN(a,b c "INPUT")
INPUT("/l/a,b.o" "/l/a b.o""/l/c.o"/l/d.o "AS_NEEDED")
GROUP("/l/libz.a" AS_NEEDED("/l/libq.so"))
ENTRY("go")
"#
            .to_vec(),
        );
        for o in ["/l/a,b.o", "/l/a b.o", "/l/c.o", "/l/d.o", "/l/AS_NEEDED"] {
            m.insert(o.into(), elf(1));
        }
        m.insert("/l/libz.a".into(), b"!<arch>\n".to_vec());
        m.insert("/l/libq.so".into(), elf(3));
        let l = Line::parse(&words("-pie -dynamic-linker /x -o p /l/x.ld"), &Mem(m));
        assert!(l.problems.is_empty(), "{:?}", l.problems);
        let ex: Vec<&str> = l.externs.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(ex, ["k_fn", "a", "b", "c", "INPUT"]);
        assert_eq!(l.script_entry.as_deref(), Some("go"));
        let paths: Vec<(&str, bool)> = l
            .items
            .iter()
            .map(|i| match i {
                Item::Relocatable { path, .. } => (path.as_str(), false),
                Item::Shared { path, as_needed, .. } => (path.as_str(), *as_needed),
            })
            .collect();
        assert_eq!(
            paths,
            [
                ("/l/a,b.o", false),
                ("/l/a b.o", false),
                ("/l/c.o", false),
                ("/l/d.o", false),
                ("/l/AS_NEEDED", false),
                ("/l/libz.a", false),
                ("/l/libq.so", true),
            ]
        );
    }
}
