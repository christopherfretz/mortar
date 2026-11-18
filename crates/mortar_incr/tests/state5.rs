//! U-2's tests (`spec/incr.md` IN-18c..IN-18f, IN-46b, IN-78; WP-214).
//! The states are made by hand (`sample`): a few files with sections,
//! a global and a local each, `MENTIONS`, `KEY_REFS`, `FRAGS` and
//! `FRAG_USE`; the files are named by their index in the full set, so the
//! capture of a subset is the full link of those inputs.

use mortar_core::incr::{IncrReason, MemoryLayout, StoredLayout};
use mortar_core::{FileId, SymbolId};
use mortar_incr::format::{SectionRow, StatRow, StateError, Str, kind, stat_tag};
use mortar_incr::ops::{StdOps, WriteFailure};
use mortar_incr::state5::{self, compact, read, write};
use mortar_incr::update::rows::{
    CommonRow, CopyRow, FileIndexRow, FileRow, FragRow, FragUseRow, GroupRow, KeyRefRow,
    MentionRow, NameRow, ResolvedRow, SymbolRow, SymtabKeyRow, file_flags, sym_kind,
};
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::{FileRanges, KeyEdits, StatePatch};

fn add(pool: &mut Vec<u8>, b: &[u8]) -> Str {
    let off = u32::try_from(pool.len()).expect("small");
    pool.extend_from_slice(b);
    Str { off, len: u32::try_from(b.len()).expect("small") }
}

/// The capture of the files `ids` (of a full set `0..`), each with
/// `sections` sections; symbols deliberately out of IN-18e's order.
fn sample(ids: &[u32], sections: u32) -> MemoryState {
    let mut m = MemoryState { plan_hash: [7; 16], ..MemoryState::default() };
    let mut pool = Vec::new();
    m.meta.version = add(&mut pool, b"mortar test");
    let out = add(&mut pool, b"a.out");
    for (f, &id) in ids.iter().enumerate() {
        let f = u32::try_from(f).expect("small");
        let path = add(&mut pool, format!("f{id}.o").as_bytes());
        m.files.push(FileRow {
            path,
            rank: u64::from(id) << 20,
            eh_offset: 64 * id,
            eh_capacity: 40,
            eh_records: 2,
            ..FileRow::default()
        });
        m.stat.push(StatRow {
            tag: stat_tag::INPUT,
            path,
            size: u64::from(id),
            ..StatRow::default()
        });
        let first = u32::try_from(m.sections.len()).expect("small");
        for k in 0..sections {
            let name = add(&mut pool, format!(".text.{id}.{k}").as_bytes());
            m.sections.push(SectionRow {
                file: f,
                name,
                ordinal: k,
                size: 16,
                ..SectionRow::default()
            });
        }
        // a local first, then the global: the capture's first-call order
        let lname = add(&mut pool, format!("l{id}").as_bytes());
        m.symbols.push(SymbolRow {
            kind: sym_kind::LOCAL,
            file: f,
            name: lname,
            at_file: f,
            at_index: 1,
            ..SymbolRow::default()
        });
        let gname = add(&mut pool, format!("g{id}").as_bytes());
        let g = u32::try_from(m.symbols.len()).expect("small");
        m.symbols.push(SymbolRow {
            kind: sym_kind::GLOBAL,
            file: u32::MAX,
            name: gname,
            at_file: f,
            at_index: 2,
            ..SymbolRow::default()
        });
        m.mentions
            .push((FileId(f), MentionRow { symbol: g, section: first, ..MentionRow::default() }));
        m.key_refs.push(KeyRefRow { key: g, file: f, demand: 1, ..KeyRefRow::default() });
        // its own fragment, and the one every file shares (`refs` is the
        // number of files with a FRAG_USE row: normalize counts it)
        m.frags.push(FragRow {
            out: 0,
            class: CLASS,
            hash: hash_of(id + 1),
            offset: u64::from(id + 1) * 8,
            ..FragRow::default()
        });
        for h in [hash_of(id + 1), hash_of(0)] {
            let row = FragUseRow { file: f, out: 0, class: CLASS, hash: h };
            m.frag_uses.push((FileId(f), row));
        }
        // WP-215/WP-220 rows: every file has a copy of one group (its own
        // `Str` of the same bytes), a common mention and a COMDAT copy of
        // the unmentioned key (row 0 below), and its `sh_info`.
        let signature = add(&mut pool, b"grp");
        m.groups.push(GroupRow { signature, file: f, tomb: false });
        m.commons.push(CommonRow {
            key: 0,
            file: f,
            size: 8 * u64::from(id + 1),
            align: 8,
            tomb: false,
        });
        m.copies.push(CopyRow { key: 0, file: f, index: 2, size: 16, typ: 2, tomb: false });
        m.file_index.push(FileIndexRow { first_global: 2 + id, ..FileIndexRow::default() });
    }
    m.meta.iterations = 1;
    m.meta.versioned = 3;
    // an unmentioned key (a --defsym), early
    let dname = add(&mut pool, b"defsym");
    m.symbols.insert(
        0,
        SymbolRow {
            kind: sym_kind::GLOBAL,
            file: u32::MAX,
            name: dname,
            at_file: u32::MAX,
            ..SymbolRow::default()
        },
    );
    for (_, r) in &mut m.mentions {
        r.symbol += 1;
    }
    for r in &mut m.key_refs {
        r.key += 1;
    }
    m.frags.push(FragRow {
        out: 0,
        class: CLASS,
        hash: hash_of(0),
        offset: 0,
        ..FragRow::default()
    });
    let users = u32::try_from(ids.len()).expect("small");
    for r in &mut m.frags {
        r.refs = if r.hash == hash_of(0) { users } else { 1 };
    }
    // `facts`: the scan's packed word, stored as given (R-4a)
    m.resolved = vec![
        ResolvedRow { section: u32::MAX, facts: FACTS, ..ResolvedRow::default() };
        m.symbols.len()
    ];
    m.symtab_keys = vec![SymtabKeyRow { index: u32::MAX, strtab: 0 }; m.symbols.len()];
    m.stat.push(StatRow { tag: stat_tag::OUTPUT, path: out, ..StatRow::default() });
    m.strings = pool;
    m
}

/// A merge class word (`MergeClass::word`: strings, align 1, entsize 1).
const CLASS: u32 = 0x8000_0001;
/// A `RESOLVED.facts` word (opaque to the state).
const FACTS: u32 = 0x0001_0005;

