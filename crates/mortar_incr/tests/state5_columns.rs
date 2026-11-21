//! WP-247 item 4 (the F-1 class, table-driven): `state5::columns::COLUMNS`
//! is every column of every format-5 table, checked against `TABLES`
//! (widths), and every column it lists as naming a row of another table
//! keeps what it names through `normalize`, `encode` and its decode, the
//! compaction (`apply` then `normalize`) and the in-place patch. Each
//! listed reference has one identity reader here; a listed reference
//! without one, or a reader of an unlisted column, fails.

use std::collections::BTreeMap;

use mortar_core::{FileId, SymbolId};
use mortar_incr::format::{
    ChunkRow, DefinedRow, IndexRow, OutSecRow, PatchRow, ProbeRow, RedirectRow, SectionRow,
    SizeRow, StatRow, Str, SymPlaceRow, index_kind, kind, patch_kind, stat_tag,
};
use mortar_incr::state5::columns::{COLUMNS, To, references};
use mortar_incr::state5::{self, TABLES, compact, read, write};
use mortar_incr::update::diags::{DiagRef, DiagRow, Subject};
use mortar_incr::update::rows::{
    ChunkUnitRow, CommonRow, CopyRow, EdgeRow, FileRow, FileTable, FragRow, FragUseRow, GroupRow,
    HoleRow, KeyRefRow, MentionRow, MergeDefRow, RegionRow, ResolvedRow, SlotRow, SymbolRow,
    SymtabKeyRow, edge_kind, sym_kind, unit_kind,
};
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::StatePatch;

const NONE: u32 = u32::MAX;

// spec: incr.md IN-18e (the table list): one entry per table, in kind
// order; a table's columns sum to its row width
#[test]
fn every_table_has_its_columns() {
    assert_eq!(COLUMNS.len(), TABLES.len());
    for ((k, cols), (tk, _, width)) in COLUMNS.iter().zip(TABLES.iter()) {
        assert_eq!(k, tk, "COLUMNS is in TABLES' order");
        let sum: usize = cols.iter().map(|c| c.width).sum();
        assert_eq!(sum, *width, "{}: the columns' widths against the row width", state5::name(*k));
        let mut names: Vec<&str> = cols.iter().map(|c| c.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), cols.len(), "{}: a column twice", state5::name(*k));
        for c in *cols {
            assert!(
                c.to != To::Strings || c.width == 8,
                "{}.{}: a Str is 8 bytes",
                state5::name(*k),
                c.name
            );
        }
    }
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}
fn bytes(m: &MemoryState, x: Str) -> Vec<u8> {
    m.strings[x.off as usize..(x.off + x.len) as usize].to_vec()
}
fn st(m: &MemoryState, x: Str) -> Option<String> {
    (x.len != 0).then(|| format!("s:{}", lossy(&bytes(m, x))))
}
fn fl(m: &MemoryState, f: u32) -> Option<String> {
    (f != NONE).then(|| format!("f:{}", lossy(&bytes(m, m.files[f as usize].path))))
}
fn sy(m: &MemoryState, k: u32) -> Option<String> {
    (k != NONE).then(|| {
        let r = &m.symbols[k as usize];
        format!("k:{}@{}", lossy(&bytes(m, r.name)), lossy(&bytes(m, r.version)))
    })
}
fn se(m: &MemoryState, s: u32) -> Option<String> {
    (s != NONE).then(|| {
        let r = &m.sections[s as usize];
        format!("sec:{:?}:{}", fl(m, r.file), lossy(&bytes(m, r.name)))
    })
}
fn ch(m: &MemoryState, c: u32) -> Option<String> {
    (c != NONE).then(|| format!("c:{}", lossy(&bytes(m, m.chunks[c as usize].name))))
}
fn os(m: &MemoryState, o: u32) -> Option<String> {
    (o != NONE).then(|| format!("o:{}", lossy(&bytes(m, m.outsecs[o as usize].name))))
}
fn live_secs(m: &MemoryState) -> impl Iterator<Item = &SectionRow> {
    m.sections.iter().filter(|r| r.file != NONE)
}
/// File `f`'s section at `local` in its `SECTIONS` range (WP-275).
fn local_sec(m: &MemoryState, f: u32, local: u32) -> Option<String> {
    let r = m.sections.iter().filter(|r| r.file == f).nth(local as usize)?;
    Some(format!("sec:{:?}:{}", fl(m, r.file), lossy(&bytes(m, r.name))))
}
/// File `f`'s region at `local` in its `REGIONS` range (WP-275).
fn local_region(m: &MemoryState, f: u32, local: u32) -> Option<String> {
    let (_, r) = m.regions.iter().filter(|(g, _)| g.0 == f).nth(local as usize)?;
    Some(format!("reg:{:?}:{:?}/{}/{}", fl(m, f), os(m, r.out), r.category, r.ordinal))
}

type Reader = fn(&MemoryState) -> Vec<Option<String>>;

