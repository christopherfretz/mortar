# WP-053 WIP (delete in the final commit)

Checkpoint 2026-11-15 ~14:30 ET (supervisor restart). Briefs:
`briefs/m2-common.md`, `briefs/wp-053.md` (in the context repo).

## Done
- Guard lifted for AArch64 Exec/Pie (`crates/mortar/src/dynamic.rs`
  `arch_status`); x86-64 refused naming WP-055, i386 refused for M2.
  The dynamic guard itself (`dynamic_guard`, `is_gate`, `Guard`,
  `DYNAMIC_OUTPUT`, `option_refusals`) is deleted; `lib.rs` uses
  `parsed.refusals()`.
- Phase 80 dynamic planning in `crates/mortar/src/dynlink.rs`; front and
  image wiring (`mortar_image/src/link.rs`: DynPlans, DynView, DynChunks,
  `rela_dyn`, sh_link/sh_info, EI_OSABI, map DynRecords + args).
- NOBITS fix (this checkpoint): `mortar_image/src/link.rs` write_link's
  synth loop skips `SynthKind::DynBss | BssRelRo` (no fill-plan row);
  this fixed the "invalid fill plan: row ... overlaps" internal error in
  every case with a copy relocation.
- CLI fold (d878609): AT_JOIN folded into ROWS and deleted (table.rs
  `join_changes`/`status_at_join`, grammar `refused_uses_at_join`, text
  `join_markdown`); `docs/status/options.md` regenerated; options.rs has
  `dynamic_options` (Check for the 16 newly supported rows); refusals.rs
  `the_dynamic_gates_are_not_refused`; grammar
  `r9_static_pie_is_a_static_pie_in_the_plan`; spec/cli.md §4.2 text.
- Clippy clean (`--workspace --all-targets -D warnings`).
- Targets 1a and 1b DONE (evidence in the hand-back of this agent): gcc
  PIE and -no-pie C hello link, run, plumb findings 0, gdb + lldb break
  on main (lldb: `settings set target.disable-aslr false`), readelf
  matches GNU ld's record sets. eu-elflint flags `.relro_padding`
  ("expected RELR", elflint's `.relr` prefix match; lld's output gets the
  same) - finding, not fixed.

## tests/link/dyn (last run, after the NOBITS fix, with the xfails edit)
mortar aarch64 70 PASS, 9 XFAIL; x86_64 6 PASS, 69 XFAIL; 0 FAIL, 0 XPASS.
58 WP-053 lines removed. XPASS lines of other packages removed for the
architecture that passes (WP-054: clangxx/gxx-default/gxx-static-libstdcxx
aarch64; WP-056: gfortran-default, gcc-rdynamic, gcc-z-now,
init-fini-option, norelro-dynamic, plt-now aarch64; WP-055:
copy-tls-refused, tls-mismatch-shared, used-underlinks-refused x86_64).
Six cases still fail and keep WP-053 lines with the diagnosis:
1. `dyn/dynsym/environ-aliases`, `dyn/reloc/mixed-mode`: plumb P-D5/P-D7,
   each copy alias is twice in `.dynsym` (e.g. entries 15-17 and 18-20,
   environ/_environ/__environ@GLIBC_2.17). Two SymbolIds resolve to one
   shared definition (the program's reference and the versioned name);
   `mortar_synth/src/dynsym.rs` `dynsym_plan` dedups keys by SymbolId
   (`keys.dedup()` ~line 474) and `copy_aliases` (~507) adds every id
   whose def is at the copy. Next: dedup by (name, version) / pick the
   resolved id; check where resolve creates the second id.
2. `dyn/dynamic/variant-pcs-plt`: vfn not bound at load:
   `ProgramDef.other` loses `STO_AARCH64_VARIANT_PCS` (no
   DT_AARCH64_VARIANT_PCS). Look at DynSource's `program()` in dynlink.rs.
3. `dyn/dynsym/versioned-export-refused`: the link succeeds; the refusal
   is never reached.
4. `dyn/reloc/tls-desc-large`: plumb P-D12, a TLSDESC record in a PIE
   (D-48 says descriptors are relaxed).
5. `dyn/shlib/closure-not-on-command-line`: plain "undefined symbol",
   without D-12's "libmid.so, which is not on the command line" note.

## Next (in order)
1. Gate: `MORTAR_REQUIRE_TOOLS=1 cargo xtask ci --quick` on the head
   (was started at the checkpoint; check its result).
2. Fix the six cases above (1 first: two tests), remove their lines.
3. Target 3: the C programs of `spec/dyn.md` §16 corpus.
4. Write `spec/landed/dyn/WP-053.md`; delete this file; hand back.

Scratch (/tmp is wiped by the restart): fixtures are rebuilt from each
test file; mortar under gcc via a `w/ld` symlink to the debug mortar and
`gcc -B$PWD/w/`; plumb from the scratch dir:
`.build/target-wp-053/debug/xtask plumb OUT --map MAP --map-format mortar`.
