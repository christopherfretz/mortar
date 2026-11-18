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
    CommonRow, CopyRow, DebugStrRow, EdgeRow, FileIndexRow, FileRow, FragRow, FragUseRow, GroupRow,
    KeyRefRow, LocalRange, MentionRow, NameRow, ResolvedRow, SymbolRow, SymtabKeyRow,
    debug_str_class, edge_kind, file_flags, sym_kind,
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
#[allow(clippy::too_many_lines)]
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
    // an unmentioned key (a --defsym: its option bit, seam 6), early
    let dname = add(&mut pool, b"defsym");
    m.symbols.insert(
        0,
        SymbolRow {
            kind: sym_kind::GLOBAL,
            file: u32::MAX,
            name: dname,
            at_file: u32::MAX,
            option: true,
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
        locals: None,
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

/// Files `0..3`; file 1 gone: its rows deleted, its global `GONE` (its
/// local by the plan, through its `FILE_INDEX.locals`: seam 6).
fn remove_file_1(m: &MemoryState) -> StatePatch {
    let mut p = StatePatch::default();
    let mut row = m.files[1].clone();
    row.flags |= file_flags::GONE;
    p.files_set.push((FileId(1), row));
    p.ranges.push(FileRanges {
        file: FileId(1),
        first_global: 0,
        locals: None,
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
        if name == b"g1" {
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
        locals: None,
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
#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
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
    m.frags.push(FragRow {
        out: 1,
        class: CLASS,
        hash: [5; 16],
        offset: 0,
        size: 4,
        refs: 3,
        tomb: false,
    });
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
        locals: None,
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
        size: 4,
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
#[allow(clippy::many_single_char_names)]
fn dead_key_dropped_by_compaction() {
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
        locals: None,
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
    let live = state5::live_keys(&left).expect("rule");
    assert!(!live[g1.0 as usize], "dead");
    assert_eq!(live.iter().filter(|x| !**x).count(), 1, "only g1");
    // its option bit (an option names it, seam 6) keeps it
    let mut named = left.clone();
    named.symbols[g1.0 as usize].option = true;
    assert!(state5::live_keys(&named).expect("rule")[g1.0 as usize]);
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

/// A patch replacing file `f`'s symbol table by locals named `names` (at
/// indexes 1..), appended to a state of `syms` keys (seam 6: the old run
/// is retired by the plan).
fn new_locals(p: &mut StatePatch, m: &MemoryState, f: u32, names: &[&[u8]]) {
    let syms = u32::try_from(m.symbols.len() + p.symbols.append.len()).expect("small");
    let old = m.strings.len();
    for (i, n) in names.iter().enumerate() {
        let name = fresh_str(p, old, n);
        p.symbols.append.push(SymbolRow {
            kind: sym_kind::LOCAL,
            file: f,
            name,
            at_file: f,
            at_index: 1 + u32::try_from(i).expect("small"),
            ..SymbolRow::default()
        });
        p.resolved.append.push(ResolvedRow {
            section: u32::MAX,
            facts: FACTS,
            ..ResolvedRow::default()
        });
    }
    let count = u32::try_from(names.len()).expect("small");
    p.ranges.push(FileRanges {
        file: FileId(f),
        first_global: m.file_index[f as usize].first_global,
        sections: None,
        mentions: None,
        index: None,
        edges: None,
        frag_uses: None,
        symtab: None,
        regions: None,
        locals: Some(LocalRange { first: syms, count }),
        gone: false,
    });
}

fn local_names(m: &MemoryState, f: u32) -> Vec<Vec<u8>> {
    let run = m.file_index[f as usize].locals;
    m.symbols[run.first as usize..(run.first + run.count) as usize]
        .iter()
        .map(|r| s(m, r.name))
        .collect()
}

// spec: incr.md IN-18d (WP-214 seam 6, ruling 1): a changed file's old
// locals are one run named by its FILE_INDEX row, marked GONE by the plan
// with no pass over SYMBOLS; after three hops the canon equals a fresh
// capture's; the plan reads O(the change)
#[test]
#[allow(clippy::many_single_char_names)]
fn changed_file_old_locals_are_gone() {
    let mut bytes = full(&sample(&[0, 1, 2], 1));
    let first = read::decode(&bytes).expect("decode");
    assert_eq!(local_names(&first, 1), [b"l1".to_vec()], "the capture's run");
    let hops: [(u32, &[&[u8]]); 3] =
        [(1, &[b"l1a".as_slice(), b"l1b"]), (1, &[b"l1c".as_slice()]), (0, &[])];
    for (hop, (f, names)) in hops.into_iter().enumerate() {
        let m = read::decode(&bytes).expect("decode");
        let old_run = m.file_index[f as usize].locals;
        let mut p = StatePatch::default();
        new_locals(&mut p, &m, f, names);
        let view = read::open_bytes(bytes.clone()).expect("open");
        let plan = state5::plan_patch(&view, &p).expect("plan");
        assert!(!plan.compacts(), "hop {hop}: {}", plan.reason());
        let patched = write::patched_bytes(&bytes, &plan, &output_of(&bytes)).expect("in place");
        let got = read::decode(&patched).expect("the patched state reads");
        for k in old_run.first..old_run.first + old_run.count {
            assert_eq!(got.symbols[k as usize].kind, sym_kind::GONE, "hop {hop}: row {k}");
        }
        let want_names: Vec<Vec<u8>> = names.iter().map(|n| n.to_vec()).collect();
        assert_eq!(local_names(&got, f), want_names, "hop {hop}: the new run");
        // the compaction's apply does the same
        let mut want = m.clone();
        compact::apply(&mut want, &p, 3).expect("apply");
        let out = want.stat.iter_mut().rfind(|r| r.tag == stat_tag::OUTPUT).expect("output row");
        *out = output_of(&bytes);
        assert_eq!(state5::canon_of(&got).expect("c"), state5::canon_of(&want).expect("c"));
        bytes = patched;
    }
    // the fresh capture of the inputs as they now are: l0 gone, l1 is l1c
    let mut want = sample(&[0, 1, 2], 1);
    let l1c = add(&mut want.strings, b"l1c");
    let k = want.symbols.iter().position(|r| s(&want, r.name) == b"l1").expect("l1");
    want.symbols[k].name = l1c;
    let k = want.symbols.iter().position(|r| s(&want, r.name) == b"l0").expect("l0");
    drop_key(&mut want, u32::try_from(k).expect("small"));
    let n = want.stat.len();
    want.stat[n - 1].size = 4242;
    let (a, b) = (state5::canon(&bytes).expect("c"), state5::canon(&full(&want)).expect("c"));
    let rows = MemoryLayout::default();
    let diffs = state5::compare(&a, &b, &StoredLayout::new(&rows));
    assert!(diffs.is_empty(), "{diffs:#?}");
    // a new run that is not the file's appended LOCAL rows is state-corrupt
    let m = read::decode(&bytes).expect("decode");
    let mut p = StatePatch::default();
    new_locals(&mut p, &m, 1, &[b"x"]);
    p.symbols.append[0].file = 2;
    let view = read::open_bytes(bytes.clone()).expect("open");
    assert!(matches!(reason(state5::plan_patch(&view, &p)), IncrReason::StateCorrupt(_)));
    // a patch that also sets a row of the retired run: a defect
    let mut p = StatePatch::default();
    new_locals(&mut p, &m, 1, &[b"x"]);
    let old = m.file_index[1].locals.first;
    p.symbols.set.push((SymbolId(old), m.symbols[old as usize]));
    assert!(matches!(reason(state5::plan_patch(&view, &p)), IncrReason::StateCorrupt(_)));
    // O(change): one file's locals replaced in a 2,000-file state (SYMBOLS
    // 36 blocks) reads its FILE_INDEX row, its run's block and the append
    // areas', never SYMBOLS whole
    let files: Vec<u32> = (0..2000).collect();
    let bytes = full(&sample(&files, 1));
    let m = read::decode(&bytes).expect("decode");
    let view = read::open_bytes(bytes).expect("open");
    let base = read::bytes_read(&view).expect("counted");
    let mut p = StatePatch::default();
    new_locals(&mut p, &m, 7, &[b"seven"]);
    let plan = state5::plan_patch(&view, &p).expect("plan");
    assert!(!plan.compacts());
    let read = read::bytes_read(&view).expect("counted") - base;
    let symbols = 4001 * 36;
    eprintln!("SYMBOLS {symbols} bytes, plan read {read}, blocks written {}", plan.blocks());
    assert!(read <= 16 * state5::BLOCK as u64, "{read}");
    assert!(read < symbols, "{read}");
    assert!(plan.blocks() <= 10, "{}", plan.blocks());
}

// spec: incr.md IN-18e (WP-214 seam 6, ruling 2): a key only an option
// names (`--defsym`, `--wrap` of names no file mentions) carries
// `SYMBOLS.option` and survives the compaction and the canon; one without
// the bit and no mention is dead and dropped. (`-u` of a name no file
// mentions has no row in either capture: the capture package's test.)
#[test]
#[allow(clippy::many_single_char_names)]
fn option_only_key_survives_compaction() {
    let with = |m: &mut MemoryState, names: &[(&[u8], bool)]| {
        for &(n, option) in names {
            let name = add(&mut m.strings, n);
            m.symbols.push(SymbolRow {
                kind: sym_kind::GLOBAL,
                file: u32::MAX,
                name,
                at_file: u32::MAX,
                option,
                ..SymbolRow::default()
            });
            m.resolved.push(ResolvedRow { section: u32::MAX, ..ResolvedRow::default() });
            m.symtab_keys.push(SymtabKeyRow { index: u32::MAX, strtab: 0 });
        }
    };
    let opts: [(&[u8], bool); 4] =
        [(b"alias", true), (b"w", true), (b"__wrap_w", true), (b"__real_w", true)];
    let mut m = sample(&[0, 1, 2], 2);
    with(&mut m, &opts);
    with(&mut m, &[(b"plain", false)]);
    let bytes = full(&m);
    let d = read::decode(&bytes).expect("decode");
    let bit: Vec<(Vec<u8>, bool)> = d.symbols.iter().map(|r| (s(&d, r.name), r.option)).collect();
    for (n, _) in opts {
        assert!(bit.contains(&(n.to_vec(), true)), "{}: written with its bit", n.escape_ascii());
    }
    assert!(bit.iter().all(|(n, _)| n != b"plain"), "no bit, no mention: dead");
    // the bit is the live-key rule's input, never the command line's
    let live = state5::live_keys(&d).expect("rule");
    assert!(d.symbols.iter().zip(&live).all(|(r, l)| !r.option || *l));
    // an update that compacts (file 1 goes) keeps them, unchanged
    let view = read::open_bytes(bytes.clone()).expect("open");
    let plan = state5::plan_patch(&view, &remove_file_1(&d)).expect("plan");
    assert!(plan.compacts(), "{}", plan.reason());
    let got = compact::plan_bytes(&plan, &output_of(&bytes)).expect("compacted");
    let mut want = sample(&[0, 2], 2);
    with(&mut want, &opts);
    let n = want.stat.len();
    want.stat[n - 1].size = 4242;
    let (a, b) = (state5::canon(&got).expect("c"), state5::canon(&full(&want)).expect("c"));
    let rows = MemoryLayout::default();
    let diffs = state5::compare(&a, &b, &StoredLayout::new(&rows));
    assert!(diffs.is_empty(), "{diffs:#?}");
    let c = read::decode(&got).expect("decode");
    for (n, _) in opts {
        assert!(c.symbols.iter().any(|r| r.option && s(&c, r.name) == n), "{}", n.escape_ascii());
    }
}

/// A warning's `DIAGS` row with `refs` and `ref_count` (WP-221 seam 7).
fn diag_with_refs(
    refs: Vec<(u32, u64, u64)>,
    ref_count: u64,
) -> mortar_incr::update::diags::DiagRow {
    use mortar_incr::update::diags::{DiagRef, DiagRow, Subject};
    DiagRow {
        subjects: vec![Subject::File(FileId(1)), Subject::Key(SymbolId(0))],
        code: "gnu-warning".to_owned(),
        text: "a stored form".to_owned(),
        key: mortar_diag::Key { phase: 70, unit: 1, seq: 3 << 32 },
        refs: refs
            .into_iter()
            .map(|(f, seq, offset)| DiagRef { file: FileId(f), seq, offset })
            .collect(),
        ref_count,
        tomb: false,
    }
}

// spec: incr.md IN-18e (DIAGS, kind 49; WP-221 seam 7: a reference-site
// warning's references and count round-trip; references not sorted, or
// fewer counted than stored, are state-corrupt)
#[test]
fn diags_references_round_trip() {
    let mut m = sample(&[0, 1, 2], 3);
    m.diags = vec![
        diag_with_refs(vec![(0, 1 << 32, 0x10), (1, 3 << 32, 0x1c), (1, (3 << 32) + 4, 0x30)], 5),
        diag_with_refs(Vec::new(), 0),
    ];
    m.diags[1].key.seq = 7;
    let bytes = full(&m);
    let back = read::decode(&bytes).expect("decode");
    let mut want = compact::normalize(&m).expect("normalize").diags;
    want.sort_by_key(|d| d.key);
    assert_eq!(want[1].refs.len(), 3, "the references kept");
    let mut got = back.diags.clone();
    got.sort_by_key(|d| d.key);
    assert_eq!(got, want);
    let view = read::open_bytes(bytes).expect("open");
    let mut got = view.diags().expect("diags");
    got.sort_by_key(|d| d.key);
    assert_eq!(got, want);
    for bad in [
        diag_with_refs(vec![(1, 3 << 32, 0x1c), (0, 1 << 32, 0x10)], 2),
        diag_with_refs(vec![(0, 1 << 32, 0x10), (0, 1 << 32, 0x10)], 2),
        diag_with_refs(vec![(0, 1 << 32, 0x10), (1, 3 << 32, 0x1c)], 1),
    ] {
        let mut m = compact::normalize(&sample(&[0, 1, 2], 3)).expect("normalize");
        m.diags = vec![bad.clone()];
        let bytes = write::encode(&m).expect("encode");
        let err = read::decode(&bytes).expect_err("decoded");
        assert!(format!("{err:?}").contains("DIAGS"), "{bad:?}: {err:?}");
    }
}

// ---- WP-238 seam 9: `EDGES` of every kind, `META.live_hash` ----------------------------

/// The row of the symbol named `name` in `m` (before or after normalize).
fn key_named(m: &MemoryState, name: &str) -> u32 {
    let at = m.symbols.iter().position(|r| s(m, r.name) == name.as_bytes());
    u32::try_from(at.expect("the key")).expect("small")
}

/// One row of every `edge_kind` (0 to `LAST`) for the file at position
/// `f` of `m` (made by `sample` with at least 3 sections; its id `id`):
/// key targets of its own global and local, of another file's global
/// (`g0`) and of the option-only key; section targets within the file,
/// split ones at an offset and at `u32::MAX`; both `REFUSE` codes.
#[allow(clippy::many_single_char_names)] // the rows of one file, named as in WP-238 S1
fn add_edges(m: &mut MemoryState, f: u32, id: u32) {
    let first = m.sections.iter().position(|r| r.file == f).expect("its sections");
    let s = |k: usize| u32::try_from(first + k).expect("small");
    let (g0, defsym) = (key_named(m, "g0"), key_named(m, "defsym"));
    let (g, l) = (key_named(m, &format!("g{id}")), key_named(m, &format!("l{id}")));
    let row = |kind, source, target, offset| EdgeRow { kind, source, target, offset };
    let rows = [
        row(edge_kind::RELOC, s(0), g0, 0),
        row(edge_kind::START_STOP, s(1), defsym, 0),
        row(edge_kind::LOCAL, s(0), s(2), 7),
        row(edge_kind::FDE_SECTION, s(1), s(2), u32::MAX),
        row(edge_kind::FDE_KEY, s(1), g, 0),
        row(edge_kind::NONALLOC_KEY, s(2), l, 0),
        row(edge_kind::NONALLOC_SECTION, s(2), s(0), 0),
        row(edge_kind::LINK_ORDER, s(0), s(1), 0),
        row(edge_kind::GROUP, s(0), s(1), 0),
        row(edge_kind::GROUP, s(1), s(0), 0),
        row(edge_kind::ROOT, s(0), 0, 0),
        row(edge_kind::NAMED, s(1), 0, 0),
        row(edge_kind::SPLIT, s(2), 0, 0),
        row(edge_kind::REFUSE, s(0), edge_kind::refuse::FDE_GLOBAL, 0),
        row(edge_kind::REFUSE, s(2), edge_kind::refuse::DEF_DROPPED, 0),
    ];
    let kinds: std::collections::BTreeSet<u8> = rows.iter().map(|r| r.kind).collect();
    assert_eq!(kinds.len(), usize::from(edge_kind::LAST) + 1, "every kind");
    m.edges.extend(rows.into_iter().map(|r| (FileId(f), r)));
}

/// `m`'s `EDGES` rows by name, independent of ids: (file path, kind,
/// source section name, target, offset); a `Key` target by its key's
/// name, a `Section` target by the section's name, `Zero` and `Code` as
/// the number.
/// (file path, kind, source name, target, offset).
type NamedEdge = (Vec<u8>, u8, Vec<u8>, Vec<u8>, u32);

fn edges_by_name(m: &MemoryState) -> Vec<NamedEdge> {
    use edge_kind::Target;
    let sec = |r: u32| s(m, m.sections[r as usize].name);
    m.edges
        .iter()
        .map(|(f, r)| {
            let target = match edge_kind::target(r.kind).expect("a kind") {
                Target::Key => s(m, m.symbols[r.target as usize].name),
                Target::Section => sec(r.target),
                Target::Zero | Target::Code => r.target.to_le_bytes().to_vec(),
            };
            (s(m, m.files[f.0 as usize].path), r.kind, sec(r.source), target, r.offset)
        })
        .collect()
}

fn sorted<T: Ord>(mut v: Vec<T>) -> Vec<T> {
    v.sort();
    v
}

const LIVE_HASH: [u8; 16] = *b"live-set-hash-16";

// spec: WP-238 "Step 2" S1, S7 (seam 9: `EDGES` of 13 bytes, a row of
// every kind 0 to 12 written and read back field for field, ids by the
// writer's renumbering), S4 (`META.live_hash` read back); IN-18c (a
// full link's state is a fixpoint of decode and encode)
#[test]
fn edges_round_trip_every_kind() {
    let mut m = sample(&[0, 1, 2], 3);
    add_edges(&mut m, 0, 0);
    add_edges(&mut m, 2, 2);
    m.meta.live_hash = LIVE_HASH;
    let bytes = full(&m);
    let back = read::decode(&bytes).expect("decode");
    assert_eq!(back.edges.len(), 2 * 15);
    assert_eq!(sorted(edges_by_name(&back)), sorted(edges_by_name(&m)));
    assert_eq!(back.meta.live_hash, LIVE_HASH);
    assert_eq!(full(&back), bytes, "a fixpoint");
    // The rows a file's range gives (the update's `edges_of`) are that
    // file's, `Zero` and `Code` targets and the offsets as written.
    let view = read::open_bytes(bytes).expect("open");
    let f2 = back.files.iter().position(|r| s(&back, r.path) == b"f2.o").expect("f2");
    let rows = view.edges_of(FileId(u32::try_from(f2).expect("small"))).expect("rows");
    let want: Vec<EdgeRow> =
        back.edges.iter().filter(|(f, _)| f.0 as usize == f2).map(|r| r.1).collect();
    assert_eq!(rows, want);
    let offsets: Vec<u32> = rows.iter().map(|r| r.offset).collect();
    assert!(offsets.contains(&7) && offsets.contains(&u32::MAX));
    let codes: Vec<u32> =
        rows.iter().filter(|r| r.kind == edge_kind::REFUSE).map(|r| r.target).collect();
    assert_eq!(codes, [edge_kind::refuse::FDE_GLOBAL, edge_kind::refuse::DEF_DROPPED]);
    assert!(rows.iter().filter(|r| r.kind == edge_kind::ROOT).all(|r| r.target == 0));
}

// spec: WP-238 "Step 2" S7 (compaction renumbers `Key` targets through
// the keys and `Section` targets through the sections, and copies `Zero`
// and `Code` targets; F4) and S4 (compaction copies `META.live_hash`);
// IN-18f (the compaction's bytes equal the full link's of the same
// inputs, but META's history fields)
#[test]
fn edges_compaction_every_kind() {
    let mut base = sample(&[0, 1, 2], 3);
    for (f, id) in [(0, 0), (1, 1), (2, 2)] {
        add_edges(&mut base, f, id);
    }
    base.meta.live_hash = LIVE_HASH;
    let bytes = full(&base);
    // hop 1: in place (as `compaction_equals_full_link_capture`)
    let view = read::open_bytes(bytes.clone()).expect("open");
    let m = read::decode(&bytes).expect("decode");
    let mut p1 = StatePatch::default();
    p1.stat_set.push((0, StatRow { size: 99, ..m.stat[0].clone() }));
    let plan = state5::plan_patch(&view, &p1).expect("plan");
    assert!(!plan.compacts());
    let bytes = write::patched_bytes(&bytes, &plan, &output_of(&bytes)).expect("in place");
    // hop 2: file 1 (before file 2's rows) goes: the compaction
    let view = read::open_bytes(bytes.clone()).expect("open");
    let old = read::decode(&bytes).expect("decode");
    let plan = state5::plan_patch(&view, &remove_file_1(&old)).expect("plan");
    assert!(plan.compacts(), "{}", plan.reason());
    let got = compact::plan_bytes(&plan, &output_of(&bytes)).expect("compacted");
    // the full link of files 0 and 2, with the same rows
    let mut want = sample(&[0, 2], 3);
    add_edges(&mut want, 0, 0);
    add_edges(&mut want, 1, 2);
    want.meta.live_hash = LIVE_HASH;
    want.stat[0].size = 99;
    let n = want.stat.len();
    want.stat[n - 1].size = 4242;
    let mut want = compact::normalize(&want).expect("normalize");
    want.meta.output_synced = true;
    want.meta.compactions = 1;
    let want = write::encode(&want).expect("encode");
    assert_eq!(state5::canon(&got).expect("c"), state5::canon(&want).expect("c"));
    assert!(got == want, "compacted bytes differ from the full link's");
    // and explicitly, per class, against the state before
    let new = read::decode(&got).expect("decode");
    assert_eq!(new.meta.live_hash, LIVE_HASH);
    let by_name = |m: &MemoryState, path: &[u8]| {
        sorted(edges_by_name(m).into_iter().filter(|e| e.0 == path).collect::<Vec<_>>())
    };
    assert!(by_name(&new, b"f1.o").is_empty(), "the gone file's rows");
    for path in [b"f0.o".as_slice(), b"f2.o"] {
        assert_eq!(by_name(&new, path), by_name(&old, path), "{}", String::from_utf8_lossy(path));
    }
    let raw = |m: &MemoryState, path: &[u8]| -> Vec<EdgeRow> {
        let f = m.files.iter().position(|r| s(m, r.path) == path).expect("file");
        m.edges.iter().filter(|(g, _)| g.0 as usize == f).map(|r| r.1).collect()
    };
    let (before, after) = (raw(&old, b"f2.o"), raw(&new, b"f2.o"));
    assert_eq!(before.len(), after.len());
    // f2's first `SECTIONS` row before and after (ranges with slack)
    let first = |m: &MemoryState| {
        let f = m.files.iter().position(|r| s(m, r.path) == b"f2.o").expect("file");
        let at = m.sections.iter().position(|r| r.file as usize == f).expect("its sections");
        u32::try_from(at).expect("small")
    };
    let (was, now) = (first(&old), first(&new));
    assert!(now < was, "file 1's sections were before f2's: {now} {was}");
    let mut moved = [0usize; 2];
    for (b, a) in before.iter().zip(&after) {
        assert_eq!((b.kind, b.offset), (a.kind, a.offset));
        assert_eq!(a.source - now, b.source - was);
        match edge_kind::target(b.kind).expect("a kind") {
            edge_kind::Target::Section => {
                assert_eq!(a.target - now, b.target - was);
                moved[0] += 1;
            }
            edge_kind::Target::Key => {
                assert_eq!(s(&new, new.symbols[a.target as usize].name), {
                    s(&old, old.symbols[b.target as usize].name)
                });
                moved[1] += usize::from(a.target != b.target);
            }
            edge_kind::Target::Zero | edge_kind::Target::Code => assert_eq!(a.target, b.target),
        }
    }
    assert_eq!(moved[0], 6, "every Section-target row renumbered");
    assert!(moved[1] > 0, "some Key target renumbered (the gone keys)");
}

// spec: WP-238 "Step 2" S4 (`META.live_hash`: zero by default, written
// and read back; an in-place update's META carries the new value)
#[test]
fn meta_live_hash_round_trip() {
    let m = sample(&[0, 1], 3);
    assert_eq!(m.meta.live_hash, [0; 16], "zero without --gc-sections");
    let bytes = full(&m);
    assert_eq!(read::decode(&bytes).expect("decode").meta.live_hash, [0; 16]);
    // an update writes its new walk's hash through META
    let view = read::open_bytes(bytes.clone()).expect("open");
    let mut p = StatePatch::default();
    let mut meta = read::decode(&bytes).expect("decode").meta;
    meta.live_hash = LIVE_HASH;
    p.meta = Some(meta);
    let plan = state5::plan_patch(&view, &p).expect("plan");
    assert!(!plan.compacts());
    let patched = write::patched_bytes(&bytes, &plan, &output_of(&bytes)).expect("in place");
    assert_eq!(read::decode(&patched).expect("decode").meta.live_hash, LIVE_HASH);
}

// spec: WP-238 "Step 2" S1, P0-1 (the reader refuses a row its class
// forbids: `state-corrupt`, never a panic), end to end through a state
#[test]
fn edges_bad_row_is_state_corrupt() {
    let bad = [
        EdgeRow { kind: edge_kind::ROOT, source: 0, target: 1, offset: 0 },
        EdgeRow { kind: edge_kind::NAMED, source: 0, target: 0, offset: 4 },
        EdgeRow { kind: edge_kind::REFUSE, source: 0, target: 0, offset: 0 },
        EdgeRow { kind: edge_kind::REFUSE, source: 0, target: 3, offset: 0 },
        EdgeRow { kind: edge_kind::RELOC, source: 0, target: 1, offset: 8 },
    ];
    for r in bad {
        let mut m = sample(&[0, 1], 3);
        m.edges.push((FileId(0), r));
        let bytes = full(&m);
        assert!(matches!(read::decode(&bytes), Err(StateError::Corrupt(_))), "{r:?}");
    }
    // a kind with no class is refused by normalize and by the writer
    let row = EdgeRow { kind: edge_kind::LAST + 1, ..EdgeRow::default() };
    let mut m = sample(&[0, 1], 3);
    m.edges.push((FileId(0), row));
    assert!(matches!(compact::normalize(&m), Err(StateError::Corrupt(_))));
    let mut n = compact::normalize(&sample(&[0, 1], 3)).expect("normalize");
    n.edges.push((FileId(0), row));
    assert!(matches!(write::encode(&n), Err(StateError::Corrupt(_))));
}

// spec: WP-238 "Step 2" S7, P0-4 (format 5 stays; seam 6's rule: the
// directory records each table's row width, and a reader whose width
// differs refuses the state `state-corrupt`, so an older state, META of
// 208 bytes or EDGES of 17, is refused and replaced, never read)
#[test]
fn older_row_widths_are_refused() {
    let bytes = full(&sample(&[0, 1], 3));
    let dir_end = state5::HEADER + state5::TABLES.len() * state5::ENTRY;
    let h = state5::Header::parse(&bytes).expect("h");
    let dir = state5::parse_directory(&h, &bytes[state5::HEADER..dir_end]).expect("dir");
    for (k, old) in [(kind::META, 208), (kind::EDGES, 17)] {
        let mut d = dir.clone();
        d[state5::table_index(k).expect("t")].width = old;
        let db = state5::directory_bytes(&d);
        let h2 = state5::Header { dir_hash: mortar_digest::l128(&db), ..h };
        let mut b = bytes.clone();
        b[..state5::HEADER].copy_from_slice(&h2.bytes());
        b[state5::HEADER..dir_end].copy_from_slice(&db);
        match read::decode(&b) {
            Err(StateError::Corrupt(why)) => {
                assert!(why.contains(&format!("of width {old}")), "{why}");
            }
            other => panic!("{k}: {other:?}"),
        }
    }
}

// spec: incr.md IN-18c with seam 9's widths (every flipped byte of a
// state with `EDGES` rows of every kind and a `META.live_hash` is
// `state-corrupt`)
#[test]
fn every_flipped_byte_with_edges_is_state_corrupt() {
    let mut m = sample(&[0, 1], 3);
    add_edges(&mut m, 1, 1);
    m.meta.live_hash = LIVE_HASH;
    let bytes = full(&m);
    assert!(read::decode(&bytes).is_ok(), "clean");
    let dir_end = state5::HEADER + state5::TABLES.len() * state5::ENTRY;
    let dir = state5::parse_directory(
        &state5::Header::parse(&bytes).expect("h"),
        &bytes[state5::HEADER..dir_end],
    )
    .expect("dir");
    let edges = &dir[state5::table_index(kind::EDGES).expect("t")];
    let meta = &dir[state5::table_index(kind::META).expect("t")];
    assert_eq!(edges.width, 13);
    assert_eq!(meta.width, 224);
    let mut flipped = 0;
    let mut check = |at: usize| {
        let mut b = bytes.clone();
        b[at] ^= 0x41;
        assert!(read::decode(&b).is_err(), "byte {at} flipped: decoded");
        flipped += 1;
    };
    // META's `live_hash` (its last 16 bytes), every byte
    let meta_at = usize::try_from(meta.body).expect("small");
    for at in meta_at + 208..meta_at + 224 {
        check(at);
    }
    // EDGES' body (rows and slack), every fifth byte and its last (a
    // decode per byte is slow in a debug build)
    let start = usize::try_from(edges.body).expect("small");
    let len = usize::try_from(edges.body_len()).expect("small");
    for at in (start..start + len).step_by(5).chain([start + len - 1]) {
        check(at);
    }
    assert!(flipped > 16 + 15 * 13 / 5, "{flipped}");
}

// ---- DEBUG_STRS (WP-247 item 1; WP-228 R-2, seam 8) -------------------------

fn dstr(out: u8, hash: u64, offset: u32) -> DebugStrRow {
    DebugStrRow { out, hash, offset }
}

// spec: WP-228 R-2.1 (kind 57, KeyKeyed, 13 bytes: `u8 out`, `u64 hash`,
// `u32 offset`, sorted by `(out, hash)`, no tomb), R-2.2 (equal keys keep
// the lower offset), the reader's one binary search
#[test]
fn debug_strs_round_trip_and_lookup() {
    let t = state5::TABLES.iter().find(|t| t.0 == kind::DEBUG_STRS).expect("listed");
    assert_eq!((t.1, t.2), (state5::Class::KeyKeyed, 13));
    let mut m = sample(&[0, 1], 3);
    let str_ = debug_str_class::STR;
    let line = debug_str_class::LINE_STR;
    m.debug_strs = vec![
        dstr(line, 5, 40),
        dstr(str_, 9, 30),
        dstr(str_, 2, 0),
        dstr(str_, 9, 12),
        dstr(line, 2, 7),
    ];
    let bytes = full(&m);
    let back = read::decode(&bytes).expect("decode");
    assert_eq!(
        back.debug_strs,
        vec![dstr(str_, 2, 0), dstr(str_, 9, 12), dstr(line, 2, 7), dstr(line, 5, 40)],
        "sorted by (out, hash), the lower offset kept"
    );
    assert_eq!(full(&back), bytes, "a fixpoint");
    let view = read::open_bytes(bytes).expect("open");
    assert_eq!(view.debug_str(str_, 9).expect("read"), Some(12));
    assert_eq!(view.debug_str(str_, 2).expect("read"), Some(0));
    assert_eq!(view.debug_str(line, 2).expect("read"), Some(7));
    assert_eq!(view.debug_str(line, 9).expect("read"), None, "the class is part of the key");
    assert_eq!(view.debug_str(str_, 3).expect("read"), None);
    assert_eq!(view.debug_str(7, 2).expect("read"), None, "an unknown class asked: a miss");
    // the memory view answers alike (the tests' StateView)
    let mv = mortar_incr::update::view::StateView::new(Box::new(&back));
    assert_eq!(mv.debug_str(str_, 9).expect("read"), Some(12));
    // an empty table: every string a miss
    let view = read::open_bytes(full(&sample(&[0, 1], 3))).expect("open");
    assert_eq!(view.debug_str(str_, 2).expect("read"), None);
}

// spec: WP-228 R-2.1 (`out` other than 0 or 1 is `state-corrupt`; the
// reader never trusts the rest: a duplicate key, an offset past the
// section or into a string's middle are not errors, the caller checks a
// hit against the output's bytes)
#[test]
fn debug_strs_what_the_reader_refuses() {
    for out in [2u8, 0xff] {
        let mut n = compact::normalize(&sample(&[0, 1], 3)).expect("normalize");
        n.debug_strs = vec![dstr(0, 1, 0), dstr(out, 1, 0)];
        let bytes = write::encode(&n).expect("the writer does not judge `out`");
        match read::decode(&bytes) {
            Err(StateError::Corrupt(why)) => assert!(why.contains("DEBUG_STRS"), "{why}"),
            other => panic!("out {out}: {other:?}"),
        }
        let view = read::open_bytes(bytes).expect("open: rows are read on demand");
        assert!(matches!(reason(view.debug_str(out, 1)), IncrReason::StateCorrupt(_)), "out {out}");
    }
    // a duplicate key (written past normalize): not an error, the lower
    // offset answers
    let mut n = compact::normalize(&sample(&[0, 1], 3)).expect("normalize");
    n.debug_strs = vec![dstr(0, 4, 90), dstr(0, 4, 3), dstr(0, 4, 50)];
    let bytes = write::encode(&n).expect("encode");
    assert_eq!(read::decode(&bytes).expect("duplicates decode").debug_strs.len(), 3);
    let view = read::open_bytes(bytes).expect("open");
    assert_eq!(view.debug_str(0, 4).expect("read"), Some(3));
    // an offset past any section, an offset into a string's middle: not
    // judged here (the sample has no output section at all)
    let mut m = sample(&[0, 1], 3);
    m.debug_strs = vec![dstr(0, 6, u32::MAX), dstr(1, 6, 3)];
    let view = read::open_bytes(full(&m)).expect("open");
    assert_eq!(view.debug_str(0, 6).expect("read"), Some(u32::MAX));
    assert_eq!(view.debug_str(1, 6).expect("read"), Some(3));
}

// spec: WP-228 R-2.3 (canon and compaction sort by key; compaction
// rewrites the table whole)
#[test]
fn debug_strs_canon_and_compaction_sort() {
    let mut a = sample(&[0, 1], 3);
    a.debug_strs = vec![dstr(1, 3, 1), dstr(0, 8, 2), dstr(0, 1, 3)];
    let mut b = a.clone();
    b.debug_strs.reverse();
    assert_eq!(state5::canon_of(&a).expect("canon"), state5::canon_of(&b).expect("canon"));
    assert_eq!(full(&a), full(&b));
    assert_eq!(
        compact::debug_strs_whole(&[dstr(1, 3, 9), dstr(0, 8, 2), dstr(1, 3, 1)]),
        vec![dstr(0, 8, 2), dstr(1, 3, 1)]
    );
    // a compaction keeps the rows (no update edits them here)
    let mut c = sample(&[0, 1, 2], 2);
    c.debug_strs = b.debug_strs.clone();
    let bytes = full(&c);
    let view = read::open_bytes(bytes.clone()).expect("open");
    let plan = state5::plan_patch(&view, &remove_file_1(&read::decode(&bytes).expect("decode")))
        .expect("plan");
    assert!(plan.compacts(), "{}", plan.reason());
    let got = compact::plan_bytes(&plan, &output_of(&bytes)).expect("compacted");
    assert_eq!(
        read::decode(&got).expect("decode").debug_strs,
        vec![dstr(0, 1, 3), dstr(0, 8, 2), dstr(1, 3, 1)]
    );
}

// spec: WP-228 seam 8a (an update only PUTS `DEBUG_STRS` rows: in place
// they go to the sorted append area, found by the reader's search of the
// body and the area; a put of a stored key replaces its row; a delete is
// refused, in place and by the compaction) and R-2.3 (the compaction
// rewrites the table whole, sorted)
#[test]
fn debug_strs_patch_puts_rows() {
    let mut m = sample(&[0, 1, 2], 2);
    m.debug_strs = vec![dstr(0, 10, 0), dstr(0, 30, 8), dstr(1, 10, 0)];
    let bytes = full(&m);
    let view = read::open_bytes(bytes.clone()).expect("open");
    let mut p = StatePatch::default();
    p.debug_strs.put = vec![dstr(0, 20, 40), dstr(0, 30, 50), dstr(1, 5, 9)];
    let plan = state5::plan_patch(&view, &p).expect("plan");
    assert!(!plan.compacts(), "{}", plan.reason());
    let patched = write::patched_bytes(&bytes, &plan, &output_of(&bytes)).expect("in place");
    let view = read::open_bytes(patched.clone()).expect("open");
    for (out, hash, want) in [
        (0, 10, Some(0)),
        (0, 20, Some(40)),
        (0, 30, Some(50)),
        (1, 5, Some(9)),
        (1, 10, Some(0)),
        (1, 20, None),
    ] {
        assert_eq!(view.debug_str(out, hash).expect("read"), want, "({out}, {hash})");
    }
    let back = read::decode(&patched).expect("decode");
    assert_eq!(
        compact::normalize(&back).expect("normalize").debug_strs,
        vec![dstr(0, 10, 0), dstr(0, 20, 40), dstr(0, 30, 50), dstr(1, 5, 9), dstr(1, 10, 0)]
    );
    // a delete is refused, in place and in memory
    let mut d = StatePatch::default();
    d.debug_strs.delete = vec![dstr(0, 10, 0)];
    let view = read::open_bytes(patched.clone()).expect("open");
    assert!(matches!(reason(state5::plan_patch(&view, &d)), IncrReason::StateCorrupt(_)));
    let mut mm = back.clone();
    assert!(compact::apply(&mut mm, &d, 0).is_err());
    // with a compaction: the rows put, sorted and whole
    let mut c = remove_file_1(&back);
    c.debug_strs.put = vec![dstr(0, 1, 77)];
    let plan = state5::plan_patch(&view, &c).expect("plan");
    assert!(plan.compacts(), "{}", plan.reason());
    let got = compact::plan_bytes(&plan, &output_of(&patched)).expect("compacted");
    let got = read::decode(&got).expect("decode");
    assert_eq!(
        got.debug_strs,
        vec![
            dstr(0, 1, 77),
            dstr(0, 10, 0),
            dstr(0, 20, 40),
            dstr(0, 30, 50),
            dstr(1, 5, 9),
            dstr(1, 10, 0)
        ]
    );
    c.debug_strs.delete = vec![dstr(0, 10, 0)];
    assert!(matches!(reason(state5::plan_patch(&view, &c)), IncrReason::StateCorrupt(_)));
}

// spec: WP-247 item d (`EH_FIXED` read by the plan's check before W1: a
// file whose range moves with the bit is `update-not-implemented (eh)`)
// and item k (`DEBUG_EXPORTS` and `EH_FIXED` are FILES flags of format
// 5: stored and read back)
#[test]
fn eh_fixed_check_reads_the_bit() {
    let mut m = sample(&[0, 1, 2], 2);
    m.files[1].flags |= file_flags::EH_FIXED;
    m.files[2].flags |= file_flags::DEBUG_EXPORTS;
    let bytes = full(&m);
    let back = read::decode(&bytes).expect("decode");
    assert_eq!(back.files[1].flags, file_flags::EH_FIXED);
    assert_eq!(back.files[2].flags, file_flags::DEBUG_EXPORTS);
    let view = read::open_bytes(bytes).expect("open");
    assert!(read::eh_fixed_check(&view, &[FileId(0), FileId(2)]).is_ok());
    assert_eq!(
        read::eh_fixed_check(&view, &[FileId(0), FileId(1)]),
        Err(IncrReason::UpdateNotImplemented(Some(mortar_core::incr::Unimplemented::Eh)))
    );
    assert!(matches!(read::eh_fixed_check(&view, &[FileId(9)]), Err(IncrReason::StateCorrupt(_))));
    // a flag past ALL is refused
    let mut m = sample(&[0, 1], 2);
    m.files[0].flags = file_flags::ALL + 1;
    assert!(matches!(read::decode(&full(&m)), Err(StateError::Corrupt(_))));
}

// spec: WP-247 item 3 (U-10 notes items a, j; WP-244 F-2): decode,
// normalize, encode of a capture is BYTE-EQUAL, with `DIAGS` rows (their
// strings in the pool once), 24-byte property strings (one shared by two
// files), `EDGES` of every kind, `META.live_hash` and `DEBUG_STRS` rows
#[test]
fn decode_normalize_encode_is_byte_equal() {
    let mut m = sample(&[0, 1, 2], 3);
    m.diags = vec![
        diag_with_refs(vec![(0, 1 << 32, 0x10), (1, 3 << 32, 0x1c)], 2),
        diag_with_refs(Vec::new(), 0),
    ];
    m.diags[1].key.seq = 7;
    let mut pool = std::mem::take(&mut m.strings);
    let p = add(&mut pool, &[1; 24]);
    let q = add(&mut pool, &[2; 24]);
    m.strings = pool;
    m.files[0].properties = p;
    m.files[1].properties = p;
    m.files[2].properties = q;
    add_edges(&mut m, 1, 1);
    m.meta.live_hash = LIVE_HASH;
    m.debug_strs = vec![dstr(1, 4, 0), dstr(0, 9, 3)];
    let bytes = full(&m);
    let back = read::decode(&bytes).expect("decode");
    assert_eq!(s(&back, back.files[1].properties), vec![1; 24]);
    assert_eq!(back.diags.len(), 2);
    let again = write::encode(&compact::normalize(&back).expect("normalize")).expect("encode");
    assert_eq!(again.len(), bytes.len());
    assert!(
        again == bytes,
        "decode, normalize, encode is not byte-equal: {}",
        where_differ(&bytes, &again)
    );
    // and once more (a fixpoint, not a coincidence of one pass)
    let third = full(&read::decode(&again).expect("decode"));
    assert!(third == bytes);
}

/// Where two states of one shape first differ: the table and the offset.
fn where_differ(a: &[u8], b: &[u8]) -> String {
    let skip = state5::HEADER;
    let Some(at) = a.iter().zip(b).skip(skip).position(|(x, y)| x != y).map(|i| i + skip) else {
        return "equal past the header".into();
    };
    let h = state5::Header::parse(a).expect("h");
    let dir_end = state5::HEADER + state5::TABLES.len() * state5::ENTRY;
    let dir = state5::parse_directory(&h, &a[state5::HEADER..dir_end]).expect("dir");
    for e in &dir {
        let (body, len) = (
            usize::try_from(e.body).expect("small"),
            usize::try_from(e.body_len()).expect("small"),
        );
        if (body..body + len).contains(&at) {
            let w = e.width as usize;
            return format!(
                "byte {at}: {} row {} byte {}",
                state5::name(e.kind),
                (at - body) / w,
                (at - body) % w
            );
        }
        let list = usize::try_from(e.list).expect("small");
        if (list..list + usize::try_from(e.list_len()).expect("small")).contains(&at) {
            return format!("byte {at}: {} block list", state5::name(e.kind));
        }
    }
    let entry = (at - state5::HEADER) / state5::ENTRY;
    format!(
        "byte {at}: directory entry {} ({}) byte {}",
        entry,
        state5::name(dir[entry].kind),
        (at - state5::HEADER) % state5::ENTRY
    )
}
