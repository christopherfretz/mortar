# WIP: WP-218 follow-up (briefs/wp-218-rulings.md)

Branch wp/218-incr-image; accepted head 087fbf4a; seam 6 cherry-picked
clean as 78ecc9ba.

## Next (rulings order)
1. F-2 build id from stored leaves (in: &[[u8;32]], out: new leaves);
   build_id_read = dirty blocks; wrong length = state-corrupt pre-W1.
2. F-1 update_map(stored); record table of what refill reads of an
   unchanged file (poison test if none).
3. F-4 frag class test.
4. F-5 folded sections: rule, fixture, map.
5. seam 6: slot lists, DeadSlot holes (+ plumb words), StoredFrag.size,
   fdes, WP-243 eh shapes.
7. SymtabPlan assumptions in record.
8. protoc plain timing base vs head (5 runs, medians).
9. plumb on incremental full-link maps, aarch64 + x86-64.
Gate.
