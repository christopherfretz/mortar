//! `spec/resolve.md` §10.1 P-1 and `spec/model.md` §4.5's invariant (no
//! place is never a number; WP-136): the place of a definition, decided by
//! `mortar_core::place::section_place` from the section's state, its
//! assignment and its liveness, on constructed tables.

use mortar_core::assignment::{Assignment, OutOf};
use mortar_core::column::Column;
use mortar_core::id::{OptId, OutSecId, SectionId};
use mortar_core::liveness::Liveness;
use mortar_core::place::{NoPlace, section_place};
use mortar_core::resolution::{DropReason, Resolution, SectionKind, SectionRow, SectionState};
use mortar_elf::SectionIndex;

fn row(kind: SectionKind, state: SectionState) -> SectionRow {
    SectionRow { kind, state, reloc: SectionIndex(0), comdat: OptId::NONE }
}

/// Six sections: 0 a placed `Regular` one; 1 a kept `Note` consumed by
/// the linker (`.note.gnu.property`); 2 an `SHF_EXCLUDE` one; 3 a kept
/// `Regular` one `--gc-sections` collected; 4 a dropped COMDAT copy; 5 a
/// kept `Regular` one assigned nowhere (a broken assignment).
fn tables() -> (Resolution, Assignment<'static>, Liveness) {
    let mut r = Resolution::empty(1, 0, 0);
    r.sections = Column::from_vec(vec![
        row(SectionKind::Regular, SectionState::Kept),
        row(SectionKind::Note, SectionState::Kept),
        row(SectionKind::Excluded, SectionState::Dropped(DropReason::Excluded)),
        row(SectionKind::Regular, SectionState::Kept),
        row(SectionKind::Regular, SectionState::Dropped(DropReason::Comdat)),
        row(SectionKind::Regular, SectionState::Kept),
    ])
    .unwrap();
    let mut a = Assignment::empty(6);
    a.out_of[SectionId(0)] = OutOf::Out(OutSecId(0));
    a.out_of[SectionId(1)] = OutOf::Consumed;
    a.out_of[SectionId(3)] = OutOf::Out(OutSecId(0));
    let mut l = Liveness::empty(6, 0);
    l.live[SectionId(0)] = true;
    l.live[SectionId(1)] = true;
    l.live[SectionId(5)] = true;
    (r, a, l)
}

// spec: resolve.md §10.1 P-1 (a kept, live, assigned section has a place)
#[test]
fn p1_placed_section_has_a_place() {
    let (r, a, l) = tables();
    assert_eq!(section_place(&r, Some(&a), &l, SectionId(0)), Ok(()));
}

// spec: resolve.md §10.1 P-1; model.md §4.5 invariant (NEGATIVE: every
// placeless definition is a typed error naming its rule, never a value)
#[test]
fn p1_placeless_sections_are_typed_errors() {
    let (r, a, l) = tables();
    let at = |s| section_place(&r, Some(&a), &l, SectionId(s));
    // The mislink of WP-136: kept and live, but consumed (no output).
    assert_eq!(at(1), Err(NoPlace::Consumed));
    assert_eq!(at(2), Err(NoPlace::Dropped(DropReason::Excluded)));
    assert_eq!(at(3), Err(NoPlace::Collected));
    assert_eq!(at(4), Err(NoPlace::Dropped(DropReason::Comdat)));
    assert_eq!(at(5), Err(NoPlace::Unassigned));
    // Outside the table: not a section of a live object.
    assert_eq!(at(6), Err(NoPlace::Unknown));
}

// spec: resolve.md §10.1 P-1 (without an assignment, a phase before
// layout's asks the state and liveness only)
#[test]
fn p1_without_assignment() {
    let (r, _, l) = tables();
    let at = |s| section_place(&r, None, &l, SectionId(s));
    assert_eq!(at(0), Ok(()));
    assert_eq!(at(1), Ok(()));
    assert_eq!(at(2), Err(NoPlace::Dropped(DropReason::Excluded)));
    assert_eq!(at(3), Err(NoPlace::Collected));
}

// spec: resolve.md §10.1 P-1 (a script's /DISCARD/ is a dropped section)
#[test]
fn p1_script_discard() {
    let (mut r, mut a, l) = tables();
    r.sections[SectionId(5)].state = SectionState::Dropped(DropReason::Script);
    a.out_of[SectionId(5)] = OutOf::Discard;
    assert_eq!(
        section_place(&r, Some(&a), &l, SectionId(5)),
        Err(NoPlace::Dropped(DropReason::Script))
    );
    let mut a2 = a;
    a2.out_of[SectionId(0)] = OutOf::Discard;
    assert_eq!(
        section_place(&r, Some(&a2), &l, SectionId(0)),
        Err(NoPlace::Dropped(DropReason::Script))
    );
}
