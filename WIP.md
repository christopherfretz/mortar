# WP-027d work in progress (deleted in the final commit)

- [x] 1. honest option table: `partial` status, last-wins overrides, audit test
  (`crates/mortar/tests/options.rs`); rows changed: --gc-sections ->refused,
  --build-id/--fix-cortex-a53-843419 ->partial, --as-needed/--no-as-needed/--hash-style
  ->noop, -z relro/norelro/execstack/noexecstack/separate-code/noseparate-code/max-page-size
  and --no-build-id ->supported.
- [ ] 2. debuggers.rs (helper agent, in progress)
- [ ] 3. BTI thunks, LO15, build-id, section-start, -gz sizing
- [ ] 4. small joins
- [ ] 5. -Map text
- [ ] 6. link-line histogram (measured; see hand-back)