fn hash_of(id: u32) -> [u8; 16] {
    let mut h = [0u8; 16];
    h[..4].copy_from_slice(&id.to_be_bytes());
    h
}

/// The full link's state bytes of `m` (the format-5 writer).
fn full(m: &MemoryState) -> Vec<u8> {
    write::encode(&compact::normalize(m).expect("normalize")).expect("encode")
}

fn reason(r: Result<impl std::fmt::Debug, IncrReason>) -> IncrReason {
    r.expect_err("an error")
}

fn s(m: &MemoryState, x: Str) -> Vec<u8> {
    m.strings[x.off as usize..(x.off + x.len) as usize].to_vec()
}

// spec: incr.md IN-18c, IN-18e (whole read of a fresh state; written by
// the writer again, the same bytes: a full link's state is compact)
#[test]
fn round_trip_is_a_fixpoint() {
    let bytes = full(&sample(&[0, 1, 2], 3));
    let back = read::decode(&bytes).expect("decode");
    assert_eq!(full(&back), bytes);
    assert_eq!(
        state5::canon(&bytes).expect("canon"),
        state5::canon_of(&sample(&[0, 1, 2], 3)).expect("canon")
    );
    let view = read::open_bytes(bytes).expect("open");
    let names: Vec<SymbolId> = (0..3)
        .map(|i| view.find_key(format!("g{i}").as_bytes(), b"").expect("lookup").expect("found"))
        .collect();
    assert_eq!(names, vec![SymbolId(0), SymbolId(1), SymbolId(2)]);
}

// spec: incr.md IN-18e "SYMBOLS' row order"
#[test]
fn symbols_row_order_is_the_stated_one() {
    let m = compact::normalize(&sample(&[2, 0, 1], 1)).expect("normalize");
    let order: Vec<(u8, Vec<u8>)> = m.symbols.iter().map(|r| (r.kind, s(&m, r.name))).collect();
    let want: Vec<(u8, Vec<u8>)> = [
        (sym_kind::GLOBAL, "g0"),
        (sym_kind::GLOBAL, "g1"),
        (sym_kind::GLOBAL, "g2"),
        (sym_kind::GLOBAL, "defsym"),
        (sym_kind::LOCAL, "l0"),
        (sym_kind::LOCAL, "l1"),
        (sym_kind::LOCAL, "l2"),
    ]
    .into_iter()
    .map(|(k, n)| (k, n.as_bytes().to_vec()))
    .collect();
    assert_eq!(order, want);
    // files by rank: f0 is FileId 0, rank 0 << 20
    assert_eq!(s(&m, m.files[0].path), b"f0.o");
    assert_eq!(m.files[2].rank, 2 << 20);
}

// spec: incr.md IN-18c (every byte under a hash; the flags word outside)
#[test]
fn every_flipped_byte_is_state_corrupt() {
    let bytes = full(&sample(&[0, 1], 2));
    let mut flipped = 0;
    let mut check = |at: usize| {
        let mut b = bytes.clone();
        b[at] ^= 0x41;
        let r = read::decode(&b);
        assert!(r.is_err(), "byte {at} flipped: decoded");
        flipped += 1;
    };
    let dir_end = state5::HEADER + state5::TABLES.len() * state5::ENTRY;
    for at in (0..dir_end).filter(|a| !(12..16).contains(a)) {
        check(at);
    }
    // every list byte, and one byte of every block (its first, middle and
    // last)
    assert!(read::decode(&bytes).is_ok(), "clean");
    let dir = state5::parse_directory(
        &state5::Header::parse(&bytes).expect("h"),
        &bytes[state5::HEADER..dir_end],
    )
    .expect("dir");
    for e in &dir {
        for at in e.list..e.list + e.list_len() {
            check(usize::try_from(at).expect("small"));
        }
        for k in 0..e.blocks() {
            let start = e.body + k * state5::BLOCK as u64;
            let len = (e.body_len() - k * state5::BLOCK as u64).min(state5::BLOCK as u64);
            for at in [start, start + len / 2, start + len - 1] {
                check(usize::try_from(at).expect("small"));
            }
        }
    }
    assert!(flipped > 1000, "{flipped}");
    // the flags word is outside every hash
    let mut b = bytes;
    b[12] ^= 1;
    assert!(read::decode(&b).is_ok());
    assert!(read::refuse_updating(&b).is_err());
}

// spec: U-2 brief (incr_state refuses a state whose flag is set)
#[test]
fn incr_state_refuses_flag_set() {
    let mut bytes = full(&sample(&[0], 1));
    assert!(read::refuse_updating(&bytes).is_ok());
    bytes[12] |= 1;
    assert!(matches!(read::refuse_updating(&bytes), Err(StateError::Corrupt(_))));
}

/// A patch over a state of files `0..n` (the full set's ids), changing
/// file 0's sections (`grow` more of them), adding a key-ref and a frag.
fn edit(base: &MemoryState, grow: u32) -> StatePatch {
    let mut p = StatePatch::default();
    let n = base.sections.iter().filter(|r| r.file == 0).count() as u32 + grow;
    let rows: Vec<SectionRow> = (0..n)
        .map(|k| SectionRow { file: 0, ordinal: k, size: 32, ..SectionRow::default() })
        .collect();
    // the update numbers the moved rows as U-2 places them (IN-18e)
    let first = if n <= state5::range_capacity(n - grow) {
        0
    } else {
        u32::try_from(base.sections.len()).expect("small")
    };
    let mentions = base
        .mentions
        .iter()
        .filter(|r| r.0 == FileId(0))
        .map(|r| MentionRow { section: first, ..r.1 })
        .collect();
    let frag_uses = base.frag_uses.iter().filter(|r| r.0 == FileId(0)).map(|r| r.1).collect();
    p.ranges.push(FileRanges {
        file: FileId(0),
        first_global: base.file_index[0].first_global,
        sections: Some(rows),
        mentions: Some(mentions),
        index: None,
        edges: None,
        frag_uses: Some(frag_uses),
        symtab: None,
        regions: None,
        gone: false,
    });
    p.key_refs.put.push(KeyRefRow { key: 1, file: 0, ..KeyRefRow::default() });
    p.frags.put.push(FragRow {
        out: 9,
        class: CLASS,
        hash: [9; 16],
        offset: 72,
        ..FragRow::default()
    });
    p
}

