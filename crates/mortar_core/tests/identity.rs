//! `spec/model.md` §7: a table written in identity terms in one link and
//! read back in a link whose ids differ (an input added in front; a
//! function added in the middle of a file) denotes the same rows.

use mortar_core::assignment::{Assignment, OutSec};
use mortar_core::column::Column;
use mortar_core::demands::DemandFlags;
use mortar_core::id::{ChunkId, FileId, IdRange, LocalSym, OutSecId, SectionId, SymKey, SymbolId};
use mortar_core::identity::{
    AllocIdentity, Anchor, ChunkTag, Dependee, FileIdentity, Identities, KeyIdentity, Ordinals,
    SectionIdentity, SectionKey, flag_dependees, outsec_id, outsec_identity, outsec_ordinals,
    patch_at, patch_identity, symbol_id, symbol_identity, symbol_lookup,
};
use mortar_core::inputs::SymbolKey;
use mortar_core::layout::{AllocKey, CopyAlloc, Erratum, Patch, PatchKind, Placement, Slots};
use mortar_core::resolution::{FileLive, Resolution};
use mortar_elf::SymbolIndex;

/// One file of a link: its identity, its sections' keys, its locals' names.
struct FileSpec {
    path: &'static [u8],
    member: Option<(&'static [u8], u32)>,
    sections: Vec<SectionKey<'static>>,
    locals: Vec<&'static [u8]>,
}

fn key(anchor: Anchor<'static>, name: &'static [u8]) -> SectionKey<'static> {
    SectionKey { anchor, name }
}

fn internal() -> FileSpec {
    FileSpec { path: b"", member: None, sections: vec![], locals: vec![] }
}

/// `a.o` with one function per section; `extra` adds `h` between `f` and `g`.
fn a_o(extra: bool) -> FileSpec {
    let mut sections = vec![
        key(Anchor::None, b""),
        key(Anchor::Global(b"f"), b".text.f"),
        key(Anchor::Global(b"g"), b".text.g"),
        key(Anchor::Local(b"counter"), b".data"),
        key(Anchor::None, b".rodata"),
        key(Anchor::None, b".rodata"),
        key(Anchor::None, b".symtab"),
    ];
    let mut locals: Vec<&'static [u8]> = vec![b"", b"a.c", b"tmp", b"counter", b"tmp"];
    if extra {
        sections.insert(2, key(Anchor::Global(b"h"), b".text.h"));
        locals.insert(2, b"scratch");
    }
    FileSpec { path: b"a.o", member: None, sections, locals }
}

/// Two members of `libx.a` with the same name.
fn member(ordinal: u32) -> FileSpec {
    FileSpec {
        path: b"libx.a",
        member: Some((b"x.o", ordinal)),
        sections: vec![key(Anchor::None, b""), key(Anchor::Global(b"x"), b".text")],
        locals: vec![b""],
    }
}

/// A link: identities and the section numbering of `Resolution`.
struct Link {
    ids: Identities<'static>,
    resolution: Resolution,
}

fn link(files: &[FileSpec]) -> Link {
    let n = files.len() as u32;
    let mut resolution = Resolution::empty(n, 0, 0);
    resolution.file_live = Column::from_vec(vec![FileLive::Always; files.len()]).unwrap();
    let mut base = 0;
    for (i, f) in files.iter().enumerate() {
        let len = f.sections.len() as u32;
        resolution.section_base[FileId(i as u32)] =
            IdRange::new(SectionId(base), SectionId(base + len));
        base += len;
    }
    let identities =
        files.iter().map(|f| FileIdentity { path: f.path, mention: 0, member: f.member }).collect();
    let sections = files.iter().map(|f| Ordinals::new(f.sections.clone())).collect();
    let locals = files.iter().map(|f| Ordinals::new(f.locals.clone())).collect();
    let ids = Identities::new(
        Column::from_vec(identities).unwrap(),
        Column::from_vec(sections).unwrap(),
        Column::from_vec(locals).unwrap(),
    );
    Link { ids, resolution }
}

