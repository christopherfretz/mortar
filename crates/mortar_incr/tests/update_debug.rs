//! G-1's tests (`spec/incr.md` IN-50..IN-57, WP-228): the debug placement
//! of an update over synthetic stored layouts (`MemoryLayout`,
//! `MemoryState`) and the delta layout's shape for debug sections
//! (`LayoutInput::debug`: old rows in `killed`, nothing placed). These
//! judge the DECISIONS (places, holes, reasons) against the rules; the
//! comparison of written outputs with a full link through gdb, lldb,
//! `llvm-dwarfdump --verify`, readelf and `cargo xtask plumb` is NOT here
//! yet (record `spec/landed/incr/WP-228.md` §B2: the test-only writer).

use mortar_core::column::Column;
use mortar_core::incr::{GrowthStep, LayoutDelta, MemoryLayout, StoredLayout, StoredSection};
use mortar_core::layout::{Chunk, ChunkKind, Hole, HoleKind, Part, Place};
use mortar_core::{ChunkId, FileId, OutSecId, SectionId};
use mortar_incr::format::{SectionRow, Str};
use mortar_incr::update::debug::{
    self, Allot, DebugStrings, Taken, aranges, choose, fits, form, place, strings_with,
};
use mortar_incr::update::rows::{FileRow, GroupRow, debug_str_class, name_hash};
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::{Change, ChangedFile, ChangedFiles, OdrRows};

// ---- the harness ------------------------------------------------------------

const PROGBITS: u32 = 1;

fn chunk(out: u32, offset: u64, size: u64, capacity: u64) -> Chunk {
    Chunk {
        kind: ChunkKind::Body(Part::first(OutSecId(out))),
        sh_type: PROGBITS,
        flags: 0,
        align: 0,
        size,
        capacity,
        addr: None,
        offset,
        shndx: Some(out + 1),
        relro: false,
    }
}

fn stored(id: u32, file: u32, name: &[u8], size: u64, chunk: u32, off: u64) -> StoredSection {
    StoredSection {
        id: SectionId(id),
        file: FileId(file),
        index: id,
        anchor_kind: 0,
        anchor: Vec::new(),
        name: name.to_vec(),
        ordinal: 0,
        sh_type: PROGBITS,
        sh_flags: 0,
        size,
        align: 1,
        place: Place::at(ChunkId(chunk), off),
    }
}

fn hole(chunk: u32, offset: u64, size: u64, kind: HoleKind) -> Hole {
    Hole { chunk: ChunkId(chunk), offset, size, kind }
}

/// A world: debug output sections (one chunk each), stored sections and
/// holes, a state with `files` FILES rows (rank = id) and `strings`.
struct World {
    mem: MemoryLayout,
    state: MemoryState,
    changed: ChangedFiles,
}

impl World {
    fn new(sections: &[(&[u8], u64, u64)]) -> Self {
        let mut mem = MemoryLayout::default();
        let mut at = 0x1000;
        for (i, &(name, size, capacity)) in sections.iter().enumerate() {
            mem.outsecs.push(name.to_vec());
            mem.chunks.push(chunk(i as u32, at, size, capacity));
            mem.order.push(ChunkId(i as u32));
            at += capacity;
        }
        let state = MemoryState {
            files: (0..8).map(|f| FileRow { rank: f * 10, ..FileRow::default() }).collect(),
            ..MemoryState::default()
        };
        Self { mem, state, changed: ChangedFiles::default() }
    }

    /// A state string (for `GROUPS` signatures).
    fn state_str(&mut self, s: &[u8]) -> Str {
        let off = self.state.strings.len() as u32;
        self.state.strings.extend_from_slice(s);
        Str { off, len: s.len() as u32 }
    }

    /// A changed file's string, past the state's `STRINGS`.
    fn new_str(&mut self, s: &[u8]) -> Str {
        let base = self.state.strings.len() as u32;
        let off = base + self.changed.strings.len() as u32;
        self.changed.strings.extend_from_slice(s);
        Str { off, len: s.len() as u32 }
    }

