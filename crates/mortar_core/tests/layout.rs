//! `spec/model.md` §4: `Layout::check` accepts a hand-built valid layout and
//! rejects one mutation per rule; the addresses of §4.5.

use mortar_core::assignment::{Assignment, MergeClass, OutOf, OutSec};
use mortar_core::check::{CheckContext, Row, Rule, Shape, Violation};
use mortar_core::column::Column;
use mortar_core::demands::{DemandFlags, Demands};
use mortar_core::id::{
    ChunkId, FileId, FragId, OptId, OutSecId, SectionId, SegmentId, Span, SymEntry, SymKey,
    SymbolId,
};
use mortar_core::layout::{
    Addresses, AllocKey, Chunk, ChunkKind, DefValue, Defined, EntryDef, Folds, Layout, MergeMap,
    Patch, PatchKind, Place, Placement, Redirect, Segment, Sized, Slots, SymPlace, ThunkTarget,
    Tls,
};
use mortar_core::liveness::Liveness;
use mortar_core::resolution::{
    CommonShape, Def, DropReason, FileLive, Resolution, SectionKind, SectionRow, SectionState,
};
use mortar_core::split::{Frag, Split, SplitRow};
use mortar_core::{Id, IdRange};
use mortar_diag::{Context, Kind};
use mortar_elf::consts::{
    PF_R, PF_W, PF_X, PT_LOAD, PT_TLS, SHF_ALLOC, SHF_EXECINSTR, SHF_MERGE, SHF_STRINGS, SHF_TLS,
    SHF_WRITE, SHT_NOBITS, SHT_PROGBITS,
};
use mortar_elf::{Class, SectionIndex, SymbolIndex};

const A: u64 = SHF_ALLOC;
const AX: u64 = SHF_ALLOC | SHF_EXECINSTR;
const WA: u64 = SHF_ALLOC | SHF_WRITE;
const WAT: u64 = SHF_ALLOC | SHF_WRITE | SHF_TLS;

/// Two objects linked into an ELF64 executable:
///
/// - `a.o` (file 1, sections 0-5): null, `.text` (0x40, 16), `.data`
///   (0x10, 8), `.tdata` (8, 8), `.rodata.str` (two strings), `.symtab`;
/// - `b.o` (file 2, sections 6-9): null, `.text` (0x18, 16), `.tbss`
///   (0x10, 8), `.bss` (0x18, 8);
/// - symbols: `main` (a.o), `_end` (linker-defined), `buf` (common, 0x20),
///   `far` (b.o), the target of a range-extension thunk from b.o's `.text`;
/// - chunks: file header, program headers, `.text` body and its patch
///   chunk, merged `.rodata`, `.tdata`, `.tbss`, `.data`, `.bss`, section
///   headers;
/// - segments: an RX `PT_LOAD`, an RW `PT_LOAD`, `PT_TLS`.
struct World {
    resolution: Resolution,
    assignment: Assignment<'static>,
    split: Split,
    liveness: Liveness,
    demands: Demands,
    shapes: Column<SectionId, Shape>,
    layout: Layout,
    class: Class,
    needs_entry: bool,
}

fn section(kind: SectionKind, kept: bool) -> SectionRow {
    let state = if kept { SectionState::Kept } else { SectionState::Dropped(DropReason::Consumed) };
    SectionRow { kind, state, reloc: SectionIndex(0), comdat: OptId::NONE }
}

