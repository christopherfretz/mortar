# WIP — WP-058 (the Mason lane on the dynamic line)

Done (committed): dyn-* modes (kiln++ shim, libcxx.rs), mortar shim passes the line to
`xtask mason verify`, programs dynamic variants (171 cells hold), census kiln x86-64 row
(56 kiln lines pass), `ci` step `mason` (sample `ci`, 11 tests, mortar + dyn-mortar; full gate
adds programs), collect-before-remove fix in libcxx.rs.

Running: full suite dyn-bfd + dyn-mortar, shards 1-6 of 6, /tmp/mortar-wp058-279433/shards.sh
(results /tmp/mortar-wp058-279433/full/d-<k>.tsv). Shard 1: 1423/1423, 0 DIFFERENT, 1201 clean.

Learned: no libc++1-22 runtime in Debian trixie (libc++-22-dev depends on libc++1-19): a
shared-libc++ mode needs a libc++ 22 runtime from elsewhere.

Next: lld/mold dyn on shards 1,2; typical-link bench (/tmp/mortar-wp058-279433/typical);
record spec/landed/mason/WP-058.md with xz data; merge main; gate; push; hosted run.