    /// A changed file: `secs` `(name, size, signature)`; its first new id
    /// is `first`.
    fn change(&mut self, file: u32, change: Change, first: u32, secs: &[(&[u8], u64, &[u8])]) {
        let mut rows = Vec::new();
        for &(name, size, sig) in secs {
            let name = self.new_str(name);
            let anchor = if sig.is_empty() { Str::default() } else { self.new_str(sig) };
            rows.push(SectionRow {
                file,
                anchor_kind: u8::from(!sig.is_empty()),
                anchor,
                name,
                sh_type: PROGBITS,
                size,
                align: 1,
                ..SectionRow::default()
            });
        }
        let f = ChangedFile {
            file: FileId(file),
            change,
            row: FileRow { rank: u64::from(file) * 10, ..FileRow::default() },
            first_section: SectionId(first),
            sections: rows,
            keys: Vec::new(),
            first_global: 0,
            mentions: Vec::new(),
            values: Vec::new(),
            index: Vec::new(),
            edges: Vec::new(),
            meta_units: 0,
            debug: true,
            versioned: 0,
            odr: OdrRows::default(),
        };
        self.changed.files.push(f);
        self.changed.files.sort_by_key(|c| c.file);
    }

    /// What `layout(debug: true)` gives for these files: the stored
    /// chunks, the changed files' old debug rows in `killed`.
    fn delta(&self) -> LayoutDelta {
        let mut d = LayoutDelta::default();
        d.placement.chunks = Column::from_vec(self.mem.chunks.clone()).unwrap();
        d.files = self.changed.files.iter().map(|c| c.file).collect();
        d.killed =
            self.mem.sections.iter().filter(|s| d.files.contains(&s.file)).map(|s| s.id).collect();
        d
    }

    fn run(&self, d: &mut LayoutDelta) -> Result<debug::DebugPlan, mortar_core::incr::IncrReason> {
        let view = self.state.view();
        let stored = StoredLayout::new(&self.mem);
        place(&view, &stored, &self.changed, DebugStrings::default(), d)
    }
}

fn holes_of(d: &LayoutDelta, mem: &MemoryLayout, c: u32) -> Vec<Hole> {
    d.holes_of(&StoredLayout::new(mem), ChunkId(c)).unwrap()
}

/// Three files' `.debug_info` contributions (56 B in a 72 B allotment, 96
/// B in a 104 B one, 2000 B) and a tail; file 1 is the one edited.
fn two_files() -> World {
    let mut w = World::new(&[(b".debug_info", 2176, 8192)]);
    w.mem.sections.push(stored(1, 1, b".debug_info", 56, 0, 0));
    w.mem.holes.push(hole(0, 56, 16, HoleKind::Debug));
    w.mem.sections.push(stored(2, 2, b".debug_info", 96, 0, 72));
    w.mem.holes.push(hole(0, 168, 8, HoleKind::Debug));
    // File 5: 2000 bytes, so that IN-57's quarter is not reached.
    w.mem.sections.push(stored(5, 5, b".debug_info", 2000, 0, 176));
    w
}

// ---- IN-50's three placements ------------------------------------------------

// spec: incr.md IN-50 step 1 (in place when the new size fits the file's
// allotment with a remainder of 0 or one neutral form)
#[test]
fn in_place_when_it_fits_its_allotment() {
    let mut w = two_files();
    w.change(1, Change::Changed, 10, &[(b".debug_info", 50, b"")]);
    let mut d = w.delta();
    let plan = w.run(&mut d).unwrap();
    assert_eq!(plan.contributions[0].taken, Taken::InPlace);
    assert_eq!(d.placement.place[SectionId(10)], Place::at(ChunkId(0), 0));
    // The remainder 72 - 50 = 22 is the slack (Debug), in neutral form.
    assert_eq!(holes_of(&d, &w.mem, 0)[0], hole(0, 50, 22, HoleKind::Debug));
    assert!(d.killed.is_empty(), "{:?}", d.killed);
    assert!(d.moved.is_empty(), "same place: not moved");
    // 60 bytes would leave 12 < 15: not in place.
    assert!(!fits(form(b".debug_info").unwrap(), 72, 60));
}

