# WP-125 part 1 (version-script parser) work in progress (deleted in the final commit)

- Measured: lld 22/19 on e10/a05 (`.symver` name in another node's exact
  entry): `version script assignment of 'local'|'V2' to symbol 'old'
  failed: symbol not defined` (error by default, accepted with
  --undefined-version). Syntax probes in .build/target-wp-125/meas/syn.
- Plan: diag kind version-script-syntax; Mode::Version in lex.rs;
  version.rs parser; tests/version_grammar.rs, version_corpus.rs (213),
  version_hostile.rs; spec §9 edits; record spec/landed/dyn/WP-125.md.
- Next: see git log.