fn first() -> Link {
    link(&[internal(), a_o(false), member(0), member(1)])
}

/// An input added in front, and a function added in the middle of `a.o`.
fn second() -> Link {
    let new = FileSpec {
        path: b"new.o",
        member: None,
        sections: vec![key(Anchor::None, b"")],
        locals: vec![b""],
    };
    link(&[internal(), new, a_o(true), member(0), member(1)])
}

// spec: model.md §7.1 (file identity: path, member name and ordinal)
#[test]
fn s7_files_survive_an_input_added_in_front() {
    let (one, two) = (first(), second());
    for (file, want) in [(1, 2), (2, 3), (3, 4), (0, 0)] {
        let identity = one.ids.file(FileId(file)).unwrap();
        assert_eq!(two.ids.file_id(&identity), Some(FileId(want)));
    }
    let gone = FileIdentity { path: b"gone.o", mention: 0, member: None };
    assert_eq!(two.ids.file_id(&gone), None);
}

// spec: model.md §7 (a table in identity terms read back under another numbering)
#[test]
fn s7_sections_survive_a_function_added_in_the_middle() {
    let (one, two) = (first(), second());
    // A column of the first link, written in identity terms...
    let total = one.resolution.section_base.as_slice().last().unwrap().end.0;
    let written: Vec<(SectionIdentity<'static>, u64)> = (0..total)
        .map(SectionId)
        .map(|s| (one.ids.section(&one.resolution, s).unwrap(), 0x1000 + u64::from(s.0)))
        .collect();
    assert_eq!(written.len(), 7 + 2 + 2);
    // ...read back in the second.
    let mut read: Vec<Option<u64>> = vec![None; 1 + 8 + 2 + 2];
    for (identity, value) in &written {
        let s = two.ids.section_id(&two.resolution, identity).expect("the section is found");
        assert_eq!(two.ids.section(&two.resolution, s).as_ref(), Some(identity));
        read[s.0 as usize] = Some(*value);
    }
    // a.o: sections 1..9 in the second link; its .text.h (index 2) is new.
    let a = |index: u32| 1 + index as usize;
    assert_eq!(read[a(1)], Some(0x1001), ".text.f");
    assert_eq!(read[a(2)], None, ".text.h is new");
    assert_eq!(read[a(3)], Some(0x1002), ".text.g");
    assert_eq!(read[a(4)], Some(0x1003), ".data");
    assert_eq!(read[a(5)], Some(0x1004), "first .rodata");
    assert_eq!(read[a(6)], Some(0x1005), "second .rodata");
    // The archive members keep their sections, by member ordinal.
    assert_eq!(read[10], Some(0x1008));
    assert_eq!(read[12], Some(0x100a));
    assert_eq!(read[0], None, "new.o is new");
}

// spec: model.md §7.1 (local symbol: name and ordinal, never the index)
#[test]
fn s7_locals_survive_by_name_and_ordinal() {
    let (one, two) = (first(), second());
    let sym = |file, index| LocalSym { file: FileId(file), index: SymbolIndex(index) };
    let second_tmp = one.ids.local(sym(1, 4)).unwrap();
    assert_eq!((second_tmp.name, second_tmp.ordinal), (&b"tmp"[..], 1));
    assert_eq!(two.ids.local_sym(&second_tmp), Some(sym(2, 5)));
    let counter = one.ids.local(sym(1, 3)).unwrap();
    assert_eq!(two.ids.local_sym(&counter), Some(sym(2, 4)));
    assert_eq!(two.ids.local(sym(2, 4)), Some(counter));
    let scratch = two.ids.local(sym(2, 2)).unwrap();
    assert_eq!(one.ids.local_sym(&scratch), None, "the new local has no row in the first link");
}

// spec: model.md §7.1 (global symbol: key bytes)
#[test]
fn s7_global_symbols_by_key() {
    let k =
        |name: &'static [u8], version: Option<&'static [u8]>| SymbolKey { name, version, hash: 0 };
    let one =
        Column::from_vec(vec![k(b"main", None), k(b"f", None), k(b"f", Some(b"V1"))]).unwrap();
    let two = Column::from_vec(vec![
        k(b"new", None),
        k(b"f", Some(b"V1")),
        k(b"main", None),
        k(b"f", None),
    ])
    .unwrap();
    let lookup = symbol_lookup(&two);
    for (id, want) in [(0, 2), (1, 3), (2, 1)] {
        let identity = symbol_identity(&one, SymbolId(id)).unwrap();
        assert_eq!(symbol_id(&two, &lookup, &identity), Some(SymbolId(want)));
    }
}

// spec: model.md §7.1 (output section: name, and an ordinal among same-named ones)
#[test]
fn s7_output_sections_by_name() {
    let out = |name: &'static [u8]| OutSec {
        name,
        sh_type: 1,
        flags: 0,
        merge: None,
        keep: false,
        fill: None,
        constraint: None,
        script_position: None,
    };
    let mut one = Assignment::empty(0);
    one.outsecs = Column::from_vec(vec![out(b".text"), out(b".data"), out(b".data")]).unwrap();
    let mut two = Assignment::empty(0);
    two.outsecs =
        Column::from_vec(vec![out(b".init"), out(b".text"), out(b".data"), out(b".data")]).unwrap();
    let (o1, o2) = (outsec_ordinals(&one), outsec_ordinals(&two));
    for (id, want) in [(0, 1), (1, 2), (2, 3)] {
        let identity = outsec_identity(&o1, OutSecId(id)).unwrap();
        assert_eq!(outsec_id(&o2, &identity), Some(OutSecId(want)));
    }
}

