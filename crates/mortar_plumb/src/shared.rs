//! Stage 3 for a shared object (`spec/plumb.md` §12.5, P-D16-P-D26; M4,
//! WP-121).
//!
//! The export set (`spec/dyn.md` D-110) and the preemption set (D-112) are
//! recomputed here from the inputs and the line's words, never read from
//! the output's `.dynsym`, `.gnu.version_d` or `DF_SYMBOLIC`; the version
//! scripts and dynamic lists the line names are read by this module's own
//! reader (`spec/script.md` §9.1) and matched by its own matcher
//! (`spec/resolve.md` §9.7).
// ELF fields read best by the specs' short names.
#![allow(clippy::many_single_char_names, clippy::similar_names)]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use mortar_elf::SymbolSection;
use mortar_elf::consts::{
    ET_DYN, PT_INTERP, STB_GLOBAL, STB_GNU_UNIQUE, STB_LOCAL, STB_WEAK, STT_FUNC, STT_GNU_IFUNC,
    STT_NOTYPE, STT_SECTION, STT_TLS, STV_DEFAULT, STV_PROTECTED,
};

use crate::dynout::{DF_1_PIE, DT_FLAGS_1, DynOut};
use crate::image::Image;
use crate::line::{Files, Line};
use crate::locate::{Obj, Place};
use crate::map::{Format, OutputKind, SharedRecords, Symbolic};
use crate::report::{Finding, Rule};

/// The words `Line::parse` leaves for a `-shared` line (its P-D1 entry until
/// §12.5 judges it).
const SHARED_OUTSIDE: &str = "-shared (M4)";

fn fd(n: u8, msg: impl Into<String>) -> Finding {
    Finding::new(Rule::Dyn(n), msg)
}

// ---------------------------------------------------------------------------
// The line (D-110, D-112, D-114)
// ---------------------------------------------------------------------------

/// `--exclude-libs`' archives (D-110 item 1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExcludeLibs {
    /// `ALL`: every archive.
    pub all: bool,
    /// The archives named, by file name (`libfoo.a`).
    pub names: BTreeSet<String>,
}

impl ExcludeLibs {
    /// Whether a member of the archive at `path` is excluded.
    pub fn excludes(&self, path: &str) -> bool {
        let file = path.rsplit_once('/').map_or(path, |(_, f)| f);
        self.all || self.names.contains(file)
    }

    /// One `--exclude-libs` value (D-110 item 1): `,` and `:` separate
    /// entries; `ALL` exactly names every archive; any other entry is a
    /// file name (an entry with a directory, or `all`, names nothing,
    /// since no archive's base name is one).
    pub fn add(&mut self, v: &str) {
        for n in v.split([',', ':']).filter(|n| !n.is_empty()) {
            if n == "ALL" {
                self.all = true;
            } else {
                self.names.insert(n.to_string());
            }
        }
    }

    /// Whether the line names any archive.
    pub fn any(&self) -> bool {
        self.all || !self.names.is_empty()
    }
}

/// What a `-shared` line says about the export and preemption sets, read
/// from its words by the verifier (§12.5: "recomputed from ... the map's
/// record of the line").
#[derive(Debug, Clone, Default)]
pub struct SharedLine {
    /// The last of `-Bsymbolic`, `-Bsymbolic-functions`, `-Bno-symbolic`.
    pub symbolic: Option<Symbolic>,
    /// `--dynamic-list` files, in line order.
    pub dynamic_lists: Vec<String>,
    /// `--version-script`/`-version-script` files, in line order.
    pub version_scripts: Vec<String>,
    pub exclude_libs: ExcludeLibs,
    /// `-soname`/`-h`, the last.
    pub soname: Option<String>,
    /// `-o`/`--output`, the last, as written (D-115's base name without
    /// `-soname`; lld writes it as given).
    pub output: Option<String>,
    /// `-E`/`--export-dynamic` (rule 12 only, D-114).
    pub export_dynamic: bool,
    /// `-static-pie` is on the line (P-D16: then not `Shared`).
    pub static_pie: bool,
    /// Forms D-112 refuses by name (each a P-D1 finding).
    pub outside: Vec<String>,
    /// The `-z` keywords of the line, in order (D-103's flags, P-D22).
    pub z: Vec<String>,
    /// `-z defs`/`--no-undefined` (D-118: an import nothing defines is
    /// then refused; without it the host resolves it, P-D17 view A).
    pub zdefs: bool,
}

impl SharedLine {
    /// The symbolic form D-112 resolves: the last form, `None` without one.
    pub fn form(&self) -> Symbolic {
        self.symbolic.unwrap_or(Symbolic::None)
    }

    /// Whether `DT_SYMBOLIC`/`DF_SYMBOLIC` belong in the output (P-D22).
    pub fn df_symbolic(&self) -> bool {
        self.form() == Symbolic::All && self.dynamic_lists.is_empty()
    }

    /// The map's record of these facts (§3.2).
    pub fn records(&self) -> SharedRecords {
        SharedRecords {
            symbolic: Some(self.form()),
            dynamic_lists: self.dynamic_lists.clone(),
            version_scripts: self.version_scripts.clone(),
        }
    }

    /// Reads the words of a link (without the program name).
    #[allow(clippy::too_many_lines)] // one arm per option
    /// `symbolic` is [`Line::symbolic`], the one reader of the
    /// `-Bsymbolic` family (the last form; `None` with none or after
    /// `-Bno-symbolic`).
    pub fn read(args: &[String], symbolic: Option<&str>) -> SharedLine {
        let mut l = SharedLine::default();
        l.symbolic = match symbolic {
            None => None,
            Some("-Bsymbolic") => Some(Symbolic::All),
            Some("-Bsymbolic-functions") => Some(Symbolic::Functions),
            Some(other) => {
                l.outside.push(format!("{other} (dyn.md D-112: refused by name)"));
                None
            }
        };
        let mut i = 0;
        let val = |a: &str, opts: &[&str]| -> Option<String> {
            opts.iter()
                .find_map(|o| a.strip_prefix(o).and_then(|r| r.strip_prefix('=')))
                .map(str::to_string)
        };
        while i < args.len() {
            let a = args[i].as_str();
            let mut next = || {
                i += 1;
                args.get(i).cloned()
            };
            match a {
                "--dynamic-list-data"
                | "-dynamic-list-data"
                | "--dynamic-list-cpp-new"
                | "-dynamic-list-cpp-new"
                | "--dynamic-list-cpp-typeinfo"
                | "-dynamic-list-cpp-typeinfo" => {
                    l.outside.push(format!("{a} (dyn.md D-112: refused by name)"));
                }
                "--dynamic-list" | "-dynamic-list" => {
                    if let Some(f) = next() {
                        l.dynamic_lists.push(f);
                    }
                }
                "--version-script" | "-version-script" => {
                    if let Some(f) = next() {
                        l.version_scripts.push(f);
                    }
                }
                "--exclude-libs" | "-exclude-libs" => {
                    if let Some(v) = next() {
                        l.exclude(&v);
                    }
                }
                "-soname" | "--soname" | "-h" => l.soname = next(),
                "-o" | "--output" => l.output = next(),
                "-E" | "--export-dynamic" | "-export-dynamic" => l.export_dynamic = true,
                "--no-export-dynamic" => l.export_dynamic = false,
                "-static-pie" | "--static-pie" => l.static_pie = true,
                "-z" => {
                    if let Some(z) = next() {
                        l.zword(&z);
                    }
                }
                "--no-undefined" | "-no-undefined" => l.zdefs = true,
                // Options whose next word is a value the loop must skip.
                "-m"
                | "-e"
                | "--entry"
                | "-u"
                | "--undefined"
                | "-T"
                | "--script"
                | "-Map"
                | "-plugin"
                | "--plugin"
                | "-plugin-opt"
                | "-y"
                | "--trace-symbol"
                | "--defsym"
                | "-Y"
                | "-A"
                | "-F"
                | "-f"
                | "-G"
                | "--sysroot"
                | "--wrap"
                | "--image-base"
                | "-Ttext"
                | "-Tdata"
                | "-Tbss"
                | "--section-start"
                | "--thread-count"
                | "-mllvm"
                | "--chroot"
                | "--hash-style"
                | "-L"
                | "--library-path"
                | "-l"
                | "--library"
                | "-rpath"
                | "-R"
                | "--rpath"
                | "-rpath-link"
                | "--rpath-link"
                | "-dynamic-linker"
                | "--dynamic-linker"
                | "-I"
                | "-init"
                | "--init"
                | "-fini"
                | "--fini"
                | "--export-dynamic-symbol"
                | "-export-dynamic-symbol"
                | "--export-dynamic-symbol-list"
                | "-export-dynamic-symbol-list"
                | "--odr-table"
                | "-odr-table"
                | "--odr"
                | "-odr" => i += 1,
                _ => {
                    if let Some(f) = val(a, &["--dynamic-list", "-dynamic-list"]) {
                        l.dynamic_lists.push(f);
                    } else if let Some(f) = val(a, &["--version-script", "-version-script"]) {
                        l.version_scripts.push(f);
                    } else if let Some(v) = val(a, &["--exclude-libs", "-exclude-libs"]) {
                        l.exclude(&v);
                    } else if let Some(s) = val(a, &["-soname", "--soname"]) {
                        l.soname = Some(s);
                    } else if let Some(o) = val(a, &["--output"]) {
                        l.output = Some(o);
                    } else if let Some(o) = a.strip_prefix("-o").filter(|o| !o.is_empty()) {
                        l.output = Some(o.to_string());
                    } else if let Some(s) = a.strip_prefix("-h").filter(|s| !s.is_empty()) {
                        l.soname = Some(s.to_string());
                    } else if let Some(z) = a.strip_prefix("-z").filter(|z| !z.is_empty()) {
                        l.zword(z);
                    }
                }
            }
            i += 1;
        }
        l
    }

    fn zword(&mut self, z: &str) {
        match z {
            "defs" => self.zdefs = true,
            "undefs" => self.zdefs = false,
            _ => {}
        }
        self.z.push(z.to_string());
    }

    /// `DT_FLAGS_1`'s bits the line's `-z` words ask for (D-103):
    /// `nodelete`, `nodlopen`, `initfirst`, `interpose`, `origin`; `now`
    /// (the last of `now`/`lazy`).
    pub fn flags_1(&self) -> u64 {
        let mut f = 0;
        for z in &self.z {
            match z.as_str() {
                "now" => f |= crate::dynout::DF_1_NOW,
                "lazy" => f &= !crate::dynout::DF_1_NOW,
                "nodelete" => f |= DF_1_NODELETE,
                "nodlopen" => f |= DF_1_NOOPEN,
                "initfirst" => f |= DF_1_INITFIRST,
                "interpose" => f |= DF_1_INTERPOSE,
                "origin" => f |= DF_1_ORIGIN,
                _ => {}
            }
        }
        f
    }

    fn exclude(&mut self, v: &str) {
        self.exclude_libs.add(v);
    }
}

/// P-D16's verdict on the kind, and the line's facts when it is `Shared`.
#[derive(Debug, Default)]
pub struct Accepted {
    pub findings: Vec<Finding>,
    /// `Some` when the line is a `-shared` line judged as `Shared`.
    pub line: Option<SharedLine>,
}

/// P-D16: a `-shared` line (and no `-static-pie`) is judged as `Shared`;
/// the headers must be a shared object's (`ET_DYN`, no `PT_INTERP`, no
/// `DF_1_PIE`; D-100). `line` loses its P-D1 "outside" entry for
/// `-shared` and gains the kind; the forms D-112 refuses take its place.
pub fn accept(image: &Image<'_>, out: &DynOut, line: &mut Line, args: &[String]) -> Accepted {
    let mut acc = Accepted::default();
    if !line.outside.iter().any(|o| o == SHARED_OUTSIDE) {
        return acc;
    }
    let sl = SharedLine::read(args, line.symbolic);
    if sl.static_pie {
        // gcc drops `-shared` before `-static-pie`; a line with both stays
        // outside stage 3, as before.
        return acc;
    }
    line.outside.retain(|o| o != SHARED_OUTSIDE);
    line.outside.extend(sl.outside.iter().cloned());
    line.kind = Some(OutputKind::Shared);
    let interp = image.phdrs.iter().any(|p| p.p_type == PT_INTERP);
    let pie = out.get(DT_FLAGS_1).is_some_and(|f| f & DF_1_PIE != 0);
    if image.header.e_type != ET_DYN || interp || pie {
        acc.findings.push(fd(
            16,
            format!(
                "the command line is -shared; the headers (e_type {}, PT_INTERP {interp}, DF_1_PIE {pie}) are not a shared object's (dyn.md D-100)",
                image.header.e_type
            ),
        ));
    }
    acc.line = Some(sl);
    acc
}

// ---------------------------------------------------------------------------
// Version scripts and dynamic lists (script.md §9.1; resolve.md §9.7)
// ---------------------------------------------------------------------------

/// One entry of a node or a dynamic list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The pattern as written (a quoted one without its quotes).
    pub text: String,
    pub quoted: bool,
    /// Under `global:` (or before any scope word, or in a dynamic list).
    pub global: bool,
}

impl Entry {
    /// resolve §9.7 "Entries": unquoted with an unescaped `*`, `?` or `[`.
    pub fn is_glob(&self) -> bool {
        if self.quoted {
            return false;
        }
        let b = self.text.as_bytes();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'\\' => i += 2,
                b'*' | b'?' | b'[' => return true,
                _ => i += 1,
            }
        }
        false
    }

    /// The star: the glob `*` alone.
    pub fn is_star(&self) -> bool {
        !self.quoted && self.text == "*"
    }

    /// An exact entry's name: a quoted entry verbatim; an unquoted one
    /// with every `\c` replaced by `c`, a final lone `\` kept.
    pub fn exact_name(&self) -> String {
        if self.quoted {
            return self.text.clone();
        }
        let b = self.text.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 2;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// Whether the entry matches `name`.
    pub fn matches(&self, name: &str) -> bool {
        if self.is_glob() {
            fnmatch(self.text.as_bytes(), name.as_bytes())
        } else {
            self.exact_name() == name
        }
    }
}

