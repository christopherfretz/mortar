# WIP: WP-155 (D-25's call-only cell; ruling (A))

Done: spec (D-25, §13.1, arch §3.12/§4.3, a64 §7.1, x64 X-16/X-57), code
(`weak.rs` verdict, `mortar_arch::weak_use_kind`, `SectionOut::writable`, a64/x64
scan+apply, image reloc, sites probe), unit tests, link tests weak-branch-{call,split,gc},
weak-class-table updated, static-identical re-recorded (AArch64), record
`spec/landed/dyn/WP-155.md` (census section to fill).

Next: census (script `.build/target-wp-140/wp155/census.sh`, logs in `wp155/census/`):
quick and --filter shared before/after; x86-64 no-pie kept dirs -> diff dynsyms
(new keys, count) -> record §5. Then the whole lane and the gate in the background,
record the lines, delete WIP.md, final commit.