/// W4's output row: the state's own (its path), retaken.
fn output_of(bytes: &[u8]) -> StatRow {
    let m = read::decode(bytes).expect("decode");
    let old = m.stat.iter().rfind(|r| r.tag == stat_tag::OUTPUT).expect("an output row");
    StatRow { size: 4242, ..old.clone() }
}

// spec: incr.md IN-46b (the in-place patch read back equals the rows
// rewritten in memory, ids included)
#[test]
fn patch_then_read_equals_rewrite() {
    let bytes = full(&sample(&[0, 1, 2], 3));
    for grow in [0, 1, 40] {
        let view = read::open_bytes(bytes.clone()).expect("open");
        let p = edit(&read::decode(&bytes).expect("decode"), grow);
        let plan = state5::plan_patch(&view, &p).expect("plan");
        assert!(!plan.compacts(), "{grow}: {}", plan.reason());
        let patched = write::patched_bytes(&bytes, &plan, &output_of(&bytes)).expect("in place");
        let got = read::decode(&patched).expect("the patched state reads");
        let mut want = read::decode(&bytes).expect("decode");
        compact::apply(&mut want, &p, 3).expect("apply");
        let out = want.stat.iter_mut().rfind(|r| r.tag == stat_tag::OUTPUT).expect("output row");
        *out = output_of(&bytes);
        assert_eq!(
            format!("{:?}", got.sections),
            format!("{:?}", want.sections),
            "{grow}: SECTIONS ids"
        );
        assert_eq!(format!("{:?}", got.frags), format!("{:?}", want.frags), "{grow}: FRAGS ids");
        assert_eq!(
            state5::canon_of(&got).expect("c"),
            state5::canon_of(&want).expect("c"),
            "{grow}"
        );
        assert!(got.meta.output_synced);
        let fresh = read::open_bytes(patched).expect("open the patched state");
        assert_eq!(fresh.key_refs(SymbolId(1)).expect("rows").len(), 2);
    }
}

// spec: incr.md IN-46b (O(change): the plan reads the blocks it writes,
// their lists, the header and the directory; numbers in WP-214's record)
#[test]
fn update_reads_only_touched_blocks() {
    let files: Vec<u32> = (0..400).collect();
    let bytes = full(&sample(&files, 4));
    let view = read::open_bytes(bytes.clone()).expect("open");
    let base = read::bytes_read(&view).expect("counted");
    let p = edit(&read::decode(&bytes).expect("decode"), 0);
    let plan = state5::plan_patch(&view, &p).expect("plan");
    assert!(!plan.compacts());
    let read = read::bytes_read(&view).expect("counted") - base;
    // header + directory at open; then: META's block, STAT's output block,
    // FILE_INDEX's block, SECTIONS' block (1 or 2), the key-ref and frag
    // lookups (log2 rows blocks each), their lists.
    let bound = 16 * state5::BLOCK as u64;
    eprintln!(
        "state {} bytes, open {base}, plan {read}, blocks written {}",
        bytes.len(),
        plan.blocks()
    );
    assert!(read <= bound, "{read} > {bound}");
    assert!(plan.blocks() <= 8, "{}", plan.blocks());
}

// spec: incr.md IN-46b "verified before re-hashed"
#[test]
fn patched_block_is_verified_first() {
    let bytes = full(&sample(&[0, 1, 2], 3));
    let dir_end = state5::HEADER + state5::TABLES.len() * state5::ENTRY;
    let dir = state5::parse_directory(
        &state5::Header::parse(&bytes).expect("h"),
        &bytes[state5::HEADER..dir_end],
    )
    .expect("dir");
    let sec = &dir[state5::table_index(kind::SECTIONS).expect("t")];
    let mut b = bytes.clone();
    b[usize::try_from(sec.body).expect("small")] ^= 1;
    // the open does not read bodies
    let view = read::open_bytes(b).expect("open");
    let p = edit(&read::decode(&bytes).expect("decode"), 0);
    assert!(matches!(reason(state5::plan_patch(&view, &p)), IncrReason::StateCorrupt(_)));
}

/// Files `0..3`; file 1 gone: its rows deleted, its keys `GONE`.
fn remove_file_1(m: &MemoryState) -> StatePatch {
    let mut p = StatePatch::default();
    let mut row = m.files[1].clone();
    row.flags |= file_flags::GONE;
    p.files_set.push((FileId(1), row));
    p.ranges.push(FileRanges {
        file: FileId(1),
        first_global: 0,
        sections: None,
        mentions: None,
        index: None,
        edges: None,
        frag_uses: None,
        symtab: None,
        regions: None,
        gone: true,
    });
    for (i, r) in m.symbols.iter().enumerate() {
        let name = s(m, r.name);
        if name == b"g1" || name == b"l1" {
            p.symbols.set.push((
                SymbolId(u32::try_from(i).expect("small")),
                SymbolRow { kind: sym_kind::GONE, ..*r },
            ));
        }
        if name == b"g1" {
            p.key_refs
                .delete
                .push(*m.key_refs.iter().find(|k| k.key == i as u32).expect("its ref"));
        }
    }
    p.frags.delete.push(*m.frags.iter().find(|f| f.hash == hash_of(2)).expect("its frag"));
    p.groups.delete.extend(m.groups.iter().filter(|g| g.file == 1).copied());
    p.commons.delete.extend(m.commons.iter().filter(|g| g.file == 1).copied());
    p.copies.delete.extend(m.copies.iter().filter(|g| g.file == 1).copied());
    p
}

// spec: incr.md IN-18f (after patches, the compaction's bytes equal the
// full link's of the same inputs, but META's history fields)
#[test]
fn compaction_equals_full_link_capture() {
    let bytes = full(&sample(&[0, 1, 2], 2));
    // hop 1: in place
    let view = read::open_bytes(bytes.clone()).expect("open");
    let m = read::decode(&bytes).expect("decode");
    let mut p1 = StatePatch::default();
    p1.stat_set.push((0, StatRow { size: 99, ..m.stat[0].clone() }));
    let plan = state5::plan_patch(&view, &p1).expect("plan");
    assert!(!plan.compacts());
    let bytes = write::patched_bytes(&bytes, &plan, &output_of(&bytes)).expect("in place");
    // hop 2: file 1 goes; a third of KEY_REFS dead: the compaction
    let view = read::open_bytes(bytes.clone()).expect("open");
    let m = read::decode(&bytes).expect("decode");
    let plan = state5::plan_patch(&view, &remove_file_1(&m)).expect("plan");
    assert!(plan.compacts(), "{}", plan.reason());
    let got = compact::plan_bytes(&plan, &output_of(&bytes)).expect("compacted");
    let mut want = sample(&[0, 2], 2);
    want.stat[0].size = 99;
    let n = want.stat.len();
    want.stat[n - 1].size = 4242;
    let mut want = compact::normalize(&want).expect("normalize");
    want.meta.output_synced = true;
    want.meta.compactions = 1;
    let want = write::encode(&want).expect("encode");
    assert_eq!(state5::canon(&got).expect("c"), state5::canon(&want).expect("c"));
    assert!(got == want, "compacted bytes differ from the full link's");
}

