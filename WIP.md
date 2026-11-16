# WP-112 work in progress (deleted in the final commit)

Count at start: 81 of 121 accepted rows unspelled by the explicit rule
(`cargo xtask spelled`); WP-100 counted 60 of 114 with the drivers'
implicit words.

Done: spelled.rs check (+ `cargo xtask spelled`), `check cmp`, link
`exit: info` / `print:`; tests: input/dependency-file, dyn/shlib/bdynamic,
layout/execstack.

Findings:
- V1 (verifier): mortar_plumb line.rs:186 knows `-dy`/`-call_shared` with
  one dash only; `--dy`/`--call_shared` (and likely `--dn`, `--non_shared`)
  leave bstatic set -> P-D2 on every linker's correct output. Tests use
  the one-dash spelling until WP-102.

Reference differences (to add to spec/harness.md table):
- --dependency-file: indent (bfd 2, lld 1, mold one line), bfd lists per mention
- -call_shared: mold unknown
- exec note: lld22 refuses without -z execstack, also with -z noexecstack; mold RW + warns

Next: rest in WP-100 order.
