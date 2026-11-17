# WP-137 WIP
- Measured (scratch: .build/target-wp-137/m1, meas.sh; ref-a64.txt, ref-x64.txt, mortar-a64-before.txt).
- Rule: in Shared, bind(k) ranges over N only (no DT_NEEDED search; a dropped C member binds nothing).
  Code: crates/mortar_resolve/src/shared.rs `binds`.
- Next: rebuild, re-measure mortar, spec edits (D-10 setting/D-11, D-27, D-111), tests
  (cxx-weak-only-libc xfail off, C twin, no-as-needed twin), census, gate, linktest, record WP-137.md.
- Finding to report: executable with a versioned weak def only in a used (U) as-needed lib is refused (D-27) today; GNU ld/lld write FUNC WEAK unversioned (versym 1).