// spec: incr.md IN-50 step 2 (best fit into a dead range of the section;
// the remainder 0 or at least the minimum, left dead) and IN-53
#[test]
fn best_fit_into_dead_range_with_remainder() {
    let mut w = two_files();
    w.mem.chunks[0].size = 2176 + 300;
    w.mem.holes.push(hole(0, 2176, 200, HoleKind::DeadDebug));
    w.mem.holes.push(hole(0, 2376, 100, HoleKind::DeadDebug));
    w.change(1, Change::Changed, 10, &[(b".debug_info", 80, b"")]);
    let mut d = w.delta();
    let plan = w.run(&mut d).unwrap();
    assert_eq!(plan.contributions[0].taken, Taken::DeadRange);
    // The smaller range that fits (100 B, remainder 20 >= 15).
    assert_eq!(d.placement.place[SectionId(10)], Place::at(ChunkId(0), 2376));
    let hs = holes_of(&d, &w.mem, 0);
    assert!(hs.contains(&hole(0, 2456, 20, HoleKind::DeadDebug)), "{hs:?}");
    assert!(hs.contains(&hole(0, 0, 72, HoleKind::DeadDebug)), "old allotment dead: {hs:?}");
    assert_eq!(d.moved, vec![SectionId(10)]);
    // A remainder of 1..15 is never taken.
    let dead = [hole(0, 0, 90, HoleKind::DeadDebug)];
    assert_eq!(choose(b".debug_info", None, 80, 1, &dead, 100, 100), None);
}

// spec: incr.md IN-50 step 3 (the tail reserve, IN-33, with its size
// class) and IN-57 (fits nowhere: debug-compaction)
#[test]
fn tail_when_nothing_fits() {
    let mut w = two_files();
    w.change(1, Change::Changed, 10, &[(b".debug_info", 200, b"")]);
    let mut d = w.delta();
    let plan = w.run(&mut d).unwrap();
    let c = &plan.contributions[0];
    assert_eq!((c.taken, c.allot.offset), (Taken::Tail, 2176));
    let want = 2176 + 200 + debug::slack_of(b".debug_info", 200);
    assert_eq!(d.placement.chunks[ChunkId(0)].size, want);
    let mut w = two_files();
    w.mem.chunks[0].capacity = 2300;
    w.change(1, Change::Changed, 10, &[(b".debug_info", 200, b"")]);
    let e = w.run(&mut w.delta()).unwrap_err();
    assert_eq!(e.name(), "debug-compaction");
}

// spec: incr.md IN-50 ("the old allotment, if left, becomes a dead
// range"), IN-3a; R-3 (always neutral: the slack hole goes into it)
#[test]
fn old_allotment_becomes_neutral_dead_range() {
    let mut w = two_files();
    w.change(1, Change::Changed, 10, &[(b".debug_info", 200, b"")]);
    let mut d = w.delta();
    w.run(&mut d).unwrap();
    let hs = holes_of(&d, &w.mem, 0);
    assert_eq!(hs[0], hole(0, 0, 72, HoleKind::DeadDebug), "{hs:?}");
    assert!(!hs.iter().any(|h| h.kind == HoleKind::Debug && h.offset == 56));
    assert!(d.hole_chunks.contains(&ChunkId(0)));
}

// spec: incr.md IN-52 (a file that leaves the link is neutralized too)
#[test]
fn removed_file_contributions_neutral() {
    let mut w = two_files();
    w.change(2, Change::Gone, 10, &[]);
    let mut d = w.delta();
    w.run(&mut d).unwrap();
    assert!(holes_of(&d, &w.mem, 0).contains(&hole(0, 72, 104, HoleKind::DeadDebug)));
    assert_eq!(d.killed, vec![SectionId(2)], "the gone file's row stays killed");
}

