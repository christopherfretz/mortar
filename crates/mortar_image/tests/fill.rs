//! Fill (§5, §6.1, §8.3): purity against the sequential reference, clips,
//! record orders, gaps, synthetic chunks, errors, the build id.
#![allow(
    clippy::many_single_char_names,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements
)]

mod support;

use mortar_arch::{OutputKind, Proof, ProofRun, Reloc};
use mortar_core::par::{self, Pool};
use mortar_core::{ChunkId, FileId, SectionId};
use mortar_diag::FileRef;
use mortar_digest::TreeStyle;
use mortar_image::IMAGE_BLOCK;
use mortar_image::fill::{self, Image};
use mortar_image::plan::{Builder, InputUnit, Pattern, RecRows, RelocSection, SynthUnit, Unit};
use mortar_image::synth::SynthChunk;
use support::Chunk;
use support::*;

fn chunks_of(w: &World) -> Vec<Chunk<'_>> {
    w.chunks.iter().map(|c| &**c as Chunk<'_>).collect()
}

// spec: image.md I-33, I-35, §16.1 "purity": identical bytes for W' of 4 KiB,
// 64 KiB and 1 MiB, equal to the sequential reference.
#[test]
fn i33_bytes_do_not_depend_on_the_window() {
    for seed in [1u64, 7, 99] {
        let mut outs = Vec::new();
        for w in [4096u64, 65536, IMAGE_BLOCK] {
            let world = random_world(seed, 3 * 1024 * 1024, 65536);
            // Same rows, other window: rebuild the plan with W'.
            let world = rewindow(world, w);
            let chunks = chunks_of(&world);
            let img = image(&world, &chunks);
            let got = fill_mem(&img, &Pool::new(4)).expect("fill");
            assert_eq!(got.len() as u64, world.plan.file_size);
            let want = reference(&img);
            assert!(
                got == want,
                "seed {seed}, W' {w}: differs from the reference at {:?}",
                first_diff(&got, &want)
            );
            outs.push(got);
        }
        assert!(outs.windows(2).all(|p| p[0] == p[1]));
    }
}

/// The same plan rows with another window.
fn rewindow(w: World, window: u64) -> World {
    let p = &w.plan;
    let mut b = Builder::new(p.target, p.file_size);
    for (f, r) in &p.paths {
        b.path(*f, r.clone());
    }
    for n in &p.names {
        b.name(n);
    }
    for r in &p.relocs {
        b.relocs(r.clone());
    }
    for row in &p.rows {
        match row.unit {
            Unit::Input(i) => b.input(row.offset, row.size, p.inputs[i as usize].clone()),
            Unit::Bytes(i) => b.bytes(row.offset, p.bytes[i as usize].clone()),
            Unit::Synth(i) => b.synth(row.offset, row.size, p.synth[i as usize].clone()),
            Unit::Merged(i) => b.merged(row.offset, row.size, p.merged[i as usize].clone()),
            Unit::Gap(pat) => b.pad(row.offset..row.end(), pat),
        }
    }
    let plan = b.finish(window, &Pool::sequential()).unwrap();
    World { plan, ..w }
}

fn first_diff(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .or((a.len() != b.len()).then_some(a.len().min(b.len())))
}

// spec: image.md I-32, I-33, rule 6, §16.2: every thread count and seed of
// `check_schedules` gives the same bytes.
#[test]
fn i32_bytes_do_not_depend_on_the_schedule() {
    let world = rewindow(random_world(3, 2 * 1024 * 1024, 4096), 4096);
    let chunks = chunks_of(&world);
    let img = image(&world, &chunks);
    let want = reference(&img);
    let got = par::check_schedules(|pool| fill_mem(&img, pool).expect("fill"));
    assert!(got == want, "differs at {:?}", first_diff(&got, &want));
}

