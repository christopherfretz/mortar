//! `spec/model.md` §2.3, §3: the invariants each derived table checks
//! alone, and the lookups it offers.

use mortar_core::assignment::{Assignment, OutOf};
use mortar_core::column::Column;
use mortar_core::demands::{DemandFlags, Demands, GlobalFlags};
use mortar_core::id::ArchiveId;
use mortar_core::id::{
    FileId, FragId, IdRange, LocalSym, OptId, OutSecId, SectionId, Span, SymEntry, SymbolId,
};
use mortar_core::inputs::{
    Archive, ArchiveKind, Blob, ClosureEdge, ContentHash, EdgeOutcome, File, FileBytes,
    GroupOrdinal, InputFlags, Inputs, LinkInputs, LinkTarget, Member, Opened, OpenedKind, Origin,
    Probe, ProbeOutcome, Reader, ScriptEntry, SearchRef, SonameUnit, StoreRef, SymbolKey,
};
use mortar_core::resolution::{
    Def, DropReason, FileLive, InternalDef, RefFlags, Resolution, SectionKind, SectionRow,
    SectionState,
};
use mortar_core::split::{Frag, Split, SplitRow};
use mortar_elf::{Machine, Object, SectionIndex, SharedObject, SymbolIndex};

/// A 64-byte ELF64 x86-64 header of type `e_type`, nothing else.
fn header(e_type: u16) -> &'static [u8] {
    let mut h = vec![0u8; 64];
    h[..7].copy_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1, 1]);
    h[16..18].copy_from_slice(&e_type.to_le_bytes());
    h[18..20].copy_from_slice(&62u16.to_le_bytes());
    h[20..24].copy_from_slice(&1u32.to_le_bytes());
    h[52..54].copy_from_slice(&64u16.to_le_bytes());
    Box::leak(h.into_boxed_slice())
}

fn shared() -> Reader<'static> {
    Reader::Shared(SharedObject::parse(header(3)).expect("an empty shared object"))
}

fn object() -> Reader<'static> {
    Reader::Object(Object::parse(header(1)).expect("an empty object"))
}

fn origin(input: u32) -> Origin {
    Origin::Input { input, arg: input, entry: None }
}

#[allow(clippy::large_types_passed_by_value, reason = "a test fixture")]
fn file(path: &'static [u8], origin: Origin, reader: Reader<'static>, syms: Span) -> File<'static> {
    let internal = matches!(reader, Reader::Internal);
    File {
        path,
        member: None,
        origin,
        state: InputFlags::empty(),
        reader,
        bytes: (!internal).then_some(FileBytes { blob: Blob(0), base: 0, len: 64 }),
        sections: 0,
        syms,
        hash: ContentHash(0),
    }
}

fn internal() -> File<'static> {
    file(b"", Origin::Internal, Reader::Internal, Span::default())
}

fn unit(file: u32, mention: Origin) -> SonameUnit {
    SonameUnit {
        file: FileId(file),
        mentions: Box::new([mention]),
        as_needed: false,
        copy_dt_needed: false,
    }
}

fn inputs(sym_map: Vec<u32>, symbols: u32) -> Inputs<'static> {
    let half = sym_map.len() as u32 / 2;
    let key = SymbolKey { name: b"s", version: None, hash: 0 };
    let mut link = LinkInputs::of_files(3);
    link.sonames = Box::new([unit(1, origin(0)), unit(2, origin(1))]);
    Inputs {
        target: LinkTarget::of(Machine::X86_64),
        files: Column::from_vec(vec![
            internal(),
            file(b"x.so", origin(0), shared(), Span { start: 0, len: half }),
            file(
                b"y.so",
                origin(1),
                shared(),
                Span { start: half, len: sym_map.len() as u32 - half },
            ),
        ])
        .unwrap(),
        sym_map: sym_map.into_iter().map(SymbolId).collect(),
        symbols: Column::from_vec(vec![key; symbols as usize]).unwrap(),
        comdats: Column::new(),
        archives: Column::new(),
        link,
        store: StoreRef::none(),
    }
}