// spec: incr.md IN-51 (a dead range at the section's offset 0: the first
// file's; the neutral unit there is the section's first unit)
#[test]
fn dead_range_at_offset_zero() {
    let mut w = two_files();
    w.change(1, Change::Gone, 10, &[]);
    let mut d = w.delta();
    let plan = w.run(&mut d).unwrap();
    assert_eq!(holes_of(&d, &w.mem, 0)[0], hole(0, 0, 72, HoleKind::DeadDebug));
    assert_eq!(plan.aranges, vec![(FileId(1), 0, 56)]);
}

// spec: incr.md IN-51, R-4 (`.debug_addr`: in place when it fits, else
// the tail; a dead table is never reused)
#[test]
fn debug_addr_in_place_or_tail() {
    let f = b".debug_addr";
    let allot = Allot { offset: 0, size: 64 };
    let old = Some(allot);
    assert_eq!(choose(f, old, 56, 8, &[], 200, 400), Some((Taken::InPlace, allot)));
    let dead = [hole(0, 64, 128, HoleKind::DeadDebug)];
    // 60: remainder 4 is no form (8 at least, a multiple of 8): the tail.
    let got = choose(f, old, 60, 8, &dead, 200, 400).unwrap();
    assert_eq!(got.0, Taken::Tail);
    assert_eq!(got.1.offset, 200);
}

// spec: R-10.3 (`.debug_loclists` and `.debug_frame` take the tail only,
// until the five-check experiment)
#[test]
fn loclists_and_frame_take_the_tail() {
    let old = Some(Allot { offset: 0, size: 64 });
    let dead = [hole(0, 64, 128, HoleKind::DeadDebug)];
    for f in [&b".debug_loclists"[..], b".debug_frame"] {
        let (t, a) = choose(f, old, 64, 1, &dead, 300, 1000).unwrap();
        assert_eq!((t, a.offset), (Taken::Tail, 300), "{}", String::from_utf8_lossy(f));
    }
}

// spec: incr.md IN-54 (`.debug_aranges` rewritten whole, in full-link
// order: a changed file's sets where its old ones were)
#[test]
fn aranges_rewritten_whole_in_full_link_order() {
    let set = |info: u32, tag: u8| {
        let mut v = 28u32.to_le_bytes().to_vec();
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&info.to_le_bytes());
        v.extend_from_slice(&[8, 0, 0, 0, 0, 0]);
        v.extend_from_slice(&[tag; 16]);
        v
    };
    let prev = [set(0, 1), set(72, 2), set(176, 3)].concat();
    let new = set(300, 9);
    let got = aranges(&prev, &[(72, 176, new.clone())]).unwrap();
    assert_eq!(got, [set(0, 1), new, set(176, 3)].concat());
    // A file with new sets and no old one: no position known.
    assert_eq!(
        aranges(&prev, &[(500, 600, set(500, 4))]).unwrap_err().name(),
        "update-not-implemented"
    );
}

// ---- strings (IN-56, R-2) -----------------------------------------------------

/// A `.debug_str` chunk at file offset 0x100 holding `strs`.
fn str_world(strs: &[&[u8]]) -> (MemoryLayout, Vec<u8>, Vec<u32>) {
    let mut sec = Vec::new();
    let mut offs = Vec::new();
    for s in strs {
        offs.push(sec.len() as u32);
        sec.extend_from_slice(s);
        sec.push(0);
    }
    let mut mem = MemoryLayout::default();
    mem.outsecs.push(b".debug_str".to_vec());
    mem.chunks.push(chunk(0, 0x100, sec.len() as u64, sec.len() as u64 + 64));
    let mut out = vec![0xeeu8; 0x100];
    out.extend_from_slice(&sec);
    (mem, out, offs)
}

const STR: u8 = debug_str_class::STR;

