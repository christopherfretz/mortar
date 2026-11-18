# WP-255c2a WIP

Done:
- M1: `crates/mortar_image/src/clip_plan.rs` (`for_clips`, `ClipSection`,
  `ClipParts`, `ClipError`/`NotYet`, `check_written`); `layout_plan.rs`
  shared helpers `is_member`, `member_front`, `abbrev_offset`, `hole_bytes`
  (from_layout / debug_holes call them; behaviour unchanged).

Next:
- M2: the clip filler in `link.rs` (a fill of a clip plan with the bitmap
  check under debug_assertions) + the link-side builder of the clip inputs.
- M3: `crates/mortar/tests/clip_plan.rs`.

Resume: scratch `/Volumes/AgentWork/mortar-helper/.build/target-wp-145/wp255c2a`,
`check.sh` there.
