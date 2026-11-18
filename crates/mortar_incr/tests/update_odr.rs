//! U-8's tests (`spec/incr.md` IN-71d). WP-213 (U-1): the entry point's
//! "until it lands" value; U-8 replaces it.

use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_core::plan::OdrMode;
use mortar_core::{FileId, SectionId};
use mortar_incr::update::odr::odr;
use mortar_incr::update::resolve::ResolveDelta;
use mortar_incr::update::rows::FileRow;
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::{Change, ChangedFile, ChangedFiles};

// spec: incr.md IN-71d (until U-8: units in a changed file are `meta`)
#[test]
fn odr_until_it_lands() {
    let m = MemoryState::default();
    let view = m.view();
    let resolved = ResolveDelta::default();
    let mut changed = ChangedFiles::default();
    assert_eq!(odr(&view, &changed, &resolved, OdrMode::Warn), Ok(Vec::new()));
    changed.files.push(ChangedFile {
        file: FileId(1),
        change: Change::Changed,
        row: FileRow::default(),
        first_section: SectionId(0),
        sections: Vec::new(),
        keys: Vec::new(),
        mentions: Vec::new(),
        index: Vec::new(),
        edges: Vec::new(),
        eh_records: Vec::new(),
        meta_units: 1,
        debug: false,
        versioned: 0,
    });
    assert_eq!(
        odr(&view, &changed, &resolved, OdrMode::Warn),
        Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Meta)))
    );
}
