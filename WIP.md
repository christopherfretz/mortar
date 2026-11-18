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

## Done
- (see git log)

## Next
1. write.rs: encode(MemoryState) [verbatim, slotting], plan_patch/patch.
2. compact.rs: apply patch in memory, normalize, compact.
3. canon.rs; tests/state5.rs (replace "until it lands"); example incr_state.
4. capture SYMBOLS order; new capture rows; size gate; gates; record spec/landed/incr/WP-214.md.

## Resume
cargo: `( cd <worktree> && unset CARGO_TARGET_DIR && source scripts/env.sh && ulimit -c 0 && cargo ... )`,
target `/Volumes/AgentWork/mortar-helper/.build/target-wp-116`.
