# WIP: WP-267 P-1c (a draw-in on P-1's rows)

Brief: `/Volumes/AgentWork/mortar-helper/briefs/wp-267-p1c.md`. Branch
`wp/267-p1c` (worktree wp-156), base 2064dd12 (int/batch-26).

## Step 1: what a drawn-in file is in the state

- **Who it is.** U-3's `resolve` answers `Resolution::DrawIn(more)` with
  UNCHANGED, LIVE objects not in `changed` (`resolve.rs` ~:515-522: a
  re-elected group's old or new keeper, `live_new`; ~:888-900: an
  unchanged live object that becomes a key's definer, or whose definition's
  merged visibility flips, S-74). A plain object or an EXTRACTED archive
  member (never a `LAZY` one: that is an extraction, `extract`'s).
- **Its `FILES` row.** The stored row: `kind` 1 (object), `member`,
  `path`, `mention`, `member_name`, `member_ordinal`, `base`, `len`,
  `hash`, `rank`, `round`. Its bytes are unchanged (by U2's detection and
  by its hash), so the full link of the new inputs captures the same row
  but the layout's columns (`eh_*`: U7's, stopped as for a changed file)
  and `properties` (P-1's `facts_after_resolve`, from the KEPT notes).
  `flags`: the capture writes `POSITIONAL` on every row
  (`capture.rs` ~:554) `| object_flags` of the bytes (`EH_FIXED`,
  `DEBUG_EXPORTS`), `LAZY` never (live): the same producer as a changed
  file's (`file_rows_of`). `stack`: `stack::file_note` of the bytes.
- **Its bytes.** A plain object: its path. An archive member: the
  archive's path at the stored `base .. base + len`, which is exactly what
  `file_front::front_of` reads (`read`, ~:243), and it checks the hash of
  those bytes against the row's `hash` before anything is taken (IN-24,
  ~:302-305). With the STORED row given as `Ids::Update`'s row, that IS
  the narrowest read: by the stored row's archive path and range, hash
  checked against the stored hash, a mismatch `input-changed` (P-1c adds
  the member's name to the text). M-1's member table is NOT needed: the
  member is not matched by (name, ordinal) in a new member table, because
  a drawn-in member's archive is not a changed path (a changed archive
  member is refused at U4 already, M-1); a member that moved or changed
  under the same name inside an archive whose change U2 missed fails the
  hash at its stored range and refuses.
- **`Change::DrawnIn` against `Change::Changed`.** In front 1's oracle
  (`changed::changed_files` / `translate`) only the `change` field
  differs: the rows are made by the same code (fresh section ids past
  `SECTIONS`, allocated in `FileId` order over changed AND drawn-in files
  together, so a drawn-in file BEFORE a changed one shifts the changed
  file's `first_section`). The consumers read the difference: `resolve`
  takes a drawn-in file's mentions as its stored ones (no election, no new
  candidate, `RESOLVE_CALLS` = 2), `odr::replaces` keeps its stored rows,
  `file_scan::drawn_in_answer` (M2b-5) maps its fresh section id to its
  stored one.
- **Consequence for P-1's cache.** `FrontCache` keys fronts by `FileId` and
  ignores the `Ids` of a second call: after a draw-in, a changed file
  whose `first_section` shifts would keep its first front's ids. The
  second pass therefore runs over a FRESH `FrontCache`, and the function
  refuses a cached front that is not the one it asks for.

## Done (commits on `wp/267-p1c`, base 2064dd12)

- Step 1: above (0dafec66).
- Steps 2-3 (e5a10a34): `changed_rows.rs`: new `pub fn
  changed_files_drawn_in(view, cache, files, drawn_in, positional, plan,
  cwd)`; `changed_files_of` = it with `drawn_in = &[]` (signature
  unchanged). The shadow's post step runs front 1's loop (`RESOLVE_CALLS`,
  a fresh `FrontCache` for the second pass, a second draw-in refused
  `input-changed` as in `changed::u5_judged`), recomputes front 1's oracle
  with the same `Change::DrawnIn` files, compares every pre and post field
  again; line `P-1: equal (N files, F fields; D drawn in) ms ...
  drawn_in=..` or `finding: P-1c ...` / `finding: P-1c drawn-in set`.
- Step 4 (52eec45d): `update_changed_rows.rs`:
  `changed_rows_draw_in_equals_the_fronts` and
  `changed_rows_drawn_in_member_refused_by_name`, both targets; the whole
  test file: 5 passed, 0 failed (`.build/target-wp-156/p1c/t1.log`).

## Left (not started, on the supervisor's word: over budget)

- Step 5, the lane at `-j 1` with findings: the script is ready,
  `/Volumes/AgentWork/mortar-helper/.build/target-wp-156/p1c/lane.sh`
  (run in the background, ~12 min; final line `FINAL lane done`); then
  pair each `P-1:` line with the lines after it up to the next `P-1:` line
  against `.build/b25-findings.txt` (expected: the 46 `M-1: a draw-in`
  lines become `P-1: equal (..; D drawn in)` or a named refusal, 0 hops
  the real path takes refused, 0 findings).
- Step 6, the full gate ONCE (quick gate, linktest, census, fmt, workspace
  clippy, static-identical). Not run: clippy has not been run on the new
  code at all (`cargo check` only).
- The record: a "P-1c" section in `spec/landed/incr/WP-267.md`.
