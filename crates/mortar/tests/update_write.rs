//! U-7's tests (`spec/incr.md` IN-76, IN-77). WP-213 (U-1): the entry
//! point's "until it lands" value; U-7 replaces it.

use mortar::update::write::{Pair, write};
use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_incr::WRITE_PATH;
use mortar_incr::ops::{RecordingOps, WriteFailure};
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::StatePatch;

// spec: incr.md IN-77 (until U-7: nothing is written)
#[test]
fn write_until_it_lands() {
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = here.join("Cargo.toml");
    let file = std::fs::File::open(&manifest).expect("a descriptor");
    let pair = Pair { out: &file, out_path: &manifest, state: &file, state_path: &manifest };
    let m = MemoryState::default();
    let mut ops = RecordingOps::default();
    let r = write(&pair, &m.view(), &[], &[], &StatePatch::default(), &mut ops, WRITE_PATH);
    let want = IncrReason::UpdateNotImplemented(Some(Unimplemented::Write));
    assert!(matches!(r, Err(WriteFailure::Fallback(ref e)) if *e == want), "{r:?}");
    assert!(ops.log.is_empty());
}
