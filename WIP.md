# WP-214 (U-2) state format 5 — work in progress

Branch `wp/214-incr-state5` on base 294101b1. Brief: `briefs/u-2-to-9.md` "U-2 = WP-214" + `briefs/u-common.md`.
Seam: `spec/landed/incr/WP-213.md`. Rulings: patch/compact take `output: &StatRow`; ODR rows PROVISIONAL (last);
supervisor 2026-11-17: META(+iterations), GROUPS (signature,file), COMMONS (key,file), FILE_INDEX(+first_global),
MENTIONS invariant change by U-3's seam commit: WAIT for its hash, then `git cherry-pick <hash>` (allowed once),
conflicts in favour of the seam; report what conflicted. Report size with re-keyed GROUPS separately.

## Design (decided)
- Header 80 B (`MORTARSR`, v5, flags@12 outside hashes, count, block 4096, len, plan hash, dir L128, header L128 of 0..12+16..64).
- Directory 72 B/entry in kind order (39 tables): kind, width, rows, capacity, aux, dead, body off, list off, list L128.
  rows = extent (file-keyed), body+used area (key-keyed; aux = sorted body len, dead = tombstones); STAT aux = output row.
- Per table: list (L128 per 4 KiB block) then body (capacity × width, zero-filled). No alignment padding.
- Codec: `state5/mod.rs` (`Rec`, `Keyed`, header/dir); format.rs W/R/Row made pub(crate).
- Reader `state5/read.rs`: `Core` (Src::File dup | Src::Bytes, verified list/block caches, byte counter);
  `open` registers the reader in a static by address so plan_patch/patch can find the descriptor
  (StateRows has no accessor: FLAG for a ruling). `open_bytes`, `bytes_read`, `decode` (whole, file id space:
  sections with NONE fillers at slack rows; keyed tables with tombs, in row order) are new pub fns.
- SectionId = SECTIONS body row (slack gaps). Encoder takes dense MemoryState and slots sections.
- normalize (compaction renumbering) shared by full-link writer and compaction => compaction == capture by construction.
- STAT: output row index in dir aux; normalize moves it last. Appended rows go after the extent.
- FRAGS: an insert into the area that would move an existing area row (FRAG_AT.frag names rows) => compaction.
- Canon: decode → normalize → Debug rendering with Str replaced by bytes; FILES.round → support bool.

## Rulings received
- WP-220 R-1 (supervisor): ODR_CLASSES (47) and ODR_REFS (48) NOT written: removed from TABLES (37 tables);
  reader returns none; encode/plan refuse rows. COPIES (55) comes with U-8's seam commit (after U-3's).
  Cherry-pick U-3's hash first, then U-8's; report COPIES' and re-keyed GROUPS' sizes separately.

## Done (commits on the branch)
- codec + lazy verified reader (read.rs), writer `encode`, `plan_patch` (in-place editor `Ed`, overflow -> compaction),
  `patch`, `patched_bytes`; compact.rs `apply` (in memory, same placement), `normalize`, `compact`, `plan_bytes`;
  canon.rs `canon`, `canon_of`, `compare`; `size_report`, `refuse_updating`; decode verifies every block;
  rank ties compact; gone files' STAT rows dropped.
- tests/state5.rs: 14 tests pass (named ones: compaction_equals_full_link_capture, compaction_reads_every_block,
  rank_midpoint_exhausted_compacts, patched_block_is_verified_first, key_refs_single_mentioner, state5_size_report,
  incr_state_refuses_flag_set, update_reads_only_touched_blocks (state 3.4 MB, plan read 43,296 B, 5 blocks written),
  symbols_row_order_is_the_stated_one).
- examples/incr_state.rs (convert/canon/compare/size).

## Next
1. size gate: `.build/target-wp-116/wp214/size.sh cxx-medium-g-static` (protoc), then cxx-large-g (delete the 1 GB output).
2. Seam commits (U-3, then U-8) when hashes arrive: cherry-pick, adapt GROUPS/COMMONS/META/FILE_INDEX/MENTIONS, COPIES.
3. Not done: capture.rs SYMBOLS order (normalize applies it in the format-5 writer instead; format-4 bytes unchanged),
   new capture rows (REGIONS, NAMES rebuilt by normalize, EDGES, COMMONS, lazy members, FILES rank/round),
   file_mentions `continue` (after the seam).
4. Gates; record spec/landed/incr/WP-214.md; delete WIP.md.

## Resume
cargo: `( cd <worktree> && unset CARGO_TARGET_DIR && source scripts/env.sh && ulimit -c 0 && cargo ... )`,
target `/Volumes/AgentWork/mortar-helper/.build/target-wp-116`.
