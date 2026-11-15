//! `spec/model.md` §2.3, §3: the invariants each derived table checks
//! alone, and the lookups it offers.

use mortar_core::assignment::{Assignment, OutOf};
use mortar_core::column::Column;
use mortar_core::demands::{DemandFlags, Demands, GlobalFlags};
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

// spec: model.md §3.4 (`Consumed`: a kept section the layout consumes, spec/layout.md L-34)
#[test]
fn s3_4_consumed() {
    let r = resolution();
    let mut a = assigned();
    a.outsecs.push(text()).unwrap();
    a.out_of[SectionId(1)] = OutOf::Consumed;
    assert_eq!(a.check(&r), Ok(()));
    assert_eq!(a.out_of[SectionId(1)].out(), None);
    a.out_of[SectionId(0)] = OutOf::Consumed;
    assert!(a.check(&r).unwrap_err().what.contains("consumed but not kept"));
}

// spec: model.md §3.4 (a COMDAT-dropped section may be assigned, spec/layout.md L-42; no other
// dropped section may)
#[test]
fn s3_4_comdat_dropped_assigned() {
    let mut r = resolution();
    let mut a = assigned();
    a.outsecs.push(text()).unwrap();
    r.sections[SectionId(1)].state = SectionState::Dropped(DropReason::Comdat);
    assert_eq!(a.check(&r), Ok(()), "assigned");
    a.out_of[SectionId(1)] = OutOf::None;
    assert_eq!(a.check(&r), Ok(()), "or not");
    a.out_of[SectionId(1)] = OutOf::Out(OutSecId(0));
    r.sections[SectionId(1)].state = SectionState::Dropped(DropReason::Excluded);
    assert!(a.check(&r).unwrap_err().what.contains("assigned but not kept"));
}

// spec: model.md §3.4 (`Discard` exactly for the sections a script drops)
#[test]
fn s3_4_discard_is_script() {
    let mut r = resolution();
    let mut a = assigned();
    a.outsecs.push(text()).unwrap();
    r.sections[SectionId(1)].state = SectionState::Dropped(DropReason::Script);
    a.out_of[SectionId(1)] = OutOf::Discard;
    assert_eq!(a.check(&r), Ok(()));
    a.out_of[SectionId(1)] = OutOf::None;
    assert!(a.check(&r).unwrap_err().what.contains("dropped by the script but not discarded"));
    r.sections[SectionId(1)].state = SectionState::Dropped(DropReason::Stripped);
    a.out_of[SectionId(1)] = OutOf::Discard;
    assert!(a.check(&r).unwrap_err().what.contains("discarded but not dropped"));
}

fn text() -> mortar_core::assignment::OutSec<'static> {
    mortar_core::assignment::OutSec {
        name: b".text",
        sh_type: 1,
        flags: 6,
        merge: None,
        keep: false,
        fill: None,
        constraint: None,
        script_position: None,
    }
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
    d.local = Box::new([(local, DemandFlags::GOT_TPREL)]);
    assert_eq!(d.check(2), Ok(()));
    assert!(d.of(SymbolId(1).into()).contains(DemandFlags::PLT));
    assert_eq!(d.of(local.into()), DemandFlags::GOT_TPREL);
    assert_eq!(d.of(SymbolId(0).into()), DemandFlags::empty());
    assert_eq!(d.keys_with(DemandFlags::GOT | DemandFlags::GOT_TPREL).count(), 2);
    assert_eq!(DemandFlags::from_bits(0x7ff).map(DemandFlags::bits), Some(0x7ff));
    assert_eq!(DemandFlags::from_bits(0x800), None);
    for flag in [DemandFlags::COPY, DemandFlags::CANON_PLT, DemandFlags::EXPORT] {
        d.local = Box::new([(local, flag)]);
        assert!(d.check(2).is_err(), "a local demanding {flag:?}");
    }
    d.local = Box::new([(local, DemandFlags::empty())]);
    assert!(d.check(2).is_err(), "an empty local row");
}