/// `fnmatch(3)` without flags, as resolve §9.7 "Entries" states it.
pub fn fnmatch(p: &[u8], s: &[u8]) -> bool {
    let Some((&c, rest)) = p.split_first() else { return s.is_empty() };
    match c {
        b'*' => (0..=s.len()).any(|k| fnmatch(rest, &s[k..])),
        b'?' => !s.is_empty() && fnmatch(rest, &s[1..]),
        b'\\' => match rest.split_first() {
            // A glob ending in a lone `\` matches nothing.
            None => false,
            Some((&e, r2)) => s.first() == Some(&e) && fnmatch(r2, &s[1..]),
        },
        b'[' => match class(rest) {
            Some((set, after)) => s.first().is_some_and(|&x| set(x)) && fnmatch(after, &s[1..]),
            // No closing `]`: the character `[`.
            None => s.first() == Some(&b'[') && fnmatch(rest, &s[1..]),
        },
        _ => s.first() == Some(&c) && fnmatch(rest, &s[1..]),
    }
}

/// A bracket class after its `[`: the membership test and the pattern
/// after its `]`, or `None` when no `]` closes it.
#[allow(clippy::type_complexity)]
fn class(p: &[u8]) -> Option<(Box<dyn Fn(u8) -> bool + '_>, &[u8])> {
    let mut i = 0;
    let neg = matches!(p.first(), Some(b'!' | b'^'));
    if neg {
        i += 1;
    }
    let start = i;
    // The members, as (lo, hi) ranges.
    let mut items: Vec<(u8, u8)> = Vec::new();
    loop {
        let &c = p.get(i)?;
        if c == b']' && i > start {
            break;
        }
        let (lo, n) = if c == b'\\' { (*p.get(i + 1)?, 2) } else { (c, 1) };
        i += n;
        // A range `a-b`, unless the `-` is last.
        if p.get(i) == Some(&b'-') && p.get(i + 1).is_some_and(|&x| x != b']') {
            let h = p[i + 1];
            let (hi, m) = if h == b'\\' { (*p.get(i + 2)?, 3) } else { (h, 2) };
            i += m;
            items.push((lo, hi));
        } else {
            items.push((lo, lo));
        }
    }
    let rest = &p[i + 1..];
    Some((Box::new(move |x: u8| items.iter().any(|&(l, h)| l <= x && x <= h) != neg), rest))
}

/// A version node: `None` names the anonymous node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: Option<String>,
    pub entries: Vec<Entry>,
    pub parents: Vec<String>,
}

/// A version script: its nodes in order (several files read as one).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Script {
    pub nodes: Vec<Node>,
}

/// What resolve §9.7 decides for a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Node `i` of the script (an anonymous node: exported unversioned).
    Node(usize),
    Local,
    /// Matched by no entry: the base version (rule 7).
    Base,
}

/// Why a script or list cannot be judged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptError {
    /// A syntax error (`version-script-syntax`), with the file and line.
    Syntax(String),
    /// A form the spec refuses by name (`extern "C++"`).
    Refused(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Name(String),
    Str(String),
    Open,
    Close,
    Semi,
    Colon,
}

fn lex(file: &str, text: &str) -> Result<Vec<(Tok, u32)>, ScriptError> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1u32;
    let err = |line: u32, m: &str| ScriptError::Syntax(format!("{file}:{line}: {m}"));
    while i < b.len() {
        let c = b[i];
        match c {
            b'\n' => {
                line += 1;
                i += 1;
            }
            b' ' | b'\t' | b'\r' | b'\x0c' | b'\x0b' => i += 1,
            b'#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                loop {
                    if i + 1 >= b.len() {
                        return Err(err(line, "unterminated comment"));
                    }
                    if b[i] == b'*' && b[i + 1] == b'/' {
                        i += 2;
                        break;
                    }
                    if b[i] == b'\n' {
                        line += 1;
                    }
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                return Err(err(line, "'//' is not a comment (script.md §9.1)"));
            }
            b'{' => {
                out.push((Tok::Open, line));
                i += 1;
            }
            b'}' => {
                out.push((Tok::Close, line));
                i += 1;
            }
            b';' => {
                out.push((Tok::Semi, line));
                i += 1;
            }
            b':' => {
                out.push((Tok::Colon, line));
                i += 1;
            }
            b'"' => {
                let s = i + 1;
                let Some(e) = b[s..].iter().position(|&x| x == b'"') else {
                    return Err(err(line, "unterminated string"));
                };
                let t = String::from_utf8_lossy(&b[s..s + e]).into_owned();
                line += t.matches('\n').count() as u32;
                out.push((Tok::Str(t), line));
                i = s + e + 1;
            }
            _ => {
                let s = i;
                while i < b.len()
                    && !matches!(
                        b[i],
                        b' ' | b'\t' | b'\r' | b'\n' | b'{' | b'}' | b';' | b':' | b'"'
                    )
                    && b[i] != b'#'
                {
                    i += 1;
                }
                out.push((Tok::Name(String::from_utf8_lossy(&b[s..i]).into_owned()), line));
            }
        }
    }
    Ok(out)
}

struct Parser<'t> {
    t: &'t [(Tok, u32)],
    i: usize,
    file: &'t str,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i).map(|x| &x.0)
    }
    fn err(&self, m: &str) -> ScriptError {
        let line = self.t.get(self.i).or(self.t.last()).map_or(0, |x| x.1);
        ScriptError::Syntax(format!("{}:{line}: syntax error in VERSION script: {m}", self.file))
    }
    fn eat(&mut self, t: &Tok) -> Result<(), ScriptError> {
        if self.peek() == Some(t) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.err(&format!("expected {t:?}")))
        }
    }

    /// `body`, up to and including its closing `}`.
    fn body(&mut self, list: bool) -> Result<Vec<Entry>, ScriptError> {
        let mut v = Vec::new();
        let mut global = true;
        loop {
            match self.peek().cloned() {
                Some(Tok::Close) => {
                    self.i += 1;
                    return Ok(v);
                }
                Some(Tok::Name(n))
                    if !list
                        && (n == "global" || n == "local")
                        && self.t.get(self.i + 1).map(|x| &x.0) == Some(&Tok::Colon) =>
                {
                    global = n == "global";
                    self.i += 2;
                }
                Some(Tok::Name(n))
                    if n == "extern"
                        && matches!(self.t.get(self.i + 1).map(|x| &x.0), Some(Tok::Str(_))) =>
                {
                    let Some(Tok::Str(lang)) = self.t.get(self.i + 1).map(|x| x.0.clone()) else {
                        unreachable!()
                    };
                    self.i += 2;
                    match lang.as_str() {
                        "C" => {}
                        "C++" => {
                            return Err(ScriptError::Refused(format!(
                                "{}: extern \"C++\" needs the demangler (resolve.md §9.7 rule 6)",
                                self.file
                            )));
                        }
                        _ => return Err(self.err(&format!("unknown language \"{lang}\""))),
                    }
                    self.eat(&Tok::Open)?;
                    loop {
                        match self.peek().cloned() {
                            Some(Tok::Close) => {
                                self.i += 1;
                                break;
                            }
                            Some(Tok::Name(n)) => {
                                self.i += 1;
                                v.push(Entry { text: n, quoted: false, global });
                            }
                            Some(Tok::Str(s)) => {
                                self.i += 1;
                                v.push(Entry { text: s, quoted: true, global });
                            }
                            Some(Tok::Semi) => self.i += 1,
                            _ => return Err(self.err("in extern block")),
                        }
                    }
                    self.eat(&Tok::Semi)?;
                }
                Some(Tok::Name(n)) => {
                    self.i += 1;
                    self.eat(&Tok::Semi)?;
                    v.push(Entry { text: n, quoted: false, global });
                }
                Some(Tok::Str(s)) => {
                    self.i += 1;
                    self.eat(&Tok::Semi)?;
                    v.push(Entry { text: s, quoted: true, global });
                }
                _ => return Err(self.err("in a node's body")),
            }
        }
    }
}

impl Script {
    /// Reads the texts of the line's `--version-script` files, in order,
    /// as one script (resolve §9.7 rule 13), with script.md §9.1's
    /// refusals.
    pub fn parse(files: &[(String, String)]) -> Result<Script, ScriptError> {
        let mut s = Script::default();
        for (file, text) in files {
            let toks = lex(file, text)?;
            let mut p = Parser { t: &toks, i: 0, file };
            while p.peek().is_some() {
                let name = match p.peek().cloned() {
                    Some(Tok::Open) => None,
                    Some(Tok::Name(n)) => {
                        p.i += 1;
                        Some(n)
                    }
                    _ => return Err(p.err("expected a version node")),
                };
                p.eat(&Tok::Open)?;
                let entries = p.body(false)?;
                let mut parents = Vec::new();
                if name.is_some() {
                    while let Some(Tok::Name(n)) = p.peek().cloned() {
                        p.i += 1;
                        parents.push(n);
                    }
                }
                p.eat(&Tok::Semi)?;
                if let Some(n) = &name {
                    if s.nodes.iter().any(|x| x.name.as_ref() == Some(n)) {
                        return Err(ScriptError::Syntax(format!(
                            "{file}: duplicate version tag `{n}'"
                        )));
                    }
                    for par in &parents {
                        if !s.nodes.iter().any(|x| x.name.as_ref() == Some(par)) {
                            return Err(ScriptError::Syntax(format!(
                                "{file}: unable to find version dependency `{par}'"
                            )));
                        }
                    }
                }
                s.nodes.push(Node { name, entries, parents });
            }
        }
        let anon = s.nodes.iter().filter(|n| n.name.is_none()).count();
        if anon > 0 && s.nodes.len() > 1 {
            return Err(ScriptError::Syntax(
                "anonymous version tag cannot be combined with other version tags".into(),
            ));
        }
        // duplicate-version-pattern: the same entry under `global:` in one
        // node and `local:` in another.
        for (a, na) in s.nodes.iter().enumerate() {
            for (b, nb) in s.nodes.iter().enumerate() {
                if a == b {
                    continue;
                }
                for x in na.entries.iter().filter(|e| e.global) {
                    for y in nb.entries.iter().filter(|e| !e.global) {
                        let same = match (x.is_glob(), y.is_glob()) {
                            (false, false) => x.exact_name() == y.exact_name(),
                            (true, true) => x.text == y.text,
                            _ => false,
                        };
                        if same {
                            return Err(ScriptError::Refused(format!(
                                "duplicate expression `{}' in version information",
                                x.text
                            )));
                        }
                    }
                }
            }
        }
        Ok(s)
    }

    /// The node named `v`.
    pub fn node(&self, v: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.name.as_deref() == Some(v))
    }

    /// resolve §9.7 "The decision" for an unversioned name.
    pub fn decide(&self, name: &str) -> Decision {
        let (mut g, mut l, mut gs, mut ls) = (None, false, None, false);
        for (i, n) in self.nodes.iter().enumerate() {
            for global in [true, false] {
                for e in n.entries.iter().filter(|e| e.global == global) {
                    if !e.is_glob() {
                        if e.exact_name() == name {
                            return if global { Decision::Node(i) } else { Decision::Local };
                        }
                    } else if fnmatch(e.text.as_bytes(), name.as_bytes()) {
                        match (global, e.is_star()) {
                            (true, false) => g = Some(i),
                            (false, false) => l = true,
                            (true, true) => gs = Some(i),
                            (false, true) => ls = true,
                        }
                    }
                }
            }
        }
        if let Some(i) = g {
            Decision::Node(i)
        } else if l {
            Decision::Local
        } else if let Some(i) = gs {
            Decision::Node(i)
        } else if ls {
            Decision::Local
        } else {
            Decision::Base
        }
    }

    /// Rule 12: a `.symver` definition `name@v`/`name@@v`. `None` when the
    /// script has no node `v` (refused, `undefined-version`).
    pub fn decide_versioned(&self, name: &str, v: &str, export_dynamic: bool) -> Option<Decision> {
        let i = self.node(v)?;
        let n = &self.nodes[i];
        let local = n.entries.iter().any(|e| !e.global && e.matches(name))
            && !n.entries.iter().any(|e| e.global && e.matches(name))
            && !export_dynamic;
        Some(if local { Decision::Local } else { Decision::Node(i) })
    }

    /// Rule 12 as `format`'s linker applies it: mortar and GNU ld by
    /// [`Script::decide_versioned`]; the references' recorded differences
    /// (`spec/dyn.md` 17.8, `exp3/vs3/expdyn.py`, `atat.py`): mold never
    /// hides a `.symver` definition; lld hides `foo@V` when one of V's
    /// `local:` entries matches `foo` and no exact `global:` entry of V
    /// names it (a `global:` glob does not keep it), with or without
    /// `--export-dynamic`, and hides `foo@@V` only for an exact `local:`
    /// entry of V naming it (WP-149).
    pub fn decide_versioned_as(
        &self,
        name: &str,
        v: &str,
        default: bool,
        export_dynamic: bool,
        format: Format,
    ) -> Option<Decision> {
        let i = self.node(v)?;
        let n = &self.nodes[i];
        let exact = |e: &&Entry| !e.is_glob() && e.exact_name() == name;
        let local = match format {
            Format::Mortar | Format::Gnu => return self.decide_versioned(name, v, export_dynamic),
            Format::Mold => false,
            Format::Lld if default => {
                n.entries.iter().filter(|e| !e.global).any(|e| exact(&e))
                    && !n.entries.iter().filter(|e| e.global).any(|e| exact(&e))
            }
            Format::Lld => {
                n.entries.iter().any(|e| !e.global && e.matches(name))
                    && !n.entries.iter().filter(|e| e.global).any(|e| exact(&e))
            }
        };
        Some(if local { Decision::Local } else { Decision::Node(i) })
    }
}