/// One code section of `words` words at file offset `at` with `records`,
/// in a file of `size` bytes, window `w`.
fn one_section(words: &[u32], at: u64, records: Vec<Reloc>, size: u64, w: u64) -> World {
    let bytes: Vec<u8> = words.iter().flat_map(|x| x.to_le_bytes()).collect();
    let len = bytes.len() as u64;
    let mut b = Builder::new(a64(), size);
    let mut source = MemSource::default();
    source.files.insert(FileId(1), (bytes, false));
    b.path(FileId(1), FileRef::new("a.o"));
    let proof = vec![ProofRun { start: 0, end: len, proof: Proof::Insn, unit: 0, unit_offset: 0 }];
    let r = b.relocs(RelocSection { section: SectionId(1), rows: RecRows::Decoded(records.into()), proof: proof.into() });
    let name = b.name(b".text");
    let unit = InputUnit {
        file: FileId(1),
        index: 1,
        section: SectionId(1),
        src: 0,
        start: 0,
        section_size: len,
        addr: 0x40_0000 + at,
        alloc: true,
        name,
        relocs: Some(r),
    };
    b.input(at, len, unit);
    let mut tables = FakeTables::default();
    for n in 1..8 {
        tables.define(key(n), 0x40_8000 + 0x100 * u64::from(n), true);
    }
    World { plan: b.finish(w, &Pool::sequential()).unwrap(), source, tables, chunks: Vec::new(), records: Default::default() }.indexed()
}

// spec: image.md I-111, I-35 step 2: the GD→LE sequence (`spec/a64.md`
// §8.4) whose words straddle a window boundary at o+4, o+8 and o+12 is
// written as the unclipped reference writes it.
#[test]
fn i111_sequence_across_window_boundaries() {
    let w = 4096u64;
    for cut in [4u64, 8, 12] {
        // The sequence's first word at file offset w − cut.
        let lead = 64u64; // words before the sequence
        let at = w - cut - 4 * lead;
        let mut words = vec![0x1234_5678u32; lead as usize];
        words.extend([ADRP_X0, ADD_X0_X0, BL, NOP]);
        words.extend([0u32; 8]);
        let o = 4 * lead;
        let tls = key(2);
        let records = vec![
            Reloc { r_offset: o, key: tls, r_type: TLSGD_ADR_PAGE21, addend: 0 },
            Reloc { r_offset: o + 4, key: tls, r_type: TLSGD_ADD_LO12_NC, addend: 0 },
            Reloc { r_offset: o + 8, key: key(3), r_type: CALL26, addend: 0 },
        ];
        let mut world = one_section(&words, at, records, 2 * w, w);
        world.tables.names.insert(key(3), b"__tls_get_addr".to_vec());
        world.tables.define(tls, TLS.start + 0x10, true);
        let chunks = Vec::new();
        let mut img = image(&world, &chunks);
        img.arch = &ARCH_M2;
        let want = reference(&img);
        // The reference relaxed it: the `nop` at o+12 is no longer a nop.
        let nop_at = (at + o + 12) as usize;
        assert_ne!(&want[nop_at..nop_at + 4], &NOP.to_le_bytes(), "the sequence was relaxed");
        let got = fill_mem(&img, &Pool::new(2)).unwrap();
        assert!(got == want, "cut {cut}: differs at {:?}", first_diff(&got, &want));
    }
}

// spec: image.md I-35, I-36: relocations at a window boundary (fields
// straddling it) are applied by both jobs and reported once.
#[test]
fn i36_straddling_field_is_written_by_both_jobs_and_reported_once() {
    let w = 4096u64;
    // An ABS64 field straddling the boundary: offsets w−4..w+4.
    let words = vec![0u32; 32];
    let at = w - 64;
    let mut records = vec![Reloc { r_offset: 60, key: key(1), r_type: ABS64, addend: 5 }];
    // A CALL26 whose target is out of range, also at the boundary: its
    // error is reported once.
    let mut words2 = words.clone();
    words2[16] = BL;
    records.push(Reloc { r_offset: 64, key: key(9), r_type: CALL26, addend: 0 });
    let mut world = one_section(&words2, at, records, 2 * w, w);
    world.tables.define(key(9), 0x7000_0000_0000, true);
    let chunks = Vec::new();
    let img = image(&world, &chunks);
    let err = fill_mem(&img, &Pool::new(2)).expect_err("out of range");
    assert_eq!(err.diagnostics.len(), 1, "{:?}", err.diagnostics);
    assert_eq!(err.diagnostics[0].1.id(), "relocation-out-of-range");
    assert_eq!(err.diagnostics[0].0, mortar_arch::cx::fill_key(FileId(1), 1, 1));
    // Without the bad call, the straddling field equals the reference.
    let world = one_section(
        &words,
        at,
        vec![Reloc { r_offset: 60, key: key(1), r_type: ABS64, addend: 5 }],
        2 * w,
        w,
    );
    let img = image(&world, &chunks);
    let got = fill_mem(&img, &Pool::new(2)).unwrap();
    assert_eq!(got, reference(&img));
    let v = u64::from_le_bytes(got[(at + 60) as usize..(at + 68) as usize].try_into().unwrap());
    assert_eq!(v, 0x40_8000 + 0x100 + 5);
}

