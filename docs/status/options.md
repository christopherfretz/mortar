# Options

Generated from `crates/mortar_cli/src/rows.rs` by `mortar_cli::status_markdown`;
do not edit. `MORTAR_BLESS=1 cargo test -p mortar_cli --test table` rewrites it.
The rules are `spec/cli.md`.

487 rows: 110 supported, 8 partial, 29 noop, 340 refused.

At `-r` (`spec/cli.md` C-R2b): 25 act (11 of them refused until their package lands), 151 noop, 271 as in a final link, 21 conflicting-options with -r, 19 refused.

Origin: 292 GNU ld, 117 lld only, 21 mold only, 39 lld and mold only, 18 mortar's own.

- AArch64: 14 rows of that target only
- x86-64: 26 rows of that target only
- i386: 18 rows of that target only

## Output

| Option | Value | Linkers | Targets | Status | At `-r` | Lands in | Help |
|---|---|---|---|---|---|---|---|
| `-o`, `--output` | FILE | GNU, lld, mold | all | supported | acts (C-R3, I-R1) | `output` | Write the output to FILE (default a.out) |
| `-shared`, `--Bshareable` |  | GNU, lld, mold | all | partial: AArch64 and x86-64 (i386 refused, `feature`, until WP-130; local-exec TLS in a shared object refused, dyn.md D-119; `extern "C++"` version-script entries refused, dyn.md D-114). Not caught (with every reference): non-PIC AArch64 code that reaches its own exported data only through a form dyn.md D-117's analysis does not follow (a spilled or reloaded base, a computed index, a 32-bit register, a value passed to a callee, code outside a sized function symbol, hand-written assembly; 3 of 306 measured bound definitions, every object refused for another use); an object whose `reloc-model` row (hod's) says `static` or `pie` and that defines an exported preemptible datum is refused whatever that analysis finds (meta.md MT-26, WP-129). Refused although correct: -fPIC code that forms an address before the start of a static object placed after exported data (`tab - 1`, GCC and Clang AArch64 -O0; dyn.md D-117; remedy -fdata-sections). Not judged on x86-64 (x64.md X-62): a section-relative PC-relative field in code naming exported data (none measured in GCC, Clang or kiln output at any -O) | conflicting-options with -r (C-R1) | `kind` | Produce a shared object |
| `-pie`, `--pic-executable` |  | GNU, lld, mold | all | supported | conflicting-options with -r (C-R1) | `kind` | Produce a position-independent executable |
| `-no-pie`, `--no-pic-executable` |  | GNU, lld, mold | all | supported | noop (C-R2) | `kind` | Produce a position-dependent executable (default) |
| `-r`, `-i`, `--relocatable` |  | GNU, lld, mold | all | refused | acts: the kind; refused until RL-7 (`relocatable output (-r)`) (C-R1) | `kind` | Produce a relocatable object |
| `-m` | EMULATION | GNU, lld, mold | all | supported | noop: a wrong emulation still refused (C-R2) | `emulation` | Link for EMULATION: aarch64linux, aarch64elf, elf_x86_64, elf_i386 |
| `-e`, `--entry` | SYMBOL | GNU, lld, mold | all | supported | acts: accepted, e_entry 0, not a root (C-R3, L-R7, RV-R2) | `entry` | Start execution at SYMBOL |
| `--oformat` | FORMAT | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `other` | Write the output in FORMAT |
| `-Ur` |  | GNU | all | refused | acts: as `-r` (`plan.ur`); refused until RL-7 (`relocatable output (-r)`) (C-R1) | `ur` | Produce a relocatable object with constructor tables built |
| `-d`, `--dc`, `--dp` |  | GNU, lld, mold | all | refused | acts: commons allocated; refused until RL-11 (`-d with -r`) (C-R3, RV-R5) | `define_common` | Allocate common symbols even in a relocatable link |
| `--no-define-common` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Leave common symbols unallocated |
| `-q`, `--emit-relocs` |  | GNU, lld, mold | all | refused | noop: the class at -r wins (C-R2, C-R2b) | `shaping.emit_relocs` | Keep relocations in the output |
| `-s`, `--strip-all` |  | GNU, lld, mold | all | supported | acts; refused until RL-11 (`-s with -r`) (C-R3, I-R9) | `shaping.strip` | Omit all symbol information |
| `-S`, `--strip-debug` |  | GNU, lld, mold | all | supported | acts; refused until RL-11 (`-S with -r`) (C-R3, I-R9) | `shaping.strip` | Omit debugging information |
| `--strip-discarded` |  | GNU | all | noop: synth.md S-76: locals of sections that are not live are always dropped (the default) | noop: measured (C-R2b, rl0-opts) | `other` | Omit symbols of discarded sections |
| `--no-strip-discarded` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Keep symbols of discarded sections |
| `-x`, `--discard-all` |  | GNU, lld, mold | all | supported | acts; refused until RL-11 (`-x with -r`) (C-R3, I-R9) | `shaping.discard` | Omit all local symbols |
| `-X`, `--discard-locals` |  | GNU, lld, mold | all | supported | noop (C-R2) | `shaping.discard` | Omit temporary local symbols |
| `--discard-none` |  | GNU, lld | all | supported | noop: measured (C-R2b, rl0-opts) | `shaping.discard` | Keep every local symbol |
| `-n`, `--nmagic` |  | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `shaping.magic` | Do not page-align sections |
| `-N`, `--omagic` |  | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `shaping.magic` | Do not page-align sections; make text writable |
| `--no-omagic` |  | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `shaping.magic` | Page-align sections; text read-only (default) |
| `--no-nmagic` |  | lld, mold | all | refused | as in a final link (C-R2b) | `shaping.magic` | Page-align sections (default) |
| `--build-id` | optional, `=` only: STYLE (style or 0xHEX) | GNU, lld, mold | all | partial: `=md5`, `=uuid` refused (synth.md S-91: md5 is not written and uuid is M2; mortar writes --build-id, =sha1, =sha256, =fast, =none and =0xHEX) | acts: the note over the -r output (C-R3, I-R8) | `shaping.build_id` | Write a build-ID note: md5, sha1, sha256, fast, uuid, none or 0xHEX |
| `--no-build-id` |  | mold | all | supported | noop (C-R2) | `shaping.build_id` | Write no build-ID note |
| `--compress-debug-sections` | none \| zlib \| zlib-gnu \| zlib-gabi \| zstd | GNU, lld, mold | all | partial: `=zlib`, `=zlib-gnu`, `=zlib-gabi`, `=zstd` refused (image.md: debug sections are written uncompressed; mortar writes =none only) | refused: `--compress-debug-sections with -r`; `=none` no-op (C-R4) | `shaping.compress_debug_sections` | Compress the debugging sections |
| `--eh-frame-hdr` |  | GNU, lld, mold | all | supported | noop (C-R2) | `shaping.eh_frame_hdr` | Write .eh_frame_hdr and PT_GNU_EH_FRAME |
| `--no-eh-frame-hdr` |  | GNU, lld, mold | all | supported | noop: measured (C-R2b, rl0-opts) | `shaping.eh_frame_hdr` | Write no .eh_frame_hdr |
| `--package-metadata` | optional, `=` only: JSON | GNU, mold | all | refused | as in a final link (C-R2b) | `other` | Write a .note.package note |
| `--gc-sections` |  | GNU, lld, mold | all | supported | refused: `--gc-sections with -r` (C-R4, I-R10) | `shaping.gc_sections` | Remove sections nothing refers to |
| `--no-gc-sections` |  | GNU, lld, mold | all | supported | noop: measured (C-R2b, rl0-opts) | `shaping.gc_sections` | Keep every section (default) |
| `--print-gc-sections` |  | GNU, lld, mold | all | supported | noop: measured (C-R2b, rl0-opts) | `shaping.print_gc_sections` | List the sections --gc-sections removes |
| `--no-print-gc-sections` |  | GNU, lld, mold | all | supported | noop: measured (C-R2b, rl0-opts) | `shaping.print_gc_sections` | Do not list removed sections (default) |
| `--gc-keep-exported` |  | GNU | all | supported | noop: measured (C-R2b, rl0-opts) | `shaping.gc_keep_exported` | Keep sections that define exported symbols |
| `--icf` | all \| safe \| none | lld, mold | all | refused | refused: `--icf with -r` (C-R4) | `shaping.icf` | Fold identical code |
| `--no-icf` |  | mold | all | refused | as in a final link (C-R2b) | `shaping.icf` | Fold nothing (default) |
| `--print-icf-sections` |  | lld, mold | all | refused | as in a final link (C-R2b) | `other` | List the folded sections |
| `--no-print-icf-sections` |  | lld, mold | all | refused | as in a final link (C-R2b) | `other` | Do not list folded sections (default) |
| `--keep-unique` | SYMBOL | lld | all | refused | as in a final link (C-R2b) | `other` | Do not fold SYMBOL |
| `--ignore-data-address-equality` |  | lld, mold | all | refused | as in a final link (C-R2b) | `other` | Allow folding data whose address is compared |
| `--ignore-function-address-equality` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Allow folding functions whose address is compared |
| `--gdb-index` |  | lld, mold | all | supported | refused: `--gdb-index with -r` (C-R4) | `shaping.gdb_index` | Write a .gdb_index section |
| `--no-gdb-index` |  | lld, mold | all | noop: the default: no .gdb_index is written; the last of --gdb-index and --no-gdb-index wins | noop: measured (C-R2b, rl0-opts (lld)) | `shaping.gdb_index` | Write no .gdb_index section (default) |
| `--debug-names` |  | lld | all | refused: gdb 16 ignores a .debug_names it did not write; use --gdb-index (spec/synth.md S-G9) | as in a final link (C-R2b) | `other` | Write a merged .debug_names section |
| `--no-debug-names` |  | lld | all | noop: the default: no merged .debug_names is written | noop: measured (C-R2b, rl0-opts (lld)) | `other` | Write no merged .debug_names (default) |
| `--relax` |  | GNU, lld, mold | all | refused | refused: `--relax with -r` (C-R4) | `other` | Relax instructions where the target allows |
| `--no-relax` |  | GNU, lld, mold | all | refused | noop: the class at -r wins (C-R2, C-R2b) | `other` | Do not relax instructions |
| `--force-group-allocation` |  | GNU, lld | all | refused | acts; refused until RL-8 (`--force-group-allocation with -r`) (C-R3, L-R3) | `force_group_allocation` | Place section group members as ordinary sections |
| `--traditional-format` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Write what the system's native linker would |
| `--embedded-relocs` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Write embedded relocations (MIPS) |
| `--force-exe-suffix` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Add .exe to the output name |
| `--out-implib` | FILE | GNU, lld | all | refused | as in a final link (C-R2b) | `other` | Write an import library |
| `--execute-only` |  | lld, mold | all | refused | as in a final link (C-R2b) | `other` | Make executable segments unreadable |
| `--no-execute-only` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Make executable segments readable (default) |
| `--compress-sections` | GLOB=TYPE | lld | all | refused | as in a final link (C-R2b) | `other` | Compress the non-alloc output sections GLOB names |
| `--separate-debug-file` | optional, `=` only: FILE | mold | all | refused | as in a final link (C-R2b) | `other` | Write the debugging information to a separate file |
| `--no-separate-debug-file` |  | mold | all | refused | as in a final link (C-R2b) | `other` | Keep the debugging information in the output (default) |

