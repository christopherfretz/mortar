//! U-3's tests (`spec/incr.md` IN-71, IN-71a..c, IN-74). WP-213 (U-1):
//! each entry point's "until it lands" value; U-3 replaces them.

use mortar_core::incr::{IncrReason, LayoutDelta, OptionKeys, Unimplemented};
use mortar_incr::format::Str;
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
    let options = OptionKeys::default();
    assert_eq!(extract::extract(&view, &changed, &options).err(), Some(RESOLVE));
    let extraction = extract::Extraction::default();
    assert_eq!(resolve::resolve(&view, &changed, &extraction, &options).err(), Some(RESOLVE));
}

// spec: incr.md IN-71a (not built: WP-238): without `--gc-sections` the
// empty delta, with it `update-not-implemented (live)` from the option
// alone (WP-215 seam 3, G3)
#[test]
fn live_is_empty_without_gc_and_falls_back_under_it() {
    let m = MemoryState::default();
    let view = m.view();
    let changed = ChangedFiles::default();
    let resolved = resolve::ResolveDelta::default();
    let mut options = OptionKeys::default();
    assert_eq!(live::live(&view, &changed, &resolved, &options), Ok(live::LiveDelta::default()));
    options.gc_sections = true;
    assert_eq!(
        live::live(&view, &changed, &resolved, &options).err(),
        Some(IncrReason::UpdateNotImplemented(Some(Unimplemented::Live)))
    );
}

// spec: incr.md IN-70 (WP-215 seam 3, G2): one reader for a `Str` of the
// update's id space; equality by bytes
#[test]
fn changed_files_string_reads_state_and_buffer() {
    let mut m = MemoryState::default();
    let a = m.intern(b".text.f");
    let view = m.view();
    let changed = ChangedFiles { strings: b".text.f.data".to_vec(), ..ChangedFiles::default() };
    let n = a.len;
    assert_eq!(changed.string(&view, a).unwrap(), b".text.f");
    // The same bytes at the buffer's offset: another `Str`, equal bytes.
    let b = Str { off: n, len: n };
    assert_ne!(a, b);
    assert_eq!(changed.string(&view, b).unwrap(), changed.string(&view, a).unwrap());
    assert_eq!(changed.string(&view, Str { off: n + 8, len: 4 }).unwrap(), b"data");
    let corrupt = |at| matches!(changed.string(&view, at), Err(IncrReason::StateCorrupt(_)));
    assert!(corrupt(Str { off: n - 1, len: 2 }), "straddles the boundary");
    assert!(corrupt(Str { off: n + 10, len: 3 }), "runs past the buffer");
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
        refill::refill_set(&view, &ChangedFiles::default(), &resolved, &layout, &[]).err(),
        Some(IncrReason::UpdateNotImplemented(Some(Unimplemented::Refill)))
    );
}