// spec: incr.md IN-18f step 1
#[test]
fn compaction_reads_every_block() {
    let bytes = full(&sample(&[0, 1, 2], 2));
    let dir_end = state5::HEADER + state5::TABLES.len() * state5::ENTRY;
    let dir = state5::parse_directory(
        &state5::Header::parse(&bytes).expect("h"),
        &bytes[state5::HEADER..dir_end],
    )
    .expect("dir");
    // the last block of PROBES (no row, no patch touches it)
    let e = &dir[state5::table_index(kind::PROBES).expect("t")];
    let mut b = bytes.clone();
    b[usize::try_from(e.body + e.body_len() - 1).expect("small")] ^= 1;
    let m = read::decode(&bytes).expect("decode");
    let in_place =
        state5::plan_patch(&read::open_bytes(b.clone()).expect("open"), &StatePatch::default());
    assert!(in_place.is_ok(), "an in-place plan reads only its blocks");
    let r = state5::plan_patch(&read::open_bytes(b).expect("open"), &remove_file_1(&m));
    assert!(matches!(reason(r), IncrReason::StateCorrupt(_)));
}

// spec: incr.md IN-18e (a new file's rank ties a live file's: no midpoint
// left, the compaction renumbers ranks)
#[test]
fn rank_midpoint_exhausted_compacts() {
    let bytes = full(&sample(&[0, 1, 2], 1));
    let m = read::decode(&bytes).expect("decode");
    let view = read::open_bytes(bytes.clone()).expect("open");
    let mut p = StatePatch::default();
    p.files_append.push(FileRow { rank: (1 << 20) + 1, ..m.files[0].clone() });
    assert!(!state5::plan_patch(&view, &p).expect("plan").compacts(), "a midpoint");
    let mut p = StatePatch::default();
    p.files_append.push(FileRow { rank: 1 << 20, ..m.files[0].clone() });
    let plan = state5::plan_patch(&view, &p).expect("plan");
    assert!(plan.compacts());
    assert!(plan.reason().contains("rank"), "{}", plan.reason());
    let c = compact::plan_bytes(&plan, &output_of(&bytes)).expect("compacted");
    let ranks: Vec<u64> = read::decode(&c).expect("d").files.iter().map(|f| f.rank).collect();
    assert_eq!(ranks, vec![0, 1 << 20, 2 << 20, 3 << 20]);
}

// spec: incr.md IN-18e (KEY_REFS: one row per (key, file); a key with one
// mentioner has one row, gone with the file)
#[test]
fn key_refs_single_mentioner() {
    let bytes = full(&sample(&[0, 1, 2], 1));
    let view = read::open_bytes(bytes.clone()).expect("open");
    let g1 = view.find_key(b"g1", b"").expect("lookup").expect("g1");
    let rows = view.key_refs(g1).expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].file, 1);
    let m = read::decode(&bytes).expect("decode");
    let mut p = remove_file_1(&m);
    p.frags = KeyEdits::default();
    p.key_refs.delete.clear();
    p.key_refs.delete.push(rows[0]);
    let plan = state5::plan_patch(&view, &p).expect("plan");
    let after = match write::patched_bytes(&bytes, &plan, &output_of(&bytes)) {
        Some(b) => b,
        None => compact::plan_bytes(&plan, &output_of(&bytes)).expect("compacted"),
    };
    let v = read::open_bytes(after).expect("open");
    assert!(
        v.find_key(b"g1", b"")
            .expect("lookup")
            .is_none_or(|k| v.key_refs(k).expect("rows").is_empty())
    );
}

// spec: incr.md IN-18e "Size, restated" (the sample's table sizes; the
// gate's numbers on protoc and the bench's workload are WP-214's record,
// from `incr_state size`)
#[test]
fn state5_size_report() {
    let files: Vec<u32> = (0..200).collect();
    let bytes = full(&sample(&files, 4));
    let report = state5::size_report(&bytes).expect("report");
    eprintln!("{report}");
    assert!(report.contains("SECTIONS"));
    assert!(report.contains("lazy members"));
}

// spec: incr.md IN-46b (the writes, through ops: an empty plan writes
// nothing; a plan of another view is refused)
#[test]
fn patch_writes_through_ops_and_checks_its_view() {
    let m = MemoryState::default();
    let view = m.view();
    let plan = state5::PatchPlan::default();
    assert!(state5::patch(&view, &plan, &StatRow::default(), &mut StdOps).is_ok());
    let r = compact(&view, &plan, &StatRow::default(), &mut StdOps, std::path::Path::new("never"));
    assert!(matches!(r, Err(WriteFailure::Fallback(IncrReason::StateCorrupt(_)))), "{r:?}");
    let rows = MemoryLayout::default();
    let p_u = StoredLayout::new(&rows);
    let c = state5::canon_of(&sample(&[0], 1)).expect("canon");
    assert!(state5::compare(&c, &c, &p_u).is_empty());
    let d = state5::compare(&c, &state5::canon_of(&sample(&[1], 1)).expect("canon"), &p_u);
    assert!(!d.is_empty());
}