/// A dynamic list: its entries, every file as one list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DynList {
    pub entries: Vec<Entry>,
}

impl DynList {
    pub fn parse(files: &[(String, String)]) -> Result<DynList, ScriptError> {
        let mut l = DynList::default();
        for (file, text) in files {
            let toks = lex(file, text)?;
            let mut p = Parser { t: &toks, i: 0, file };
            while p.peek().is_some() {
                p.eat(&Tok::Open)?;
                l.entries.extend(p.body(true)?);
                p.eat(&Tok::Semi)?;
            }
        }
        Ok(l)
    }

    pub fn lists(&self, name: &str) -> bool {
        self.entries.iter().any(|e| e.matches(name))
    }
}

// ---------------------------------------------------------------------------
// The export set (D-110), imports (D-111), preemption (D-112)
// ---------------------------------------------------------------------------

/// The version an export gets (D-115).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum VerOut {
    /// Index 1: no script, an anonymous node, or matched by no node.
    Base,
    /// A node's index (`foo`, `foo@@V`).
    Node(String),
    /// A node's index with `VERSYM_HIDDEN` (`foo@V`).
    Hidden(String),
}

/// One key of the export set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    pub name: String,
    pub version: VerOut,
    pub ty: u8,
    pub bind: u8,
    pub vis: u8,
    pub size: u64,
    /// Bound by D-112 (the verifier's computation, never the output's).
    pub preemptible: bool,
    /// A reference may leave it out of `.dynsym` (mold's `__start_`/
    /// `__stop_`, D-110 item 2: a difference, not a wrong output).
    pub optional: bool,
}

/// An import (D-111): a key a live object references that no live object
/// defines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// Every reference is weak.
    pub weak: bool,
    /// The type of the references (`STT_TLS`, `STT_OBJECT`, `STT_FUNC`,
    /// `STT_NOTYPE`).
    pub ty: u8,
    /// Some reference names a version (`foo@V`, a `.symver` reference):
    /// view A owes its definition (W42), never the host's (D-118).
    pub versioned: bool,
}

/// The one derivation of a key's export (D-110, D-114, D-115; resolve
/// §9.7, rule 12 per reference): whether a definition named `key` (`foo`,
/// `foo@V`, `foo@@V`) of default or protected visibility (merged by base
/// name; the caller asks) is exported, and with which version. P-D19's
/// export set, P-1's placeless keys and M10's roots (`fate.rs`, WP-147)
/// all ask it (WP-149). `--exclude-libs` enters it as the keys whose
/// definition it excludes (`excluded`, D-110 item 1: exported by nothing),
/// and an executable's `--dynamic-list` without `-E` as the keys it lists
/// (`only`, resolve §9.6; WP-161); P-D19 still reads the exclusion per
/// object for the chosen definition.
#[derive(Debug, Clone)]
pub struct ExportRule {
    pub script: Option<Script>,
    /// `-E` (lld's rule 12 ignores it; GNU ld's and mortar's do not).
    pub export_dynamic: bool,
    pub format: Format,
    /// Base names whose definition `--exclude-libs` excludes: never
    /// exported (D-110 item 1; WP-161).
    pub excluded: BTreeSet<String>,
    /// An executable's exports when not every definition is one (no
    /// `-E`): the keys its `--dynamic-list` lists (resolve §9.6; WP-161).
    pub only: Option<BTreeSet<String>>,
}

impl ExportRule {
    /// No version script (an executable with `-E`, a library without
    /// one): every key without a version is exported at the base.
    pub fn plain(format: Format) -> ExportRule {
        ExportRule {
            script: None,
            export_dynamic: false,
            format,
            excluded: BTreeSet::new(),
            only: None,
        }
    }

    /// An executable's rule (WP-161): `-E` exports every key, else only
    /// the keys `listed` (its `--dynamic-list`); `None` when it exports
    /// nothing. `excluded` are never exported (D-110 item 1).
    pub fn executable(
        format: Format,
        export_dynamic: bool,
        listed: BTreeSet<String>,
        excluded: BTreeSet<String>,
    ) -> Option<ExportRule> {
        if !export_dynamic && listed.is_empty() {
            return None;
        }
        let only = (!export_dynamic).then_some(listed);
        Some(ExportRule { excluded, only, ..ExportRule::plain(format) })
    }

    /// `Ok(None)`: the key is local. `Err`: the link is outside (D-114:
    /// undefined-version), with P-D1's text.
    pub fn version(&self, key: &str) -> Result<Option<VerOut>, String> {
        let (base, ver) = split_version(key);
        match (&self.script, ver) {
            (None, None) => Ok(Some(VerOut::Base)),
            (None, Some((v, _))) => Err(format!(
                "'{key}': version {v} and no version script (dyn.md D-114: undefined-version)"
            )),
            (Some(s), None) => Ok(match s.decide(base) {
                Decision::Local => None,
                Decision::Base => Some(VerOut::Base),
                Decision::Node(i) => {
                    Some(s.nodes[i].name.clone().map_or(VerOut::Base, VerOut::Node))
                }
            }),
            (Some(s), Some((v, default))) => {
                match s.decide_versioned_as(base, v, default, self.export_dynamic, self.format) {
                    None => Err(format!(
                        "'{key}': no node {v} in the version script (dyn.md D-114: undefined-version)"
                    )),
                    Some(Decision::Local) => Ok(None),
                    Some(_) => Ok(Some(if default {
                        VerOut::Node(v.to_string())
                    } else {
                        VerOut::Hidden(v.to_string())
                    })),
                }
            }
        }
    }

    /// Whether `key` is exported (an `Err` is not: the link is outside).
    pub fn exports(&self, key: &str) -> bool {
        let base = split_version(key).0;
        !self.excluded.contains(base)
            && self.only.as_ref().is_none_or(|o| o.contains(base))
            && matches!(self.version(key), Ok(Some(_)))
    }
}

/// The sets of §12.5, recomputed from the inputs and the line.
#[derive(Debug, Default)]
pub struct Sets {
    /// By name: an unversioned key and its `.symver` forms may share one.
    pub exports: BTreeMap<String, Vec<Export>>,
    /// Keys a live object defines that a version script or
    /// `--exclude-libs` made local (bound, not exported).
    pub local: BTreeSet<String>,
    pub imports: BTreeMap<String, Import>,
    /// Node names of the script, by `Verdef` index order (index 2 first).
    pub nodes: Vec<String>,
    /// Each node's parents as the script writes them, in `nodes`' order.
    pub parents: Vec<Vec<String>>,
    /// An input the verifier cannot read (P-D2).
    pub problems: Vec<String>,
    /// A form refused by name (P-D1).
    pub outside: Vec<String>,
    /// `__start_X`/`__stop_X` a live section's relocation names, with `X`
    /// a section of the inputs that the map keeps none of (layout L-96
    /// keeps them all live; W23): the key and `X`.
    pub dropped: Vec<(String, String)>,
    /// Keys whose definition has no place (`spec/resolve.md` §10.1 P-1,
    /// §4.4 step 0's predicate `Obj::no_place`): never in the export set
    /// (D-110; WP-147), by name with the definition's input `st_value`.
    pub placeless: BTreeMap<String, u64>,
    /// The one derivation of the export set (WP-149): what P-D19's set,
    /// P-1's placeless keys and M10's roots ask.
    pub rule: Option<ExportRule>,
}

impl Sets {
    /// Whether `name` is an export D-112 makes preemptible.
    pub fn preemptible(&self, name: &str) -> bool {
        self.exports.get(name).is_some_and(|v| v.iter().any(|e| e.preemptible))
    }

    /// Whether `name` is exported and bound.
    pub fn bound(&self, name: &str) -> bool {
        self.exports.get(name).is_some_and(|v| v.iter().all(|e| !e.preemptible))
    }
}

/// gABI's order of constraint: default < protected < hidden < internal.
fn rank(vis: u8) -> u8 {
    match vis {
        STV_DEFAULT => 0,
        STV_PROTECTED => 1,
        2 => 2,
        _ => 3,
    }
}

/// `foo@@V` -> (`foo`, `V`, default), `foo@V` -> (`foo`, `V`, not).
pub fn split_version(name: &str) -> (&str, Option<(&str, bool)>) {
    match name.split_once('@') {
        None => (name, None),
        Some((n, v)) => match v.strip_prefix('@') {
            Some(v) => (n, Some((v, true))),
            None => (n, Some((v, false))),
        },
    }
}

/// D-112's binding of an exported, non-protected, version-global key, as
/// the line's form and list decide; `format` admits GNU ld's own forms
/// (it binds `NOTYPE` under `-Bsymbolic-functions`, and keeps `TLS` and
/// `GNU_IFUNC` preemptible under a list: record §F.2).
pub fn line_preempts(
    sl: &SharedLine,
    list: Option<&DynList>,
    ty: u8,
    name: &str,
    format: Format,
) -> bool {
    if let Some(l) = list {
        return l.lists(name) || (format == Format::Gnu && matches!(ty, STT_TLS | STT_GNU_IFUNC));
    }
    match sl.form() {
        Symbolic::All => false,
        Symbolic::Functions => !(ty == STT_FUNC || (format == Format::Gnu && ty == STT_NOTYPE)),
        Symbolic::None => true,
    }
}

