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

// spec: incr.md IN-75a, diag.md's subjects column (WP-221: filled; the
// column and the code agree both ways in mortar_diag's spec_tables)
#[test]
fn subjects_of_a_reference_site_warning() {
    use mortar_diag::subject::{SubjectKind, subjects};
    assert_eq!(subjects("undefined-symbol"), [SubjectKind::File, SubjectKind::Key]);
    assert_eq!(subjects("gnu-warning"), [SubjectKind::File, SubjectKind::Key]);
    assert!(subjects("duplicate-symbol").is_empty());
}
