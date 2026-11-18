//! The input set (`spec/plumb.md` §3.8, WP-156): which files a link must
//! contain, derived from the command line, the input files and the
//! archives' members by `spec/resolve.md` §4, and compared with the files
//! the map says are in the link.
//!
//! - **M11 (missing)**: the closure requires a file the link lacks.
//! - **M12 (extra)**: a file is in the link that no root or live reference
//!   reaches; for GNU ld's map also a reason its preamble prints that the
//!   inputs do not bear out.
//!
//! mortar's map is held to the closure of resolve §4.1 exactly. A
//! reference's map is held to what every linker of §16 does: a member it
//! included is reached by some reference of the link, and a strong
//! reference left with no definition in the link is missing when a member
//! defines it and the linker's own rule (GNU ld's walk; lld without
//! `--fortran-common` ignoring commons) would have extracted it.

use crate::hasher::{HashMap, HashSet};
use crate::line::{Files, Item, Line};
use crate::map::{Format, IncludedBy, Map, MapFile};
use crate::report::{Finding, Rule};
use crate::store::{At, Loc};
use mortar_elf::consts::{
    SHF_ALLOC, SHF_MERGE, SHN_X86_64_LCOMMON, STB_LOCAL, STB_WEAK, STT_FILE, STT_FUNC,
    STT_GNU_IFUNC, STT_SECTION,
};
use mortar_elf::{Object, SymbolSection};
use std::fmt::Write as _;

/// The rule ids (§7.1).
pub(crate) const MISSING: Rule = Rule::Map(11);
pub(crate) const EXTRA: Rule = Rule::Map(12);

/// A mention's kind, as bits.
const DEF: u8 = 1;
const WEAK: u8 = 2;
const COMMON: u8 = 4;
/// Not `STT_FUNC`/`STT_GNU_IFUNC` (resolve §4.5's data definition).
const DATA: u8 = 8;
/// A non-weak undefined mention (after `--wrap`).
const UNDEF: u8 = 16;
/// A weak undefined mention (no edge; GNU's printed reason may name it).
const WUNDEF: u8 = 32;
/// The plain name of a shared object's strong versioned reference
/// (`name` of `name@V`): no edge (resolve §9.2), but mold extracts by it
/// (§10.2 A49), so an M12 it alone explains says so.
const VPLAIN: u8 = 64;
/// A strong undefined mention of an `--as-needed` command-line shared
/// object: an edge exactly when the object is a source of D-12's last
/// stage (in N, named by the output's `DT_NEEDED`; WP-171).
const ASN: u8 = 128;

/// The end of an M12 that only [`VPLAIN`] mentions explain (A49).
pub(crate) const VERSIONED_PLAIN: &str = "only a shared object's versioned reference reaches it, by its plain name (resolve §9.2: not an edge)";
/// The end of an M12 that only [`ASN`] mentions of an object that is not
/// a source explain (A70 on lld).
pub(crate) const UNNEEDED_AS_NEEDED: &str = "only the references of an --as-needed shared object the output does not need reach it (dyn.md D-12: not an edge)";

const NONE: u32 = u32::MAX;

/// What the rule made of a link: its findings and its coverage line.
#[derive(Debug, Default)]
pub(crate) struct Verdict {
    pub findings: Vec<Finding>,
    pub coverage: String,
}

/// Judges the input set of `map`, the link of `words` (the mortar map's
/// `arg` records or the caller's line), reading inputs through `files`.
/// With no line, the coverage says so and nothing is judged.
/// `staged`: the inputs as D-12's replay left them (its last stage's
/// sources), read once for both.
pub(crate) fn judge(
    map: &Map,
    words: Option<&[String]>,
    files: &dyn Files,
    staged: Option<Stager>,
) -> Verdict {
    let Some(words) = words else {
        return not_judged("no command line (a reference's map with no --inputs-from line)");
    };
    if map.format == Format::Mortar && map.files.is_empty() {
        return not_judged("the map names no file");
    }
    let (line, set) = match staged {
        Some(st) => (st.line, st.set),
        None => match read_line(words, files) {
            Ok(x) => x,
            Err(e) => return not_judged(&e),
        },
    };
    let mut v =
        if map.format == Format::Mortar { set.strict(map) } else { set.loose(map, &line, words) };
    if set.sources.is_none() && set.files.iter().any(|f| f.mentions.iter().any(|m| m.1 & ASN != 0))
    {
        v.coverage.push_str(
            "; an --as-needed shared object's references open (D-12's stages not replayed)",
        );
    }
    v
}

/// The line and its inputs, read, or why they are not judged.
fn read_line(words: &[String], files: &dyn Files) -> Result<(Line, Inputs), String> {
    let line = Line::parse(words, files);
    if let Some(p) = line.problems.first() {
        return Err(format!("the line is not read: {p}"));
    }
    if let Some(u) = line.unmodelled.first() {
        return Err(format!("{u} is not modelled"));
    }
    let set = Inputs::read(&line, words, files)?;
    Ok((line, set))
}

/// D-12's stages over the line's inputs (WP-171): resolve §4.1's closure
/// with a stage's sources' strong references as edges, for the dyn
/// model's replay of D-11 ([`crate::dynmodel::Staging`]).
pub(crate) struct Stager {
    line: Line,
    set: Inputs,
    /// `first(K)` of resolve §4.1 (the lower bound's).
    first: Vec<u32>,
    /// The last stage's live files.
    live: Vec<bool>,
    index: HashMap<Ident, Vec<usize>>,
}

impl Stager {
    /// The stager of a line that names a shared object, `None` for any
    /// other (a static link pays nothing) or a line not read.
    pub(crate) fn new(words: &[String], files: &dyn Files) -> Option<Stager> {
        let line = Line::parse(words, files);
        if !line.items.iter().any(|i| matches!(i, Item::Shared { .. })) {
            return None;
        }
        let (line, set) = read_line(words, files).ok()?;
        let (first, _) = set.targets(&set.maybe_defined);
        let index = set.by_ident().into_iter().map(|(k, v)| (k.clone(), v)).collect();
        Some(Stager { line, set, first, live: Vec::new(), index })
    }

    /// The stager of an output with no `DT_NEEDED`: no source.
    pub(crate) fn without_sources(mut self) -> Stager {
        self.set.sources = Some(vec![false; self.set.files.len()]);
        self
    }
}

impl crate::dynmodel::Staging for Stager {
    fn stage(
        &mut self,
        objs: &[crate::locate::Obj<'_>],
        sources: &std::collections::BTreeSet<String>,
    ) -> Vec<bool> {
        let src: Vec<bool> = self
            .set
            .files
            .iter()
            .map(|f| {
                f.kind == Kind::Shared && f.soname.as_ref().is_some_and(|n| sources.contains(n))
            })
            .collect();
        self.set.sources = Some(src);
        self.live = self.set.closure(false, true).0;
        objs.iter()
            .map(|o| {
                let id = Ident { path: norm(&o.path), member: o.member.clone() };
                self.index.get(&id).is_none_or(|v| v.iter().any(|&i| self.live[i]))
            })
            .collect()
    }

    fn lazy_first(&self, name: &str, ver: Option<&str>) -> bool {
        let key = match ver {
            Some(v) => format!("{name}@{v}"),
            None => name.to_string(),
        };
        let Some(k) = self.set.find(&key) else { return false };
        let f = self.first.get(k as usize).copied().unwrap_or(NONE);
        f != NONE
            && self.set.files[f as usize].lazy
            && !self.live.get(f as usize).copied().unwrap_or(true)
    }
}

/// A missing key's demand: the demanding file, its name, the members
/// that define the key.
type Demand = (usize, String, Vec<usize>);

fn not_judged(why: &str) -> Verdict {
    Verdict { findings: Vec::new(), coverage: format!("input set: not judged ({why})") }
}

/// The identity of a file for comparison (resolve §4.6, `spec/model.md`
/// §7.1): its path normalized lexically, and for a member its name and
/// header offset. A map that gives no offset matches any member of the
/// name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Ident {
    path: String,
    member: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// File 0: the line's own definitions (`--defsym`, a script's
    /// assignments) and roots.
    Zero,
    Object,
    Member,
    Shared,
}

#[derive(Debug)]
struct File {
    kind: Kind,
    ident: Ident,
    /// The member's header offset.
    offset: Option<u64>,
    /// The line item it comes from (its position, GNU's walk).
    item: usize,
    group: Option<usize>,
    lazy: bool,
    whole: bool,
    /// `(key, bits)`, each key once per bit set.
    mentions: Vec<(u32, u8)>,
    /// A thin archive's member by its own path (GNU ld's map names it so).
    alt: Option<Ident>,
    /// It has a section every map format lists when the file is in the
    /// link: allocated, not mergeable, not `.eh_frame` (mold lists
    /// neither, §3.6).
    listed: bool,
    /// A shared object's soname, or its name as the line writes it
    /// (dyn.md D-19a's identity).
    soname: Option<String>,
}

/// A root key and the item position it is read at (GNU ld reads `EXTERN`
/// and `--defsym` in place; the others before every input).
#[derive(Debug, Clone)]
struct Root {
    key: u32,
    at: usize,
    /// Where the line says it (the message).
    by: String,
    /// Not certain to be a root (resolve §4.2's rows this reader does not
    /// read exactly): followed in the upper bound only.
    uncertain: bool,
    /// GNU ld's position is not known: its demands are not judged.
    no_position: bool,
}