// ---- §7.2: the dependees of the dependency index ----------------------------

fn sym_key(name: &'static [u8]) -> SymbolKey<'static> {
    SymbolKey { name, version: None, hash: 0 }
}

// spec: model.md §7.2 (keys, global and local, survive a renumbering)
#[test]
fn s7_2_keys_survive() {
    let (one, two) = (first(), second());
    let g1 = Column::from_vec(vec![sym_key(b"main"), sym_key(b"f")]).unwrap();
    let g2 = Column::from_vec(vec![sym_key(b"new"), sym_key(b"f"), sym_key(b"main")]).unwrap();
    let lookup = symbol_lookup(&g2);
    let local =
        |file, index| SymKey::local(LocalSym { file: FileId(file), index: SymbolIndex(index) });
    for (key, want) in [
        (SymKey::global(SymbolId(0)), SymKey::global(SymbolId(2))),
        (SymKey::global(SymbolId(1)), SymKey::global(SymbolId(1))),
        (local(1, 4), local(2, 5)),
        (local(1, 3), local(2, 4)),
    ] {
        let identity = one.ids.key(&g1, key).unwrap();
        assert!(matches!(
            (identity, key.as_global()),
            (KeyIdentity::Global(_), Some(_)) | (KeyIdentity::Local(_), None)
        ));
        assert_eq!(two.ids.key_id(&g2, &lookup, &identity), Some(want), "{key:?}");
    }
}

// spec: model.md §7.2 (a redirected site: a section and an offset in it)
#[test]
fn s7_2_sites_survive() {
    let (one, two) = (first(), second());
    // .data of a.o: section 3 in the first link, 5 in the second.
    let site = one.ids.site(&one.resolution, SectionId(3), 0x14).unwrap();
    assert_eq!(site.offset, 0x14);
    assert_eq!(two.ids.site_id(&two.resolution, &site), Some((SectionId(5), 0x14)));
    let new = two.ids.site(&two.resolution, SectionId(3), 0).unwrap();
    assert_eq!(new.section.key.name, b".text.h");
    assert_eq!(one.ids.site_id(&one.resolution, &new), None, ".text.h is new");
}

// spec: model.md §7.2 (a patch: its chunk and its offset there)
#[test]
fn s7_2_patches_survive() {
    let thunk = |chunk, offset| Patch {
        chunk: ChunkId(chunk),
        offset,
        kind: PatchKind::Erratum { erratum: Erratum::E843419, section: SectionId(3), offset: 8 },
    };
    // Chunk keys: in the second link a patch chunk of another output section
    // comes first, so the `.text` patch chunk moves from 1 to 2.
    let first = Ordinals::new(vec![
        (ChunkTag::Body, Some(&b".text"[..])),
        (ChunkTag::Patches, Some(&b".text"[..])),
    ]);
    let second = Ordinals::new(vec![
        (ChunkTag::Patches, Some(&b".init"[..])),
        (ChunkTag::Body, Some(&b".text"[..])),
        (ChunkTag::Patches, Some(&b".text"[..])),
    ]);
    let placement = Placement {
        patches: Box::new([thunk(0, 0), thunk(2, 0), thunk(2, 0x10)]),
        ..Placement::default()
    };
    let identity = patch_identity(&first, &thunk(1, 0x10)).unwrap();
    assert_eq!(identity.offset, 0x10);
    assert_eq!(patch_at(&second, &placement, &identity), Some(2));
    let gone = patch_identity(&first, &thunk(1, 0x20)).unwrap();
    assert_eq!(patch_at(&second, &placement, &gone), None);
}

// spec: model.md §7.2 (a symbol-anchored allocation: a common's key, or a copy
// allocation's shared object and value)
#[test]
fn s7_2_allocations_survive() {
    let (one, two) = (first(), second());
    let g1 = Column::from_vec(vec![sym_key(b"buf")]).unwrap();
    let g2 = Column::from_vec(vec![sym_key(b"new"), sym_key(b"buf")]).unwrap();
    let lookup = symbol_lookup(&g2);
    let s1 = Slots { copies: Box::new([CopyAlloc::at(FileId(3), 0x40)]), ..Slots::default() };
    let s2 = Slots {
        copies: Box::new([CopyAlloc::at(FileId(3), 0x10), CopyAlloc::at(FileId(4), 0x40)]),
        ..Slots::default()
    };
    let common = one.ids.alloc(&g1, &s1, AllocKey::Common(SymbolId(0))).unwrap();
    assert_eq!(two.ids.alloc_key(&g2, &lookup, &s2, &common), Some(AllocKey::Common(SymbolId(1))));
    let copy = one.ids.alloc(&g1, &s1, AllocKey::Copy(0)).unwrap();
    let AllocIdentity::Copy { file, value } = copy else { panic!("{copy:?}") };
    assert_eq!((file.member, value), (Some((&b"x.o"[..], 1)), 0x40));
    assert_eq!(two.ids.alloc_key(&g2, &lookup, &s2, &copy), Some(AllocKey::Copy(1)));
    assert_eq!(one.ids.alloc(&g1, &s1, AllocKey::Copy(1)), None);
}

// spec: model.md §7.2 (one dependee per cross-file flag of a key: GOT_NORELAX,
// TLSIE_NORELAX, CANON_PLT, COPY; no other flag)
#[test]
fn s7_2_flag_dependees() {
    let key = KeyIdentity::Global(
        symbol_identity(&Column::from_vec(vec![sym_key(b"f")]).unwrap(), SymbolId(0)).unwrap(),
    );
    let all = DemandFlags::from_bits(0x7ff).unwrap();
    let flags: Vec<DemandFlags> = flag_dependees(key, all)
        .map(|d| match d {
            Dependee::Flag { key: k, flag } => {
                assert_eq!(k, key);
                flag
            }
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        flags,
        [
            DemandFlags::COPY,
            DemandFlags::CANON_PLT,
            DemandFlags::GOT_NORELAX,
            DemandFlags::TLSIE_NORELAX
        ]
    );
    let none = DemandFlags::GOT | DemandFlags::PLT | DemandFlags::IPLT | DemandFlags::EXPORT;
    assert_eq!(flag_dependees(key, none).count(), 0);
}