// spec: image.md I-29: a Permuted section (records out of order) fills as
// the reference; overlapping records apply in record order (the review).
#[test]
fn i29_permuted_and_overlapping_records() {
    let w = 4096u64;
    let words = vec![0u32; 2048];
    let mut records: Vec<Reloc> = (0..200u64)
        .rev()
        .map(|k| Reloc {
            r_offset: 8 * k + 4000,
            key: key(1 + (k % 5) as u32),
            r_type: ABS64,
            addend: k as i64,
        })
        .collect();
    // Two records at overlapping fields: the later in record order wins.
    records.push(Reloc { r_offset: 4004, key: key(6), r_type: ABS32, addend: 1 });
    let world = one_section(&words, 100, records, 3 * w, w);
    assert!(matches!(world.records.get(SectionId(1)), mortar_core::records::Order::Permuted(_)));
    let chunks = Vec::new();
    let img = image(&world, &chunks);
    let want = reference(&img);
    let got = par::check_schedules(|pool| fill_mem(&img, pool).unwrap());
    assert_eq!(got, want);
}

// spec: image.md I-25, I-41, I-42: every byte is a row; gaps are written
// with their pattern, positioned by offset; zero-size rows vanish.
#[test]
fn i41_gaps_patterns_and_zero_size_rows() {
    let t = a64();
    let mut b = Builder::new(t, 64);
    b.bytes(10, vec![0xaa; 2].into_boxed_slice());
    b.bytes(30, Vec::new().into_boxed_slice()); // zero-size: no row
    b.pad(13..22, t.trap);
    b.pad(40..50, t.nop);
    let plan = b.finish(16, &Pool::sequential()).unwrap();
    let world = World {
        plan,
        source: MemSource::default(),
        tables: FakeTables::default(),
        chunks: Vec::new(),
        records: Default::default(),
    }
    .indexed();
    let chunks = Vec::new();
    let img = image(&world, &chunks);
    let got = fill_mem(&img, &Pool::sequential()).unwrap();
    let mut want = vec![0u8; 64];
    want[10..12].fill(0xaa);
    let nop = 0xd503_201fu32.to_le_bytes();
    for o in 40..50 {
        want[o] = nop[o % 4];
    }
    assert_eq!(got, want);
    assert!(world.plan.rows.iter().all(|r| r.size > 0));
    assert_eq!(world.plan.rows.iter().map(|r| r.size).sum::<u64>(), 64);
    // x86's trap is 0xcc.
    let mut out = [0u8; 5];
    Pattern::byte(0xcc).fill(3, &mut out);
    assert_eq!(out, [0xcc; 5]);
}

// spec: image.md I-27, I-44: overlapping rows, or a row past the file,
// are `internal-error` "invalid fill plan".
#[test]
fn i27_invalid_plans_are_internal_errors() {
    let mut b = Builder::new(a64(), 32);
    b.bytes(0, vec![1; 8].into_boxed_slice());
    b.bytes(4, vec![2; 8].into_boxed_slice());
    let e = b.finish(16, &Pool::sequential()).unwrap_err();
    assert_eq!(e.id(), "internal-error");
    assert!(format!("{e:?}").contains("invalid fill plan"));
    let mut b = Builder::new(a64(), 8);
    b.bytes(4, vec![2; 8].into_boxed_slice());
    assert!(b.finish(16, &Pool::sequential()).is_err());
}