/// Which files' [`ASN`] mentions are edges.
#[derive(Clone, Copy)]
enum Asn<'a> {
    None,
    All,
    /// By file: the sources of D-12's last stage.
    Of(&'a [bool]),
}

impl Asn<'_> {
    fn of(self, f: usize) -> bool {
        match self {
            Asn::None => false,
            Asn::All => true,
            Asn::Of(s) => s.get(f).copied().unwrap_or(false),
        }
    }
}

struct Inputs {
    files: Vec<File>,
    /// The sources of D-12's last stage, by file ([`Stager`]); `None`
    /// without a replay.
    sources: Option<Vec<bool>>,
    keys: HashMap<Box<[u8]>, u32>,
    roots: Vec<Root>,
    /// Keys file 0 may define (a script's assignments, an expression this
    /// reader does not read): defined in the lower bound only.
    maybe_defined: Vec<u32>,
    /// `--export-dynamic-symbol`: roots this reader does not expand.
    export_globs: bool,
    lld_fortran_common: bool,
}

/// A file up to this size is read whole, once (a caller's [`Files`] may
/// read a whole file for every positional read); a larger one by
/// position (§8.1).
const WHOLE: u64 = 64 << 20;

/// [`crate::Source`] over [`Files`]: the files read by position, or held
/// whole while their members are read when small.
struct Positional<'f> {
    files: &'f dyn Files,
    paths: Vec<String>,
    lens: Vec<u64>,
    whole: Vec<Option<Vec<u8>>>,
}

impl crate::Source for Positional<'_> {
    fn count(&self) -> usize {
        self.paths.len()
    }
    fn path(&self, i: usize) -> &str {
        self.paths.get(i).map_or("", String::as_str)
    }
    fn len(&self, i: usize) -> u64 {
        self.lens.get(i).copied().unwrap_or(0)
    }
    fn read_at(&self, i: usize, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        if let Some(Some(b)) = self.whole.get(i) {
            let got = usize::try_from(offset)
                .ok()
                .and_then(|at| b.get(at..at.checked_add(buf.len())?))
                .ok_or_else(|| {
                    format!("{}: {} bytes at {offset:#x} are past its end", self.path(i), buf.len())
                })?;
            buf.copy_from_slice(got);
            return Ok(());
        }
        self.files.read_at(self.path(i), offset, buf)
    }
}

impl Positional<'_> {
    fn add(&mut self, path: &str) -> Result<usize, String> {
        let len = self.files.len(path).ok_or_else(|| format!("{path}: cannot be read"))?;
        let whole = if len <= WHOLE {
            let b = self.files.read(path).ok_or_else(|| format!("{path}: cannot be read"))?;
            Some(b)
        } else {
            None
        };
        self.paths.push(path.to_string());
        self.lens.push(whole.as_ref().map_or(len, |b| b.len() as u64));
        self.whole.push(whole);
        Ok(self.paths.len() - 1)
    }
}

/// A path normalized lexically: `.` and empty components dropped, `..`
/// removing the component before it.
pub(crate) fn norm(p: &str) -> String {
    let abs = p.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." if out.last().is_some_and(|l| *l != "..") => {
                out.pop();
            }
            ".." if abs => {}
            c => out.push(c),
        }
    }
    let j = out.join("/");
    if abs {
        format!("/{j}")
    } else if j.is_empty() {
        ".".into()
    } else {
        j
    }
}

fn map_ident(f: &MapFile) -> Ident {
    Ident { path: norm(&f.path), member: f.member.clone() }
}

impl Inputs {
    fn key(&mut self, name: &[u8]) -> u32 {
        if let Some(&k) = self.keys.get(name) {
            return k;
        }
        let k = u32::try_from(self.keys.len()).unwrap_or(NONE - 1);
        self.keys.insert(name.into(), k);
        k
    }

    fn find(&self, name: &str) -> Option<u32> {
        self.keys.get(name.as_bytes()).copied()
    }

    /// The names of the keys, for messages.
    fn names(&self) -> Vec<&[u8]> {
        let mut v: Vec<&[u8]> = vec![&[]; self.keys.len()];
        for (n, &k) in &self.keys {
            if let Some(s) = v.get_mut(k as usize) {
                *s = n;
            }
        }
        v
    }

    #[allow(clippy::too_many_lines)] // one pass over the line's items
    fn read(line: &Line, words: &[String], files: &dyn Files) -> Result<Inputs, String> {
        let mut s = Inputs {
            files: Vec::new(),
            sources: None,
            keys: HashMap::default(),
            roots: Vec::new(),
            maybe_defined: Vec::new(),
            export_globs: line.export_symbols,
            lld_fortran_common: words
                .iter()
                .any(|w| w == "--fortran-common" || w == "-fortran-common"),
        };
        let wrap = crate::wrap::Wrap::new(line.wrap.iter().cloned());
        // File 0.
        let mut zero = File {
            kind: Kind::Zero,
            ident: Ident { path: String::new(), member: None },
            offset: None,
            item: 0,
            group: None,
            lazy: false,
            whole: false,
            mentions: Vec::new(),
            alt: None,
            listed: false,
            soname: None,
        };
        // The roots of resolve §4.2.
        let entry = line.entry.clone().or_else(|| line.script_entry.clone());
        match entry {
            Some(e) => {
                let k = s.key(e.as_bytes());
                s.roots.push(Root {
                    key: k,
                    at: 0,
                    by: "the entry".into(),
                    uncertain: false,
                    no_position: false,
                });
            }
            None if !line.shared && !line.relocatable => {
                let k = s.key(b"_start");
                s.roots.push(Root {
                    key: k,
                    at: 0,
                    by: "the default entry".into(),
                    uncertain: false,
                    no_position: false,
                });
            }
            None => {}
        }
        for u in &line.undefined {
            let k = s.key(u.as_bytes());
            s.roots.push(Root {
                key: k,
                at: 0,
                by: format!("-u {u}"),
                uncertain: false,
                no_position: false,
            });
        }
        for u in &line.require_defined {
            let k = s.key(u.as_bytes());
            s.roots.push(Root {
                key: k,
                at: 0,
                by: format!("--require-defined {u}"),
                uncertain: false,
                no_position: false,
            });
        }
        for (x, at) in &line.externs {
            let k = s.key(x.as_bytes());
            s.roots.push(Root {
                key: k,
                at: *at,
                by: format!("EXTERN({x})"),
                uncertain: false,
                no_position: false,
            });
        }
        for (d, at) in &line.defsym {
            let (target, names, exact) = defsym(d);
            let Some(target) = target else {
                return Err(format!("--defsym {d}: not read"));
            };
            let k = s.key(target.as_bytes());
            if exact {
                zero.mentions.push((k, DEF | DATA));
            } else {
                s.maybe_defined.push(k);
            }
            for n in names {
                let k = s.key(n.as_bytes());
                s.roots.push(Root {
                    key: k,
                    at: *at,
                    by: format!("--defsym {d}"),
                    uncertain: !exact,
                    no_position: false,
                });
            }
        }
        for t in &line.scripts {
            let text = files.read(t).ok_or_else(|| format!("{t}: cannot be read"))?;
            let sc = script(&String::from_utf8_lossy(&text)).map_err(|e| format!("{t}: {e}"))?;
            for x in sc.roots {
                let k = s.key(x.as_bytes());
                s.roots.push(Root {
                    key: k,
                    at: 0,
                    by: t.clone(),
                    uncertain: false,
                    no_position: true,
                });
            }
            if let Some(e) = sc.entry
                && line.entry.is_none()
            {
                let k = s.key(e.as_bytes());
                s.roots.push(Root {
                    key: k,
                    at: 0,
                    by: format!("ENTRY in {t}"),
                    uncertain: false,
                    no_position: true,
                });
            }
            for x in sc.assigned {
                let k = s.key(x.as_bytes());
                s.maybe_defined.push(k);
            }
        }
        s.files.push(zero);
        for (item, it) in line.items.iter().enumerate() {
            match it {
                Item::Shared { path, named, as_needed, group } => {
                    let bytes =
                        files.read(path).ok_or_else(|| format!("{path}: cannot be read"))?;
                    let d = crate::dynmodel::Dso::read(path, bytes, true)?;
                    let mut mentions = Vec::new();
                    let mut defs: Vec<(&String, &crate::dynmodel::SoDef)> =
                        d.defs.iter().flat_map(|(n, v)| v.iter().map(move |x| (n, x))).collect();
                    defs.sort_by(|a, b| a.0.cmp(b.0));
                    for (name, sd) in defs {
                        if !sd.visible() {
                            continue;
                        }
                        let mut bits = DEF;
                        if sd.bind == STB_WEAK {
                            bits |= WEAK;
                        }
                        if !matches!(sd.ty, STT_FUNC | STT_GNU_IFUNC) {
                            bits |= DATA;
                        }
                        let key = match (&sd.version, sd.default) {
                            (Some(v), false) => format!("{name}@{v}"),
                            _ => name.clone(),
                        };
                        let k = s.key(key.as_bytes());
                        mentions.push((k, bits));
                        if let (Some(v), true) = (&sd.version, sd.default) {
                            let k = s.key(format!("{name}@{v}").as_bytes());
                            mentions.push((k, bits));
                        }
                    }
                    // resolve §4.4: a non-as-needed command-line shared
                    // object's strong references are edges (dyn.md
                    // "Archive extraction by the references of shared
                    // objects"; an as-needed mention's are not). §9.2: a
                    // reference requiring version V has the key `name@V`,
                    // which only a hidden `name@V` or a default `name@@V`
                    // defines; never an object's plain `name`.
                    for r in &d.refs {
                        let key = match &r.version {
                            Some(v) => format!("{}@{v}", r.name),
                            None => r.name.clone(),
                        };
                        let k = s.key(key.as_bytes());
                        let bits = if r.weak {
                            WUNDEF
                        } else if *as_needed {
                            ASN
                        } else {
                            UNDEF
                        };
                        mentions.push((k, bits));
                        // A49: mold extracts by it, `--as-needed` or not.
                        if r.version.is_some() && !r.weak {
                            mentions.push((s.key(r.name.as_bytes()), VPLAIN));
                        }
                    }
                    let soname = if d.soname == *path { named.clone() } else { d.soname.clone() };
                    s.files.push(File {
                        kind: Kind::Shared,
                        ident: Ident { path: norm(path), member: None },
                        offset: None,
                        item,
                        group: *group,
                        lazy: false,
                        whole: false,
                        mentions,
                        alt: None,
                        listed: false,
                        soname: Some(soname),
                    });
                }
                Item::Relocatable { path, group, whole, lib } => {
                    let mut src = Positional {
                        files,
                        paths: Vec::new(),
                        lens: Vec::new(),
                        whole: Vec::new(),
                    };
                    let i = src.add(path)?;
                    let head = files.head(path, 8).unwrap_or_default();
                    if head == b"!<arch>\n" || head == b"!<thin>\n" {
                        let members = crate::store::members_in_order(&src, i)
                            .map_err(|e| format!("{path}: {e}"))?;
                        for (name, offset, loc) in members {
                            let mut alt = None;
                            let at = match loc {
                                Loc::Inline { offset, size } => {
                                    At { file: i, base: offset, len: size }
                                }
                                Loc::External(stored) => {
                                    let p = crate::locate::thin_member_path(path, &stored);
                                    let j = src.add(&p)?;
                                    alt = Some(Ident { path: norm(&p), member: None });
                                    At { file: j, base: 0, len: src.lens[j] }
                                }
                                Loc::ExternalMember => {
                                    return Err(format!(
                                        "{path}: a thin archive's member that is a member of another archive"
                                    ));
                                }
                            };
                            let member = String::from_utf8_lossy(&name).into_owned();
                            let (mentions, listed) = s
                                .object(&src, at, &wrap)
                                .map_err(|e| format!("{path}({member}): {e}"))?;
                            if at.file != i {
                                src.whole[at.file] = None;
                            }
                            s.files.push(File {
                                kind: Kind::Member,
                                ident: Ident { path: norm(path), member: Some(member) },
                                offset: Some(offset),
                                item,
                                group: *group,
                                lazy: !*whole,
                                whole: *whole,
                                mentions,
                                alt,
                                listed,
                                soname: None,
                            });
                        }
                    } else {
                        let len = src.lens[i];
                        let (mentions, listed) = s
                            .object(&src, At { file: i, base: 0, len }, &wrap)
                            .map_err(|e| format!("{path}: {e}"))?;
                        s.files.push(File {
                            kind: Kind::Object,
                            ident: Ident { path: norm(path), member: None },
                            offset: None,
                            item,
                            group: *group,
                            lazy: *lib,
                            whole: false,
                            mentions,
                            alt: None,
                            listed,
                            soname: None,
                        });
                    }
                }
            }
        }
        Ok(s)
    }

