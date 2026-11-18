//! U-9's tests (`spec/incr.md` IN-75a). WP-213 (U-1): each entry point's
//! "until it lands" value; U-9 replaces them.

use mortar_incr::update::diags::{DiagRow, Subject, diags};
use mortar_incr::update::resolve::ResolveDelta;
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::ChangedFiles;

// spec: incr.md IN-75a (until U-9: the fresh warnings only)
#[test]
fn diags_until_it_lands() {
    let m = MemoryState::default();
    let view = m.view();
    let fresh = vec![DiagRow {
        subjects: vec![Subject::Link],
        code: "w".to_owned(),
        text: "t".to_owned(),
        key: mortar_diag::Key { phase: 1, unit: 0, seq: 0 },
        tomb: false,
    }];
    let out = diags(&view, &ChangedFiles::default(), &ResolveDelta::default(), fresh.clone());
    assert_eq!(out, fresh);
}

// spec: incr.md IN-75a, diag.md's subjects column (until U-9: none)
#[test]
fn subjects_until_it_lands() {
    assert!(mortar_diag::subject::subjects("undefined-symbol").is_empty());
}