// spec: image.md I-31, I-104: a whole chunk is one call: the windows it
// touches are one job, together with the windows already in it; jobs are
// a function of the plan and W.
#[test]
fn i31_whole_chunks_join_windows() {
    let w = 4096u64;
    let mut b = Builder::new(a64(), 10 * w);
    let name = b.name(b".x");
    // A whole chunk of 2.5 windows from 1.5w, and one of 100 bytes
    // straddling 5w.
    b.synth(w + w / 2, 2 * w + w / 2, SynthUnit { chunk: 0, id: ChunkId(1), name, whole: true, eh: false });
    b.synth(5 * w - 50, 100, SynthUnit { chunk: 1, id: ChunkId(2), name, whole: true, eh: false });
    b.synth(6 * w - 50, 100, SynthUnit { chunk: 2, id: ChunkId(3), name, whole: false, eh: false });
    let plan = b.finish(w, &Pool::sequential()).unwrap();
    let jobs: Vec<_> = plan.jobs.iter().map(|j| (j.start / w, j.end / w)).collect();
    assert_eq!(jobs, vec![(0, 1), (1, 4), (4, 6), (6, 7), (7, 8), (8, 9), (9, 10)]);
    let chunks: Vec<Box<dyn SynthChunk + Sync>> = vec![
        Box::new(Summing { size: 2 * w + w / 2 }),
        Box::new(Summing { size: 100 }),
        Box::new(Counting { size: 100, seed: 3, sealed: None }),
    ];
    let world = World { plan, source: MemSource::default(), tables: FakeTables::default(), chunks, records: Default::default() }.indexed();
    let chunks = chunks_of(&world);
    let img = image(&world, &chunks);
    let got = par::check_schedules(|pool| fill_mem(&img, pool).unwrap());
    assert_eq!(got, reference(&img));
}

// spec: image.md I-43, I-65 (§16.1 "a failure in the middle of fill"): a
// failed read stops nothing else, is reported, and nothing more is written
// after the failing job's fill.
#[test]
fn i43_read_failure_is_reported() {
    let mut world = random_world(5, 512 * 1024, 4096);
    let (file, unit) = world
        .plan
        .inputs
        .iter()
        .find(|u| u.section_size > 64)
        .map(|u| (u.file, u.src + 8))
        .unwrap();
    world.source.fail = Some((file, unit));
    if let Some(f) = world.source.files.get_mut(&file) {
        f.1 = false;
    }
    let chunks = chunks_of(&world);
    let img = image(&world, &chunks);
    let a = fill_mem(&img, &Pool::new(4)).unwrap_err();
    let b = fill_mem(&img, &Pool::sequential()).unwrap_err();
    assert_eq!(a, b, "the diagnostics do not depend on the schedule");
    assert!(a.diagnostics.iter().any(|(_, d)| d.id() == "cannot-open-file"));
}

// spec: image.md I-64: a write error is `cannot-write-output` at (105, 0,
// 0), naming the output, once.
#[test]
fn i64_write_error_is_reported_once() {
    let world = random_world(11, 256 * 1024, 4096);
    let chunks = chunks_of(&world);
    let img = image(&world, &chunks);
    let sink = MemSink { fail_at: Some(70_000), ..MemSink::default() };
    let e = fill::write(&img, &sink, "a.out", &Pool::new(4)).unwrap_err();
    assert_eq!(e.diagnostics.len(), 1);
    let (k, d) = &e.diagnostics[0];
    assert_eq!(*k, mortar_diag::Key::new(105, 0, 0));
    assert_eq!(d.id(), "cannot-write-output");
    assert!(format!("{:?}", d.kind()).contains("a.out"));
}