    /// An object's non-local mentions (resolve §4.1, the I-file summary):
    /// its own symbol table, never an archive's index.
    fn object(
        &mut self,
        src: &dyn crate::Source,
        at: At,
        wrap: &crate::wrap::Wrap,
    ) -> Result<(Vec<(u32, u8)>, bool), String> {
        let skel = crate::store::skeleton(src, at)?;
        let view = skel.view(at.len);
        let o = Object::parse_view(view).map_err(|e| e.to_string())?;
        let listed = o.sections().any(|sec| {
            sec.flags() & SHF_ALLOC != 0
                && sec.flags() & SHF_MERGE == 0
                && sec.name().is_ok_and(|n| n != b".eh_frame")
        });
        let Some(t) = o.symbols().map_err(|e| e.to_string())? else {
            return Ok((Vec::new(), listed));
        };
        let mut out = Vec::new();
        for (idx, sym) in t.iter() {
            if idx.0 == 0 || sym.binding() == STB_LOCAL {
                continue;
            }
            let ty = sym.sym_type();
            if matches!(ty, STT_SECTION | STT_FILE) {
                continue;
            }
            let name = t.name(&sym).map_err(|e| e.to_string())?;
            if name.is_empty() {
                continue;
            }
            let weak = sym.binding() == STB_WEAK;
            let data = if matches!(ty, STT_FUNC | STT_GNU_IFUNC) { 0 } else { DATA };
            match t.section_of(idx, &sym).map_err(|e| e.to_string())? {
                SymbolSection::Undefined => {
                    // resolve §7.1: the undefined mention is redirected;
                    // §9.4: `foo@V` is its own key.
                    let n = redirect(name, wrap);
                    let k = self.key(&n);
                    out.push((k, if weak { WUNDEF } else { UNDEF }));
                }
                SymbolSection::Common | SymbolSection::Reserved(SHN_X86_64_LCOMMON) => {
                    let k = self.key(name);
                    out.push((k, COMMON | if weak { WEAK } else { 0 }));
                }
                _ => {
                    let bits = DEF | data | if weak { WEAK } else { 0 };
                    // §9.4: `foo@@V` is `foo`, and binds `foo@V` too.
                    if let Some(at) = find(name, b"@@") {
                        let base = &name[..at];
                        let k = self.key(base);
                        out.push((k, bits));
                        let mut v = base.to_vec();
                        v.extend_from_slice(&name[at + 1..]);
                        let k = self.key(&v);
                        out.push((k, bits));
                    } else {
                        let k = self.key(name);
                        out.push((k, bits));
                    }
                }
            }
        }
        Ok((out, listed))
    }

    /// `first(K)` and `cfirst(K)` of resolve §4.1, file 0 defining
    /// `extra` as well.
    fn targets(&self, extra: &[u32]) -> (Vec<u32>, Vec<u32>) {
        let n = self.keys.len();
        let mut first = vec![NONE; n];
        let mut cfirst = vec![NONE; n];
        for &k in extra {
            first[k as usize] = 0;
            cfirst[k as usize] = 0;
        }
        for (i, f) in self.files.iter().enumerate() {
            let i = i as u32;
            for &(k, b) in &f.mentions {
                let k = k as usize;
                if b & (DEF | COMMON) != 0 && first[k] == NONE {
                    first[k] = i;
                }
                let target = if f.lazy {
                    b & DEF != 0 && b & (WEAK | COMMON) == 0 && b & DATA != 0
                } else {
                    b & DEF != 0 && b & (WEAK | COMMON) == 0
                };
                if target && cfirst[k] == NONE {
                    cfirst[k] = i;
                }
            }
        }
        (first, cfirst)
    }

    /// The closure of resolve §4.1 over the roots of §4.2: the live files,
    /// and for each extracted file the pulling file and key. `upper`:
    /// the uncertain roots followed and the uncertain definitions not
    /// counted; else the reverse. `commons`: the edges of a common.
    fn closure(&self, upper: bool, commons: bool) -> (Vec<bool>, Vec<(u32, u32)>) {
        self.closure_with(upper, commons, false, self.asn(upper))
    }