// spec: incr.md IN-46b, IN-77 (patch and compact on a real descriptor)
#[test]
fn patch_and_compact_on_a_file() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("state5-file");
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("s.mortar-state");
    let bytes = full(&sample(&[0, 1, 2], 2));
    std::fs::write(&path, &bytes).expect("write");
    let file = std::fs::OpenOptions::new().read(true).write(true).open(&path).expect("open");
    let view = state5::open(&file, [7; 16]).expect("open");
    let m = read::decode(&bytes).expect("decode");
    let plan = state5::plan_patch(&view, &edit(&m, 0)).expect("plan");
    state5::patch(&view, &plan, &output_of(&bytes), &mut StdOps).expect("patch");
    let on_disk = std::fs::read(&path).expect("read");
    assert_eq!(on_disk, write::patched_bytes(&bytes, &plan, &output_of(&bytes)).expect("in place"));
    drop(view);
    let view = state5::open(&file, [7; 16]).expect("open");
    let m = read::decode(&on_disk).expect("decode");
    let mut p = remove_file_1(&m);
    p.key_refs
        .delete
        .push(*m.key_refs.iter().find(|k| k.file == 0 && k.key == 1).expect("the added ref"));
    let plan = state5::plan_patch(&view, &p).expect("plan");
    assert!(plan.compacts());
    compact(&view, &plan, &output_of(&bytes), &mut StdOps, &path).expect("compact");
    assert_eq!(
        std::fs::read(&path).expect("read"),
        compact::plan_bytes(&plan, &output_of(&bytes)).expect("bytes")
    );
    assert!(state5::open(&file, [8; 16]).is_err(), "another plan hash: options-changed");
    let _ = NameRow::default();
}

// spec: incr.md IN-18e's GROUPS (44, by (signature, file)), COMMONS (46, by
// (key, file)), COPIES (55, by (key, file, index)), META's `iterations` and
// `versioned`, FILE_INDEX's `first_global` (WP-215, WP-220): written, read
// back by accessor and decode, kept by a patch and by the compaction
#[test]
#[allow(clippy::many_single_char_names)]
fn seam_rows_round_trip_and_patch() {
    let bytes = full(&sample(&[0, 1, 2], 2));
    let view = read::open_bytes(bytes.clone()).expect("open");
    let mut files: Vec<u32> = view.group(b"grp").expect("rows").iter().map(|g| g.file).collect();
    files.sort_unstable();
    assert_eq!(files, vec![0, 1, 2], "one GROUPS row per copy, no keeper");
    assert!(view.group(b"grq").expect("rows").is_empty());
    let key = view.find_key(b"defsym", b"").expect("lookup").expect("defsym");
    assert_eq!(view.commons(key).expect("rows").len(), 3);
    let win = view.common(key).expect("rows").expect("a winner");
    assert_eq!((win.file, win.size), (0, 24), "least rank, largest size");
    let copies = view.copies(key).expect("rows");
    assert_eq!(copies.len(), 3);
    assert!(copies.iter().all(|c| c.size == 16 && c.typ == 2 && c.index == 2));
    let m = read::decode(&bytes).expect("decode");
    assert_eq!((m.meta.iterations, m.meta.versioned), (1, 3));
    let fg: Vec<u32> = m.file_index.iter().map(|i| i.first_global).collect();
    assert_eq!(fg, vec![2, 3, 4]);
    // in place: file 2's copy goes, a new one of file 0 at index 5
    let mut p = StatePatch::default();
    p.copies.delete.extend(m.copies.iter().filter(|c| c.file == 2).copied());
    p.copies.put.push(CopyRow { key: key.0, file: 0, index: 5, size: 4, typ: 1, tomb: false });
    let plan = state5::plan_patch(&view, &p).expect("plan");
    assert!(!plan.compacts(), "{}", plan.reason());
    let patched = write::patched_bytes(&bytes, &plan, &output_of(&bytes)).expect("in place");
    let v = read::open_bytes(patched.clone()).expect("open");
    let mut got: Vec<(u32, u32)> =
        v.copies(key).expect("rows").iter().map(|c| (c.file, c.index)).collect();
    got.sort_unstable();
    assert_eq!(got, vec![(0, 2), (0, 5), (1, 2)]);
    let d = read::decode(&patched).expect("decode");
    assert_eq!(d.file_index.iter().map(|i| i.first_global).collect::<Vec<_>>(), fg);
    // compaction: file 1 goes; its GROUPS, COMMONS and COPIES rows with it
    let plan = state5::plan_patch(&view, &remove_file_1(&m)).expect("plan");
    assert!(plan.compacts(), "{}", plan.reason());
    let c = compact::plan_bytes(&plan, &output_of(&bytes)).expect("compacted");
    let d = read::decode(&c).expect("decode");
    assert_eq!(d.groups.len(), 2);
    assert_eq!(d.commons.iter().map(|r| r.file).collect::<Vec<_>>(), vec![0, 1]);
    assert_eq!(d.copies.len(), 2);
    assert_eq!(d.file_index.iter().map(|i| i.first_global).collect::<Vec<_>>(), vec![2, 4]);
    assert_eq!((d.meta.iterations, d.meta.versioned), (1, 3));
}

/// A string of the update's own pool (U-10's `ChangedFiles::strings`
/// convention): its bytes appended to `p.strings`, at the state's length
/// plus their place.
fn fresh_str(p: &mut StatePatch, old: usize, b: &[u8]) -> Str {
    let off = u32::try_from(old + p.strings.len()).expect("small");
    p.strings.extend_from_slice(b);
    Str { off, len: u32::try_from(b.len()).expect("small") }
}

