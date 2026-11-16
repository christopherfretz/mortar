## D. The cut into work packages

The first joined target: **a `-shared` C library with a soname on
AArch64, from the default driver line (and Python's `LDSHARED` line),
loaded by a GNU-ld-linked host and by a mortar-linked host, verified by
stage 3.** Then versions, C++, x86-64, the option tail. Names are
provisional (the supervisor numbers them).

**Estimates** are anchored to landed packages (merge `+lines` under
`crates/` and `tests/`, wall time of the implementer), in WP-040's
classes: **S** about 1-2k lines (about 1 hour), **M** about 4-6k (1-2
hours), **L** about 8-9k (about 2 hours or more). Anchors: WP-041 step 0
+2.2k, 69 min; WP-042 cli +1.4k, 78 min; WP-043 resolve +3.6k, 111 min;
WP-044 `.dynsym` +2.8k, 48 min; WP-045 `.dynamic` +1.8k, 152 min; WP-046
relocations +3.1k, 76 min; WP-047 a64 +1.6k, 100 min; WP-048 x64 +2.0k,
125 min; WP-049 layout +1.3k, 68 min; WP-050 plumb stage 3 +6.2k, 197
min; WP-051 harness +7.8k, 89 min; WP-053 a64 join +1.8k/-0.6k, 115 min;
WP-055 x64 join +0.2k, 50 min.

| package | content (rules) | depends on | files owned (no other package running at the same time touches them) | anchor | estimate |
|---|---|---|---|---|---|
| **S0** spec step 0 (docs) | apply §18's requests into the owning specs: R-A10, R-X10, R-R10, R-R11, R-M10, R-C10 (S1a's rows), R-L10-R-L12, R-S10, R-I10, R-O10, R-MT10, R-N10, R-D10 (diag.md kinds), R-D43 (D-43), P-R1-P-R9 into `plumb.md` §12; R-A11 after WP-102/103/104/109 land | this design merged | `spec/` (except dyn.md §17), `docs/status/options.md` | — | docs, 1 session |
| **S0c** code step 0 (shared types) | the types every later package compiles against, with no behaviour: `Resolution.preemptible: IdSet<SymbolId>` (R-M10, always empty outside `Shared`); `Plan.dynamic`'s symbolic form as D-112 resolves it (`None`/`All`/`Functions`, last wins, with `has_dynamic_list`), the version-script and dynamic-list file lists; `mortar_image` `OutputKind::Shared` and the map's record of the line's symbolic form, list and scripts (S-V reads them); the version-script AST (nodes, parents, entries with language, quoting and scope); the R-D10 diagnostic kinds with their `spec_tables` rows | S0 | `crates/mortar_core/src/{resolution.rs,plan.rs}`, `crates/mortar_image/src/map.rs`, `crates/mortar_script/src/ast.rs`, `crates/mortar_diag/src/{model.rs,render.rs}`, `crates/mortar_diag/tests/{kinds.rs,spec_tables.rs}` | WP-041 | S (1.5-2.5k) |
| **S-H** harness | `linktest` for a `.so` output: link it with each reference and with mortar, link hosts against it (by a reference and by mortar), run them plain and under an `LD_PRELOAD` interposer; fixture libraries built by mortar; the census and real-builds lanes count the `-shared` lines that newly link (the review's "unowned work") | S0c; **parallel with S1a, S1b, S-V** | `xtask/src/linktest/*`, `xtask/tests/linktest.rs`, `xtask/src/census.rs`, `tests/link/fixtures/` (new `shared-*.fixture` files only), `tests/link/shared/` (new: the harness's own smoke tests) | WP-051 | M (3-5k) |
| **S1a** `Shared` front, resolve and scan (AArch64) | `-shared`, `-soname`/`-h`, `-Bsymbolic-functions`, `-Bno-symbolic` (R-C10); D-110 (no script, no `--exclude-libs`; `__start_`/`__stop_` protected), D-112 (default, protected, `-Bsymbolic-functions`; the refusals of `-Bsymbolic-non-weak*`, `--dynamic-list-data`, `-cpp-*`), D-113, D-117's AArch64 table **and its address-level rule**, D-118, D-122; TLS and IFUNC in `Shared` refused by name (`feature`, until S2) | S0c | `crates/mortar_cli/src/{table.rs,rows.rs,grammar.rs}`, `crates/mortar_cli/tests/`, `docs/status/options.md`, `crates/mortar_resolve/src/{shared.rs,options.rs,linker.rs}` and new `export.rs`, `crates/mortar_scan/src/{walk.rs,driver.rs,view.rs}` and new `bound.rs`, `crates/mortar_arch/src/{class.rs,need.rs}`, `crates/mortar_a64/src/table.rs` | WP-043 + WP-046 | M-L (5-7k), 2 sessions |
| **S1b** `Shared` synth, layout and image (AArch64) | D-100, D-101, D-102 (`DT_SONAME`, `DF_SYMBOLIC` per D-112, `-init`/`-fini` rows), D-103 (`-z now` in `Shared`: `BIND_NOW`, `DF_1_NOW`; rustc's line), D-111, D-116 (no verdef), D-120, D-123, D-124, D-125; built against S0c's types and a hand-made `Resolution` until S1j | S0c; **parallel with S1a** | `crates/mortar_synth/src/{dynamic.rs,dynsym.rs,dynstr.rs,chunks.rs,rela.rs,got.rs,gnuhash.rs}`, `crates/mortar_layout/src/{order.rs,headers.rs,symbols.rs}`, `crates/mortar_image/src/{link.rs,headers.rs,dynamic.rs,dynrel.rs,output.rs}` | WP-044 + WP-045 + WP-049 | M (4-6k), 2 sessions |
| **S1j** join (AArch64) | wire S1a and S1b in `crates/mortar` (`front.rs`, `dynlink.rs`); the first target's tests in `tests/link/dyn/shared/` | S1a, S1b, S-H | `crates/mortar/src/{front.rs,dynlink.rs,dynamic.rs}`, `tests/link/dyn/shared/` | WP-053 | S (1-2k) |
| **S-V** stage 3 for `Shared` | `mortar_plumb`: P-R1-P-R8 (an independent export set and preemption set from the inputs and the map's line; the interposer view; D-117's address clause with its own decoder; the P-R8 mutations, W44 asserted as not caught) | S0c; **parallel with S1a, S1b, S-H** | `crates/mortar_plumb/src/{dynverify.rs,dynmodel.rs,loader.rs,dynmutate.rs,dynout.rs}` and new `shared.rs` (not `dyncheck.rs`: S-R43's) | WP-050 | L (6-8k), 2-3 sessions |
| **S-R43** copies of locally bound data (executables) | R-D43 (D-43 amended: refuse writable, warn read-only/RELRO, refuse outside every `PT_LOAD`, decided from program headers), P-R9 into P-D11 | S0, S0c; **parallel with every S1 package and S-V** | `crates/mortar_scan/src/copies.rs`, `crates/mortar_plumb/src/dyncheck.rs`, `tests/link/dyn/shlib/` (new `copy-of-symbolic*` tests only) | WP-055 | S (1-1.5k) |
| **S2** TLS and IFUNC, AArch64 | D-119 (trad GD, TLSDESC eager, IE with `DF_STATIC_TLS`, LE refused), D-121 | S1j | `crates/mortar_a64/src/{relax.rs,synth.rs}`, `crates/mortar_synth/src/got.rs`, `crates/mortar_synth/src/rela.rs` | WP-047 | S (1-2k) |
| **S3** versions | the script.md §9 parser (can start right after S0c: it owns only `mortar_script`); then D-114, resolve §9.7 (the vmatch2 model's mortar column is the test), D-115, D-116's verdef names, `.symver` definitions, `--no-undefined-version` (rustc's line) | parser: S0c; the rest: S1j | `crates/mortar_script/src/{lex.rs,parse.rs,lib.rs}` and new `version.rs`; later `crates/mortar_resolve/src/` new `version.rs`, `crates/mortar_synth/src/version.rs` | WP-042 + WP-044 | M (3-5k) |
| **S4** C++ | googletest end to end (`STB_GNU_UNIQUE`, vague linkage, EH across the boundary, typeinfo); `-static-libstdc++` in `-shared` (its `libstdc++.a` brings a `TLSDESC` and `.tbss`: D-119, hence S2); `extern "C++"` in scripts once the demangler exists | S1j, **S2**, S3; the demangler | tests mostly: `tests/link/dyn/cxx/` (new `shared-*` tests) | WP-055 | S (1-1.5k, mostly tests) |
| **S5** x86-64 | D-117's x86-64 rows (the address rule's `+4+t` form), D-119 (GD/LD, gnu2, IE), D-120, D-121 on x86-64; the hosted x86-64 lane | S2 | `crates/mortar_x64/src/*` | WP-048 + WP-055 | S-M (2-3k) |
| **S6** option tail | D-103 (`-z nodelete`/`nodlopen`/`initfirst`/`interpose`/`origin`), `-Bsymbolic` (`DT_SYMBOLIC`), `--dynamic-list` (grammar from S3; the list decides), `--exclude-libs`, `-init`/`-fini` with `-shared`, `--whole-archive` | S1j, S3 | `crates/mortar_cli/src/rows.rs`, `crates/mortar_resolve/src/export.rs`, `crates/mortar_synth/src/dynamic.rs` (after S1a/S1b/S3 have merged) | WP-042 | S (1.5-2k) |
| **S7** metadata | D-126 items 1 and 3 (`.note.mason` rows with `NONPREEMPTIBLE`, versions; `--keep-mason-meta`) | S1j, S3 | `crates/mortar_meta/src/*`, `crates/mortar_odr/src/dynamic.rs` | — | S (0.5-1k) |
| **S8** i386 | D-130 | i386 dynamic executables, S5 | `crates/mortar_x86/src/*` | — | later |

**What starts first, and the parallel fronts.** S0, then S0c (both
short). Then six worktrees at once: S1a, S1b, S-H, S-V, S-R43 and S3's
parser, with disjoint files as the table says. S1j joins S1a and S1b
once S-H can run a host; **the first target is gated on S1j + S-V +
S-H** (a library loaded by a GNU-ld host and a mortar host under the
harness, verified by stage 3). After S1j: S2, S3's rest, S6 (after S3)
and S7 in parallel; S4 after S2 and S3; S5 after S2. Every package
keeps every M1/M2 output byte-identical (a non-shared link pays nothing
for `Shared`: the only new per-key datum is `preemptible`, empty outside
`Shared`).

**Where the refusal stays.** Until its package lands each item keeps a
named refusal: TLS and IFUNC in `Shared` (S2), `--version-script` and
`.symver` definitions in a shared output (S3), `extern "C++"` patterns
(the demangler), the option tail (S6), i386 `Shared` (S8), `-shared`
with `--incremental` (IN-12 stays: R-N10). Permanently refused, as
designed: text relocations (`-z notext`), LE TLS, every row D-117
refuses and its address-level rule, `-Bsymbolic-non-weak*`,
`--dynamic-list-data`, `--dynamic-list-cpp-*`, `-f`/`-F` filters,
`--hash-style=sysv` and `both`, `--spare-dynamic-tags`, RELR until its
own package.

**Lines unlocked, in order:** S1j: Lua 10/15 build lines, Python
extension modules (`LDSHARED`), and the gate. S3: zlib's build line and
2 probes, rustc `cdylib` lines. S4: googletest 4, expat and gmp probes
(19/19 of the survey).