    /// Which [`ASN`] mentions are edges in a bound: the last stage's
    /// sources; with no replay of D-12's stages (the output's `DT_NEEDED`
    /// unread), every one in the upper bound and none in the lower.
    fn asn(&self, upper: bool) -> Asn<'_> {
        match (&self.sources, upper) {
            (Some(s), _) => Asn::Of(s),
            (None, true) => Asn::All,
            (None, false) => Asn::None,
        }
    }

    /// [`Self::closure`]; `plain`: the [`VPLAIN`] mentions edges too;
    /// `asn`: which [`ASN`] mentions are.
    fn closure_with(
        &self,
        upper: bool,
        commons: bool,
        plain: bool,
        asn: Asn<'_>,
    ) -> (Vec<bool>, Vec<(u32, u32)>) {
        let extra: &[u32] = if upper { &[] } else { &self.maybe_defined };
        let (first, cfirst) = self.targets(extra);
        let n = self.files.len();
        let mut live: Vec<bool> = self.files.iter().map(|f| !f.lazy).collect();
        let mut why = vec![(NONE, NONE); n];
        let mut work: Vec<u32> = (0..n as u32).filter(|&i| live[i as usize]).collect();
        let mut pull = |t: u32, from: u32, k: u32, live: &mut Vec<bool>, work: &mut Vec<u32>| {
            if t != NONE && !live[t as usize] {
                live[t as usize] = true;
                why[t as usize] = (from, k);
                work.push(t);
            }
        };
        for r in &self.roots {
            if r.uncertain && !upper {
                continue;
            }
            pull(first[r.key as usize], 0, r.key, &mut live, &mut work);
        }
        while let Some(f) = work.pop() {
            let src = asn.of(f as usize);
            for &(k, b) in &self.files[f as usize].mentions {
                if b & UNDEF != 0 || (plain && b & VPLAIN != 0) || (src && b & ASN != 0) {
                    pull(first[k as usize], f, k, &mut live, &mut work);
                }
                if commons && b & COMMON != 0 {
                    pull(cfirst[k as usize], f, k, &mut live, &mut work);
                }
            }
        }
        (live, why)
    }

    fn label(&self, i: usize) -> String {
        let f = &self.files[i];
        match (&f.kind, &f.ident.member) {
            (Kind::Zero, _) => "the command line".into(),
            (_, Some(m)) => format!("{}({m})", f.ident.path),
            (_, None) => f.ident.path.clone(),
        }
    }

    fn by_ident(&self) -> HashMap<&Ident, Vec<usize>> {
        let mut m: HashMap<&Ident, Vec<usize>> = HashMap::default();
        for (i, f) in self.files.iter().enumerate() {
            if f.kind != Kind::Zero {
                m.entry(&f.ident).or_default().push(i);
                if let Some(a) = &f.alt {
                    m.entry(a).or_default().push(i);
                }
            }
        }
        m
    }

    /// mortar's map: the closure exactly (§3.8 rule 1).
    #[allow(clippy::too_many_lines)] // one pass per bound and its findings
    fn strict(&self, map: &Map) -> Verdict {
        let (lo, why) = self.closure(false, true);
        let (hi, _) = self.closure(true, true);
        let (hv, _) = self.closure_with(true, true, true, self.asn(true));
        let (ha, _) = self.closure_with(true, true, false, Asn::All);
        let names = self.names();
        let name = |k: u32| {
            String::from_utf8_lossy(names.get(k as usize).copied().unwrap_or(b"")).into_owned()
        };
        let index = self.by_ident();
        let mut v = Verdict::default();
        let mut in_map = vec![false; self.files.len()];
        for mf in &map.files {
            let id = map_ident(mf);
            let cands: Vec<usize> = index
                .get(&id)
                .map(|c| {
                    c.iter()
                        .copied()
                        .filter(|&i| {
                            mf.member_offset.is_none() || self.files[i].offset == mf.member_offset
                        })
                        .collect()
                })
                .unwrap_or_default();
            if cands.is_empty() {
                v.findings.push(Finding::new(
                    EXTRA,
                    format!("{mf} is in the link and is not an input of its line"),
                ));
                continue;
            }
            for &i in &cands {
                in_map[i] = true;
            }
            if !self.export_globs && !cands.iter().any(|&i| hi[i]) {
                let m = if cands.iter().any(|&i| hv[i]) {
                    format!("{mf} is in the link and {VERSIONED_PLAIN}")
                } else if cands.iter().any(|&i| ha[i]) {
                    format!("{mf} is in the link and {UNNEEDED_AS_NEEDED}")
                } else {
                    format!(
                        "{mf} is in the link and no root or live reference reaches it (resolve §4.1)"
                    )
                };
                v.findings.push(Finding::new(EXTRA, m));
            }
        }
        // An identity is required when any of its mentions is (an archive
        // named twice is one file).
        let mut required: HashMap<&Ident, usize> = HashMap::default();
        for (i, f) in self.files.iter().enumerate() {
            if matches!(f.kind, Kind::Object | Kind::Member) && lo[i] {
                required.entry(&f.ident).or_insert(i);
            }
        }
        let mut present: HashMap<&Ident, bool> = HashMap::default();
        for (i, f) in self.files.iter().enumerate() {
            if in_map[i] {
                present.insert(&f.ident, true);
            }
        }
        let mut missing: Vec<usize> =
            required.iter().filter(|(id, _)| !present.contains_key(*id)).map(|(_, &i)| i).collect();
        missing.sort_unstable();
        for i in missing {
            let (from, k) = why[i];
            let reason = if from == NONE {
                "it is live from the start (resolve §4.2)".to_string()
            } else if from == 0 {
                let by = self.roots.iter().find(|r| r.key == k).map_or("a root", |r| r.by.as_str());
                format!("{by} roots `{}` (resolve §4.2) and it is first(`{}`)", name(k), name(k))
            } else if self.files[from as usize].mentions.iter().any(|m| m.0 == k && m.1 & ASN != 0)
                && !self.files[from as usize].mentions.iter().any(|m| m.0 == k && m.1 & UNDEF != 0)
            {
                format!(
                    "{} refers to `{}` and it is the target (resolve §4.1; a source of D-12's last stage)",
                    self.label(from as usize),
                    name(k)
                )
            } else {
                format!(
                    "{} refers to `{}` and it is the target (resolve §4.1)",
                    self.label(from as usize),
                    name(k)
                )
            };
            let mut f =
                Finding::new(MISSING, format!("{} is not in the link: {reason}", self.label(i)));
            if k != NONE {
                f.symbol = Some(name(k));
            }
            v.findings.push(f);
        }
        let total =
            self.files.iter().filter(|f| matches!(f.kind, Kind::Object | Kind::Member)).count();
        let open = self
            .files
            .iter()
            .enumerate()
            .filter(|(i, f)| matches!(f.kind, Kind::Object | Kind::Member) && hi[*i] != lo[*i])
            .count();
        let judged = if self.export_globs { 0 } else { total - open };
        let mut c = format!(
            "input set: files judged {judged} of {total} (in the link {}, required {})",
            map.files.len(),
            lo.iter()
                .zip(&self.files)
                .filter(|(l, f)| **l && matches!(f.kind, Kind::Object | Kind::Member))
                .count()
        );
        if self.export_globs {
            c.push_str("; extra not judged: --export-dynamic-symbol roots not expanded");
        }
        v.coverage = c;
        v
    }

    /// [`crate::wrap::WRAPPED_ONLY`] when lazy file `i` defines a
    /// `--wrap` word SYM and a reached file's undefined mention binds to
    /// `__wrap_SYM`: only wrapped references name it (resolve §7.1; lld
    /// extracts it, A83). Empty otherwise.
    fn wrapped_only(&self, i: usize, reached: &[bool], line: &Line) -> &'static str {
        let defines =
            |f: usize, k: u32| self.files[f].mentions.iter().any(|&(x, b)| x == k && b & DEF != 0);
        let wanted = |k: u32| {
            (0..self.files.len()).any(|j| {
                reached[j]
                    && self.files[j]
                        .mentions
                        .iter()
                        .any(|&(x, b)| x == k && b & (UNDEF | WUNDEF) != 0)
            })
        };
        let hit = line.wrap.iter().any(|w| {
            let key = |n: &str| self.keys.get(n.as_bytes()).copied();
            key(w).is_some_and(|k| defines(i, k)) && key(&format!("__wrap_{w}")).is_some_and(wanted)
        });
        if hit { crate::wrap::WRAPPED_ONLY } else { "" }
    }

    /// A reference's map (§3.8 rule 2).
    #[allow(clippy::too_many_lines)]
    fn loose(&self, map: &Map, line: &Line, words: &[String]) -> Verdict {
        let names = self.names();
        // Whether the map would name file `i` were it in the link: lld's
        // and mold's maps leave out what --gc-sections removed, and mold's
        // a file with only mergeable or `.eh_frame` sections (§3.6).
        let gc = words.iter().any(|w| w == "--gc-sections" || w == "-gc-sections");
        let reliable = |i: usize| match map.format {
            Format::Lld => !gc,
            Format::Mold => !gc && self.files[i].listed,
            _ => true,
        };
        let name = |k: u32| {
            String::from_utf8_lossy(names.get(k as usize).copied().unwrap_or(b"")).into_owned()
        };
        let index = self.by_ident();
        let mut v = Verdict::default();
        let n = self.files.len();
        // The files the map says are in the link.
        let mut present = vec![false; self.files.len()];
        let mut unmatched = 0usize;
        let mut seen: HashSet<Ident> = HashSet::default();
        let mut listed: Vec<MapFile> = Vec::new();
        for mf in map.files.iter().chain(map.included.iter().map(|i| &i.file)) {
            if mf.path.starts_with('<') || !seen.insert(map_ident(mf)) {
                continue;
            }
            listed.push(mf.clone());
        }
        for mf in &listed {
            match index.get(&map_ident(mf)) {
                Some(c) => {
                    for &i in c {
                        present[i] = true;
                    }
                }
                None if mf.member.is_some() => unmatched += 1,
                None => {}
            }
        }
        // A file live from the start is in the link whatever the map says
        // (the rest of the rule reads it so); one the map leaves out is
        // missing.
        let in_link: Vec<bool> =
            present.iter().zip(&self.files).map(|(p, f)| *p || !f.lazy).collect();
        let mut gone: HashMap<&Ident, usize> = HashMap::default();
        for (i, f) in self.files.iter().enumerate() {
            if matches!(f.kind, Kind::Object | Kind::Member) && !f.lazy {
                gone.entry(&f.ident).or_insert(i);
            }
        }
        for (i, f) in self.files.iter().enumerate() {
            if present[i] {
                gone.remove(&f.ident);
            }
        }
        let mut gone: Vec<usize> = gone.into_values().collect();
        gone.sort_unstable();
        let unlisted = gone.iter().filter(|&&i| !reliable(i)).count();
        gone.retain(|&i| reliable(i));
        for i in gone {
            v.findings.push(Finding::new(
                MISSING,
                format!(
                    "{} is not in the link: it is live from the start (resolve §4.2)",
                    self.label(i)
                ),
            ));
        }
        // Which keys the link defines, and its lazy definers by key.
        let nk = self.keys.len();
        let mut defined = vec![false; nk];
        let mut strong = vec![false; nk];
        for &k in &self.maybe_defined {
            defined[k as usize] = true;
            strong[k as usize] = true;
        }
        let mut lazy_defs: HashMap<u32, Vec<usize>> = HashMap::default();
        for (i, f) in self.files.iter().enumerate() {
            for &(k, b) in &f.mentions {
                if b & (DEF | COMMON) == 0 {
                    continue;
                }
                if in_link[i] {
                    defined[k as usize] = true;
                    if b & DEF != 0 && b & WEAK == 0 {
                        strong[k as usize] = true;
                    }
                }
                if f.lazy {
                    lazy_defs.entry(k).or_default().push(i);
                }
            }
        }
        // Extra: an in-link lazy file no root or reference reaches, over
        // every edge any linker of §16 follows.
        // `plain`: and by the VPLAIN mentions (A49's reach, the text only);
        // `asn`: the ASN mentions of these files (the sources of D-12's
        // last stage; every one: A70's reach, the text only).
        let reaching = |plain: bool, asn: Asn<'_>| {
            let mut reached: Vec<bool> = self.files.iter().map(|f| !f.lazy).collect();
            let mut work: Vec<usize> = (0..n).filter(|&i| reached[i]).collect();
            let reach = |k: u32, reached: &mut Vec<bool>, work: &mut Vec<usize>| {
                for &t in lazy_defs.get(&k).map_or(&[][..], Vec::as_slice) {
                    if in_link[t] && !reached[t] {
                        reached[t] = true;
                        work.push(t);
                    }
                }
            };
            for r in &self.roots {
                reach(r.key, &mut reached, &mut work);
            }
            while let Some(f) = work.pop() {
                let src = asn.of(f);
                for &(k, b) in &self.files[f].mentions {
                    if (b & (UNDEF | COMMON | WUNDEF) != 0
                        && (b & WUNDEF == 0 || self.files[f].kind == Kind::Shared))
                        || (plain && b & VPLAIN != 0)
                        || (src && b & ASN != 0)
                    {
                        reach(k, &mut reached, &mut work);
                    }
                }
            }
            reached
        };
        let reached = reaching(false, self.asn(true));
        let reached_plain = reaching(true, self.asn(true));
        let reached_asn = reaching(false, Asn::All);
        let mut reported: HashSet<&Ident> = HashSet::default();
        if !self.export_globs {
            for i in 0..n {
                if self.files[i].lazy
                    && in_link[i]
                    && !reached[i]
                    && reported.insert(&self.files[i].ident)
                    && !self
                        .files
                        .iter()
                        .enumerate()
                        .any(|(j, g)| reached[j] && g.ident == self.files[i].ident)
                {
                    let m = if reached_plain[i] {
                        format!("{} is in the link and {VERSIONED_PLAIN}", self.label(i))
                    } else if reached_asn[i] {
                        format!("{} is in the link and {UNNEEDED_AS_NEEDED}", self.label(i))
                    } else {
                        format!(
                            "{} is in the link and no root or reference of the link reaches it (§3.8){}",
                            self.label(i),
                            self.wrapped_only(i, &reached, line)
                        )
                    };
                    v.findings.push(Finding::new(EXTRA, m));
                }
            }
        }
        // Missing: a strong demand nothing in the link defines, that a
        // lazy file defines and the linker's own rule extracts.
        let gnu = map.format == Format::Gnu;
        let lld = map.format == Format::Lld;
        let mut not_judged = 0usize;
        let mut hidden = unlisted;
        let mut missing: HashMap<u32, Demand> = HashMap::default();
        let mut demand =
            |k: u32, from: usize, at: usize, common: bool, exempt: bool, by: String| {
                if common {
                    if strong[k as usize] || (lld && !self.lld_fortran_common) {
                        return;
                    }
                } else if defined[k as usize] {
                    return;
                }
                let ls: Vec<usize> = lazy_defs
                    .get(&k)
                    .map_or(&[][..], Vec::as_slice)
                    .iter()
                    .copied()
                    .filter(|&l| !in_link[l])
                    .filter(|&l| {
                        let b = self.files[l].mentions.iter().find(|m| m.0 == k).map_or(0, |m| m.1);
                        !common || (b & DEF != 0 && b & (WEAK | COMMON) == 0 && b & DATA != 0)
                    })
                    .collect();
                if ls.is_empty() {
                    return;
                }
                if !ls.iter().all(|&l| reliable(l)) {
                    hidden += 1;
                    return;
                }
                let after: Vec<usize> = if !gnu {
                    ls
                } else if exempt {
                    Vec::new()
                } else {
                    let fr = &self.files[from];
                    ls.into_iter()
                        .filter(|&l| {
                            let g = &self.files[l];
                            if from == 0 {
                                return g.item >= at;
                            }
                            g.item > fr.item
                                || (g.item == fr.item && fr.kind == Kind::Member)
                                || (g.group.is_some() && g.group == fr.group)
                        })
                        .collect()
                };
                if after.is_empty() {
                    not_judged += 1;
                    return;
                }
                missing.entry(k).or_insert((from, by, after));
            };
        for r in &self.roots {
            if !r.uncertain {
                demand(r.key, 0, r.at, false, r.no_position, r.by.clone());
            }
        }
        let sources = self.asn(false);
        for (i, f) in self.files.iter().enumerate() {
            if !in_link[i] {
                continue;
            }
            let src = sources.of(i);
            for &(k, b) in &f.mentions {
                if b & UNDEF != 0 || (src && b & ASN != 0) {
                    demand(k, i, f.item, false, false, self.label(i));
                }
                if b & COMMON != 0 {
                    demand(k, i, f.item, true, false, self.label(i));
                }
            }
        }
        let mut ms: Vec<(u32, Demand)> = missing.into_iter().collect();
        ms.sort_by_key(|m| m.1.2[0]);
        for (k, (_, by, ls)) in ms {
            let mut f = Finding::new(
                MISSING,
                format!(
                    "`{}` ({by}) is defined by no file of the link, and {} defines it (§3.8)",
                    name(k),
                    self.label(ls[0])
                ),
            );
            f.symbol = Some(name(k));
            v.findings.push(f);
        }
        // GNU ld's preamble: each printed reason is true.
        let mut reasons_unjudged = 0usize;
        if gnu {
            let mut said: HashSet<Ident> = HashSet::default();
            for inc in &map.included {
                said.insert(map_ident(&inc.file));
                let Some(ls) = index.get(&map_ident(&inc.file)) else {
                    reasons_unjudged += 1;
                    continue;
                };
                let holds = |k: Option<u32>| {
                    k.is_some_and(|k| {
                        ls.iter().any(|&l| {
                            self.files[l]
                                .mentions
                                .iter()
                                .any(|m| m.0 == k && m.1 & (DEF | COMMON) != 0)
                        })
                    })
                };
                let bad = match &inc.by {
                    // GNU ld prints a C++ name demangled: not judged.
                    IncludedBy::Root(sym) | IncludedBy::Reference { symbol: sym, .. }
                        if demangled(sym) =>
                    {
                        reasons_unjudged += 1;
                        None
                    }
                    IncludedBy::WholeArchive => {
                        (!ls.iter().any(|&l| self.files[l].whole)).then(|| {
                            "its archive is not under --whole-archive on the line".to_string()
                        })
                    }
                    IncludedBy::Root(sym) => {
                        let k = self.find(sym);
                        if !self.roots.iter().any(|r| Some(r.key) == k) {
                            Some(format!("`{sym}` is not a root of the line (resolve §4.2)"))
                        } else if !holds(k) {
                            Some(format!("it does not define `{sym}`"))
                        } else {
                            None
                        }
                    }
                    IncludedBy::Reference { file, symbol } => {
                        let k = self.find(symbol);
                        if line.wrap.iter().any(|w| symbol.ends_with(w.as_str())) {
                            reasons_unjudged += 1;
                            None
                        } else {
                            let refs: Vec<usize> =
                                index.get(&map_ident(file)).cloned().unwrap_or_default();
                            if refs.is_empty() {
                                reasons_unjudged += 1;
                                None
                            } else if !refs.iter().any(|&i| in_link[i]) {
                                Some(format!("{file} is not in the link"))
                            } else if !refs.iter().any(|&i| {
                                self.files[i].mentions.iter().any(|m| {
                                    Some(m.0) == k && m.1 & (UNDEF | WUNDEF | ASN | COMMON) != 0
                                })
                            }) {
                                Some(format!("{file} has no undefined or common `{symbol}`"))
                            } else if !holds(k) {
                                Some(format!("it does not define `{symbol}`"))
                            } else {
                                None
                            }
                        }
                    }
                };
                if let Some(b) = bad {
                    v.findings.push(Finding::new(
                        EXTRA,
                        format!(
                            "GNU ld's map (line {}) says {} was included for {}: {b}",
                            inc.line,
                            inc.file,
                            shown(&inc.by)
                        ),
                    ));
                }
            }
            for mf in &map.files {
                if mf.member.is_some() && !said.contains(&map_ident(mf)) {
                    v.findings.push(Finding::new(
                        EXTRA,
                        format!(
                            "GNU ld's map places {mf} and its preamble does not say it was included"
                        ),
                    ));
                }
            }
        }
        // The closure of resolve §4.1, for comparison (not a finding:
        // §16 records where each reference differs).
        let commons = !lld || self.lld_fortran_common;
        let (lo, _) = self.closure(false, commons);
        let (hi, _) = self.closure(true, commons);
        let mut plus = 0usize;
        let mut minus = 0usize;
        let mut open = 0usize;
        for i in 0..n {
            if !self.files[i].lazy {
                continue;
            }
            if lo[i] != hi[i] {
                open += 1;
            } else if in_link[i] && !lo[i] {
                plus += 1;
            } else if !in_link[i] && lo[i] {
                minus += 1;
            }
        }
        let total =
            self.files.iter().filter(|f| matches!(f.kind, Kind::Object | Kind::Member)).count();
        let judged = total.saturating_sub(not_judged + unmatched + hidden);
        let mut c = format!(
            "input set: files judged {judged} of {total} (in the link {})",
            in_link
                .iter()
                .zip(&self.files)
                .filter(|(l, f)| **l && matches!(f.kind, Kind::Object | Kind::Member))
                .count()
        );
        if plus + minus == 0 && open == 0 {
            c.push_str("; mortar's closure: the same");
        } else {
            let _ = write!(c, "; mortar's closure: differs (+{plus} -{minus}, {open} open)");
        }
        if not_judged > 0 {
            let _ = write!(c, "; demands not judged {not_judged} (GNU ld's back-references)");
        }
        if hidden > 0 {
            let why = if gc { "--gc-sections" } else { "no section the map lists" };
            let _ = write!(c, "; not judged {hidden} (files the map may leave out: {why})");
        }
        if unmatched > 0 {
            let _ = write!(c, "; map members not on the line {unmatched}");
        }
        if reasons_unjudged > 0 {
            let _ = write!(c, "; preamble reasons not judged {reasons_unjudged}");
        }
        if self.export_globs {
            c.push_str("; extra not judged: --export-dynamic-symbol roots not expanded");
        }
        v.coverage = c;
        v
    }
}