// spec: image.md I-47, I-48, I-45, I-46: the tombstone per referring
// section, truncated to the field (the values are `mortar_arch::dead`'s;
// the truncation and the referring-section rule as fill applies them are
// the image's). ELFCLASS32's values are checked on the arch's function.
#[test]
fn i47_tombstone_table() {
    use mortar_image::tombstone::field;
    let a = arch();
    let cases: [(&[u8], bool, u8, Option<u64>); 14] = [
        (b".debug_ranges", false, 8, Some(1)),
        (b".debug_loc", false, 4, Some(1)),
        (b".debug_aranges", false, 8, Some(u64::MAX)),
        (b".debug_aranges", false, 4, Some(0xffff_ffff)),
        (b".debug_frame", false, 4, Some(0xffff_ffff)),
        (b".debug_info", false, 8, Some(u64::MAX)),
        (b".debug_info", false, 4, Some(0xffff_ffff)),
        (b".debug_info", false, 2, Some(0xffff)),
        (b".zdebug_info", false, 8, Some(u64::MAX)),
        (b".gnu_debuglink", false, 8, Some(u64::MAX)),
        (b".gcc_except_table", true, 8, Some(0)),
        (b".gcc_except_table", false, 4, Some(0)),
        // An alloc section with a debug name, and any other section: the
        // scan's `DiscardedSection` (I-46); fill applies the record.
        (b".debug_info", true, 8, None),
        (b".data", false, 8, None),
    ];
    for (name, alloc, width, want) in cases {
        assert_eq!(
            field(a, name, alloc, width),
            want,
            "{} alloc {alloc} width {width}",
            String::from_utf8_lossy(name)
        );
    }
    for name in [&b".line"[..], b".stab", b".gnu.linkonce.wi.x", b".gnu.debuglto_.debug_info"] {
        assert!(field(a, name, false, 8).is_some());
    }
    use mortar_elf::Class::Elf32;
    assert_eq!(mortar_arch::tombstone(b".debug_info", Elf32), 0);
    assert_eq!(mortar_arch::tombstone(b".debug_ranges", Elf32), 1);
    assert_eq!(mortar_arch::tombstone(b".debug_frame", Elf32) & 0xffff_ffff, 0xffff_ffff);
}

/// A non-alloc section `name` with an `ABS64` record against a dead target
/// at 0 and another record at `second` (if any).
fn tomb_world(name: &[u8], second: Option<u64>) -> Vec<u8> {
    let bytes = vec![0x77u8; 32];
    let mut b = Builder::new(a64(), 32);
    let mut source = MemSource::default();
    source.files.insert(FileId(1), (bytes, true));
    let mut records = vec![Reloc { r_offset: 0, key: key(1), r_type: ABS64, addend: 3 }];
    if let Some(o) = second {
        records.push(Reloc { r_offset: o, key: key(2), r_type: ABS64, addend: 0 });
    }
    let r = b.relocs(RelocSection { section: SectionId(1), rows: RecRows::Decoded(records.into()), proof: Vec::new().into() });
    let name = b.name(name);
    let unit = InputUnit {
        file: FileId(1),
        index: 1,
        section: SectionId(1),
        src: 0,
        start: 0,
        section_size: 32,
        addr: 0,
        alloc: false,
        name,
        relocs: Some(r),
    };
    b.input(0, 32, unit);
    let mut tables = FakeTables::default();
    tables.define(key(1), 0x1234, false);
    tables.define(key(2), 0x40_0000, true);
    let world = World {
        plan: b.finish(4096, &Pool::sequential()).unwrap(),
        source,
        tables,
        chunks: Vec::new(),
        records: Default::default(),
    }
    .indexed();
    let chunks = Vec::new();
    let img = image(&world, &chunks);
    let got = fill_mem(&img, &Pool::sequential()).unwrap();
    assert_eq!(got, reference(&img));
    got
}

// spec: image.md I-47 against a dead target, I-112: `.debug_aranges`
// writes all ones and the tuple's length 0, unless a record is there.
#[test]
fn i112_aranges_length_is_zeroed() {
    let got = tomb_world(b".debug_aranges", None);
    assert_eq!(&got[0..8], &[0xff; 8]);
    assert_eq!(&got[8..16], &[0; 8]);
    assert_eq!(&got[16..], &[0x77; 16]);
    let got = tomb_world(b".debug_aranges", Some(8));
    assert_eq!(&got[0..8], &[0xff; 8]);
    assert_eq!(
        u64::from_le_bytes(got[8..16].try_into().unwrap()),
        0x40_0000,
        "a record's field is kept"
    );
    let got = tomb_world(b".debug_ranges", None);
    assert_eq!(u64::from_le_bytes(got[0..8].try_into().unwrap()), 1);
    assert_eq!(&got[8..16], &[0x77; 8]);
    let got = tomb_world(b".debug_info", None);
    assert_eq!(&got[0..8], &[0xff; 8]);
}

