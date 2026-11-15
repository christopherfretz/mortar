//! The map file (§9.2, I-77..I-79): `spec/plumb.md` §3.2's `mortar-map 1`.

use std::fmt::Write as _;

use mortar_core::par::Pool;

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
    pub defines: Vec<(Vec<u8>, u64)>,
    /// The dynamic records (`spec/plumb.md` §3.2, `spec/dyn.md` P-D3);
    /// `None` for a static link, which then writes none of them.
    pub dynamic: Option<DynRecords>,
}

/// The output kind of the `output` record (`spec/dyn.md` D-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputKind {
    Static,
    Exec,
    Pie,
    StaticPie,
}

impl OutputKind {
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Exec => "exec",
            Self::Pie => "pie",
            Self::StaticPie => "static-pie",
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

/// A `copy` record: the name of the alias the `COPY` record names, the
/// allocation's address and size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyHint {
    pub name: Vec<u8>,
    pub addr: u64,
    pub size: u64,
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
    /// The command line's words in order, response files expanded, the
    /// program name excluded.
    pub args: Vec<Vec<u8>>,
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
    let mut out = String::from("mortar-map 1\n");
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
    for (name, value) in &map.defines {
        out.push_str("define ");
        token(name, &mut out);
        let _ = writeln!(out, " {value:x}");
    }
    if let Some(d) = &map.dynamic {
        render_dynamic(d, &mut out);
    }
    out.push_str("end\n");
    out.into_bytes()
}

/// The dynamic records, in the format's fixed order (`spec/plumb.md` §3.2).
fn render_dynamic(d: &DynRecords, out: &mut String) {
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
    for a in &d.args {
        named(out, "arg", a);
    }
    for n in &d.needed {
        named(out, "needed", n);
    }
    for c in &d.copies {
        out.push_str("copy ");
        token(&c.name, out);
        let _ = writeln!(out, " {:x} {:x}", c.addr, c.size);
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
