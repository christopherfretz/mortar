//! U-2's tests (`spec/incr.md` IN-18c..IN-18f, IN-46b, IN-78). WP-213
//! (U-1): each entry point's "until it lands" value; U-2 replaces them.

use mortar_core::incr::{IncrReason, MemoryLayout, StoredLayout, Unimplemented};
use mortar_incr::format::StatRow;
use mortar_incr::ops::{StdOps, WriteFailure};
use mortar_incr::state5;
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::StatePatch;

const STATE5: IncrReason = IncrReason::UpdateNotImplemented(Some(Unimplemented::State5));

// spec: incr.md IN-18c (until U-2: no format-5 reader)
#[test]
fn open_until_it_lands() {
    let file = std::fs::File::open(env!("CARGO_MANIFEST_DIR")).expect("a descriptor");
    assert_eq!(state5::open(&file, [0; 16]).err(), Some(STATE5));
}

// spec: incr.md IN-46b, IN-18f (until U-2: no plan, no patch, no compaction)
#[test]
fn plan_patch_and_compact_until_they_land() {
    let m = MemoryState::default();
    let view = m.view();
    assert_eq!(state5::plan_patch(&view, &StatePatch::default()).err(), Some(STATE5));
    let plan = state5::PatchPlan::default();
    assert!(!plan.compacts());
    let output = StatRow::default();
    let r = state5::patch(&view, &plan, &output, &mut StdOps);
    assert!(matches!(r, Err(WriteFailure::Fallback(ref e)) if *e == STATE5), "{r:?}");
    let never = std::path::Path::new("never-written");
    let r = state5::compact(&view, &plan, &output, &mut StdOps, never);
    assert!(matches!(r, Err(WriteFailure::Fallback(ref e)) if *e == STATE5), "{r:?}");
}

// spec: incr.md IN-78 (until U-2: canon refuses, compare finds nothing)
#[test]
fn canon_until_it_lands() {
    assert!(matches!(state5::canon(b""), Err(mortar_incr::format::StateError::Version(_))));
    let rows = MemoryLayout::default();
    let p_u = StoredLayout::new(&rows);
    let c = state5::Canon::default();
    assert!(state5::compare(&c, &c, &p_u).is_empty());
}

// spec: incr.md IN-18c (until U-2: the stored layout of a state is not read)
#[test]
fn layout_of_until_it_lands() {
    let m = MemoryState::default();
    let view = m.view();
    let of = state5::LayoutOf { view: &view };
    let stored = StoredLayout::new(&of);
    assert_eq!(stored.chunks().err(), Some(STATE5));
}
