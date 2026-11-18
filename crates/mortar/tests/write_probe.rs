//! W-1's tests (`spec/incr.md` IN-76a). WP-213 (U-1): the entry point's
//! "until it lands" value; W-1 replaces it.

use mortar::update::probe::clone_capable;
use mortar_incr::update::rows::MetaRow;

// spec: incr.md IN-76a (until W-1: unknown)
#[test]
fn clone_capable_until_it_lands() {
    assert_eq!(clone_capable(0, &MetaRow::default()), None);
}
