# WP-027d work in progress (deleted in the final commit)

- [x] 1. honest option table: `partial` status, last-wins overrides, audit test
  (`crates/mortar/tests/options.rs`); rows changed: --gc-sections ->refused,
  --build-id/--fix-cortex-a53-843419 ->partial, --as-needed/--no-as-needed/--hash-style
  ->noop, -z relro/norelro/execstack/noexecstack/separate-code/noseparate-code/max-page-size
  and --no-build-id ->supported.
- [x] 2. debuggers.rs: 10 tests (5 drivers x sessions/dwarf), 20 configs; I-47 rnglists xfail
- [x] --sysroot (supervisor): 7e3b834 standalone, merged as 0b0a09e; main c851579 merged.
- [ ] 3. [x] BTI thunks (link.rs thunk_target_word/relocated_near/plt_first_word; Q15
  answered; pipeline.rs a_bti_*; plumb gap: reloc.rs:1618 IPLT S+A off by 4).
  [x] LO15 (row 313 M1; data-pointers XFAIL->PASS). [x] build-id split (md5 alone XFAIL; plumb B1 hex-length gap). [x] --warn-backrefs
  supported (archive-backref XFAIL->PASS). [ ] -Ttext/--section-start stay refused (layout.md
  §20.2: until scripts); [ ] -gz sizing
- [ ] 4. small joins: [x] .eh_frame DeadRef::DeadFde (scan name match gone); [x] R-L6 verified (L-159, __TMC_END__); [x] ATTRIBUTES consumed (Clang 22 test; plumb BTI IPLT gap)
- [~] 5. -Map text: specified (image.md §9.3 I-120..I-122, proposed), not implemented
- [ ] 6. link-line histogram (measured; see hand-back)
