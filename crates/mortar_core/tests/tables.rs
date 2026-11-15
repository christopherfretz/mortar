//! `spec/model.md` §2.3, §3: the invariants each derived table checks
//! alone, and the lookups it offers.

use mortar_core::assignment::{Assignment, OutOf};
use mortar_core::column::Column;
use mortar_core::demands::{DemandFlags, Demands};
use mortar_core::id::{
    FileId, FragId, IdRange, LocalSym, OptId, OutSecId, SectionId, Span, SymEntry, SymbolId,
};
use mortar_core::inputs::{ContentHash, File, InputFlags, Inputs, LinkTarget, Reader, SymbolKey};
use mortar_core::resolution::{
    Def, DropReason, FileLive, Resolution, SectionKind, SectionRow, SectionState,
};
use mortar_core::split::{Frag, Split, SplitRow};
use mortar_elf::{Machine, SectionIndex, SymbolIndex};

fn file(reader: Reader<'static>, syms: Span) -> File<'static> {
    File {
        path: b"x.so",
        member: None,
        input: Some(0),
        state: InputFlags::empty(),
        reader,
        sections: 0,
        syms,
        hash: ContentHash(0),
    }
}

fn inputs(sym_map: Vec<u32>, symbols: u32) -> Inputs<'static> {
    let half = sym_map.len() as u32 / 2;
    let key = SymbolKey { name: b"s", version: None, hash: 0 };
    Inputs {
        target: LinkTarget::of(Machine::X86_64),
        files: Column::from_vec(vec![
            file(Reader::Internal, Span::default()),
            file(Reader::Shared(b""), Span { start: 0, len: half }),
            file(Reader::Shared(b""), Span { start: half, len: sym_map.len() as u32 - half }),
        ])
        .unwrap(),
        sym_map: sym_map.into_iter().map(SymbolId).collect(),
        symbols: Column::from_vec(vec![key; symbols as usize]).unwrap(),
        comdats: Column::new(),
        archives: Column::new(),
    }
}

// spec: model.md §2.3 (symbol ids are numbered by first mention, in FileId then table order)
#[test]
fn s2_3_symbol_numbering_is_by_first_mention() {
    assert_eq!(inputs(vec![0, 1, 0, 2], 3).check(), Ok(()));
    assert_eq!(inputs(vec![0, 1, 1, 2, 0, 3], 4).check(), Ok(()));
    assert!(inputs(vec![1, 0, 2, 2], 3).check().unwrap_err().what.contains("first mention"));
    assert!(inputs(vec![0, 1, 0, 1], 3).check().unwrap_err().what.contains("mentioned"));
    let mut gap = inputs(vec![0, 1, 2, 3], 4);
    gap.files[FileId(2)].syms.start = 3;
    assert!(gap.check().unwrap_err().what.contains("contiguous"));
    let mut internal = inputs(vec![0, 1], 2);
    internal.files[FileId(1)].reader = Reader::Internal;
    assert!(internal.check().unwrap_err().what.contains("internal file"));
    assert_eq!(inputs(vec![0, 1, 0, 1], 2).sym_map_of(FileId(2)), &[SymbolId(0), SymbolId(1)]);
}

// spec: model.md §3.2 (the target: class and relocation form follow from the machine)
#[test]
fn s3_2_target() {
    use mortar_core::inputs::RelocForm;
    use mortar_elf::Class;
    assert_eq!(LinkTarget::of(Machine::I386).class, Class::Elf32);
    assert_eq!(LinkTarget::of(Machine::I386).reloc, RelocForm::Rel);
    assert_eq!(LinkTarget::of(Machine::AArch64).reloc, RelocForm::Rela);
}

fn kept(kind: SectionKind) -> SectionRow {
    SectionRow { kind, state: SectionState::Kept, reloc: SectionIndex(0), comdat: OptId::NONE }
}

fn resolution() -> Resolution {
    let mut r = Resolution::empty(3, 2, 0);
    r.file_live =
        Column::from_vec(vec![FileLive::Always, FileLive::Always, FileLive::Dead]).unwrap();
    r.section_base[FileId(1)] = IdRange::new(SectionId(0), SectionId(3));
    r.section_base[FileId(2)] = IdRange::empty(SectionId(3));
    r.sections = Column::from_vec(vec![
        SectionRow {
            state: SectionState::Dropped(DropReason::Consumed),
            ..kept(SectionKind::Null)
        },
        kept(SectionKind::Regular),
        kept(SectionKind::Merge),
    ])
    .unwrap();
    r.def[SymbolId(0)] = Def::Defined(SymEntry { file: FileId(1), index: SymbolIndex(1) });
    r
}

