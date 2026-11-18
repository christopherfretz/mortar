//! U-5's tests (`spec/incr.md` IN-49a, WP-217): the full link's tables in
//! an incremental-mode output (per-file blocks, fillers, the tail reserve,
//! the `.strtab` regions) and an update's plan (`symtab_incr::plan`) on
//! hand-made stored layouts, with what the plan reads counted (O(change)).

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use common::{Addrs, Secs, Syms, collector};
use mortar_core::incr::{
    GrowthStep, IncrReason, LayoutDelta, LayoutRows, MemoryLayout, StoredEhRange, StoredFrag,
    StoredFragUse, StoredKeyRef, StoredLayout, StoredRegion, StoredSection, StoredSlot,
    SymtabBlock, SymtabEntry, SymtabPlan as UpdatePlan, SymtabTables, Unimplemented,
};
use mortar_core::layout::{
    Chunk, ChunkKind, Defined, Hole, Part, Patch, PatchKind, Place, Placement, Redirect, Segment,
    SymPlace, ThunkTarget, Tls,
};
use mortar_core::par::Pool;
use mortar_core::plan::Discard;
use mortar_core::{ChunkId, Column, FileId, OutSecId, SectionId, SymKey, SymbolId};
use mortar_elf::Class;
use mortar_elf::consts::{
    SHF_ALLOC, SHF_EXECINSTR, SHF_TLS, SHF_WRITE, SHN_ABS, SHT_PROGBITS, STB_GLOBAL, STB_LOCAL,
    STT_FILE, STT_FUNC, STT_NOTYPE, STT_OBJECT, STT_SECTION, STT_TLS, STV_HIDDEN,
};
use mortar_synth::out::Window;
use mortar_synth::symtab::{
    GlobalIn, InternalValue, LocalIn, PatchFacts, SymCtx, SymDef, SymtabPlan, block_room,
    file_slack, tail_room,
};
use mortar_synth::symtab_incr::{
    FileSymbols, GlobalChange, GlobalKind, LocalSymbol, area_after_removals, plan,
};

// ---- the full link in incremental mode ------------------------------------

const ROOM: PatchFacts =
    PatchFacts { thunk_size: 12, erratum_843419_size: 8, mapping: true, room: true };

