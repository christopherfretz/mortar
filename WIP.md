# WP-071 work in progress (deleted in the final commit)

Checkpoint 2026-11-15 14:3x ET (supervisor restart). Briefs:
`briefs/m6-common.md`, `briefs/wp-071.md` (workspace root). Merges of
main used: 3 of 3 (last at main d23258b, merge 364f466; diag conflicts
resolved meta rows then odr rows; spec_tables green).

## Scope items (brief wp-071.md §1-6)

1. Reader + writer (`mortar_meta`): DONE. `crates/mortar_meta/src/{lib,read,write,anchor}.rs`;
   tests `crates/mortar_meta/tests/format.rs` (T-MT-4, 10..14, 25, 26, 35,
   36, 38), `tests/mutate.rs` (4000-seed mutation + random bytes, no panic).
   diag kinds `meta-malformed`, `meta-version` (`mortar_diag` model.rs,
   render.rs; spec/diag.md §3.1 rows moved into the first table).
2. MT-6 classification: DONE. `mortar_core` resolution.rs `SectionKind::Meta`
   (no output), `Resolution.meta` (sorted (FileId, SectionIndex)), check rule;
   `mortar_resolve` sections.rs:151 (row before SHF_EXCLUDE), resolve.rs:455
   `misnamed_container` (one warning per file), problem.rs:146, tables.rs:1269;
   T-MT-6 = `crates/mortar_resolve/tests/rules.rs` `s10_mason_containers`.
3. A-8 / MT-42 byte rule: DONE. `mortar_input` store.rs:116
   `StoreConfig.containers` (default false = the switch for WP-073),
   store.rs:319/342 `ReadSite`/`Hooks.on_read` (test byte counter),
   store.rs:1386 `stage_around_containers`, staged.rs:187 `container_ranges`,
   :251 `clip`, :323 `merge` (never bridges a container).
   T-MT-42: `crates/mortar_input/tests/t_mt_42.rs` (4 tests, pass) and
   end to end `crates/mortar/tests/meta.rs` (3 targets: garbage containers
   in a small and a large object, output byte-identical to the objcopy-
   stripped link, no diagnostic, no container in the output; pass).
   Known exceptions (for the record's findings):
   - store.rs:1399: the first 64 bytes are always resident (all readers start
     there; facts/mortar_elf need prefix(64)). An ELF32 object whose container
     starts before byte 64 (i386 GNU as with empty .text: container at 0x34)
     has up to 12 container bytes read. Found by the linktest on i386
     ("0x364 bytes at offset 0x0 are not resident" before the fix). Test:
     t_mt_42.rs `elf32_head_is_the_one_exception`. Alternative (read 52
     first) costs every ELF64 object +1 read. Propose MT-42 text: "no
     container byte past the file's first 64 bytes is read".
   - Archives: Chain windows and member head/tail reads may read container
     bytes (members' headers unknown when read); a large member's container
     is never resident (t_mt_42.rs `member_container_is_not_resident`);
     small members (<=64 KiB) whole-resident. Propose A-8 text for members.
   - `make_full` (descriptor budget) and the resident policy read whole files.
4. Test producer + dumper: DONE. `xtask/src/meta.rs` (`cargo xtask meta
   write <desc> -o x.s | --bin | --object o --target x86_64|aarch64|i386 |
   --into in.o -o out.o`; `meta dump <obj>`; the dump is a description that
   writes back to the same bytes; unit tests in the module). objcopy finding:
   binutils 2.44 `--add-section` gives SHF_WRITE unless `readonly` and ignores
   `--set-section-alignment` on a section it adds in the same run: two runs
   (meta.rs `add_to`). GNU as on i386 does not 8-align the container's file
   offset (0x34): legal, sh_addralign is 8.
   T-MT-3 + T-MT-4 + MT-42 no-warning: `tests/link/input/mason-meta.test`
   (producer unit + garbage unit, direct and archive member): 12/12 PASS
   (bfd, lld, mold, mortar x aarch64, x86_64, i386).
   T-MT-5: DONE. `crates/mortar/tests/meta.rs` `mt5_tools_that_keep_it_and_tools_that_lose_it`:
   every row of MT-5 (binutils 2.44, llvm 19 AND 22, elfutils 0.192, ar/thin,
   ld -r by bfd/lld/mold: one section, concatenated, flags E, align 8) holds.
5. Experiment 7: IN PROGRESS. Scripts + objects in
   `/Volumes/AgentWork/mortar-helper/.build/wp071-exp7/` (scripts/gen.py,
   run.sh, sizes.py, bench.py; obj/ = 216 MB, DELETE at the end; base-src/
   base-target = release mortar at d23258b, DELETE at the end). Store half:
   `crates/mortar_input/tests/exp7.rs` (#[ignore], MORTAR_EXP7=...).
   Sizes done (sizes.py): protoc 168 units 14.57 MB = 31.4 % of the objects,
   median 71 KB, 88 > 64 KiB; heavy TU 112,392 B (MT-41 said ~92 KB) =
   23.6 % of -O0, 10.7 % of -O0 -g; type names dominate (avg ~107 B).
   Outputs: base == branch == branch with containers (cmp).
6. Record `spec/landed/meta/WP-071.md`: NOT STARTED (findings list above
   and in the hand-back notes: MT-35 fixed offsets, bad magic/unit_size
   ends the section, MT-14 unsorted/duplicate kinds, strtab rules, key vs
   fnv(name) unchecked, MT-2 not checked by reader, Meta list in
   Resolution, new dependency edges resolve->meta, input->meta,
   xtask->meta).

## Gate

Ran on 573b975 (log `/Volumes/AgentWork/mortar-helper/.build/wp071-gate.log`):
fmt, clippy, test, linktest passed; static-identical failed only because
of 6 NEW links (mason-meta.test, 2 links x 3 arches, all `+`; the 182 old
links unchanged). The baseline now has them (`static-identical --update`,
+6 lines in tests/identical/baseline.tsv); `static-identical` ok after
that. The brief's gate line therefore becomes "188 links of 44 tests; 0
differ": say so in the record. The full gate has not run again since the
baseline commit; run it on the final head.

## Next concrete step

Experiment 7 (item 5), then the record, the gate, delete WIP.md.