// spec: model.md §2.2, §3.3 (section ranges are contiguous per live file; definitions in live files)
#[test]
fn s3_3_resolution_check() {
    let r = resolution();
    assert_eq!(r.check(), Ok(()));
    assert_eq!(r.file_of(SectionId(2)), Some(FileId(1)));
    assert_eq!(r.file_of(SectionId(3)), None);
    assert_eq!(r.sh_index(SectionId(1)), Some((FileId(1), SectionIndex(1))));
    assert_eq!(r.section_id(FileId(1), SectionIndex(2)), Some(SectionId(2)));
    assert_eq!(r.section_id(FileId(1), SectionIndex(3)), None);
    let mut dead = resolution();
    dead.def[SymbolId(1)] = Def::Common(SymEntry { file: FileId(2), index: SymbolIndex(1) });
    assert!(dead.check().unwrap_err().what.contains("dead"));
    let mut gap = resolution();
    gap.section_base[FileId(2)] = IdRange::empty(SectionId(4));
    assert!(gap.check().unwrap_err().what.contains("contiguous"));
    let mut kept_symtab = resolution();
    kept_symtab.sections[SectionId(0)].state = SectionState::Kept;
    assert!(kept_symtab.check().unwrap_err().what.contains("without output is kept"));
}

// spec: model.md §3.4 (every kept, output-carrying section is assigned)
#[test]
fn s3_4_assignment_check() {
    let r = resolution();
    let mut a = Assignment::empty(3);
    a.outsecs
        .push(mortar_core::assignment::OutSec {
            name: b".text",
            sh_type: 1,
            flags: 6,
            merge: None,
            keep: false,
            fill: None,
            constraint: None,
            script_position: None,
        })
        .unwrap();
    a.out_of[SectionId(1)] = OutOf::Out(OutSecId(0));
    assert!(a.check(&r).unwrap_err().what.contains("section 2"));
    a.out_of[SectionId(2)] = OutOf::Out(OutSecId(0));
    assert_eq!(a.check(&r), Ok(()));
    a.out_of[SectionId(0)] = OutOf::Discard;
    assert!(a.check(&r).unwrap_err().what.contains("discarded"));
}

fn split() -> Split {
    let frag = |section, offset| Frag { section: SectionId(section), offset, size: 2, align: 0 };
    Split {
        merge_of: Box::new([
            SplitRow { section: SectionId(1), pieces: Span { start: 0, len: 3 } },
            SplitRow { section: SectionId(2), pieces: Span { start: 3, len: 2 } },
        ]),
        piece_off: Box::new([0, 2, 6, 0, 2]),
        piece_frag: Box::new([FragId(0), FragId(1), FragId(0), FragId(1), FragId(2)]),
        frags: Column::from_vec(vec![frag(1, 0), frag(1, 2), frag(2, 2)]).unwrap(),
        ..Split::default()
    }
}

fn assigned() -> Assignment<'static> {
    let mut a = Assignment::empty(3);
    a.out_of[SectionId(1)] = OutOf::Out(OutSecId(0));
    a.out_of[SectionId(2)] = OutOf::Out(OutSecId(0));
    a
}

// spec: model.md §3.5 (fragments numbered by first occurrence, within one output section)
#[test]
fn s3_5_split_check() {
    let a = assigned();
    assert_eq!(split().check(&a), Ok(()));
    let mut renumbered = split();
    renumbered.piece_frag.swap(0, 1);
    assert!(renumbered.check(&a).unwrap_err().what.contains("first occurrence"));
    let mut apart = assigned();
    apart.out_of[SectionId(2)] = OutOf::Out(OutSecId(1));
    assert!(split().check(&apart).unwrap_err().what.contains("across output sections"));
    let mut unsorted = split();
    unsorted.piece_off[1] = 0;
    assert!(unsorted.check(&a).unwrap_err().what.contains("ascending"));
}

// spec: model.md §3.5 (a reference into a split section is resolved to a piece by offset)
#[test]
fn s3_5_piece_at() {
    let s = split();
    let at = s.merge_piece_at(SectionId(1), 7).unwrap();
    assert_eq!((at.piece, at.ordinal, at.within), (2, 2, 1));
    let at = s.merge_piece_at(SectionId(2), 1).unwrap();
    assert_eq!((at.piece, at.ordinal, at.within), (3, 0, 1));
    assert_eq!(s.merge_piece_at(SectionId(0), 0), None);
    assert_eq!(s.merge_pieces(SectionId(2)), Some(Span { start: 3, len: 2 }));
}

// spec: model.md §3.7 (flags per key; locals demand no copy, canonical PLT or export)
#[test]
fn s3_7_demands() {
    let local = LocalSym { file: FileId(1), index: SymbolIndex(3) };
    let mut d = Demands::empty(2);
    d.global[SymbolId(1)] = DemandFlags::GOT | DemandFlags::PLT;
    d.local = Box::new([(local, DemandFlags::TLS_IE)]);
    assert_eq!(d.check(2), Ok(()));
    assert!(d.of(SymbolId(1).into()).contains(DemandFlags::PLT));
    assert_eq!(d.of(local.into()), DemandFlags::TLS_IE);
    assert_eq!(d.of(SymbolId(0).into()), DemandFlags::empty());
    assert_eq!(d.keys_with(DemandFlags::GOT | DemandFlags::TLS_IE).count(), 2);
    assert_eq!(DemandFlags::from_bits(0xff).map(DemandFlags::bits), Some(0xff));
    d.local = Box::new([(local, DemandFlags::EXPORT)]);
    assert!(d.check(2).is_err());
}
