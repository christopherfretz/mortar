//! U-6's tests (`spec/incr.md` IN-73, IN-75). WP-213 (U-1): the entry
//! points' signatures, checked at compile time: an image `Link` is built by
//! the `mortar` crate's front (`mortar::image::with_link`), which this
//! crate's tests cannot call, so the "until it lands" values
//! (`update-not-implemented (image)`; the full link's `Map`) are not run
//! here. U-6 replaces this with tests over a full link's image.

use std::fs::File;

use mortar_core::incr::{DirtyRange, LayoutDelta, RefillSet, SymtabPlan};
use mortar_image::link::Link;
use mortar_image::map::{Map, update_map};
use mortar_image::update::{RefillError, refill};

type Refill = fn(
    &Link<'_, '_>,
    &LayoutDelta,
    &SymtabPlan,
    &RefillSet,
    &File,
) -> Result<(Vec<DirtyRange>, Vec<u8>), RefillError>;
type UpdateMap = fn(&Link<'_, '_>, &LayoutDelta, &SymtabPlan) -> Map;

// spec: incr.md IN-73, IN-75 (until U-6: the signatures)
#[test]
fn refill_and_update_map_signatures() {
    let refill_fn: Refill = refill;
    let map_fn: UpdateMap = update_map;
    let _ = (refill_fn, map_fn);
}