// spec: incr.md IN-56, R-2.1 (a hit verified against the previous
// output; a miss appended once at the tail, its row for the patch)
#[test]
fn strings_found_verified_or_appended() {
    let (mem, out, offs) = str_world(&[b"int", b"main"]);
    let rows = [(name_hash(b"int", b""), offs[0]), (name_hash(b"main", b""), offs[1])];
    let lookup = |_: u8, h: u64| Ok(rows.iter().find(|r| r.0 == h).map(|r| r.1));
    let input = b"main\0new\0int\0new\0";
    let s =
        strings_with(&lookup, &StoredLayout::new(&mem), &[(FileId(1), STR, input)], &out).unwrap();
    let size = mem.chunks[0].size;
    assert_eq!(s.output_offset(FileId(1), STR, 0), Some(u64::from(offs[1])));
    assert_eq!(s.output_offset(FileId(1), STR, 5), Some(size));
    assert_eq!(s.output_offset(FileId(1), STR, 9), Some(u64::from(offs[0])));
    assert_eq!(s.output_offset(FileId(1), STR, 13), Some(size), "appended once");
    assert_eq!(s.appended[&STR].2, b"new\0");
    assert_eq!(s.rows.len(), 1);
    assert_eq!((s.hits, s.misses), (2, 1));
}

/// One string looked up through a row that says `row`: its output offset.
fn through(row: u32, strs: &[&[u8]], want: &[u8]) -> (u64, u64) {
    let (mem, out, _) = str_world(strs);
    let lookup = |_: u8, _: u64| Ok(Some(row));
    let input = [want, b"\0"].concat();
    let s =
        strings_with(&lookup, &StoredLayout::new(&mem), &[(FileId(1), STR, &input)], &out).unwrap();
    (s.output_offset(FileId(1), STR, 0).unwrap(), mem.chunks[0].size)
}

// spec: R-2.1 (corrupt rows: a wrong offset is appended, never named)
#[test]
fn debug_strs_wrong_offset() {
    let (got, size) = through(0, &[b"alpha", b"beta"], b"beta");
    assert_eq!(got, size);
}

// spec: R-2.1 (an offset in the middle of another string: appended unless
// the bytes there ARE the string and its NUL, a valid suffix)
#[test]
fn debug_strs_mid_string() {
    let (got, size) = through(2, &[b"alpha", b"beta"], b"al");
    assert_eq!(got, size, "\"pha\" is not \"al\"");
    let (got, _) = through(2, &[b"alpha", b"beta"], b"pha");
    assert_eq!(got, 2, "a suffix with its NUL is the string");
}

// spec: R-2.1 (a row past the section's end; one whose NUL would be past it)
#[test]
fn debug_strs_past_end() {
    let (got, size) = through(1000, &[b"alpha"], b"alpha");
    assert_eq!(got, size);
    let (got, size) = through(5, &[b"alpha"], b"");
    assert_eq!(got, 5, "the empty string at alpha's NUL");
    assert_eq!(size, 6);
}

// spec: R-2.1 (a duplicated key, "of rows with one key it returns any":
// each hit verified, the wrong one appended)
#[test]
fn debug_strs_duplicate_key() {
    for row in [0u32, 6] {
        let (got, size) = through(row, &[b"alpha", b"beta"], b"beta");
        assert_eq!(got, if row == 6 { 6 } else { size }, "row {row}");
    }
}

// spec: B1-5 (an appended string past 2^32: debug-str-limit), through
// `strings` with a faked chunk size
#[test]
fn debug_str_limit_through_strings() {
    let (mut mem, out, _) = str_world(&[b"a"]);
    mem.chunks[0].size = (1 << 32) - 2;
    mem.chunks[0].capacity = u64::MAX;
    let lookup = |_: u8, _: u64| Ok(None);
    let e = strings_with(&lookup, &StoredLayout::new(&mem), &[(FileId(1), STR, b"xy\0zz\0")], &out)
        .unwrap_err();
    assert_eq!(e.name(), "debug-str-limit");
}

// ---- COMDAT groups (R-5, image.md I-47a) --------------------------------------

