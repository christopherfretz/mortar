//! `spec/model.md` §4: `Layout::check` accepts a hand-built valid layout and
//! rejects one mutation per rule; the addresses of §4.5.

use mortar_core::assignment::{Assignment, MergeClass, OutOf, OutSec};
use mortar_core::check::{CheckContext, Row, Rule, Shape, Violation};
use mortar_core::column::Column;
use mortar_core::demands::{DemandFlags, Demands, GlobalFlags};
use mortar_core::id::{
    ChunkId, FileId, FragId, OptId, OutSecId, SectionId, SegmentId, Span, SymEntry, SymKey,
    SymbolId,
};
use mortar_core::layout::{
    Addresses, AllocKey, Chunk, ChunkKind, DefValue, Defined, EntryDef, Erratum, Folds, Layout,
    MergeMap, Part, Patch, PatchKind, Place, Placement, Redirect, Segment, SegmentRules, Sized,
    SlotKind, Slots, SymPlace, Synth, ThunkTarget, Tls, derive_segments,
};
use mortar_core::liveness::Liveness;
use mortar_core::resolution::{
    CommonShape, Def, DropReason, FileLive, Resolution, SectionKind, SectionRow, SectionState,
};
use mortar_core::split::{Frag, Split, SplitRow};
use mortar_core::{Id, IdRange};
use mortar_diag::{Context, Kind, Severity};
use mortar_elf::consts::{
    PF_R, PF_W, PF_X, PT_GNU_STACK, PT_LOAD, PT_TLS, SHF_ALLOC, SHF_EXECINSTR, SHF_MERGE,
    SHF_STRINGS, SHF_TLS, SHF_WRITE, SHT_NOBITS, SHT_PROGBITS,
};
use mortar_elf::{Class, SectionIndex, SymbolIndex};

