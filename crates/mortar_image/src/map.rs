//! The map file (§9.2, I-77..I-79): `spec/plumb.md` §3.2's `mortar-map 2`.

use std::fmt::Write as _;

use std::os::unix::ffi::OsStrExt;

use mortar_core::par::Pool;
use mortar_core::plan::{Plan, Symbolic};

/// One `file` record (I-78): the found path, and the member name and
/// member header offset of an archive member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapFile {
    pub path: Vec<u8>,
    pub member: Option<(Vec<u8>, u64)>,
}

/// Why a section is not in the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiscardWhy {
    Gc,
    Comdat,
    Script,
    Unspecified,
}

impl DiscardWhy {
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Gc => "gc",
            Self::Comdat => "comdat",
            Self::Script => "script",
            Self::Unspecified => "unspecified",
        }
    }
}

/// A placed record of one file: `place` or `piece`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Placed {
    Place { section: u32, addr: u64, size: u64 },
    Piece { section: u32, input_offset: u64, addr: u64, size: u64 },
}

/// A non-alloc record of one file (R-9, `spec/plumb.md` §3.2): `nplace`,
/// a section placed whole at `offset` of the non-alloc output section
/// `out`; `merged`, an `SHF_MERGE` non-alloc section whose pieces are in
/// `out` and are not listed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NonAlloc {
    Place { section: u32, out: Vec<u8>, offset: u64, size: u64 },
    Merged { section: u32, out: Vec<u8> },
}

/// `chunk`'s kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChunkKind {
    Thunk,
    Patch,
    Synthetic,
    Common,
}

impl ChunkKind {
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Thunk => "thunk",
            Self::Patch => "patch",
            Self::Synthetic => "synthetic",
            Self::Common => "common",
        }
    }
}

/// A `chunk` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapChunk {
    pub kind: ChunkKind,
    pub name: Vec<u8>,
    pub addr: Option<u64>,
    pub size: u64,
    pub target: Option<(Vec<u8>, i64)>,
}

/// A `common` record (`spec/plumb.md` §3.2, the allocation records): a
/// common symbol's allocation and the alignment it was placed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapCommon {
    pub name: Vec<u8>,
    pub addr: u64,
    pub size: u64,
    /// A power of two.
    pub align: u64,
}

/// A `fold` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fold {
    pub file: u32,
    pub section: u32,
    pub into_file: u32,
    pub into_section: u32,
}

/// The records of a map, by group, each in output order (I-78). File ids
/// are map ids: indexes into `files`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Map {
    pub files: Vec<MapFile>,
    /// Per map file, its `place` and `piece` records.
    pub placed: Vec<Vec<Placed>>,
    /// Per map file, its `nplace` and `merged` records (R-9).
    pub nonalloc: Vec<Vec<NonAlloc>>,
    /// Per map file, its `discard` records.
    pub discards: Vec<Vec<(u32, DiscardWhy)>>,
    pub folds: Vec<Fold>,
    pub chunks: Vec<MapChunk>,
    /// The `common` records, in `SymbolId` order (layout L-88).
    pub commons: Vec<MapCommon>,
    pub defines: Vec<(Vec<u8>, u64)>,
    /// The `incremental` record (`spec/plumb.md` §3.2): the link was made
    /// in incremental mode, so the map claims IN-60's coverage, with or
    /// without `hole`/`dead` lines. `false` in a plain link.
    pub incremental: bool,
    /// The `hole` and `dead` records (`spec/plumb.md` §3.2, `spec/incr.md`
    /// IN-60), by output section in header order, then offset: empty in
    /// a plain link.
    pub dead: Vec<DeadLine>,
    /// The dynamic records (`spec/plumb.md` §3.2, `spec/dyn.md` P-D3);
    /// `None` for a static link, which then writes none of them.
    pub dynamic: Option<DynRecords>,
    /// The `arg` records, in every map (`spec/plumb.md` §3.2a, image.md
    /// I-78): the command line's words in order, response files
    /// expanded, the program name excluded.
    pub args: Vec<Vec<u8>>,
}

/// The kind of a `dead` record (`spec/plumb.md` §3.2 and §6.8,
/// `spec/incr.md` IN-3a): `neutral` an IN-51/IN-51b neutral form of a
/// debug section, `contribution` a whole dead debug contribution as its
/// compiler wrote it (IN-53), `strings` dead fragments of a merged string
/// section (L-147), `cie` unused CIEs in `.eh_frame`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeadKind {
    Neutral,
    Contribution,
    Strings,
    Cie,
}

impl DeadKind {
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Contribution => "contribution",
            Self::Strings => "strings",
            Self::Cie => "cie",
        }
    }
}

