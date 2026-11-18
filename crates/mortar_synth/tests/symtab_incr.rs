//! U-5's tests (`spec/incr.md` IN-49a). WP-213 (U-1): the entry point's
//! "until it lands" value; U-5 replaces it.

use mortar_core::incr::{IncrReason, LayoutDelta, MemoryLayout, StoredLayout, Unimplemented};
use mortar_synth::symtab_incr::plan;

// spec: incr.md IN-49a (until U-5)
#[test]
fn plan_until_it_lands() {
    let rows = MemoryLayout::default();
    let stored = StoredLayout::new(&rows);
    assert_eq!(
        plan(&stored, &LayoutDelta::default(), &[], &[]).err(),
        Some(IncrReason::UpdateNotImplemented(Some(Unimplemented::Symtab)))
    );
}
