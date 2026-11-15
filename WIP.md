# WIP: WP-060 revision after the adversarial review

Brief: the coordinator's revision message (review at
`briefs/reviews/review-060-incr.md` in the context repo). Deleted in the
final commit.

## Done (scripts in `.build/review-incr/`)
- baseline re-measured on main 2a31ada (baseline.sh); floor emulation
  (floor.py); stamp rule (stamp_rule.py); state as bytes + M12 hash cost +
  armap (state_emul.py); packed holes (packed_holes.sh); eh hole shapes
  incl. absorb4/min16/min12 (eh_holes.sh, ehpatch.py).

## Running / queued (one helper at a time)
- corpus2 (BRIEF-corpus2.md): drift, kiln MD5, exp 2 rows, debug reuse.
- dwarf (BRIEF-dwarf.md): block form, neutral forms, E10, E11.
- oracle (BRIEF-oracle.md): E3 + mutation, E13.

## Spec edits done
- IN-1(B), IN-2 items 4-6, IN-3/3a/3b, IN-21/22/22a, IN-24, IN-25, IN-27,
  IN-29, IN-30/31/32, IN-34 rows, IN-34a, IN-40, IN-45/46/47/48/49/49a, IN-19.

## Next
- after corpus2: section 1.1/1.2/1.5, IN-33, IN-50..57, Table 1.
- after dwarf: IN-51/51b/55; after oracle: E3/E13 text, mutation table.
- rewrite spec/incr.md per the rulings; record; README row.