fn local(name: &'static [u8], typ: u8, section: Option<u32>, value: u64) -> LocalIn<'static> {
    let def = match section {
        Some(s) => SymDef::Section { section: SectionId(s), value },
        None => SymDef::Absolute(value),
    };
    LocalIn { name, info: typ, other: 0, size: 4, def, referenced: false }
}

fn global(name: &'static [u8], typ: u8, other: u8, section: u32, file: u32) -> GlobalIn<'static> {
    GlobalIn {
        name,
        typ,
        binding: STB_GLOBAL,
        other,
        size: 8,
        def: SymDef::Section { section: SectionId(section), value: 0x10 },
        file: FileId(file),
        referenced: true,
    }
}

/// Three files: the internal one (a hidden linker symbol, demoted), a.c
/// (four locals) and b.c (one local, one hidden global demoted to it).
fn program() -> (Syms, Secs, Addrs) {
    let syms = Syms {
        files: vec![
            vec![],
            vec![
                local(b"a.c", STT_FILE, None, 0),
                local(b"s1", STT_FUNC, Some(1), 0),
                local(b"s1b", STT_FUNC, Some(1), 8),
                local(b"tl", STT_TLS, Some(900), 8),
            ],
            vec![local(b"s2", STT_OBJECT, Some(3), 0)],
        ],
        globals: vec![
            Some(global(b"main", STT_FUNC, 0, 1, 1)),
            Some(global(b"hid", STT_OBJECT, STV_HIDDEN, 3, 2)),
            Some(GlobalIn {
                def: SymDef::Internal,
                ..global(b"_GLOBAL_OFFSET_TABLE_", STT_NOTYPE, STV_HIDDEN, 0, 0)
            }),
            Some(global(b"pub2", STT_OBJECT, 0, 3, 2)),
        ],
    };
    let addrs = Addrs {
        internal: BTreeMap::from([(2, InternalValue::At { shndx: 1, addr: 0x10000 })]),
        high: None,
    };
    (syms, Secs::default(), addrs)
}

fn full(syms: &Syms, secs: &Secs, facts: PatchFacts, patches: &[Patch]) -> SymtabPlan {
    let c = SymCtx { syms, sections: secs, eh: None, discard: Some(Discard::Locals) };
    let diag = collector();
    SymtabPlan::new(&c, Class::Elf64, patches, facts, &Pool::sequential(), &diag).unwrap()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    name: String,
    info: u8,
    other: u8,
    shndx: u16,
    value: u64,
}

fn written(p: &SymtabPlan, syms: &Syms, secs: &Secs, addrs: &Addrs) -> (Vec<Entry>, Vec<u8>) {
    let c = SymCtx { syms, sections: secs, eh: None, discard: Some(Discard::Locals) };
    let mut sym = vec![0xaa; p.symtab_size() as usize];
    let mut str = vec![0xaa; p.strtab_size() as usize];
    p.write_symtab(&c, addrs, &mut Window::new(0, &mut sym)).unwrap();
    p.write_strtab(&c, &mut Window::new(0, &mut str)).unwrap();
    let es = sym
        .chunks(24)
        .map(|e| {
            let at = u32::from_le_bytes(e[0..4].try_into().unwrap()) as usize;
            let end = at + str[at..].iter().position(|&b| b == 0).unwrap();
            Entry {
                name: String::from_utf8_lossy(&str[at..end]).into_owned(),
                info: e[4],
                other: e[5],
                shndx: u16::from_le_bytes(e[6..8].try_into().unwrap()),
                value: u64::from_le_bytes(e[8..16].try_into().unwrap()),
            }
        })
        .collect();
    (es, str)
}

fn is_filler(e: &Entry) -> bool {
    e.name.is_empty() && e.info == STT_SECTION && e.shndx == 1
}

// spec: incr.md IN-49a (the filler's form), plumb.md §6.9 (P-E3)
#[test]
fn filler_value_is_section_1_addr() {
    let (syms, secs, addrs) = program();
    let p = full(&syms, &secs, ROOM, &[]);
    let (es, _) = written(&p, &syms, &secs, &addrs);
    let blocks = p.incr_blocks().unwrap();
    let tables = p.incr_tables().unwrap();
    // Every index of a block's unused capacity and of the tail reserve.
    let mut fill: BTreeSet<u32> = BTreeSet::new();
    for (_, b) in &blocks {
        fill.extend(b.first + b.count..b.first + b.capacity);
    }
    fill.extend(tables.tail_first..tables.tail_first + tables.tail_capacity);
    assert!(fill.len() > 64, "fillers: {}", fill.len());
    for (i, e) in es.iter().enumerate() {
        if fill.contains(&(i as u32)) {
            // st_name 0, STB_LOCAL|STT_SECTION, st_other 0, st_shndx 1,
            // st_value section 1's sh_addr (common::Addrs: 0x10000).
            assert_eq!(
                *e,
                Entry {
                    name: String::new(),
                    info: STB_LOCAL << 4 | STT_SECTION,
                    other: 0,
                    shndx: 1,
                    value: 0x10000
                },
                "entry {i}"
            );
        } else if i > 0 {
            assert!(!is_filler(e), "entry {i} {e:?} is not in a filler range");
        }
    }
    // Fillers are locals: all before sh_info.
    assert!(fill.iter().all(|&i| i < p.sh_info()));
}

// spec: incr.md IN-49a (capacities, the tail reserve, the regions)
#[test]
fn in49a_blocks_capacities_and_regions() {
    let (syms, secs, addrs) = program();
    let plain = full(&syms, &secs, PatchFacts { room: false, ..ROOM }, &[]);
    let p = full(&syms, &secs, ROOM, &[]);
    assert_eq!(plain.incr_blocks(), None);
    assert_eq!(plain.incr_tables(), None);
    let blocks = p.incr_blocks().unwrap();
    let files: Vec<FileId> = blocks.iter().map(|b| b.0).collect();
    assert_eq!(files, [FileId(0), FileId(1), FileId(2)]);
    // The internal block holds `_GLOBAL_OFFSET_TABLE_`; a.c's its four
    // locals; b.c's `s2` and the demoted `hid`.
    let counts: Vec<u32> = blocks.iter().map(|b| b.1.count).collect();
    assert_eq!(counts, [1, 4, 2]);
    let mut next = 1;
    for (f, b) in &blocks {
        assert_eq!(b.first, next, "file {f:?}: blocks are contiguous from entry 1");
        assert_eq!(b.capacity, b.count + block_room(b.count, 0).0, "file {f:?}");
        assert_eq!(b.capacity - b.count, 2, "max(2, count/8)");
        assert_eq!(b.strtab_capacity, b.strtab_len + block_room(b.count, b.strtab_len).1);
        next = b.first + b.capacity;
        // S-88 in incremental mode: the span is the block's capacity.
        let span = p.symtab_locals()[*f];
        assert_eq!((span.start, span.len), (b.first, b.capacity), "file {f:?}");
    }
    let tables = p.incr_tables().unwrap();
    assert_eq!(tables.tail_first, next);
    assert_eq!(tables.tail_capacity, tail_room(7));
    assert_eq!(tables.tail_capacity, 64);
    assert_eq!(tables.sh_info, next + 64);
    assert_eq!(tables.sh_info, p.sh_info());
    assert_eq!(tables.symtab_size, p.symtab_size());
    assert_eq!(tables.symtab_slack, file_slack(p.symtab_size()));
    assert_eq!(tables.strtab_slack, 4096);
    let area = p.incr_global_area().unwrap();
    assert_eq!((area.first, area.count), (tables.sh_info, 2), "main, pub2");
    // The regions follow each other; the global area's names start where
    // the last region's slack ends.
    let mut at = blocks[0].1.strtab_offset;
    for (_, b) in &blocks {
        assert_eq!(b.strtab_offset, at);
        at += b.strtab_capacity;
    }
    assert_eq!(area.strtab_offset, at);
    assert_eq!(area.strtab_offset + area.strtab_len, tables.strtab_size);
    // The same entries as the plain link's, in the same order, fillers
    // aside.
    let (plain_es, _) = written(&plain, &syms, &secs, &addrs);
    let (es, str) = written(&p, &syms, &secs, &addrs);
    let kept: Vec<&Entry> =
        es.iter().enumerate().filter(|(i, e)| *i == 0 || !is_filler(e)).map(|x| x.1).collect();
    assert_eq!(kept, plain_es.iter().collect::<Vec<_>>());
    // A region's slack is zeroes.
    for (_, b) in &blocks {
        let s = (b.strtab_offset + b.strtab_len) as usize;
        let e = (b.strtab_offset + b.strtab_capacity) as usize;
        assert!(str[s..e].iter().all(|&x| x == 0));
    }
}

// spec: incr.md IN-49a; synth.md S-80 (`with_patches` in incremental mode)
#[test]
fn room_with_patches_equals_a_new_plan() {
    let main = SymKey::global(SymbolId(0));
    let thunk = |offset| Patch {
        chunk: ChunkId(9),
        offset,
        kind: PatchKind::Thunk(ThunkTarget::Sym { key: main, addend: 0 }),
    };
    let (syms, sections, _) = program();
    let sets: [Vec<Patch>; 4] =
        [vec![], vec![thunk(0), thunk(16)], (0..40).map(|k| thunk(16 * k)).collect(), vec![]];
    let mut p = full(&syms, &sections, ROOM, &[]);
    for patches in &sets {
        p = p.with_patches(&syms, patches);
        assert_eq!(p, full(&syms, &sections, ROOM, patches), "{} patches", patches.len());
    }
}

// spec: incr.md IN-49a; synth.md S-74 (a demoted global is a local of its
// definer's block); WP-207's N2 row
#[test]
fn n2_visibility_demotes_unchanged_definer() {
    // The full link: `hid` (b.c's, hidden) is in b.c's block.
    let (syms, secs, addrs) = program();
    let p = full(&syms, &secs, ROOM, &[]);
    let (es, _) = written(&p, &syms, &secs, &addrs);
    let b = p.incr_blocks().unwrap()[2].1;
    let names: Vec<&str> =
        (b.first..b.first + b.count).map(|i| es[i as usize].name.as_str()).collect();
    assert_eq!(names, ["s2", "hid"]);
    // The update: c.o declares `pub2` hidden, so `pub2` (unchanged b.c's)
    // leaves the global area for b.c's block. Its stored index (the area
    // entry to take back) reaches `plan` through neither the state's rows
    // nor `GlobalChange` (WP-217's seam finding): refused by name.
    let rows = layout();
    let stored = StoredLayout::new(&rows);
    let pub2 = GlobalChange {
        key: SymbolId(3),
        kind: GlobalKind::Demoted,
        name: b"pub2",
        info: STB_LOCAL << 4 | STT_OBJECT,
        other: STV_HIDDEN,
        section: Some(SectionId(3)),
        value: 0x10,
        size: 8,
    };
    let b_c = FileSymbols {
        file: FileId(2),
        locals: vec![lsym(b"s2", STT_OBJECT, Some(3), 0), lsym(b"pub2", STT_OBJECT, Some(3), 0x10)],
    };
    let d = delta(&rows, &[2]);
    assert_eq!(plan(&stored, &d, &[b_c], &[pub2]), Err(not_built()));
}

// ---- an update's plan ------------------------------------------------------

fn not_built() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Symtab))
}