// spec: model.md §3.7 (the flags of one key follow from one target kind: IPLT
// excludes PLT, CANON_PLT and COPY; COPY excludes CANON_PLT)
#[test]
fn s3_7_exclusive_flags() {
    let local = LocalSym { file: FileId(1), index: SymbolIndex(3) };
    let mut d = Demands::empty(2);
    // Accepted: an IFUNC's GOT slot and IPLT entry; a local IFUNC; the
    // inhibitors with anything.
    d.global[SymbolId(0)] = DemandFlags::GOT | DemandFlags::IPLT | DemandFlags::GOT_NORELAX;
    d.global[SymbolId(1)] = DemandFlags::COPY | DemandFlags::EXPORT | DemandFlags::TLSIE_NORELAX;
    d.local = Box::new([(local, DemandFlags::IPLT | DemandFlags::GOT)]);
    assert_eq!(d.check(2), Ok(()));
    d.local = Box::new([]);
    let bad = [
        (DemandFlags::IPLT, DemandFlags::PLT),
        (DemandFlags::IPLT, DemandFlags::CANON_PLT),
        (DemandFlags::IPLT, DemandFlags::COPY),
        (DemandFlags::COPY, DemandFlags::CANON_PLT),
    ];
    assert_eq!(bad, DemandFlags::EXCLUSIVE);
    for (a, b) in bad {
        let mut g = d.clone();
        g.global[SymbolId(1)] = a | b | DemandFlags::GOT;
        let err = g.check(2).expect_err("exclusive flags on a global");
        assert!(err.what.contains("demands both"), "{err:?}");
        assert_eq!((a | b).conflict(), Some((a, b)));
    }
    // The same rule holds for a local.
    d.local = Box::new([(local, DemandFlags::IPLT | DemandFlags::PLT)]);
    assert!(d.check(2).is_err(), "exclusive flags on a local");
}

// spec: model.md §3.7 (per-key and per-output flags merge by union, §6 rule 2)
#[test]
fn s3_7_flags_merge_by_union() {
    let parts = [DemandFlags::GOT, DemandFlags::GOT_NORELAX, DemandFlags::GOT, DemandFlags::EXPORT];
    let one = parts.iter().fold(DemandFlags::empty(), |a, &b| a | b);
    let other = parts.iter().rev().fold(DemandFlags::empty(), |a, &b| a | b);
    assert_eq!(one, other, "commutative");
    assert_eq!(one | one, one, "idempotent");
    assert_eq!(one, DemandFlags::GOT | DemandFlags::GOT_NORELAX | DemandFlags::EXPORT);
    let mut out = GlobalFlags::empty();
    for f in [GlobalFlags::TLS_LD, GlobalFlags::GOT_BASE, GlobalFlags::TLS_LD] {
        out |= f;
    }
    assert_eq!(out, GlobalFlags::GOT_BASE | GlobalFlags::TLS_LD);
    assert!(!out.contains(GlobalFlags::STATIC_TLS));
    assert_eq!(GlobalFlags::from_bits(0x7).map(GlobalFlags::bits), Some(0x7));
    assert_eq!(GlobalFlags::from_bits(0x8), None);
    assert_eq!(Demands::empty(3).output, GlobalFlags::empty());
}

// spec: model.md §7.2 (the flags another file's bytes depend on: GOT_NORELAX,
// TLSIE_NORELAX, CANON_PLT, COPY)
#[test]
fn s7_2_cross_file_flags() {
    assert_eq!(
        DemandFlags::CROSS_FILE,
        DemandFlags::GOT_NORELAX
            | DemandFlags::TLSIE_NORELAX
            | DemandFlags::CANON_PLT
            | DemandFlags::COPY
    );
    let local = LocalSym { file: FileId(1), index: SymbolIndex(3) };
    let mut d = Demands::empty(3);
    d.global[SymbolId(0)] = DemandFlags::GOT | DemandFlags::PLT;
    d.global[SymbolId(1)] = DemandFlags::GOT | DemandFlags::GOT_NORELAX | DemandFlags::CANON_PLT;
    d.global[SymbolId(2)] = DemandFlags::TLS_GD | DemandFlags::EXPORT;
    d.local = Box::new([(local, DemandFlags::GOT_TPREL | DemandFlags::TLSIE_NORELAX)]);
    let rows: Vec<_> = d.cross_file().collect();
    assert_eq!(
        rows,
        [
            (SymbolId(1).into(), DemandFlags::GOT_NORELAX | DemandFlags::CANON_PLT),
            (local.into(), DemandFlags::TLSIE_NORELAX),
        ]
    );
}
