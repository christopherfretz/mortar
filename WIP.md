# WIP — WP-121 (verifier stage 3 for shared objects, P-D16..P-D26)

Branch `wp/121-plumb-shared`. Brief: `/Volumes/AgentWork/mortar-helper/briefs/wp-121.md`.
Record: `spec/landed/plumb/WP-121.md` (part 1 written).

## Done (part 1, handed back; gate `[ci] ok` on the tree with int/batch-4)
- maprec line records; shared.rs (line, scripts, lists, sets, P-D16,
  P-D18 via view A, P-D19-P-D22, P-D25, P-D26, dyncheck filter, view B);
  dynverify Shared branch; loader interposer; tests/shared.rs (forms,
  more, three hostile tests).

## Next (part 2)
- P-D17 address clause: D-117 relocation-level rule (exact fields, the
  boundary rule, "before the start"; no x86 decoder) + `sharedflow.rs`
  (AArch64 items 1-9). Compare off-gate with
  `tests/shared-design/exp3/s1d.py` (run from its directory) on §G.1's
  corpora (`exp3/lists/*.lst`).
- P-D23 table in `dynmutate.rs` (every W-row a mutation of a correct
  library; residual hole asserted NOT caught).
- §8.1 sizing (rewritten by WP-109b) on the largest AArch64 bench link
  as if `-shared` (bench-work rules: prlimit 8 GiB, nice 10, load < 10).
- After WP-131 lands: use `Line.symbolic` instead of SharedLine's own
  `-B` reading where it overlaps; do not duplicate `SYMBOLIC`.