/// A `hole` record (`kind` `None`) or a `dead` record: a range of output
/// section `out` at `offset` from its start.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeadLine {
    pub out: Vec<u8>,
    pub offset: u64,
    pub size: u64,
    pub kind: Option<DeadKind>,
}

/// The output kind of the `output` record (`spec/dyn.md` D-1; `Shared`,
/// M4: `spec/plumb.md` §3.2, planned by WP-116).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputKind {
    Static,
    Exec,
    Pie,
    StaticPie,
    /// A shared object (`-shared`). No link writes it before the front
    /// accepts `-shared` (`spec/cli.md` §4.2, WP-117).
    Shared,
}

impl OutputKind {
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Exec => "exec",
            Self::Pie => "pie",
            Self::StaticPie => "static-pie",
            Self::Shared => "shared",
        }
    }
}

/// The tag of the run path: `runpath` (`DT_RUNPATH`) or `rpath`
/// (`DT_RPATH`, `--disable-new-dtags`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunpathTag {
    Runpath,
    Rpath,
}

/// The records of a `Shared` link's line (`spec/plumb.md` §3.2, the line
/// records of a shared object; M4, planned by WP-116, read by WP-121):
/// D-112's form of the line, then each `--dynamic-list` and each
/// `--version-script` file as the line names it, in order. Facts of the
/// line: the verifier recomputes each from the `arg` words and reports a
/// record that disagrees.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SharedLine {
    /// `symbolic none|all|functions`: the last of `-Bno-symbolic`,
    /// `-Bsymbolic` and `-Bsymbolic-functions` (`spec/dyn.md` D-112).
    pub symbolic: Symbolic,
    /// `dynamic-list <path>`, one per `--dynamic-list`, the path as given.
    pub dynamic_lists: Vec<Vec<u8>>,
    /// `version-script <path>`, one per `--version-script`, the path as
    /// given.
    pub version_scripts: Vec<Vec<u8>>,
}

impl SharedLine {
    /// The records of `plan`'s line (`Plan::symbolic`, the list and script
    /// files); `None` for a line whose last `-Bsymbolic` form D-112
    /// refuses, which never links.
    #[must_use]
    pub fn of(plan: &Plan) -> Option<Self> {
        let line = plan.symbolic()?;
        let path = |s: &mortar_core::plan::ScriptArg| s.path.as_bytes().to_vec();
        Some(Self {
            symbolic: line.form,
            dynamic_lists: plan.dynamic_lists().map(path).collect(),
            version_scripts: plan.version_scripts().map(path).collect(),
        })
    }
}

/// The token of a `symbolic` record.
#[must_use]
pub const fn symbolic_token(form: Symbolic) -> &'static str {
    match form {
        Symbolic::None => "none",
        Symbolic::All => "all",
        Symbolic::Functions => "functions",
    }
}

/// A `copy` record: the name of the alias the `COPY` record names, the
/// allocation's address and size, and the alignment it was placed at
/// (`spec/dyn.md` D-42; a power of two).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyHint {
    pub name: Vec<u8>,
    pub addr: u64,
    pub size: u64,
    pub align: u64,
}

/// The dynamic records (`spec/plumb.md` §3.2; P-D3): facts of the command
/// line (`output`, `interp`, `runpath`/`rpath`, `bind`, `relro`, `arg`),
/// then order hints the verifier never takes as the expected value
/// (`needed`, `copy`, `export`, `plt`, `iplt`). Each package that makes
/// the fact fills its field; a field left at its default writes no record.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DynRecords {
    pub kind: Option<OutputKind>,
    /// The `-dynamic-linker` path.
    pub interp: Option<Vec<u8>>,
    /// The `-rpath` list joined by `:`, and its tag.
    pub runpath: Option<(RunpathTag, Vec<u8>)>,
    /// `Some(true)` under `-z now`.
    pub now: Option<bool>,
    /// `Some(false)` under `-z norelro`.
    pub relro: Option<bool>,
    /// The line records of a shared object (`symbolic`, `dynamic-list`,
    /// `version-script`): `Some` exactly when `kind` is `Shared`; `None`
    /// writes none of them, so the map of every other kind is unchanged.
    pub shared: Option<SharedLine>,
    /// The `DT_NEEDED` names, in the order written.
    pub needed: Vec<Vec<u8>>,
    pub copies: Vec<CopyHint>,
    /// The exported names, in `.dynsym` order.
    pub exports: Vec<Vec<u8>>,
    /// The PLT entries' keys in PLT order (`name` or `name@version`).
    pub plt: Vec<Vec<u8>>,
    /// The IPLT entries' keys in IPLT order.
    pub iplt: Vec<Vec<u8>>,
}