const A: u64 = SHF_ALLOC;
const RULES: SegmentRules =
    SegmentRules { class: Class::Elf64, max_page: 0x1000, stack: PF_R | PF_W, pie: false };
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
            Def::Internal(mortar_core::resolution::InternalDef::Assign(0)),
            Def::Common(entry(2, 5)),
            Def::Defined(entry(2, 6)),
        ]);
        resolution.common =
            Box::new([(SymbolId(2), CommonShape { size: 0x20, align: 3, large: false })]);

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
                ChunkKind::Body(Part::first(OutSecId(0))),
                SHT_PROGBITS,
                AX,
                4,
                0x60,
                Some(0x40_1000),
                0x1000,
                Some(1),
            ),
            chunk(
                ChunkKind::Patches(Part::first(OutSecId(0))),
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
                ChunkKind::Body(Part::first(OutSecId(2))),
                SHT_PROGBITS,
                WAT,
                3,
                8,
                Some(0x40_3000),
                0x3000,
                Some(3),
            ),
            chunk(
                ChunkKind::Body(Part::first(OutSecId(3))),
                SHT_NOBITS,
                WAT,
                3,
                0x10,
                Some(0x40_3008),
                0x3008,
                Some(4),
            ),
            chunk(
                ChunkKind::Body(Part::first(OutSecId(4))),
                SHT_PROGBITS,
                WA,
                3,
                0x10,
                Some(0x40_3008),
                0x3008,
                Some(5),
            ),
            chunk(
                ChunkKind::Body(Part::first(OutSecId(5))),
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
        // The segments derive_segments gives (spec/layout.md L-125): R
        // (headers), RX (.text and its island), R (.rodata), RW, PT_TLS,
        // PT_GNU_STACK.
        let segments = col(vec![
            segment(PT_LOAD, PF_R, 0, 0x40_0000, 0xe8, 0xe8, 12),
            segment(PT_LOAD, PF_R | PF_X, 0x1000, 0x40_1000, 0x70, 0x70, 12),
            segment(PT_LOAD, PF_R, 0x2000, 0x40_2000, 6, 6, 12),
            segment(PT_LOAD, PF_R | PF_W, 0x3000, 0x40_3000, 0x18, 0x50, 12),
            segment(PT_TLS, PF_R, 0x3000, 0x40_3000, 8, 0x18, 3),
            segment(PT_GNU_STACK, PF_R | PF_W, 0, 0, 0, 0, 4),
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
            holes: Box::new([]),
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
            segment_rules: RULES,
            resolution: &self.resolution,
            assignment: &self.assignment,
            split: &self.split,
            liveness: &self.liveness,
            demands: &self.demands,
            shapes: &self.shapes,
            shared: false,
        }
    }

    fn validate(&self) -> Result<(), Violation> {
        self.layout.validate(&self.cx())
    }

    fn p(&mut self) -> &mut Placement {
        &mut self.layout.placement
    }

    /// Gives the layout the segments its chunks derive (after a mutation of
    /// chunks that a test means to be valid).
    fn rederive(&mut self) {
        let p = &self.layout.placement;
        let segments = derive_segments(&p.chunks, &p.order, &RULES);
        self.layout.placement.segments = col(segments);
    }

    /// Gives the chunks' section indices again, densely in the new chunk
    /// order (L-86), after a mutation of `order` that a test means to break
    /// another rule.
    fn renumber(&mut self) {
        let p = &mut self.layout.placement;
        let mut new = std::collections::HashMap::new();
        for &id in &p.order {
            if let Some(old) = p.chunks[id].shndx {
                let next = new.len() as u32 + 1;
                p.chunks[id].shndx = Some(*new.entry(old).or_insert(next));
            }
        }
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
    // A layout mortar computed that is not valid is mortar's bug (§4.4).
    assert_eq!(diag.id(), "internal-error");
    assert_eq!(diag.severity(), Severity::Error);
    let Kind::InternalError { what } = diag.kind() else { panic!("{diag:?}") };
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
    w.rederive();
    assert_eq!(w.validate(), Ok(()));
}

// ---- §4.4 rule 3 ------------------------------------------------------------

// spec: model.md §4.4 rule 3 (a wrong segment: a chunk outside every PT_LOAD)
#[test]
fn s4_4_rule3_wrong_segment() {
    let mut w = World::new();
    w.p().segments[SegmentId(3)].memsz = 0x40;
    let v = w.rejects(Rule::Segments, "not in exactly one PT_LOAD");
    assert_eq!(v.rows, [Row::Chunk(ChunkId(8))]);
}

// spec: model.md §4.4 rule 3 (the segment maps the chunk's bytes)
#[test]
fn s4_4_rule3_file_mapping() {
    let mut w = World::new();
    w.p().segments[SegmentId(3)].filesz = 0x10;
    w.rejects(Rule::Segments, "where its PT_LOAD maps them");
}

// spec: model.md §4.4 rule 3 (segment flags cover the chunks')
#[test]
fn s4_4_rule3_segment_flags() {
    let mut w = World::new();
    w.p().segments[SegmentId(3)].flags = PF_R;
    w.rejects(Rule::Segments, "read-only PT_LOAD");
}

// spec: model.md §4.4 rule 3 (TLS initialized chunks directly followed by the uninitialized ones)
#[test]
fn s4_4_rule3_split_tls() {
    let mut w = World::new();
    w.p().order.swap(6, 7);
    w.renumber();
    w.rejects(Rule::Segments, "TLS chunks are split");
}

// spec: model.md §4.4 rule 3 (`tls` describes the TLS chunks; PT_TLS matches)
#[test]
fn s4_4_rule3_tls_template() {
    let mut w = World::new();
    w.p().tls.as_mut().unwrap().size = 0x10;
    w.rejects(Rule::Segments, "tls does not describe");
    let mut w = World::new();
    w.p().segments[SegmentId(4)].filesz = 0x18;
    w.rejects(Rule::Segments, "PT_TLS does not match");
    let mut w = World::new();
    w.p().tls = None;
    w.rejects(Rule::Segments, "without a TLS template");
}

/// The world with `.data` moved in front of the TLS chunks: `.data` at
/// 0x403000, `.tdata` at 0x403010, `.tbss` at 0x403018, so the template
/// starts at 0x403010, a multiple of 8 but not of 16 or 32.
fn tls_after_data() -> World {
    let mut w = World::new();
    w.p().order = [0, 1, 2, 3, 4, 7, 5, 6, 8, 9].into_iter().map(ChunkId).collect();
    w.renumber();
    let mut at = |c: u32, addr: u64| {
        let chunk = &mut w.layout.placement.chunks[ChunkId(c)];
        chunk.addr = Some(addr);
        chunk.offset = addr - 0x40_0000;
    };
    at(7, 0x40_3000);
    at(5, 0x40_3010);
    at(6, 0x40_3018);
    w.p().chunks[ChunkId(6)].offset = 0x3018;
    let tls = &mut w.p().segments[SegmentId(4)];
    (tls.offset, tls.addr) = (0x3010, 0x40_3010);
    w.p().tls = Some(Tls { start: 0x40_3010, size: 0x18, align: 3 });
    w
}

// spec: model.md §4.4 rule 3 (`tls.start` is a multiple of `tls.align`, M-3)
#[test]
fn s4_4_rule3_tls_start_aligned() {
    let mut w = tls_after_data();
    assert_eq!(w.validate(), Ok(()), "a template at 0x403010 with alignment 8");
    // .tbss aligned to 32: the template's alignment is 32, its start is not.
    w.p().chunks[ChunkId(6)].align = 5;
    w.p().chunks[ChunkId(6)].addr = Some(0x40_3020);
    w.p().chunks[ChunkId(6)].offset = 0x3018;
    w.p().tls = Some(Tls { start: 0x40_3010, size: 0x20, align: 5 });
    let tls = &mut w.p().segments[SegmentId(4)];
    (tls.memsz, tls.align) = (0x20, 5);
    let v = w.rejects(Rule::Segments, "start is not a multiple of its alignment");
    assert_eq!(v.rows, [Row::Chunk(ChunkId(5))]);
}

// spec: model.md §4.4 rule 3 (the segments are exactly those derive_segments gives,
// spec/layout.md L-125)
#[test]
fn s4_4_rule3_segments_are_derived() {
    use mortar_elf::consts::PT_NOTE;
    let mut w = World::new();
    let extra =
        Segment { p_type: PT_NOTE, flags: PF_R, offset: 0, addr: 0, filesz: 0, memsz: 0, align: 2 };
    w.p().segments.push(extra).unwrap();
    let v = w.rejects(Rule::Segments, "not derived from the chunks");
    assert_eq!(v.rows, [Row::Segment(SegmentId(6))]);
    let mut w = World::new();
    w.p().segments = col(w.layout.placement.segments.as_slice()[..5].to_vec());
    w.rejects(Rule::Segments, "a derived segment is missing");
    let mut w = World::new();
    w.p().segments[SegmentId(5)].flags = PF_R | PF_W | PF_X;
    let v = w.rejects(Rule::Segments, "is not the derived one");
    assert_eq!(v.rows, [Row::Segment(SegmentId(5))]);
    // A PT_LOAD with an alignment other than the page size and its chunks'.
    let mut w = World::new();
    w.p().segments[SegmentId(1)].align = 4;
    w.rejects(Rule::Segments, "is not the derived one");
    // The same table under the RELRO-free rules of another stack: other flags.
    let mut w = World::new();
    w.p().segments[SegmentId(5)].flags = PF_R | PF_W;
    assert_eq!(w.validate(), Ok(()));
}

// spec: model.md §4.4 rule 3 (PT_TLS's file offset is a multiple of the template's alignment,
// spec/layout.md L-76)
#[test]
fn s4_4_rule3_tls_offset_aligned() {
    // .tdata aligned to 8 KiB, above the 4 KiB page: at 0x404000 but file
    // offset 0x3000. The derived PT_LOAD takes the 8 KiB alignment, so the
    // chunk's bytes are not where it maps them; the TLS rule is the backstop
    // for a PT_TLS whose load does not.
    let mut w = World::new();
    w.p().chunks[ChunkId(5)].align = 13;
    for (c, addr) in [(5, 0x40_4000), (6, 0x40_4008), (7, 0x40_4008), (8, 0x40_4018)] {
        w.p().chunks[ChunkId(c)].addr = Some(addr);
    }
    w.p().tls = Some(Tls { start: 0x40_4000, size: 0x18, align: 13 });
    w.rederive();
    w.rejects(Rule::Segments, "where its PT_LOAD maps them");
    // At an 8 KiB-aligned offset it is valid.
    for (c, off) in [(5, 0x4000), (6, 0x4008), (7, 0x4008), (8, 0x4018)] {
        w.p().chunks[ChunkId(c)].offset = off;
    }
    w.p().chunks[ChunkId(9)].offset = 0x4018;
    w.rederive();
    assert_eq!(w.validate(), Ok(()));
}

/// The world with `.tdata` and `.tbss` RELRO: `.pad.relro` (chunk 10)
/// follows them to the page end, and `.data` and `.bss` move to the next page
/// in their own `PT_LOAD` (spec/layout.md L-64, L-70).
fn relro_world() -> World {
    let mut w = World::new();
    w.p().chunks[ChunkId(5)].relro = true;
    w.p().chunks[ChunkId(6)].relro = true;
    let mut pad = chunk(
        ChunkKind::Synthetic(Synth::RELRO_PADDING),
        SHT_NOBITS,
        WA,
        0,
        0xff8,
        Some(0x40_3008),
        0x3008,
        None,
    );
    pad.relro = true;
    w.p().chunks.push(pad).unwrap();
    w.p().order = [0, 1, 2, 3, 4, 5, 6, 10, 7, 8, 9].into_iter().map(ChunkId).collect();
    w.p().chunks[ChunkId(7)].addr = Some(0x40_4008);
    w.p().chunks[ChunkId(8)].addr = Some(0x40_4018);
    w.rederive();
    w
}

// spec: model.md §4.4 rule 3 (the RELRO run is contiguous, ends with .pad.relro and on a
// page boundary; PT_GNU_RELRO is the derived one)
#[test]
fn s4_4_rule3_relro() {
    use mortar_elf::consts::PT_GNU_RELRO;
    let w = relro_world();
    assert_eq!(w.validate(), Ok(()));
    let relro = w.layout.placement.segments.iter().find(|(_, s)| s.p_type == PT_GNU_RELRO);
    let (id, s) = relro.expect("a PT_GNU_RELRO is derived");
    assert_eq!((s.addr, s.memsz), (0x40_3000, 0x1000));

    // The padding is not RELRO: the run ends at .tbss.
    let mut w = relro_world();
    w.p().chunks[ChunkId(10)].relro = false;
    w.rejects(Rule::Segments, "does not end with .pad.relro");

    // .data RELRO after the padding: the run is contiguous, the padding not
    // last.
    let mut w = relro_world();
    w.p().chunks[ChunkId(7)].relro = true;
    w.rejects(Rule::Segments, "does not end with .pad.relro");

    // .rodata RELRO and .tdata not: not contiguous.
    let mut w = relro_world();
    w.p().chunks[ChunkId(4)].relro = true;
    w.p().chunks[ChunkId(5)].relro = false;
    w.rejects(Rule::Segments, "RELRO run is not contiguous");

    // The padding one word short: the run ends off the page.
    let mut w = relro_world();
    w.p().chunks[ChunkId(10)].size = 0xff0;
    w.p().chunks[ChunkId(10)].capacity = 0xff0;
    w.rederive();
    w.rejects(Rule::Segments, "page boundary");

    // PT_GNU_RELRO stretched over .data: a non-RELRO chunk inside it.
    let mut w = relro_world();
    w.p().segments[id].memsz = 0x2000;
    w.rejects(Rule::Segments, "not RELRO inside PT_GNU_RELRO");

    // PT_GNU_RELRO not the derived one.
    let mut w = relro_world();
    w.p().segments[id].filesz = 0;
    w.rejects(Rule::Segments, "is not the derived one");
}

// spec: model.md §4.4 rule 3 (a copy-relocation allocation lies in writable memory)
#[test]
fn s4_4_rule3_copy_in_read_only_memory() {
    use mortar_core::layout::CopyAlloc;
    let mut w = World::new();
    w.resolution.def[SymbolId(3)] =
        Def::Shared(SymEntry { file: FileId(2), index: SymbolIndex(6) });
    w.resolution.props[SymbolId(3)].refs = mortar_core::resolution::RefFlags::PREEMPTIBLE;
    w.demands.global[SymbolId(3)] = DemandFlags::COPY;
    // D-42: the record's alias is the copied key; D-21 item 3: it is in
    // .dynsym.
    w.layout.slots.copies =
        Box::new([CopyAlloc { alias: SymbolId(3), align: 8, ..CopyAlloc::at(FileId(2), 0x100) }]);
    w.layout.slots.copy_of = Box::new([(SymbolId(3), 0)]);
    w.layout.slots.dynsym = Box::new([SymbolId(3)]);
    let copy =
        SymPlace { key: AllocKey::Copy(0), chunk: ChunkId(8), offset: 0x8, size: 8, align: 3 };
    w.p().sym_place = Box::new([w.layout.placement.sym_place[0], copy]);
    w.p().place[SectionId(9)] = Place::NONE;
    w.liveness.live[SectionId(9)] = false;
    // spec/dyn.md D-42: the allocation is in the chunk its place names.
    w.rejects(Rule::Segments, "a copy allocation outside its chunk .dynbss");
    w.p().chunks[ChunkId(8)].kind =
        ChunkKind::Synthetic(Synth { name: b".dynbss", address_dependent: false });
    assert_eq!(w.validate(), Ok(()));
    w.p().chunks[ChunkId(8)].flags = A;
    w.p().segments[SegmentId(3)].flags = PF_R | PF_W;
    w.rejects(Rule::Segments, "copy allocation in read-only memory");
}

// spec: dyn.md D-42 (alignment and extent of a copy allocation in its
// chunk, and the copy chunk first in its section). The order of several
// allocations is tested with mortar_layout's dynamic fixtures (this world
// has one free symbol).
#[test]
fn d42_copy_placement() {
    use mortar_core::layout::{CopyAlloc, CopyPlace};
    let mut w = World::new();
    w.resolution.def[SymbolId(3)] =
        Def::Shared(SymEntry { file: FileId(2), index: SymbolIndex(6) });
    w.resolution.props[SymbolId(3)].refs = mortar_core::resolution::RefFlags::PREEMPTIBLE;
    w.demands.global[SymbolId(3)] = DemandFlags::COPY;
    w.layout.slots.copies =
        Box::new([CopyAlloc { alias: SymbolId(3), align: 8, ..CopyAlloc::at(FileId(2), 0x100) }]);
    w.layout.slots.copy_of = Box::new([(SymbolId(3), 0)]);
    w.layout.slots.dynsym = Box::new([SymbolId(3)]);
    w.p().chunks[ChunkId(8)].kind =
        ChunkKind::Synthetic(Synth { name: b".dynbss", address_dependent: false });
    w.p().place[SectionId(9)] = Place::NONE;
    w.liveness.live[SectionId(9)] = false;
    let at = |offset, align| SymPlace {
        key: AllocKey::Copy(0),
        chunk: ChunkId(8),
        offset,
        size: 8,
        align,
    };
    let common = w.layout.placement.sym_place[0];
    w.p().sym_place = Box::new([common, at(0, 3)]);
    assert_eq!(w.validate(), Ok(()));
    // Misaligned (alignment 8, log2 3): the general placement rule.
    w.p().sym_place = Box::new([common, at(0xc, 3)]);
    w.rejects(Rule::Places, "misaligned");
    // The row's alignment is not the allocation's.
    w.p().sym_place = Box::new([common, at(0, 2)]);
    w.rejects(Rule::Segments, "not at its alignment");
    // Past the chunk's size (0x38).
    w.p().sym_place = Box::new([common, at(0x38, 3)]);
    w.rejects(Rule::Places, "not inside an alloc chunk");
    // A read-only copy belongs in .bss.rel.ro.
    w.p().sym_place = Box::new([common, at(0, 3)]);
    w.layout.slots.copies[0].place = CopyPlace::RelRo;
    w.rejects(Rule::Segments, "outside its chunk .bss.rel.ro");
}

// spec: layout.md L-68, dyn.md D-1 (base 0 exactly for a position-independent output)
#[test]
fn d1_pie_base() {
    let w = World::new();
    let mut cx = w.cx();
    cx.segment_rules.pie = true;
    let v = w.layout.validate(&cx).expect_err("a PIE at 0x400000");
    assert!(v.what.contains("position-independent output at base 0x400000"), "{v:?}");
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

// spec: model.md §4.4 rule 4 (a patch no site uses is allowed, M-4)
#[test]
fn s4_4_rule4_unused_patch() {
    let mut w = World::new();
    w.p().redirects = Box::new([]);
    assert_eq!(w.validate(), Ok(()));
}

fn erratum(section: u32, offset: u64) -> PatchKind {
    PatchKind::Erratum { erratum: Erratum::E843419, section: SectionId(section), offset }
}

// spec: model.md §4.4 `patches` (an erratum patch is keyed by its site, M-7) and rule 4 (an
// erratum redirect names the erratum patch of its own site)
#[test]
fn s4_4_rule4_erratum_patch_of_its_site() {
    let mut w = World::new();
    w.p().patches[0].kind = erratum(7, 4);
    assert_eq!(w.validate(), Ok(()), "the site (7, 4) redirected to its own erratum patch");
    w.p().patches[0].kind = erratum(7, 8);
    let v = w.rejects(Rule::Patches, "erratum patch of another site");
    assert_eq!(v.rows, [Row::Entry("redirects", 0), Row::Entry("patches", 0)]);
    w.p().patches[0].kind = erratum(1, 4);
    w.rejects(Rule::Patches, "erratum patch of another site");
    // Unused, an erratum patch keyed by any placed site is allowed.
    w.p().redirects = Box::new([]);
    assert_eq!(w.validate(), Ok(()));
}

/// The world with a second island, `Patches(.text, 1)` (chunk 10), after the
/// first.
fn two_islands() -> World {
    let mut w = World::new();
    let island = chunk(
        ChunkKind::Patches(Part { out: OutSecId(0), part: 1 }),
        SHT_PROGBITS,
        AX,
        2,
        0x10,
        Some(0x40_1070),
        0x1070,
        Some(1),
    );
    w.p().chunks.push(island).unwrap();
    w.p().order = [0, 1, 2, 3, 10, 4, 5, 6, 7, 8, 9].into_iter().map(ChunkId).collect();
    w.p().sized = Box::new([
        Sized { chunk: ChunkId(3), size: 0x10 },
        Sized { chunk: ChunkId(10), size: 0x10 },
    ]);
    w.rederive();
    w
}

// spec: model.md §4.4 rule 4b (an erratum patch lies in the island of its site's part,
// spec/layout.md L-111)
#[test]
fn s4_4_rule4b_erratum_in_its_part() {
    let mut w = two_islands();
    w.p().patches[0].kind = erratum(7, 4);
    assert_eq!(w.validate(), Ok(()), "in Patches(.text, 0), the part of the site");
    w.p().patches[0].chunk = ChunkId(10);
    let v = w.rejects(Rule::Patches, "outside the island of its site's part");
    assert_eq!(v.rows[0], Row::Entry("patches", 0));
}

// spec: model.md §4.4 rule 4b (one island holds one patch per target and per site,
// spec/layout.md L-112)
#[test]
fn s4_4_rule4b_no_duplicate_in_an_island() {
    let thunk = w_patch(ChunkId(3), 8);
    let mut w = two_islands();
    let first = w.layout.placement.patches[0];
    w.p().patches = Box::new([first, Patch { chunk: ChunkId(10), offset: 0, ..thunk }]);
    assert_eq!(w.validate(), Ok(()), "one thunk of a target in each of two islands");
    w.p().patches = Box::new([first, thunk]);
    let v = w.rejects(Rule::Patches, "two patches of one target or site in one island");
    assert_eq!(v.rows, [Row::Entry("patches", 0), Row::Entry("patches", 1)]);
    // Another addend is another target.
    let other = PatchKind::Thunk(ThunkTarget::Sym { key: SymbolId(3).into(), addend: 4 });
    w.p().patches = Box::new([first, Patch { kind: other, ..thunk }]);
    assert_eq!(w.validate(), Ok(()));
    // Two erratum patches of one site in one island.
    w.p().patches =
        Box::new([Patch { kind: erratum(7, 4), ..first }, Patch { kind: erratum(7, 4), ..thunk }]);
    w.rejects(Rule::Patches, "two patches of one target or site in one island");
}

fn w_patch(chunk: ChunkId, offset: u64) -> Patch {
    Patch {
        chunk,
        offset,
        kind: PatchKind::Thunk(ThunkTarget::Sym { key: SymbolId(3).into(), addend: 0 }),
    }
}

// spec: model.md §4.4 rule 4 (a `Plt(key)` thunk's key has a PLT or IPLT index)
#[test]
fn s4_4_rule4_plt_thunk_needs_an_entry() {
    let mut w = World::new();
    w.p().patches[0].kind = PatchKind::Thunk(ThunkTarget::Plt(SymbolId(3).into()));
    let v = w.rejects(Rule::Patches, "no PLT or IPLT entry");
    assert_eq!(v.rows, [Row::Entry("patches", 0), Row::Symbol(SymbolId(3).into())]);
    w.demands.global[SymbolId(3)] = DemandFlags::PLT;
    w.layout.slots.plt = Box::new([SymbolId(3).into()]);
    assert_eq!(w.validate(), Ok(()), "a PLT entry");
    w.demands.global[SymbolId(3)] = DemandFlags::CANON_PLT;
    assert_eq!(w.validate(), Ok(()), "a canonical PLT entry");
    w.demands.global[SymbolId(3)] = DemandFlags::IPLT;
    w.layout.slots.plt = Box::new([]);
    w.layout.slots.iplt = Box::new([SymbolId(3).into()]);
    assert_eq!(w.validate(), Ok(()), "an IPLT entry");
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
    w.p().patches[0].kind = erratum(10, 4);
    let v = w.rejects(Rule::Range, "out of range");
    assert_eq!(v.rows, [Row::Entry("patches", 0)]);
    let mut w = World::new();
    w.p().chunks[ChunkId(2)].kind = ChunkKind::Body(Part::first(OutSecId(17)));
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

// spec: model.md §4.3 (an index per slot kind: one key holds slots of several kinds)
#[test]
fn s4_3_slots_per_kind() {
    let mut w = World::new();
    let k: SymKey = SymbolId(0).into();
    w.demands.global[SymbolId(0)] =
        DemandFlags::GOT | DemandFlags::GOT_TPREL | DemandFlags::TLS_GD | DemandFlags::TLS_DESC;
    w.layout.slots.got = Box::new([k]);
    w.layout.slots.tls_ie = Box::new([k]);
    w.layout.slots.tls_gd = Box::new([k]);
    w.layout.slots.tls_desc = Box::new([k]);
    assert_eq!(w.validate(), Ok(()));
    let one = [k];
    for kind in SlotKind::ALL {
        let want: &[SymKey] = match kind {
            SlotKind::Plt | SlotKind::Iplt => &[],
            _ => &one,
        };
        assert_eq!(w.layout.slots.list(kind), want, "{kind:?}");
    }
    w.layout.slots.tls_ie = Box::new([]);
    w.rejects(Rule::Slots, "tls_ie demand without a slot");
    w.layout.slots.tls_ie = Box::new([k]);
    w.layout.slots.tls_gd = Box::new([]);
    w.rejects(Rule::Slots, "tls_gd demand without a slot");
    w.layout.slots.tls_gd = Box::new([k]);
    w.layout.slots.tls_desc = Box::new([k, k]);
    w.rejects(Rule::Slots, "tls_desc slot given twice");
}

// spec: model.md §4.3 (the IPLT index of each key that demands IPLT; a PLT index or an
// IPLT index, never both)
#[test]
fn s4_3_iplt() {
    let mut w = World::new();
    let k: SymKey = SymbolId(3).into();
    w.demands.global[SymbolId(3)] = DemandFlags::IPLT | DemandFlags::GOT;
    w.layout.slots.got = Box::new([k]);
    w.rejects(Rule::Slots, "iplt demand without a slot");
    w.layout.slots.iplt = Box::new([k]);
    assert_eq!(w.validate(), Ok(()));
    w.layout.slots.plt = Box::new([k]);
    w.rejects(Rule::Slots, "plt slot without a demand");
    w.layout.slots.plt = Box::new([]);
    w.demands.global[SymbolId(3)] = DemandFlags::GOT;
    w.rejects(Rule::Slots, "iplt slot without a demand");
}

// spec: model.md §4.3 (the output's local-dynamic module slot exists exactly when TLS_LD
// is demanded)
#[test]
fn s4_3_module_slot() {
    let mut w = World::new();
    w.demands.output = GlobalFlags::TLS_LD | GlobalFlags::GOT_BASE;
    w.rejects(Rule::Slots, "TLS_LD demand without the module slot");
    w.layout.slots.tls_ld = true;
    assert_eq!(w.validate(), Ok(()));
    w.demands.output = GlobalFlags::GOT_BASE;
    w.rejects(Rule::Slots, "module slot without a TLS_LD demand");
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

// ---- derive_segments (spec/layout.md L-65…L-67, L-76, L-125) -------------------

/// A 64 KiB-page table: headers, two notes of two alignments (one of them
/// `.note.gnu.property`), `.eh_frame_hdr`, `.text`, and a RW run that starts
/// with the `.tbss`-only TLS template.
fn derive_table() -> (Column<ChunkId, Chunk>, Box<[ChunkId]>) {
    use mortar_elf::consts::SHT_NOTE;
    let synth = |name| ChunkKind::Synthetic(Synth { name, address_dependent: false });
    let body = |o| ChunkKind::Body(Part::first(OutSecId(o)));
    let chunks = col(vec![
        chunk(ChunkKind::FileHeader, SHT_PROGBITS, A, 3, 0x40, Some(0x40_0000), 0, None),
        chunk(ChunkKind::ProgramHeaders, SHT_PROGBITS, A, 3, 0x100, Some(0x40_0040), 0x40, None),
        chunk(synth(b".note.gnu.property"), SHT_NOTE, A, 3, 0x20, Some(0x40_0140), 0x140, Some(1)),
        chunk(body(0), SHT_NOTE, A, 2, 0x20, Some(0x40_0160), 0x160, Some(2)),
        chunk(synth(b".eh_frame_hdr"), SHT_PROGBITS, A, 2, 0x14, Some(0x40_0180), 0x180, Some(3)),
        chunk(body(1), SHT_PROGBITS, AX, 6, 0x100, Some(0x41_0000), 0x1_0000, Some(4)),
        chunk(body(2), SHT_NOBITS, WAT, 4, 0x30, Some(0x42_0100), 0x1_0100, Some(5)),
        chunk(body(3), SHT_NOBITS, WA, 3, 0x40, Some(0x42_0100), 0x1_0100, Some(6)),
    ]);
    (chunks, (0..8).map(ChunkId).collect())
}

// spec: layout.md L-65, L-66, L-67, L-76, L-125 (the derived program headers)
#[test]
fn derive_segments_shape() {
    use mortar_elf::consts::{PT_GNU_EH_FRAME, PT_GNU_PROPERTY, PT_NOTE};
    let (chunks, order) = derive_table();
    let rules =
        SegmentRules { class: Class::Elf64, max_page: 0x1_0000, stack: PF_R | PF_W, pie: false };
    let seg = |p_type, flags, offset, addr, filesz, memsz, align| Segment {
        p_type,
        flags,
        offset,
        addr,
        filesz,
        memsz,
        align,
    };
    assert_eq!(
        derive_segments(&chunks, &order, &rules),
        [
            seg(PT_LOAD, PF_R, 0, 0x40_0000, 0x194, 0x194, 16),
            seg(PT_LOAD, PF_R | PF_X, 0x1_0000, 0x41_0000, 0x100, 0x100, 16),
            // A NOBITS first chunk: the offset congruent to its address.
            seg(PT_LOAD, PF_R | PF_W, 0x1_0100, 0x42_0100, 0, 0x40, 16),
            seg(PT_NOTE, PF_R, 0x140, 0x40_0140, 0x20, 0x20, 3),
            seg(PT_NOTE, PF_R, 0x160, 0x40_0160, 0x20, 0x20, 2),
            seg(PT_TLS, PF_R, 0x1_0100, 0x42_0100, 0, 0x30, 4),
            seg(PT_GNU_PROPERTY, PF_R, 0x140, 0x40_0140, 0x20, 0x20, 3),
            seg(PT_GNU_EH_FRAME, PF_R, 0x180, 0x40_0180, 0x14, 0x14, 2),
            seg(PT_GNU_STACK, PF_R | PF_W, 0, 0, 0, 0, 4),
        ]
    );
    // A chunk aligned above the page raises its PT_LOAD's alignment (L-67).
    let mut big = chunks;
    big[ChunkId(5)].align = 20;
    assert_eq!(derive_segments(&big, &order, &rules)[1].align, 20);
    // ELF32: PT_GNU_PROPERTY is word (4-byte) aligned.
    let rules32 = SegmentRules { class: Class::Elf32, ..rules };
    assert_eq!(derive_segments(&big, &order, &rules32)[6].align, 2);
}

// spec: layout.md L-125 (derive_segments is total: any table derives some segments)
#[test]
fn derive_segments_never_panics() {
    let (mut chunks, _) = derive_table();
    chunks[ChunkId(5)].addr = None;
    chunks[ChunkId(6)].addr = Some(u64::MAX);
    chunks[ChunkId(6)].capacity = u64::MAX;
    chunks[ChunkId(7)].align = 200;
    chunks[ChunkId(7)].offset = u64::MAX;
    let order = [ChunkId(7), ChunkId(99), ChunkId(5), ChunkId(6), ChunkId(0)];
    for max_page in [0, 3, 1 << 63] {
        let rules = SegmentRules { class: Class::Elf64, max_page, stack: 0, pie: false };
        let _ = derive_segments(&chunks, &order, &rules);
        let _ = derive_segments(&chunks, &[], &rules);
    }
}

// spec: model.md §4.3; dyn.md D-21, D-22 (the .dynsym set), D-27
// (verneed), D-24 (.dynstr): the M2 lists of `Slots`, as fixtures build them
// (WP-041 step 0b/0h).
#[test]
fn s4_3_dynamic_lists() {
    use mortar_core::idset::IdSet;
    use mortar_core::layout::{DynStr, VerIndex};
    let mut w = World::new();
    w.layout.slots.dynsym = Box::new([SymbolId(3)]);
    w.rejects(Rule::Slots, "no import, export, copy alias or WeakDyn key");
    // A WeakDyn key (D-25, decided by the scan) is an entry.
    w.demands.weak_dyn = IdSet::from_ids(4, [SymbolId(3)]).unwrap();
    assert_eq!(w.validate(), Ok(()));
    w.layout.slots.dynsym = Box::new([]);
    w.rejects(Rule::Slots, "not in .dynsym");
    w.demands.weak_dyn = IdSet::new();
    w.demands.global[SymbolId(3)] = DemandFlags::EXPORT;
    w.layout.slots.dynsym = Box::new([SymbolId(3)]);
    w.layout.slots.dynsym_unhashed = 2;
    w.rejects(Rule::Slots, "more unhashed");
    w.layout.slots.dynsym_unhashed = 0;
    w.layout.slots.gnu_hash_buckets = 1;
    assert_eq!(w.validate(), Ok(()));
    // D-27: a need of a file that is not needed.
    w.layout.slots.verneed = Box::new([(FileId(2), Box::new([VerIndex(2)]) as Box<[_]>)]);
    w.rejects(Rule::Slots, "version need of a file not needed");
    w.layout.slots.verneed = Box::new([]);
    // D-24: offsets into a .dynstr that is not there, then one per string.
    w.layout.slots.dynstr = DynStr { runpath: Some(1), ..DynStr::default() };
    w.rejects(Rule::Slots, "offsets into an empty .dynstr");
    w.layout.slots.dynstr = DynStr { symbols: Box::new([1]), size: 6, ..DynStr::default() };
    assert_eq!(w.validate(), Ok(()));
    w.layout.slots.dynstr.size = 1;
    w.rejects(Rule::Slots, "outside the section");
}