// spec: incr.md IN-18e (F-11: an update appends a string only when the
// state does not hold its bytes where the update looks: the old version's
// rows of the same file, a GROUPS row of the same signature); IN-78 (the
// deduplicated state's canon equals a fresh capture's)
#[test]
fn update_reuses_stored_strings() {
    let bytes = full(&sample(&[0, 1, 2], 3));
    let m = read::decode(&bytes).expect("decode");
    let old = m.strings.len();
    let view = read::open_bytes(bytes.clone()).expect("open");
    // file 0 recompiled: its three sections again (every name and anchor
    // in the update's own pool), one new section, its group's copy again
    let mut p = StatePatch::default();
    let mut rows = Vec::new();
    for r in m.sections.iter().filter(|r| r.file == 0) {
        let name = fresh_str(&mut p, old, &s(&m, r.name));
        rows.push(SectionRow { name, size: 32, ..r.clone() });
    }
    let name = fresh_str(&mut p, old, b".text.0.new");
    let twice = fresh_str(&mut p, old, b".text.0.new");
    rows.push(SectionRow { file: 0, name, ordinal: 3, size: 32, ..SectionRow::default() });
    let mut fr = FileRanges {
        file: FileId(0),
        first_global: m.file_index[0].first_global,
        sections: Some(rows),
        mentions: None,
        index: None,
        edges: None,
        frag_uses: None,
        symtab: None,
        regions: None,
        gone: false,
    };
    // the same new bytes named twice: one place
    if let Some(rows) = &mut fr.sections {
        rows[0].anchor = twice;
        rows[0].anchor_kind = 2;
    }
    p.ranges.push(fr);
    let signature = fresh_str(&mut p, old, b"grp");
    p.groups.put.push(GroupRow { signature, file: 0, tomb: false });
    let appended = p.strings.len();
    let plan = state5::plan_patch(&view, &p).expect("plan");
    assert!(!plan.compacts(), "{}", plan.reason());
    let patched = write::patched_bytes(&bytes, &plan, &output_of(&bytes)).expect("in place");
    let got = read::decode(&patched).expect("decode");
    let grew = got.strings.len() - old;
    eprintln!("the patch's pool {appended} bytes, STRINGS grew {grew} bytes");
    assert_eq!(grew, b".text.0.new".len(), "only the new name is appended, once");
    // the fresh capture of the same inputs
    let mut want = sample(&[0, 1, 2], 3);
    let new = add(&mut want.strings, b".text.0.new");
    for r in want.sections.iter_mut().filter(|r| r.file == 0) {
        r.size = 32;
    }
    let at = want.sections.iter().rposition(|r| r.file == 0).expect("file 0") + 1;
    want.sections.insert(
        at,
        SectionRow { file: 0, name: new, ordinal: 3, size: 32, ..SectionRow::default() },
    );
    if let Some(r) = want.sections.iter_mut().find(|r| r.file == 0) {
        r.anchor = new;
        r.anchor_kind = 2;
    }
    for (_, r) in &mut want.mentions {
        if r.section >= u32::try_from(at).expect("small") {
            r.section += 1;
        }
    }
    let n = want.stat.len();
    want.stat[n - 1].size = 4242;
    assert_eq!(
        state5::canon(&patched).expect("canon"),
        state5::canon(&full(&want)).expect("canon"),
        "the deduplicated state's canon is the fresh capture's"
    );
}

// spec: incr.md IN-71 (`at_file` is the first LIVE mentioner by current
// rank), IN-18e (SYMBOLS' order), IN-18f, IN-78: after the mentioner a
// key's `at_file` named goes, a compaction and a fresh capture of the
// remaining inputs give the SYMBOLS rows in one order
#[test]
fn compaction_and_capture_order_symbols_alike() {
    // `shared`: mentioned by files 1 and 2 (entry 3 of each)
    let with_shared = |ids: &[u32]| {
        let mut m = sample(ids, 2);
        let name = add(&mut m.strings, b"shared");
        let k = u32::try_from(m.symbols.len()).expect("small");
        let mentioners: Vec<u32> = ids
            .iter()
            .enumerate()
            .filter(|(_, id)| **id != 0)
            .map(|(f, _)| u32::try_from(f).expect("small"))
            .collect();
        m.symbols.push(SymbolRow {
            kind: sym_kind::GLOBAL,
            file: u32::MAX,
            name,
            at_file: mentioners[0],
            at_index: 3,
            ..SymbolRow::default()
        });
        m.resolved.push(ResolvedRow { section: u32::MAX, ..ResolvedRow::default() });
        m.symtab_keys.push(SymtabKeyRow { index: u32::MAX, strtab: 0 });
        for &f in &mentioners {
            m.mentions.push((FileId(f), MentionRow { symbol: k, ..MentionRow::default() }));
            m.key_refs.push(KeyRefRow { key: k, file: f, ..KeyRefRow::default() });
        }
        m
    };
    let bytes = full(&with_shared(&[0, 1, 2]));
    let m = read::decode(&bytes).expect("decode");
    let view = read::open_bytes(bytes.clone()).expect("open");
    let shared = view.find_key(b"shared", b"").expect("lookup").expect("shared");
    let mut p = remove_file_1(&m);
    // IN-71: the next live mentioner by rank is file 2
    let row = m.symbols[shared.0 as usize];
    assert_eq!(row.at_file, 1);
    p.symbols.set.push((shared, SymbolRow { at_file: 2, ..row }));
    p.key_refs
        .delete
        .push(*m.key_refs.iter().find(|r| r.key == shared.0 && r.file == 1).expect("its ref"));
    let plan = state5::plan_patch(&view, &p).expect("plan");
    assert!(plan.compacts(), "{}", plan.reason());
    let got = compact::plan_bytes(&plan, &output_of(&bytes)).expect("compacted");
    let mut want = with_shared(&[0, 2]);
    let n = want.stat.len();
    want.stat[n - 1].size = 4242;
    let want = full(&want);
    let order = |b: &[u8]| -> Vec<Vec<u8>> {
        let d = read::decode(b).expect("decode");
        d.symbols.iter().map(|r| s(&d, r.name)).collect()
    };
    assert_eq!(order(&got), order(&want), "SYMBOLS' row order");
    assert_eq!(state5::canon(&got).expect("c"), state5::canon(&want).expect("c"));
}

/// `m` without its `SYMBOLS` row `k` (every reference renumbered): the
/// fresh capture of inputs that no longer name that key.
fn drop_key(m: &mut MemoryState, k: u32) {
    let fix = |x: &mut u32| {
        assert_ne!(*x, k, "a row still names the dropped key");
        if *x > k {
            *x -= 1;
        }
    };
    m.symbols.remove(k as usize);
    m.resolved.remove(k as usize);
    m.symtab_keys.remove(k as usize);
    for (_, r) in &mut m.mentions {
        fix(&mut r.symbol);
    }
    for r in &mut m.key_refs {
        fix(&mut r.key);
    }
    for r in &mut m.commons {
        fix(&mut r.key);
    }
    for r in &mut m.copies {
        fix(&mut r.key);
    }
}