fn block(first: u32, count: u32, capacity: u32, str_at: u64, len: u64, cap: u64) -> SymtabBlock {
    SymtabBlock {
        first,
        count,
        capacity,
        strtab_offset: str_at,
        strtab_len: len,
        strtab_capacity: cap,
    }
}

fn chunk(out: u32, flags: u64, addr: Option<u64>, shndx: u32) -> Chunk {
    Chunk {
        kind: ChunkKind::Body(Part::first(OutSecId(out))),
        sh_type: SHT_PROGBITS,
        flags,
        align: 4,
        size: 0x1000,
        capacity: 0x1000,
        addr,
        offset: 0x1000 * u64::from(shndx),
        shndx: Some(shndx),
        relro: false,
    }
}

fn section(id: u32, file: u32, place: Place) -> StoredSection {
    StoredSection {
        id: SectionId(id),
        file: FileId(file),
        index: id + 1,
        anchor_kind: 0,
        anchor: vec![],
        name: b".text".to_vec(),
        ordinal: 0,
        sh_type: SHT_PROGBITS,
        sh_flags: SHF_ALLOC,
        size: 0x100,
        align: 4,
        place,
    }
}

const SH_INFO: u32 = 78;
const AREA_END: u32 = 84;

/// A stored incremental-mode layout: `.text` (chunk 0, `.symtab` section
/// 1), `.tdata` (chunk 1, the TLS template), `.comment` (chunk 2, not
/// allocated). File 1: sections 0 (`.text`), 1 (`.tdata`), 2
/// (`.comment`); file 2: sections 3 (`.text`) and 4 (dead). Blocks: the
/// internal one at 1, file 1's at 4 (4 of 6), file 2's at 10 (2 of 4);
/// the tail reserve 14..78; the global area 78..84.
fn layout() -> MemoryLayout {
    let c0 = ChunkId(0);
    MemoryLayout {
        chunks: vec![
            chunk(0, SHF_ALLOC | SHF_EXECINSTR, Some(0x40_0000), 1),
            chunk(1, SHF_ALLOC | SHF_WRITE | SHF_TLS, Some(0x50_0000), 2),
            chunk(2, 0, None, 3),
        ],
        sections: vec![
            section(0, 1, Place::at(c0, 0x100)),
            section(1, 1, Place::at(ChunkId(1), 0x10)),
            section(2, 1, Place::at(ChunkId(2), 0)),
            section(3, 2, Place::at(c0, 0x800)),
            section(4, 2, Place::NONE),
        ],
        tls: Some(Tls { start: 0x50_0000, size: 0x100, align: 3 }),
        symtab_blocks: vec![
            (FileId(0), block(1, 1, 3, 1, 15, 31)),
            (FileId(1), block(4, 4, 6, 32, 20, 36)),
            (FileId(2), block(10, 2, 4, 68, 8, 24)),
        ],
        global_area: block(SH_INFO, AREA_END - SH_INFO, AREA_END - SH_INFO, 92, 108, 108),
        symtab_tables: SymtabTables {
            sh_info: SH_INFO,
            tail_first: 14,
            tail_capacity: 64,
            symtab_size: u64::from(AREA_END) * 24,
            symtab_slack: 4096,
            strtab_size: 200,
            strtab_slack: 4096,
        },
        ..MemoryLayout::default()
    }
}