/// File 1 keeps a type-unit group `sig` in `.debug_info` (with its CU);
/// file 3 has a copy (not kept).
fn groups() -> World {
    let mut w = World::new(&[(b".debug_info", 2200, 8192)]);
    w.mem.sections.push(stored(5, 5, b".debug_info", 2000, 0, 200));
    let sig = w.state_str(b"sig");
    let mut tu = stored(1, 1, b".debug_info", 40, 0, 0);
    tu.anchor_kind = 1;
    tu.anchor = b"sig".to_vec();
    w.mem.sections.push(tu);
    w.mem.sections.push(stored(2, 1, b".debug_info", 64, 0, 40));
    w.mem.holes.push(hole(0, 104, 16, HoleKind::Debug));
    let mut copy = stored(3, 3, b".debug_info", 40, 0, 0);
    copy.anchor_kind = 1;
    copy.anchor = b"sig".to_vec();
    copy.place = Place::NONE;
    w.mem.sections.push(copy);
    w.mem.sections.push(stored(4, 3, b".debug_info", 80, 0, 120));
    w.state.groups.push(GroupRow { signature: sig, file: 1, tomb: false });
    w.state.groups.push(GroupRow { signature: sig, file: 3, tomb: false });
    w
}

// spec: R-5 (a): a kept group copy of a changed file, same signature and
// size, stays at its old offset (the bytes compared by the join)
#[test]
fn type_unit_group_kept_in_place() {
    let mut w = groups();
    w.change(1, Change::Changed, 10, &[(b".debug_info", 40, b"sig"), (b".debug_info", 70, b"")]);
    let mut d = w.delta();
    let plan = w.run(&mut d).unwrap();
    assert_eq!(d.placement.place[SectionId(10)], Place::at(ChunkId(0), 0));
    assert_eq!(plan.comdat_in_place, vec![(SectionId(10), SectionId(1))]);
    // The CU (70 > 64 + 16 - 15) left its range: dead, the group copy kept.
    let hs = holes_of(&d, &w.mem, 0);
    assert!(hs.contains(&hole(0, 40, 80, HoleKind::DeadDebug)), "{hs:?}");
    assert!(!hs.iter().any(|h| h.offset < 40), "{hs:?}");
    assert!(!d.killed.contains(&SectionId(1)));
}

// spec: R-5 (b): the keeper drops the group, or its copy must move
#[test]
fn keeper_drops_group_is_debug_comdat_keeper() {
    let mut w = groups();
    w.change(1, Change::Changed, 10, &[(b".debug_info", 64, b"")]);
    // File 1 has no copy now: file 3's revives (another file): the
    // group's kept copy moves.
    let e = w.run(&mut w.delta()).unwrap_err();
    assert_eq!(e.name(), "debug-comdat-keeper", "{e:?}");
    let mut w = groups();
    w.change(1, Change::Changed, 10, &[(b".debug_info", 48, b"sig"), (b".debug_info", 64, b"")]);
    let e = w.run(&mut w.delta()).unwrap_err();
    assert_eq!(e.name(), "debug-comdat-keeper", "the kept copy changed size");
}

// spec: R-5 (c), image.md I-47a: a changed file's import of a group it
// does not keep takes the keeper's copy's stored place (the keeper is
// never read); none: the tombstone
#[test]
fn import_of_unkept_group_takes_keepers_offset() {
    let mut w = groups();
    w.change(3, Change::Changed, 10, &[(b".debug_info", 40, b"sig"), (b".debug_info", 80, b"")]);
    let view = w.state.view();
    let stored = StoredLayout::new(&w.mem);
    let mut d = w.delta();
    let plan = place(&view, &stored, &w.changed, DebugStrings::default(), &mut d).unwrap();
    assert_eq!(d.placement.place.get(SectionId(10)).copied().unwrap_or(Place::NONE), Place::NONE);
    assert_eq!(plan.contributions.len(), 1, "only the CU is placed");
    let got = debug::import_target(&view, &stored, &w.changed, &d, b"sig", b".debug_info", 40);
    assert_eq!(got.unwrap(), Some((ChunkId(0), 0)));
    let other = debug::import_target(&view, &stored, &w.changed, &d, b"sig", b".debug_info", 41);
    assert_eq!(other.unwrap(), None, "not the same size: I-47's tombstone");
}

// ---- the whole-output fall-backs ----------------------------------------------

