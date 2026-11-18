//! U-2's tests (`spec/incr.md` IN-18c..IN-18f, IN-46b, IN-78; WP-214).
//! The states are made by hand (`sample`): a few files with sections,
//! a global and a local each, `MENTIONS`, `KEY_REFS`, `FRAGS` and
//! `FRAG_AT`; the files are named by their index in the full set, so the
//! capture of a subset is the full link of those inputs.

use mortar_core::incr::{IncrReason, MemoryLayout, StoredLayout};
use mortar_core::{FileId, SymbolId};
use mortar_incr::format::{FragAtRow, SectionRow, StatRow, StateError, Str, kind, stat_tag};
use mortar_incr::ops::{StdOps, WriteFailure};
use mortar_incr::state5::{self, compact, read, write};
use mortar_incr::update::rows::{
    CommonRow, CopyRow, FileIndexRow, FileRow, FragRow, GroupRow, KeyRefRow, MentionRow, NameRow,
    ResolvedRow, SymbolRow, SymtabKeyRow, file_flags, sym_kind,
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
        m.files.push(FileRow { path, rank: u64::from(id) << 20, ..FileRow::default() });
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
        m.key_refs.push(KeyRefRow { key: g, file: f, ..KeyRefRow::default() });
        let frag = m.frags.len() as u64;
        m.frags.push(FragRow {
            out: 0,
            ordinal: id,
            hash: hash_of(id),
            offset: u64::from(id) * 8,
            ..FragRow::default()
        });
        m.frag_at.push((FileId(f), FragAtRow { section: first, offset: 0, frag }));
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
    m.resolved = vec![ResolvedRow { section: u32::MAX, ..ResolvedRow::default() }; m.symbols.len()];
    m.symtab_keys = vec![SymtabKeyRow { index: u32::MAX, strtab: 0 }; m.symbols.len()];
    m.stat.push(StatRow { tag: stat_tag::OUTPUT, path: out, ..StatRow::default() });
    m.strings = pool;
    m
}

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
    let frag_at = base
        .frag_at
        .iter()
        .filter(|r| r.0 == FileId(0))
        .map(|r| FragAtRow { section: first, ..r.1 })
        .collect();
    p.ranges.push(FileRanges {
        file: FileId(0),
        sections: Some(rows),
        mentions: Some(mentions),
        index: None,
        edges: None,
        frag_at: Some(frag_at),
        eh_records: None,
        symtab: None,
        regions: None,
        gone: false,
    });
    p.key_refs.put.push(KeyRefRow { key: 1, file: 0, ..KeyRefRow::default() });
    p.frags.put.push(FragRow {
        out: 9,
        ordinal: 0,
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
        sections: None,
        mentions: None,
        index: None,
        edges: None,
        frag_at: None,
        eh_records: None,
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
    p.frags.delete.push(*m.frags.iter().find(|f| f.ordinal == 1).expect("its frag"));
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
