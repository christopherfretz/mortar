# WIP — WP-058 (the Mason lane on the dynamic line)

Done:
- `tests/mason/lane/kiln++`: `dyn-<linker>` modes keep kiln's dynamic line, add `-B` lane ld.
- `tests/mason/lane/ld/mortar/ld`: passes the link's words to `xtask mason verify` (stage 3).
- `xtask/src/mason.rs`: `plumb(.., line)`, `verify <out> <map> [<line>]`.
- `xtask/src/mason/libcxx.rs`: modes `dyn-bfd|lld|mold|mortar`, `reference_of`, baseline on both mortar modes.
- `xtask/src/mason/programs.rs`: `Link` (Static/Pie/NoPie), dynamic variants, qemu `-L`, x86-64 dynamic verified.

Half-done: programs table run; suite not started.

Next: programs table -> suite sample -> full suite shards (scratch /tmp/mortar-wp058-279433) ->
census kiln x86-64 row -> ci step -> record spec/landed/mason/WP-058.md.
