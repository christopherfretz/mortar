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
    /// Per map file, its `discard` records.
    pub discards: Vec<Vec<(u32, DiscardWhy)>>,
    pub folds: Vec<Fold>,
    pub chunks: Vec<MapChunk>,
    pub defines: Vec<(Vec<u8>, u64)>,
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
                Placed::Place { section, addr, size } => writeln!(s, "place {id} {section} {addr:x} {size:x}"),
                Placed::Piece { section, input_offset, addr, size } => {
                    writeln!(s, "piece {id} {section} {input_offset:x} {addr:x} {size:x}")
                }
            };
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
    out.push_str("end\n");
    out.into_bytes()
}
