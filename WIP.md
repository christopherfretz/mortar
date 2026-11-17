# WP-128 WIP (deleted in the final commit)

Scratch: /Volumes/AgentWork/mortar-helper/.build/target-wp-141/wp128/ (m1 flags, m4 exclude-libs,
m5 -init, m6 dynamic-list exe/shared, m7-m9 manual mortar checks of the xfail(mortar) tests).

## Done (commits a687103, a89e29e, then part 3)
- parts 1, 2, 5, 4, 3 implemented, link tests + crate tests written; spec edits in dyn.md
  (D-102 rows 3-4, D-103, D-110 item 1, D-112, D-124 item 2, D-1 static-pie rows),
  resolve.md (§4.2 rows -init/-fini and --dynamic-list, §4.7, §9.6), cli.md §4.2.
- --dynamic-list measured: exe exports listed (all three), no extraction, gc root;
  several lists one (union); quoted entries; static-pie: references export, mortar no effect (D-1).

## Next
- census (`cargo xtask census --filter shared`, quick census), static-identical, sweep.
- record spec/landed/dyn/WP-128.md; gates (linktest whole lane, ci --quick); hand back.