// spec: image.md I-72, I-73, §16.1: the build id written at the sealed
// range equals `mortar_digest::tree_root` over the file with the
// descriptor zeroed, for every schedule; the window holding it is hashed
// with its zeros.
#[test]
fn i73_build_id_matches_the_reference_tree() {
    let w = IMAGE_BLOCK;
    let size = 3 * w + 12345;
    let mut b = Builder::new(a64(), size);
    let name = b.name(b".note.gnu.build-id");
    let chunk_at = 2 * w - 10;
    // The descriptor at chunk offset 16..36, straddling 2w.
    b.synth(chunk_at, 64, SynthUnit { chunk: 0, id: ChunkId(1), name, whole: false, eh: false });
    b.synth(0, chunk_at, SynthUnit { chunk: 1, id: ChunkId(2), name, whole: false, eh: false });
    b.sealed(chunk_at + 16..chunk_at + 36);
    let chunks: Vec<Box<dyn SynthChunk + Sync>> = vec![
        Box::new(Counting { size: 64, seed: 9, sealed: Some(16..36) }),
        Box::new(Counting { size: chunk_at, seed: 1, sealed: None }),
    ];
    let world = World {
        plan: b.finish(w, &Pool::sequential()).unwrap(),
        source: MemSource::default(),
        tables: FakeTables::default(),
        chunks,
        records: Default::default(),
    }
    .indexed();
    let chunks = chunks_of(&world);
    for style in [TreeStyle::Default, TreeStyle::Fast, TreeStyle::Sha1, TreeStyle::Sha256] {
        let mut img = image(&world, &chunks);
        img.build_id = Some(style);
        let got = par::check_schedules(|pool| {
            let sink = MemSink::default();
            let f = fill::write(&img, &sink, "out", pool).unwrap();
            (sink.bytes.into_inner().unwrap(), f.build_id)
        });
        let (bytes, id) = got;
        let id = id.unwrap();
        assert_eq!(id.len(), style.id_len());
        let z = (chunk_at + 16) as usize..(chunk_at + 36) as usize;
        let root = mortar_digest::tree_root(style.tree_fn(), &bytes, z.clone());
        assert_eq!(style.id(&root).unwrap(), &id[..]);
        let mut want_desc = id.clone();
        want_desc.resize(20, 0);
        assert_eq!(&bytes[z], &want_desc[..], "the id is written at the descriptor");
    }
    // A window other than IMAGE_BLOCK cannot hash leaves (I-75).
    let mut b = Builder::new(a64(), 100);
    b.bytes(0, vec![0; 100].into_boxed_slice());
    let world = World {
        plan: b.finish(4096, &Pool::sequential()).unwrap(),
        source: MemSource::default(),
        tables: FakeTables::default(),
        chunks: Vec::new(),
        records: Default::default(),
    }
    .indexed();
    let mut img = image(&world, &[]);
    img.build_id = Some(TreeStyle::Default);
    assert!(fill_mem(&img, &Pool::sequential()).is_err());
}

// spec: image.md I-30 (redirect), arch.md §3.6: `redirect` sees the
// record's section offset, not the shifted one of the clip stand-in.
#[test]
fn i35_redirect_sees_section_offsets() {
    let mut words = vec![0u32; 64];
    words[40] = BL;
    let records = vec![Reloc { r_offset: 160, key: key(1), r_type: CALL26, addend: 0 }];
    let mut world = one_section(&words, 0, records, 8192, 4096);
    world.tables.redirects.insert((SectionId(1), 160), 0x40_0400);
    let chunks = Vec::new();
    let img = image(&world, &chunks);
    let got = fill_mem(&img, &Pool::sequential()).unwrap();
    assert_eq!(got, reference(&img));
    let word = u32::from_le_bytes(got[160..164].try_into().unwrap());
    // bl to 0x400400 from 0x4000a0: imm26 = (0x400400 − 0x4000a0) / 4.
    assert_eq!(word, BL | ((0x40_0400 - 0x40_00a0) / 4));
}