// spec: WP-239 seam 5 as format 5 (FRAGS by (out, class, hash) with
// `refs` per file, FRAG_USE 56, FILES' eh columns, KEY_REFS.demand,
// SLOTS.refs, RESOLVED.facts, FILE_INDEX.first_global from FileRanges;
// no `.debug_*` fragment): written, read back by the accessors and
// `LayoutOf`, patched
#[test]
#[allow(clippy::too_many_lines)]
fn seam5_rows_round_trip_and_patch() {
    use mortar_core::incr::LayoutRows as _;
    use mortar_core::{OutSecId, SectionId};
    use mortar_incr::update::rows::SlotRow;
    let mut m = sample(&[0, 1, 2], 2);
    // a `.debug_str` output section with a fragment every file names
    let name = add(&mut m.strings, b".debug_str");
    m.outsecs.push(mortar_incr::format::OutSecRow { name, ordinal: 0 });
    let name = add(&mut m.strings, b".rodata");
    m.outsecs.insert(0, mortar_incr::format::OutSecRow { name, ordinal: 0 });
    m.frags.push(FragRow { out: 1, class: CLASS, hash: [5; 16], offset: 0, refs: 3, tomb: false });
    for f in 0..3 {
        let row = FragUseRow { file: f, out: 1, class: CLASS, hash: [5; 16] };
        m.frag_uses.push((FileId(f), row));
    }
    m.slots.push(SlotRow { kind: 0, symbol: 1, refs: 2 });
    let bytes = full(&m);
    let d = read::decode(&bytes).expect("decode");
    assert!(d.frags.iter().all(|r| r.out == 0), "no .debug_* row");
    assert!(d.frag_uses.iter().all(|r| r.1.out == 0), "no .debug_* row");
    assert_eq!(d.frags.len(), 4);
    assert_eq!(d.frag_uses.len(), 6);
    let view = read::open_bytes(bytes.clone()).expect("open");
    let shared = view.frags_of(OutSecId(0), CLASS, hash_of(0)).expect("lookup").expect("a row");
    assert_eq!((shared.offset, shared.refs), (0, 3), "refs: the files with a FRAG_USE row");
    assert!(view.frags_of(OutSecId(0), CLASS + 1, hash_of(0)).expect("lookup").is_none());
    let layout = read::LayoutOf { view: &view };
    let f = layout.frags_of(OutSecId(0), CLASS, hash_of(2)).expect("lookup").expect("a row");
    assert_eq!((f.class, f.offset, f.refs), (CLASS, 16, 1));
    let uses = layout.frag_uses_of(FileId(1)).expect("rows");
    let mut want = vec![hash_of(0), hash_of(2)];
    want.sort_unstable();
    assert_eq!(uses.iter().map(|u| u.hash).collect::<Vec<_>>(), want, "by key");
    assert!(uses.iter().all(|u| u.file == FileId(1) && u.class == CLASS));
    let eh = layout.eh_range_of(FileId(2)).expect("row").expect("a range");
    assert_eq!((eh.offset, eh.capacity, eh.records), (128, 40, 2));
    let refs = layout.key_refs_of(FileId(1)).expect("rows");
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].demand.bits(), 1);
    assert_eq!(view.resolved(refs[0].key).expect("row").facts, FACTS);
    let slots = layout.slots().expect("rows");
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].refs, 2);
    let first = d.file_index[1].ranges[0].first;
    let sec = layout.section(SectionId(first + 1)).expect("row").expect("a section");
    assert_eq!((sec.file, sec.index), (FileId(1), 1));
    assert!(layout.section(SectionId(first + 2)).expect("row").is_none(), "slack");
    let past = u32::try_from(view.count(kind::SECTIONS)).expect("small");
    assert!(layout.section(SectionId(past)).expect("row").is_none(), "new to the update");
    // a patch: file 1's FRAG_USE range and first_global rewritten; a new
    // fragment; file 1's eh columns and a key's facts changed
    let mut p = StatePatch::default();
    p.ranges.push(FileRanges {
        file: FileId(1),
        first_global: 9,
        sections: None,
        mentions: None,
        index: None,
        edges: None,
        frag_uses: Some(vec![FragUseRow { file: 1, out: 0, class: CLASS, hash: [7; 16] }]),
        symtab: None,
        regions: None,
        gone: false,
    });
    p.frags.put.push(FragRow {
        out: 0,
        class: CLASS,
        hash: [7; 16],
        offset: 99,
        refs: 1,
        tomb: false,
    });
    p.files_set.push((
        FileId(1),
        FileRow { eh_offset: 1, eh_capacity: 2, eh_records: 1, ..d.files[1].clone() },
    ));
    p.resolved
        .set
        .push((refs[0].key, ResolvedRow { facts: 7, ..d.resolved[refs[0].key.0 as usize] }));
    let plan = state5::plan_patch(&view, &p).expect("plan");
    assert!(!plan.compacts(), "{}", plan.reason());
    let patched = write::patched_bytes(&bytes, &plan, &output_of(&bytes)).expect("in place");
    let v = read::open_bytes(patched.clone()).expect("open");
    let layout = read::LayoutOf { view: &v };
    assert_eq!(v.file_index(FileId(1)).expect("row").first_global, 9, "F-10");
    let uses = layout.frag_uses_of(FileId(1)).expect("rows");
    assert_eq!(uses.iter().map(|u| u.hash).collect::<Vec<_>>(), vec![[7; 16]]);
    let f = layout.frags_of(OutSecId(0), CLASS, [7; 16]).expect("lookup").expect("a row");
    assert_eq!(f.offset, 99);
    let eh = layout.eh_range_of(FileId(1)).expect("row").expect("a range");
    assert_eq!((eh.offset, eh.capacity, eh.records), (1, 2, 1));
    assert_eq!(v.resolved(refs[0].key).expect("row").facts, 7);
    // the compaction recounts `refs` from FRAG_USE: the shared fragment
    // lost file 1
    let mut c = read::decode(&patched).expect("decode");
    compact::apply(&mut c, &StatePatch::default(), 0).expect("apply");
    let n = compact::normalize(&c).expect("normalize");
    let r = n.frags.iter().find(|r| r.hash == hash_of(0)).expect("shared");
    assert_eq!(r.refs, 2);
    // the same through the in-memory path
    let mut want = read::decode(&bytes).expect("decode");
    compact::apply(&mut want, &p, 4).expect("apply");
    let out = want.stat.iter_mut().rfind(|r| r.tag == stat_tag::OUTPUT).expect("output row");
    *out = output_of(&bytes);
    assert_eq!(
        state5::canon_of(&read::decode(&patched).expect("decode")).expect("c"),
        state5::canon_of(&want).expect("c")
    );
}