/// A link of every kind of file: the internal file; `a.o` (input 0);
/// the members `m1.o`, `m2.o` of `lib.a` (input 1, in `--start-group`
/// 1); `x.so` (input 2), which needs `y.so` (the closure). The opened
/// files: `a.o`, `lib.a`, `x.so`, a script `s.ld` with one entry; two
/// probes for `x.so`.
fn world() -> Inputs<'static> {
    let none = Span::default();
    let mut files = vec![
        internal(),
        file(b"a.o", origin(0), object(), none),
        file(b"lib.a", origin(1), object(), none),
        file(b"lib.a", origin(1), object(), none),
        file(b"x.so", origin(2), shared(), none),
        file(b"y.so", Origin::Closure { needing: FileId(4), index: 0 }, shared(), none),
    ];
    for (i, (name, offset)) in [(&b"m1.o"[..], 8u64), (&b"m2.o"[..], 100)].into_iter().enumerate() {
        files[2 + i].member =
            Some(Member { archive: ArchiveId(0), name, ordinal: 0, offset, thin: None });
    }
    let mut link = LinkInputs::of_files(6);
    link.group[FileId(2)] = Some(GroupOrdinal(1));
    link.group[FileId(3)] = Some(GroupOrdinal(1));
    let closure = Origin::Closure { needing: FileId(4), index: 0 };
    link.sonames = Box::new([unit(4, origin(2)), unit(5, closure)]);
    link.closure = Box::new([ClosureEdge {
        needing: FileId(4),
        index: 0,
        outcome: EdgeOutcome::Satisfied(FileId(5)),
    }]);
    let opened = |path, kind, input| Opened { path, kind, origin: origin(input) };
    link.opened = Box::new([
        opened(&b"a.o"[..], OpenedKind::Object(FileId(1)), 0),
        opened(b"lib.a", OpenedKind::Archive(ArchiveId(0)), 1),
        opened(b"/l/x.so", OpenedKind::Shared(FileId(4)), 2),
        opened(b"s.ld", OpenedKind::Script, 3),
    ]);
    link.script_entries = Box::new([ScriptEntry { script: 3, offset: 6, parent: None }]);
    link.probes = Box::new([
        Probe { path: b"/k/x.so", search: SearchRef::Input(2), outcome: ProbeOutcome::Absent },
        Probe { path: b"/l/x.so", search: SearchRef::Input(2), outcome: ProbeOutcome::Found(2) },
    ]);
    Inputs {
        target: LinkTarget::of(Machine::X86_64),
        files: Column::from_vec(files).unwrap(),
        sym_map: Box::new([]),
        symbols: Column::new(),
        comdats: Column::new(),
        archives: Column::from_vec(vec![Archive {
            path: b"lib.a",
            kind: ArchiveKind::Gnu,
            origin: origin(1),
            members: IdRange::new(FileId(2), FileId(4)),
        }])
        .unwrap(),
        link,
        store: StoreRef::none(),
    }
}