#[test]
fn output_kind_is_static() {
    // The fakes model a static executable (M1).
    let world = one_section(&[0; 4], 0, Vec::new(), 16, 16);
    let img = image(&world, &[]);
    assert_eq!(img.output, OutputKind::Static);
}

#[allow(dead_code)]
fn _unused(_: Image<'_>) {}

// spec: image.md I-111: no `apply` writes outside `[r_offset − before,
// r_offset + width + after)` with the stated `max_reach` (AArch64 (0, 4)):
// every record of random sections, and the GD→LE sequence, applied alone
// to the input bytes.
#[test]
fn i111_max_reach_bounds_every_write() {
    use mortar_arch::{Arch, FillCx, RelocAt, SectionOut, Site};
    let (before, after) = a64().max_reach;
    let mut worlds = vec![random_world(17, 1 << 19, 4096)];
    let o = 64u64;
    let mut words = vec![0u32; 16];
    words.extend([ADRP_X0, ADD_X0_X0, BL, NOP, 0, 0]);
    let records = vec![
        Reloc { r_offset: o, key: key(2), r_type: TLSGD_ADR_PAGE21, addend: 0 },
        Reloc { r_offset: o + 4, key: key(2), r_type: TLSGD_ADD_LO12_NC, addend: 0 },
        Reloc { r_offset: o + 8, key: key(3), r_type: CALL26, addend: 0 },
    ];
    let mut gd = one_section(&words, 0, records, 4096, 4096);
    gd.tables.names.insert(key(3), b"__tls_get_addr".to_vec());
    gd.tables.define(key(2), TLS.start + 0x10, true);
    worlds.push(gd);
    let mut checked = 0;
    for w in &worlds {
        let cx = FillCx {
            tables: &w.tables,
            output: OutputKind::Static,
            got_base: None,
            tls: Some(TLS),
        };
        for u in &w.plan.inputs {
            let Some(r) = u.relocs else { continue };
            let rs = &w.plan.relocs[r as usize];
            let mut input = vec![0u8; u.section_size as usize];
            mortar_image::source::Source::copy_to(
                &w.source,
                u.file,
                u.src..u.src + u.section_size,
                &mut input,
            )
            .unwrap();
            let name = w.plan.name(u.name).to_vec();
            let recs: &[Reloc] = decoded(rs);
            for (i, rec) in recs.iter().enumerate() {
                let Ok(h) =
                    ARCH_M2.classify(rec.r_type, Site { bytes: &input, offset: rec.r_offset })
                else {
                    continue;
                };
                let at = RelocAt {
                    file: u.file,
                    section: u.section,
                    index: i as u32,
                    r_offset: rec.r_offset,
                    key: rec.key,
                    r_type: rec.r_type,
                    addend: rec.addend,
                    howto: &h,
                    site: Site { bytes: &input, offset: rec.r_offset },
                    records: &recs,
                    proof: &rs.proof,
                    leads: true,
                };
                // A sentinel buffer: every byte apply writes differs from it
                // in at least one of two fills.
                for fillb in [0x00u8, 0xff] {
                    let mut bytes = vec![fillb; input.len()];
                    let mut out = SectionOut::whole(&mut bytes, u.addr, u.alloc, &name);
                    let _ = ARCH_M2.apply(&cx, &at, &mut out);
                    let lo = rec.r_offset.saturating_sub(before);
                    let hi = rec.r_offset + u64::from(h.field.width()) + after;
                    for (k, b) in bytes.iter().enumerate() {
                        if *b != fillb {
                            assert!(
                                (lo..hi).contains(&(k as u64)),
                                "{} at {:#x} wrote {k:#x}",
                                h.name,
                                rec.r_offset
                            );
                        }
                    }
                }
                checked += 1;
            }
        }
    }
    assert!(checked > 100, "{checked}");
}
