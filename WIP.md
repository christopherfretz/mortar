# WIP — WP-077 (load-time ODR check, prototype package)

Done:
- read the brief, m6-common, odr.md §10, the prototypes; mortar release build in
  `.build/target-wp-077/release/mortar`; xtask debug build.

Half-done:
- `tests/hook/mason_odr_rt.cpp` (the runtime, mechanism "read the binding":
  each module's dynamic relocations against its own rows, the slot's value,
  the defining module's row at that address).

Next:
- `tests/hook/run.sh` (the matrix), cost tables, fail-the-load experiments,
  audit experiment, the record `spec/landed/meta/WP-077.md`, §10.3 rewrite.
