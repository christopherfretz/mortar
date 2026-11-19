//! WP-238 Part 4 (P3b-2): `MemoryState::index_sections`, the capture's
//! step before the live-set hash (S4), refuses by name a state whose
//! `SECTIONS` rows of one file are not contiguous (S4 sums a file's
//! sections by its range: another file's rows inside it would be summed
//! as its own), and a row of a file past `FILES`; `compact::normalize`'s
//! rows (by file, then position) pass with the ranges the rule gives.

use mortar_core::incr::IncrReason;
use mortar_incr::format::SectionRow;
use mortar_incr::update::rows::{FileIndexRow, FileRow, FileTable};
use mortar_incr::update::testing::MemoryState;

/// Three files; `SECTIONS` rows of the files `order` names, in that order.
fn state(order: &[u32]) -> MemoryState {
    let mut m = MemoryState::default();
    for f in 0..3u64 {
        m.files.push(FileRow { rank: f << 20, ..FileRow::default() });
        m.file_index.push(FileIndexRow::default());
    }
    for (i, &f) in order.iter().enumerate() {
        m.sections.push(SectionRow {
            file: f,
            ordinal: u32::try_from(i).expect("rows"),
            ..SectionRow::default()
        });
    }
    m
}

fn range(m: &MemoryState, f: usize) -> (u32, u32) {
    let r = m.file_index[f].ranges[FileTable::Sections as usize];
    (r.first, r.count)
}

#[test]
fn contiguous_files_are_indexed() {
    let mut m = state(&[0, 0, 2, 2, 2]);
    assert_eq!(m.index_sections(), Ok(()));
    assert_eq!([range(&m, 0), range(&m, 1), range(&m, 2)], [(0, 2), (0, 0), (2, 3)]);
}

#[test]
fn a_file_with_rows_apart_is_refused_by_name() {
    let mut m = state(&[0, 1, 0, 2]);
    assert_eq!(
        m.index_sections(),
        Err(IncrReason::StateCorrupt("SECTIONS of file 0 are not contiguous".into()))
    );
    // The last file's rows apart, the first run unbroken.
    let mut m = state(&[0, 2, 1, 1, 2]);
    assert_eq!(
        m.index_sections(),
        Err(IncrReason::StateCorrupt("SECTIONS of file 2 are not contiguous".into()))
    );
}

#[test]
fn a_row_of_a_file_past_files_is_refused() {
    let mut m = state(&[0, 3]);
    assert_eq!(
        m.index_sections(),
        Err(IncrReason::StateCorrupt("SECTIONS row 1 names file 3 past FILES (3)".into()))
    );
}

#[test]
fn normalized_rows_pass() {
    // `normalize` sorts `SECTIONS` by (file, position): the interleaved
    // rows come out contiguous, each file's in its order.
    let m = state(&[2, 0, 1, 0, 2]);
    let mut n = mortar_incr::state5::compact::normalize(&m).expect("normalize");
    assert_eq!(n.index_sections(), Ok(()));
    assert_eq!([range(&n, 0), range(&n, 1), range(&n, 2)], [(0, 2), (2, 1), (3, 2)]);
    let ord: Vec<u32> = n.sections.iter().map(|s| s.ordinal).collect();
    assert_eq!(ord, [1, 3, 2, 0, 4]);
}
