# Options at `-r`

Hand-written companion of `docs/status/options.md` **until RL-1 (WP-181)
generates the "at `-r`" column** from the `at_r` status of each row of
`crates/mortar_cli/src/rows.rs` (`spec/cli.md` C-R2: "`docs/status/
options.md` gains a column \"at `-r`\""). `options.md` is generated and
checked by `crates/mortar_cli/tests/table.rs`, so WP-180 (RL-0) does not
edit it; RL-1 makes this file's column the rows' `at_r` and deletes this
file. Written from `spec/cli.md` §3.16 (C-R1-C-R6), with the rows those
rules do not name measured at `-r` (`tests/relocatable-design/exp/rl0/
opts.sh`, `work/rl0-opts.log`: each option's `-r` output compared byte
for byte with the plain `-r`, by GNU ld 2.44 and by lld 22).

Classes: **acts** (a rule says what it does at `-r`); **noop** (accepted,
no byte changes); **as in a final link** (a diagnostic, informational or
input-selecting switch that does at `-r` what it does elsewhere);
**conflicting-options with -r**; **refused** (by the name in
`spec/diag.md` §4.4). The classes of the rows C-R2, C-R2a, C-R3 and C-R4
do not name are `spec/cli.md` C-R2b's (the supervisor's rulings on
WP-180's Q-O1 to Q-O7): "noop: measured (C-R2b)" is a row GNU ld or
lld measured byte-identical at `-r`.

## The rows mortar accepts in a final link

| Option | Final link | At `-r` | Rule |
|---|---|---|---|
| `-o`, `--output` | supported | acts | C-R3, I-R1 |
| `-shared`, `--Bshareable` | partial | conflicting-options with -r | C-R1 |
| `-pie`, `--pic-executable` | supported | conflicting-options with -r | C-R1 |
| `-no-pie`, `--no-pic-executable` | supported | noop (C-R2) | C-R2 |
| `-m` | supported | noop (C-R2); a wrong emulation still refused | C-R2 |
| `-e`, `--entry` | supported | acts: accepted, e_entry 0, not a root | C-R3, L-R7, RV-R2 |
| `-s`, `--strip-all` | supported | acts; refused until RL-11 (`-s with -r`) | C-R3, I-R9 |
| `-S`, `--strip-debug` | supported | acts; refused until RL-11 | C-R3, I-R9 |
| `--strip-discarded` | noop | noop: measured (C-R2b) | rl0-opts |
| `-x`, `--discard-all` | supported | acts; refused until RL-11 | C-R3, I-R9 |
| `-X`, `--discard-locals` | supported | noop (C-R2) | C-R2 |
| `--discard-none` | supported | noop: measured (C-R2b) | rl0-opts |
| `--build-id` | partial | acts: the note over the -r output | C-R3, I-R8 |
| `--no-build-id` | supported | noop (C-R2) | C-R2 |
| `--compress-debug-sections` | partial | refused (`--compress-debug-sections with -r`); `=none` no-op | C-R4 |
| `--eh-frame-hdr` | supported | noop (C-R2) | C-R2 |
| `--no-eh-frame-hdr` | supported | noop: measured (C-R2b) | rl0-opts |
| `--gc-sections` | supported | refused (`--gc-sections with -r`) | C-R4, I-R10 |
| `--no-gc-sections` | supported | noop: measured (C-R2b) | rl0-opts |
| `--print-gc-sections` | supported | noop: measured (C-R2b) | rl0-opts |
| `--no-print-gc-sections` | supported | noop: measured (C-R2b) | rl0-opts |
| `--gc-keep-exported` | supported | noop: measured (C-R2b) | rl0-opts |
| `--gdb-index` | supported | refused (`--gdb-index with -r`) | C-R4 |
| `--no-gdb-index` | noop | noop: measured (C-R2b) | rl0-opts (lld) |
| `--no-debug-names` | noop | noop: measured (C-R2b) | rl0-opts (lld) |
| `-l`, `--library` | supported | noop (C-R2): picks the inputs, extraction by RV-R2 | C-R2, RV-R2 |
| `-(`, `--start-group` | supported | noop (C-R2): acts on extraction | C-R2, RV-R2 |
| `-)`, `--end-group` | supported | noop (C-R2) | C-R2 |
| `--start-lib` | supported | as in a final link: lazy objects (lld's reading) | C-R2b, RV-R2 |
| `--end-lib` | supported | with `--start-lib` | RV-R2 |
| `--as-needed` | supported | noop (C-R2) | C-R2 |
| `--no-as-needed` | supported | noop (C-R2) | C-R2 |
| `--whole-archive` | supported | noop (C-R2): acts on extraction | C-R2, RV-R2 |
| `--no-whole-archive` | supported | noop (C-R2) | C-R2 |
| `-Bstatic`, `--dn`, `--non_shared`, `--static` | supported | noop (C-R2); a `.so` is refused | C-R2, C-R5 |
| `-Bdynamic`, `--dy`, `--call_shared` | supported | noop (C-R2); a `.so` is refused | C-R2, C-R5 |
| `--no-copy-dt-needed-entries`, `--no-add-needed` | noop | noop: measured (C-R2b) | rl0-opts |
| `--push-state` | supported | noop (C-R2) | C-R2 |
| `--pop-state` | supported | noop (C-R2) | C-R2 |
| `--no-dependent-libraries` | noop | noop: measured (C-R2b) | rl0-opts (lld) |
| `--warn-backrefs` | supported | as in a final link (a diagnostic) | rl0-opts |
| `--no-warn-backrefs` | supported | as in a final link | rl0-opts |
| `-L`, `--library-path` | supported | noop (C-R2) | C-R2 |
| `--sysroot` | supported | as in a final link (the search) | rl0-opts |
| `-rpath-link` | supported | noop: measured (C-R2b) | rl0-opts |
| `--version-script` | supported | noop (C-R2) | C-R2 |
| `--dynamic-list` | supported | conflicting-options with -r | C-R4 |
| `--sort-common`, `--sort_common` | supported | noop (C-R2) | C-R2 |
| `--sort-section` | supported | noop (C-R2) | C-R2 |
| `-u`, `--undefined` | supported | acts: a root, and `UND` written | C-R3, RV-R2, RV-R3 |
| `--require-defined` | supported | acts: a root for extraction like `-u`, nothing written; undefined at the end reported as in a final link (RL-2 measures) | C-R2b, RV-R2 |
| `--defsym` | partial | refused until RL-11 (`--defsym with -r`) | C-R3, RV-R10 |
| `--wrap` | partial | refused (`--wrap with -r`) | C-R4 |
| `--export-dynamic-symbol` | supported | conflicting-options with -r (C-R4's `--export-dynamic*`) | C-R4 |
| `--exclude-libs` | partial | noop (C-R2) | C-R2 |
| `-init` | supported | conflicting-options with -r | C-R2b |
| `-fini` | supported | conflicting-options with -r | C-R2b |
| `--undefined-version` | supported | noop: measured (C-R2b) | rl0-opts |
| `--no-undefined-version` | supported | noop: measured (C-R2b) | rl0-opts |
| `-E`, `--export-dynamic` | supported | conflicting-options with -r | C-R4 |
| `--no-export-dynamic` | supported | noop (C-R2b, by what the option is) | C-R2b |
| `-Bsymbolic` | supported | conflicting-options with -r | C-R4 |
| `-Bsymbolic-functions` | supported | conflicting-options with -r | C-R4 |
| `-Bno-symbolic` | supported | noop (C-R2b, by what the option is) | C-R2b |
| `-h`, `-soname` | supported | conflicting-options with -r | C-R4 |
| `-rpath` | supported | conflicting-options with -r | C-R4 |
| `-I`, `--dynamic-linker` | supported | noop (C-R2) | C-R2 |
| `--no-dynamic-linker` | supported | noop (C-R2) | C-R2 |
| `--enable-new-dtags` | supported | conflicting-options with -r | C-R4 |
| `--disable-new-dtags` | supported | noop (C-R2b, by what the option is) | C-R2b |
| `--hash-style` | partial | noop (C-R2) | C-R2 |
| `--rosegment` | noop | noop: measured (C-R2b) | rl0-opts |
| `--error-limit` | supported | as in a final link | rl0-opts (lld) |
| `--fatal-warnings` | supported | as in a final link | rl0-opts |
| `--no-fatal-warnings` | supported | as in a final link | rl0-opts |
| `-w`, `--no-warnings` | supported | as in a final link; never hides a refusal | rl0-opts, diag §4.3 |
| `--color-diagnostics` | supported | as in a final link | rl0-opts (lld) |
| `--no-color-diagnostics` | supported | as in a final link | rl0-opts (lld) |
| `--no-undefined` | supported | noop (C-R2): nothing is undefined at -r | C-R2 |
| `--allow-multiple-definition` | supported | as in a final link | RV-R4 |
| `--no-allow-multiple-definition` | supported | as in a final link | RV-R4 |
| `--warn-common` | supported | noop (C-R2) | C-R2 |
| `--no-warn-common` | supported | as in a final link | rl0-opts (lld) |
| `--warn-execstack` | noop | as in a final link | rl0-opts |
| `--odr` | supported | as in a final link over the -r's units | MT-30a |
| `--no-odr-check` | supported | as in a final link | MT-30a |
| `--odr-stats` | supported | as in a final link | MT-30a |
| `--odr-check-archives` | supported | as in a final link | MT-30a |
| `--odr-table` | supported | noop (C-R2b, by what the option is): the final link writes the table | C-R2b |
| `--keep-mason-meta` | supported | noop (C-R2b, by what the option is): MT-30 always writes the container | C-R2b, MT-30 |
| `--detect-odr-violations` | noop | noop (its final-link class; not measured: no reference knows it; C-R2b does not name it) | — |
| `--version` | supported | as in a final link | — |
| `-v` | supported | as in a final link | — |
| `-V` | supported | as in a final link | — |
| `--help` | supported | as in a final link | — |
| `-Map` | supported | noop (C-R2): written as in any link | C-R2, I-R12 |
| `-M`, `--print-map` | supported | as in a final link | rl0-opts |
| `--verbose` | supported | as in a final link | rl0-opts |
| `--dependency-file` | supported | as in a final link | rl0-opts |
| `--threads` | supported | noop (C-R2) | C-R2 |
| `--mortar-map` | supported | noop (C-R2): written (§14.2 of plumb.md) | C-R2, I-R12 |
| `--time-phases` | supported | as in a final link | — |
| `--incremental` | supported | IN-12's warning, a plain -r | C-R4, IN-12 |
| `--no-incremental` | supported | noop | C-R2b |
| `--incremental-state` | supported | IN-12's warning, no effect | C-R2b, IN-12 |
| `--incremental-report` | supported | IN-12's warning, no effect | C-R2b, IN-12 |
| `-plugin` | noop | noop (C-R2); an LTO object still refused | C-R2, RV-R9 |
| `-plugin-opt` | noop | noop (C-R2) | C-R2 |
| `-plugin-save-temps` | noop | noop: measured (C-R2b) | rl0-opts |
| `-flto` | noop | noop: measured (C-R2b) | rl0-opts |
| `-flto-partition` | noop | noop: measured (C-R2b) | rl0-opts |
| `-fuse-ld` | noop | noop: measured (C-R2b) | rl0-opts |
| `--fix-cortex-a53-843419` | partial | noop (C-R2) | C-R2 |
| `-z now` | supported | noop (C-R2) (C-R2a's first row) | C-R2a |
| `-z lazy` | supported | noop (C-R2) (C-R2a's first row) | C-R2a |
| `-z relro` | supported | noop (C-R2) (C-R2a's first row) | C-R2a |
| `-z norelro` | supported | noop (C-R2) (C-R2a's first row) | C-R2a |
| `-z execstack` | supported | acts: L-R6a | C-R3, L-R6a |
| `-z noexecstack` | supported | acts: L-R6a | C-R3, L-R6a |
| `-z text` | supported | noop (C-R2) (C-R2a's first row) | C-R2a |
| `-z separate-code` | supported | noop (C-R2) (C-R2a's first row) | C-R2a |
| `-z noseparate-code` | supported | noop (C-R2) (C-R2a's first row) | C-R2a |
| `-z defs` | supported | noop (C-R2) (C-R2a's first row) | C-R2a |
| `-z max-page-size` | supported | noop (C-R2) (C-R2a's first row) | C-R2a |
| `-z common-page-size` | noop | noop (C-R2) (C-R2a's first row) | C-R2a |
| `-z sectionheader` | noop | noop (C-R2) (C-R2a's first row) | C-R2a |
| `-z muldefs` | supported | as in a final link | RV-R4, C-R2a |
| `-z nostart-stop-gc` | noop | noop (C-R2) (C-R2a's first row) | C-R2a |
| `-z initfirst` | supported | noop (C-R2) (C-R2a's first row) | C-R2a |
| `-z interpose` | supported | noop (C-R2) (C-R2a's first row) | C-R2a |
| `-z nodelete` | supported | noop (C-R2) (C-R2a's first row) | C-R2a |
| `-z nodlopen` | supported | noop (C-R2) (C-R2a's first row) | C-R2a |
| `-z origin` | supported | noop (C-R2) (C-R2a's first row) | C-R2a |
| `-z memory-seal` | supported | refused (`-z memory-seal with -r`) | C-R2a |
| `-z nomemory-seal` | supported | noop (C-R2) (C-R2a's first row) | C-R2a |
| `-EL` | noop | noop (C-R2) | C-R2 |
| `-g` | noop | noop: measured (C-R2b) | rl0-opts |
| `-O` | noop | noop (C-R2) | C-R2 |
| `-Qy` | noop | noop: measured (C-R2b) | rl0-opts |
| `-qmagic` | noop | noop: measured (C-R2b) | rl0-opts |
| `-assert` | noop | noop: measured (C-R2b) | rl0-opts |
| `--map-whole-files` | noop | noop: measured (C-R2b) | rl0-opts |
| `--no-map-whole-files` | noop | noop: measured (C-R2b) | rl0-opts |
| `--no-keep-memory` | noop | noop: measured (C-R2b) | rl0-opts |
| `--reduce-memory-overheads` | noop | noop (GNU ld reorders `.symtab`; mortar the same bytes) | rl0-opts |
| `--hash-size` | noop | noop (as `--reduce-memory-overheads`) | rl0-opts |
| `--max-cache-size` | noop | noop: measured (C-R2b) | rl0-opts |

## Rows refused in a final link that `-r` names

| Option | Final link | At `-r` | Rule |
|---|---|---|---|
| `-r`, `-i`, `--relocatable` | refused | acts: the kind; refused (`relocatable output (-r)`) until RL-7 | C-R1 |
| `-Ur` | refused | acts as `-r` (`plan.ur`) | C-R1 |
| `-d`, `--dc`, `--dp` | refused | acts: commons allocated; refused until RL-11 | C-R3, RV-R5 |
| `-q`, `--emit-relocs` | refused | noop (the class at -r wins) | C-R2, C-R2b |
| `--no-relax` | refused | noop (the class at -r wins) | C-R2, C-R2b |
| `--relax` | refused | refused (`--relax with -r`) | C-R4 |
| `-T`, `--script` | refused | refused until RL-11 (`-T with -r`) | C-R3 |
| `-b`, `--format` | refused | acts: `binary` (L-R9); refused until RL-10 | C-R3, L-R9 |
| `--force-group-allocation` | refused | acts; refused until RL-8 | C-R3, L-R3 |
| `--unique` | refused | acts; refused until RL-11 (`--unique with -r`) | C-R3, C-R4, L-R11 |
| `-z nosectionheader` | refused | conflicting-options with -r | C-R2a |
| `-z` `combreloc`, `nocombreloc`, `common`, `nocommon`, `undefs`, `global`, `globalaudit`, `loadfltr`, `nocopyreloc`, `nodefaultlib`, `nodump`, `notext`, `start-stop-gc`, `unique`, `nounique`, `nounique-symbol`, `pac-plt`, `call-nop=`, `ibtplt`, `dynamic-undefined-weak`, `nodynamic-undefined-weak`, `noextern-protected-data`, `noindirect-extern-access`, `pack-relative-relocs`, `nopack-relative-relocs`, `report-relative-reloc`, `mark-plt`, `nomark-plt`, `noreloc-overflow` (29) | refused | noop (C-R2a's first row; the class at -r wins) | C-R2a, C-R2b |
| `-z` property keywords (`force-bti`, `gcs=always`, `stack-size=`, `ibt`, `shstk`, `indirect-extern-access`, `x86-64-v2/v3/v4`, `lam-u48`, `lam-u57`), `-z unique-symbol` | refused | refused (`-z KEYWORD with -r`) | C-R2a |

Every other row refused in a final link keeps its final-link status at
`-r` (refused by its name, or unknown: C-R2a's last paragraph, C-R2b).
