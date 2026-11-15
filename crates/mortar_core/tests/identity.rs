//! `spec/model.md` §7: a table written in identity terms in one link and
//! read back in a link whose ids differ (an input added in front; a
//! function added in the middle of a file) denotes the same rows.

use mortar_core::assignment::{Assignment, OutSec};
use mortar_core::column::Column;
use mortar_core::id::{FileId, IdRange, LocalSym, OutSecId, SectionId, SymbolId};
use mortar_core::identity::{
    Anchor, FileIdentity, Identities, Ordinals, SectionIdentity, SectionKey, outsec_id,
    outsec_identity, outsec_ordinals, symbol_id, symbol_identity, symbol_lookup,
};
use mortar_core::inputs::SymbolKey;
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
        files.iter().map(|f| FileIdentity { path: f.path, member: f.member }).collect();
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
    let gone = FileIdentity { path: b"gone.o", member: None };
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