// spec: R-8 (`debug-growth`: a growth step in an output with debug sections)
#[test]
fn growth_step_with_debug_is_debug_growth() {
    let mut w = two_files();
    w.change(1, Change::Changed, 10, &[(b".debug_info", 50, b"")]);
    let mut d = w.delta();
    d.growth_steps.push(GrowthStep { out: OutSecId(9), step: 4096 });
    assert_eq!(w.run(&mut d).unwrap_err().name(), "debug-growth");
}

// spec: incr.md IN-57 (dead bytes over a quarter of the debug bytes)
#[test]
fn compaction_when_dead_over_quarter() {
    let mut w = two_files();
    w.change(2, Change::Gone, 10, &[]);
    // 104 of 2176 dead: under a quarter.
    let e = w.run(&mut w.delta());
    assert!(e.is_ok(), "{e:?}");
    let mut w = two_files();
    w.mem.chunks[0].size = 4000;
    w.mem.holes.push(hole(0, 2176, 1100, HoleKind::DeadDebug));
    w.change(1, Change::Changed, 10, &[(b".debug_info", 50, b"")]);
    assert_eq!(w.run(&mut w.delta()).unwrap_err().name(), "debug-compaction");
    assert!(!debug::over_quarter(250, 1000));
    assert!(debug::over_quarter(251, 1000));
}

// spec: R-6 and the brief ("a section of an unchanged file that dies or
// revives under --gc-sections is the named fall-back"; G-3 lifts it)
#[test]
fn unchanged_file_section_dies_under_gc_is_named() {
    let mut w = two_files();
    w.change(1, Change::Changed, 10, &[(b".debug_info", 50, b"")]);
    let mut d = w.delta();
    d.killed.push(SectionId(2));
    let e = w.run(&mut d).unwrap_err();
    assert_eq!(e.name(), "update-not-implemented", "{e:?}");
}

// spec: R-1's other side: after `place`, no live debug section of a file
// of `files` is unplaced (strings, aranges and unkept group copies are
// not placed in a full link either)
#[test]
fn place_leaves_no_debug_section_unplaced() {
    let mut w = World::new(&[
        (b".debug_info", 176, 4096),
        (b".debug_line", 64, 1024),
        (b".debug_frame", 32, 512),
    ]);
    w.mem.sections.push(stored(1, 1, b".debug_info", 56, 0, 0));
    w.mem.sections.push(stored(2, 1, b".debug_line", 64, 1, 0));
    w.mem.sections.push(stored(3, 1, b".debug_frame", 32, 2, 0));
    w.change(
        1,
        Change::Changed,
        10,
        &[(b".debug_info", 60, b""), (b".debug_line", 64, b""), (b".debug_frame", 40, b"")],
    );
    let mut d = w.delta();
    w.run(&mut d).unwrap();
    for id in 10..13 {
        assert!(d.placement.place[SectionId(id)].get().is_some(), "section {id}");
    }
    assert_eq!(d.killed, Vec::<SectionId>::new(), "every old row has a successor");
}

// spec: incr.md IN-55 (`debug-name-index`), B1 (`SHF_COMPRESSED` input:
// `debug-unit-kind`), and a `.debug_*` section the table does not know
#[test]
fn named_fall_backs_of_inputs() {
    let mut w = two_files();
    w.change(1, Change::Changed, 10, &[(b".debug_names", 50, b"")]);
    assert_eq!(w.run(&mut w.delta()).unwrap_err().name(), "debug-name-index");
    let mut w = two_files();
    w.change(1, Change::Changed, 10, &[(b".debug_info", 50, b"")]);
    w.changed.files[0].sections[0].sh_flags = 0x800;
    assert_eq!(w.run(&mut w.delta()).unwrap_err().name(), "debug-unit-kind");
    let mut w = two_files();
    w.change(1, Change::Changed, 10, &[(b".debug_cu_index", 50, b"")]);
    assert_eq!(w.run(&mut w.delta()).unwrap_err().name(), "update-not-implemented");
    let mut w = two_files();
    w.change(1, Change::Changed, 10, &[(b".debug_line", 50, b"")]);
    assert_eq!(w.run(&mut w.delta()).unwrap_err().name(), "new-output-section");
}