/// One identity reader per listed reference: what each row's column
/// names, by the named row's own identity (a path, a name, ...).
#[allow(clippy::too_many_lines)]
fn readers() -> BTreeMap<(u32, &'static str), Reader> {
    let mut r: BTreeMap<(u32, &'static str), Reader> = BTreeMap::new();
    r.insert((kind::META, "version"), |m| vec![st(m, m.meta.version)]);
    r.insert((kind::STAT, "path"), |m| m.stat.iter().map(|x| st(m, x.path)).collect());
    r.insert((kind::STAT, "fstype"), |m| m.stat.iter().map(|x| st(m, x.fstype)).collect());
    r.insert((kind::FILES, "path"), |m| m.files.iter().map(|x| st(m, x.path)).collect());
    r.insert((kind::FILES, "member_name"), |m| {
        m.files.iter().map(|x| st(m, x.member_name)).collect()
    });
    r.insert((kind::FILES, "properties"), |m| {
        m.files.iter().map(|x| st(m, x.properties)).collect()
    });
    r.insert((kind::PROBES, "path"), |m| m.probes.iter().map(|x| st(m, x.path)).collect());
    r.insert((kind::OUTSECS, "name"), |m| m.outsecs.iter().map(|x| st(m, x.name)).collect());
    r.insert((kind::SYMBOLS, "file"), |m| {
        let f = |x: &SymbolRow| if x.kind == sym_kind::LOCAL { fl(m, x.file) } else { None };
        m.symbols.iter().map(|x| f(x).map(|v| format!("{:?}={v}", st(m, x.name)))).collect()
    });
    r.insert((kind::SYMBOLS, "name"), |m| m.symbols.iter().map(|x| st(m, x.name)).collect());
    r.insert((kind::SYMBOLS, "version"), |m| m.symbols.iter().map(|x| st(m, x.version)).collect());
    r.insert((kind::SYMBOLS, "at_file"), |m| {
        m.symbols
            .iter()
            .map(|x| fl(m, x.at_file).map(|v| format!("{:?}={v}", st(m, x.name))))
            .collect()
    });
    r.insert((kind::SECTIONS, "file"), |m| live_secs(m).map(|x| fl(m, x.file)).collect());
    r.insert((kind::SECTIONS, "anchor"), |m| live_secs(m).map(|x| st(m, x.anchor)).collect());
    r.insert((kind::SECTIONS, "name"), |m| live_secs(m).map(|x| st(m, x.name)).collect());
    r.insert((kind::SECTIONS, "place_chunk"), |m| {
        live_secs(m).map(|x| ch(m, x.place_chunk)).collect()
    });
    r.insert((kind::CHUNKS, "name"), |m| m.chunks.iter().map(|x| st(m, x.name)).collect());
    r.insert((kind::CHUNKS, "out"), |m| m.chunks.iter().map(|x| os(m, x.out)).collect());
    r.insert((kind::ORDER, "chunk"), |m| m.order.iter().map(|&c| ch(m, c)).collect());
    r.insert((kind::FRAGS, "out"), |m| m.frags.iter().map(|x| os(m, x.out)).collect());
    r.insert((kind::SLOTS, "symbol"), |m| m.slots.iter().map(|x| sy(m, x.symbol)).collect());
    r.insert((kind::SIZES, "chunk"), |m| m.sizes.iter().map(|x| ch(m, x.chunk)).collect());
    r.insert((kind::SYM_PLACE, "symbol"), |m| {
        m.sym_place.iter().map(|x| sy(m, x.symbol)).collect()
    });
    r.insert((kind::SYM_PLACE, "chunk"), |m| m.sym_place.iter().map(|x| ch(m, x.chunk)).collect());
    r.insert((kind::DEFINED, "symbol"), |m| m.defined.iter().map(|x| sy(m, x.symbol)).collect());
    r.insert((kind::DEFINED, "chunk"), |m| m.defined.iter().map(|x| ch(m, x.chunk)).collect());
    r.insert((kind::PATCHES, "chunk"), |m| m.patches.iter().map(|x| ch(m, x.chunk)).collect());
    r.insert((kind::PATCHES, "symbol"), |m| {
        m.patches
            .iter()
            .map(|x| if x.kind == patch_kind::E843419 { None } else { sy(m, x.symbol) })
            .collect()
    });
    r.insert((kind::PATCHES, "section"), |m| {
        m.patches
            .iter()
            .map(|x| if x.kind == patch_kind::E843419 { se(m, x.section) } else { None })
            .collect()
    });
    r.insert((kind::REDIRECTS, "section"), |m| {
        m.redirects.iter().map(|x| se(m, x.section)).collect()
    });
    r.insert((kind::REDIRECTS, "patch"), |m| {
        m.redirects
            .iter()
            .map(|x| {
                let p = &m.patches[x.patch as usize];
                Some(format!("p:{:?}:{}", ch(m, p.chunk), p.offset))
            })
            .collect()
    });
    r.insert((kind::SIZED, "chunk"), |m| m.sized.iter().map(|x| ch(m, x.chunk)).collect());
    r.insert((kind::MENTIONS, "symbol"), |m| {
        m.mentions
            .iter()
            .map(|(f, x)| sy(m, x.symbol).map(|v| format!("{:?}:{v}", fl(m, f.0))))
            .collect()
    });
    r.insert((kind::MENTIONS, "section"), |m| {
        m.mentions
            .iter()
            .map(|(f, x)| se(m, x.section).map(|v| format!("{:?}:{v}", fl(m, f.0))))
            .collect()
    });
    r.insert((kind::RESOLVED, "section"), |m| {
        (0..m.symbols.len())
            .map(|k| {
                let s = m.resolved.get(k).map_or(NONE, |x| x.section);
                se(m, s).map(|v| format!("{:?}={v}", sy(m, k as u32)))
            })
            .collect()
    });
    r.insert((kind::INDEX, "unit"), |m| m.index.iter().map(|(_, x)| se(m, x.unit)).collect());
    r.insert((kind::INDEX, "target"), |m| {
        m.index
            .iter()
            .map(|(_, x)| match x.kind {
                index_kind::GLOBAL | index_kind::SLOT => sy(m, x.target),
                _ => se(m, x.target),
            })
            .collect()
    });
    r.insert((kind::HOLES, "chunk"), |m| m.holes.iter().map(|x| ch(m, x.chunk)).collect());
    // FILE_INDEX's ranges and locals run: from the written ranges when
    // there are any (a decoded state), else from the rows' own files (a
    // state in memory: the writer builds the ranges).
    r.insert((kind::FILE_INDEX, "ranges"), |m| {
        let written = m.file_index.iter().any(|i| i.ranges.iter().any(|r| r.count != 0));
        (0..m.files.len() as u32)
            .map(|f| {
                let secs: Vec<String> = if written {
                    let r = m.file_index[f as usize].ranges[0];
                    (r.first..r.first + r.count).filter_map(|s| se(m, s)).collect()
                } else {
                    (0..m.sections.len() as u32)
                        .filter(|&s| m.sections[s as usize].file == f)
                        .filter_map(|s| se(m, s))
                        .collect()
                };
                Some(format!("{:?}: {secs:?}", fl(m, f)))
            })
            .collect()
    });
    r.insert((kind::FILE_INDEX, "locals.first"), |m| {
        let written = m.file_index.iter().any(|i| i.locals.count != 0);
        (0..m.files.len() as u32)
            .map(|f| {
                let mut ks: Vec<String> = if written {
                    let r = m.file_index[f as usize].locals;
                    (r.first..r.first + r.count).filter_map(|k| sy(m, k)).collect()
                } else {
                    (0..m.symbols.len() as u32)
                        .filter(|&k| {
                            let s = &m.symbols[k as usize];
                            s.kind == sym_kind::LOCAL && s.file == f
                        })
                        .filter_map(|k| sy(m, k))
                        .collect()
                };
                ks.sort();
                Some(format!("{:?}: {ks:?}", fl(m, f)))
            })
            .collect()
    });
    r.insert((kind::EDGES, "source"), |m| m.edges.iter().map(|(_, x)| se(m, x.source)).collect());
    r.insert((kind::EDGES, "target"), |m| {
        m.edges
            .iter()
            .map(|(_, x)| match edge_kind::target(x.kind) {
                Some(edge_kind::Target::Section) => se(m, x.target),
                Some(edge_kind::Target::Key) => sy(m, x.target),
                _ => None,
            })
            .collect()
    });
    r.insert((kind::GROUPS, "signature"), |m| {
        m.groups
            .iter()
            .map(|x| st(m, x.signature).map(|v| format!("{v}/{:?}", fl(m, x.file))))
            .collect()
    });
    r.insert((kind::GROUPS, "file"), |m| m.groups.iter().map(|x| fl(m, x.file)).collect());
    r.insert((kind::KEY_REFS, "key"), |m| {
        m.key_refs
            .iter()
            .map(|x| sy(m, x.key).map(|v| format!("{v}/{:?}", fl(m, x.file))))
            .collect()
    });
    r.insert((kind::KEY_REFS, "file"), |m| m.key_refs.iter().map(|x| fl(m, x.file)).collect());
    r.insert((kind::COMMONS, "key"), |m| {
        m.commons.iter().map(|x| sy(m, x.key).map(|v| format!("{v}/{:?}", fl(m, x.file)))).collect()
    });
    r.insert((kind::COMMONS, "file"), |m| m.commons.iter().map(|x| fl(m, x.file)).collect());
    r.insert((kind::DIAGS, "unit"), |m| {
        m.diags
            .iter()
            .map(|d| match d.key.phase {
                30 | 40 | 45 | 50 | 60 | 70 | 100 => {
                    fl(m, d.key.unit).map(|v| format!("{}={v}", d.text))
                }
                _ => Some(format!("{}={}", d.text, d.key.unit)),
            })
            .collect()
    });
    r.insert((kind::DIAGS, "subjects"), |m| {
        m.diags
            .iter()
            .map(|d| {
                let v: Vec<Option<String>> = d
                    .subjects
                    .iter()
                    .map(|s| match s {
                        Subject::File(f) => fl(m, f.0),
                        Subject::Key(k) => sy(m, k.0),
                        Subject::OdrKey { name, class } => {
                            st(m, *name).map(|v| format!("{v}/{class}"))
                        }
                        other => Some(format!("{other:?}")),
                    })
                    .collect();
                Some(format!("{}={v:?}", d.text))
            })
            .collect()
    });
    r.insert((kind::DIAGS, "code"), |m| m.diags.iter().map(|d| Some(d.code.clone())).collect());
    r.insert((kind::DIAGS, "text"), |m| m.diags.iter().map(|d| Some(d.text.clone())).collect());
    r.insert((kind::DIAGS, "refs"), |m| {
        m.diags
            .iter()
            .map(|d| {
                let mut v: Vec<(Option<String>, u64, u64)> =
                    d.refs.iter().map(|x| (fl(m, x.file.0), x.seq, x.offset)).collect();
                v.sort();
                Some(format!("{}={v:?}", d.text))
            })
            .collect()
    });
    r.insert((kind::REGIONS, "out"), |m| m.regions.iter().map(|(_, x)| os(m, x.out)).collect());
    r.insert((kind::REGIONS, "chunk"), |m| m.regions.iter().map(|(_, x)| ch(m, x.chunk)).collect());
    r.insert((kind::NAMES, "row"), |m| m.names.iter().map(|x| sy(m, x.row)).collect());
    r.insert((kind::COPIES, "key"), |m| {
        m.copies.iter().map(|x| sy(m, x.key).map(|v| format!("{v}/{:?}", fl(m, x.file)))).collect()
    });
    r.insert((kind::COPIES, "file"), |m| m.copies.iter().map(|x| fl(m, x.file)).collect());
    r.insert((kind::FRAG_USE, "file"), |m| {
        m.frag_uses
            .iter()
            .map(|(f, x)| fl(m, x.file).map(|v| format!("{:?}/{v}", fl(m, f.0))))
            .collect()
    });
    r.insert((kind::FRAG_USE, "out"), |m| m.frag_uses.iter().map(|(_, x)| os(m, x.out)).collect());
    r.insert((kind::CHUNK_UNITS, "chunk"), |m| {
        m.chunk_units.iter().filter(|x| !x.tomb).map(|x| ch(m, x.chunk)).collect()
    });
    r.insert((kind::CHUNK_UNITS, "file"), |m| {
        m.chunk_units.iter().filter(|x| !x.tomb).map(|x| fl(m, x.file)).collect()
    });
    r.insert((kind::CHUNK_UNITS, "local"), |m| {
        m.chunk_units
            .iter()
            .filter(|x| !x.tomb)
            .map(|x| match x.unit {
                unit_kind::REGION => local_region(m, x.file, x.local),
                _ => local_sec(m, x.file, x.local),
            })
            .collect()
    });
    r.insert((kind::MERGE_DEFS, "file"), |m| {
        m.merge_defs.iter().filter(|x| !x.tomb).map(|x| fl(m, x.file)).collect()
    });
    r.insert((kind::MERGE_DEFS, "local"), |m| {
        m.merge_defs.iter().filter(|x| !x.tomb).map(|x| local_sec(m, x.file, x.local)).collect()
    });
    r.insert((kind::MERGE_DEFS, "out"), |m| {
        m.merge_defs.iter().filter(|x| !x.tomb).map(|x| os(m, x.out)).collect()
    });
    r
}

/// Every listed reference has its reader, every reader is of a listed
/// reference (a column added to the list without one fails here).
#[test]
fn every_listed_reference_has_its_reader() {
    let listed: Vec<(u32, &str)> = references().map(|(k, c)| (k, c.name)).collect();
    let have: Vec<(u32, &str)> = readers().keys().copied().collect();
    let mut sorted = listed.clone();
    sorted.sort_unstable();
    let name = |v: &[(u32, &str)]| -> Vec<String> {
        v.iter().map(|(k, c)| format!("{}.{c}", state5::name(*k))).collect()
    };
    assert_eq!(name(&sorted), name(&have), "the listed references against the test's readers");
}

/// What every listed reference names, per column, as a sorted multiset
/// (rows move: the writer sorts, the compaction renumbers).
fn identities(m: &MemoryState) -> BTreeMap<String, Vec<String>> {
    let readers = readers();
    references()
        .map(|(k, c)| {
            let mut v: Vec<String> = readers[&(k, c.name)](m).into_iter().flatten().collect();
            v.sort();
            (format!("{}.{}", state5::name(k), c.name), v)
        })
        .collect()
}

fn same(a: &MemoryState, b: &MemoryState, what: &str) {
    let (x, y) = (identities(a), identities(b));
    let mut bad = Vec::new();
    for (col, v) in &x {
        if y.get(col) != Some(v) {
            bad.push(format!("{col}:\n  before {v:?}\n  after  {:?}", y.get(col)));
        }
    }
    assert!(bad.is_empty(), "{what}: a reference names another row\n{}", bad.join("\n"));
}

fn add(m: &mut MemoryState, b: &[u8]) -> Str {
    let off = u32::try_from(m.strings.len()).expect("small");
    m.strings.extend_from_slice(b);
    Str { off, len: u32::try_from(b.len()).expect("small") }
}

/// A state with a row in every table that has a listed reference, and
/// every renumbering the compaction does: files out of rank order, keys
/// out of IN-18e's order, a filler `SECTIONS` row first, strings at other
/// offsets (one string stored twice).
#[allow(clippy::too_many_lines)]
fn every_reference() -> MemoryState {
    let mut m = MemoryState { plan_hash: [3; 16], ..MemoryState::default() };
    m.meta.version = add(&mut m, b"mortar test");
    m.meta.iterations = 1;
    let _junk = add(&mut m, b"junk");
    // three files, ranks 3, 1, 2: the compaction orders them 1, 2, 0
    for (i, rank) in [3u64, 1, 2].iter().enumerate() {
        let path = add(&mut m, format!("f{i}.o").as_bytes());
        let member_name = add(&mut m, format!("m{i}").as_bytes());
        let properties = add(&mut m, format!("p{i}").as_bytes());
        m.files.push(FileRow {
            path,
            member_name,
            properties,
            rank: rank << 20,
            ..FileRow::default()
        });
        let fstype = add(&mut m, b"ext4");
        m.stat.push(StatRow { tag: stat_tag::INPUT, path, fstype, ..StatRow::default() });
        m.file_index.push(mortar_incr::update::rows::FileIndexRow::default());
    }
    let out_path = add(&mut m, b"a.out");
    m.stat.push(StatRow { tag: stat_tag::OUTPUT, path: out_path, ..StatRow::default() });
    let probe = add(&mut m, b"libz.a");
    m.probes.push(ProbeRow { path: probe, present: 1 });
    for (o, n) in [b".text".as_slice(), b".rodata"].iter().enumerate() {
        let name = add(&mut m, n);
        m.outsecs.push(OutSecRow { name, ordinal: o as u32 });
    }
    // chunks: 0 in outsec 1, 1 in outsec 0
    for (c, out) in [1u32, 0].iter().enumerate() {
        let name = add(&mut m, format!("chunk{c}").as_bytes());
        m.chunks.push(ChunkRow { name, ordinal: c as u32, out: *out, ..ChunkRow::default() });
    }
    m.order = vec![1, 0];
    // a filler, then two sections of each file, the last file first
    m.sections.push(SectionRow { file: NONE, ..SectionRow::default() });
    for f in [2u32, 0, 1] {
        for k in 0..2u32 {
            let name = add(&mut m, format!(".text.f{f}.{k}").as_bytes());
            let anchor = add(&mut m, format!("a{f}{k}").as_bytes());
            m.sections.push(SectionRow {
                file: f,
                anchor_kind: 1,
                anchor,
                name,
                ordinal: k,
                place_chunk: k,
                size: 16,
                ..SectionRow::default()
            });
        }
    }
    let sec = |f: u32, k: u32| -> u32 { 1 + 2 * [1, 2, 0][f as usize] + k };
    // keys: an unmentioned option key first, globals of files 0, 1, 2
    // (IN-18e orders them by first file, so 1, 2, 0), each file's local
    let opt = add(&mut m, b"opt");
    m.symbols.push(SymbolRow {
        kind: sym_kind::GLOBAL,
        file: NONE,
        name: opt,
        at_file: NONE,
        option: true,
        ..SymbolRow::default()
    });
    for f in 0..3u32 {
        let name = add(&mut m, format!("g{f}").as_bytes());
        let version = if f == 1 { add(&mut m, b"V1") } else { Str::default() };
        let kind = if f == 1 { sym_kind::VERSIONED } else { sym_kind::GLOBAL };
        m.symbols.push(SymbolRow {
            kind,
            file: NONE,
            name,
            version,
            at_file: f,
            at_index: 1,
            ..SymbolRow::default()
        });
    }
    for f in 0..3u32 {
        let name = add(&mut m, format!("l{f}").as_bytes());
        m.symbols.push(SymbolRow {
            kind: sym_kind::LOCAL,
            file: f,
            name,
            at_file: f,
            at_index: 0,
            ..SymbolRow::default()
        });
    }
    let g = |f: u32| 1 + f;
    let l = |f: u32| 4 + f;
    m.resolved = vec![ResolvedRow { section: NONE, ..ResolvedRow::default() }; m.symbols.len()];
    m.symtab_keys = vec![SymtabKeyRow { index: NONE, strtab: 0 }; m.symbols.len()];
    for f in 0..3u32 {
        m.resolved[g(f) as usize].section = sec(f, 0);
        m.resolved[g(f) as usize].kind = 1;
        m.resolved[l(f) as usize].section = sec(f, 1);
        m.mentions.push((
            FileId(f),
            MentionRow { symbol: g(f), section: sec(f, 0), ..MentionRow::default() },
        ));
        m.mentions.push((
            FileId(f),
            MentionRow { symbol: g((f + 1) % 3), section: NONE, ..MentionRow::default() },
        ));
        m.mentions.push((
            FileId(f),
            MentionRow { symbol: l(f), section: sec(f, 1), ..MentionRow::default() },
        ));
        m.key_refs.push(KeyRefRow { key: g(f), file: f, def: 1, ..KeyRefRow::default() });
        m.key_refs.push(KeyRefRow { key: g((f + 1) % 3), file: f, ..KeyRefRow::default() });
        m.index.push((
            FileId(f),
            IndexRow { unit: sec(f, 0), kind: index_kind::GLOBAL, target: g(f) },
        ));
        m.index.push((
            FileId(f),
            IndexRow { unit: sec(f, 1), kind: index_kind::SECTION, target: sec(f, 0) },
        ));
        m.edges.push((
            FileId(f),
            EdgeRow {
                kind: edge_kind::FDE_SECTION,
                source: sec(f, 0),
                target: sec(f, 1),
                offset: 0,
            },
        ));
        m.edges.push((
            FileId(f),
            EdgeRow {
                kind: edge_kind::FDE_KEY,
                source: sec(f, 1),
                target: g((f + 2) % 3),
                offset: 0,
            },
        ));
        m.regions.push((
            FileId(f),
            RegionRow { out: f % 2, chunk: (f + 1) % 2, ordinal: f, ..RegionRow::default() },
        ));
        let signature = add(&mut m, b"grp");
        m.groups.push(GroupRow { signature, file: f, tomb: false });
        m.commons.push(CommonRow { key: 0, file: f, size: 8, align: 8, tomb: false });
        m.copies.push(CopyRow { key: g(f), file: f, index: 2, size: 16, typ: 1, tomb: false });
        // WP-275: the file's region (in chunk (f + 1) % 2) and its second
        // section (in chunk 1) as units; seq values not yet `k << 32`
        m.chunk_units.push(ChunkUnitRow {
            chunk: (f + 1) % 2,
            seq: 7 + u64::from(f),
            file: f,
            local: 0,
            unit: unit_kind::REGION,
            tomb: false,
        });
        m.chunk_units.push(ChunkUnitRow {
            chunk: 1,
            seq: 100 - u64::from(f),
            file: f,
            local: 1,
            unit: unit_kind::SECTION,
            tomb: false,
        });
        m.merge_defs.push(MergeDefRow {
            file: f,
            local: 0,
            value: 4 * u64::from(f),
            out: f % 2,
            class: 0x8000_0001,
            hash: [f as u8 + 1; 16],
            within: f,
            ordinal: 2 * f,
            tomb: false,
        });
        let hash = [f as u8 + 1; 16];
        m.frags.push(FragRow {
            out: f % 2,
            class: 0x8000_0001,
            hash,
            offset: 8 * u64::from(f),
            refs: 1,
            ..FragRow::default()
        });
        m.frag_uses.push((FileId(f), FragUseRow { file: f, out: f % 2, class: 0x8000_0001, hash }));
        let text = format!("finding in f{f}");
        m.diags.push(DiagRow {
            subjects: vec![
                Subject::File(FileId(f)),
                Subject::Key(SymbolId(g((f + 1) % 3))),
                Subject::OdrKey { name: opt, class: 1 },
            ],
            code: format!("code{f}"),
            text,
            key: mortar_diag::Key { phase: 30, unit: f, seq: u64::from(f) },
            refs: vec![DiagRef { file: FileId((f + 1) % 3), seq: 1, offset: 2 }],
            ref_count: 1,
            tomb: false,
        });
    }
    m.diags.push(DiagRow {
        subjects: vec![Subject::Link],
        code: "plain".into(),
        text: "an argument".into(),
        key: mortar_diag::Key { phase: 10, unit: 7, seq: 0 },
        refs: Vec::new(),
        ref_count: 0,
        tomb: false,
    });
    m.slots.push(SlotRow { kind: 0, symbol: g(2), refs: 1 });
    m.sizes.push(SizeRow { chunk: 1, size: 64 });
    m.sized.push(SizeRow { chunk: 0, size: 32 });
    m.sym_place.push(SymPlaceRow { kind: 0, symbol: g(0), chunk: 1, ..SymPlaceRow::default() });
    m.defined.push(DefinedRow { symbol: g(1), kind: 1, chunk: 0, value: 0 });
    m.patches.push(PatchRow {
        chunk: 1,
        kind: patch_kind::THUNK,
        offset: 0,
        symbol: g(2),
        section: NONE,
        value: 0,
    });
    m.patches.push(PatchRow {
        chunk: 0,
        kind: patch_kind::E843419,
        offset: 8,
        symbol: NONE,
        section: sec(1, 0),
        value: 0,
    });
    m.redirects.push(RedirectRow { section: sec(2, 1), patch: 1, offset: 4 });
    m.holes.push(HoleRow { chunk: 1, kind: 0, offset: 0, size: 8, tomb: false });
    m.name_rows();
    m
}

fn view_of(m: &MemoryState) -> MemoryState {
    read::decode(&write::encode(m).expect("encode")).expect("decode")
}

// spec: incr.md IN-18e (references), IN-78 (the compaction's renumbering)
#[test]
fn normalize_encode_and_compaction_keep_every_reference() {
    let m = every_reference();
    let n = compact::normalize(&m).expect("normalize");
    same(&m, &n, "normalize");
    let back = view_of(&n);
    same(&m, &back, "encode, decode");
    // the compaction of the decoded state (no patch): apply, normalize
    let mut c = back.clone();
    let aux = c.frags.len();
    compact::apply(&mut c, &StatePatch::default(), aux).expect("apply");
    same(&m, &compact::normalize(&c).expect("normalize"), "compaction");
    // a state written without the compaction (fillers, files out of rank)
    same(&m, &view_of(&m), "encode of an uncompacted state");
}

/// A patch with a new file (rank first: the compaction moves it to row 0)
/// and rows naming it, an existing key and a new string, in every
/// key-keyed table with a reference.
fn patch_of(base: &MemoryState) -> StatePatch {
    let mut p = StatePatch::default();
    let at = base.strings.len();
    let mut fresh = |b: &[u8]| {
        let off = u32::try_from(at + p.strings.len()).expect("small");
        p.strings.extend_from_slice(b);
        Str { off, len: u32::try_from(b.len()).expect("small") }
    };
    let path = fresh(b"new.o");
    let sig = fresh(b"newgrp");
    let new = u32::try_from(base.files.len()).expect("small");
    p.files_append.push(FileRow { path, rank: 1, ..FileRow::default() });
    let key = base
        .symbols
        .iter()
        .position(|r| bytes(base, r.name) == b"g2")
        .map(|k| k as u32)
        .expect("g2");
    p.key_refs.put.push(KeyRefRow { key, file: new, ..KeyRefRow::default() });
    p.groups.put.push(GroupRow { signature: sig, file: new, tomb: false });
    p.commons.put.push(CommonRow { key, file: new, size: 4, align: 4, tomb: false });
    p.copies.put.push(CopyRow { key, file: new, index: 3, size: 8, typ: 1, tomb: false });
    p.diags.put.push(DiagRow {
        subjects: vec![Subject::File(FileId(new)), Subject::Key(SymbolId(key))],
        code: "code-new".into(),
        text: "finding in new.o".into(),
        key: mortar_diag::Key { phase: 30, unit: new, seq: 0 },
        refs: vec![DiagRef { file: FileId(new), seq: 0, offset: 0 }],
        ref_count: 1,
        tomb: false,
    });
    p
}

// spec: incr.md IN-46b (the patch's rows are in the state's own row
// space), IN-78 (the compaction after it)
#[test]
fn the_patch_and_its_compaction_keep_every_reference() {
    let bytes = write::encode(&compact::normalize(&every_reference()).expect("n")).expect("encode");
    let base = read::decode(&bytes).expect("decode");
    let p = patch_of(&base);
    let mut want = base.clone();
    compact::apply(&mut want, &p, base.frags.len()).expect("apply");
    let view = read::open_bytes(bytes.clone()).expect("open");
    let plan = state5::plan_patch(&view, &p).expect("plan");
    let m = read::decode(&bytes).expect("decode");
    let out = m.stat.iter().rfind(|r| r.tag == stat_tag::OUTPUT).expect("output").clone();
    assert!(!plan.compacts(), "the patch fits in place: {}", plan.reason());
    let patched = write::patched_bytes(&bytes, &plan, &out).expect("in place");
    same(&want, &read::decode(&patched).expect("decode"), "the patch in place");
    same(
        &want,
        &compact::normalize(&want).expect("normalize"),
        "the compaction of the patched state",
    );
    let compacted = view_of(&compact::normalize(&want).expect("normalize"));
    same(&want, &compacted, "the compaction written");
}

/// Seals `b` again after a test changed bytes of a table's body: every
/// block list, every list hash, the directory's hash and the header's,
/// from the bodies as they are (WP-247 item g). A mutation that the
/// hashes would catch then reaches the reader's row checks.
fn reseal(b: &mut [u8]) {
    let dir_end = state5::HEADER + TABLES.len() * state5::ENTRY;
    let h = state5::Header::parse(b).expect("header");
    let mut dir = state5::parse_directory(&h, &b[state5::HEADER..dir_end]).expect("directory");
    for e in &mut dir {
        let body = &b[e.body as usize..(e.body + e.body_len()) as usize];
        let list: Vec<u8> = body.chunks(state5::BLOCK).flat_map(mortar_digest::l128).collect();
        assert_eq!(list.len() as u64, e.list_len());
        b[e.list as usize..(e.list + e.list_len()) as usize].copy_from_slice(&list);
        e.list_hash = mortar_digest::l128(&list);
    }
    let dir_bytes = state5::directory_bytes(&dir);
    b[state5::HEADER..dir_end].copy_from_slice(&dir_bytes);
    let h = state5::Header { dir_hash: mortar_digest::l128(&dir_bytes), ..h };
    b[..state5::HEADER].copy_from_slice(&h.bytes());
}

fn entry(b: &[u8], k: u32) -> state5::Entry {
    let dir_end = state5::HEADER + TABLES.len() * state5::ENTRY;
    let h = state5::Header::parse(b).expect("header");
    state5::parse_directory(&h, &b[state5::HEADER..dir_end])
        .expect("directory")
        .into_iter()
        .find(|e| e.kind == k)
        .expect("the table")
}

/// Every reader of a state: the whole decode and its compaction, the
/// canon, and the lazy view's per-file and per-key reads. `Err` if any
/// refused; a panic is the test's failure.
fn read_all(b: &[u8]) -> Result<(), String> {
    let m = read::decode(b).map_err(|e| format!("decode: {e}"))?;
    compact::normalize(&m).map_err(|e| format!("normalize: {e}"))?;
    state5::canon(b).map_err(|e| format!("canon: {e}"))?;
    let view = read::open_bytes(b.to_vec()).map_err(|e| format!("open: {e:?}"))?;
    let e = |x: mortar_core::incr::IncrReason| format!("{x:?}");
    for f in 0..view.count(kind::FILES) as u32 {
        let f = FileId(f);
        view.file(f).map_err(e)?;
        view.sections_of(f).map_err(e)?;
        view.mentions_of(f).map_err(e)?;
        view.index_of(f).map_err(e)?;
        view.edges_of(f).map_err(e)?;
        view.frag_uses_of(f).map_err(e)?;
        view.symtab_of(f).map_err(e)?;
        view.regions_of(f).map_err(e)?;
    }
    for k in 0..view.count(kind::SYMBOLS) as u32 {
        let k = SymbolId(k);
        view.key_refs(k).map_err(e)?;
        view.commons(k).map_err(e)?;
        view.copies(k).map_err(e)?;
    }
    view.diags().map_err(e)?;
    state5::plan_patch(&view, &patch_of(&m)).map_err(e)?;
    Ok(())
}

fn no_panic(b: &[u8], what: &str) -> Result<(), String> {
    std::panic::catch_unwind(|| read_all(b)).unwrap_or_else(|_| panic!("{what}: a reader panicked"))
}

fn fresh_bytes() -> Vec<u8> {
    write::encode(&compact::normalize(&every_reference()).expect("normalize")).expect("encode")
}

// spec: incr.md IN-18c, IN-18e (a row out of range is state-corrupt);
// WP-247 item g
#[test]
fn file_index_row_out_of_range_is_state_corrupt() {
    let mut b = fresh_bytes();
    assert_eq!(no_panic(&b, "clean"), Ok(()));
    let e = entry(&b, kind::FILE_INDEX);
    // row 0's SECTIONS range: first 0xFFFF_FFFF, count 2
    let at = e.body as usize;
    b[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    b[at + 4..at + 8].copy_from_slice(&2u32.to_le_bytes());
    reseal(&mut b);
    let r = no_panic(&b, "FILE_INDEX first 0xFFFF_FFFF count 2");
    assert!(r.as_ref().is_err_and(|e| e.contains("state-corrupt")), "{r:?}");
}

/// The references format 5's readers take unchecked (WP-247 item g's
/// finding; the record proposes the check): the whole tables' chunk,
/// output-section and patch rows (format 4's `validate` checked them;
/// format 5's decode does not), `NAMES.row`, a global's `SYMBOLS.file`
/// (not read for a global), and `DIAGS.unit` (a value outside the file
/// phases: the row mutated here is phase 10's).
const UNCHECKED: &[&str] = &[
    "SYMBOLS.file",
    "SECTIONS.place_chunk",
    "CHUNKS.out",
    "ORDER.chunk",
    "SIZES.chunk",
    "SYM_PLACE.chunk",
    "DEFINED.chunk",
    "PATCHES.chunk",
    "PATCHES.symbol",
    "PATCHES.section",
    "REDIRECTS.patch",
    "SIZED.chunk",
    "HOLES.chunk",
    "REGIONS.out",
    "REGIONS.chunk",
    "NAMES.row",
    "DIAGS.unit",
];

// WP-247 item g: every column of every table's first and last row set to
// all ones, then to all ones but the low byte (0x..FE: out of range for
// any row count here), the state sealed again: no reader panics, and a
// reference column so changed is refused (or names NONE, its "no row").
#[test]
fn every_column_mutated_no_reader_panics() {
    let clean = fresh_bytes();
    let mut accepted_refs = Vec::new();
    for (k, cols) in &COLUMNS {
        let e = entry(&clean, *k);
        if e.rows == 0 || *k == kind::STRINGS {
            continue;
        }
        let width = u64::from(e.width);
        // a file-keyed table: rows inside a file's range (slack rows are
        // never read)
        let rows: Vec<u64> = match FileTable::ALL.iter().position(|t| t.kind() == *k) {
            Some(t) => {
                let m = read::decode(&clean).expect("decode");
                let mut v: Vec<u64> = m
                    .file_index
                    .iter()
                    .filter(|i| i.ranges[t].count != 0)
                    .flat_map(|i| [i.ranges[t].first, i.ranges[t].first + i.ranges[t].count - 1])
                    .map(u64::from)
                    .collect();
                v.sort_unstable();
                v.dedup();
                v
            }
            None if *k == kind::META => vec![0],
            None => vec![0, e.rows.min(e.capacity) - 1],
        };
        for row in rows {
            let mut off = 0;
            for c in *cols {
                for fill in [0xFFu8, 0xFE] {
                    let mut b = clean.clone();
                    let at = (e.body + row * width) as usize + off;
                    for (i, x) in b[at..at + c.width].iter_mut().enumerate() {
                        *x = if i == 0 { fill } else { 0xFF };
                    }
                    if b == clean {
                        continue;
                    }
                    reseal(&mut b);
                    let what = format!("{}.{} row {row} fill {fill:#x}", state5::name(*k), c.name);
                    let r = no_panic(&b, &what);
                    let named = format!("{}.{}", state5::name(*k), c.name);
                    if r.is_ok()
                        && c.to != To::Value
                        && !(fill == 0xFF && c.width == 4)
                        && !UNCHECKED.contains(&named.as_str())
                    {
                        accepted_refs.push(what);
                    }
                }
                off += c.width;
            }
        }
    }
    assert!(
        accepted_refs.is_empty(),
        "references out of range read without a refusal:\n{}",
        accepted_refs.join("\n")
    );
}

// ---- WP-255g: CHUNKS.members (VERSION 12) -----------------------------------------

/// `TAGS`' byte of a `Body` chunk.
const BODY: u8 = 3;

fn members(m: &MemoryState) -> Vec<u32> {
    m.chunks.iter().map(|c| c.members).collect()
}

// spec: incr.md IN-18e (`CHUNKS.members`, WP-255g): the count survives
// normalize, encode and decode, the in-place patch that rewrites `CHUNKS`
// and the compaction after it; a count on a chunk that is not `Body` is
// refused, and a state of VERSION 11 is `state-version`
#[test]
fn chunk_members_survive_the_patch_and_the_compaction() {
    let mut m = every_reference();
    for (c, n) in m.chunks.iter_mut().zip([5u32, 0]) {
        c.tag = BODY;
        c.members = n;
    }
    let bytes = write::encode(&compact::normalize(&m).expect("n")).expect("encode");
    let base = read::decode(&bytes).expect("decode");
    assert_eq!(members(&base), [5, 0], "encode, decode");
    let view = read::open_bytes(bytes.clone()).expect("open");
    let stored = read::LayoutOf { view: &view };
    assert_eq!(
        mortar_core::incr::LayoutRows::chunk_members(&stored).expect("the count"),
        [5, 0],
        "LayoutRows::chunk_members"
    );
    // The update's CHUNKS: chunk 0 lost two members, chunk 1 gained one.
    let mut p = patch_of(&base);
    let mut rows = base.chunks.clone();
    rows[0].members = 3;
    rows[1].members = 1;
    p.chunks = Some(rows);
    let mut want = base.clone();
    compact::apply(&mut want, &p, base.frags.len()).expect("apply");
    assert_eq!(members(&want), [3, 1], "apply");
    let plan = state5::plan_patch(&view, &p).expect("plan");
    assert!(!plan.compacts(), "the patch fits in place: {}", plan.reason());
    let out = base.stat.iter().rfind(|r| r.tag == stat_tag::OUTPUT).expect("output").clone();
    let patched = write::patched_bytes(&bytes, &plan, &out).expect("in place");
    let after = read::decode(&patched).expect("decode");
    assert_eq!(members(&after), [3, 1], "the patch in place");
    let compacted = view_of(&compact::normalize(&after).expect("normalize"));
    assert_eq!(members(&compacted), [3, 1], "the compaction written");
    // A count on a chunk that is not `Body` is refused by its reader.
    let mut bad = base.clone();
    bad.chunks[1].tag = 0;
    bad.chunks[1].members = 1;
    let b =
        write::encode(&compact::normalize(&bad).expect("n")).expect("the writer does not judge");
    assert!(read::decode(&b).is_err(), "a FileHeader chunk with a member count decodes");
    // VERSION 11 (no `CHUNKS.members`) is refused by name.
    assert_eq!(state5::VERSION, 13);
    let mut old = bytes.clone();
    old[8..12].copy_from_slice(&11u32.to_le_bytes());
    let e = read::decode(&old).expect_err("a version-11 state decodes");
    assert!(format!("{e:?}").contains("format version 11"), "{e:?}");
}

// ---- WP-275: CHUNK_UNITS and MERGE_DEFS (VERSION 13) -----------------------------

/// Per chunk, its live units in key order: (chunk, file path, unit, what
/// `local` names).
fn unit_list(m: &MemoryState) -> Vec<(u32, Option<String>, u8, Option<String>)> {
    let mut v: Vec<&ChunkUnitRow> = m.chunk_units.iter().filter(|r| !r.tomb).collect();
    v.sort_by_key(|r| (r.chunk, r.seq));
    v.iter()
        .map(|r| {
            let named = match r.unit {
                unit_kind::REGION => local_region(m, r.file, r.local),
                _ => local_sec(m, r.file, r.local),
            };
            (r.chunk, fl(m, r.file), r.unit, named)
        })
        .collect()
}

/// Per chunk, its live units' `seq`s in key order.
fn seqs(m: &MemoryState) -> Vec<(u32, u64)> {
    let mut v: Vec<(u32, u64)> =
        m.chunk_units.iter().filter(|r| !r.tomb).map(|r| (r.chunk, r.seq)).collect();
    v.sort_unstable();
    v
}

/// The live `MERGE_DEFS` rows by identity, sorted.
fn merge_list(m: &MemoryState) -> Vec<String> {
    let mut v: Vec<String> = m
        .merge_defs
        .iter()
        .filter(|r| !r.tomb)
        .map(|r| {
            format!(
                "{:?} {:?} v{} {:?} {:#x} {:02x?} {} {}",
                fl(m, r.file),
                local_sec(m, r.file, r.local),
                r.value,
                os(m, r.out),
                r.class,
                &r.hash[..2],
                r.within,
                r.ordinal
            )
        })
        .collect();
    v.sort();
    v
}

/// The live row of `chunk` at its `k`-th place (1-based).
fn unit_at(m: &MemoryState, chunk: u32, k: usize) -> ChunkUnitRow {
    let mut v: Vec<&ChunkUnitRow> =
        m.chunk_units.iter().filter(|r| !r.tomb && r.chunk == chunk).collect();
    v.sort_by_key(|r| r.seq);
    *v[k - 1]
}

// spec: WP-275 §2.2, §2.4, §3 (P-275a): both tables survive normalize,
// encode and decode, an in-place PATCH that inserts a unit by midpoint
// between two rows, tombstones one and rewrites a `local` (and a
// `MERGE_DEFS` delete and insert), and the compaction after it (`seq`
// renumbered `k << 32`, the order kept); a VERSION 12 state is refused
// `state-version`
#[test]
fn chunk_units_and_merge_defs_survive_the_patch_and_the_compaction() {
    let m = every_reference();
    let bytes = write::encode(&compact::normalize(&m).expect("n")).expect("encode");
    let base = read::decode(&bytes).expect("decode");
    assert_eq!(unit_list(&base), unit_list(&m), "encode, decode: the units in their order");
    assert_eq!(merge_list(&base), merge_list(&m), "encode, decode: MERGE_DEFS");
    // chunk 0: f1's region; chunk 1: f0's and f2's regions, then the
    // second sections of f2, f1, f0 (`seq` 98, 99, 100)
    let k32 = |k: u64| k << 32;
    assert_eq!(
        seqs(&base),
        [(0, k32(1)), (1, k32(1)), (1, k32(2)), (1, k32(3)), (1, k32(4)), (1, k32(5))],
        "normalize renumbers seq"
    );
    let order: Vec<(Option<String>, u8)> =
        unit_list(&base).into_iter().map(|(_, f, u, _)| (f, u)).collect();
    let f = |i: u32| Some(format!("f:f{i}.o"));
    assert_eq!(
        order,
        [(f(1), 0), (f(0), 0), (f(2), 0), (f(2), 1), (f(1), 1), (f(0), 1)],
        "the order kept"
    );
    // The PATCH: in chunk 1 a unit inserted between its 2nd and 3rd rows
    // (f0's first section, the midpoint `seq`), its 4th row (f1's
    // section) tombstoned, its 5th row's `local` rewritten (f0's second
    // section to its first);
    // MERGE_DEFS: one row deleted, one inserted.
    let mut p = StatePatch::default();
    let (a, b) = (unit_at(&base, 1, 2), unit_at(&base, 1, 3));
    let f0 = unit_at(&base, 1, 1).file;
    let mid = ChunkUnitRow {
        chunk: 1,
        seq: (a.seq + b.seq) / 2,
        file: f0,
        local: 0,
        unit: unit_kind::SECTION,
        tomb: false,
    };
    p.chunk_units.put.push(mid);
    let gone = unit_at(&base, 1, 4);
    p.chunk_units.delete.push(gone);
    let moved = unit_at(&base, 1, 5);
    p.chunk_units.put.push(ChunkUnitRow { local: 0, ..moved });
    let dead = *base.merge_defs.iter().find(|r| r.file == f0).expect("f0's row");
    p.merge_defs.delete.push(dead);
    p.merge_defs.put.push(MergeDefRow { local: 1, value: 3, ordinal: 9, ..dead });
    let mut want = base.clone();
    compact::apply(&mut want, &p, base.frags.len()).expect("apply");
    let expect = [
        (f(1), 0, "reg"),
        (f(0), 0, "reg"),
        (f(2), 0, "reg"),
        (f(0), 1, ".text.f0.0"),
        (f(2), 1, ".text.f2.1"),
        (f(0), 1, ".text.f0.0"),
    ];
    let named = |m: &MemoryState| -> Vec<(Option<String>, u8, String)> {
        unit_list(m)
            .into_iter()
            .map(|(_, f, u, n)| {
                let n = n.expect("names a row");
                let n = if u == 0 {
                    "reg".to_owned()
                } else {
                    n.rsplit(':').next().expect("name").to_owned()
                };
                (f, u, n)
            })
            .collect()
    };
    let expect: Vec<(Option<String>, u8, String)> =
        expect.iter().map(|(f, u, n)| (f.clone(), *u, (*n).to_owned())).collect();
    assert_eq!(named(&want), expect, "apply");
    let view = read::open_bytes(bytes.clone()).expect("open");
    let plan = state5::plan_patch(&view, &p).expect("plan");
    assert!(!plan.compacts(), "the patch fits in place: {}", plan.reason());
    let out = base.stat.iter().rfind(|r| r.tag == stat_tag::OUTPUT).expect("output").clone();
    let patched = write::patched_bytes(&bytes, &plan, &out).expect("in place");
    let after = read::decode(&patched).expect("decode");
    assert_eq!(named(&after), expect, "the patch in place");
    assert_eq!(merge_list(&after), merge_list(&want), "MERGE_DEFS: the patch in place");
    assert!(merge_list(&after).iter().any(|r| r.contains("v3 ")), "the inserted MERGE_DEFS row");
    assert_eq!(after.merge_defs.iter().filter(|r| !r.tomb).count(), 3, "one deleted, one put");
    assert!(
        seqs(&after).iter().any(|&(c, s)| c == 1 && s % (1 << 32) != 0),
        "the inserted unit's midpoint seq"
    );
    // The compaction: seq renumbered `k << 32`, the order kept.
    let compacted = view_of(&compact::normalize(&after).expect("normalize"));
    assert_eq!(named(&compacted), expect, "the compaction written");
    assert_eq!(merge_list(&compacted), merge_list(&want), "MERGE_DEFS: the compaction");
    assert_eq!(
        seqs(&compacted),
        [(0, k32(1)), (1, k32(1)), (1, k32(2)), (1, k32(3)), (1, k32(4)), (1, k32(5))],
        "the compaction renumbers seq"
    );
    // VERSION 12 (neither table) is refused `state-version`.
    assert_eq!(state5::VERSION, 13);
    let mut old = bytes.clone();
    old[8..12].copy_from_slice(&12u32.to_le_bytes());
    let e = read::decode(&old).expect_err("a version-12 state decodes");
    assert_eq!(e.reason(), "state-version", "{e:?}");
    assert!(format!("{e:?}").contains("format version 12"), "{e:?}");
}

// spec: WP-275 §2.2 (`canon`: `seq` is forgotten), IN-78: two states that
// differ only in `CHUNK_UNITS.seq` have one canon (`incr_state canon` and
// `compare`); two that differ in a chunk's order do not
#[test]
fn incr_state_canon_is_seq_blind_and_a_reorder_differs() {
    let m = every_reference();
    let mut other = m.clone();
    for r in &mut other.chunk_units {
        r.seq = r.seq * 1000 + 17;
    }
    let canon = |m: &MemoryState| {
        state5::canon(&write::encode(&compact::normalize(m).expect("n")).expect("encode"))
            .expect("canon")
    };
    let stored = mortar_core::incr::MemoryLayout::default();
    let p_u = mortar_core::incr::StoredLayout::new(&stored);
    let (a, b) = (canon(&m), canon(&other));
    assert!(state5::compare(&a, &b, &p_u).is_empty(), "seq alone differs");
    // the uncompacted states' canon (seq as written) too
    let raw = |m: &MemoryState| state5::canon(&write::encode(m).expect("encode")).expect("canon");
    assert!(state5::compare(&raw(&m), &raw(&other), &p_u).is_empty(), "seq alone, uncompacted");
    // chunk 1's last two units swapped: a reorder
    let mut swapped = m.clone();
    let mut v: Vec<usize> =
        (0..swapped.chunk_units.len()).filter(|&i| swapped.chunk_units[i].chunk == 1).collect();
    v.sort_by_key(|&i| swapped.chunk_units[i].seq);
    let (x, y) = (v[v.len() - 2], v[v.len() - 1]);
    let s = swapped.chunk_units[x].seq;
    swapped.chunk_units[x].seq = swapped.chunk_units[y].seq;
    swapped.chunk_units[y].seq = s;
    let d = state5::compare(&a, &canon(&swapped), &p_u);
    assert!(
        !d.is_empty() && d.iter().all(|d| d.kind == kind::CHUNK_UNITS),
        "a reorder differs, in CHUNK_UNITS only: {d:?}"
    );
}
