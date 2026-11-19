//! WP-225 M2b-2b: the join's placement.
//!
//! - Ruling M2b2a-1: `front_of_changed` reads an `Ids::Update` row's
//!   `path` through `ChangedFiles::string` (the update's string space), so
//!   an APPEARED file (no stored row; its path a string of the update's
//!   buffer, past `STRINGS`) opens; `front_of` (the state's `STRINGS`
//!   only) refuses the same row `(resolve)`.
//!
//! On AArch64 and x86-64.

#![allow(dead_code, reason = "support/update.rs's helpers that other tests use")]

mod support;
#[path = "support/update.rs"]
mod update;

use mortar::update::file_front::{FrontCache, Ids, front_of, front_of_changed};
use mortar_core::id::{FileId, SectionId};
use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_incr::format::Str;
use mortar_incr::update::rows::FileRow;
use mortar_incr::update::view::ChangedFiles;
use update::{first_free, lines, pool, state};

#[test]
fn an_appeared_file_opens_through_the_update_strings() {
    for line in lines(&[]) {
        line.archive(&["m1.o", "m2.o"]);
        let view = state(&line, &line.args(&["wp255d_m2", "wp255d_m1"]), "ap");
        // `m1e.o`: named on no line of the state, its path a string the
        // state does not hold.
        let path = b"m1e.o".to_vec();
        let bytes = std::fs::read(line.dir.join("m1e.o")).expect("m1e.o");
        let base = u32::try_from(view.count(mortar_incr::format::kind::STRINGS)).expect("STRINGS");
        let changed = ChangedFiles { strings: path.clone(), ..ChangedFiles::default() };
        let len = u32::try_from(path.len()).expect("a short path");
        let row = FileRow {
            path: Str { off: base, len },
            len: bytes.len() as u64,
            hash: mortar_input::store::content_hash(&bytes).0.to_le_bytes(),
            rank: u64::from(u32::MAX) << 20,
            ..FileRow::default()
        };
        assert_eq!(changed.string(&view, row.path).expect("the path"), path, "{}", line.label);
        let file = FileId(u32::try_from(view.count(mortar_incr::format::kind::FILES)).expect("F"));
        let first = SectionId(first_free(&view));
        let ids = Ids::Update { first_section: first, row: row.clone() };

        // The state's strings alone: the path is past them.
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        let r = front_of(&mut cache, &view, file, ids.clone()).map(|x| x.file);
        let want = IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve));
        assert_eq!(r, Err(want), "{}: front_of", line.label);

        // Through the update's strings: it opens, with the update's ids.
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        let ff = front_of_changed(&mut cache, &view, &changed, file, ids)
            .unwrap_or_else(|e| panic!("{}: front_of_changed: {}", line.label, e.report()));
        assert_eq!(ff.file, file, "{}", line.label);
        assert_eq!(ff.row, row, "{}: the row it was given", line.label);
        let object = mortar_elf::Object::parse(&bytes).expect("m1e.o parses");
        assert_eq!(ff.sections.len(), object.section_count() as usize, "{}", line.label);
        for (k, s) in ff.sections.iter().enumerate() {
            assert_eq!(s.id, SectionId(first.0 + k as u32), "{}: section {k}", line.label);
            assert!(s.place.is_none(), "{}: an appeared section has no stored place", line.label);
        }
        assert!(
            ff.keys.iter().any(|k| k.name == b"wp255d_m1" && k.key.is_some()),
            "{}: its keys are the state's by name",
            line.label
        );
        // A string past the update's buffer is refused, not read.
        let short = ChangedFiles { strings: b"m1e".to_vec(), ..ChangedFiles::default() };
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        let ids = Ids::Update { first_section: first, row };
        let r = front_of_changed(&mut cache, &view, &short, file, ids).map(|x| x.file);
        assert!(matches!(r, Err(IncrReason::StateCorrupt(_))), "{}: {r:?}", line.label);
    }
}
