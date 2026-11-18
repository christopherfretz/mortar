//! U-3's tests (`spec/incr.md` IN-71, IN-71a..c, IN-74). WP-213 (U-1):
//! each entry point's "until it lands" value; U-3 replaces them.

use mortar_core::incr::{IncrReason, LayoutDelta, Unimplemented};
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::ChangedFiles;
use mortar_incr::update::{defined, extract, live, refill, resolve};

const RESOLVE: IncrReason = IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve));

// spec: incr.md IN-71, IN-71a, IN-71b (until U-3)
#[test]
fn resolve_extract_live_until_they_land() {
    let m = MemoryState::default();
    let view = m.view();
    let changed = ChangedFiles::default();
    assert_eq!(extract::extract(&view, &changed).err(), Some(RESOLVE));
    let extraction = extract::Extraction::default();
    assert_eq!(resolve::resolve(&view, &changed, &extraction).err(), Some(RESOLVE));
    let resolved = resolve::ResolveDelta::default();
    assert_eq!(live::live(&view, &changed, &resolved).err(), Some(RESOLVE));
}

// spec: incr.md IN-71c, IN-74 (until U-3)
#[test]
fn recompute_and_refill_set_until_they_land() {
    let m = MemoryState::default();
    let view = m.view();
    let layout = LayoutDelta::default();
    assert!(defined::recompute(&view, &layout).is_empty());
    let resolved = resolve::ResolveDelta::default();
    assert_eq!(
        refill::refill_set(&view, &resolved, &layout, &[]).err(),
        Some(IncrReason::UpdateNotImplemented(Some(Unimplemented::Refill)))
    );
}
