//! `spec/model.md` §1, §2, §6 rule 1, §9: id arithmetic, the order of
//! `SymKey`, `Column::split_by`, the row sizes.

use mortar_core::column::{Column, SplitError};
use mortar_core::id::{
    FileId, IdOverflow, IdRange, LocalSym, MAX_ROWS, OptId, SectionId, SymKey, SymKind, SymbolId,
    row_count,
};
use mortar_diag::RefusalClass;
use mortar_elf::SymbolIndex;

fn local(file: u32, index: u32) -> SymKey {
    SymKey::local(LocalSym { file: FileId(file), index: SymbolIndex(index) })
}

fn range(start: u32, end: u32) -> IdRange<SectionId> {
    IdRange::new(SectionId(start), SectionId(end))
}

// spec: model.md §2 (id arithmetic over ranges)
#[test]
fn s2_id_ranges() {
    let r = range(4, 9);
    assert_eq!(r.len(), 5);
    assert!(r.contains(SectionId(4)) && r.contains(SectionId(8)));
    assert!(!r.contains(SectionId(9)) && !r.contains(SectionId(3)));
    assert_eq!(r.nth(0), Some(SectionId(4)));
    assert_eq!(r.nth(4), Some(SectionId(8)));
    assert_eq!(r.nth(5), None);
    assert_eq!(r.offset_of(SectionId(6)), Some(2));
    assert_eq!(r.offset_of(SectionId(9)), None);
    assert_eq!(r.iter().collect::<Vec<_>>(), (4..9).map(SectionId).collect::<Vec<_>>());
    assert!(range(7, 3).is_empty());
    assert_eq!(IdRange::from_len(SectionId(10), 3), Some(range(10, 13)));
    assert_eq!(IdRange::from_len(SectionId(u32::MAX - 1), 2), None);
}

// spec: model.md §1 (ids are u32; more than 2^32 - 1 rows is refused by name)
#[test]
fn s1_id_limit() {
    assert_eq!(MAX_ROWS, u32::MAX);
    assert_eq!(row_count(5), Ok(5));
    assert_eq!(row_count(u32::MAX as usize), Ok(u32::MAX));
    if let Ok(len) = usize::try_from(u64::from(u32::MAX) + 1) {
        assert_eq!(row_count(len), Err(IdOverflow { rows: len }));
    }
    let diag = IdOverflow { rows: 0 }.diagnostic("sections");
    assert_eq!(diag.refusal(), Some((RefusalClass::Feature, "more than 2^32 - 1 sections")));
    // u32::MAX is never an id, so it can mean "none".
    assert_eq!(OptId::<SectionId>::NONE.get(), None);
    assert_eq!(OptId::some(SectionId(u32::MAX - 1)).get(), Some(SectionId(u32::MAX - 1)));
    assert_eq!(OptId::from(Some(SectionId(3))), OptId::some(SectionId(3)));
}

// spec: model.md §2 (SymKey: globals first, then locals by (FileId, index))
#[test]
fn s2_symkey_order() {
    let keys = [
        SymKey::global(SymbolId(0)),
        SymKey::global(SymbolId(7)),
        SymKey::global(SymbolId(u32::MAX - 1)),
        local(0, 0),
        local(0, u32::MAX),
        local(1, 0),
        local(1, 5),
        local(u32::MAX - 1, u32::MAX),
    ];
    for pair in keys.windows(2) {
        assert!(pair[0] < pair[1], "{pair:?}");
    }
    assert_eq!(keys[1].kind(), SymKind::Global(SymbolId(7)));
    assert_eq!(keys[6].kind(), SymKind::Local(LocalSym { file: FileId(1), index: SymbolIndex(5) }));
    assert_eq!(
        keys[7].kind(),
        SymKind::Local(LocalSym { file: FileId(u32::MAX - 1), index: SymbolIndex(u32::MAX) })
    );
    assert_eq!(keys[0].as_global(), Some(SymbolId(0)));
    assert_eq!(keys[3].as_global(), None);
    let mut shuffled = [keys[5], keys[0], keys[7], keys[3], keys[2], keys[6], keys[1], keys[4]];
    shuffled.sort();
    assert_eq!(shuffled, keys);
}