fn outsec(name: &'static [u8], sh_type: u32, flags: u64) -> OutSec<'static> {
    OutSec {
        name,
        sh_type,
        flags,
        merge: None,
        keep: false,
        fill: None,
        constraint: None,
        script_position: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn chunk(
    kind: ChunkKind,
    ty: u32,
    flags: u64,
    align: u8,
    size: u64,
    addr: Option<u64>,
    offset: u64,
    shndx: Option<u32>,
) -> Chunk {
    Chunk {
        kind,
        sh_type: ty,
        flags,
        align,
        size,
        capacity: size,
        addr,
        offset,
        shndx,
        relro: false,
    }
}

fn col<I: Id, T>(rows: Vec<T>) -> Column<I, T> {
    Column::from_vec(rows).unwrap()
}

impl World {
    #[allow(clippy::too_many_lines)]
    fn new() -> Self {
        use SectionKind::{Merge, Nobits, Null, Regular, Symtab};
        let mut resolution = Resolution::empty(3, 4, 0);
        resolution.file_live = col(vec![FileLive::Always; 3]);
        resolution.section_base = col(vec![
            IdRange::empty(SectionId(0)),
            IdRange::new(SectionId(0), SectionId(6)),
            IdRange::new(SectionId(6), SectionId(10)),
        ]);
        resolution.sections = col(vec![
            section(Null, false),
            section(Regular, true),
            section(Regular, true),
            section(Regular, true),
            section(Merge, true),
            section(Symtab, false),
            section(Null, false),
            section(Regular, true),
            section(Nobits, true),
            section(Nobits, true),
        ]);
        let entry = |file, index| SymEntry { file: FileId(file), index: SymbolIndex(index) };
        resolution.def = col(vec![
            Def::Defined(entry(1, 1)),
            Def::Internal,
            Def::Common(entry(2, 5)),
            Def::Defined(entry(2, 6)),
        ]);
        resolution.common = Box::new([(SymbolId(2), CommonShape { size: 0x20, align: 3 })]);

        let mut assignment = Assignment::empty(10);
        assignment.outsecs = col(vec![
            outsec(b".text", SHT_PROGBITS, AX),
            OutSec {
                merge: Some(MergeClass { strings: true, entsize: 1 }),
                ..outsec(b".rodata", SHT_PROGBITS, A | SHF_MERGE | SHF_STRINGS)
            },
            outsec(b".tdata", SHT_PROGBITS, WAT),
            outsec(b".tbss", SHT_NOBITS, WAT),
            outsec(b".data", SHT_PROGBITS, WA),
            outsec(b".bss", SHT_NOBITS, WA),
        ]);
        for (s, o) in [(1, 0), (2, 4), (3, 2), (4, 1), (7, 0), (8, 3), (9, 5)] {
            assignment.out_of[SectionId(s)] = OutOf::Out(OutSecId(o));
        }

        let split = Split {
            merge_of: Box::new([SplitRow {
                section: SectionId(4),
                pieces: Span { start: 0, len: 2 },
            }]),
            piece_off: Box::new([0, 3]),
            piece_frag: Box::new([FragId(0), FragId(1)]),
            frags: col(vec![
                Frag { section: SectionId(4), offset: 0, size: 3, align: 0 },
                Frag { section: SectionId(4), offset: 3, size: 3, align: 0 },
            ]),
            ..Split::default()
        };

        let mut liveness = Liveness::empty(10, 2);
        for s in [1, 2, 3, 4, 7, 8, 9] {
            liveness.live[SectionId(s)] = true;
        }
        liveness.frag_live = col(vec![true, true]);

        let shape = |size, align| Shape { size, align };
        let shapes = col(vec![
            shape(0, 0),
            shape(0x40, 4),
            shape(0x10, 3),
            shape(8, 3),
            shape(6, 0),
            shape(0x48, 3),
            shape(0, 0),
            shape(0x18, 4),
            shape(0x10, 3),
            shape(0x18, 3),
        ]);

        let chunks = col(vec![
            chunk(ChunkKind::FileHeader, SHT_PROGBITS, A, 3, 0x40, Some(0x40_0000), 0, None),
            chunk(ChunkKind::ProgramHeaders, SHT_PROGBITS, A, 3, 0xa8, Some(0x40_0040), 0x40, None),
            chunk(
                ChunkKind::Body(OutSecId(0)),
                SHT_PROGBITS,
                AX,
                4,
                0x60,
                Some(0x40_1000),
                0x1000,
                Some(1),
            ),
            chunk(
                ChunkKind::Patches(OutSecId(0)),
                SHT_PROGBITS,
                AX,
                2,
                0x10,
                Some(0x40_1060),
                0x1060,
                Some(1),
            ),
            chunk(
                ChunkKind::Merged(OutSecId(1)),
                SHT_PROGBITS,
                A | SHF_MERGE | SHF_STRINGS,
                0,
                6,
                Some(0x40_2000),
                0x2000,
                Some(2),
            ),
            chunk(
                ChunkKind::Body(OutSecId(2)),
                SHT_PROGBITS,
                WAT,
                3,
                8,
                Some(0x40_3000),
                0x3000,
                Some(3),
            ),
            chunk(
                ChunkKind::Body(OutSecId(3)),
                SHT_NOBITS,
                WAT,
                3,
                0x10,
                Some(0x40_3008),
                0x3008,
                Some(4),
            ),
            chunk(
                ChunkKind::Body(OutSecId(4)),
                SHT_PROGBITS,
                WA,
                3,
                0x10,
                Some(0x40_3008),
                0x3008,
                Some(5),
            ),
            chunk(
                ChunkKind::Body(OutSecId(5)),
                SHT_NOBITS,
                WA,
                3,
                0x38,
                Some(0x40_3018),
                0x3018,
                Some(6),
            ),
            chunk(ChunkKind::SectionHeaders, SHT_PROGBITS, 0, 3, 0x1c0, None, 0x3018, None),
        ]);
        let segment = |p_type, flags, offset, addr, filesz, memsz, align| Segment {
            p_type,
            flags,
            offset,
            addr,
            filesz,
            memsz,
            align,
        };
        let segments = col(vec![
            segment(PT_LOAD, PF_R | PF_X, 0, 0x40_0000, 0x2006, 0x2006, 12),
            segment(PT_LOAD, PF_R | PF_W, 0x3000, 0x40_3000, 0x18, 0x50, 12),
            segment(PT_TLS, PF_R, 0x3000, 0x40_3000, 8, 0x18, 3),
        ]);
        let mut place = Column::filled(10, Place::NONE);
        for (s, c, off) in [(1, 2, 0), (7, 2, 0x40), (2, 7, 0), (3, 5, 0), (8, 6, 0), (9, 8, 0)] {
            place[SectionId(s)] = Place::at(ChunkId(c), off);
        }
        let placement = Placement {
            chunks,
            order: (0..10).map(ChunkId).collect(),
            segments,
            place,
            sym_place: Box::new([SymPlace {
                key: AllocKey::Common(SymbolId(2)),
                chunk: ChunkId(8),
                offset: 0x18,
                size: 0x20,
                align: 3,
            }]),
            defined: Box::new([Defined {
                symbol: SymbolId(1),
                value: DefValue::At { chunk: ChunkId(8), offset: 0x38 },
            }]),
            patches: Box::new([Patch {
                chunk: ChunkId(3),
                offset: 0,
                kind: PatchKind::Thunk(ThunkTarget::Sym { key: SymbolId(3).into(), addend: 0 }),
            }]),
            redirects: Box::new([Redirect { section: SectionId(7), offset: 4, patch: 0 }]),
            sized: Box::new([Sized { chunk: ChunkId(3), size: 0x10 }]),
            tls: Some(Tls { start: 0x40_3000, size: 0x18, align: 3 }),
            entry: Some(0x40_1000),
        };
        let slots = Slots {
            symtab_locals: col(vec![
                Span { start: 1, len: 0 },
                Span { start: 1, len: 2 },
                Span { start: 3, len: 1 },
            ]),
            ..Slots::default()
        };
        let layout = Layout {
            folds: Folds::default(),
            merge: MergeMap { frag_off: col(vec![0, 3]) },
            slots,
            placement,
        };
        Self {
            resolution,
            assignment,
            split,
            liveness,
            demands: Demands::empty(4),
            shapes,
            layout,
            class: Class::Elf64,
            needs_entry: true,
        }
    }

    fn cx(&self) -> CheckContext<'_, 'static> {
        CheckContext {
            class: self.class,
            page_size: 0x1000,
            needs_entry: self.needs_entry,
            resolution: &self.resolution,
            assignment: &self.assignment,
            split: &self.split,
            liveness: &self.liveness,
            demands: &self.demands,
            shapes: &self.shapes,
        }
    }

    fn validate(&self) -> Result<(), Violation> {
        self.layout.validate(&self.cx())
    }

    fn p(&mut self) -> &mut Placement {
        &mut self.layout.placement
    }

    /// The rule the layout breaks, and the message.
    #[track_caller]
    fn rejects(&self, rule: Rule, words: &str) -> Violation {
        let v = self.validate().expect_err("the mutation is rejected");
        assert_eq!(v.rule, rule, "{v:?}");
        assert!(v.what.contains(words), "{v:?} does not say {words:?}");
        v
    }
}

// spec: model.md §4.4 (the valid layout every mutation starts from), §3
#[test]
fn s4_4_valid_layout_is_accepted() {
    let w = World::new();
    w.resolution.check().unwrap();
    w.assignment.check(&w.resolution).unwrap();
    w.split.check(&w.assignment).unwrap();
    w.liveness.check(&w.resolution, &w.split).unwrap();
    w.demands.check(4).unwrap();
    assert_eq!(w.validate(), Ok(()));
    assert_eq!(w.layout.check(&w.cx()), Ok(()));
}

// spec: model.md §4.4 (the report names the rule and the rows)
#[test]
fn s4_4_check_reports_rule_and_rows() {
    let mut w = World::new();
    w.p().place[SectionId(1)] = Place::NONE;
    let diag = w.layout.check(&w.cx()).unwrap_err();
    assert_eq!(diag.id(), "malformed-input");
    let Kind::Malformed { what } = diag.kind() else { panic!("{diag:?}") };
    assert_eq!(what, "invalid layout: §4.4 rule 1: a live section has no place");
    assert_eq!(diag.context(), &[Context::Note("section 1".to_owned())]);
}

// ---- §4.4 rule 1 ------------------------------------------------------------

// spec: model.md §4.4 rule 1 (every live, output-carrying section has a place)
#[test]
fn s4_4_rule1_missing_place() {
    let mut w = World::new();
    w.p().place[SectionId(7)] = Place::NONE;
    let v = w.rejects(Rule::Places, "has no place");
    assert_eq!(v.rows, [Row::Section(SectionId(7))]);
}

// spec: model.md §4.4 rule 1 (exactly one: a section that needs none has none)
#[test]
fn s4_4_rule1_place_for_a_section_without_output() {
    let mut w = World::new();
    w.p().place[SectionId(5)] = Place::at(ChunkId(2), 0);
    w.rejects(Rule::Places, "needs no place");
    let mut w = World::new();
    w.liveness.live[SectionId(2)] = false;
    w.rejects(Rule::Places, "needs no place");
    let mut w = World::new();
    w.p().place[SectionId(4)] = Place::at(ChunkId(4), 0);
    w.rejects(Rule::Places, "needs no place");
}

// spec: model.md §4.4 rule 1 (places inside a chunk do not overlap)
#[test]
fn s4_4_rule1_overlap() {
    let mut w = World::new();
    w.p().place[SectionId(7)] = Place::at(ChunkId(2), 0x30);
    let v = w.rejects(Rule::Places, "overlap");
    assert_eq!(v.rows, [Row::Section(SectionId(1)), Row::Section(SectionId(7))]);
    let mut w = World::new();
    w.p().sym_place[0].offset = 0x10;
    w.rejects(Rule::Places, "overlap");
}

// spec: model.md §4.4 rule 1 (places lie within the chunk's size)
#[test]
fn s4_4_rule1_place_past_size() {
    let mut w = World::new();
    w.p().place[SectionId(7)] = Place::at(ChunkId(2), 0x50);
    w.rejects(Rule::Places, "past its chunk's size");
}

// spec: model.md §4.4 rule 1 (places honor the section's alignment)
#[test]
fn s4_4_rule1_misalignment() {
    let mut w = World::new();
    w.p().place[SectionId(7)] = Place::at(ChunkId(2), 0x44);
    w.rejects(Rule::Places, "misaligned");
    let mut w = World::new();
    w.p().chunks[ChunkId(2)].align = 3;
    w.p().chunks[ChunkId(2)].addr = Some(0x40_1008);
    w.p().chunks[ChunkId(2)].offset = 0x1008;
    w.rejects(Rule::Places, "misaligned");
}

// spec: model.md §4.4 rule 1 (a section is placed in its output section's body)
#[test]
fn s4_4_rule1_wrong_chunk() {
    let mut w = World::new();
    w.p().place[SectionId(2)] = Place::at(ChunkId(2), 0x58);
    w.rejects(Rule::Places, "not in the body of its output section");
}

// ---- §4.4 rule 2 ------------------------------------------------------------

// spec: model.md §4.4 rule 2 (chunks do not overlap in the file)
#[test]
fn s4_4_rule2_file_overlap() {
    let mut w = World::new();
    w.p().chunks[ChunkId(4)].offset = 0x1060;
    w.p().chunks[ChunkId(4)].addr = Some(0x40_2060);
    let v = w.rejects(Rule::Chunks, "overlap in the file");
    assert_eq!(v.rows, [Row::Chunk(ChunkId(3)), Row::Chunk(ChunkId(4))]);
}

// spec: model.md §4.4 rule 2 (chunks do not overlap in memory; .tbss may)
#[test]
fn s4_4_rule2_memory_overlap() {
    let mut w = World::new();
    w.p().chunks[ChunkId(8)].addr = Some(0x40_3010);
    w.rejects(Rule::Chunks, "overlap in memory");
}

// spec: model.md §4.4 rule 2 (addr ≡ offset modulo the page size)
#[test]
fn s4_4_rule2_page_congruence() {
    let mut w = World::new();
    w.p().chunks[ChunkId(4)].offset = 0x2008;
    w.rejects(Rule::Chunks, "modulo the page size");
}

// spec: model.md §4.4 rule 2 (alignments hold)
#[test]
fn s4_4_rule2_chunk_misaligned() {
    let mut w = World::new();
    w.p().chunks[ChunkId(3)].addr = Some(0x40_1061);
    w.p().chunks[ChunkId(3)].offset = 0x1061;
    w.rejects(Rule::Chunks, "address is misaligned");
}

// spec: model.md §4.4 rule 2 (non-alloc chunks have no address)
#[test]
fn s4_4_rule2_non_alloc_address() {
    let mut w = World::new();
    w.p().chunks[ChunkId(9)].addr = Some(0);
    w.rejects(Rule::Chunks, "non-alloc chunk has an address");
}

// spec: model.md §4.4 `order` (every chunk once, in file order)
#[test]
fn s4_4_rule2_order() {
    let mut w = World::new();
    w.p().order = (0..9).map(ChunkId).collect();
    w.rejects(Rule::Chunks, "missing from order");
    let mut w = World::new();
    w.p().order.swap(2, 4);
    w.rejects(Rule::Chunks, "not file order");
}

// spec: model.md §4.4 `chunks` (capacity ≥ size)
#[test]
fn s4_4_rule2_capacity() {
    let mut w = World::new();
    w.p().chunks[ChunkId(3)].capacity = 8;
    w.rejects(Rule::Chunks, "size exceeds capacity");
}

// spec: model.md §4.4 `capacity − size` is slack, which a valid layout may have
#[test]
fn s4_4_slack_is_valid() {
    let mut w = World::new();
    w.p().chunks[ChunkId(3)].capacity = 0x20;
    w.p().chunks[ChunkId(8)].capacity = 0x40;
    w.p().segments[SegmentId(1)].memsz = 0x58;
    assert_eq!(w.validate(), Ok(()));
}

// ---- §4.4 rule 3 ------------------------------------------------------------

// spec: model.md §4.4 rule 3 (a wrong segment: a chunk outside every PT_LOAD)
#[test]
fn s4_4_rule3_wrong_segment() {
    let mut w = World::new();
    w.p().segments[SegmentId(1)].memsz = 0x40;
    let v = w.rejects(Rule::Segments, "not in exactly one PT_LOAD");
    assert_eq!(v.rows, [Row::Chunk(ChunkId(8))]);
}

// spec: model.md §4.4 rule 3 (the segment maps the chunk's bytes)
#[test]
fn s4_4_rule3_file_mapping() {
    let mut w = World::new();
    w.p().segments[SegmentId(1)].filesz = 0x10;
    w.rejects(Rule::Segments, "where its PT_LOAD maps them");
}

// spec: model.md §4.4 rule 3 (segment flags cover the chunks')
#[test]
fn s4_4_rule3_segment_flags() {
    let mut w = World::new();
    w.p().segments[SegmentId(1)].flags = PF_R;
    w.rejects(Rule::Segments, "read-only PT_LOAD");
}

// spec: model.md §4.4 rule 3 (TLS initialized chunks directly followed by the uninitialized ones)
#[test]
fn s4_4_rule3_split_tls() {
    let mut w = World::new();
    w.p().order.swap(6, 7);
    w.rejects(Rule::Segments, "TLS chunks are split");
}

// spec: model.md §4.4 rule 3 (`tls` describes the TLS chunks; PT_TLS matches)
#[test]
fn s4_4_rule3_tls_template() {
    let mut w = World::new();
    w.p().tls.as_mut().unwrap().size = 0x10;
    w.rejects(Rule::Segments, "tls does not describe");
    let mut w = World::new();
    w.p().segments[SegmentId(2)].filesz = 0x18;
    w.rejects(Rule::Segments, "PT_TLS does not match");
    let mut w = World::new();
    w.p().tls = None;
    w.rejects(Rule::Segments, "without a TLS template");
}

// spec: model.md §4.4 rule 3 (the RELRO run is contiguous and ends on a page boundary)
#[test]
fn s4_4_rule3_relro() {
    use mortar_elf::consts::PT_GNU_RELRO;
    let mut w = World::new();
    // .tdata and .tbss are RELRO; PT_GNU_RELRO covers them to the page end.
    w.p().chunks[ChunkId(5)].relro = true;
    w.p().chunks[ChunkId(6)].relro = true;
    let relro = Segment {
        p_type: PT_GNU_RELRO,
        flags: PF_R,
        offset: 0x3000,
        addr: 0x40_3000,
        filesz: 8,
        memsz: 0x1000,
        align: 0,
    };
    w.p().segments.push(relro).unwrap();
    // .data and .bss then share the RELRO page: not valid.
    w.rejects(Rule::Segments, "not RELRO inside PT_GNU_RELRO");
    w.p().chunks[ChunkId(7)].relro = true;
    w.p().chunks[ChunkId(8)].relro = true;
    assert_eq!(w.validate(), Ok(()));
    w.p().chunks[ChunkId(7)].relro = false;
    w.rejects(Rule::Segments, "RELRO run is not contiguous");
    w.p().chunks[ChunkId(7)].relro = true;
    w.p().segments[SegmentId(3)].memsz = 0x800;
    w.rejects(Rule::Segments, "page boundary");
}

// spec: model.md §4.4 rule 3 (a copy-relocation allocation lies in writable memory)
#[test]
fn s4_4_rule3_copy_in_read_only_memory() {
    use mortar_core::layout::CopyAlloc;
    let mut w = World::new();
    w.resolution.def[SymbolId(3)] =
        Def::Shared(SymEntry { file: FileId(2), index: SymbolIndex(6) });
    w.demands.global[SymbolId(3)] = DemandFlags::COPY;
    w.layout.slots.copies = Box::new([CopyAlloc { file: FileId(2), value: 0x100 }]);
    w.layout.slots.copy_of = Box::new([(SymbolId(3), 0)]);
    let copy =
        SymPlace { key: AllocKey::Copy(0), chunk: ChunkId(8), offset: 0x8, size: 8, align: 3 };
    w.p().sym_place = Box::new([w.layout.placement.sym_place[0], copy]);
    w.p().place[SectionId(9)] = Place::NONE;
    w.liveness.live[SectionId(9)] = false;
    assert_eq!(w.validate(), Ok(()));
    w.p().chunks[ChunkId(8)].flags = A;
    w.p().segments[SegmentId(1)].flags = PF_R | PF_W;
    w.rejects(Rule::Segments, "copy allocation in read-only memory");
}

// ---- §4.4 rule 4 ------------------------------------------------------------

// spec: model.md §4.4 rule 4 (a missing patch: a site redirected to no patch)
#[test]
fn s4_4_rule4_missing_patch() {
    let mut w = World::new();
    w.p().patches = Box::new([]);
    w.p().sized = Box::new([Sized { chunk: ChunkId(3), size: 0x10 }]);
    w.rejects(Rule::Patches, "patch that does not exist");
}

// spec: model.md §4.4 rule 4 (every patch is used)
#[test]
fn s4_4_rule4_unused_patch() {
    let mut w = World::new();
    w.p().redirects = Box::new([]);
    let v = w.rejects(Rule::Patches, "no site uses");
    assert_eq!(v.rows, [Row::Entry("patches", 0)]);
}

// spec: model.md §4.4 rule 4 (patches lie in a patch chunk; redirected sites in their section)
#[test]
fn s4_4_rule4_patch_out_of_place() {
    let mut w = World::new();
    w.p().patches[0].offset = 0x10;
    w.rejects(Rule::Patches, "outside a patch chunk");
    let mut w = World::new();
    w.p().redirects[0].offset = 0x18;
    w.rejects(Rule::Patches, "outside its section");
}

// ---- §4.4 rule 5 ------------------------------------------------------------

// spec: model.md §4.4 rule 5 (sym_place for exactly the common symbols)
#[test]
fn s4_4_rule5_sym_place() {
    let mut w = World::new();
    w.p().sym_place = Box::new([]);
    w.rejects(Rule::Presence, "common symbol with no allocation");
    let mut w = World::new();
    w.p().sym_place[0].size = 0x10;
    w.rejects(Rule::Presence, "common's size and alignment");
}

// spec: model.md §4.4 rule 5 (defined for exactly the linker-defined symbols)
#[test]
fn s4_4_rule5_defined() {
    let mut w = World::new();
    w.p().defined = Box::new([]);
    w.rejects(Rule::Presence, "linker-defined symbol with no value");
    let mut w = World::new();
    w.resolution.def[SymbolId(1)] = Def::Undefined { weak: true };
    w.rejects(Rule::Presence, "does not define");
}

// spec: model.md §4.4 rule 5 (sized for exactly the address-dependent chunks)
#[test]
fn s4_4_rule5_sized() {
    let mut w = World::new();
    w.p().sized = Box::new([]);
    w.rejects(Rule::Presence, "no sized row");
    let mut w = World::new();
    w.p().sized[0].size = 8;
    w.rejects(Rule::Presence, "not its sized row's");
}

// spec: model.md §4.4 rule 5 (entry present exactly when needed)
#[test]
fn s4_4_rule5_entry() {
    let mut w = World::new();
    w.p().entry = None;
    w.rejects(Rule::Presence, "needs an entry");
    let mut w = World::new();
    w.needs_entry = false;
    w.rejects(Rule::Presence, "does not need");
}

// ---- fields out of range, widths ----------------------------------------------

// spec: model.md §4.4 (a field out of range: an id no row has)
#[test]
fn s4_4_field_out_of_range() {
    let mut w = World::new();
    w.p().place[SectionId(1)] = Place::at(ChunkId(99), 0);
    w.rejects(Rule::Range, "chunk out of range");
    let mut w = World::new();
    w.p().patches[0].kind = PatchKind::Thunk(ThunkTarget::Plt(SymbolId(40).into()));
    w.rejects(Rule::Range, "out of range");
    let mut w = World::new();
    w.p().chunks[ChunkId(2)].kind = ChunkKind::Body(OutSecId(17));
    w.rejects(Rule::Range, "output section 17 out of range");
}

// spec: model.md §1 (widths: a value that does not fit the class is refused, never truncated)
#[test]
fn s1_width_of_the_class() {
    let mut w = World::new();
    w.class = Class::Elf32;
    assert_eq!(w.validate(), Ok(()));
    w.p().chunks[ChunkId(9)].offset = 0xffff_ff00;
    w.rejects(Rule::Width, "does not fit the class");
}

// ---- §4.1, §4.2, §4.3 ---------------------------------------------------------------

// spec: model.md §4.1 (a folded section has its target's place; targets are not folded)
#[test]
fn s4_1_folds() {
    let mut w = World::new();
    let mut folded = Column::filled(10, OptId::NONE);
    folded[SectionId(7)] = OptId::some(SectionId(1));
    w.layout.folds = Folds { folded: Some(folded) };
    w.rejects(Rule::Places, "does not have its target's place");
    w.p().place[SectionId(7)] = Place::at(ChunkId(2), 0);
    w.p().redirects = Box::new([]);
    w.p().patches = Box::new([]);
    assert_eq!(w.validate(), Ok(()));
    w.layout.folds.folded.as_mut().unwrap()[SectionId(1)] = OptId::some(SectionId(7));
    w.rejects(Rule::Folds, "itself folded");
    w.layout.folds.folded.as_mut().unwrap()[SectionId(1)] = OptId::some(SectionId(1));
    w.rejects(Rule::Folds, "folded into itself");
}

// spec: model.md §4.2 (live fragments do not overlap and honor their alignment)
#[test]
fn s4_2_merge_map() {
    let mut w = World::new();
    w.layout.merge.frag_off[FragId(1)] = 2;
    w.rejects(Rule::Merge, "overlap");
    let mut w = World::new();
    w.split.frags[FragId(1)].align = 1;
    w.rejects(Rule::Merge, "misaligned");
    let mut w = World::new();
    w.liveness.frag_live[FragId(1)] = false;
    w.rejects(Rule::Merge, "dead fragment has an offset");
    w.layout.merge.frag_off[FragId(1)] = MergeMap::NONE;
    assert_eq!(w.validate(), Ok(()));
    w.layout.merge.frag_off[FragId(0)] = 4;
    w.rejects(Rule::Merge, "ends past its chunk");
}

// spec: model.md §4.3 (every demand has exactly one slot, no slot is given twice)
#[test]
fn s4_3_slots() {
    let local = SymKey::local(SymEntry { file: FileId(1), index: SymbolIndex(2) });
    let mut w = World::new();
    w.demands.global[SymbolId(0)] = DemandFlags::GOT;
    w.demands.local =
        Box::new([(SymEntry { file: FileId(1), index: SymbolIndex(2) }, DemandFlags::GOT)]);
    w.rejects(Rule::Slots, "got demand without a slot");
    w.layout.slots.got = Box::new([local, SymbolId(0).into()]);
    assert_eq!(w.validate(), Ok(()));
    w.layout.slots.got = Box::new([local, SymbolId(0).into(), local]);
    w.rejects(Rule::Slots, "got slot given twice");
    w.layout.slots.got = Box::new([local, SymbolId(0).into(), SymbolId(3).into()]);
    w.rejects(Rule::Slots, "got slot without a demand");
    let mut w = World::new();
    w.demands.global[SymbolId(3)] = DemandFlags::EXPORT;
    w.rejects(Rule::Slots, "not in .dynsym");
    w.layout.slots.dynsym = Box::new([SymbolId(3)]);
    assert_eq!(w.validate(), Ok(()));
}

// ---- §4.5 ---------------------------------------------------------------------

// spec: model.md §4.5 (addresses are computed from the layout)
#[test]
fn s4_5_addresses() {
    let w = World::new();
    let p = &w.layout.placement;
    assert_eq!(p.section_addr(SectionId(7), 4), Some(0x40_1044));
    assert_eq!(p.section_addr(SectionId(5), 0), None);
    assert_eq!(p.defined_of(SymbolId(1)), Some(0x40_3050));
    assert_eq!(p.redirect_of(SectionId(7), 4).map(|patch| patch.chunk), Some(ChunkId(3)));
    let at = Addresses::new(&w.resolution, &w.assignment, &w.split, &w.layout);
    assert_eq!(at.frag_addr(FragId(1)), Some(0x40_2003));
    assert_eq!(at.addr_in(SectionId(4), 4), Some(0x40_2004));
    let entries = |e: SymEntry| match (e.file.0, e.index.0) {
        (1, 1) => Some(EntryDef::Section(SectionIndex(1), 0x10)),
        (2, 6) => Some(EntryDef::Section(SectionIndex(1), 0x8)),
        _ => None,
    };
    assert_eq!(at.symbol_addr(SymbolId(0), entries), Some(0x40_1010));
    assert_eq!(at.symbol_addr(SymbolId(1), entries), Some(0x40_3050));
    assert_eq!(at.symbol_addr(SymbolId(2), entries), Some(0x40_3030));
    assert_eq!(at.symbol_addr(SymbolId(3), entries), Some(0x40_1048));
    let mut redirected = World::new();
    redirected.resolution.redirect[SymbolId(0)] = SymbolId(3);
    let at = Addresses::new(
        &redirected.resolution,
        &redirected.assignment,
        &redirected.split,
        &redirected.layout,
    );
    assert_eq!(at.symbol_addr(SymbolId(0), entries), Some(0x40_1048));
}
