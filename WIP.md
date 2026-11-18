# WIP: WP-229 (G-2)

Done: incr_discard.rs (LEFT_OUT, left_out, discard -> Dropped(Stripped)); front.rs call (separate commit, by the supervisor's word);
split.rs (Handled, classify_object, classify_rows); scripts tests/incr-design/wp229 (classify.py, detect.sh, neutral.py, skel.sh, protoc.sh).
Rulings: briefs/wp-229-rulings.md.
Next: run detect.sh/skel.sh/protoc.sh (scratch .build/target-wp-157/wp229), edge cases of ruling 4, ruling 9 (cross-file relocs),
update_split.rs tests, lane test, plumb runs, record spec/landed/incr/WP-229.md, gate.
Resume: build with .build/target-wp-157/wp229/build.sh.
