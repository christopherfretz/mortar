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
`spec/diag.md` §4.4); **open** (no rule classifies it: a question of
`spec/landed/plumb/WP-180.md`, Q-O1 to Q-O7). "noop: measured,
unlisted" is a row C-R2 does not list but every reference measured
leaves byte-identical: WP-180 proposes it for C-R2 (Q-O1).

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
| `--strip-discarded` | noop | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `-x`, `--discard-all` | supported | acts; refused until RL-11 | C-R3, I-R9 |
| `-X`, `--discard-locals` | supported | noop (C-R2) | C-R2 |
| `--discard-none` | supported | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `--build-id` | partial | acts: the note over the -r output | C-R3, I-R8 |
| `--no-build-id` | supported | noop (C-R2) | C-R2 |
| `--compress-debug-sections` | partial | refused (`--compress-debug-sections with -r`); `=none` no-op | C-R4 |
| `--eh-frame-hdr` | supported | noop (C-R2) | C-R2 |
| `--no-eh-frame-hdr` | supported | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `--gc-sections` | supported | refused (`--gc-sections with -r`) | C-R4, I-R10 |
| `--no-gc-sections` | supported | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `--print-gc-sections` | supported | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `--no-print-gc-sections` | supported | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `--gc-keep-exported` | supported | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `--gdb-index` | supported | refused (`--gdb-index with -r`) | C-R4 |
| `--no-gdb-index` | noop | noop: measured, unlisted (open, Q-O1) | rl0-opts (lld) |
| `--no-debug-names` | noop | noop: measured, unlisted (open, Q-O1) | rl0-opts (lld) |
| `-l`, `--library` | supported | noop (C-R2): picks the inputs, extraction by RV-R2 | C-R2, RV-R2 |
| `-(`, `--start-group` | supported | noop (C-R2): acts on extraction | C-R2, RV-R2 |
| `-)`, `--end-group` | supported | noop (C-R2) | C-R2 |
| `--start-lib` | supported | open (Q-O2): lazy objects as in a final link (lld's form; GNU ld does not know it) | RV-R2 |
| `--end-lib` | supported | with `--start-lib` | RV-R2 |
| `--as-needed` | supported | noop (C-R2) | C-R2 |
| `--no-as-needed` | supported | noop (C-R2) | C-R2 |
| `--whole-archive` | supported | noop (C-R2): acts on extraction | C-R2, RV-R2 |
| `--no-whole-archive` | supported | noop (C-R2) | C-R2 |
| `-Bstatic`, `--dn`, `--non_shared`, `--static` | supported | noop (C-R2); a `.so` is refused | C-R2, C-R5 |
| `-Bdynamic`, `--dy`, `--call_shared` | supported | noop (C-R2); a `.so` is refused | C-R2, C-R5 |
| `--no-copy-dt-needed-entries`, `--no-add-needed` | noop | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `--push-state` | supported | noop (C-R2) | C-R2 |
| `--pop-state` | supported | noop (C-R2) | C-R2 |
| `--no-dependent-libraries` | noop | noop: measured, unlisted (open, Q-O1) | rl0-opts (lld) |
| `--warn-backrefs` | supported | as in a final link (a diagnostic) | rl0-opts |
| `--no-warn-backrefs` | supported | as in a final link | rl0-opts |
| `-L`, `--library-path` | supported | noop (C-R2) | C-R2 |
| `--sysroot` | supported | as in a final link (the search) | rl0-opts |
| `-rpath-link` | supported | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `--version-script` | supported | noop (C-R2) | C-R2 |
| `--dynamic-list` | supported | conflicting-options with -r | C-R4 |
| `--sort-common`, `--sort_common` | supported | noop (C-R2) | C-R2 |
| `--sort-section` | supported | noop (C-R2) | C-R2 |
| `-u`, `--undefined` | supported | acts: a root, and `UND` written | C-R3, RV-R2, RV-R3 |
| `--require-defined` | supported | open (Q-O3): GNU ld's output identical; a root or not | rl0-opts |
| `--defsym` | partial | refused until RL-11 (`--defsym with -r`) | C-R3, RV-R10 |
| `--wrap` | partial | refused (`--wrap with -r`) | C-R4 |
| `--export-dynamic-symbol` | supported | conflicting-options with -r (C-R4's `--export-dynamic*`) | C-R4 |
| `--exclude-libs` | partial | noop (C-R2) | C-R2 |
| `-init` | supported | open (Q-O4): GNU ld identical; no-op or conflicting | rl0-opts |
| `-fini` | supported | open (Q-O4) | rl0-opts |
| `--undefined-version` | supported | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `--no-undefined-version` | supported | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `-E`, `--export-dynamic` | supported | conflicting-options with -r | C-R4 |
| `--no-export-dynamic` | supported | open (Q-O4): GNU ld identical | rl0-opts |
| `-Bsymbolic` | supported | conflicting-options with -r | C-R4 |
| `-Bsymbolic-functions` | supported | conflicting-options with -r | C-R4 |
| `-Bno-symbolic` | supported | open (Q-O4): GNU ld identical; C-R4's `-Bsymbolic*` does not name it | rl0-opts |
| `-h`, `-soname` | supported | conflicting-options with -r | C-R4 |
| `-rpath` | supported | conflicting-options with -r | C-R4 |
| `-I`, `--dynamic-linker` | supported | noop (C-R2) | C-R2 |
| `--no-dynamic-linker` | supported | noop (C-R2) | C-R2 |
| `--enable-new-dtags` | supported | conflicting-options with -r | C-R4 |
| `--disable-new-dtags` | supported | open (Q-O4): C-R4 names only `--enable-new-dtags` | — |
| `--hash-style` | partial | noop (C-R2) | C-R2 |
| `--rosegment` | noop | noop: measured, unlisted (open, Q-O1) | rl0-opts |
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
| `--odr-table` | supported | open (Q-O5): a table at -r? | MT-30a |
| `--keep-mason-meta` | supported | open (Q-O5): MT-30 always writes the container at -r | MT-30 |
| `--detect-odr-violations` | noop | noop: measured, unlisted (open, Q-O1) (no reference knows it) | rl0-opts |
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
| `--no-incremental` | supported | noop: measured, unlisted (open, Q-O1) | rl0-opts (no reference knows it) |
| `--incremental-state` | supported | open (Q-O6): with `--incremental`'s warning | IN-12 |
| `--incremental-report` | supported | open (Q-O6) | IN-12 |
| `-plugin` | noop | noop (C-R2); an LTO object still refused | C-R2, RV-R9 |
| `-plugin-opt` | noop | noop (C-R2) | C-R2 |
| `-plugin-save-temps` | noop | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `-flto` | noop | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `-flto-partition` | noop | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `-fuse-ld` | noop | noop: measured, unlisted (open, Q-O1) | rl0-opts |
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
| `-g` | noop | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `-O` | noop | noop (C-R2) | C-R2 |
| `-Qy` | noop | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `-qmagic` | noop | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `-assert` | noop | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `--map-whole-files` | noop | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `--no-map-whole-files` | noop | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `--no-keep-memory` | noop | noop: measured, unlisted (open, Q-O1) | rl0-opts |
| `--reduce-memory-overheads` | noop | noop (GNU ld reorders `.symtab`; mortar the same bytes) | rl0-opts |
| `--hash-size` | noop | noop (as `--reduce-memory-overheads`) | rl0-opts |
| `--max-cache-size` | noop | noop: measured, unlisted (open, Q-O1) | rl0-opts |

## Rows refused in a final link that `-r` names

| Option | Final link | At `-r` | Rule |
|---|---|---|---|
| `-r`, `-i`, `--relocatable` | refused | acts: the kind; refused (`relocatable output (-r)`) until RL-7 | C-R1 |
| `-Ur` | refused | acts as `-r` (`plan.ur`) | C-R1 |
| `-d`, `--dc`, `--dp` | refused | acts: commons allocated; refused until RL-11 | C-R3, RV-R5 |
| `-q`, `--emit-relocs` | refused | noop (C-R2; open Q-O7: refused in a final link) | C-R2 |
| `--no-relax` | refused | noop (C-R2; open Q-O7) | C-R2 |
| `--relax` | refused | refused (`--relax with -r`) | C-R4 |
| `-T`, `--script` | refused | refused until RL-11 (`-T with -r`) | C-R3 |
| `-b`, `--format` | refused | acts: `binary` (L-R9); refused until RL-10 | C-R3, L-R9 |
| `--force-group-allocation` | refused | acts; refused until RL-8 | C-R3, L-R3 |
| `--unique` | refused | acts; refused until RL-11 (`--unique with -r`) | C-R3, C-R4, L-R11 |
| `-z nosectionheader` | refused | conflicting-options with -r | C-R2a |
| `-z` property keywords (`force-bti`, `gcs=always`, `stack-size=`, `ibt`, `shstk`, `indirect-extern-access`, `x86-64-v2/v3/v4`, `lam-u48`, `lam-u57`), `-z unique-symbol` | refused | refused (`-z KEYWORD with -r`) | C-R2a |

Every other row refused in a final link stays refused at `-r` (C-R2a's
last sentence; `spec/cli.md` §3.16).