fn column() -> Column<SectionId, u32> {
    Column::from_vec((0..10).collect()).unwrap()
}

// spec: model.md §6 rule 1 (split_by: one disjoint mutable slice per unit)
#[test]
fn s6_1_split_by_valid_ranges() {
    let mut c = column();
    let parts = c.split_by(&[range(0, 3), range(3, 3), range(3, 5), range(7, 10)]).unwrap();
    assert_eq!(parts.len(), 4);
    assert_eq!(&*parts[0], &[0, 1, 2]);
    assert!(parts[1].is_empty());
    assert_eq!(&*parts[2], &[3, 4]);
    assert_eq!(&*parts[3], &[7, 8, 9]);
    for part in parts {
        for cell in part {
            *cell += 100;
        }
    }
    assert_eq!(c.as_slice(), &[100, 101, 102, 103, 104, 5, 6, 107, 108, 109]);
    assert_eq!(c.split_by(&[]).unwrap().len(), 0);
}

// spec: model.md §6 rule 1 (split_by refuses overlapping, out-of-order and out-of-bounds ranges)
#[test]
fn s6_1_split_by_refuses_bad_ranges() {
    let mut c = column();
    assert_eq!(
        c.split_by(&[range(0, 4), range(3, 6)]).unwrap_err(),
        SplitError::NotAscending { index: 1 }
    );
    assert_eq!(
        c.split_by(&[range(5, 6), range(1, 2)]).unwrap_err(),
        SplitError::NotAscending { index: 1 }
    );
    assert_eq!(
        c.split_by(&[range(0, 2), range(8, 11)]).unwrap_err(),
        SplitError::OutOfBounds { index: 1 }
    );
    assert_eq!(c.split_by(&[range(11, 11)]).unwrap_err(), SplitError::OutOfBounds { index: 0 });
    assert_eq!(c.as_slice(), column().as_slice());
}

// spec: model.md §10 (a column is a Vec indexed by an id, iterated in id order)
#[test]
fn s10_column() {
    let mut c: Column<SectionId, &str> = Column::new();
    assert_eq!(c.push("a"), Ok(SectionId(0)));
    assert_eq!(c.push("b"), Ok(SectionId(1)));
    assert_eq!(c[SectionId(1)], "b");
    assert_eq!(c.get(SectionId(2)), None);
    assert_eq!(c.ids(), range(0, 2));
    assert_eq!(c.iter().collect::<Vec<_>>(), [(SectionId(0), &"a"), (SectionId(1), &"b")]);
    assert_eq!(c.range(range(1, 2)), Some(&["b"][..]));
    assert_eq!(c.range(range(1, 3)), None);
}

// spec: model.md §9 (every row type states its size; these are the record's numbers)
#[test]
fn s9_row_sizes() {
    use mortar_core::assignment::OutOf;
    use mortar_core::demands::DemandFlags;
    use mortar_core::layout::Place;
    use mortar_core::resolution::{Def, FileLive, SectionRow, SymProps};
    use mortar_core::split::{EhPiece, Frag, SplitRow};
    let sizes = [
        ("SymKey", size_of::<SymKey>(), 8),
        ("OptId", size_of::<OptId<SectionId>>(), 4),
        ("IdRange", size_of::<IdRange<SectionId>>(), 8),
        ("FileLive", size_of::<FileLive>(), 12),
        ("Def", size_of::<Def>(), 12),
        ("SymProps", size_of::<SymProps>(), 6),
        ("SectionRow", size_of::<SectionRow>(), 12),
        ("OutOf", size_of::<OutOf>(), 8),
        ("SplitRow", size_of::<SplitRow>(), 12),
        ("Frag", size_of::<Frag>(), 16),
        ("EhPiece", size_of::<EhPiece>(), 8),
        ("DemandFlags", size_of::<DemandFlags>(), 2),
        ("Place", size_of::<Place>(), 12),
        ("sym_map cell", size_of::<SymbolId>(), 4),
        ("merge piece", size_of::<u32>() + size_of::<mortar_core::FragId>(), 8),
    ];
    for (name, size, want) in sizes {
        assert_eq!(size, want, "{name}");
    }
}
