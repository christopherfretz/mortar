# WP-032f WIP (deleted in the final commit)

Done:
- fix 1 (record_sym_at): a deferred record's value and its check's facts from one facts read.
  protoc 1T fill median 181 -> 170 ms (min 173 -> 157), 8T 71.5 -> 66 ms; View::facts 9.7% -> 3.8% of fill. Byte-identical (30 links).
- fix 2 (Memo in Scratch): per-worker direct-mapped memo (1024 slots, ~72 KB/worker) of record_sym/record_sym_at by key (+mention).
  protoc 1T fill 183 -> 168 ms, total 426 -> 412; 8T fill 36.4 -> 33.9. sym_value_of 5.9% -> 0.7% of fill. Identical.

Next: item 2 (batch decode in relocate), item 3 (value table), item 4 (piece memo), item 5 (kernel).
Bench dir: /tmp/mortar-wp032f-652827 (golden.sh, ab.sh, prof.sh; base hashes in golden-base.txt).
