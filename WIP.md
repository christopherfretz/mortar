# WP-106 WIP

## Done
- Item 1: `detect.rs` `thin_member_paths` (commits 0f3907b test, 7f8a5dd fix).
- Item 2: `delta.rs` `REPORT_BOUND`, `Bounded`; `the_report_is_bounded`
  (100k sections, 109,522 B / 3,022 lines), `delta::tests::the_report_bound`;
  oracle fails an elided report.
- Item 3: `incremental.rs` `dry_run_wanted`, `self.old = None` after the
  target check (failed on main: a dry run on a state of another target).
- Item 4: `mortar_incr::SMALL_OUTPUT`, `small_output`; reason in `start`.
- Item 5: WP-061b section 7 corrected.
- Spec: incr.md IN-10 row, IN-10a, IN-10b, IN-11 test, IN-24, section 7
  rows; cli.md report.

## Next
- Measure (meas/run.py, release, bounded vs unbounded) for the record.
- Record spec/landed/incr/WP-106.md; gate; hosted CI.
