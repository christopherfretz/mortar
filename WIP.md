# WP-071 work in progress (deleted in the final commit)

Checkpoint 2026-11-15 14:3x ET (supervisor restart). Briefs:
`briefs/m6-common.md`, `briefs/wp-071.md` (workspace root). Merges of
main used: 2 of 3 (last at main 862977c).

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
   T-MT-5 (strip/objcopy table re-run): NOT DONE (optional; say so).
5. Experiment 7: NOT STARTED. Plan:
   - Objects: bench workload `cxx-medium-static` (protoc, g++ -O0) at
     `/Volumes/AgentWork/mortar-helper/.build/bench-work/workloads/cxx-medium-static/`
     (`response.txt` = the link line; its objects are main.o +
     libprotoc.a + libprotobuf.a members, 167 of protoc's). The heavy TU:
     meta.md §10 table (kiln -O0 object 475,720 B; 595 vague, 661 globals,
     334 types) — find it under `.build/review-meta*` or rebuild per
     meta.md; else use the table's counts.
   - Per object: rows = kind 2 per vague (STB_WEAK/GNU_UNIQUE defined)
     symbol, kind 5 per defined+undefined global, kind 3 per DWARF type
     (use the -g workload `cxx-medium-g-static` for counts, or the
     table's proxy). Write units with mortar_meta's writer (anchored rows)
     — e.g. a small `xtask meta` helper or an `#[ignore]` test — add them
     with `meta write --into`, and relink.
   - Measure: unit sizes by kind (sum, median, max vs MT-41's ~92 KB);
     bytes read with containers unwanted (hook: must be 0 except the ELF32
     head case) and wanted; read+group cost (O-22) vs O-45's 3 %.
   - Bench before/after (cost of this package when unused): release
     mortar at merge-base 862977c (export with `git archive` to the /tmp
     scratch, separate CARGO_TARGET_DIR under .build, delete after) vs this
     branch; `--thread-count 1`; median of 15 on protoc
     (`cxx-medium-static` response.txt), hello-c-static,
     synthetic-objects-static. Expected: +2 reads per small top-level
     object, +1 per small non-object file, +2 per large object.
   - Set MT-41's gate in the record from the numbers.
6. Record `spec/landed/meta/WP-071.md`: NOT STARTED (findings list above
   and in the hand-back notes: MT-35 fixed offsets, bad magic/unit_size
   ends the section, MT-14 unsorted/duplicate kinds, strtab rules, key vs
   fnv(name) unchecked, MT-2 not checked by reader, Meta list in
   Resolution, new dependency edges resolve->meta, input->meta,
   xtask->meta).

## Gate

Not yet run on this head; run `MORTAR_REQUIRE_TOOLS=1 cargo xtask ci
--quick` in the background (must end `[ci] ok`, static-identical 182
links, 0 differ).

## Next concrete step

Experiment 7 (item 5), then the record, the gate, delete WIP.md.