## Inputs and their positional state

| Option | Value | Linkers | Targets | Status | At `-r` | Lands in | Help |
|---|---|---|---|---|---|---|---|
| `-l`, `--library` | NAME | GNU, lld, mold | all | supported | noop: picks the inputs, extraction by RV-R2 (C-R2, RV-R2) | `inputs` | Link libNAME (or :FILE, by file name) |
| `-R`, `--just-symbols` | FILE | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `inputs` | Take only the symbols of FILE |
| `-b`, `--format` | FORMAT | GNU, lld | all | refused | acts: `binary` (L-R9); refused until RL-10 (`-b with -r`) (C-R3, L-R9) | `inputs[].state.format` | Read the following inputs as FORMAT |
| `-(`, `--start-group` |  | GNU, lld, mold | all | supported | noop: acts on extraction (C-R2, RV-R2) | `inputs[].state.group` | Start a group of archives searched repeatedly |
| `-)`, `--end-group` |  | GNU, lld, mold | all | supported | noop (C-R2) | `inputs[].state.group` | End a group |
| `--start-lib` |  | lld, mold | all | supported | as in a final link: lazy objects (lld's reading) (C-R2b, RV-R2) | `inputs[].state.lib` | Read the following objects as members of an archive |
| `--end-lib` |  | lld, mold | all | supported | as in a final link: with `--start-lib` (RV-R2) | `inputs[].state.lib` | End --start-lib |
| `--as-needed` |  | GNU, lld, mold | all | supported | noop (C-R2) | `inputs[].state.as_needed` | Record DT_NEEDED for the following shared objects only if used |
| `--no-as-needed` |  | GNU, lld, mold | all | supported | noop (C-R2) | `inputs[].state.as_needed` | Record DT_NEEDED for every following shared object (default) |
| `--whole-archive` |  | GNU, lld, mold | all | supported | noop: acts on extraction (C-R2, RV-R2) | `inputs[].state.whole_archive` | Take every member of the following archives |
| `--no-whole-archive` |  | GNU, lld, mold | all | supported | noop (C-R2) | `inputs[].state.whole_archive` | Take the members of archives that are needed (default) |
| `-Bstatic`, `--dn`, `--non_shared`, `--static` |  | GNU, lld, mold | all | supported | noop: a `.so` is refused (C-R2, C-R5) | `inputs[].state.link_static` | Search -l for archives only |
| `-Bdynamic`, `--dy`, `--call_shared` |  | GNU, lld, mold | all | supported | noop: a `.so` is refused (C-R2, C-R5) | `inputs[].state.link_static` | Search -l for shared objects too (default) |
| `--copy-dt-needed-entries`, `--add-needed` |  | GNU | all | refused | as in a final link (C-R2b) | `inputs[].state.copy_dt_needed` | Follow the DT_NEEDED entries of the following shared objects |
| `--no-copy-dt-needed-entries`, `--no-add-needed` |  | GNU, lld, mold | all | noop: dyn.md D-15: the default; a library reached only through another's DT_NEEDED entries never defines a symbol of the link (D-12), and --copy-dt-needed-entries is refused | noop: measured (C-R2b, rl0-opts) | `inputs[].state.copy_dt_needed` | Do not follow DT_NEEDED entries (default) |
| `--push-state` |  | GNU, lld, mold | all | supported | noop (C-R2) | `inputs[].state (saved, restored)` | Save the positional state |
| `--pop-state` |  | GNU, lld, mold | all | supported | noop (C-R2) | `inputs[].state (saved, restored)` | Restore the positional state last saved |
| `--accept-unknown-input-arch` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Accept inputs of an unknown architecture |
| `--no-accept-unknown-input-arch` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Reject inputs of an unknown architecture (default) |
| `--no-warn-mismatch` |  | GNU, lld | all | refused | as in a final link (C-R2b) | `other` | Link inputs that do not match the output's machine |
| `--no-warn-search-mismatch` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Do not warn about skipped incompatible libraries |
| `--remap-inputs-file` | FILE | GNU, lld | all | refused | as in a final link (C-R2b) | `other` | Remap inputs as FILE says |
| `--remap-inputs` | PATTERN=FILE | GNU, lld | all | refused | as in a final link (C-R2b) | `other` | Read FILE for the inputs matching PATTERN |
| `--dependent-libraries` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Link the libraries inputs ask for (default) |
| `--no-dependent-libraries` |  | lld | all | noop: mortar never loads the libraries .deplibs sections name (resolve.md §10) | noop: measured (C-R2b, rl0-opts (lld)) | `other` | Ignore the libraries inputs ask for (mortar's behavior, always on) |
| `--fortran-common` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Take archive members that define common symbols (mortar's behavior, always on) |
| `--no-fortran-common` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Do not take archive members for common symbols |
| `--warn-backrefs` |  | lld | all | supported | as in a final link: a diagnostic (rl0-opts) | `diagnostics.warn_backrefs` | Warn when an archive member is taken for an earlier reference |
| `--no-warn-backrefs` |  | lld | all | supported | as in a final link (rl0-opts) | `diagnostics.warn_backrefs` | Do not warn about backward references (default) |
| `--warn-backrefs-exclude` | GLOB | lld | all | refused | as in a final link (C-R2b) | `other` | Do not warn about backward references into GLOB |
| `--why-extract` | FILE | lld | all | refused | as in a final link (C-R2b) | `other` | Write why each archive member is taken to FILE |
| `--print-archive-stats` | FILE | lld | all | refused | as in a final link (C-R2b) | `other` | Write archive statistics to FILE |
| `--fat-lto-objects` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Read the bitcode of fat LTO objects |
| `--no-fat-lto-objects` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Read the code of fat LTO objects (default) |

## Library search

| Option | Value | Linkers | Targets | Status | At `-r` | Lands in | Help |
|---|---|---|---|---|---|---|---|
| `-L`, `--library-path` | DIR | GNU, lld, mold | all | supported | noop (C-R2) | `library_paths` | Search DIR for -l libraries and scripts |
| `--sysroot` | DIR | GNU, lld, mold | all | supported | as in a final link: the search (rl0-opts) | `sysroot` | Use DIR as the system root |
| `-nostdlib` |  | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `nostdlib` | Do not search the SEARCH_DIR directories of linker scripts |
| `-rpath-link` | DIR | GNU, lld, mold | all | supported | noop: measured (C-R2b, rl0-opts) | `dynamic.rpath_link` | Search DIR for the DT_NEEDED entries of shared objects; a :-separated list |
| `-Y` | PATH | GNU | all | refused | as in a final link (C-R2b) | `other` | Default search path for Solaris compatibility |
| `-C`, `--directory` | DIR | mold | all | refused | as in a final link (C-R2b) | `other` | Change to DIR first |
| `--chroot` | DIR | lld, mold | all | refused | as in a final link (C-R2b) | `other` | Prefix every path with DIR |

## Scripts and lists

| Option | Value | Linkers | Targets | Status | At `-r` | Lands in | Help |
|---|---|---|---|---|---|---|---|
| `-T`, `--script` | FILE | GNU, lld, mold | all | refused | acts; refused until RL-11 (`-T with -r`) (C-R3) | `scripts` | Read the linker script FILE |
| `--default-script`, `--dT` | FILE | GNU, lld | all | refused | as in a final link (C-R2b) | `scripts` | Use FILE as the default linker script |
| `-c`, `--mri-script` | FILE | GNU | all | refused | as in a final link (C-R2b) | `other` | Read the MRI script FILE |
| `--version-script` | FILE | GNU, lld, mold | all | supported | noop (C-R2) | `scripts` | Read the version script FILE |
| `--dynamic-list` | FILE | GNU, lld, mold | all | supported | conflicting-options with -r (C-R4) | `scripts` | Read the dynamic list FILE |
| `--export-dynamic-symbol-list` | FILE | GNU, lld, mold | all | refused | conflicting-options with -r (C-R4) | `scripts` | Export the symbols FILE lists |
| `--retain-symbols-file` | FILE | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `scripts` | Keep only the symbols FILE lists |
| `--section-ordering-file` | FILE | GNU | all | refused | as in a final link (C-R2b) | `other` | Order sections as FILE says |
| `--symbol-ordering-file` | FILE | lld | all | refused | as in a final link (C-R2b) | `other` | Order sections by the symbols FILE lists |
| `--no-warn-symbol-ordering` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Do not warn about the symbol ordering file |
| `--warn-symbol-ordering` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Warn about the symbol ordering file (default) |
| `--call-graph-ordering-file` | FILE | lld | all | refused | as in a final link (C-R2b) | `other` | Order sections by the call graph FILE gives |
| `--call-graph-profile-sort` | none \| hfsort \| cdsort | lld | all | refused | noop: by what the option is (C-R2b) | `other` | Order sections by the call graph profile |
| `--print-symbol-order` | FILE | lld | all | refused | as in a final link (C-R2b) | `other` | Write the symbol order used to FILE |
| `--irpgo-profile` | FILE | lld | all | refused | as in a final link (C-R2b) | `other` | Read a temporal profile |
| `--bp-startup-sort` | none \| function | lld | all | refused | noop: by what the option is (C-R2b) | `other` | Order sections for startup |
| `--bp-compression-sort` | none \| function \| data \| both | lld | all | refused | noop: by what the option is (C-R2b) | `other` | Order sections for compression |
| `--bp-compression-sort-startup-functions` |  | lld | all | refused | noop: by what the option is (C-R2b) | `other` | Order startup functions for compression too |
| `--verbose-bp-section-orderer` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Report on balanced partitioning |
| `--orphan-handling` | place \| discard \| warn \| error | GNU, lld | all | refused | noop: by what the option is (C-R2b) | `other` | Handle sections no script statement places |
| `--sort-common`, `--sort_common` | optional, `=` only: ascending \| descending | GNU, lld, mold | all | supported | noop (C-R2) | `shaping.sort_common` | Sort common symbols by alignment |
| `--sort-section` | name \| alignment | GNU, lld, mold | all | supported | noop (C-R2) | `shaping.sort_section` | Sort sections matched by wildcards |
| `--unique` | optional, `=` only: SECTION | GNU, lld, mold | all | refused | acts; refused until RL-11 (`--unique with -r`) (C-R3, C-R4, L-R11) | `unique` | Do not merge orphan sections (or SECTION) |
| `--enable-non-contiguous-regions` |  | GNU, lld | all | refused | as in a final link (C-R2b) | `other` | Spill sections into later matching memory regions |
| `--enable-non-contiguous-regions-warnings` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Warn where non-contiguous regions can surprise |
| `--disable-linker-version` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Make the LINKER_VERSION script command write nothing |
| `--enable-linker-version` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Make the LINKER_VERSION script command write the version |
| `--check-sections` |  | GNU, lld | all | refused | as in a final link (C-R2b) | `other` | Check output sections for overlaps (default) |
| `--no-check-sections` |  | GNU, lld | all | refused | as in a final link (C-R2b) | `other` | Do not check output sections for overlaps |
| `--shuffle-sections` | optional, `=` only: GLOB=SEED | lld, mold | all | refused | as in a final link (C-R2b) | `other` | Shuffle the sections GLOB matches |
| `--reverse-sections` |  | mold | all | refused | as in a final link (C-R2b) | `other` | Reverse the order of input sections |
| `--randomize-section-padding` | SEED | lld | all | refused | as in a final link (C-R2b) | `other` | Insert random padding between sections |

## Symbols

| Option | Value | Linkers | Targets | Status | At `-r` | Lands in | Help |
|---|---|---|---|---|---|---|---|
| `-u`, `--undefined` | SYMBOL | GNU, lld, mold | all | supported | acts: a root, and `UND` written (C-R3, RV-R2, RV-R3) | `symbols` | Start with an undefined reference to SYMBOL |
| `--require-defined` | SYMBOL | GNU, mold | all | supported | acts: a root for extraction like `-u`, nothing written; undefined at the end reported as in a final link (RL-2 measures) (C-R2b, RV-R2) | `symbols` | Require that SYMBOL is defined |
| `--undefined-glob` | PATTERN | lld, mold | all | refused | as in a final link (C-R2b) | `other` | Add an undefined reference for each symbol PATTERN matches |
| `--defsym` | SYMBOL=EXPR | GNU, lld, mold | all | partial: resolve.md §7.2: EXPR is a number (0x... hexadecimal, or decimal without a leading 0), a symbol, or a symbol + or - a number; a number with a leading 0 (GNU ld reads it octal, lld and mold decimal: write it in hexadecimal) and every other expression (k/M suffixes, arithmetic, parentheses, shifts, ALIGN, two symbols) are refused by name | acts; refused until RL-11 (`--defsym with -r`) (C-R3, RV-R10) | `symbols` | Define SYMBOL as EXPR |
| `--wrap` | SYMBOL | GNU, lld, mold | all | partial: resolve.md §7.1: a versioned SYMBOL (f@V: GNU ld and lld wrap the reference f@V, mold f) and --wrap of another --wrap's __real_ name (GNU ld wraps it, lld takes the last option, mold binds it to the symbol) are refused by name | refused: `--wrap with -r` (C-R4) | `symbols` | Send references to SYMBOL to __wrap_SYMBOL |
| `--export-dynamic-symbol` | SYMBOL | GNU, lld, mold | all | supported | conflicting-options with -r: C-R4's `--export-dynamic*` (C-R4) | `symbols` | Export SYMBOL (a glob) |
| `--exclude-libs` | LIBS | GNU, lld, mold | all | partial: dyn.md D-110 item 1: an entry is an archive's file name (ALL: every archive); an entry without .a that names an archive's file name without its .a (libfoo for libfoo.a) is refused by name (GNU ld matches it, lld and mold do not) | noop (C-R2) | `symbols` | Do not export the symbols of the archives LIBS |
| `-y`, `--trace-symbol` | SYMBOL | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `symbols` | Report each file that mentions SYMBOL |
| `--ignore-unresolved-symbol` | SYMBOL | GNU | all | refused | as in a final link (C-R2b) | `symbols` | Do not report SYMBOL as unresolved |
| `-init` | SYMBOL | GNU, lld, mold | all | supported | conflicting-options with -r (C-R2b) | `dynamic.init` | Call SYMBOL when the object is loaded |
| `-fini` | SYMBOL | GNU, lld, mold | all | supported | conflicting-options with -r (C-R2b) | `dynamic.fini` | Call SYMBOL when the object is unloaded |
| `--demangle` | optional, `=` only: STYLE | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `other` | Demangle symbol names in messages |
| `--no-demangle` |  | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `other` | Do not demangle symbol names |
| `--default-symver` |  | GNU, mold | all | refused | as in a final link (C-R2b) | `other` | Give exported symbols the output's version |
| `--default-imported-symver` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Give imported symbols their library's version |
| `--undefined-version` |  | GNU, lld, mold | all | supported | noop: measured (C-R2b, rl0-opts) | `dynamic.undefined_version` | Accept version scripts that name undefined symbols |
| `--no-undefined-version` |  | GNU, lld, mold | all | supported | noop: measured (C-R2b, rl0-opts) | `dynamic.undefined_version` | Reject version scripts that name undefined symbols |
| `--version-exports-section` | SYMBOL | GNU | all | refused | as in a final link (C-R2b) | `other` | Take the exported symbols from .exports |
| `--disable-multiple-abs-defs` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Reject absolute symbols defined twice through -R |
| `--gnu-unique` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Keep the STB_GNU_UNIQUE binding (default) |
| `--no-gnu-unique` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Make STB_GNU_UNIQUE symbols global |
| `--task-link` | SYMBOL | GNU | all | refused | as in a final link (C-R2b) | `other` | Link at task level |

## Dynamic linking

| Option | Value | Linkers | Targets | Status | At `-r` | Lands in | Help |
|---|---|---|---|---|---|---|---|
| `-E`, `--export-dynamic` |  | GNU, lld, mold | all | supported | conflicting-options with -r (C-R4) | `dynamic.export_dynamic` | Export every symbol |
| `--no-export-dynamic` |  | GNU, lld, mold | all | supported | noop: by what the option is (C-R2b) | `dynamic.export_dynamic` | Export only the symbols that need it (default) |
| `-Bsymbolic` |  | GNU, lld, mold | all | supported | conflicting-options with -r (C-R4) | `dynamic.bsymbolic` | Bind a shared object's references to its own definitions (DT_SYMBOLIC); nothing in an executable |
| `-Bsymbolic-functions` |  | GNU, lld, mold | all | supported | conflicting-options with -r (C-R4) | `dynamic.bsymbolic` | Bind references to global functions locally |
| `--Bsymbolic-non-weak` |  | lld, mold | all | refused: dyn.md D-112: GNU ld does not know it (lld and mold do); use -Bsymbolic-functions, or a --dynamic-list | conflicting-options with -r (C-R4) | `dynamic.bsymbolic` | Bind references to non-weak globals locally |
| `--Bsymbolic-non-weak-functions` |  | lld, mold | all | refused: dyn.md D-112: GNU ld does not know it (lld and mold do); use -Bsymbolic-functions, or a --dynamic-list | conflicting-options with -r (C-R4) | `dynamic.bsymbolic` | Bind references to non-weak global functions locally |
| `-Bno-symbolic` |  | GNU, lld, mold | all | supported | noop: by what the option is (C-R2b) | `dynamic.bsymbolic` | Undo -Bsymbolic (default) |
| `-h`, `-soname` | NAME | GNU, lld, mold | all | supported | conflicting-options with -r (C-R4) | `dynamic.soname` | Record NAME as DT_SONAME |
| `-rpath` | DIR | GNU, lld, mold | all | supported | conflicting-options with -r (C-R4) | `dynamic.rpath` | Add DIR to the run-time search path |
| `-I`, `--dynamic-linker` | FILE | GNU, lld, mold | all | supported | noop (C-R2) | `dynamic.dynamic_linker` | Use FILE as the program interpreter |
| `--no-dynamic-linker` |  | GNU, lld, mold | all | supported | noop (C-R2) | `dynamic.dynamic_linker` | Write no program interpreter |
| `--enable-new-dtags` |  | GNU, lld, mold | all | supported | conflicting-options with -r (C-R4) | `dynamic.new_dtags` | Write DT_RUNPATH for -rpath |
| `--disable-new-dtags` |  | GNU, lld, mold | all | supported | noop: by what the option is (C-R2b) | `dynamic.new_dtags` | Write DT_RPATH for -rpath |
| `--hash-style` | sysv \| gnu \| both \| none | GNU, lld, mold | all | partial: `=sysv`, `=both`, `=none` refused (dyn.md D-20: an executable gets .gnu.hash and no .hash; mortar writes =gnu) | noop (C-R2) | `dynamic.hash_style` | Write DT_HASH (sysv), DT_GNU_HASH (gnu), both or none |
| `--pack-dyn-relocs` | none \| relr \| android \| android+relr | lld, mold | all | refused | conflicting-options with -r (C-R2b) | `dynamic.pack_dyn_relocs` | Pack dynamic relocations |
| `-f`, `--auxiliary` | NAME | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `other` | Record NAME as DT_AUXILIARY |
| `-F`, `--filter` | NAME | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `other` | Record NAME as DT_FILTER |
| `--audit` | LIB | GNU | all | refused | as in a final link (C-R2b) | `other` | Record LIB as DT_AUDIT |
| `-P`, `--depaudit` | LIB | GNU | all | refused | as in a final link (C-R2b) | `other` | Record LIB as DT_DEPAUDIT |
| `-Bgroup` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Use group lookup for this object's dependencies |
| `--dynamic-list-data` |  | GNU, mold | all | refused: dyn.md D-112: the references disagree (GNU ld binds functions and NOTYPE symbols, mold changes nothing, lld does not know it); use -Bsymbolic-functions, or a --dynamic-list | conflicting-options with -r (C-R4) | `other` | Add the data symbols to the dynamic list |
| `--dynamic-list-cpp-new` |  | GNU | all | refused: dyn.md D-112: GNU ld only (lld and mold refuse it); list the symbols in a --dynamic-list | conflicting-options with -r (C-R4) | `other` | Add operator new and delete to the dynamic list |
| `--dynamic-list-cpp-typeinfo` |  | GNU | all | refused: dyn.md D-112: GNU ld only (lld and mold refuse it); list the symbols in a --dynamic-list | conflicting-options with -r (C-R4) | `other` | Add the C++ type information to the dynamic list |
| `--allow-shlib-undefined` |  | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `dynamic.allow_shlib_undefined` | Accept undefined symbols in shared objects |
| `--no-allow-shlib-undefined` |  | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `dynamic.allow_shlib_undefined` | Report undefined symbols in shared objects |
| `--spare-dynamic-tags` | COUNT (number) | GNU, mold | all | refused | as in a final link (C-R2b) | `other` | Reserve COUNT entries in .dynamic |
| `--spare-program-headers` | COUNT (number) | mold | all | refused | as in a final link (C-R2b) | `other` | Reserve COUNT program headers |
| `--apply-dynamic-relocs` |  | lld, mold | all | refused | as in a final link (C-R2b) | `other` | Write link-time values where dynamic relocations apply |
| `--use-android-relr-tags` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Use the Android RELR tags |
| `--no-use-android-relr-tags` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Use the SHT_RELR tags (default) |
| `--android-memtag-heap` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Ask the loader for MTE heap protection |
| `--android-memtag-stack` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Ask the loader for MTE stack protection |
| `--android-memtag-mode` | async \| sync \| none | lld | all | refused | conflicting-options with -r (C-R2b) | `other` | The MTE mode the loader starts in |

## Sections, segments and addresses

| Option | Value | Linkers | Targets | Status | At `-r` | Lands in | Help |
|---|---|---|---|---|---|---|---|
| `-Ttext` | ADDRESS | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `shaping.starts` | Place .text at ADDRESS |
| `-Tdata` | ADDRESS | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `shaping.starts` | Place .data at ADDRESS |
| `-Tbss` | ADDRESS | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `shaping.starts` | Place .bss at ADDRESS |
| `-Ttext-segment` | ADDRESS | GNU, lld | all | refused | as in a final link (C-R2b) | `shaping.starts` | Place the text segment at ADDRESS |
| `-Trodata-segment` | ADDRESS | GNU | all | refused | as in a final link (C-R2b) | `shaping.starts` | Place the read-only data segment at ADDRESS |
| `-Tldata-segment` | ADDRESS | GNU | all | refused | as in a final link (C-R2b) | `shaping.starts` | Place the large data segment at ADDRESS |
| `--section-start` | SECTION=ADDRESS | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `shaping.starts` | Place SECTION at ADDRESS |
| `--image-base` | ADDRESS | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `shaping.image_base` | Place the image at ADDRESS |
| `--rosegment` |  | GNU, lld, mold | all | noop: layout.md L-65: read-only data always has a PT_LOAD of its own | noop: measured (C-R2b, rl0-opts) | `other` | Put read-only data in its own segment |
| `--no-rosegment` |  | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `other` | Do not give read-only data its own segment |
| `--xosegment` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Put execute-only sections in their own segment |
| `--no-xosegment` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Do not give execute-only sections their own segment (default) |
| `--split-by-file` | optional, `=` only: SIZE (number) | GNU | all | refused | as in a final link (C-R2b) | `other` | Split output sections every SIZE bytes, at file boundaries |
| `--split-by-reloc` | optional, `=` only: COUNT (number) | GNU | all | refused | as in a final link (C-R2b) | `other` | Split output sections every COUNT relocations |
| `-G`, `--gpsize` | SIZE (number) | GNU, lld | all | refused | as in a final link (C-R2b) | `other` | Small data size |
| `--optimize-bb-jumps` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Remove jumps to the next basic block |
| `--no-optimize-bb-jumps` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Keep jumps to the next basic block (default) |
| `--branch-to-branch` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Retarget branches to branches |
| `--no-branch-to-branch` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Do not retarget branches to branches |

## Diagnostics

| Option | Value | Linkers | Targets | Status | At `-r` | Lands in | Help |
|---|---|---|---|---|---|---|---|
| `--error-limit` | N (number) | lld | all | supported | as in a final link (rl0-opts (lld)) | `diagnostics.error_limit` | Show at most N errors; 0 shows all (default 20) |
| `--fatal-warnings` |  | GNU, lld, mold | all | supported | as in a final link (rl0-opts) | `diagnostics.fatal_warnings` | Treat warnings as errors |
| `--no-fatal-warnings` |  | GNU, lld, mold | all | supported | as in a final link (rl0-opts) | `diagnostics.fatal_warnings` | Do not treat warnings as errors (default) |
| `-w`, `--no-warnings` |  | GNU, lld | all | supported | as in a final link: never hides a refusal (rl0-opts, diag §4.3) | `diagnostics.no_warnings` | Report no warnings |
| `--color-diagnostics` | optional, `=` only: auto \| always \| never | lld, mold | all | supported | as in a final link (rl0-opts (lld)) | `diagnostics.color` | Color diagnostics: always (alone), auto (default) or never |
| `--no-color-diagnostics` |  | lld | all | supported | as in a final link (rl0-opts (lld)) | `diagnostics.color` | Do not color diagnostics |
| `--no-undefined` |  | GNU, lld, mold | all | supported | noop: nothing is undefined at -r (C-R2) | `z.defs` | Report undefined symbols even in a shared object |
| `--unresolved-symbols` | report-all \| ignore-all \| ignore-in-object-files \| ignore-in-shared-libs | GNU, lld, mold | all | refused | noop: by what the option is (C-R2b) | `diagnostics.unresolved_symbols` | How to treat unresolved symbols |
| `--warn-unresolved-symbols` |  | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `diagnostics.warn_unresolved_symbols` | Report unresolved symbols as warnings |
| `--error-unresolved-symbols` |  | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `diagnostics.warn_unresolved_symbols` | Report unresolved symbols as errors (default) |
| `--allow-multiple-definition` |  | GNU, lld, mold | all | supported | as in a final link (RV-R4) | `diagnostics.allow_multiple_definition` | Take the first of several definitions of a symbol |
| `--no-allow-multiple-definition` |  | lld | all | supported | as in a final link (RV-R4) | `diagnostics.allow_multiple_definition` | Report symbols defined more than once (default) |
| `--noinhibit-exec`, `--noinhibit_exec` |  | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `diagnostics.noinhibit_exec` | Write the output even after errors |
| `--error-handling-script` | SCRIPT | GNU, lld | all | refused | as in a final link (C-R2b) | `other` | Run SCRIPT for undefined symbols and missing libraries |
| `--warn-common` |  | GNU, lld, mold | all | supported | noop (C-R2) | `diagnostics.warn_common` | Warn about common symbols |
| `--no-warn-common` |  | lld, mold | all | supported | as in a final link (rl0-opts (lld)) | `diagnostics.warn_common` | Do not warn about common symbols (default) |
| `--warn-constructors` |  | GNU, mold | all | refused | as in a final link (C-R2b) | `other` | Warn about global constructors |
| `--warn-multiple-gp` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Warn about several GP values |
| `--warn-once` |  | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `other` | Warn about each undefined symbol once |
| `--warn-section-align` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Warn when alignment moves a section's start |
| `--warn-textrel`, `--warn-shared-textrel` |  | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `other` | Warn about text relocations |
| `--warn-alternate-em` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Warn about objects with an alternate ELF machine code |
| `--warn-execstack-objects` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Warn about objects that ask for an executable stack |
| `--warn-execstack` |  | GNU, lld, mold | all | noop: layout.md L-84: the executable-stack warning is the default | as in a final link (rl0-opts) | `other` | Warn when the stack is made executable |
| `--no-warn-execstack` |  | GNU, mold | all | refused | as in a final link (C-R2b) | `other` | Do not warn when the stack is made executable |
| `--error-execstack` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Make executable stack warnings errors |
| `--no-error-execstack` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Do not make executable stack warnings errors |
| `--warn-rwx-segments` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Warn about loadable segments that are writable and executable |
| `--no-warn-rwx-segments` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Do not warn about RWX segments |
| `--error-rwx-segments` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Make RWX segment warnings errors |
| `--no-error-rwx-segments` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Do not make RWX segment warnings errors |
| `--warn-ifunc-textrel` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Warn about ifunc symbols with text relocations |
| `--no-warn-ifunc-textrel` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Do not warn about ifunc text relocations (default) |
| `--vs-diagnostics` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Format diagnostics for Visual Studio |
| `--why-live` | GLOB | lld | all | refused | as in a final link (C-R2b) | `other` | Report what keeps the symbols GLOB matches alive |
| `--odr` | warn \| error \| strict \| off | mortar | all | supported | as in a final link: over the -r's units (MT-30a) | `diagnostics.odr.mode` | Check the one-definition rule: warn (default), error, strict or off |
| `--no-odr-check` |  | mortar | all | supported | as in a final link (MT-30a) | `diagnostics.odr.mode` | Run no one-definition-rule check (--odr=off) |
| `--odr-stats` |  | mortar | all | supported | as in a final link (MT-30a) | `diagnostics.odr.stats` | Print one line of one-definition-rule statistics |
| `--odr-check-archives` |  | mortar | all | supported | as in a final link (MT-30a) | `diagnostics.odr.check_archives` | Also compare the archive members that are not linked (odr.md O-23) |
| `--odr-table` | off \| exported \| full | mortar | all | supported | noop: by what the option is; the final link writes the table (C-R2b) | `diagnostics.odr.table` | Write a loadable one-definition-rule table: off (default), exported or full |
| `--keep-mason-meta` |  | mortar | all | supported | noop: by what the option is; MT-30 always writes the container (C-R2b, MT-30) | `diagnostics.odr.keep_meta` | Keep the inputs' Mason metadata in the output (meta.md MT-32) |
| `--detect-odr-violations` |  | mortar | all | noop: odr.md O-41: mortar checks the one-definition rule by default; --detect-odr-violations has no further effect | noop: gold's spelling, a no-op in a final link (C-R2b) | `other` | GNU gold's: check the one-definition rule |

## Maps, traces, threads and information

| Option | Value | Linkers | Targets | Status | At `-r` | Lands in | Help |
|---|---|---|---|---|---|---|---|
| `--version` |  | GNU, lld, mold | all | supported | as in a final link | `exit` | Print the version and exit |
| `-v` |  | GNU, lld, mold | all | supported | as in a final link | `print_version` | Print the version and go on |
| `-V` |  | GNU, lld, mold | all | supported | as in a final link | `print_version` | Print the version and the emulations and go on |
| `--help` |  | GNU, lld, mold | all | supported | as in a final link | `exit` | Print this help and exit |
| `-Map` | FILE | GNU, lld, mold | all | supported | noop: written as in any link (C-R2, I-R12) | `bookkeeping.map` | Write a link map to FILE |
| `-M`, `--print-map` |  | GNU, lld, mold | all | supported | as in a final link (rl0-opts) | `bookkeeping.print_map` | Write a link map to standard output |
| `--print-map-discarded` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Show discarded sections in the map (default) |
| `--no-print-map-discarded` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Do not show discarded sections in the map |
| `--print-map-locals` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Show local symbols in the map |
| `--no-print-map-locals` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Do not show local symbols in the map (default) |
| `--cref` |  | GNU, lld | all | refused | as in a final link (C-R2b) | `bookkeeping.cref` | Write a cross-reference table |
| `-t`, `--trace` |  | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `bookkeeping.trace` | Print the name of each input file |
| `--verbose` | optional, `=` only: LEVEL (number) | GNU, lld, mold | all | supported | as in a final link (rl0-opts) | `bookkeeping.verbose` | Print the version and the emulations, as -V does |
| `--dependency-file` | FILE | GNU, lld, mold | all | supported | as in a final link (rl0-opts) | `bookkeeping.dependency_file` | Write make dependencies of the link to FILE |
| `--reproduce` | FILE | lld | all | refused | as in a final link (C-R2b) | `bookkeeping.reproduce` | Write a tar archive that reproduces the link |
| `--repro` |  | mold | all | refused | as in a final link (C-R2b) | `other` | Embed the inputs in a .repro section |
| `--threads` | optional, `=` only: N (number) | lld, mold | all | supported | noop (C-R2) | `bookkeeping.threads` | Use N threads (alone: all processors) |
| `--thread-count` | N (number) | mold | all | refused | as in a final link (C-R2b) | `bookkeeping.threads` | Use N threads |
| `--no-threads` |  | mold | all | refused | as in a final link (C-R2b) | `bookkeeping.threads` | Use one thread |
| `--mortar-map` | FILE | mortar | all | supported | noop: written (§14.2 of plumb.md) (C-R2, I-R12) | `bookkeeping.mortar_map` | Write the machine-readable map (mortar-map 2) to FILE |
| `--time-phases` |  | mortar | all | supported | as in a final link | `bookkeeping.time_phases` | Report the time and memory of each phase |
| `--verify` |  | mortar | all | refused | as in a final link (C-R2b) | `bookkeeping.verify` | Check the output with mortar_plumb (reserved) |
| `--incremental` | optional, `=` only: full \| update | mortar | all | supported | acts: IN-12's warning, a plain -r (C-R4, IN-12) | `incremental.mode` | Link incrementally: update the output from its state if possible (alone), always from scratch (full), or update or fail (update) |
| `--no-incremental` |  | mortar | all | supported | noop (C-R2b) | `incremental.mode` | Link plainly, never reading or writing a state (default) |
| `--incremental-state` | PATH | mortar | all | supported | acts: IN-12's warning, no effect (C-R2b, IN-12) | `incremental.state` | Keep the incremental link state at PATH (default: OUTPUT.mortar-state) |
| `--incremental-report` | optional, `=` only: FILE | mortar | all | supported | acts: IN-12's warning, no effect (C-R2b, IN-12) | `incremental.report` | Report how each incremental link went, to FILE (default: standard error) |
| `--incremental-write` | copy \| in-place | mortar | all | refused | acts: IN-12's warning, no effect (C-R2b, IN-12) | `other` | How an update writes the output: copy (default) or in-place (reserved) |
| `--incremental-sync` | full \| next-link | mortar | all | refused | acts: IN-12's warning, no effect (C-R2b, IN-12) | `other` | Sync an update's writes: full (default) or next-link (reserved) |
| `--incremental-verify` |  | mortar | all | refused | acts: IN-12's warning, no effect (C-R2b, IN-12) | `other` | Hash every input of an update regardless of its stat (reserved) |
| `--incremental-changed` | PATH | mortar | all | refused | acts: IN-12's warning, no effect (C-R2b, IN-12) | `other` | Treat input PATH as changed (reserved) |
| `--stats` |  | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `other` | Print statistics |
| `--perf` |  | mold | all | refused | as in a final link (C-R2b) | `other` | Print performance statistics |
| `--time-trace` | optional, `=` only: FILE | lld | all | refused | as in a final link (C-R2b) | `other` | Write a time trace |
| `--time-trace-granularity` | MICROSECONDS (number) | lld | all | refused | as in a final link (C-R2b) | `other` | Granularity of the time trace |
| `--print-memory-usage` |  | GNU, lld | all | refused | as in a final link (C-R2b) | `other` | Report the use of memory regions |
| `--print-output-format` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Print the default output format |
| `--print-sysroot` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Print the system root |
| `--target-help` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Print the options of the targets |
| `--fork` |  | mold | all | refused | as in a final link (C-R2b) | `other` | Link in a child process |
| `--no-fork` |  | mold | all | refused | as in a final link (C-R2b) | `other` | Link in this process |
| `--quick-exit` |  | mold | all | refused | as in a final link (C-R2b) | `other` | Exit without cleanup |
| `--no-quick-exit` |  | mold | all | refused | as in a final link (C-R2b) | `other` | Exit with cleanup |
| `--detach` |  | mold | all | refused | as in a final link (C-R2b) | `other` | Write the separate debug file in the background |
| `--no-detach` |  | mold | all | refused | as in a final link (C-R2b) | `other` | Write the separate debug file before exiting |
| `--mmap-output-file` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Map the output file into memory (default) |
| `--no-mmap-output-file` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Write the output file without mapping it |
| `--rsp-quoting` | posix \| windows | lld | all | refused | as in a final link (C-R2b) | `other` | Quoting style of response files |
| `--run` | COMMAND (the rest) | mold | all | refused | as in a final link (C-R2b) | `other` | Run COMMAND with this linker as ld |

## Linker plugins and LTO

| Option | Value | Linkers | Targets | Status | At `-r` | Lands in | Help |
|---|---|---|---|---|---|---|---|
| `-plugin` | PLUGIN | GNU, lld, mold | all | noop: mortar loads no linker plugin; an LTO input is refused by name when it is read | noop: an LTO object still refused (C-R2, RV-R9) | `plugin.path` | Load a linker plugin |
| `-plugin-opt` | OPTION | GNU, lld, mold | all | noop: only the plugin reads it, and mortar loads none | noop (C-R2) | `plugin.options` | Pass OPTION to the plugin |
| `-plugin-save-temps` |  | GNU | all | noop: only the plugin reads it, and mortar loads none | noop: measured (C-R2b, rl0-opts) | `other` | Keep the plugin's temporary files |
| `-flto` |  | GNU | all | noop: GNU ld ignores it too; GCC passes it on some LTO links | noop: measured (C-R2b, rl0-opts) | `other` | Ignored, for GCC |
| `-flto-partition` | MODE | GNU | all | noop: GNU ld ignores it too; GCC passes it on some LTO links | noop: measured (C-R2b, rl0-opts) | `other` | Ignored, for GCC |
| `-fuse-ld` | LINKER | GNU | all | noop: GNU ld ignores it too; it chooses the linker, which is chosen | noop: measured (C-R2b, rl0-opts) | `other` | Ignored, for GCC |
| `--lto` | full \| thin | lld | all | refused | noop: by what the option is (C-R2b) | `other` | LTO back end |
| `--lto-O` | glued: LEVEL (number) | lld, mold | all | refused | noop: by what the option is (C-R2b) | `other` | LTO optimization level |
| `--lto-CGO` | glued: LEVEL (number) | lld | all | refused | noop: by what the option is (C-R2b) | `other` | LTO code generation level |
| `--lto-partitions` | N (number) | lld, mold | all | refused | noop: by what the option is (C-R2b) | `other` | Number of LTO partitions |
| `--lto-aa-pipeline` | PIPELINE | lld | all | refused | noop: by what the option is (C-R2b) | `other` | LTO alias analysis pipeline |
| `--lto-newpm-passes` | PASSES | lld | all | refused | noop: by what the option is (C-R2b) | `other` | LTO passes |
| `--lto-debug-pass-manager` |  | lld, mold | all | refused | noop: by what the option is (C-R2b) | `other` | Debug the LTO pass manager |
| `--lto-emit-asm` |  | lld, mold | all | refused | noop: by what the option is (C-R2b) | `other` | Write assembly instead of linking |
| `--lto-emit-llvm` |  | lld | all | refused | noop: by what the option is (C-R2b) | `other` | Write bitcode instead of linking |
| `--lto-sample-profile` | FILE | lld, mold | all | refused | noop: by what the option is (C-R2b) | `other` | LTO sample profile |
| `--lto-cs-profile-file` | FILE | lld, mold | all | refused | noop: by what the option is (C-R2b) | `other` | LTO context-sensitive profile |
| `--lto-cs-profile-generate` |  | lld, mold | all | refused | noop: by what the option is (C-R2b) | `other` | LTO context-sensitive instrumentation |
| `--lto-whole-program-visibility` |  | lld | all | refused | noop: by what the option is (C-R2b) | `other` | LTO: the link sees the whole program |
| `--no-lto-whole-program-visibility` |  | lld | all | refused | as in a final link (C-R2b) | `other` | LTO: the link does not see the whole program |
| `--lto-pgo-warn-mismatch` |  | lld | all | refused | noop: by what the option is (C-R2b) | `other` | Warn about profile mismatches (default) |
| `--no-lto-pgo-warn-mismatch` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Do not warn about profile mismatches |
| `--lto-basic-block-sections` | VALUE | lld | all | refused | noop: by what the option is (C-R2b) | `other` | LTO basic block sections |
| `--lto-basic-block-address-map` |  | lld | all | refused | noop: by what the option is (C-R2b) | `other` | LTO basic block address map |
| `--no-lto-basic-block-address-map` |  | lld | all | refused | as in a final link (C-R2b) | `other` | No LTO basic block address map (default) |
| `--lto-unique-basic-block-section-names` |  | lld | all | refused | noop: by what the option is (C-R2b) | `other` | Unique basic block section names |
| `--no-lto-unique-basic-block-section-names` |  | lld | all | refused | as in a final link (C-R2b) | `other` | No unique basic block section names (default) |
| `--lto-validate-all-vtables-have-type-infos` |  | lld | all | refused | noop: by what the option is (C-R2b) | `other` | Check that vtables have type information |
| `--no-lto-validate-all-vtables-have-type-infos` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Do not check vtables (default) |
| `--lto-known-safe-vtables` | GLOB | lld | all | refused | noop: by what the option is (C-R2b) | `other` | Vtables the check skips |
| `--load-pass-plugin` | FILE | lld | all | refused | as in a final link (C-R2b) | `other` | Load LLVM passes from FILE |
| `--mllvm` | OPTION | lld | all | refused | as in a final link (C-R2b) | `other` | Pass OPTION to LLVM |
| `--save-temps` | optional, `=` only: WHAT | lld, mold | all | refused | as in a final link (C-R2b) | `other` | Keep LTO intermediate files |
| `--opt-remarks-filename` | FILE | lld, mold | all | refused | as in a final link (C-R2b) | `other` | Optimization remarks file |
| `--opt-remarks-format` | FORMAT | lld, mold | all | refused | as in a final link (C-R2b) | `other` | Optimization remarks format |
| `--opt-remarks-passes` | REGEX | lld, mold | all | refused | as in a final link (C-R2b) | `other` | Passes whose remarks are kept |
| `--opt-remarks-hotness-threshold` | N | lld, mold | all | refused | as in a final link (C-R2b) | `other` | Remark hotness threshold |
| `--opt-remarks-with-hotness` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Remarks with hotness |
| `--thinlto-cache-dir` | DIR | lld, mold | all | refused | as in a final link (C-R2b) | `other` | ThinLTO cache directory |
| `--thinlto-cache-policy` | POLICY | lld, mold | all | refused | as in a final link (C-R2b) | `other` | ThinLTO cache policy |
| `--thinlto-jobs` | N | lld, mold | all | refused | as in a final link (C-R2b) | `other` | ThinLTO jobs |
| `--thinlto-single-module` | MODULE | lld | all | refused | as in a final link (C-R2b) | `other` | ThinLTO single module |
| `--thinlto-distributor` | FILE | lld | all | refused | as in a final link (C-R2b) | `other` | ThinLTO distributor |
| `--thinlto-distributor-arg` | ARG | lld | all | refused | as in a final link (C-R2b) | `other` | ThinLTO distributor argument |
| `--thinlto-remote-compiler` | FILE | lld | all | refused | as in a final link (C-R2b) | `other` | ThinLTO remote compiler |
| `--thinlto-remote-compiler-arg` | ARG | lld | all | refused | as in a final link (C-R2b) | `other` | ThinLTO remote compiler argument |
| `--thinlto-remote-compiler-prepend-arg` | ARG | lld | all | refused | as in a final link (C-R2b) | `other` | ThinLTO remote compiler prepended argument |

## Options of one target

| Option | Value | Linkers | Targets | Status | At `-r` | Lands in | Help |
|---|---|---|---|---|---|---|---|
| `--fix-cortex-a53-835769` |  | GNU, mold | AArch64 | refused | as in a final link (C-R2b) | `other` | Work around Cortex-A53 erratum 835769 |
| `--fix-cortex-a53-843419` | optional, `=` only: full \| adr \| adrp | GNU, lld, mold | AArch64 | partial: `=adr`, `=adrp` refused (a64.md §11.2: GNU ld's =adr and =adrp restrict the fix, which mortar does not do; use --fix-cortex-a53-843419 or =full) | noop (C-R2) | `other` | Work around Cortex-A53 erratum 843419 |
| `--no-apply-dynamic-relocs` |  | GNU, lld, mold | AArch64 | refused | as in a final link (C-R2b) | `other` | Write zero where dynamic relocations apply |
| `--pic-veneer` |  | GNU, lld | AArch64 | refused | as in a final link (C-R2b) | `other` | Always use position-independent veneers |
| `--stub-group-size` | N | GNU | AArch64 | refused | as in a final link (C-R2b) | `other` | Size of a group of sections one stub section serves |
| `--no-enum-size-warning` |  | GNU | AArch64 | refused | as in a final link (C-R2b) | `other` | Do not warn about objects with other enum sizes |
| `--no-wchar-size-warning` |  | GNU | AArch64 | refused | as in a final link (C-R2b) | `other` | Do not warn about objects with another wchar_t size |
| `--no-pipeline-knowledge` |  | GNU | AArch64 | refused | as in a final link (C-R2b) | `other` | (hidden in GNU ld) |
| `--ld-generated-unwind-info` |  | GNU | x86-64, i386 | refused | as in a final link (C-R2b) | `other` | Write unwind information for the PLT |
| `--no-ld-generated-unwind-info` |  | GNU | x86-64, i386 | refused | as in a final link (C-R2b) | `other` | Write no unwind information for the PLT |
| `-z force-bti` |  | GNU, lld | AArch64 | refused | refused: `-z force-bti with -r` (C-R2a) | `other` | Require BTI and write BTI PLTs |
| `-z bti-report` | optional, `=` only: none \| warning \| error | GNU, lld | AArch64 | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Report inputs without BTI markings |
| `-z pac-plt` |  | GNU, lld | AArch64 | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Protect PLTs with pointer authentication |
| `-z gcs` | always \| never \| implicit | GNU, lld | AArch64 | refused | refused: `-z gcs=always with -r`; `=never` is a no-op (C-R2a's first row) (C-R2a) | `other` | Mark the output for the Guarded Control Stack |
| `-z gcs-report` | optional, `=` only: none \| warning \| error | GNU, lld | AArch64 | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Report inputs without GCS markings |
| `-z gcs-report-dynamic` | none \| warning \| error | GNU, lld | AArch64 | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Report shared objects without GCS markings |
| `-z noextern-protected-data` |  | GNU | x86-64, i386 | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Do not treat protected data as external |
| `-z indirect-extern-access` |  | GNU | x86-64, i386 | refused | refused: `-z indirect-extern-access with -r` (C-R2a) | `other` | Access external data indirectly |
| `-z noindirect-extern-access` |  | GNU | x86-64, i386 | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Access external data directly (default) |
| `-z dynamic-undefined-weak` |  | GNU, lld, mold | x86-64, i386 | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Make undefined weak symbols dynamic |
| `-z nodynamic-undefined-weak` |  | GNU, lld, mold | x86-64, i386 | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Do not make undefined weak symbols dynamic |
| `-z call-nop` | PADDING | GNU | x86-64, i386 | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | The 1-byte NOP used to pad converted branches |
| `-z ibtplt` |  | GNU, mold | x86-64, i386 | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Write IBT PLT entries |
| `-z ibt` |  | GNU, mold | x86-64, i386 | refused | refused: `-z ibt with -r` (C-R2a) | `other` | Mark the output for IBT |
| `-z shstk` |  | GNU, lld, mold | x86-64, i386 | refused | refused: `-z shstk with -r` (C-R2a) | `other` | Mark the output for the shadow stack |
| `-z cet-report` | none \| warning \| error | GNU, lld, mold | x86-64, i386 | refused | as in a final link: the same check as in a final link; `=none` and `=warning` are no-ops (C-R2a) | `other` | Report inputs without IBT and SHSTK markings |
| `-z report-relative-reloc` |  | GNU | x86-64, i386 | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Report relative relocations |
| `-z x86-64-baseline` |  | GNU | x86-64, i386 | refused | as in a final link (C-R2b) | `other` | Mark the x86-64 baseline ISA level as needed |
| `-z x86-64-v2` |  | GNU, mold | x86-64, i386 | refused | refused: `-z x86-64-v2 with -r` (C-R2a) | `other` | Mark the x86-64-v2 ISA level as needed |
| `-z x86-64-v3` |  | GNU, mold | x86-64, i386 | refused | refused: `-z x86-64-v3 with -r` (C-R2a) | `other` | Mark the x86-64-v3 ISA level as needed |
| `-z x86-64-v4` |  | GNU, mold | x86-64, i386 | refused | refused: `-z x86-64-v4 with -r` (C-R2a) | `other` | Mark the x86-64-v4 ISA level as needed |
| `-z isa-level-report` | none \| all \| needed \| used | GNU | x86-64, i386 | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Report the x86-64 ISA levels |
| `-z noreloc-overflow` |  | GNU | x86-64 | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Do not check relocation overflow |
| `-z lam-u48` |  | GNU | x86-64 | refused | refused: `-z lam-u48 with -r` (C-R2a) | `other` | Mark the output for LAM_U48 |
| `-z lam-u48-report` | none \| warning \| error | GNU | x86-64 | refused | as in a final link: the same check as in a final link; `=none` and `=warning` are no-ops (C-R2a) | `other` | Report inputs without LAM_U48 markings |
| `-z lam-u57` |  | GNU | x86-64 | refused | refused: `-z lam-u57 with -r` (C-R2a) | `other` | Mark the output for LAM_U57 |
| `-z lam-u57-report` | none \| warning \| error | GNU | x86-64 | refused | as in a final link: the same check as in a final link; `=none` and `=warning` are no-ops (C-R2a) | `other` | Report inputs without LAM_U57 markings |
| `-z lam-report` | none \| warning \| error | GNU | x86-64 | refused | as in a final link: the same check as in a final link; `=none` and `=warning` are no-ops (C-R2a) | `other` | Report inputs without LAM markings |
| `-z mark-plt` |  | GNU | x86-64 | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Mark the PLT with dynamic tags |
| `-z nomark-plt` |  | GNU | x86-64 | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Do not mark the PLT (default) |

## -z keywords

| Option | Value | Linkers | Targets | Status | At `-r` | Lands in | Help |
|---|---|---|---|---|---|---|---|
| `-z now` |  | GNU, lld, mold | all | supported | noop: C-R2a's first row (C-R2a) | `z.now` | Resolve every symbol at load time |
| `-z lazy` |  | GNU, lld, mold | all | supported | noop: C-R2a's first row (C-R2a) | `z.now` | Resolve functions when first called (default) |
| `-z relro` |  | GNU, lld, mold | all | supported | noop: C-R2a's first row (C-R2a) | `z.relro` | Write PT_GNU_RELRO (default) |
| `-z norelro` |  | GNU, lld, mold | all | supported | noop: C-R2a's first row (C-R2a) | `z.relro` | Write no PT_GNU_RELRO |
| `-z execstack` |  | GNU, lld, mold | all | supported | acts: L-R6a (C-R3, L-R6a) | `z.execstack` | Make the stack executable |
| `-z noexecstack` |  | GNU, lld, mold | all | supported | acts: L-R6a (C-R3, L-R6a) | `z.execstack` | Make the stack not executable |
| `-z text` |  | GNU, lld, mold | all | supported | noop: C-R2a's first row (C-R2a) | `z.text` | Report text relocations as errors |
| `-z notext`, `-z textoff` |  | GNU, lld, mold | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `z.text` | Accept text relocations |
| `-z separate-code` |  | GNU, lld, mold | all | supported | noop: C-R2a's first row (C-R2a) | `z.separate_code` | Put code in segments of its own pages |
| `-z noseparate-code` |  | GNU, lld, mold | all | supported | noop: C-R2a's first row (C-R2a) | `z.separate_code` | Let code share pages with other data |
| `-z defs` |  | GNU, lld, mold | all | supported | noop: C-R2a's first row (C-R2a) | `z.defs` | Report undefined symbols of object files |
| `-z undefs` |  | GNU, lld, mold | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `z.defs` | Accept undefined symbols in a shared object |
| `-z max-page-size` | SIZE | GNU, lld, mold | all | supported | noop: C-R2a's first row (C-R2a) | `z.max_page_size` | The largest page size the output supports |
| `-z common-page-size` | SIZE | GNU, lld, mold | all | noop: layout.md L-75: the value is checked, and no layout rule reads it | noop: C-R2a's first row (C-R2a) | `z.common_page_size` | The common page size |
| `-z stack-size` | SIZE | GNU, lld, mold | all | refused | refused: `-z stack-size=SIZE with -r`, the value as written (C-R2a) | `z.stack_size` | The size of the stack segment |
| `-z pack-relative-relocs` |  | GNU, lld, mold | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `z.pack_relative_relocs` | Pack relative relocations in DT_RELR |
| `-z nopack-relative-relocs` |  | GNU, lld, mold | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `z.pack_relative_relocs` | Do not pack relative relocations (default) |
| `-z copyreloc` |  | lld | all | refused | as in a final link (C-R2b) | `z.copyreloc` | Make copy relocations (default) |
| `-z nocopyreloc` |  | GNU, lld, mold | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `z.copyreloc` | Make no copy relocations |
| `-z combreloc` |  | GNU, lld, mold | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `z.combreloc` | Sort dynamic relocations into one section (default) |
| `-z nocombreloc` |  | GNU, lld, mold | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `z.combreloc` | Do not sort dynamic relocations |
| `-z sectionheader` |  | GNU, lld, mold | all | noop: layout.md L-63: the section header table is always written (the default) | noop: C-R2a's first row (C-R2a) | `z.sectionheader` | Write the section header table (default) |
| `-z nosectionheader` |  | GNU, lld, mold | all | refused | conflicting-options with -r (C-R2a) | `z.sectionheader` | Write no section header table |
| `-z muldefs` |  | GNU, lld, mold | all | supported | as in a final link (RV-R4, C-R2a) | `diagnostics.allow_multiple_definition` | Take the first of several definitions of a symbol |
| `-z unique-symbol` |  | GNU | all | refused | refused: `-z unique-symbol with -r` (C-R2a) | `other` | Give local symbols unique names |
| `-z nounique-symbol` |  | GNU | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Keep duplicate local symbol names (default) |
| `-z globalaudit` |  | GNU | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Mark the executable for global auditing |
| `-z start-stop-gc` |  | GNU, lld | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Let __start_/__stop_ references not keep sections |
| `-z nostart-stop-gc` |  | GNU, lld | all | noop: layout.md L-96: the default | noop: C-R2a's first row (C-R2a) | `other` | Let __start_/__stop_ references keep sections (default) |
| `-z start-stop-visibility` | default \| internal \| hidden \| protected | GNU, lld | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Visibility of __start_ and __stop_ symbols |
| `-z global` |  | GNU, lld | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Make the symbols of this object available to later loads |
| `-z initfirst` |  | GNU, lld, mold | all | supported | noop: C-R2a's first row (C-R2a) | `other` | Initialize this object first |
| `-z interpose` |  | GNU, lld, mold | all | supported | noop: C-R2a's first row (C-R2a) | `other` | Let this object interpose all but the executable |
| `-z unique` |  | GNU | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Load this object at most once |
| `-z nounique` |  | GNU | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Do not mark the object as unique |
| `-z loadfltr` |  | GNU | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Process the filtees at load time |
| `-z nodefaultlib` |  | GNU, lld, mold | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Do not search the default directories for dependencies |
| `-z nodelete` |  | GNU, lld, mold | all | supported | noop: C-R2a's first row (C-R2a) | `other` | Never unload this object |
| `-z nodlopen` |  | GNU, lld, mold | all | supported | noop: C-R2a's first row (C-R2a) | `other` | Do not allow dlopen of this object |
| `-z nodump` |  | GNU, mold | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Do not allow dldump of this object |
| `-z origin` |  | GNU, lld, mold | all | supported | noop: C-R2a's first row (C-R2a) | `other` | The object uses $ORIGIN |
| `-z common` |  | GNU | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Give common symbols the type STT_COMMON |
| `-z nocommon` |  | GNU | all | refused | noop: C-R2a's first row; the class at -r wins (C-R2a, C-R2b) | `other` | Give common symbols the type STT_OBJECT |
| `-z memory-seal` |  | GNU | all | supported | refused: `-z memory-seal with -r` (C-R2a) | `z.memory_seal` | Mark the object for memory sealing |
| `-z nomemory-seal` |  | GNU | all | supported | noop: C-R2a's first row (C-R2a) | `z.memory_seal` | Do not mark the object for memory sealing (default) |
| `-z dead-reloc-in-nonalloc` | GLOB=VALUE | lld | all | refused | as in a final link (C-R2b) | `other` | The value of relocations to discarded sections in non-alloc sections |
| `-z execute-only-report` | none \| warning \| error | lld | all | refused | noop: by what the option is (C-R2b) | `other` | Report inputs that are not execute-only |
| `-z force-ibt` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Write IBT PLT entries and mark the output for IBT |
| `-z hazardplt` |  | lld | all | refused | as in a final link (C-R2b) | `other` | MIPS hazard-barrier PLT entries |
| `-z ifunc-noplt` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Resolve ifuncs without PLT entries |
| `-z keep-text-section-prefix` |  | lld, mold | all | refused | as in a final link (C-R2b) | `other` | Keep .text.hot and similar output sections apart |
| `-z nokeep-text-section-prefix` |  | lld, mold | all | refused | as in a final link (C-R2b) | `other` | Merge .text.hot and similar into .text (default) |
| `-z nognustack` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Write no PT_GNU_STACK |
| `-z rel` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Write REL dynamic relocations |
| `-z rela` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Write RELA dynamic relocations |
| `-z retpolineplt` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Write retpoline PLT entries |
| `-z rodynamic` |  | lld, mold | all | refused | as in a final link (C-R2b) | `other` | Make .dynamic read-only |
| `-z separate-loadable-segments` |  | lld, mold | all | refused | as in a final link (C-R2b) | `other` | Put every loadable segment on pages of its own |
| `-z wxneeded` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Mark the object as needing writable and executable memory |
| `-z lrodata-after-bss` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Place .lrodata after .bss |
| `-z nolrodata-after-bss` |  | lld | all | refused | as in a final link (C-R2b) | `other` | Place .lrodata before .bss |
| `-z zicfilp` | MODE | lld | all | refused | as in a final link (C-R2b) | `other` | RISC-V landing pads |
| `-z zicfiss` | MODE | lld | all | refused | as in a final link (C-R2b) | `other` | RISC-V shadow stack |
| `-z zicfilp-unlabeled-report` | LEVEL | lld | all | refused | as in a final link (C-R2b) | `other` | RISC-V landing pad report |
| `-z zicfilp-func-sig-report` | LEVEL | lld | all | refused | as in a final link (C-R2b) | `other` | RISC-V landing pad report |
| `-z zicfiss-report` | LEVEL | lld | all | refused | as in a final link (C-R2b) | `other` | RISC-V shadow stack report |
| `-z execstack-if-needed` |  | mold | all | refused | as in a final link (C-R2b) | `other` | Make the stack executable if an input asks |
| `-z rewrite-endbr` |  | mold | all | refused | as in a final link (C-R2b) | `other` | Rewrite unneeded endbr instructions as NOPs |
| `-z norewrite-endbr` |  | mold | all | refused | as in a final link (C-R2b) | `other` | Keep endbr instructions (default) |

## Kept for other systems and tools

| Option | Value | Linkers | Targets | Status | At `-r` | Lands in | Help |
|---|---|---|---|---|---|---|---|
| `-a` | KEYWORD | GNU | all | refused | as in a final link (C-R2b) | `other` | HP-UX shared library control |
| `-A`, `--architecture` | ARCH | GNU | all | refused | as in a final link (C-R2b) | `other` | Set the architecture |
| `-EB` |  | GNU, lld, mold | all | refused | as in a final link (C-R2b) | `other` | Link big-endian objects |
| `-EL` |  | GNU, lld, mold | all | noop: every target mortar links for is little-endian | noop (C-R2) | `other` | Link little-endian objects |
| `-g` |  | GNU, lld | all | noop: GNU ld ignores it too | noop: measured (C-R2b, rl0-opts) | `other` | Ignored |
| `-O` | LEVEL | GNU, lld, mold | all | noop: synth.md S-32: mortar merges at every level, as GNU ld and mold do | noop (C-R2) | `other` | Optimize the output |
| `-Qy` |  | GNU | all | noop: GNU ld ignores it too (SVR4) | noop: measured (C-R2b, rl0-opts) | `other` | Ignored |
| `-qmagic` |  | GNU | all | noop: GNU ld ignores it too (Linux a.out) | noop: measured (C-R2b, rl0-opts) | `other` | Ignored |
| `-assert` | KEYWORD | GNU | all | noop: GNU ld ignores it too (SunOS) | noop: measured (C-R2b, rl0-opts) | `other` | Ignored |
| `--map-whole-files` |  | GNU | all | noop: GNU ld ignores it too (gold) | noop: measured (C-R2b, rl0-opts) | `other` | Ignored |
| `--no-map-whole-files` |  | GNU | all | noop: GNU ld ignores it too (gold) | noop: measured (C-R2b, rl0-opts) | `other` | Ignored |
| `--no-keep-memory` |  | GNU, lld, mold | all | noop: a memory-use hint for BFD; it does not change the output | noop: measured (C-R2b, rl0-opts) | `other` | Use less memory, more I/O |
| `--reduce-memory-overheads` |  | GNU | all | noop: a memory-use hint for BFD; it does not change the output | noop: GNU ld reorders `.symtab`; mortar the same bytes (rl0-opts) | `other` | Use less memory, more time |
| `--hash-size` | NUMBER (number) | GNU | all | noop: a table-size hint for BFD; it does not change the output | noop: as `--reduce-memory-overheads` (rl0-opts) | `other` | Hash table size hint |
| `--max-cache-size` | SIZE | GNU, mold | all | noop: a memory-use hint for BFD; it does not change the output | noop: measured (C-R2b, rl0-opts) | `other` | Cache size limit |
| `--dll-verbose` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | (hidden in GNU ld) |
| `--ctf-variables` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Write CTF for static variables |
| `--no-ctf-variables` |  | GNU | all | refused | as in a final link (C-R2b) | `other` | Write no CTF for static variables |
| `--ctf-share-types` | share-unconflicted \| share-duplicated | GNU | all | refused | noop: by what the option is (C-R2b) | `other` | How CTF types are shared |
| `--be8` |  | lld | all | refused | as in a final link (C-R2b) | `other` | (lld, Arm) BE8 output |
| `--fix-cortex-a8` |  | lld | all | refused | as in a final link (C-R2b) | `other` | (lld, Arm) Cortex-A8 erratum |
| `--merge-exidx-entries` |  | lld | all | refused | as in a final link (C-R2b) | `other` | (lld, Arm) merge .ARM.exidx |
| `--no-merge-exidx-entries` |  | lld | all | refused | as in a final link (C-R2b) | `other` | (lld, Arm) do not merge .ARM.exidx |
| `--target1-abs` |  | lld | all | refused | as in a final link (C-R2b) | `other` | (lld, Arm) R_ARM_TARGET1 is absolute |
| `--target1-rel` |  | lld | all | refused | as in a final link (C-R2b) | `other` | (lld, Arm) R_ARM_TARGET1 is relative |
| `--target2` | TYPE | lld | all | refused | as in a final link (C-R2b) | `other` | (lld, Arm) R_ARM_TARGET2 |
| `--cmse-implib` |  | lld | all | refused | as in a final link (C-R2b) | `other` | (lld, Arm) CMSE import library |
| `--in-implib` | FILE | lld | all | refused | as in a final link (C-R2b) | `other` | (lld, Arm) CMSE input import library |
| `--relax-gp` |  | lld | all | refused | as in a final link (C-R2b) | `other` | (lld, RISC-V) global pointer relaxation |
| `--no-relax-gp` |  | lld | all | refused | as in a final link (C-R2b) | `other` | (lld, RISC-V) no global pointer relaxation |
| `--pcrel-optimize` |  | lld | all | refused | as in a final link (C-R2b) | `other` | (lld, PowerPC64) |
| `--no-pcrel-optimize` |  | lld | all | refused | as in a final link (C-R2b) | `other` | (lld, PowerPC64) |
| `--toc-optimize` |  | lld | all | refused | as in a final link (C-R2b) | `other` | (lld, PowerPC64) |
| `--no-toc-optimize` |  | lld | all | refused | as in a final link (C-R2b) | `other` | (lld, PowerPC64) |
| `--power10-stubs` | optional, `=` only: MODE | lld | all | refused | as in a final link (C-R2b) | `other` | (lld, PowerPC64) |
| `--no-power10-stubs` |  | lld | all | refused | as in a final link (C-R2b) | `other` | (lld, PowerPC64) |
| `--split-stack-adjust-size` | SIZE | lld | all | refused | as in a final link (C-R2b) | `other` | (lld) split-stack adjustment |