/// A string token (`spec/plumb.md` §3.2): `!`..`~` except `%` stand for
/// themselves, every other byte is `%XX`, the empty string a lone `%`.
pub fn token(s: &[u8], out: &mut String) {
    if s.is_empty() {
        out.push('%');
        return;
    }
    for &b in s {
        if (b'!'..=b'~').contains(&b) && b != b'%' {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
}

/// The text of `map` (I-79): groups formatted in parallel per file and
/// concatenated in the format's group order.
#[must_use]
pub fn render(map: &Map, pool: &Pool) -> Vec<u8> {
    let mut out = String::from("mortar-map 2\n");
    for (id, f) in map.files.iter().enumerate() {
        let _ = write!(out, "file {id} ");
        token(&f.path, &mut out);
        if let Some((member, off)) = &f.member {
            out.push(' ');
            token(member, &mut out);
            let _ = write!(out, " {off:x}");
        }
        out.push('\n');
    }
    let placed = pool.map(map.placed.len(), |id| {
        let mut s = String::new();
        for p in &map.placed[id] {
            let _ = match *p {
                Placed::Place { section, addr, size } => {
                    writeln!(s, "place {id} {section} {addr:x} {size:x}")
                }
                Placed::Piece { section, input_offset, addr, size } => {
                    writeln!(s, "piece {id} {section} {input_offset:x} {addr:x} {size:x}")
                }
            };
        }
        s
    });
    let nonalloc = pool.map(map.nonalloc.len(), |id| {
        let mut s = String::new();
        for r in &map.nonalloc[id] {
            match r {
                NonAlloc::Place { section, out, offset, size } => {
                    let _ = write!(s, "nplace {id} {section} ");
                    token(out, &mut s);
                    let _ = writeln!(s, " {offset:x} {size:x}");
                }
                NonAlloc::Merged { section, out } => {
                    let _ = write!(s, "merged {id} {section} ");
                    token(out, &mut s);
                    s.push('\n');
                }
            }
        }
        s
    });
    let discards = pool.map(map.discards.len(), |id| {
        let mut s = String::new();
        for (section, why) in &map.discards[id] {
            let _ = writeln!(s, "discard {id} {section} {}", why.token());
        }
        s
    });
    out.extend(placed);
    out.extend(nonalloc);
    out.extend(discards);
    for f in &map.folds {
        let _ = writeln!(out, "fold {} {} {} {}", f.file, f.section, f.into_file, f.into_section);
    }
    for c in &map.chunks {
        let _ = write!(out, "chunk {} ", c.kind.token());
        token(&c.name, &mut out);
        match c.addr {
            Some(a) => {
                let _ = write!(out, " {a:x} {:x}", c.size);
            }
            None => {
                let _ = write!(out, " - {:x}", c.size);
            }
        }
        if let Some((t, addend)) = &c.target {
            out.push(' ');
            token(t, &mut out);
            if *addend < 0 {
                let _ = write!(out, " -{:x}", addend.unsigned_abs());
            } else {
                let _ = write!(out, " {addend:x}");
            }
        }
        out.push('\n');
    }
    for c in &map.commons {
        out.push_str("common ");
        token(&c.name, &mut out);
        let _ = writeln!(out, " {:x} {:x} {:x}", c.addr, c.size, c.align);
    }
    for (name, value) in &map.defines {
        out.push_str("define ");
        token(name, &mut out);
        let _ = writeln!(out, " {value:x}");
    }
    render_dead(map, &mut out);
    render_dynamic(map.dynamic.as_ref(), &map.args, &mut out);
    out.push_str("end\n");
    out.into_bytes()
}

/// The `incremental`, `hole` and `dead` records (`spec/plumb.md` §3.2).
fn render_dead(map: &Map, out: &mut String) {
    if map.incremental {
        out.push_str("incremental\n");
    }
    for d in &map.dead {
        out.push_str(if d.kind.is_some() { "dead " } else { "hole " });
        token(&d.out, out);
        let _ = write!(out, " {:x} {:x}", d.offset, d.size);
        if let Some(k) = d.kind {
            out.push(' ');
            out.push_str(k.token());
        }
        out.push('\n');
    }
}

/// The dynamic records, in the format's fixed order (`spec/plumb.md` §3.2),
/// with the `arg` records at their place: a static map has those only.
fn render_dynamic(d: Option<&DynRecords>, args: &[Vec<u8>], out: &mut String) {
    let empty = DynRecords::default();
    let d = d.unwrap_or(&empty);
    let named = |out: &mut String, record: &str, name: &[u8]| {
        out.push_str(record);
        out.push(' ');
        token(name, out);
        out.push('\n');
    };
    if let Some(k) = d.kind {
        let _ = writeln!(out, "output {}", k.token());
    }
    if let Some(i) = &d.interp {
        named(out, "interp", i);
    }
    if let Some((tag, paths)) = &d.runpath {
        let record = match tag {
            RunpathTag::Runpath => "runpath",
            RunpathTag::Rpath => "rpath",
        };
        named(out, record, paths);
    }
    if let Some(now) = d.now {
        out.push_str(if now { "bind now\n" } else { "bind lazy\n" });
    }
    if let Some(relro) = d.relro {
        out.push_str(if relro { "relro on\n" } else { "relro off\n" });
    }
    if let Some(line) = &d.shared {
        let _ = writeln!(out, "symbolic {}", symbolic_token(line.symbolic));
        for l in &line.dynamic_lists {
            named(out, "dynamic-list", l);
        }
        for v in &line.version_scripts {
            named(out, "version-script", v);
        }
    }
    for a in args {
        named(out, "arg", a);
    }
    for n in &d.needed {
        named(out, "needed", n);
    }
    for c in &d.copies {
        out.push_str("copy ");
        token(&c.name, out);
        let _ = writeln!(out, " {:x} {:x} {:x}", c.addr, c.size, c.align);
    }
    for e in &d.exports {
        named(out, "export", e);
    }
    for p in &d.plt {
        named(out, "plt", p);
    }
    for p in &d.iplt {
        named(out, "iplt", p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mortar_core::plan::{Bsymbolic, ScriptArg, ScriptRole};

    fn render_dyn(d: DynRecords) -> String {
        let map = Map { dynamic: Some(d), args: vec![b"-shared".to_vec()], ..Map::default() };
        String::from_utf8(render(&map, &Pool::new(1))).unwrap()
    }

    // spec: plumb.md §3.2 (the line records of a shared object, planned by
    // WP-116): `output shared`, then `symbolic`, `dynamic-list` and
    // `version-script` after `relro` and before `arg`, paths as tokens.
    #[test]
    fn s3_2_shared_line_records() {
        let d = DynRecords {
            kind: Some(OutputKind::Shared),
            now: Some(false),
            relro: Some(true),
            shared: Some(SharedLine {
                symbolic: Symbolic::Functions,
                dynamic_lists: vec![b"a.list".to_vec()],
                version_scripts: vec![b"v 1.map".to_vec(), b"v2.map".to_vec()],
            }),
            ..DynRecords::default()
        };
        assert_eq!(
            render_dyn(d),
            "mortar-map 2\noutput shared\nbind lazy\nrelro on\nsymbolic functions\n\
             dynamic-list a.list\nversion-script v%201.map\nversion-script v2.map\n\
             arg -shared\nend\n"
        );
    }

    // spec: plumb.md §3.2: without `shared` no line record is written, so
    // the map of every other kind is byte-identical to its map before them.
    #[test]
    fn s3_2_no_shared_line_records_outside_shared() {
        let d = DynRecords {
            kind: Some(OutputKind::Pie),
            now: Some(true),
            relro: Some(true),
            ..DynRecords::default()
        };
        assert_eq!(
            render_dyn(d),
            "mortar-map 2\noutput pie\nbind now\nrelro on\narg -shared\nend\n"
        );
    }

    // spec: dyn.md D-112 (the last form wins, `--dynamic-list` beside it),
    // cli.md §3.6; the refused forms have no record.
    #[test]
    fn d112_shared_line_of_a_plan() {
        let mut plan = Plan::default();
        assert_eq!(SharedLine::of(&plan), Some(SharedLine::default()));
        plan.dynamic.bsymbolic = Some(Bsymbolic::All);
        plan.scripts = vec![
            ScriptArg { arg: 1, role: ScriptRole::VersionScript, path: "v.map".into() },
            ScriptArg { arg: 2, role: ScriptRole::DynamicList, path: "d.list".into() },
            ScriptArg { arg: 3, role: ScriptRole::ExportDynamicSymbolList, path: "e".into() },
            ScriptArg { arg: 4, role: ScriptRole::VersionScript, path: "w.map".into() },
        ];
        let line = SharedLine::of(&plan).unwrap();
        assert_eq!(line.symbolic, Symbolic::All);
        assert_eq!(line.dynamic_lists, [b"d.list".to_vec()]);
        assert_eq!(line.version_scripts, [b"v.map".to_vec(), b"w.map".to_vec()]);
        plan.dynamic.bsymbolic = Some(Bsymbolic::NonWeakFunctions);
        assert_eq!(SharedLine::of(&plan), None);
    }
}