/// `world()` changed by `change` breaks the rule whose message starts
/// with `rule`.
fn breaks(rule: &str, change: impl FnOnce(&mut Inputs<'static>)) {
    let mut w = world();
    change(&mut w);
    let err = w.check().expect_err(rule);
    assert!(err.what.starts_with(rule), "{rule}: {err:?}");
}

// spec: model.md §3.2 (the checks of Inputs: the accepting table)
#[test]
fn s3_2_inputs_world_is_accepted() {
    let w = world();
    assert_eq!(w.check(), Ok(()));
    assert_eq!(w.first_closure_file(), FileId(5));
    assert_eq!(w.group_of(FileId(3)), Some(GroupOrdinal(1)));
    assert_eq!(w.group_of(FileId(4)), None);
    assert_eq!(w.soname_unit(FileId(5)).map(|u| u.file), Some(FileId(5)));
    assert_eq!(w.soname_unit(FileId(1)), None);
    assert_eq!(w.resident(FileId(1), 0..4), None, "no store");
}

// spec: model.md §3.2 rule 1 (only file 0 is internal, with no bytes; sections are the reader's)
#[test]
fn s3_2_rule1_internal_file() {
    breaks("rule 1", |w| w.files[FileId(1)].reader = Reader::Internal);
    breaks("rule 1", |w| w.files[FileId(0)].origin = origin(0));
    breaks("rule 1", |w| w.files[FileId(1)].bytes = None);
    breaks("rule 1", |w| w.files[FileId(0)].bytes = w.files[FileId(1)].bytes);
    breaks("rule 1", |w| w.files[FileId(1)].sections = 3);
    breaks("rule 1", |w| w.files[FileId(4)].sections = 1);
}

// spec: model.md §3.2 rule 3 (members: objects of their archive, contiguous, in archive order)
#[test]
fn s3_2_rule3_members() {
    breaks("rule 3", |w| w.files[FileId(2)].member.as_mut().unwrap().archive = ArchiveId(1));
    breaks("rule 3", |w| w.files[FileId(3)].path = b"other.a");
    breaks("rule 3", |w| w.files[FileId(3)].member.as_mut().unwrap().offset = 4);
    breaks("rule 3", |w| w.archives[ArchiveId(0)].members = IdRange::new(FileId(2), FileId(3)));
    breaks("rule 3", |w| w.archives[ArchiveId(0)].members = IdRange::new(FileId(1), FileId(4)));
}

// spec: model.md §3.2 rule 4 (LAZY: a --start-lib object, never a member)
#[test]
fn s3_2_rule4_lazy() {
    let mut w = world();
    w.files[FileId(1)].state = InputFlags::LAZY;
    assert_eq!(w.check(), Ok(()));
    assert!(w.files[FileId(1)].is_lazy());
    breaks("rule 4", |w| w.files[FileId(2)].state = InputFlags::LAZY);
    breaks("rule 4", |w| w.files[FileId(4)].state = InputFlags::LAZY);
}

// spec: model.md §3.2 rule 5 (input order; the closure last, shared, needed by an earlier file)
#[test]
fn s3_2_rule5_order() {
    breaks("rule 5", |w| w.files[FileId(1)].origin = origin(2));
    breaks("rule 5", |w| {
        w.files[FileId(4)].origin = Origin::Closure { needing: FileId(1), index: 0 }
    });
    breaks("rule 5", |w| {
        w.files[FileId(5)].origin = Origin::Closure { needing: FileId(1), index: 0 }
    });
    breaks("rule 5", |w| {
        w.files[FileId(5)].origin = Origin::Closure { needing: FileId(5), index: 0 }
    });
}

// spec: model.md §3.2 rule 7 (one group row per file; ordinals from 1 in load order)
#[test]
fn s3_2_rule7_groups() {
    breaks("rule 7", |w| w.link.group = Column::filled(5, None));
    breaks("rule 7", |w| w.link.group[FileId(2)] = Some(GroupOrdinal(0)));
    breaks("rule 7", |w| w.link.group[FileId(1)] = Some(GroupOrdinal(2)));
}

// spec: model.md §3.2 rule 8 (one soname unit per shared object, each mentioned)
#[test]
fn s3_2_rule8_soname_units() {
    breaks("rule 8", |w| w.link.sonames = Box::new([unit(4, origin(2))]));
    breaks("rule 8", |w| w.link.sonames[0].mentions = Box::new([]));
    breaks("rule 8", |w| w.link.sonames.swap(0, 1));
}

// spec: model.md §3.2 rule 9 (the closure edges: sorted, between shared objects)
#[test]
fn s3_2_rule9_closure() {
    let edge = |needing, index, outcome| ClosureEdge { needing: FileId(needing), index, outcome };
    let mut w = world();
    w.link.closure = Box::new([
        edge(4, 0, EdgeOutcome::Satisfied(FileId(5))),
        edge(4, 1, EdgeOutcome::NotFound),
    ]);
    assert_eq!(w.check(), Ok(()));
    breaks("rule 9", |w| {
        w.link.closure = Box::new([
            edge(4, 1, EdgeOutcome::NotFound),
            edge(4, 0, EdgeOutcome::Satisfied(FileId(5))),
        ]);
    });
    breaks("rule 9", |w| w.link.closure[0].needing = FileId(1));
    breaks("rule 9", |w| w.link.closure[0].outcome = EdgeOutcome::Satisfied(FileId(2)));
}

// spec: model.md §3.2 rule 10 (indices into opened, script_entries and closure)
#[test]
fn s3_2_rule10_indices() {
    breaks("rule 10", |w| {
        w.files[FileId(1)].origin = Origin::Input { input: 0, arg: 0, entry: Some(1) }
    });
    breaks("rule 10", |w| w.link.opened[0].kind = OpenedKind::Object(FileId(4)));
    breaks("rule 10", |w| w.link.opened[2].kind = OpenedKind::Shared(FileId(1)));
    breaks("rule 10", |w| w.link.opened[1].kind = OpenedKind::Archive(ArchiveId(3)));
    breaks("rule 10", |w| w.link.script_entries[0].script = 0);
    breaks("rule 10", |w| w.link.script_entries[0].parent = Some(0));
    breaks("rule 10", |w| w.link.probes[1].outcome = ProbeOutcome::Found(4));
    breaks("rule 10", |w| w.link.probes[0].search = SearchRef::Edge(1));
    breaks("rule 10", |w| w.link.probes[0].search = SearchRef::Entry(1));
}

// spec: model.md §7.1 (a file's identity: the found path and the mention ordinal)
#[test]
fn s7_1_mention_ordinals() {
    let mut w = world();
    // a.o named twice, lib.a named twice: four more files, one more archive.
    let none = Span::default();
    let mut files: Vec<File<'static>> = w.files.as_slice()[..4].to_vec();
    files.push(file(b"a.o", origin(2), object(), none));
    for (name, offset) in [(&b"m1.o"[..], 8u64), (b"m2.o", 100)] {
        let mut m = file(b"lib.a", origin(3), object(), none);
        m.member = Some(Member { archive: ArchiveId(1), name, ordinal: 0, offset, thin: None });
        files.push(m);
    }
    w.files = Column::from_vec(files).unwrap();
    w.archives
        .push(Archive { members: IdRange::new(FileId(5), FileId(7)), ..w.archives[ArchiveId(0)] })
        .unwrap();
    let ids = w.file_identities();
    let at = |f: u32| {
        let id = ids[FileId(f)];
        (id.path, id.mention, id.member.map(|m| m.0))
    };
    assert_eq!(at(0), (&b""[..], 0, None));
    assert_eq!(at(1), (&b"a.o"[..], 0, None));
    assert_eq!(at(4), (&b"a.o"[..], 1, None));
    assert_eq!(at(2), (&b"lib.a"[..], 0, Some(&b"m1.o"[..])));
    assert_eq!(at(5), (&b"lib.a"[..], 1, Some(&b"m1.o"[..])));
    assert_eq!(at(6), (&b"lib.a"[..], 1, Some(&b"m2.o"[..])));
    assert_ne!(ids[FileId(2)], ids[FileId(5)], "two mentions are two files");
}

// spec: model.md §7.1 (the probes that chose a file are what an incremental link re-checks)
#[test]
fn s7_1_probe_checks() {
    use mortar_core::identity::ProbeCheck;
    use mortar_core::inputs::{Errno, TargetName};
    let mut w = world();
    let checks = w.probe_checks().unwrap();
    assert_eq!(
        checks,
        [
            ProbeCheck { path: b"/k/x.so", present: false },
            ProbeCheck { path: b"/l/x.so", present: true },
        ]
    );
    w.link.probes[0].outcome = ProbeOutcome::Skipped(TargetName::Elf { machine: 183, class: 2 });
    assert!(w.probe_checks().unwrap()[0].present, "a skipped candidate is there");
    w.link.probes[0].outcome = ProbeOutcome::Error(Errno(13));
    assert_eq!(w.probe_checks(), None);
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

// spec: model.md §3.3 rule 5 (comdat_copies: sorted, live; the keeper is the least live copy)
#[test]
fn s3_3_rule5_comdat_copies() {
    use mortar_core::id::ComdatId;
    let with = |copies: &[(u32, u32)], keepers: [Option<u32>; 2]| {
        let mut r = resolution();
        r.comdat = Column::from_vec(
            keepers.iter().map(|k| k.map_or(OptId::NONE, |f| OptId::some(FileId(f)))).collect(),
        )
        .unwrap();
        r.file_live[FileId(2)] = FileLive::Always;
        r.comdat_copies = copies.iter().map(|&(g, f)| (ComdatId(g), FileId(f))).collect();
        r
    };
    let r = with(&[(0, 1), (0, 2), (1, 2)], [Some(1), Some(2)]);
    assert_eq!(r.check(), Ok(()));
    assert_eq!(r.copies_of(ComdatId(0)), &[(ComdatId(0), FileId(1)), (ComdatId(0), FileId(2))]);
    assert_eq!(r.copies_of(ComdatId(1)).len(), 1);
    assert_eq!(with(&[], [None, None]).check(), Ok(()));
    for (copies, keepers) in [
        (&[(0, 2), (0, 1)][..], [Some(1), None]),
        (&[(0, 1), (0, 1)][..], [Some(1), None]),
        (&[(0, 1), (0, 2)][..], [Some(2), None]),
        (&[(0, 1)][..], [Some(1), Some(2)]),
        (&[(0, 1), (2, 1)][..], [Some(1), None]),
    ] {
        let err = with(copies, keepers).check().expect_err("a broken copy list");
        assert!(err.what.contains("rule 5"), "{copies:?}: {err:?}");
    }
    let mut dead = with(&[(0, 1), (0, 2)], [Some(1), None]);
    dead.file_live[FileId(2)] = FileLive::Dead;
    assert!(dead.check().is_err(), "a copy in a dead file");
}

// spec: model.md §3.3 rule 6 (merged properties: a strong reference makes an undefined key
// strong; a shared definition is preemptible)
#[test]
fn s3_3_rule6_merged_properties() {
    let mut r = resolution();
    r.props[SymbolId(1)].refs = RefFlags::STRONG | RefFlags::FROM_OBJECT;
    assert_eq!(r.check(), Ok(()));
    r.def[SymbolId(1)] = Def::Undefined { weak: true };
    assert!(r.check().unwrap_err().what.contains("rule 6"));
    r.props[SymbolId(1)].refs = RefFlags::FROM_OBJECT;
    assert_eq!(r.check(), Ok(()));
    r.def[SymbolId(1)] = Def::Shared(SymEntry { file: FileId(1), index: SymbolIndex(2) });
    assert!(r.check().unwrap_err().what.contains("rule 6"));
    r.props[SymbolId(1)].refs |= RefFlags::PREEMPTIBLE | RefFlags::DEFINED_SHARED;
    assert_eq!(r.check(), Ok(()));
    r.def[SymbolId(1)] = Def::Internal(InternalDef::Alias(SymbolId(2)));
    assert!(r.check().unwrap_err().what.contains("alias"));
    r.def[SymbolId(1)] = Def::Internal(InternalDef::Alias(SymbolId(0)));
    assert_eq!(r.check(), Ok(()));
}

/// A resolution of `world()`: every file live by its kind.
fn world_resolution() -> Resolution {
    let mut r = Resolution::empty(6, 0, 0);
    r.file_live = Column::from_vec(vec![
        FileLive::Always,
        FileLive::Always,
        FileLive::Extracted(SymEntry { file: FileId(1), index: SymbolIndex(3) }),
        FileLive::Dead,
        FileLive::Shared { needed: true },
        FileLive::Closure,
    ])
    .unwrap();
    r
}

// spec: model.md §3.3 (the checks against Inputs: liveness and definitions by kind of file)
#[test]
fn s3_3_resolution_against_inputs() {
    let mut w = world();
    let r = world_resolution();
    assert_eq!(r.check(), Ok(()));
    assert_eq!(r.check_against(&w), Ok(()));
    let rule = |r: &Resolution, w: &Inputs<'static>, rule: &str| {
        let err = r.check_against(w).expect_err(rule);
        assert!(err.what.starts_with(rule), "{rule}: {err:?}");
    };
    rule(&Resolution::empty(5, 0, 0), &w, "rule 1");
    let live = |f: u32, l: FileLive| {
        let mut r = world_resolution();
        r.file_live[FileId(f)] = l;
        r
    };
    let by = FileLive::Extracted(SymEntry { file: FileId(1), index: SymbolIndex(3) });
    rule(&live(1, FileLive::Shared { needed: false }), &w, "rule 2");
    rule(&live(5, FileLive::Shared { needed: true }), &w, "rule 2");
    rule(&live(4, FileLive::Closure), &w, "rule 2");
    rule(&live(1, by), &w, "rule 2");
    rule(&live(2, FileLive::Always), &w, "rule 2");
    w.files[FileId(2)].state = InputFlags::WHOLE_ARCHIVE;
    assert_eq!(live(2, FileLive::Always).check_against(&w), Ok(()));
    // A --start-lib object is extracted; Always only under --whole-archive.
    w.files[FileId(1)].state = InputFlags::LAZY;
    assert_eq!(live(1, by).check_against(&w), Ok(()));
    rule(&live(1, FileLive::Always), &w, "rule 2");
    w.files[FileId(1)].state = InputFlags::LAZY | InputFlags::WHOLE_ARCHIVE;
    assert_eq!(live(1, FileLive::Always).check_against(&w), Ok(()));
    // Definitions by the kind of their file.
    w.symbols.push(SymbolKey { name: b"f", version: None, hash: 0 }).unwrap();
    let def = |d: Def| {
        let mut r = live(1, FileLive::Always);
        r.def = Column::from_vec(vec![d]).unwrap();
        r.props = Column::filled(1, mortar_core::resolution::SymProps::default());
        r.props[SymbolId(0)].refs = RefFlags::PREEMPTIBLE;
        r.clash = Column::filled(1, false);
        r.redirect = Column::from_vec(vec![SymbolId(0)]).unwrap();
        r
    };
    let at = |f| SymEntry { file: FileId(f), index: SymbolIndex(1) };
    for ok in [Def::Defined(at(1)), Def::Common(at(2)), Def::Shared(at(4)), Def::Shared(at(5))] {
        assert_eq!(def(ok).check_against(&w), Ok(()), "{ok:?}");
    }
    for bad in [Def::Defined(at(4)), Def::Common(at(5)), Def::Shared(at(1))] {
        rule(&def(bad), &w, "rule 3");
    }
}

// spec: model.md §3.4 (withdrawn conditional linker-defined symbols are undefined after assignment)
#[test]
fn s3_4_withdrawn() {
    let mut r = resolution();
    r.def[SymbolId(1)] = Def::Internal(InternalDef::Provide { conditional: true });
    let mut a = assigned();
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
    assert_eq!(a.def(&r, SymbolId(1)), Some(r.def[SymbolId(1)]));
    a.withdrawn = Box::new([SymbolId(1)]);
    assert_eq!(a.check(&r), Ok(()));
    assert!(a.is_withdrawn(SymbolId(1)));
    assert_eq!(a.def(&r, SymbolId(1)), Some(Def::Undefined { weak: true }));
    r.props[SymbolId(1)].refs = RefFlags::STRONG;
    assert_eq!(a.def(&r, SymbolId(1)), Some(Def::Undefined { weak: false }));
    a.withdrawn = Box::new([SymbolId(0)]);
    assert!(a.check(&r).unwrap_err().what.contains("withdrawn"));
    r.def[SymbolId(1)] = Def::Internal(InternalDef::Provide { conditional: false });
    a.withdrawn = Box::new([SymbolId(1)]);
    assert!(a.check(&r).unwrap_err().what.contains("withdrawn"));
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