/// The update's placement: the stored chunks, the sections of `files`
/// where the stored layout had them (section 4 still nowhere).
fn delta(rows: &MemoryLayout, files: &[u32]) -> LayoutDelta {
    let mut place = vec![Place::NONE; rows.sections.len()];
    for s in &rows.sections {
        if files.contains(&s.file.0) {
            place[s.id.0 as usize] = s.place;
        }
    }
    LayoutDelta {
        placement: Placement {
            chunks: Column::from_vec(rows.chunks.clone()).unwrap(),
            place: Column::from_vec(place).unwrap(),
            tls: rows.tls,
            ..Placement::default()
        },
        files: files.iter().map(|&f| FileId(f)).collect(),
        ..LayoutDelta::default()
    }
}

fn lsym(name: &'static [u8], typ: u8, section: Option<u32>, value: u64) -> LocalSymbol<'static> {
    LocalSymbol {
        name,
        info: STB_LOCAL << 4 | typ,
        other: 0,
        section: section.map(SectionId),
        value,
        size: 4,
    }
}

fn added(key: u32, name: &'static [u8], section: u32) -> GlobalChange<'static> {
    GlobalChange {
        key: SymbolId(key),
        kind: GlobalKind::Added,
        name,
        info: STB_GLOBAL << 4 | STT_FUNC,
        other: 0,
        section: Some(SectionId(section)),
        value: 0x20,
        size: 8,
    }
}

/// `LayoutRows` that counts what is read of it (O(change), IN-49a "what
/// an update reads and writes").
struct Counting {
    rows: MemoryLayout,
    reads: Mutex<BTreeMap<&'static str, u32>>,
    sections: Mutex<Vec<SectionId>>,
}

impl Counting {
    fn new(rows: MemoryLayout) -> Self {
        Self { rows, reads: Mutex::default(), sections: Mutex::default() }
    }
    fn read(&self, what: &'static str) {
        *self.reads.lock().unwrap().entry(what).or_default() += 1;
    }
    fn reads(&self) -> BTreeMap<&'static str, u32> {
        self.reads.lock().unwrap().clone()
    }
    /// The plan read the tables once, one `SYMTAB` row a file of the
    /// update, each section at most once, and nothing else.
    fn assert_o_change(&self, files: usize) {
        let mut want = BTreeMap::from([("symtab_tables", 1)]);
        if files > 0 {
            want.insert("symtab_block", files as u32);
        }
        let mut got = self.reads();
        let sections = got.remove("section").unwrap_or(0);
        assert_eq!(got, want);
        let distinct: BTreeSet<SectionId> = self.sections.lock().unwrap().iter().copied().collect();
        assert_eq!(sections as usize, distinct.len(), "each section's row read once");
    }
}

impl LayoutRows for Counting {
    fn chunks(&self) -> Result<Vec<(ChunkId, Chunk)>, IncrReason> {
        self.read("chunks");
        self.rows.chunks()
    }
    fn order(&self) -> Result<Vec<ChunkId>, IncrReason> {
        self.read("order");
        self.rows.order()
    }
    fn segments(&self) -> Result<Vec<Segment>, IncrReason> {
        self.read("segments");
        self.rows.segments()
    }
    fn outsecs(&self) -> Result<Vec<(OutSecId, Vec<u8>)>, IncrReason> {
        self.read("outsecs");
        self.rows.outsecs()
    }
    fn sections_of(&self, file: FileId) -> Result<Vec<StoredSection>, IncrReason> {
        self.read("sections_of");
        self.rows.sections_of(file)
    }
    fn regions_of(&self, file: FileId) -> Result<Vec<StoredRegion>, IncrReason> {
        self.read("regions_of");
        self.rows.regions_of(file)
    }
    fn holes_in(&self, chunk: ChunkId) -> Result<Vec<Hole>, IncrReason> {
        self.read("holes_in");
        self.rows.holes_in(chunk)
    }
    fn frags_of(
        &self,
        out: OutSecId,
        class: u32,
        hash: [u8; 16],
    ) -> Result<Option<StoredFrag>, IncrReason> {
        self.read("frags_of");
        self.rows.frags_of(out, class, hash)
    }
    fn frag_uses_of(&self, file: FileId) -> Result<Vec<StoredFragUse>, IncrReason> {
        self.read("frag_uses_of");
        self.rows.frag_uses_of(file)
    }
    fn eh_range_of(&self, file: FileId) -> Result<Option<StoredEhRange>, IncrReason> {
        self.read("eh_range_of");
        self.rows.eh_range_of(file)
    }
    fn key_refs_of(&self, file: FileId) -> Result<Vec<StoredKeyRef>, IncrReason> {
        self.read("key_refs_of");
        self.rows.key_refs_of(file)
    }
    fn section(&self, id: SectionId) -> Result<Option<StoredSection>, IncrReason> {
        self.read("section");
        self.sections.lock().unwrap().push(id);
        self.rows.section(id)
    }
    fn slots(&self) -> Result<Vec<StoredSlot>, IncrReason> {
        self.read("slots");
        self.rows.slots()
    }
    fn patches(&self) -> Result<Vec<Patch>, IncrReason> {
        self.read("patches");
        self.rows.patches()
    }
    fn redirects(&self) -> Result<Vec<Redirect>, IncrReason> {
        self.read("redirects");
        self.rows.redirects()
    }
    fn sym_place(&self) -> Result<Vec<SymPlace>, IncrReason> {
        self.read("sym_place");
        self.rows.sym_place()
    }
    fn defined(&self) -> Result<Vec<Defined>, IncrReason> {
        self.read("defined");
        self.rows.defined()
    }
    fn tls_and_entry(&self) -> Result<(Option<Tls>, Option<u64>), IncrReason> {
        self.read("tls_and_entry");
        self.rows.tls_and_entry()
    }
    fn symtab_block(&self, file: FileId) -> Result<Option<SymtabBlock>, IncrReason> {
        self.read("symtab_block");
        self.rows.symtab_block(file)
    }
    fn symtab_tables(&self) -> Result<(SymtabBlock, SymtabTables), IncrReason> {
        self.read("symtab_tables");
        self.rows.symtab_tables()
    }
}

fn entry(index: u32, name: u32, typ: u8, shndx: u32, value: u64) -> SymtabEntry {
    SymtabEntry { index, name, info: STB_LOCAL << 4 | typ, other: 0, shndx, value, size: 4 }
}

// spec: incr.md IN-49a (a block rewritten in place)
#[test]
fn changed_file_rewritten_in_place() {
    let rows = Counting::new(layout());
    let stored = StoredLayout::new(&rows);
    let a_c = FileSymbols {
        file: FileId(1),
        locals: vec![
            lsym(b"a.c", STT_FILE, None, 0),
            lsym(b"s1", STT_FUNC, Some(0), 4),
            lsym(b"tl", STT_TLS, Some(1), 8),
            lsym(b"s1b", STT_FUNC, Some(0), 0x40),
        ],
    };
    let p = plan(&stored, &delta(&rows.rows, &[1]), &[a_c], &[]).unwrap();
    rows.assert_o_change(1);
    assert_eq!(
        p.entries,
        [
            entry(4, 32, STT_FILE, u32::from(SHN_ABS), 0),
            entry(5, 36, STT_FUNC, 1, 0x40_0104),
            // S-81: a TLS symbol's value is its offset in the template.
            entry(6, 39, STT_TLS, 2, 0x18),
            entry(7, 42, STT_FUNC, 1, 0x40_0140),
        ]
    );
    assert_eq!(p.fillers, [(8, 10)]);
    assert_eq!(p.strtab, [(32, b"a.c\0s1\0tl\0s1b\0".to_vec())]);
    assert_eq!(p.strtab_dead, [(46, 22)]);
    assert_eq!(p.blocks, [(FileId(1), block(4, 4, 6, 32, 14, 36))]);
    // Nothing else moved: the tables and the global area are the stored.
    let rows = layout();
    assert_eq!(p.tables, rows.symtab_tables);
    assert_eq!(p.global_area, rows.global_area);
    assert!(p.moves.is_empty() && p.remap.is_empty() && p.globals.is_empty());
}

// spec: incr.md IN-49a (a grown block moves to the tail reserve; a grown
// region to `.strtab`'s end)
#[test]
fn grown_block_and_region_move() {
    let rows = Counting::new(layout());
    let stored = StoredLayout::new(&rows);
    let seven: Vec<LocalSymbol<'static>> = [b"l0", b"l1", b"l2", b"l3", b"l4", b"l5", b"l6"]
        .iter()
        .map(|n| lsym(*n, STT_FUNC, Some(0), 0))
        .collect();
    let a_c = FileSymbols { file: FileId(1), locals: seven };
    let long = b"a_rather_long_name_19";
    let b_c = FileSymbols {
        file: FileId(2),
        locals: vec![lsym(long, STT_FUNC, Some(3), 0), lsym(long, STT_FUNC, Some(3), 4)],
    };
    let files = [b_c.clone(), a_c.clone()];
    let d = delta(&rows.rows, &[1, 2]);
    let p = plan(&stored, &d, &files, &[]).unwrap();
    rows.assert_o_change(2);
    // The input's order does not matter.
    assert_eq!(p, plan(&stored, &d, &[a_c, b_c], &[]).unwrap());
    // a.c: 7 entries > 6: to the tail reserve with 7 + 2, its old place
    // fillers. Its names (21 bytes) still fit its region (36).
    // b.c: 2 entries fit in place; its names (44 bytes) do not fit its
    // region (24): a new one at 200 with max(16, 44/8) slack.
    assert_eq!(
        p.blocks,
        [(FileId(1), block(14, 7, 9, 32, 21, 36)), (FileId(2), block(10, 2, 4, 200, 44, 60))]
    );
    assert_eq!(p.fillers, [(4, 10), (12, 14), (21, 23)]);
    let idx: Vec<u32> = p.entries.iter().map(|e| e.index).collect();
    assert_eq!(idx, [10, 11, 14, 15, 16, 17, 18, 19, 20]);
    assert_eq!(p.entries[0].name, 200);
    assert_eq!(p.entries[1].name, 222);
    assert_eq!(p.strtab_dead, [(53, 15), (68, 24), (244, 16)]);
    assert_eq!(
        p.tables,
        SymtabTables {
            tail_first: 23,
            tail_capacity: 55,
            strtab_size: 260,
            strtab_slack: 4036,
            ..layout().symtab_tables
        }
    );
    assert_eq!(p.global_area.strtab_len, 260 - 92);
}

// spec: incr.md IN-49a (`symtab-exhausted`: the tail reserve; what is
// left of it when the entries still fit)
#[test]
fn tail_reserve_exhausted_by_name() {
    let seven: Vec<LocalSymbol<'static>> =
        (0..7).map(|_| lsym(b"x", STT_FUNC, Some(0), 0)).collect();
    let a_c = FileSymbols { file: FileId(1), locals: seven };
    for (cap, want) in [(8, Ok((14, 8, 22, 0))), (6, Err("tail"))] {
        let mut rows = layout();
        rows.symtab_tables.tail_capacity = cap;
        let stored = StoredLayout::new(&rows);
        let got = plan(&stored, &delta(&rows, &[1]), std::slice::from_ref(&a_c), &[]);
        match (got, want) {
            (Ok(p), Ok((first, capacity, tail_first, tail_capacity))) => {
                assert_eq!((p.blocks[0].1.first, p.blocks[0].1.capacity), (first, capacity));
                assert_eq!(
                    (p.tables.tail_first, p.tables.tail_capacity),
                    (tail_first, tail_capacity)
                );
            }
            (Err(IncrReason::SymtabExhausted(what)), Err(_)) => {
                assert!(what.contains("tail reserve"), "{what}");
            }
            (got, _) => panic!("tail capacity {cap}: {got:?}"),
        }
    }
}

// spec: incr.md IN-49a (`symtab-exhausted`: the tables' file slack)
#[test]
fn file_slack_exhausted_by_name() {
    // .strtab: a region that does not fit what is left.
    let mut rows = layout();
    rows.symtab_tables.strtab_slack = 40;
    let stored = StoredLayout::new(&rows);
    let long = b"a_rather_long_name_19";
    let b_c = FileSymbols {
        file: FileId(2),
        locals: vec![lsym(long, STT_FUNC, Some(3), 0), lsym(long, STT_FUNC, Some(3), 4)],
    };
    match plan(&stored, &delta(&rows, &[2]), &[b_c], &[]) {
        Err(IncrReason::SymtabExhausted(what)) => assert!(what.contains(".strtab"), "{what}"),
        got => panic!("{got:?}"),
    }
    // .symtab: one appended global fits the slack, two do not.
    let mut rows = layout();
    rows.symtab_tables.symtab_slack = 24;
    let stored = StoredLayout::new(&rows);
    let d = delta(&rows, &[]);
    let p = plan(&stored, &d, &[], &[added(9, b"g9", 3)]).unwrap();
    assert_eq!((p.tables.symtab_size, p.tables.symtab_slack), (u64::from(AREA_END + 1) * 24, 0));
    match plan(&stored, &d, &[], &[added(9, b"g9", 3), added(10, b"g10", 3)]) {
        Err(IncrReason::SymtabExhausted(what)) => assert!(what.contains(".symtab"), "{what}"),
        got => panic!("{got:?}"),
    }
}

// spec: incr.md IN-49a (new globals appended at the area's end, in key
// order; names at `.strtab`'s end; their `SYMTAB_KEYS` rows)
#[test]
fn added_globals_appended_in_key_order() {
    let rows = Counting::new(layout());
    let stored = StoredLayout::new(&rows);
    let globals = [added(9, b"zz", 0), added(3, b"aa", 3)];
    let p = plan(&stored, &delta(&rows.rows, &[]), &[], &globals).unwrap();
    rows.assert_o_change(0);
    let g = |index, name, value| SymtabEntry {
        index,
        name,
        info: STB_GLOBAL << 4 | STT_FUNC,
        other: 0,
        shndx: 1,
        value,
        size: 8,
    };
    // Section 3 (b.c's, not in the update) and 0 (a.c's): the stored places.
    assert_eq!(p.entries, [g(84, 200, 0x40_0820), g(85, 203, 0x40_0120)]);
    assert_eq!(p.strtab, [(200, b"aa\0zz\0".to_vec())]);
    assert_eq!(p.globals, [(SymbolId(3), 84, 200), (SymbolId(9), 85, 203)]);
    assert_eq!(p.global_area, block(SH_INFO, 8, 8, 92, 114, 114));
    assert_eq!(p.tables.symtab_size, 86 * 24);
    assert_eq!(p.tables.symtab_slack, 4096 - 48);
    assert_eq!((p.tables.strtab_size, p.tables.strtab_slack), (206, 4090));
    assert!(p.blocks.is_empty() && p.fillers.is_empty() && p.strtab_dead.is_empty());
}

// spec: incr.md IN-49a (a growth step: the entries of files the update
// does not read are remapped)
#[test]
fn growth_step_remaps_the_rest() {
    let rows = layout();
    let stored = StoredLayout::new(&rows);
    let mut d = delta(&rows, &[1]);
    d.growth_steps = vec![GrowthStep { out: OutSecId(0), step: 0x1000 }];
    let a_c = FileSymbols { file: FileId(1), locals: vec![lsym(b"s1", STT_FUNC, Some(0), 0)] };
    let p = plan(&stored, &d, std::slice::from_ref(&a_c), &[added(9, b"g", 3)]).unwrap();
    // File 1's block (4..10) and the appended global (84) are written.
    assert_eq!(p.remap, [(1, 4), (10, 84)]);
    // No growth step: nothing remapped.
    let p = plan(&stored, &delta(&rows, &[1]), &[a_c], &[]).unwrap();
    assert!(p.remap.is_empty());
}

// spec: incr.md IN-72 item 2a, IN-49a (a drawn-in file's block is
// rewritten; a section the GC revives brings its locals)
#[test]
fn gc_revived_section_has_its_locals() {
    let rows = Counting::new(layout());
    let stored = StoredLayout::new(&rows);
    // b.c is drawn in: its section 4, dead in the previous link, is kept
    // now (the delta's `revived`), placed at .text + 0xc00.
    let mut d = delta(&rows.rows, &[2]);
    let mut place: Vec<Place> = (0..5).map(|i| d.placement.place[SectionId(i)]).collect();
    place[4] = Place::at(ChunkId(0), 0xc00);
    d.placement.place = Column::from_vec(place).unwrap();
    d.revived = vec![SectionId(4)];
    let b_c = FileSymbols {
        file: FileId(2),
        locals: vec![lsym(b"s2", STT_OBJECT, Some(3), 0), lsym(b"rev", STT_FUNC, Some(4), 8)],
    };
    let p = plan(&stored, &d, std::slice::from_ref(&b_c), &[]).unwrap();
    rows.assert_o_change(1);
    assert_eq!(
        p.entries,
        [entry(10, 68, STT_OBJECT, 1, 0x40_0800), entry(11, 71, STT_FUNC, 1, 0x40_0c08)]
    );
    assert_eq!(p.fillers, [(12, 14)]);
    // The section not revived: its local is not plannable (the join drops
    // a dead section's locals, S-74; one given anyway is refused by name).
    let rows = layout();
    let stored = StoredLayout::new(&rows);
    assert_eq!(plan(&stored, &delta(&rows, &[2]), &[b_c], &[]), Err(not_built()));
}

// spec: incr.md IN-49a (a removed global's entry is taken by the area's
// last; review-in77 M-6: the moved entry's `SYMTAB_KEYS` row)
#[test]
fn global_removed_twice_last_moved() {
    // G1 at 79 removed: the last entry (83) takes 79.
    assert_eq!(area_after_removals(SH_INFO, AREA_END, &[79]), (83, vec![(83, 79)]));
    // The next update removes the moved global (now at 79): the new last
    // entry (82) takes it, read from the previous output's 82.
    assert_eq!(area_after_removals(SH_INFO, 83, &[79]), (82, vec![(82, 79)]));
    // Two in one update: applied from the highest index down, so a move
    // never reads an index an earlier move emptied.
    assert_eq!(area_after_removals(SH_INFO, AREA_END, &[79, 82]), (82, vec![(83, 79)]));
    assert_eq!(area_after_removals(SH_INFO, AREA_END, &[83, 79]), (82, vec![(82, 79)]));
    assert_eq!(area_after_removals(SH_INFO, AREA_END, &[83]), (83, vec![]));
    assert_eq!(
        area_after_removals(SH_INFO, AREA_END, &[78, 79, 80]),
        (81, vec![(81, 78), (82, 79), (83, 80)])
    );
    // `plan` refuses a removal by name until the seam carries the removed
    // global's stored index (WP-217's seam finding).
    let rows = layout();
    let stored = StoredLayout::new(&rows);
    let gone = GlobalChange { kind: GlobalKind::Removed, ..added(5, b"g1", 3) };
    assert_eq!(plan(&stored, &delta(&rows, &[]), &[], &[gone]), Err(not_built()));
    let changed = GlobalChange { kind: GlobalKind::Changed, ..added(5, b"g1", 3) };
    assert_eq!(plan(&stored, &delta(&rows, &[]), &[], &[changed]), Err(not_built()));
}

// spec: incr.md IN-49a (what v1 does not plan: refused by name)
#[test]
fn unplannable_symbols_refused_by_name() {
    let rows = layout();
    let stored = StoredLayout::new(&rows);
    let d = delta(&rows, &[1]);
    // A local in a non-allocated section (`.comment`, chunk 2).
    let a_c = FileSymbols { file: FileId(1), locals: vec![lsym(b"c", STT_OBJECT, Some(2), 0)] };
    assert_eq!(plan(&stored, &d, &[a_c], &[]), Err(not_built()));
    // A global with no section (absolute, undefined, common untold).
    let abs = GlobalChange { section: None, ..added(9, b"abs", 0) };
    assert_eq!(plan(&stored, &d, &[], &[abs]), Err(not_built()));
    // A file with no entry and no block has none.
    let none = FileSymbols { file: FileId(3), locals: vec![] };
    let p = plan(&stored, &d, &[none], &[]).unwrap();
    assert_eq!(
        p,
        UpdatePlan {
            global_area: rows.global_area,
            tables: rows.symtab_tables,
            ..UpdatePlan::default()
        }
    );
}

// spec: incr.md IN-49a (stored rows that contradict each other)
#[test]
fn contradicting_tables_are_state_corrupt() {
    let mut rows = layout();
    rows.symtab_tables.sh_info = SH_INFO + 1;
    let stored = StoredLayout::new(&rows);
    assert!(matches!(
        plan(&stored, &delta(&rows, &[]), &[], &[]),
        Err(IncrReason::StateCorrupt(_))
    ));
}
