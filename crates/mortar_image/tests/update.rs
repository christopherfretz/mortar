//! U-6's entry points' signatures, checked at compile time (`spec/incr.md`
//! IN-73, IN-75): an image `Link` is built by the `mortar` crate's front,
//! which this crate's tests cannot call; the tests over linked fixtures
//! are `crates/mortar/tests/update_image.rs` (WP-218).

use std::fs::File;

use mortar_core::incr::{IncrReason, LayoutDelta, RefillSet, StoredLayout, SymtabPlan};
use mortar_image::link::Link;
use mortar_image::map::{Map, update_map};
use mortar_image::update::{Refill as Refilled, RefillError, refill};

type Refill = fn(
    &Link<'_, '_>,
    &LayoutDelta,
    &SymtabPlan,
    &RefillSet,
    &StoredLayout<'_>,
    &File,
    &[[u8; 32]],
) -> Result<Refilled, RefillError>;
type UpdateMap = for<'a, 'b, 'c, 'd> fn(
    &Link<'a, 'b>,
    &LayoutDelta,
    &StoredLayout<'c>,
    &'d SymtabPlan,
) -> Result<Map, IncrReason>;

// spec: incr.md IN-73, IN-75 (until U-6: the signatures)
#[test]
fn refill_and_update_map_signatures() {
    let refill_fn: Refill = refill;
    let map_fn: UpdateMap = update_map;
    let _ = (refill_fn, map_fn);
}