// spec: WP-239 R-2 (`state-collision`: two different fragments with one
// key at the capture's sort; no state is written), S4-3 (`state-limit`:
// an `.eh_frame` range past the `u32` columns); the injection point is
// `state5::check_capture` over the capture's rows (no format-5 capture
// from a `Link` exists yet: WP-214 §10)
#[test]
fn capture_collision_and_limit_are_refused() {
    let mut m = sample(&[0, 1], 1);
    assert_eq!(state5::check_capture(&m), Ok(()));
    // the same row twice is one fragment
    let again = m.frags[0];
    m.frags.push(again);
    assert_eq!(state5::check_capture(&m), Ok(()));
    // an injected collision: another fragment (another offset) of one key
    m.frags.push(FragRow { offset: 4096, ..again });
    let r = state5::check_capture(&m).expect_err("a collision");
    assert_eq!(r.name(), "state-collision", "{r:?}");
    // the writer refuses it too (a defect if the capture did not)
    assert!(compact::normalize(&m).is_err());
    assert_eq!(state5::eh_columns(64, 40, 2), Ok([64, 40, 2]));
    let r = state5::eh_columns(u64::from(u32::MAX) - 8, 40, 2).expect_err("past u32");
    assert_eq!(r.name(), "state-limit", "{r:?}");
    let r = state5::eh_columns(0, 1 << 32, 2).expect_err("past u32");
    assert_eq!(r.name(), "state-limit");
}

// spec: incr.md IN-71b "Live keys", IN-78 canon (wp-215-rulings-3 item 3):
// an update leaves a dead key (its last mention went, the key not
// `GONE`); the compaction and the canon drop it by `live_keys`; the
// result equals a fresh capture's of inputs that never named it
#[test]
fn dead_key_dropped_by_compaction() {
    use mortar_core::incr::{OptionKey, OptionKeys};
    // eight files: one KEY_REFS tombstone in eight stays in place
    let ids: Vec<u32> = (0..8).collect();
    let bytes = full(&sample(&ids, 2));
    let m = read::decode(&bytes).expect("decode");
    let view = read::open_bytes(bytes.clone()).expect("open");
    let g1 = view.find_key(b"g1", b"").expect("lookup").expect("g1");
    // file 1 recompiled without g1: its MENTIONS range emptied, its
    // KEY_REFS row deleted; the key's SYMBOLS row is left as it is
    let mut p = StatePatch::default();
    p.ranges.push(FileRanges {
        file: FileId(1),
        first_global: m.file_index[1].first_global,
        sections: None,
        mentions: Some(Vec::new()),
        index: None,
        edges: None,
        frag_uses: None,
        symtab: None,
        regions: None,
        gone: false,
    });
    p.key_refs.delete.push(*m.key_refs.iter().find(|r| r.key == g1.0).expect("its row"));
    let plan = state5::plan_patch(&view, &p).expect("plan");
    assert!(!plan.compacts(), "{}", plan.reason());
    let patched = write::patched_bytes(&bytes, &plan, &output_of(&bytes)).expect("in place");
    let left = read::decode(&patched).expect("decode");
    assert_eq!(left.symbols[g1.0 as usize].kind, sym_kind::GLOBAL, "left in place");
    let live = state5::live_keys(&left, &OptionKeys::default()).expect("rule");
    assert!(!live[g1.0 as usize], "dead");
    assert_eq!(live.iter().filter(|x| !**x).count(), 1, "only g1");
    // an option naming it keeps it
    let named = OptionKeys {
        undefined: vec![OptionKey { name: b"g1".to_vec().into_boxed_slice(), arg: 1 }],
        ..OptionKeys::default()
    };
    assert!(state5::live_keys(&left, &named).expect("rule")[g1.0 as usize]);
    // the compaction (apply + normalize, the plan's own steps) drops it
    let mut c = read::decode(&bytes).expect("decode");
    compact::apply(&mut c, &p, 4).expect("apply");
    let n = compact::normalize(&c).expect("normalize");
    assert!(n.symbols.iter().all(|r| s(&n, r.name) != b"g1"), "dropped");
    // the fresh capture of the same inputs: g1 never named
    let mut want = sample(&ids, 2);
    let k = want.symbols.iter().position(|r| s(&want, r.name) == b"g1").expect("g1");
    want.mentions.retain(|(f, _)| *f != FileId(1));
    want.key_refs.retain(|r| r.file != 1);
    drop_key(&mut want, u32::try_from(k).expect("small"));
    let n_want = want.stat.len();
    want.stat[n_want - 1].size = 4242;
    let out = c.stat.iter_mut().rfind(|r| r.tag == stat_tag::OUTPUT).expect("output row");
    *out = output_of(&bytes);
    let (a, b) = (state5::canon_of(&c).expect("c"), state5::canon(&full(&want)).expect("c"));
    let rows = MemoryLayout::default();
    let diffs = state5::compare(&a, &b, &StoredLayout::new(&rows));
    assert!(diffs.is_empty(), "{diffs:#?}");
    // and the bytes but META's history (`output-synced`: the patch's)
    let mut n = compact::normalize(&c).expect("n");
    n.meta.output_synced = false;
    assert_eq!(write::encode(&n).expect("e"), full(&want));
    let mut cp = read::decode(&patched).expect("decode");
    let out = cp.stat.iter_mut().rfind(|r| r.tag == stat_tag::OUTPUT).expect("output row");
    *out = output_of(&bytes);
    assert_eq!(state5::canon_of(&cp).expect("c"), state5::canon(&full(&want)).expect("c"));
}

// spec: U-3's §R.7 (WP-214 §10): a new local key's `SYMBOLS.ordinal` is
// its rank among its file's locals of the same name by symbol-table
// index (the capture's `LocalIdentity::ordinal`), written by the plan
#[test]
fn new_local_keys_get_their_ordinal() {
    let bytes = full(&sample(&[0, 1], 1));
    let m = read::decode(&bytes).expect("decode");
    let old = m.strings.len();
    let view = read::open_bytes(bytes.clone()).expect("open");
    let mut p = StatePatch::default();
    for at in [7, 3, 5] {
        let name = fresh_str(&mut p, old, if at == 5 { b"y" } else { b"x" });
        p.symbols.append.push(SymbolRow {
            kind: sym_kind::LOCAL,
            file: 0,
            name,
            at_file: 0,
            at_index: at,
            ordinal: 0,
            ..SymbolRow::default()
        });
    }
    let plan = state5::plan_patch(&view, &p).expect("plan");
    let patched = write::patched_bytes(&bytes, &plan, &output_of(&bytes)).expect("in place");
    let d = read::decode(&patched).expect("decode");
    let got: Vec<(u32, u32)> =
        d.symbols[m.symbols.len()..].iter().map(|r| (r.at_index, r.ordinal)).collect();
    assert_eq!(got, vec![(7, 1), (3, 0), (5, 0)]);
}