/// A C identifier (`__start_`/`__stop_` sections, layout L-96).
fn c_ident(s: &str) -> bool {
    !s.is_empty()
        && !s.as_bytes()[0].is_ascii_digit()
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// Layout L-96 (W23): each `__start_X`/`__stop_X` that a relocation of a
/// live input section names, where `X` is a C identifier and an allocated
/// section of some input: the link defines it, and every input section
/// `X` is live. Maps the key to `X` and whether the map keeps one of them.
/// Only objects whose symbol table references such a name are read for
/// their relocations (§8.1).
fn start_stop(
    objs: &[Obj<'_>],
    refs: &BTreeMap<String, Import>,
) -> BTreeMap<String, (String, bool)> {
    let wanted: BTreeMap<&str, &str> = refs
        .keys()
        .filter_map(|n| {
            let sec = n.strip_prefix("__start_").or_else(|| n.strip_prefix("__stop_"))?;
            c_ident(sec).then_some((n.as_str(), sec))
        })
        .collect();
    let mut secs: BTreeMap<&str, bool> = BTreeMap::new();
    if !wanted.is_empty() {
        for o in objs {
            for s in &o.secs {
                if s.alloc() && wanted.values().any(|w| *w == s.name) {
                    let kept = !matches!(s.place, Place::Absent | Place::Discarded);
                    *secs.entry(s.name.as_str()).or_default() |= kept;
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    if secs.is_empty() {
        return out;
    }
    for o in objs {
        let Some(t) = &o.symtab else { continue };
        let named: BTreeSet<u32> = t
            .globals()
            .filter(|(_, s)| s.st_shndx == 0)
            .filter_map(|(i, s)| {
                let n = String::from_utf8_lossy(t.name(&s).ok()?).into_owned();
                let sec = wanted.get(n.as_str())?;
                secs.contains_key(sec).then_some(i.0)
            })
            .collect();
        if named.is_empty() {
            continue;
        }
        for s in &o.secs {
            if matches!(s.place, Place::Absent | Place::Discarded) {
                continue;
            }
            let Some(recs) = s.relocs.and_then(|rs| o.records(rs)) else { continue };
            for (_, sym, _, _) in recs {
                if !named.contains(&sym) {
                    continue;
                }
                let Ok(z) = t.get(mortar_elf::SymbolIndex(sym)) else { continue };
                let Ok(n) = t.name(&z) else { continue };
                let n = String::from_utf8_lossy(n).into_owned();
                if let Some(sec) = wanted.get(n.as_str()) {
                    let kept = secs.get(sec).copied().unwrap_or(false);
                    out.insert(n, ((*sec).to_string(), kept));
                }
            }
        }
    }
    out
}

/// Reads the line's files (version scripts or dynamic lists).
fn read_files(
    paths: &[String],
    files: &dyn Files,
    sets: &mut Sets,
) -> Option<Vec<(String, String)>> {
    let mut v = Vec::new();
    for p in paths {
        let Some(b) = files.read(p) else {
            sets.problems.push(format!("{p}: cannot be read (a file of the -shared line)"));
            return None;
        };
        v.push((p.clone(), String::from_utf8_lossy(&b).into_owned()));
    }
    Some(v)
}

/// The export set, the local keys and the imports of a `Shared` link,
/// from the live inputs, the output's linker-defined symbols and the
/// line (D-110, D-111, D-112, D-114, D-115).
#[allow(clippy::too_many_lines)] // D-110's items in order
pub(crate) fn compute(
    objs: &[Obj<'_>],
    image: &Image<'_>,
    ref_dynamic: &HashSet<String>,
    sl: &SharedLine,
    files: &dyn Files,
    format: Format,
) -> Sets {
    let mut sets = Sets::default();
    let script = match read_files(&sl.version_scripts, files, &mut sets) {
        None => return sets,
        Some(t) if t.is_empty() => None,
        Some(t) => match Script::parse(&t) {
            Ok(s) => Some(s),
            Err(ScriptError::Syntax(m)) => {
                sets.problems.push(format!("a version script the verifier cannot read: {m}"));
                return sets;
            }
            Err(ScriptError::Refused(m)) => {
                sets.outside.push(format!("{m} (dyn.md D-114)"));
                return sets;
            }
        },
    };
    let list = match read_files(&sl.dynamic_lists, files, &mut sets) {
        None => return sets,
        Some(t) if t.is_empty() => None,
        Some(t) => match DynList::parse(&t) {
            Ok(l) => Some(l),
            Err(ScriptError::Syntax(m)) => {
                sets.problems.push(format!("a dynamic list the verifier cannot read: {m}"));
                return sets;
            }
            Err(ScriptError::Refused(m)) => {
                sets.outside.push(format!("{m} (dyn.md D-112)"));
                return sets;
            }
        },
    };
    if let Some(s) = &script {
        sets.nodes = s.nodes.iter().filter_map(|n| n.name.clone()).collect();
        sets.parents =
            s.nodes.iter().filter(|n| n.name.is_some()).map(|n| n.parents.clone()).collect();
    }
    // Every mention of a name by a live object: definitions (full names,
    // `.symver` forms kept) and the merged visibility (by base name).
    #[allow(clippy::items_after_statements, clippy::struct_excessive_bools)] // next to its one use
    struct D {
        ty: u8,
        bind: u8,
        size: u64,
        strong: bool,
        common: bool,
        excluded: bool,
        /// P-1: its section has no place (WP-147).
        placeless: bool,
        value: u64,
    }
    let out_eh_frame = image.sections.iter().any(|x| x.name == ".eh_frame" && x.alloc());
    let mut defs: BTreeMap<String, Vec<D>> = BTreeMap::new();
    let mut gone: BTreeMap<String, u64> = BTreeMap::new();
    let mut vis: HashMap<String, u8> = HashMap::new();
    let mut refs: BTreeMap<String, Import> = BTreeMap::new();
    for o in objs {
        let Some(t) = o.symtab else { continue };
        let excluded = o.member.is_some() && sl.exclude_libs.excludes(&o.path);
        for (i, s) in t.globals() {
            if s.binding() == STB_LOCAL {
                continue;
            }
            let Ok(n) = t.name(&s) else { continue };
            let name = String::from_utf8_lossy(n).into_owned();
            let base = split_version(&name).0.to_string();
            let v = vis.entry(base.clone()).or_insert(STV_DEFAULT);
            if rank(s.visibility()) > rank(*v) {
                *v = s.visibility();
            }
            let live = match t.section_of(i, &s) {
                Ok(SymbolSection::Undefined) => {
                    let versioned = split_version(&name).1.is_some();
                    let r = refs.entry(name).or_insert(Import {
                        weak: true,
                        ty: STT_NOTYPE,
                        versioned,
                    });
                    r.weak &= s.binding() == STB_WEAK;
                    if s.sym_type() != STT_NOTYPE {
                        r.ty = s.sym_type();
                    }
                    continue;
                }
                Ok(SymbolSection::Section(k)) => {
                    o.secs.get(k.0 as usize).is_some_and(|x| !matches!(x.place, Place::Discarded))
                }
                Ok(_) => true,
                Err(_) => false,
            };
            if !live {
                // A definition in a discarded section: P-1's "no place"
                // when no live object defines the key (below; WP-147).
                gone.entry(name).or_insert(s.st_value);
                continue;
            }
            let common = matches!(t.section_of(i, &s), Ok(SymbolSection::Common));
            let placeless = matches!(t.section_of(i, &s), Ok(SymbolSection::Section(k))
                if o.no_place(k.0 as usize, out_eh_frame));
            defs.entry(name).or_default().push(D {
                ty: if common { mortar_elf::consts::STT_OBJECT } else { s.sym_type() },
                bind: s.binding(),
                size: s.st_size,
                strong: matches!(s.binding(), STB_GLOBAL | STB_GNU_UNIQUE) && !common,
                common,
                excluded,
                placeless,
                value: s.st_value,
            });
        }
    }
    let defined_base: HashSet<String> =
        defs.keys().map(|n| split_version(n).0.to_string()).collect();
    let mut rule = ExportRule { script, ..ExportRule::plain(format) };
    rule.export_dynamic = sl.export_dynamic;
    // D-110 item 1: the keys whose chosen definition `--exclude-libs`
    // excludes (below), exported by nothing (WP-161).
    let mut excluded: BTreeSet<String> = BTreeSet::new();
    // P-1 (WP-147): a key every definition of which is discarded (not a
    // COMDAT copy another object keeps) has no place, when the one
    // derivation of the export set exports it (WP-149).
    for (name, value) in gone {
        let base = split_version(&name).0;
        let v = vis.get(base).copied().unwrap_or(STV_DEFAULT);
        if !defined_base.contains(base)
            && matches!(v, STV_DEFAULT | STV_PROTECTED)
            && rule.exports(&name)
        {
            sets.placeless.entry(base.to_string()).or_insert(value);
        }
    }
    // Layout L-96: `__start_X`/`__stop_X` named from a live section, `X` a
    // C-identifier section of the inputs, is the link's own definition.
    let startstop = start_stop(objs, &refs);
    for (name, r) in refs {
        let base = split_version(&name).0;
        if defined_base.contains(base) {
            continue;
        }
        let versioned = r.versioned;
        sets.imports.entry(base.to_string()).or_insert(r).versioned |= versioned;
    }
    // A key's version and whether a script hides it: the one derivation
    // (`ExportRule`), which M10's roots ask too (WP-149).
    let decide = |name: &str, sets: &mut Sets| -> Option<VerOut> {
        rule.version(name).unwrap_or_else(|m| {
            sets.outside.push(m);
            None
        })
    };
    for (name, ds) in &defs {
        let base = split_version(name).0.to_string();
        let v = vis.get(&base).copied().unwrap_or(STV_DEFAULT);
        if !matches!(v, STV_DEFAULT | STV_PROTECTED) {
            continue;
        }
        let d = ds
            .iter()
            .find(|d| d.strong)
            .or_else(|| ds.iter().find(|d| !d.common))
            .unwrap_or(&ds[0]);
        if d.excluded {
            excluded.insert(base.clone());
            sets.local.insert(base);
            continue;
        }
        let Some(version) = decide(name, &mut sets) else {
            // A hidden `.symver` definition is the key `foo@V1` (or
            // `foo@@V1`): the plain `foo` may still be another node's
            // export, preemptible (WP-149).
            sets.local.insert(if split_version(name).1.is_some() { name.clone() } else { base });
            continue;
        };
        // P-1: an export whose definition has no place is omitted
        // (`spec/resolve.md` §10.1, D-110; WP-147); a key the script
        // makes local is local first (WP-149).
        if d.placeless {
            sets.placeless.insert(base, d.value);
            continue;
        }
        let preemptible =
            v != STV_PROTECTED && line_preempts(sl, list.as_ref(), d.ty, &base, format);
        sets.exports.entry(base.clone()).or_default().push(Export {
            name: base,
            version,
            ty: d.ty,
            bind: d.bind,
            vis: v,
            size: d.size,
            preemptible,
            optional: false,
        });
    }
    // D-110 item 2: the linker's own definitions (file 0): exported only
    // when a shared input references them, but `__start_`/`__stop_` of a
    // kept C-identifier section a live object references, `PROTECTED`.
    for s in &image.symbols {
        if s.sym.st_shndx == 0
            || !matches!(s.sym.binding(), STB_GLOBAL | STB_WEAK)
            || defined_base.contains(split_version(&s.name).0)
            || sets.exports.contains_key(&s.name)
            || matches!(s.name.as_str(), "_DYNAMIC" | "_GLOBAL_OFFSET_TABLE_")
            || s.sym.sym_type() == STT_SECTION
        {
            continue;
        }
        let startstop = s
            .name
            .strip_prefix("__start_")
            .or_else(|| s.name.strip_prefix("__stop_"))
            .is_some_and(|sec| c_ident(sec) && sets.imports.contains_key(&s.name));
        if !startstop && !ref_dynamic.contains(&s.name) {
            continue;
        }
        let Some(version) = decide(&s.name, &mut sets) else {
            sets.local.insert(s.name.clone());
            continue;
        };
        let vis = if startstop { STV_PROTECTED } else { STV_DEFAULT };
        let preemptible =
            !startstop && line_preempts(sl, list.as_ref(), s.sym.sym_type(), &s.name, format);
        sets.exports.entry(s.name.clone()).or_default().push(Export {
            name: s.name.clone(),
            version,
            ty: s.sym.sym_type(),
            bind: STB_GLOBAL,
            vis,
            size: s.sym.st_size,
            preemptible,
            optional: startstop && format == Format::Mold,
        });
    }
    // L-96's keys the output does not define (W23): still the link's own,
    // decided as above from the inputs and the script; their section is
    // gone (`dropped`) unless the map keeps one.
    for (name, (sec, kept)) in &startstop {
        let in_output = image
            .symbols
            .iter()
            .any(|s| s.name == *name && s.sym.st_shndx != 0 && s.sym.sym_type() != STT_SECTION);
        if !kept {
            sets.dropped.push((name.clone(), sec.clone()));
        }
        if in_output || sets.exports.contains_key(name) || sets.local.contains(name) {
            continue;
        }
        let Some(version) = decide(name, &mut sets) else {
            sets.local.insert(name.clone());
            continue;
        };
        sets.exports.entry(name.clone()).or_default().push(Export {
            name: name.clone(),
            version,
            ty: STT_NOTYPE,
            bind: STB_GLOBAL,
            vis: STV_PROTECTED,
            size: 0,
            preemptible: false,
            optional: format == Format::Mold,
        });
    }
    // A name the linker defines (`__start_`/`__stop_`, mold's local
    // forms included) is not an import; unexported, it is bound.
    let mut own = Vec::new();
    sets.imports.retain(|n, _| {
        let linker_def = startstop.contains_key(n)
            || image
                .symbols
                .iter()
                .any(|s| s.name == *n && s.sym.st_shndx != 0 && s.sym.sym_type() != STT_SECTION);
        if linker_def && !sets.exports.contains_key(n) {
            own.push(n.clone());
        }
        !(sets.exports.contains_key(n) || sets.local.contains(n) || linker_def)
    });
    sets.local.extend(own);
    rule.excluded = excluded;
    sets.rule = Some(rule);
    sets
}

/// P-D19: `.dynsym`'s defined entries against the export set, with
/// binding, type, visibility and version (D-110, D-115).
#[allow(clippy::too_many_lines)] // one pass over .dynsym, then the set
pub fn pd19(out: &DynOut, sets: &Sets, format: Format) -> Vec<Finding> {
    let mut v = Vec::new();
    let sym = |name: &str, m: String| {
        let mut f = fd(19, m);
        f.symbol = Some(name.to_string());
        f
    };
    let mut seen: BTreeSet<(String, VerOut)> = BTreeSet::new();
    for (j, s) in out.syms.iter().enumerate().skip(1) {
        if !s.defined() || s.binding() == STB_LOCAL || s.sym_type() == STT_SECTION {
            continue;
        }
        let version = match out.def_version(j) {
            None => VerOut::Base,
            Some((n, false)) => VerOut::Node(n),
            Some((n, true)) => VerOut::Hidden(n),
        };
        // GNU ld's version node names (D-110 item 3): `V1@@V1`, SHN_ABS.
        if format == Format::Gnu
            && s.st_shndx == mortar_elf::consts::SHN_ABS
            && version == VerOut::Node(s.name.clone())
            && sets.nodes.contains(&s.name)
        {
            continue;
        }
        if let Some(&value) = sets.placeless.get(&s.name)
            && !sets.exports.contains_key(&s.name)
        {
            // GNU ld's form (measured, WP-147): an absolute symbol whose
            // value is the definition's offset in its section.
            if format == Format::Gnu
                && s.st_shndx == mortar_elf::consts::SHN_ABS
                && s.st_value == value
            {
                continue;
            }
            v.push(sym(
                &s.name,
                format!(
                    "'{}' is defined in .dynsym (at {:#x}), but its definition has no place: P-1 omits it (resolve.md §10.1, D-110)",
                    s.name, s.st_value
                ),
            ));
            continue;
        }
        let Some(e) =
            sets.exports.get(&s.name).and_then(|x| x.iter().find(|e| e.version == version))
        else {
            let key = match &version {
                VerOut::Base => s.name.clone(),
                VerOut::Node(n) => format!("{}@@{n}", s.name),
                VerOut::Hidden(n) => format!("{}@{n}", s.name),
            };
            let why = if sets.local.contains(&s.name) || sets.local.contains(&key) {
                " (a version script or --exclude-libs makes it local)"
            } else if sets.exports.contains_key(&s.name) {
                " at this version"
            } else {
                ""
            };
            v.push(sym(
                &s.name,
                format!(
                    "'{}' ({version:?}) is in .dynsym and not in the export set{why} (D-110)",
                    s.name
                ),
            ));
            continue;
        };
        seen.insert((e.name.clone(), e.version.clone()));
        if s.binding() != e.bind {
            v.push(sym(
                &s.name,
                format!(
                    "'{}' is exported with binding {}, its definition's is {} (D-111)",
                    s.name,
                    s.binding(),
                    e.bind
                ),
            ));
        }
        // A bound IFUNC may be exported at its IPLT entry (D-121).
        let ty_ok = s.sym_type() == e.ty
            || (e.ty == STT_GNU_IFUNC && !e.preemptible && s.sym_type() == STT_FUNC);
        if !ty_ok {
            v.push(sym(
                &s.name,
                format!(
                    "'{}' is exported with type {}, its definition's is {} (D-111)",
                    s.name,
                    s.sym_type(),
                    e.ty
                ),
            ));
        }
        if s.visibility() != e.vis {
            v.push(sym(
                &s.name,
                format!(
                    "'{}' is exported with visibility {}, its definitions' is {} (D-110)",
                    s.name,
                    s.visibility(),
                    e.vis
                ),
            ));
        }
    }
    for es in sets.exports.values() {
        for e in es {
            if !e.optional && !seen.contains(&(e.name.clone(), e.version.clone())) {
                v.push(sym(
                    &e.name,
                    format!(
                        "'{}' ({:?}) is in the export set and not defined in .dynsym (D-110)",
                        e.name, e.version
                    ),
                ));
            }
        }
    }
    v
}

// ---------------------------------------------------------------------------
// The static rules: P-D20, P-D21, P-D22, P-D25, P-D26
// ---------------------------------------------------------------------------

pub const DT_SYMBOLIC: u64 = 16;
pub const DF_SYMBOLIC: u64 = 2;
/// `DT_FLAGS_1` bits of D-103's `-z` words.
pub const DF_1_NODELETE: u64 = 0x8;
pub const DF_1_INITFIRST: u64 = 0x20;
pub const DF_1_NOOPEN: u64 = 0x40;
pub const DF_1_ORIGIN: u64 = 0x80;
pub const DF_1_INTERPOSE: u64 = 0x400;
/// `GNU_PROPERTY_1_NEEDED` and its `INDIRECT_EXTERN_ACCESS` bit.
const GNU_PROPERTY_1_NEEDED: u32 = 0xb000_8000;
const NEEDED_INDIRECT_EXTERN_ACCESS: u64 = 1;

/// Whether an input relocation type is a local-exec TLS form (P-D20).
pub fn le_type(machine: u16, ty: u32) -> bool {
    match machine {
        mortar_elf::consts::EM_AARCH64 => (544..=559).contains(&ty) || matches!(ty, 570 | 571),
        mortar_elf::consts::EM_X86_64 => matches!(ty, 23 | 18),
        _ => false,
    }
}

/// The `r_sym` 0 kinds P-D25 allows, per target; a reference may add a
/// `DTPOFF` with `r_sym` 0 holding the static offset (P-D26).
/// P-D25's finding on GNU ld's spare `R_AARCH64_NONE` record, which the
/// harness accepts as anomaly A46 (§5.8).
pub const SPARE_NONE: &str = "a spare R_AARCH64_NONE record (r_sym 0, offset 0, addend 0) after DT_RELACOUNT's RELATIVE prefix, at most one per local general-dynamic pair (A46)";

/// The indices in `out.rels` of A46's shape (P-D25, WP-144): in a
/// reference's AArch64 output, `DT_RELA` records of type 0 with `r_sym`,
/// `r_offset` and `r_addend` all 0, none inside the `DT_RELACOUNT`
/// records glibc applies as `RELATIVE` without reading their type, and
/// no more of them than `R_AARCH64_TLS_DTPMOD64` records with `r_sym` 0
/// (GNU ld reserves one record per local general-dynamic pair and writes
/// the DTP offset as a constant instead). Anything else of type 0 stays
/// the plain finding; mortar's own output never has one (D-30).
fn spare_none(machine: u16, format: Format, out: &DynOut) -> Vec<usize> {
    if format == Format::Mortar || machine != mortar_elf::consts::EM_AARCH64 {
        return Vec::new();
    }
    let prefix = out.get(crate::dynout::DT_RELACOUNT).unwrap_or(0);
    let pairs = out.rels.iter().filter(|r| r.ty == 1028 && r.sym == 0).count();
    let spare: Vec<usize> = out
        .rels
        .iter()
        .enumerate()
        .filter(|(i, r)| {
            !r.plt
                && *i as u64 >= prefix
                && r.ty == 0
                && r.sym == 0
                && r.offset == 0
                && r.addend == Some(0)
        })
        .map(|(i, _)| i)
        .collect();
    if spare.len() > pairs { Vec::new() } else { spare }
}

fn sym0_allowed(machine: u16, ty: u32, format: Format) -> bool {
    match machine {
        mortar_elf::consts::EM_AARCH64 => {
            matches!(ty, 1027 | 1032 | 1028 | 1030 | 1031)
                || (format != Format::Mortar && ty == 1029)
        }
        mortar_elf::consts::EM_X86_64 => {
            matches!(ty, 8 | 37 | 16 | 18 | 36) || (format != Format::Mortar && ty == 17)
        }
        _ => true,
    }
}

/// The `GNU_PROPERTY_1_NEEDED` words of a `.note.gnu.property` section.
fn needed_bits(data: &[u8], word: usize) -> u64 {
    let mut bits = 0;
    let mut at = 0usize;
    let u32at =
        |b: &[u8], o: usize| b.get(o..o + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]));
    while let (Some(nsz), Some(dsz), Some(ty)) =
        (u32at(data, at), u32at(data, at + 4), u32at(data, at + 8))
    {
        let name_end = at + 12 + (nsz as usize).next_multiple_of(4);
        let desc = name_end;
        let desc_end = desc + dsz as usize;
        if desc_end > data.len() {
            break;
        }
        if ty == 5 && data.get(at + 12..at + 16) == Some(b"GNU\0") {
            let mut p = desc;
            while p + 8 <= desc_end {
                let (Some(pt), Some(sz)) = (u32at(data, p), u32at(data, p + 4)) else { break };
                if pt == GNU_PROPERTY_1_NEEDED && sz >= 4 {
                    bits |= u64::from(u32at(data, p + 8).unwrap_or(0));
                }
                p += 8 + (sz as usize).next_multiple_of(word);
            }
        }
        at = desc_end.next_multiple_of(word.max(4));
    }
    bits
}

/// The static rules of §12.5 on a `Shared` output.
#[allow(clippy::too_many_lines)] // one block per rule
pub(crate) fn static_checks(
    image: &Image<'_>,
    out: &DynOut,
    objs: &[Obj<'_>],
    sl: &SharedLine,
    sets: &Sets,
    args: &[String],
    format: Format,
) -> Vec<Finding> {
    use crate::loader::{Kind, kind};
    let mut v = pd19(out, sets, format);
    // P-D17 (W23): the input's relocation against `__start_X` computes
    // section X's address, and the output has no X to hold.
    for (key, sec) in &sets.dropped {
        let mut f = fd(
            17,
            format!(
                "view A: a live input section's relocation against '{key}' computes the address \
                 of section '{sec}', which layout L-96 keeps live, and the output holds no \
                 '{sec}' (W23)"
            ),
        );
        f.symbol = Some(key.clone());
        v.push(f);
    }
    let m = image.header.e_machine;
    let name_of = |r: &crate::dynout::DynRel| {
        out.syms.get(r.sym as usize).map_or_else(String::new, |s| s.name.clone())
    };
    // P-D20: the forbidden forms.
    for r in &out.rels {
        if kind(m, r.ty) == Kind::Copy {
            v.push(
                fd(20, format!("a COPY record of '{}' in a shared object (D-122)", name_of(r)))
                    .at(r.offset),
            );
        }
    }
    for s in out.syms.iter().skip(1) {
        if !s.defined() && s.st_value != 0 {
            let mut f = fd(
                20,
                format!(
                    "'{}' has a canonical PLT entry (SHN_UNDEF, st_value {:#x}; D-122)",
                    s.name, s.st_value
                ),
            );
            f.symbol = Some(s.name.clone());
            v.push(f);
        }
    }
    if out.has(crate::dynout::DT_TEXTREL)
        || out.get(crate::dynout::DT_FLAGS).is_some_and(|f| f & crate::dynout::DF_TEXTREL != 0)
    {
        v.push(fd(20, "DT_TEXTREL in a shared object (D-117, D-47)"));
    }
    let relro: Vec<(u64, u64)> = image
        .phdrs
        .iter()
        .filter(|q| q.p_type == mortar_elf::consts::PT_GNU_RELRO)
        .map(|q| (q.p_vaddr, q.p_vaddr.saturating_add(q.p_memsz)))
        .collect();
    let word: u64 = if image.class() == mortar_elf::Class::Elf64 { 8 } else { 4 };
    for r in &out.rels {
        if matches!(kind(m, r.ty), Kind::None | Kind::Copy) {
            continue;
        }
        if let Some(lp) = image.load_of(r.offset, word)
            && lp.p_flags & mortar_elf::consts::PF_W == 0
            && !relro.iter().any(|&(lo, hi)| lo <= r.offset && r.offset + word <= hi)
        {
            v.push(
                fd(20, format!("a dynamic record (type {}) writes a non-writable place outside PT_GNU_RELRO: a text relocation (D-47)", r.ty))
                    .at(r.offset),
            );
        }
    }
    for o in objs {
        let machine = o.elf.machine().e_machine();
        for s in &o.secs {
            let (Some(rs), true) = (s.relocs, s.alloc()) else { continue };
            if matches!(s.place, Place::Absent | Place::Discarded) {
                continue;
            }
            let Some(recs) = o.records(rs) else { continue };
            for &(off, _, ty, _) in &recs {
                if le_type(machine, ty) {
                    let mut f = fd(
                        20,
                        format!(
                            "a local-exec TLS form (type {ty}) in a shared object's code (D-119)"
                        ),
                    );
                    f.site = Some(
                        o.site(o.secs.iter().position(|x| std::ptr::eq(x, s)).unwrap_or(0), off),
                    );
                    v.push(f);
                }
            }
        }
    }
    for s in &image.sections {
        if s.name == ".ctors" || s.name == ".dtors" {
            v.push(fd(20, format!("an output section {} in a shared object (D-124)", s.name)));
        }
    }
    // P-D21: each PT_LOAD's alignment is the max page size.
    let mut page: u64 = if m == mortar_elf::consts::EM_AARCH64 { 0x10000 } else { 0x1000 };
    let mut i = 0;
    while i < args.len() {
        let z = if args[i] == "-z" {
            i += 1;
            args.get(i).map(String::as_str)
        } else {
            args[i].strip_prefix("-z")
        };
        if let Some(n) = z.and_then(|z| z.strip_prefix("max-page-size=")) {
            let n = n.trim();
            let p = n
                .strip_prefix("0x")
                .map_or_else(|| n.parse().ok(), |h| u64::from_str_radix(h, 16).ok());
            if let Some(p) = p {
                page = p;
            }
        }
        i += 1;
    }
    for p in image.phdrs.iter().filter(|p| p.p_type == mortar_elf::consts::PT_LOAD) {
        if p.p_align != page {
            v.push(fd(21, format!("a PT_LOAD at {:#x} has p_align {:#x}, the max page size is {page:#x} (D-123)", p.p_vaddr, p.p_align)).at(p.p_vaddr));
        }
        if p.p_align > 0 && p.p_offset % p.p_align != p.p_vaddr % p.p_align {
            v.push(
                fd(
                    21,
                    format!(
                        "a PT_LOAD at {:#x}: p_offset {:#x} is not p_vaddr modulo p_align {:#x}",
                        p.p_vaddr, p.p_offset, p.p_align
                    ),
                )
                .at(p.p_vaddr),
            );
        }
    }
    // P-D22: the shared object's `.dynamic`.
    let dt_sym = out.has(DT_SYMBOLIC);
    let df_sym = out.get(crate::dynout::DT_FLAGS).is_some_and(|f| f & DF_SYMBOLIC != 0);
    let want = sl.df_symbolic();
    // The references' forms (D-102, D-103): GNU ld writes both, lld the
    // flag, mold neither; mortar both.
    let ok = match (want, format) {
        (false, _) => !dt_sym && !df_sym,
        (true, Format::Mortar | Format::Gnu) => dt_sym && df_sym,
        (true, Format::Lld) => df_sym,
        (true, Format::Mold) => true,
    };
    if !ok {
        v.push(fd(
            22,
            format!(
                "DT_SYMBOLIC {dt_sym}, DF_SYMBOLIC {df_sym}: the line's symbolic form is {} {} --dynamic-list (D-112, W31)",
                sl.form().name(),
                if sl.dynamic_lists.is_empty() { "without" } else { "with" }
            ),
        ));
    }
    // D-115: of the definitions of one name (`.symver` `foo@V1` and
    // `foo@@V2`), every one but the default carries VERSYM_HIDDEN, or an
    // unversioned lookup finds the first, not the default.
    if let Some(vs) = &out.versym {
        let mut defaults: BTreeMap<&str, usize> = BTreeMap::new();
        for (j, x) in out.syms.iter().enumerate().skip(1) {
            if x.defined()
                && !x.name.is_empty()
                && x.binding() != STB_LOCAL
                && vs.get(j).is_some_and(|&w| w & 0x8000 == 0)
            {
                *defaults.entry(x.name.as_str()).or_default() += 1;
            }
        }
        for (n, c) in defaults.into_iter().filter(|e| e.1 > 1) {
            let mut f = fd(
                22,
                format!("'{n}' has {c} definitions without VERSYM_HIDDEN in .dynsym (D-115)"),
            );
            f.symbol = Some(n.to_string());
            v.push(f);
        }
    }
    // D-102's order (mortar's form; D-30's is an executable's).
    if format == Format::Mortar {
        v.extend(d102_order(&out.entries));
    }
    // DT_FLAGS_1: D-103's bits, each iff its `-z` word is on the line
    // (D-30's executable row is not a library's: `filter_dyncheck`).
    // `DF_1_NOW` stays D-30's (P-D6).
    let f1 = out.get(DT_FLAGS_1).unwrap_or(0);
    let asked = sl.flags_1() & !crate::dynout::DF_1_NOW;
    let extra = f1 & !(asked | crate::dynout::DF_1_NOW | DF_1_PIE);
    let missing = asked & !f1;
    if extra != 0 || missing != 0 {
        v.push(fd(
            22,
            format!(
                "DT_FLAGS_1 {f1:#x}: bits {extra:#x} no -z word of the line asks for, bits {missing:#x} the line asks for are absent (D-103)"
            ),
        ));
    }
    // DT_FLAGS' `DF_ORIGIN` iff the line has `-z origin` (D-103, all
    // three; WP-161).
    let origin =
        out.get(crate::dynout::DT_FLAGS).is_some_and(|f| f & crate::dynout::DF_ORIGIN != 0);
    let asked = sl.flags_1() & DF_1_ORIGIN != 0;
    if origin != asked {
        v.push(fd(
            22,
            format!("DT_FLAGS' DF_ORIGIN {origin}; the line asks for -z origin {asked} (D-103)"),
        ));
    }
    // DT_INIT/DT_FINI at the defined, live function they name (D-124).
    let line = Line::parse(args, &crate::line::HostFiles);
    for (tag, opt, default) in [
        (crate::dynout::DT_INIT, line.init.as_deref(), "_init"),
        (crate::dynout::DT_FINI, line.fini.as_deref(), "_fini"),
    ] {
        let name = opt.unwrap_or(default);
        let def = image.symbols.iter().find(|s| s.name == name && s.sym.st_shndx != 0);
        let tagname = crate::dynout::tag_name(tag);
        match (out.get(tag), def) {
            (Some(val), Some(d)) => {
                let exec = image
                    .load_of(val, 1)
                    .is_some_and(|p| p.p_flags & mortar_elf::consts::PF_X != 0);
                if val != d.sym.st_value {
                    v.push(fd(
                        22,
                        format!(
                            "{tagname} is {val:#x}; '{name}' is at {:#x} (D-124, W22)",
                            d.sym.st_value
                        ),
                    ));
                } else if !exec {
                    // The loader calls it (D-102 rows 3-4): a data object
                    // named by `-init`/`-fini` is refused by mortar; the
                    // references write its address (WP-161).
                    v.push(fd(
                        22,
                        format!(
                            "{tagname} is {val:#x}, '{name}', which is not in an executable segment: the loader calls it (D-102 rows 3-4, D-124)"
                        ),
                    ));
                }
            }
            (Some(val), None) => {
                v.push(fd(
                    22,
                    format!(
                        "{tagname} is {val:#x} and the output defines no '{name}' (D-124, W22)"
                    ),
                ));
            }
            (None, Some(_)) => {
                v.push(fd(
                    22,
                    format!("no {tagname}, and the output defines '{name}' (D-30 rows 3-4, W22)"),
                ));
            }
            (None, None) => {}
        }
    }
    // DT_SONAME: the `-soname` string.
    let soname = out.get(crate::dynout::DT_SONAME).map(|o| out.string(image, o));
    match (&sl.soname, soname) {
        (None, None) => {}
        (Some(want), Some(Some(got))) if *want == got => {}
        (want, got) => v.push(fd(
            22,
            format!("DT_SONAME is {:?}; the line's -soname is {want:?} (D-102)", got.flatten()),
        )),
    }
    // P-D27: the version definitions and every entry's `.gnu.version`
    // index (D-115; WP-149).
    v.extend(pd27(image, out, sets, sl, format));
    // NEEDED_INDIRECT_EXTERN_ACCESS kept iff an input has it (D-112).
    let w = word as usize;
    let input_has = objs.iter().any(|o| {
        // One small note per object, read by position (§8.1).
        o.secs.iter().enumerate().any(|(si, s)| {
            s.name == ".note.gnu.property"
                && needed_bits(&o.data(si), w) & NEEDED_INDIRECT_EXTERN_ACCESS != 0
        })
    });
    let output_has = image.sections.iter().any(|s| {
        s.name == ".note.gnu.property"
            && image
                .bytes(s.shdr.sh_addr, s.shdr.sh_size)
                .is_some_and(|d| needed_bits(d, w) & NEEDED_INDIRECT_EXTERN_ACCESS != 0)
    });
    if input_has != output_has {
        v.push(fd(22, format!("GNU_PROPERTY_1_NEEDED_INDIRECT_EXTERN_ACCESS: an input has it {input_has}, the output {output_has} (D-112, W41)")));
    }
    // P-D25: records name only preemptible keys and imports; the `r_sym` 0
    // kinds per target.
    let spare = spare_none(m, format, out);
    for (k, r) in out.rels.iter().enumerate() {
        if r.sym as usize >= out.syms.len() {
            v.push(
                fd(
                    25,
                    format!(
                        "a dynamic record names .dynsym entry {}, past the table's {} entries",
                        r.sym,
                        out.syms.len()
                    ),
                )
                .at(r.offset),
            );
            continue;
        }
        if r.sym == 0 {
            if spare.contains(&k) {
                v.push(fd(25, SPARE_NONE).at(r.offset));
            } else if !sym0_allowed(m, r.ty, format) {
                v.push(
                    fd(
                        25,
                        format!("a dynamic record of type {} with r_sym 0 (P-D25's kinds)", r.ty),
                    )
                    .at(r.offset),
                );
            }
            continue;
        }
        let n = name_of(r);
        // A type-0 record that names a key: no kind of P-D25 (A46's
        // shape has r_sym 0; WP-144).
        if r.ty == 0 {
            let mut f = fd(25, format!("a dynamic record of type 0 names '{n}' (P-D25's kinds)"))
                .at(r.offset);
            f.symbol = Some(n);
            v.push(f);
            continue;
        }
        // From a reference, a record naming a protected key, or any bound
        // key of a `-Bsymbolic` library that says so (`DT_SYMBOLIC`/
        // `DF_SYMBOLIC`): glibc binds both to the object's own definition
        // (GNU ld's form; view B checks the value).
        let own = format != Format::Mortar
            && ((dt_sym || df_sym) && sl.df_symbolic()
                || sets
                    .exports
                    .get(&n)
                    .is_some_and(|es| es.iter().all(|e| e.vis == STV_PROTECTED)));
        if (sets.bound(&n) || sets.local.contains(&n)) && !own {
            let mut f = fd(
                25,
                format!(
                    "a dynamic record (type {}) names '{n}', which the line binds (D-112)",
                    r.ty
                ),
            )
            .at(r.offset);
            f.symbol = Some(n);
            v.push(f);
        }
    }
    // P-D26: TLSDESC records in `.rela.dyn` for mortar (D-119);
    // `DF_STATIC_TLS` iff an IE slot exists.
    if format == Format::Mortar {
        for r in out.rels.iter().filter(|r| r.plt && kind(m, r.ty) == Kind::TlsDesc) {
            v.push(
                fd(26, "a TLSDESC record in DT_JMPREL (D-119: eager, in .rela.dyn)").at(r.offset),
            );
        }
    }
    let ie = out.rels.iter().any(|r| kind(m, r.ty) == Kind::TpOff);
    let static_tls =
        out.get(crate::dynout::DT_FLAGS).is_some_and(|f| f & crate::dynout::DF_STATIC_TLS != 0);
    // GNU ld on AArch64 never sets it (dyn.md's reference table, D-119).
    let gnu_a64 = format == Format::Gnu && m == mortar_elf::consts::EM_AARCH64;
    if ie != static_tls && (static_tls || !gnu_a64) {
        v.push(fd(
            26,
            format!("DF_STATIC_TLS {static_tls}; an initial-exec slot exists {ie} (D-119)"),
        ));
    }
    v
}

/// `VER_FLG_BASE`.
const VER_FLG_BASE: u16 = 1;
/// `VERSYM_HIDDEN`.
const VERSYM_HIDDEN: u16 = 0x8000;

/// The `.gnu.version` index D-115 gives an export of version `v`, with the
/// script's nodes `nodes` at indices 2, 3, ... (`None`: a node the script
/// does not have, which P-D19 reports).
fn versym_of(v: &VerOut, nodes: &[String]) -> Option<u16> {
    let ndx = |n: &str| nodes.iter().position(|x| x == n).and_then(|i| u16::try_from(i + 2).ok());
    match v {
        VerOut::Base => Some(1),
        VerOut::Node(n) => ndx(n),
        VerOut::Hidden(n) => ndx(n).map(|i| i | VERSYM_HIDDEN),
    }
}

/// P-D27 (D-115, WP-149): a `Shared` output's version definitions, against
/// the script the line names (read from its files, `--version-script` of
/// the map's `arg` records or of the reference's line) and the export set
/// recomputed from the inputs, never from the output:
///
/// - `DT_VERDEF` iff the script names a node; `DT_VERDEFNUM` 1 + the
///   nodes, and the chain that long (each `vd_next` but the last's non-0,
///   the last's 0);
/// - the base: index 1, `vd_flags` `VER_FLG_BASE`, `vd_cnt` 1, named by
///   `-soname`, else by the output's basename (lld: the `-o` path as
///   written, `spec/dyn.md` D-115's [X-F] F3);
/// - node k (script order): index k + 2, `vd_flags` 0, `vd_version` 1,
///   `vd_hash` the ELF hash of its name, its `Verdaux` first, then (GNU ld
///   and mortar) its parents last-written first and `vd_cnt` 1 + their
///   number; lld and mold write no parent (`vd_cnt` 1: measured,
///   `parents.test`);
/// - `.gnu.version_d`'s header: `sh_info` the count, `sh_link` `.dynstr`,
///   `sh_addr` `DT_VERDEF`; mortar's `sh_addralign` 4;
/// - every `.dynsym` entry's index: 0 for a local entry; for a definition,
///   the index of its version in the export set (1 unversioned, the
///   node's, the node's with `VERSYM_HIDDEN` for `foo@V`), each export's
///   index once; an import's index is 0, 1 or a `Vernaux`'s, never a
///   `Verdef`'s, and the two spaces share `.gnu.version`: definitions
///   1 ..= `DT_VERDEFNUM`, then every `vna_other` above them.
#[allow(clippy::too_many_lines)] // one block per field
pub(crate) fn pd27(
    image: &Image<'_>,
    out: &DynOut,
    sets: &Sets,
    sl: &SharedLine,
    format: Format,
) -> Vec<Finding> {
    use crate::dynout::{DT_VERDEF, DT_VERDEFNUM};
    let mut v = Vec::new();
    let any_node = !sets.nodes.is_empty();
    let want_num = if any_node { sets.nodes.len() as u64 + 1 } else { 0 };
    let num = out.get(DT_VERDEFNUM);
    match (any_node, out.has(DT_VERDEF)) {
        (false, true) => {
            v.push(fd(27, "DT_VERDEF and the version script names no version node (D-115)"));
        }
        (true, false) => {
            v.push(fd(27, "no DT_VERDEF and the version script names a version node (D-115)"));
        }
        (false, false) => {
            if num.is_some() {
                v.push(fd(27, "DT_VERDEFNUM without DT_VERDEF (D-115)"));
            }
        }
        (true, true) => {
            if num != Some(want_num) {
                v.push(fd(
                    27,
                    format!(
                        "DT_VERDEFNUM is {num:?}; the script names {} nodes, so {want_num} Verdefs with the base (D-115)",
                        sets.nodes.len()
                    ),
                ));
            }
            if num != Some(out.verdefs.len() as u64) {
                v.push(fd(
                    27,
                    format!(
                        "DT_VERDEFNUM is {num:?}; the Verdef chain has {} entries (D-115)",
                        out.verdefs.len()
                    ),
                ));
            }
            let base_want = match (&sl.soname, format) {
                (Some(s), _) => s.clone(),
                (None, Format::Lld) => sl.output.clone().unwrap_or_else(|| "a.out".into()),
                (None, _) => {
                    let o = sl.output.as_deref().unwrap_or("a.out");
                    o.rsplit_once('/').map_or(o, |(_, f)| f).to_string()
                }
            };
            let parents_written = matches!(format, Format::Mortar | Format::Gnu);
            let last = out.verdefs.len().saturating_sub(1);
            for (k, d) in out.verdefs.iter().enumerate() {
                let n = d.names.first().cloned().unwrap_or_default();
                let base = k == 0;
                let mut bad = Vec::new();
                if d.version != 1 {
                    bad.push(format!("vd_version {}", d.version));
                }
                if usize::from(d.ndx) != k + 1 {
                    bad.push(format!("vd_ndx {} at position {k}", d.ndx));
                }
                let flags = if base { VER_FLG_BASE } else { 0 };
                if d.flags != flags {
                    bad.push(format!("vd_flags {:#x}, not {flags:#x}", d.flags));
                }
                if d.hash != crate::dynout::elf_hash(n.as_bytes()) {
                    bad.push(format!("vd_hash {:#x}, not the ELF hash of its name", d.hash));
                }
                let want_names: Vec<String> = if base {
                    vec![base_want.clone()]
                } else if let Some(node) = sets.nodes.get(k - 1) {
                    let mut w = vec![node.clone()];
                    if parents_written {
                        w.extend(sets.parents.get(k - 1).into_iter().flatten().rev().cloned());
                    }
                    w
                } else {
                    Vec::new()
                };
                if !want_names.is_empty() && d.names != want_names {
                    bad.push(format!(
                        "names (its own, then its parents) {:?}, not {want_names:?}",
                        d.names
                    ));
                }
                if usize::from(d.cnt) != d.names.len()
                    || (!want_names.is_empty() && usize::from(d.cnt) != want_names.len())
                {
                    bad.push(format!(
                        "vd_cnt {}, with {} Verdaux entries read",
                        d.cnt,
                        d.names.len()
                    ));
                }
                if d.aux_next != 0 {
                    bad.push(format!("the last Verdaux's vda_next {:#x}, not 0", d.aux_next));
                }
                if (d.next == 0) != (k == last) {
                    bad.push(format!("vd_next {:#x}", d.next));
                }
                if !bad.is_empty() {
                    v.push(fd(27, format!("Verdef {k} ('{n}'): {} (D-115)", bad.join(", "))));
                }
            }
            // The section header, when the output has one.
            if let Some(s) = image.sections.iter().find(|s| s.name == ".gnu.version_d") {
                let dynstr = image.sections.iter().position(|x| x.name == ".dynstr");
                let mut bad = Vec::new();
                if u64::from(s.shdr.sh_info) != out.verdefs.len() as u64 {
                    bad.push(format!("sh_info {}", s.shdr.sh_info));
                }
                if dynstr.is_some_and(|i| i as u64 != u64::from(s.shdr.sh_link)) {
                    bad.push(format!("sh_link {} (not .dynstr)", s.shdr.sh_link));
                }
                if out.get(DT_VERDEF) != Some(s.shdr.sh_addr) {
                    bad.push(format!("sh_addr {:#x} (not DT_VERDEF)", s.shdr.sh_addr));
                }
                if format == Format::Mortar && s.shdr.sh_addralign != 4 {
                    bad.push(format!("sh_addralign {}", s.shdr.sh_addralign));
                }
                if !bad.is_empty() {
                    v.push(fd(
                        27,
                        format!(
                            ".gnu.version_d's header: {} ({} Verdefs; D-115)",
                            bad.join(", "),
                            out.verdefs.len()
                        ),
                    ));
                }
            }
        }
    }
    // Every entry's `.gnu.version` index.
    let Some(vs) = &out.versym else {
        if any_node {
            v.push(fd(
                27,
                "no .gnu.version (DT_VERSYM) and the version script names a node (D-115)",
            ));
        }
        return v;
    };
    let defs = num.unwrap_or(0);
    for n in &out.needs {
        if u64::from(n.other) <= defs.max(1) {
            v.push(fd(
                27,
                format!(
                    "vna_other {} of '{}' is not above the version definitions' indices 1..={} (D-115: definitions first, needs after)",
                    n.other,
                    n.name,
                    defs.max(1)
                ),
            ));
        }
    }
    let sym = |name: &str, m: String| {
        let mut f = fd(27, m);
        f.symbol = Some(name.to_string());
        f
    };
    // The expected indices of each name's exports, consumed as `.dynsym`
    // entries take them.
    let mut want: BTreeMap<&str, Vec<u16>> = BTreeMap::new();
    for (name, es) in &sets.exports {
        for e in es {
            if let Some(x) = versym_of(&e.version, &sets.nodes) {
                want.entry(name.as_str()).or_default().push(x);
            }
        }
    }
    for (j, s) in out.syms.iter().enumerate().skip(1) {
        let Some(&x) = vs.get(j) else { break };
        if s.binding() == STB_LOCAL {
            if x != 0 {
                v.push(sym(
                    &s.name,
                    format!("the local .dynsym entry {j} has .gnu.version {x:#x}, not 0 (D-115)"),
                ));
            }
            continue;
        }
        if !s.defined() {
            let i = u64::from(x & 0x7fff);
            // 0 on an undefined entry is unversioned too (resolve §9.2;
            // lld and mold write it for an unversioned weak import).
            if (2..=defs).contains(&i) || (x & VERSYM_HIDDEN != 0) {
                v.push(sym(
                    &s.name,
                    format!(
                        "the import '{}' has .gnu.version {x:#x}: 0, 1 or a Vernaux's index above the {defs} definitions, never hidden (D-115)",
                        s.name
                    ),
                ));
            }
            continue;
        }
        // GNU ld's node-name symbols (`V1@@V1`, SHN_ABS; D-110 item 3).
        if format == Format::Gnu
            && s.st_shndx == mortar_elf::consts::SHN_ABS
            && sets.nodes.contains(&s.name)
        {
            let w = versym_of(&VerOut::Node(s.name.clone()), &sets.nodes);
            if Some(x) != w {
                v.push(sym(
                    &s.name,
                    format!(
                        "GNU ld's node symbol '{}' has .gnu.version {x:#x}, not {w:?} (D-115)",
                        s.name
                    ),
                ));
            }
            continue;
        }
        let Some(ws) = want.get_mut(s.name.as_str()) else { continue }; // P-D19's
        if let Some(p) = ws.iter().position(|&w| w == x) {
            ws.remove(p);
        } else {
            v.push(sym(
                &s.name,
                format!(
                    "'{}' has .gnu.version {x:#x}; the version script and its .symver definitions give {:?} (D-115{})",
                    s.name,
                    ws.iter().map(|w| format!("{w:#x}")).collect::<Vec<_>>(),
                    if x & VERSYM_HIDDEN != 0 {
                        ": VERSYM_HIDDEN only on a non-default foo@V"
                    } else {
                        ""
                    }
                ),
            ));
        }
    }
    v
}

/// P-D26's "no relaxation" (D-119): a TLS sequence the engine accepted
/// as rewritten by the linker.
pub(crate) fn relaxed(cov: &crate::report::Coverage) -> Option<Finding> {
    use crate::report::Rewrite;
    let n: Vec<String> = cov
        .accepted
        .iter()
        .filter(|(r, _)| {
            matches!(
                r,
                Rewrite::TlsIeToLe
                    | Rewrite::TlsDescToLe
                    | Rewrite::TlsDescToIe
                    | Rewrite::TlsGdToIe
                    | Rewrite::TlsGdToLe
                    | Rewrite::TlsLdToLe
            )
        })
        .map(|(r, k)| format!("{r:?} x{k}"))
        .collect();
    (!n.is_empty()).then(|| {
        fd(
            26,
            format!(
                "TLS sequences relaxed in a shared object ({}): D-119 keeps the input's model",
                n.join(", ")
            ),
        )
    })
}

// ---------------------------------------------------------------------------
// The executable rules a `Shared` output answers differently
// ---------------------------------------------------------------------------

/// D-102's `.dynamic` order for a `Shared` output: D-30's with
/// `DT_SONAME` and `DT_SYMBOLIC` after `DT_NEEDED` (before the run path,
/// GNU ld's and mold's place), no `DT_PREINIT_ARRAY*` (refused) and no
/// `DT_DEBUG`, `DT_VERDEF`/`DT_VERDEFNUM` after `DT_FLAGS_1`.
const D102_ORDER: &[u64] = {
    use crate::dynout::{
        DT_AARCH64_BTI_PLT, DT_AARCH64_VARIANT_PCS, DT_FINI, DT_FINI_ARRAY, DT_FINI_ARRAYSZ,
        DT_FLAGS, DT_GNU_HASH, DT_INIT, DT_INIT_ARRAY, DT_INIT_ARRAYSZ, DT_JMPREL, DT_NEEDED,
        DT_PLTGOT, DT_PLTREL, DT_PLTRELSZ, DT_RELA, DT_RELACOUNT, DT_RELAENT, DT_RELASZ, DT_RPATH,
        DT_RUNPATH, DT_SONAME, DT_STRSZ, DT_STRTAB, DT_SYMENT, DT_SYMTAB, DT_VERDEF, DT_VERDEFNUM,
        DT_VERNEED, DT_VERNEEDNUM, DT_VERSYM,
    };
    &[
        DT_NEEDED,
        DT_SONAME,
        DT_SYMBOLIC,
        DT_RUNPATH,
        DT_RPATH,
        DT_INIT,
        DT_FINI,
        DT_INIT_ARRAY,
        DT_INIT_ARRAYSZ,
        DT_FINI_ARRAY,
        DT_FINI_ARRAYSZ,
        DT_GNU_HASH,
        DT_STRTAB,
        DT_SYMTAB,
        DT_STRSZ,
        DT_SYMENT,
        DT_PLTGOT,
        DT_PLTRELSZ,
        DT_PLTREL,
        DT_JMPREL,
        DT_RELA,
        DT_RELASZ,
        DT_RELAENT,
        DT_AARCH64_BTI_PLT,
        DT_AARCH64_VARIANT_PCS,
        DT_FLAGS,
        DT_FLAGS_1,
        DT_VERDEF,
        DT_VERDEFNUM,
        DT_VERNEED,
        DT_VERNEEDNUM,
        DT_VERSYM,
        DT_RELACOUNT,
    ]
};

/// P-D22: mortar's `.dynamic` tags (the `(tag, value)` entries) in
/// D-102's order, each one D-102 writes.
fn d102_order(entries: &[(u64, u64)]) -> Vec<Finding> {
    let mut v = Vec::new();
    let ranks: Vec<usize> = entries
        .iter()
        .filter(|e| e.0 != crate::dynout::DT_NULL)
        .map(|e| D102_ORDER.iter().position(|&t| t == e.0).unwrap_or(usize::MAX))
        .collect();
    if ranks.windows(2).any(|w| w[0] > w[1]) {
        v.push(fd(22, "the .dynamic tags are not in D-102's order"));
    }
    if ranks.contains(&usize::MAX) {
        v.push(fd(22, "a tag D-102 does not write"));
    }
    v
}

/// The `.dynamic` tags a library carries and no executable does (D-102,
/// D-103, D-115, D-119).
const SHARED_TAGS: &[u64] = &[
    crate::dynout::DT_SONAME,
    DT_SYMBOLIC,
    crate::dynout::DT_VERDEF,
    crate::dynout::DT_VERDEFNUM,
    0x6fff_fef6, // DT_TLSDESC_PLT (GNU ld)
    0x6fff_fef7, // DT_TLSDESC_GOT (GNU ld)
];

/// §12.5 view A, D-118: `name` is an unversioned import, which the host
/// resolves when nothing the link loads defines it and the line has no
/// `-z defs` (a versioned one needs its version, W42). Decided from the
/// sets the verifier recomputes from the inputs, never from the output's
/// `.dynsym` or `.gnu.version` (a name the link defines, L-96's
/// `__start_X` included, is no import: W23).
pub(crate) fn host_import(sets: &Sets, name: &str) -> bool {
    sets.imports.get(split_version(name).0).is_some_and(|i| !i.versioned)
}

/// D-115: every entry of `name` with `VERSYM_HIDDEN` is a definition whose
/// index names a `Verdef` (2 ..= `DT_VERDEFNUM`).
fn hidden_definition(out: &DynOut, name: &str) -> bool {
    let Some(vs) = &out.versym else { return false };
    let defs = out.get(crate::dynout::DT_VERDEFNUM).unwrap_or(0);
    out.syms
        .iter()
        .enumerate()
        .filter(|(j, s)| s.name == name && vs.get(*j).is_some_and(|x| x & 0x8000 != 0))
        .all(|(j, s)| {
            s.defined() && vs.get(j).is_some_and(|&x| (2..=defs).contains(&u64::from(x & 0x7fff)))
        })
}

fn hex_after(m: &str, key: &str) -> Option<u64> {
    let r = m.split_once(key)?.1.trim_start();
    let h = r.strip_prefix("0x")?;
    let end = h.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(h.len());
    u64::from_str_radix(&h[..end], 16).ok()
}

/// The executable rules of stage 3 that §12.5 replaces for a `Shared`
/// output, dropped from `dyncheck::check`'s findings (WP-122's module is
/// not changed; it may own the `Shared` forms later):
/// P-D1 (P-D16 decides the kind), P-D4's entry point when `e_entry` is 0
/// and its `PT_PHDR`/`PT_INTERP` rows (an executable's), P-D6's
/// executable-only tags and flags (P-D22 judges them), P-D8's "no Vernaux"
/// on a defined entry (a `Verdef` index: P-D19), P-D7's "not exported"
/// (P-D19's export set decides), P-D12 (P-D26), and P-D6's text
/// relocation (P-D20).
pub(crate) fn filter_dyncheck(
    v: &mut Vec<Finding>,
    image: &Image<'_>,
    out: &DynOut,
    sl: &SharedLine,
    sets: &Sets,
    format: Format,
) {
    let entry0 = image.header.e_entry == 0;
    // D-103: `DF_ORIGIN` is `-z origin`'s in a `Shared` output, judged by
    // P-D22 (WP-161), as `DF_SYMBOLIC` and `DF_STATIC_TLS` are.
    let mask = DF_SYMBOLIC | crate::dynout::DF_STATIC_TLS | crate::dynout::DF_ORIGIN;
    v.retain(|f| {
        let m = f.message.as_str();
        match f.rule {
            Rule::Dyn(1 | 12) => false,
            Rule::Dyn(4) => {
                !((entry0 && m.starts_with("the entry point 0x0 "))
                    || m.contains("PT_PHDR")
                    || m.contains("PT_INTERP"))
            }
            Rule::Dyn(6) => {
                if m.starts_with("no DT_DEBUG") || m.starts_with("DT_TEXTREL") {
                    return false;
                }
                if m.starts_with("a tag 0x") {
                    return !hex_after(m, "a tag").is_some_and(|t| SHARED_TAGS.contains(&t));
                }
                if m.starts_with("DT_FLAGS_1 0x") {
                    // P-D22 judges D-103's bits; DF_1_NOW and DF_1_PIE stay.
                    let lib =
                        DF_1_NODELETE | DF_1_INITFIRST | DF_1_NOOPEN | DF_1_ORIGIN | DF_1_INTERPOSE;
                    return hex_after(m, "has bits").is_none_or(|b| b & !lib != 0);
                }
                if m.starts_with("DT_FLAGS 0x") {
                    return hex_after(m, "has bits").is_none_or(|b| b & !mask != 0);
                }
                if m.starts_with("DT_FLAGS is present iff -z now") {
                    // mortar's form: DT_FLAGS also carries D-103's bits.
                    return out.get(crate::dynout::DT_FLAGS).is_none_or(|f| f & mask == 0);
                }
                // D-30's order has no library tags: D-102's is P-D22's.
                m != "a tag D-30 does not write" && m != "the .dynamic tags are not in D-30's order"
            }
            // lld's form of an unreferenced export with no place
            // (measured, WP-147): an import no input references.
            Rule::Dyn(7)
                if format == Format::Lld
                    && m.contains("is imported and no input references it")
                    && f.symbol.as_deref().is_some_and(|n| sets.placeless.contains_key(n)) =>
            {
                false
            }
            Rule::Dyn(7) => {
                !m.contains("is not exported (D-12, W8)")
                    && (!m.contains("is imported and no needed or used object defines it visibly")
                        || sl.zdefs
                        || !f.symbol.as_deref().is_some_and(|n| host_import(sets, n)))
            }
            Rule::Dyn(8)
                if m.starts_with("a .gnu.version entry with bit 15 set")
                    && f.symbol.as_deref().is_some_and(|n| hidden_definition(out, n)) =>
            {
                // D-115: `foo@V` (non-default) is an export with the
                // node's index and VERSYM_HIDDEN; D-27's "never" is an
                // executable's.
                false
            }
            Rule::Dyn(8) => {
                !(m.contains("names no Vernaux")
                    && f.symbol
                        .as_ref()
                        .is_some_and(|n| out.syms.iter().any(|s| &s.name == n && s.defined())))
            }
            _ => true,
        }
    });
}

/// Y3's binding and visibility rows for a key a version script or
/// `--exclude-libs` made local: the linker makes it `STB_LOCAL`
/// (D-114), which the inputs' global definition cannot say.
/// A use of a preemptible IFUNC key that view A finds wrong is P-D18's:
/// it must reach the resolver's result through the record naming it.
pub(crate) fn filter_view1(
    v: &mut Vec<Finding>,
    sets: &Sets,
    ifr: &HashMap<String, crate::loader::Interposed>,
) {
    v.retain(|f| {
        !(f.rule == Rule::SymbolTable(3)
            && f.symbol
                .as_ref()
                .is_some_and(|n| sets.local.contains(n) || sets.local.contains(split_version(n).0))
            && (f.message.contains("binding") || f.message.contains("visibility")))
    });
    for f in v.iter_mut() {
        if f.reloc.is_some()
            && let Some(k) = f.symbol.clone().filter(|k| ifr.contains_key(k))
        {
            f.rule = Rule::Dyn(18);
            f.message = format!(
                "view A: a use of the preemptible IFUNC '{k}' does not reach the resolver's result through a record naming it (W20, W21): {}",
                f.message
            );
        }
    }
}

/// Where the interposer of view B defines the keys (§12.5): functions
/// one page apart, data at fresh allocations of their size, TLS at
/// offsets of its own block.
pub(crate) fn interposer(sets: &Sets) -> HashMap<String, crate::loader::Interposed> {
    use crate::loader::{INTERPOSER, Interposed, lib_base};
    let base = lib_base(INTERPOSER);
    let (mut code, mut data, mut tls) = (base + 0x1000_0000, base + 0x2000_0000, 0x40u64);
    let mut m = HashMap::new();
    let mut put = |name: &str, ty: u8, size: u64| {
        let x = match ty {
            STT_TLS => {
                let a = tls;
                tls += size.max(1).next_multiple_of(16);
                Interposed { addr: base + a, size, tls: true }
            }
            STT_FUNC | STT_GNU_IFUNC => {
                let a = code;
                code += 0x1000;
                Interposed { addr: a, size, tls: false }
            }
            _ => {
                let a = data;
                data += size.max(1).next_multiple_of(16) + 16;
                Interposed { addr: a, size, tls: false }
            }
        };
        m.insert(name.to_string(), x);
    };
    // Every exported key, preemptible and bound alike (a record naming a
    // bound key resolves to the interposer, and P-D25 names the cause).
    for (name, es) in &sets.exports {
        if let Some(e) = es.iter().find(|e| e.preemptible).or_else(|| es.first()) {
            put(name, e.ty, e.size);
        }
    }
    for (name, i) in &sets.imports {
        put(name, if i.ty == STT_NOTYPE { STT_FUNC } else { i.ty }, 16);
    }
    m
}

/// The addresses view A gives each preemptible IFUNC key: the resolver's
/// result, reachable only through a record that names the key (P-D18).
pub(crate) fn ifunc_results(sets: &Sets) -> HashMap<String, crate::loader::Interposed> {
    use crate::loader::{INTERPOSER, Interposed, lib_base};
    let mut m = HashMap::new();
    let mut at = lib_base(INTERPOSER) + 0x3000_0000;
    for (name, es) in &sets.exports {
        if es.iter().any(|e| e.preemptible && e.ty == STT_GNU_IFUNC) {
            m.insert(name.clone(), Interposed { addr: at, size: 0, tls: false });
            at += 0x1000;
        }
    }
    m
}

/// Tells a shared object's view engine where the loader puts `keys`
/// (`m`'s addresses; a TLS key in the interposer's block), whatever the
/// output's own definitions or records say: the expected `S` (§12.5).
pub(crate) fn tell_engine(
    e: &mut crate::reloc::Engine<'_, '_>,
    m: &HashMap<String, crate::loader::Interposed>,
    keep: impl Fn(&str) -> bool,
    tls_start: u64,
) {
    use crate::loader::{INTERPOSER, lib_base, tls_fake};
    for (k, x) in m.iter().filter(|(k, _)| keep(k)) {
        e.loader_keys.insert(k.clone());
        if x.tls {
            e.dyn_tls.insert(k.clone());
            e.dyn_tls_mod.insert(k.clone(), 2 + INTERPOSER as u64);
            e.dynbind
                .insert(k.clone(), tls_fake(tls_start, INTERPOSER, x.addr - lib_base(INTERPOSER)));
        } else {
            e.dyn_tls.remove(k);
            e.dyn_tls_mod.remove(k);
            e.dynbind.insert(k.clone(), x.addr);
        }
    }
}

/// The names view B judges: every export and import.
pub(crate) fn view_b_names(sets: &Sets) -> HashSet<String> {
    let mut n: HashSet<String> = sets.exports.keys().cloned().collect();
    n.extend(sets.imports.keys().cloned());
    n
}

/// P-D3 for a `Shared` mortar map: its shared-object line records (§3.2)
/// agree with the words.
pub(crate) fn map_facts(sl: &SharedLine, d: &crate::map::DynRecords) -> Vec<Finding> {
    let words = sl.records();
    match &d.shared {
        Some(m) if *m == words => Vec::new(),
        m => vec![Finding::new(
            Rule::Dyn(3),
            format!("the map's shared-object records say {m:?}; the command line says {words:?}"),
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(t: &str) -> Result<Script, ScriptError> {
        Script::parse(&[("v.map".to_string(), t.to_string())])
    }

    fn words(w: &[&str]) -> Vec<String> {
        w.iter().map(|s| (*s).to_string()).collect()
    }

    // spec/plumb.md P-D22 (D-102): mortar's first libraries' tag order
    // (WP-123's `libt.so`, identical to GNU ld's library) is D-102's;
    // D-30's executable order put DT_SONAME nowhere (WP-121).
    #[test]
    fn p_d22_d102_order() {
        use crate::dynout::*;
        let mortar = [
            DT_NEEDED,
            DT_SONAME,
            DT_INIT,
            DT_FINI,
            DT_INIT_ARRAY,
            DT_INIT_ARRAYSZ,
            DT_FINI_ARRAY,
            DT_FINI_ARRAYSZ,
            DT_GNU_HASH,
            DT_STRTAB,
            DT_SYMTAB,
            DT_STRSZ,
            DT_SYMENT,
            DT_PLTGOT,
            DT_PLTRELSZ,
            DT_PLTREL,
            DT_JMPREL,
            DT_RELA,
            DT_RELASZ,
            DT_RELAENT,
            DT_VERNEED,
            DT_VERNEEDNUM,
            DT_VERSYM,
            DT_RELACOUNT,
            DT_NULL,
        ];
        let e = |t: &[u64]| t.iter().map(|&t| (t, 0)).collect::<Vec<_>>();
        assert!(d102_order(&e(&mortar)).is_empty());
        let mut sym = mortar.to_vec();
        sym.insert(2, DT_SYMBOLIC);
        sym.insert(3, DT_RUNPATH);
        sym.insert(22, DT_VERDEF);
        sym.insert(23, DT_VERDEFNUM);
        assert!(d102_order(&e(&sym)).is_empty());
        let mut swapped = mortar.to_vec();
        swapped.swap(0, 1);
        let f = d102_order(&e(&swapped));
        assert!(
            f.iter().any(|f| f.message == "the .dynamic tags are not in D-102's order"),
            "{f:?}"
        );
        let mut debug = mortar.to_vec();
        debug.insert(13, DT_DEBUG);
        let f = d102_order(&e(&debug));
        assert!(f.iter().any(|f| f.message == "a tag D-102 does not write"), "{f:?}");
    }

    struct NoFiles;
    impl Files for NoFiles {
        fn read(&self, _: &str) -> Option<Vec<u8>> {
            None
        }
    }

    /// The line read as `accept` reads it: the `-B` family by `Line`.
    fn sline(w: &[&str]) -> SharedLine {
        let w = words(w);
        SharedLine::read(&w, Line::parse(&w, &NoFiles).symbolic)
    }

    // spec: resolve.md §9.7 — globs are fnmatch(3) without flags.
    #[test]
    fn s9_7_fnmatch() {
        let m = |p: &str, s: &str| fnmatch(p.as_bytes(), s.as_bytes());
        assert!(m("*", "anything"));
        assert!(m("f?o", "foo") && !m("f?o", "fo"));
        assert!(m("f[a-c]o", "fbo") && !m("f[a-c]o", "fdo"));
        assert!(m("f[!a-c]o", "fdo") && !m("f[!a-c]o", "fbo"));
        assert!(m("f[^a]o", "fbo") && !m("f[^a]o", "fao"));
        assert!(m("f[o", "f[o"), "an unterminated [ is literal");
        assert!(!m("foo\\", "foo") && !m("foo\\", "foo\\"), "a trailing lone \\ matches nothing");
        assert!(m("a\\*b", "a*b") && !m("a\\*b", "axb"));
        assert!(m("_Z*", "_ZN3fooEv") && !m("_Z*", "foo"));
    }

    // spec: resolve.md §9.7 "The decision" — exact global, exact local,
    // last glob global, glob local, star global, star local, base.
    #[test]
    fn s9_7_the_decision() {
        let s =
            script("V1 { global: foo; f*; local: bar; b*; }; V2 { global: fa*; local: *; } V1;")
                .unwrap();
        assert_eq!(s.decide("foo"), Decision::Node(0));
        assert_eq!(s.decide("bar"), Decision::Local);
        assert_eq!(s.decide("fab"), Decision::Node(1), "the last node with a glob match");
        assert_eq!(s.decide("fxx"), Decision::Node(0));
        assert_eq!(s.decide("baz"), Decision::Local);
        assert_eq!(s.decide("zzz"), Decision::Local, "local: *");
        let s = script("V1 { global: *; local: foo; };").unwrap();
        assert_eq!(s.decide("foo"), Decision::Local);
        assert_eq!(s.decide("x"), Decision::Node(0));
        let s = script("V1 { global: foo; };").unwrap();
        assert_eq!(s.decide("x"), Decision::Base);
        // A quoted entry is exact.
        let s = script("V1 { global: \"f*\"; local: *; };").unwrap();
        assert_eq!(s.decide("f*"), Decision::Node(0));
        assert_eq!(s.decide("fx"), Decision::Local);
        // Rule 12: a `.symver` name, by its own node only.
        let s = script("V1 { global: a; local: *; }; V2 { local: b; } V1;").unwrap();
        assert_eq!(s.decide_versioned("b", "V2", false), Some(Decision::Local));
        assert_eq!(s.decide_versioned("b", "V2", true), Some(Decision::Node(1)));
        assert_eq!(s.decide_versioned("c", "V1", false), Some(Decision::Local));
        assert_eq!(s.decide_versioned("a", "V1", false), Some(Decision::Node(0)));
        assert_eq!(s.decide_versioned("a", "V9", false), None);
    }

    // spec: script.md §9.1 — the refusals and syntax errors of a version
    // script.
    #[test]
    fn s9_1_version_script_errors() {
        assert!(matches!(script("V1 { extern \"C++\" { foo; }; };"), Err(ScriptError::Refused(_))));
        assert!(matches!(
            script("V1 { global: foo; }; V2 { local: foo; };"),
            Err(ScriptError::Refused(_))
        ));
        assert!(matches!(
            script("V1 { global: a; }; V1 { global: b; };"),
            Err(ScriptError::Syntax(_))
        ));
        assert!(matches!(script("V2 { global: a; } V1;"), Err(ScriptError::Syntax(_))));
        assert!(matches!(
            script("{ global: a; }; V1 { global: b; };"),
            Err(ScriptError::Syntax(_))
        ));
        assert!(matches!(script("V1 { global: a; }; // c"), Err(ScriptError::Syntax(_))));
        let s = script("# c\nV1 { /* c */ global: extern \"C\" { a; }; b; };").unwrap();
        assert_eq!(s.decide("a"), Decision::Node(0));
        assert_eq!(s.decide("b"), Decision::Node(0));
    }

    // spec: dyn.md D-112 — the line's last form decides, a dynamic list
    // alone when present; the refused forms by name.
    #[test]
    fn d112_the_line_forms() {
        let sl = sline(&["-shared", "-Bsymbolic", "-Bsymbolic-functions"]);
        assert_eq!(sl.form(), Symbolic::Functions);
        assert!(!sl.df_symbolic());
        let sl = sline(&["-shared", "-Bsymbolic-functions", "-Bsymbolic"]);
        assert!(sl.df_symbolic());
        let sl = sline(&["-shared", "-Bsymbolic", "--dynamic-list", "d"]);
        assert!(!sl.df_symbolic(), "a list: no DF_SYMBOLIC");
        let l = DynList::parse(&[("d".into(), "{ fn; d*; };".into())]).unwrap();
        assert!(line_preempts(&sl, Some(&l), STT_FUNC, "fn", Format::Mortar));
        assert!(!line_preempts(&sl, Some(&l), STT_FUNC, "g", Format::Mortar));
        assert!(line_preempts(&sl, Some(&l), STT_TLS, "g", Format::Gnu), "GNU's TLS form");
        let sl = sline(&["-shared", "-Bsymbolic-functions"]);
        assert!(!line_preempts(&sl, None, STT_FUNC, "f", Format::Mortar));
        assert!(line_preempts(&sl, None, STT_NOTYPE, "f", Format::Mortar));
        assert!(!line_preempts(&sl, None, STT_NOTYPE, "f", Format::Gnu));
        for w in ["--dynamic-list-data", "-Bsymbolic-non-weak-functions", "--dynamic-list-cpp-new"]
        {
            assert!(!sline(&["-shared", w]).outside.is_empty(), "{w}");
        }
    }

    // spec: plumb.md §12.5 P-D22 — `GNU_PROPERTY_1_NEEDED` read from a
    // `.note.gnu.property` of either padding.
    #[test]
    fn s12_5_needed_bits() {
        let mut n = Vec::new();
        for w in [4u32, 16, 5] {
            n.extend(w.to_le_bytes());
        }
        n.extend(b"GNU\0");
        n.extend(GNU_PROPERTY_1_NEEDED.to_le_bytes());
        n.extend(4u32.to_le_bytes());
        n.extend(1u32.to_le_bytes());
        n.extend(0u32.to_le_bytes());
        assert_eq!(needed_bits(&n, 8), 1);
        assert_eq!(needed_bits(&n[..20], 8), 0, "truncated: nothing, no panic");
    }
}
