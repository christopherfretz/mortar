# WIP — WP-147 (verifier: placeless exports, P-1)
- [ ] build; measure refs (-shared, -pie --export-dynamic; aarch64, x86_64)
- [ ] one predicate: Sec placeless (reloc placeless_def + shared::compute)
- [ ] P-D19 + mutation tests (placeless present = finding; placed missing = finding)
- [ ] link tests: -shared checks in place-note-property / place-dropped-global
- [ ] open items: -s, Y4 locals placeless, gc roots
- [ ] place-empty-merge: gms/gmid value mutation
- [ ] gates; spec; landed record