fn shown(by: &IncludedBy) -> String {
    match by {
        IncludedBy::WholeArchive => "(--whole-archive)".into(),
        IncludedBy::Root(s) => format!("({s})"),
        IncludedBy::Reference { file, symbol } => format!("{file} ({symbol})"),
    }
}

/// A name as GNU ld's map prints a demangled C++ name (it demangles by
/// default): characters no ELF name of a compiler has.
fn demangled(sym: &str) -> bool {
    sym.contains([' ', '(', ':', '<', '~', ','])
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

/// resolve §7.1: an undefined `SYM` is `__wrap_SYM`, `__real_SYM` is `SYM`
/// (the one derivation, `wrap.rs`).
fn redirect(name: &[u8], wrap: &crate::wrap::Wrap) -> Vec<u8> {
    match std::str::from_utf8(name) {
        Ok(n) => wrap.key(n).into_owned().into_bytes(),
        Err(_) => name.to_vec(),
    }
}

/// `--defsym A=EXPR`: `A`, the names `EXPR` refers to, and whether the
/// expression was read exactly (the one reader, `defsym.rs`, WP-170).
fn defsym(op: &str) -> (Option<String>, Vec<String>, bool) {
    use crate::defsym::Operand;
    match crate::defsym::operand(op) {
        Operand::Judged { sym, expr } => {
            (Some(sym), expr.base().map(str::to_string).into_iter().collect(), true)
        }
        Operand::Other { sym, names, exact, .. } => (Some(sym), names, exact),
        Operand::Unread => (None, Vec::new(), false),
    }
}

/// What the input set reads of a `-T` script: `ENTRY`, `EXTERN`, and the
/// names assigned (file 0's candidates, not read exactly).
#[derive(Debug, Default)]
struct Script {
    entry: Option<String>,
    roots: Vec<String>,
    assigned: Vec<String>,
}

#[allow(clippy::many_single_char_names)] // a scanner: s, w, i, j, k
fn script(text: &str) -> Result<Script, String> {
    // A quoted word is never a keyword or punctuation (script.md §1.6):
    // `w` is the match key (`"` for a string), `text` the word.
    let toks = crate::line::tokens(text, &['(', ')', '{', '}', ';', ',', '=']);
    let w: Vec<&str> = toks.iter().map(|(t, q)| if *q { "\"" } else { t.as_str() }).collect();
    let text_of = |k: usize| toks[k].0.as_str();
    let mut out = Script::default();
    let mut i = 0;
    while i < w.len() {
        match w[i] {
            "INPUT" | "GROUP" | "STARTUP" | "INCLUDE" => {
                return Err(format!("{} is not modelled by the input set", w[i]));
            }
            "ENTRY" | "EXTERN" if w.get(i + 1) == Some(&"(") => {
                let mut j = i + 2;
                let mut names = Vec::new();
                while let Some(&x) = w.get(j) {
                    if x == ")" {
                        break;
                    }
                    if x != "," {
                        names.push(text_of(j).to_string());
                    }
                    j += 1;
                }
                if w[i] == "ENTRY" {
                    out.entry = names.pop();
                } else {
                    out.roots.extend(names);
                }
                i = j;
            }
            "=" if i > 0 => {
                // `+=` and the like arrive as `+` before `=`.
                let mut k = i - 1;
                if matches!(w[k], "+" | "-" | "*" | "/" | "<<" | ">>" | "&" | "|") && k > 0 {
                    k -= 1;
                }
                let n = text_of(k).trim_end_matches(['+', '-', '*', '/', '&', '|', '<', '>']);
                if n != "." && !n.is_empty() {
                    out.assigned.push(n.to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// [`super::judge`] without a replay of D-12's stages.
    fn judge(map: &Map, words: Option<&[String]>, files: &dyn Files) -> Verdict {
        super::judge(map, words, files, None)
    }

    /// A symbol of a synthetic object: `name`, and `D` a function
    /// definition, `O` a data definition, `W` a weak function definition,
    /// `U` a strong reference, `w` a weak reference, `C` a common.
    fn obj(syms: &[(&str, char)]) -> Vec<u8> {
        let mut strtab = vec![0u8];
        let mut symtab = vec![0u8; 24];
        for (n, k) in syms {
            let name = strtab.len() as u32;
            strtab.extend_from_slice(n.as_bytes());
            strtab.push(0);
            let (bind, ty, shndx): (u8, u8, u16) = match k {
                'D' => (1, 2, 1),
                'O' => (1, 1, 1),
                'W' => (2, 2, 1),
                'U' => (1, 0, 0),
                'w' => (2, 0, 0),
                'C' => (1, 1, 0xfff2),
                _ => panic!("kind {k}"),
            };
            symtab.extend_from_slice(&name.to_le_bytes());
            symtab.push(bind << 4 | ty);
            symtab.push(0);
            symtab.extend_from_slice(&shndx.to_le_bytes());
            symtab.extend_from_slice(&(if shndx == 0xfff2 { 4u64 } else { 0 }).to_le_bytes());
            symtab.extend_from_slice(&4u64.to_le_bytes());
        }
        let shstr = b"\0.text\0.shstrtab\0.strtab\0.symtab\0".to_vec();
        let mut f = vec![0u8; 64];
        let text_off = f.len();
        f.extend_from_slice(&[0u8; 16]);
        let shstr_off = f.len();
        f.extend_from_slice(&shstr);
        let str_off = f.len();
        f.extend_from_slice(&strtab);
        while !f.len().is_multiple_of(8) {
            f.push(0);
        }
        let sym_off = f.len();
        f.extend_from_slice(&symtab);
        let sh_off = f.len();
        let sh = |name: u32,
                  ty: u32,
                  off: usize,
                  size: usize,
                  link: u32,
                  info: u32,
                  align: u64,
                  es: u64| {
            let mut h = Vec::new();
            h.extend_from_slice(&name.to_le_bytes());
            h.extend_from_slice(&ty.to_le_bytes());
            h.extend_from_slice(&(if ty == 1 { 6u64 } else { 0 }).to_le_bytes());
            h.extend_from_slice(&0u64.to_le_bytes());
            h.extend_from_slice(&(off as u64).to_le_bytes());
            h.extend_from_slice(&(size as u64).to_le_bytes());
            h.extend_from_slice(&link.to_le_bytes());
            h.extend_from_slice(&info.to_le_bytes());
            h.extend_from_slice(&align.to_le_bytes());
            h.extend_from_slice(&es.to_le_bytes());
            h
        };
        f.extend_from_slice(&[0u8; 64]);
        f.extend(sh(1, 1, text_off, 16, 0, 0, 4, 0));
        f.extend(sh(7, 3, shstr_off, shstr.len(), 0, 0, 1, 0));
        f.extend(sh(17, 3, str_off, strtab.len(), 0, 0, 1, 0));
        f.extend(sh(25, 2, sym_off, symtab.len(), 3, 1, 8, 24));
        f[..4].copy_from_slice(b"\x7fELF");
        f[4] = 2;
        f[5] = 1;
        f[6] = 1;
        f[16..18].copy_from_slice(&1u16.to_le_bytes());
        f[18..20].copy_from_slice(&62u16.to_le_bytes());
        f[20..24].copy_from_slice(&1u32.to_le_bytes());
        f[40..48].copy_from_slice(&(sh_off as u64).to_le_bytes());
        f[52..54].copy_from_slice(&64u16.to_le_bytes());
        f[58..60].copy_from_slice(&64u16.to_le_bytes());
        f[60..62].copy_from_slice(&5u16.to_le_bytes());
        f[62..64].copy_from_slice(&2u16.to_le_bytes());
        f
    }

    /// A regular archive of `members`, without a symbol index.
    fn ar(members: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut a = b"!<arch>\n".to_vec();
        for (n, b) in members {
            a.extend_from_slice(
                format!(
                    "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
                    format!("{n}/"),
                    0,
                    0,
                    0,
                    644,
                    b.len()
                )
                .as_bytes(),
            );
            a.extend_from_slice(b);
            if !a.len().is_multiple_of(2) {
                a.push(b'\n');
            }
        }
        a
    }

    struct Mem(BTreeMap<String, Vec<u8>>);
    impl Files for Mem {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.0.get(path).cloned()
        }
    }

    fn mem(files: Vec<(&str, Vec<u8>)>) -> Mem {
        Mem(files.into_iter().map(|(p, b)| (p.to_string(), b)).collect())
    }

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    /// The files the closure makes live, as `path` or `path(member)`.
    fn live(line: &str, files: &Mem) -> Vec<String> {
        let w = words(line);
        let l = Line::parse(&w, files);
        assert!(l.problems.is_empty(), "{:?}", l.problems);
        let s = Inputs::read(&l, &w, files).unwrap();
        let (lo, _) = s.closure(false, true);
        let (hi, _) = s.closure(true, true);
        assert_eq!(lo, hi);
        (1..s.files.len())
            .filter(|&i| lo[i] && s.files[i].kind != Kind::Shared)
            .map(|i| s.label(i))
            .collect()
    }

    /// A mortar map naming `files` (`path` or `path(member)`).
    fn map_of(files: &[&str]) -> Map {
        let mut m = Map::new(Format::Mortar);
        m.files = files.iter().map(|f| crate::map::split_member(f)).collect();
        m
    }

    fn ids(v: &Verdict) -> Vec<String> {
        v.findings.iter().map(|f| format!("{} {}", f.rule.id(), f.message)).collect()
    }

    #[test]
    fn s3_8_a_cycle_between_members_is_closed_once() {
        // a.o needs x; x.o (defines x) needs y; y.o (defines y) needs x.
        let f = mem(vec![
            ("m.o", obj(&[("_start", 'D'), ("x", 'U')])),
            (
                "lib.a",
                ar(&[
                    ("y.o", obj(&[("y", 'D'), ("x", 'U')])),
                    ("x.o", obj(&[("x", 'D'), ("y", 'U')])),
                    ("z.o", obj(&[("z", 'D')])),
                ]),
            ),
        ]);
        assert_eq!(live("m.o lib.a", &f), ["m.o", "lib.a(y.o)", "lib.a(x.o)"]);
        let v = judge(&map_of(&["m.o", "lib.a(y.o)", "lib.a(x.o)"]), Some(&words("m.o lib.a")), &f);
        assert!(v.findings.is_empty(), "{:?}", ids(&v));
        assert_eq!(v.coverage, "input set: files judged 4 of 4 (in the link 3, required 3)");
        let v = judge(&map_of(&["m.o", "lib.a(x.o)"]), Some(&words("m.o lib.a")), &f);
        assert_eq!(
            ids(&v),
            [
                "M11 lib.a(y.o) is not in the link: lib.a(x.o) refers to `y` and it is the target (resolve §4.1)"
            ]
        );
        let v = judge(
            &map_of(&["m.o", "lib.a(y.o)", "lib.a(x.o)", "lib.a(z.o)"]),
            Some(&words("m.o lib.a")),
            &f,
        );
        assert_eq!(
            ids(&v),
            [
                "M12 lib.a(z.o) is in the link and no root or live reference reaches it (resolve §4.1)"
            ]
        );
    }

    #[test]
    fn s3_8_the_same_symbol_in_two_archives_the_first_is_taken() {
        let f = mem(vec![
            ("m.o", obj(&[("_start", 'D'), ("x", 'U')])),
            ("liba.a", ar(&[("xa.o", obj(&[("x", 'D')]))])),
            ("libb.a", ar(&[("xb.o", obj(&[("x", 'D')]))])),
        ]);
        assert_eq!(live("m.o liba.a libb.a", &f), ["m.o", "liba.a(xa.o)"]);
        assert_eq!(live("m.o libb.a liba.a", &f), ["m.o", "libb.a(xb.o)"]);
        // Before the reference too (resolve §4.3: position-independent).
        assert_eq!(live("libb.a m.o liba.a", &f), ["libb.a(xb.o)", "m.o"]);
        let v = judge(&map_of(&["m.o", "libb.a(xb.o)"]), Some(&words("m.o liba.a libb.a")), &f);
        assert_eq!(
            ids(&v),
            [
                "M12 libb.a(xb.o) is in the link and no root or live reference reaches it (resolve §4.1)",
                "M11 liba.a(xa.o) is not in the link: m.o refers to `x` and it is the target (resolve §4.1)",
            ]
        );
        // A weak definition in a named object after both still loses to
        // the first lazy definer (resolve §4.1, finding 1), one before
        // them keeps both out (E3).
        let f2 = mem(vec![
            ("m.o", obj(&[("_start", 'D'), ("x", 'U')])),
            ("w.o", obj(&[("x", 'W')])),
            ("liba.a", ar(&[("xa.o", obj(&[("x", 'D')]))])),
        ]);
        assert_eq!(live("m.o liba.a w.o", &f2), ["m.o", "liba.a(xa.o)", "w.o"]);
        assert_eq!(live("m.o w.o liba.a", &f2), ["m.o", "w.o"]);
    }

    #[test]
    fn s3_8_an_archive_named_twice_is_one_file() {
        let f = mem(vec![
            ("m.o", obj(&[("_start", 'D'), ("x", 'U')])),
            ("n.o", obj(&[("y", 'U')])),
            ("lib.a", ar(&[("x.o", obj(&[("x", 'D')])), ("y.o", obj(&[("y", 'D')]))])),
        ]);
        let line = "m.o lib.a n.o lib.a";
        assert_eq!(live(line, &f), ["m.o", "lib.a(x.o)", "lib.a(y.o)", "n.o"]);
        let v = judge(&map_of(&["m.o", "lib.a(x.o)", "n.o", "lib.a(y.o)"]), Some(&words(line)), &f);
        assert!(v.findings.is_empty(), "{:?}", ids(&v));
        let v = judge(&map_of(&["m.o", "lib.a(x.o)", "n.o"]), Some(&words(line)), &f);
        assert_eq!(
            ids(&v),
            [
                "M11 lib.a(y.o) is not in the link: n.o refers to `y` and it is the target (resolve §4.1)"
            ]
        );
    }

    #[test]
    fn s3_8_weak_references_and_commons() {
        let f = mem(vec![
            ("m.o", obj(&[("_start", 'D'), ("w", 'w'), ("c", 'C')])),
            (
                "lib.a",
                ar(&[
                    ("w.o", obj(&[("w", 'D')])),
                    ("cf.o", obj(&[("c", 'D')])),
                    ("cd.o", obj(&[("c", 'O')])),
                ]),
            ),
        ]);
        // A weak reference never extracts; a common extracts the first
        // lazy data definition, not a function (resolve §4.5).
        assert_eq!(live("m.o lib.a", &f), ["m.o", "lib.a(cd.o)"]);
        let v =
            judge(&map_of(&["m.o", "lib.a(w.o)", "lib.a(cd.o)"]), Some(&words("m.o lib.a")), &f);
        assert_eq!(
            ids(&v),
            [
                "M12 lib.a(w.o) is in the link and no root or live reference reaches it (resolve §4.1)"
            ]
        );
    }

    #[test]
    #[allow(clippy::many_single_char_names)]
    fn s3_8_roots_and_whole_archive() {
        let f = mem(vec![
            ("m.o", obj(&[("_start", 'D')])),
            ("lib.a", ar(&[("k.o", obj(&[("k", 'D')])), ("j.o", obj(&[("j", 'D')]))])),
            ("x.ld", b"EXTERN(k)".to_vec()),
        ]);
        assert_eq!(live("m.o lib.a", &f), ["m.o"]);
        assert_eq!(live("m.o x.ld lib.a", &f), ["m.o", "lib.a(k.o)"]);
        assert_eq!(live("-u j m.o lib.a", &f), ["m.o", "lib.a(j.o)"]);
        assert_eq!(live("--defsym=q=j+1 m.o lib.a", &f), ["m.o", "lib.a(j.o)"]);
        assert_eq!(
            live("m.o --whole-archive lib.a --no-whole-archive", &f),
            ["m.o", "lib.a(k.o)", "lib.a(j.o)"]
        );
        // `-e` replaces `_start` as the root.
        let g = mem(vec![(
            "lib.a",
            ar(&[("s.o", obj(&[("_start", 'D')])), ("k.o", obj(&[("k", 'D')]))]),
        )]);
        assert_eq!(live("lib.a", &g), ["lib.a(s.o)"]);
        assert_eq!(live("-e k lib.a", &g), ["lib.a(k.o)"]);
        // A script's assignment may define the key: not judged.
        let h = mem(vec![
            ("m.o", obj(&[("_start", 'D'), ("k", 'U')])),
            ("lib.a", ar(&[("k.o", obj(&[("k", 'D')]))])),
            ("t.ld", b"SECTIONS { k = 0x10; }".to_vec()),
        ]);
        let w = words("-T t.ld m.o lib.a");
        for files in [&["m.o"][..], &["m.o", "lib.a(k.o)"][..]] {
            let v = judge(&map_of(files), Some(&w), &h);
            assert!(v.findings.is_empty(), "{:?}", ids(&v));
            assert_eq!(
                v.coverage,
                format!("input set: files judged 1 of 2 (in the link {}, required 1)", files.len())
            );
        }
    }

    /// A thin archive of `members`: headers only, the members in their
    /// own files beside it.
    fn thin(members: &[(&str, usize)]) -> Vec<u8> {
        let mut a = b"!<thin>\n".to_vec();
        for (n, len) in members {
            a.extend_from_slice(
                format!("{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n", format!("{n}/"), 0, 0, 0, 644, len)
                    .as_bytes(),
            );
        }
        a
    }

    /// `o` with its `.text` made mergeable: a file mold's map does not list.
    fn unlisted(mut o: Vec<u8>) -> Vec<u8> {
        let sh = u64::from_le_bytes(o[40..48].try_into().unwrap()) as usize + 64;
        o[sh + 8..sh + 16].copy_from_slice(&0x12u64.to_le_bytes());
        o
    }

    /// GNU ld names a thin archive's member by its path; lld's and mold's
    /// maps leave out what --gc-sections removed, and mold's a file of
    /// mergeable sections only: those files are not judged (§3.8).
    #[test]
    fn s3_8_thin_members_and_maps_that_leave_files_out() {
        let k = obj(&[("k", 'D')]);
        let f = mem(vec![
            ("m.o", obj(&[("_start", 'D'), ("k", 'U')])),
            ("n.o", unlisted(obj(&[("n", 'D')]))),
            ("lib/libk.a", thin(&[("k.o", k.len())])),
            ("lib/k.o", k),
        ]);
        let line = "m.o lib/libk.a";
        let reference = |format: Format, files: &[&str]| {
            let mut m = Map::new(format);
            m.files = files.iter().map(|f| crate::map::split_member(f)).collect();
            m
        };
        let v = judge(&reference(Format::Gnu, &["m.o", "lib/k.o"]), Some(&words(line)), &f);
        assert!(v.findings.is_empty(), "{:?}", ids(&v));
        assert_eq!(
            v.coverage,
            "input set: files judged 2 of 2 (in the link 2); mortar's closure: the same"
        );
        let v = judge(&reference(Format::Gnu, &["m.o"]), Some(&words(line)), &f);
        assert_eq!(
            ids(&v),
            [
                "M11 `k` (m.o) is defined by no file of the link, and lib/libk.a(k.o) defines it (§3.8)"
            ]
        );
        // lld under --gc-sections: neither m.o's absence nor k.o's.
        let gc = "m.o lib/libk.a --gc-sections";
        let v = judge(&reference(Format::Lld, &[]), Some(&words(gc)), &f);
        assert!(v.findings.is_empty(), "{:?}", ids(&v));
        assert!(
            v.coverage.contains("not judged 2 (files the map may leave out: --gc-sections)"),
            "{}",
            v.coverage
        );
        let v = judge(&reference(Format::Lld, &["m.o"]), Some(&words(line)), &f);
        assert_eq!(
            ids(&v),
            [
                "M11 `k` (m.o) is defined by no file of the link, and lib/libk.a(k.o) defines it (§3.8)"
            ]
        );
        // mold: n.o has no section its map lists.
        let v = judge(
            &reference(Format::Mold, &["m.o", "lib/libk.a(k.o)"]),
            Some(&words("m.o n.o lib/libk.a")),
            &f,
        );
        assert!(v.findings.is_empty(), "{:?}", ids(&v));
        assert!(
            v.coverage
                .contains("not judged 1 (files the map may leave out: no section the map lists)"),
            "{}",
            v.coverage
        );
        let v = judge(
            &reference(Format::Lld, &["m.o", "lib/libk.a(k.o)"]),
            Some(&words("m.o n.o lib/libk.a")),
            &f,
        );
        assert_eq!(
            ids(&v),
            ["M11 n.o is not in the link: it is live from the start (resolve §4.2)"]
        );
    }

    #[test]
    fn s3_8_a_reference_map_gnu_ld_s_position() {
        let f = mem(vec![
            ("m.o", obj(&[("_start", 'D'), ("x", 'U')])),
            ("lib.a", ar(&[("x.o", obj(&[("x", 'D')])), ("j.o", obj(&[("j", 'D')]))])),
        ]);
        let gnu = |files: &[&str]| {
            let mut m = Map::new(Format::Gnu);
            m.files = files.iter().map(|f| crate::map::split_member(f)).collect();
            m
        };
        let v = judge(&gnu(&["m.o"]), Some(&words("m.o lib.a")), &f);
        assert_eq!(
            ids(&v),
            ["M11 `x` (m.o) is defined by no file of the link, and lib.a(x.o) defines it (§3.8)"]
        );
        // A back-reference: GNU ld would not extract it; not judged.
        let v = judge(&gnu(&["m.o"]), Some(&words("lib.a m.o")), &f);
        assert!(v.findings.is_empty(), "{:?}", ids(&v));
        assert!(v.coverage.contains("demands not judged 1"), "{}", v.coverage);
        let v = judge(&gnu(&["m.o", "lib.a(x.o)", "lib.a(j.o)"]), Some(&words("m.o lib.a")), &f);
        assert_eq!(
            ids(&v),
            [
                "M12 lib.a(j.o) is in the link and no root or reference of the link reaches it (§3.8)",
                "M12 GNU ld's map places lib.a(x.o) and its preamble does not say it was included",
                "M12 GNU ld's map places lib.a(j.o) and its preamble does not say it was included",
            ]
        );
    }

    #[test]
    fn paths_are_normalized_lexically() {
        assert_eq!(norm("/a//b/./c"), "/a/b/c");
        assert_eq!(norm("/usr/lib/gcc/x/13/../../../y/libc.a"), "/usr/lib/y/libc.a");
        assert_eq!(norm("./m.o"), "m.o");
        assert_eq!(norm("../m.o"), "../m.o");
        assert_eq!(norm("/.."), "/");
    }

    #[test]
    fn defsym_operands() {
        assert_eq!(defsym("a=b"), (Some("a".into()), vec!["b".into()], true));
        assert_eq!(defsym("a=0x10"), (Some("a".into()), vec![], true));
        assert_eq!(defsym("a=b+4"), (Some("a".into()), vec!["b".into()], true));
        assert_eq!(defsym("a=ADDR(.text)"), (Some("a".into()), vec![], false));
        assert_eq!(defsym("a=ABSOLUTE(b)"), (Some("a".into()), vec!["b".into()], false));
        assert!(!defsym("a=b<<2").2);
        assert_eq!(defsym("=b").0, None);
    }

    #[test]
    fn wrap_redirects_undefined_mentions() {
        let w = crate::wrap::Wrap::new(["f".to_string()]);
        assert_eq!(redirect(b"f", &w), b"__wrap_f");
        assert_eq!(redirect(b"__real_f", &w), b"f");
        assert_eq!(redirect(b"g", &w), b"g");
        assert_eq!(redirect(b"__wrap_f", &w), b"__wrap_f");
    }

    #[test]
    fn a_script_s_roots_and_assignments() {
        let s =
            script("ENTRY(go) EXTERN(a b) SECTIONS { x = .; y += 4; PROVIDE(z = 1); . = 0x1000; }")
                .unwrap();
        assert_eq!(s.entry.as_deref(), Some("go"));
        assert_eq!(s.roots, vec!["a", "b"]);
        assert_eq!(s.assigned, vec!["x", "y", "z"]);
        assert!(script("INPUT(a.o)").is_err());
        // script.md §1.6: a quoted name is the name, never a keyword; a
        // comma separates names; a string followed by a name is two.
        let s =
            script(r#"ENTRY("EXTERN") EXTERN("k_fn", j_fn "a b""c"d /* "x" */) "q" = 1;"#).unwrap();
        assert_eq!(s.entry.as_deref(), Some("EXTERN"));
        assert_eq!(s.roots, vec!["k_fn", "j_fn", "a b", "c", "d"]);
        assert_eq!(s.assigned, vec!["q"]);
        assert!(script(r#"EXTERN("INPUT")"#).is_ok());
    }
}
