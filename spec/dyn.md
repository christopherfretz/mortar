# spec: dynamic executables (`mortar_synth` dynamic tables; M2)

**Status: draft (WP-040; awaiting review).** Skeleton; sections are filled
in order and committed one by one.

## 1. Scope and status
## 2. Census (measured)
### 2.1 Default link lines
### 2.2 Inputs on a default line
### 2.3 Options on the lines and their status today
### 2.4 Relocation types sent into a dynamic context
### 2.5 Dynamic features of the system's shared objects
### 2.6 kiln's libc++ lane
### 2.7 What is refused today, ranked
## 3. Inventory of what the specs already say

Read at 7bfe1af. Status: **ok** complete and consistent; **unmeasured**
specified, not backed by an experiment in its spec (the column "here"
says which this package re-measured, §A); **incomplete** (what is
missing); **contradicts** (both rule ids). Line numbers are of 7bfe1af.
Only M2 rules and the M4 rules a dynamic executable also needs are
listed; rules that are right are cited by the rules of this spec, not
restated.

### 3.1 Input side: `resolve.md`, `input.md`, `cli.md`, `script.md`, `model.md`

| area | rules | status | here |
|---|---|---|---|
| Reading a shared object | input §8.1 (L730-763): `DT_SONAME`, `DT_FLAGS_1`, `DT_NEEDED`, `DT_RUNPATH`/`RPATH`, versioned `.dynsym`; `DF_1_PIE` input, no section headers, two sonames refused | ok; incomplete: which verdef/verneed fields matter (D-14) | E-shlib |
| `.dynsym` entries as mentions | resolve §9.1 (L1097-1115) | ok; unmeasured "hidden entries ignored" (L1112) | E-shlib |
| `versym` and keys | resolve §9.2-§9.4, model §2.3 | ok (E16 of resolve) | — |
| Rank of a shared definition | resolve §3.1 L113, §3.2 rules 2, 3, L161 | ok; incomplete: closure files are not excluded (D-12) | E-shlib |
| Soname units, needed names | input §8.2-§8.5 | ok | E-shlib |
| The `DT_NEEDED` closure | input §8.6 (L829-945) | ok for loading and search; incomplete: what a closure file's definitions satisfy (D-12); **contradicts** resolve §5.2 L769 on a missing indirect library (D-13) | E-shlib |
| `--as-needed` | resolve §9.5 L1196-1211, §4.4 | ok for the strong/weak rule; incomplete: merged soname state vs row state, order against `ref_dynamic` (D-11) | E-shlib |
| `--copy-dt-needed-entries` | resolve §9.5 L1213 "refused (M4)" | **contradicts** input §8.6 L928-934 and §15 L1645 "supported (M2)"; ruled in D-15 | E-shlib |
| `--no-allow-shlib-undefined` | resolve §5.2 L769 (lld's rule) | **contradicts** input §8.6 L940-945 (GNU's rule); ruled in D-13 | E-shlib |
| Export from an executable | resolve §9.6 L1224-1231 | ok; incomplete: closure files' references (D-12); `--export-dynamic-symbol` M2 here **contradicts** resolve §1.2 L63 (M4) | E-dynsym |
| `.dynsym` form of a `Shared` def | resolve §3.7 L315 (always an undefined entry) | **contradicts** §9.6 L1245 (a copy is defined) and the canonical-PLT form; ruled in D-23 | E-dynsym, E-reloc |
| Preemption in an executable | resolve §9.6 L1237 | ok | — |
| Copy-relocation inputs | resolve §9.6 L1245-1257, §7.3 aliases | ok; unmeasured: protected (L1256); incomplete: `STT_TLS`, `st_size` 0, IFUNC-typed (D-42) | E-reloc |
| Weak undefined, dynamic or not | resolve §5.4 L819 "arch.md says" | **contradicts** arch §2.1 L77 "resolve.md's" (circular); ruled in D-25 | E-reloc |
| `STB_GNU_UNIQUE` from a shared object | resolve §3.4 L251, §9.1 | incomplete: output binding of a copied unique, `EI_OSABI` (D-24) | E-corpus |
| `tls-mismatch` | resolve §3.4 L230-245 | incomplete: a shared `STT_TLS` definition vs a non-TLS object reference (D-16) | — |
| `_DYNAMIC`, `_TLS_MODULE_BASE_` | resolve §8.1 L1042, L1053 | ok; static-pie counts as dynamic (D-30) | E-dynamic |
| Output kind from the line | cli §3.2 L297-317 | ok (`-pie` without an interpreter is static-pie) | E-census |
| Default interpreter | input §8.4 L807-817 (refused: none) | **contradicts** synth S-106 "or the target's default"; ruled in D-31 | E-census |
| `plan.dynamic` fields | cli §3.6 L399-415 | incomplete: defaults of `new_dtags` and `hash_style` (D-33, D-20) | E-dynamic |
| `--hash-style` | cli §2.3 L107, options row L209 (noop, all values) | **contradicts** synth S-102 ("sysv/both refused (cli.md)"); ruled in D-20 | E-dynsym |
| Option rows | cli §4.2 L586-631, `docs/status/options.md` | the dynamic rows are noop only "because dynamic is refused": each re-judged in §2.3 | E-census |
| Implicit scripts | script §2.3-§2.5, §6; input §5 | ok (libc.so, libm.so, libgcc_s.so for the three targets) | E-corpus |
| Model columns | model §3.3 `file_live`, `def`; §3.7; §4.3 `Slots` | ok; incomplete: no `file_live` state for a closure file reached only from unneeded objects; two sources of export (`exported`, `EXPORT`) (D-12, D-22) | — |

### 3.2 Architecture: `arch.md`, `a64.md`, `x64.md`, `x86.md`

| area | rules | status | here |
|---|---|---|---|
| Output and target kinds | arch §2.1 | ok, except the weak-undefined circle above | — |
| Demand classes per output × target | arch §4.3 L792-808, a64 §5 | ok for a64 and x64; **contradicts** x86 X86-50 for an i386 IFUNC in a PIE (arch L793/797 vs X86-50 L1230-1253; request R-14 of x86.md unapplied) | — |
| Slot contents and dynamic record per output × target | arch §4.4 delegates; a64 §10.1 | ok for a64; **incomplete** for x64 (X-35 static only) and x86 (X86-46 static only) | E-reloc |
| Dynamic types | a64 §6.2; x64 §6.2; x86 §6.2 | a64 ok, except DTPMOD64/DTPREL64 "in shared" **contradicts** a64 §10.1 L1091 (exec/PIE unrelaxed GD); x64 lacks `DTPOFF64`, `TPOFF64`, `R_X86_64_64` as dynamic types and the word/`r_sym`/addend columns; x86 lacks the REL word contents (X86 §6.2 L693-700) | E-reloc |
| PLT, PLT0, `.got.plt` | a64 §10.2-§10.3 (measured bytes); arch A.8 (x86-64), B.4 (i386), X86-52 | a64 ok; x86-64 "not measured" (x64 §10.3); i386 IBT "as x64's" cannot be literal; lazy word of x86 and `-z now` unspecified everywhere | E-reloc |
| `.got.plt[0]` / `.got[0]` = `_DYNAMIC` | a64 §10.1 Q8, synth S-65 | x86-64 dynamic unspecified; S-65 **contradicts** a64 §10.1 L1084 for the static AArch64 header (M1) | E-dynamic |
| Canonical PLT `.dynsym` form | a64 §10.5 L1162 (`st_shndx` = the PLT's section) | **contradicts** the gABI convention (`SHN_UNDEF`, `st_value` = the entry; measured, D-23) | E-reloc |
| Copy target section | a64 §10.5 (`.data.rel.ro`) | **contradicts** layout L-80 and §8.2 (`.bss.rel.ro`); ruled in D-42 | E-reloc |
| TLS in Exec/PIE | a64 R1-R5, §10.1, §10.6 (TLSDESC eager); x64 X-23-X-32; x86 X86-32-37 | a64 ok (TLSDESC eager unmeasured, Q10); **x64 and x86: every TLS relaxation is scoped to static and static-pie**; GD→IE and TLSDESC→IE have no bytes | E-reloc |
| GOT relaxation in Exec/PIE | a64 R5; x64 X-19 ("every M1 output") vs arch A.2 ("every output kind"); x86 X86-25 (static), X86-27 (candidates) | a64 ok; x64/x86 incomplete and mildly contradictory | E-reloc |
| IFUNC in a dynamic output | a64 §10.4 (`STT_FUNC` at the IPLT entry in `.dynsym`); synth S-71 (`STT_GNU_IFUNC` at the resolver in `.symtab`) | **contradicts** (S-71 vs a64 §10.4; only `.symtab` is S-71's); x86 Exec/PIE unspecified | E-reloc |
| AArch64 dynamic tags | a64 §10.3 L1124-1128 (BTI_PLT, PAC_PLT, VARIANT_PCS) | ok; incomplete: VARIANT_PCS for an IRELATIVE in `.rela.plt` | E-dynamic |
| RELR | a64 §6.3, arch §3.10 | eligibility "place aligned to word" is address-dependent on x86 (D-46) | E-dynamic |
| Generic contract | arch §3.7 `PltShape`, §3.10 `DynRole` | incomplete: lazy-word target, `-z now` variant, per-arch dynamic tags, record format per role (REL vs RELA), WeakDyn decision (§11) | — |

### 3.3 Output side: `synth.md`, `layout.md`, `image.md`, `plumb.md`

| area | rules | status | here |
|---|---|---|---|
| Chunk list | synth S-3 rows 1, 4-11, 18 | incomplete: no `.plt.sec`, no `.hash`, `.interp` "dynamic output" includes static-pie (contradicts cli §3.2 L315), the `GLIBC_ABI_DT_RELR` need | E-dynamic |
| Division and sizes | synth S-4, S-108 | incomplete: no rows for any dynamic chunk | — |
| `.dynsym` | synth S-101 | incomplete: no nbuckets formula, "hashed" undefined, import `st_value`/`st_shndx` | E-dynsym |
| `.gnu.hash` | synth S-102 | incomplete: Bloom size and shift | E-dynsym |
| Versions | synth S-103 | incomplete: `vna_other`, `vna_hash`, `vn_file`, `VER_FLG_WEAK`; S-3 row 8 (verdef) tagged M2 **contradicts** resolve §9.7 / ROADMAP (M4) | E-dynsym |
| `.rela.dyn` | synth S-104 | order unmeasured; IRELATIVE "to `.rela.plt`" **contradicts** S-70 for static-pie; RELR eligibility contradicts S-107 on x86 | E-reloc |
| `.dynamic` | synth S-105 | **incomplete, the largest gap**: no `DT_INIT*`/`FINI*`, `PLTGOT`, `JMPREL`, `RELA*`, `VERSYM`, `VERNEED`, `GNU_HASH`, `SYMTAB` … and no presence rule per tag | E-dynamic |
| `.interp` | synth S-106 | **contradicts** input §8.4 (default) | E-census |
| IPLT word | synth S-69 (written 0) | **contradicts** a64 §6.2 L598, L605, §10.2 L1105 (`S+A`); M1, out of this package: listed in §15 | — |
| Section order | layout L-60 (ranks 1, 3, 4, 7, 8) | unmeasured for dynamic links (E-8 was static); no slot for `.plt.sec` or the writable copy area | E-dynamic |
| Segments | layout L-65, L-67, L-68, L-80 | ok for PIE base 0 and RELRO contents; `PT_INTERP` "dynamic, not -shared" includes static-pie (contradicts cli §3.2); phdr order measured on static only | E-dynamic |
| Milestones | layout §1 L52, L-100 | "M2 … and shared objects" **contradicts** ROADMAP (M4) | — |
| `.relr.dyn` in the fixed point | layout L-21, L-121; arch §6 | ok | — |
| Section headers of dynamic chunks | image I-40 | incomplete: `sh_link`/`sh_info`/`sh_entsize` of every dynamic section, `e_type` per kind | E-dynamic |
| Scan counts | image I-15 | incomplete: counts do not order or split RELATIVE/RELR | — |
| Debug tombstone in a PIE | image I-47 note | open M2 item (lldb lost lines at low addresses) | E-dynamic |
| Verifier | plumb §1 (stage 3 "not started"), S1 (`ET_EXEC` only), inputs `ET_REL` only, no copy or dynsym map records | **cannot verify any dynamic output**; requests in §12 | — |
## 4. Output kinds
## 5. Shared objects as inputs

Most of this is specified (input.md §8, resolve.md §9); E-shlib (116
experiments × 4 linkers, AArch64 and x86-64, every output run) confirmed
it rule by rule. The rules below fill the gaps, settle the two
contradictions and record what was measured. lld 19 and 22 agreed in
every experiment.

**D-10 [M2] (what is read; confirmed).** input §8.1 and resolve §9.1-§9.3
stand. Measured: one file per soname, a later file with that soname
ignored (E-shlib 1.5); the needed name rules of input §8.3 (1.2a-e);
`STB_GNU_UNIQUE` binds like `STB_GLOBAL` (5.9); a reference to `foo`
binds `foo@@V2`, `.symver` `foo@V1` binds the hidden `foo@V1`, a missing
version is undefined, a hidden-only version never binds an unversioned
reference (5.8a-d); an IFUNC defined in a shared object is an ordinary
function definition, address equality holding in PIE and non-PIE (5.10).
A `STV_HIDDEN` entry in a shared object's `.dynsym` is ignored (resolve
§9.1 L1112): all three references bind to it and their outputs fail to
load (`symbol lookup error: undefined symbol: vf`, 5.7, a wrong output
of all three; `patch_vis.py`); mortar reports the reference undefined.
*Test:* `shlib-hidden-dynsym` (the patched library; mortar refuses with
`undefined-symbol`, never writes an output that cannot load).

**D-11 [M2] (needed).** A shared object of the command line or a script
is **needed** (`file_live = Shared{needed: true}`) when its soname unit's
*merged* `as_needed` (input §8.2: on only if on at every mention) is
off, or when it gives `def` to a key with

1. a strong reference from a live relocatable object (any of its
   relocations or symbol-table mentions, whether or not the section
   holding it survives `--gc-sections`: needed is decided by resolution,
   before liveness; GNU ld and mold agree, lld drops the library, E-shlib
   2.6a, all three run), or from file 0 (`-u`, `--defsym`, `ENTRY`); or
2. a strong reference from a needed shared object or a used closure file
   (D-12) that does not itself list the defining object in its own
   `DT_NEEDED` (an under-linked library: GNU ld makes the provider
   needed and the program runs; lld and mold drop it and the program
   fails with `undefined symbol: mid_fn`, a wrong output of both, E-shlib
   2.3b).

It is a least fixed point over shared objects (a needed one's references
can make another needed), computed after `def` (resolve §12). A weak
reference alone never makes an object needed (GNU ld and lld; mold
differs, 2.2a); the key's output view is then `Undefined{weak: true}`
(resolve §9.5) and D-25 gives its `.dynsym` form. A library supplying only
a weak definition that a strong object definition beats is not needed
(2.9). Position does not matter (resolve §9.5; GNU ld refuses
`--as-needed liba.so m.o`, 2.8a). `libc.so`'s
`AS_NEEDED(ld-linux-*.so.*)` makes the loader needed only when a
reference binds to a symbol only it defines (`__tls_get_addr`, 1.8a);
a hello needs `libc.so.6` alone (1.7). *Tests:* `as-needed-weak`,
`as-needed-underlinked` (runs), `as-needed-gc`, `as-needed-merge`,
`needed-ldso-tls`.

**D-12 [M2] (closure files).** The `DT_NEEDED` closure is loaded (input
§8.6). Resolution treats a closure file `c` thus:

- `c` is **used** when some needed shared object or used closure file
  lists it in `DT_NEEDED` (a reachability over closure edges from needed
  objects only); `file_live = Closure` for a used one, `Dead` otherwise
  (request to model.md §3.3, R-M1).
- `c`'s definitions are never candidates for a key referenced by a
  relocatable object or file 0: such a reference is undefined, with the
  note `defined in <c>, which is not on the command line` (GNU ld:
  ``undefined reference to symbol 'mid_fn'`` and `DSO missing from command
  line`; lld and mold: `undefined symbol: mid_fn`; E-shlib 2.10a).
- `c`'s definitions do satisfy references of shared objects when the
  shlib-undefined check judges them (D-13), and `c`'s references set
  `ref_dynamic`: a program symbol referenced only by a used closure file is
  exported (GNU ld; lld and mold do not export and the program fails with
  `symbol lookup error: … undefined symbol: hook`, a wrong output of both,
  E-shlib 4.2a).
- `c` is never `DT_NEEDED` (input §8.6).

`ref_dynamic` and `defined_dynamic` count needed shared objects and used
closure files only, so they are computed after D-11's fixed point (request
R-R3 to resolve.md §3.6, §12.1; an executable definition that an
as-needed, unneeded library also defines is not exported: GNU ld and mold;
lld exports it, harmless, E-shlib 2.5a). Archive extraction (resolve
§4.2) still takes every shared object's references as edges, needed or
not, and closure files' too [open Q1: unmeasured whether GNU ld extracts
for an unneeded as-needed library's reference].
*Tests:* `closure-export` (runs), `closure-not-on-command-line`
(the note), `closure-unused`.

**D-13 [M2] (shared objects' undefined references).** In `Exec` and `Pie`
the check of resolve §5.2 is on by default (`--no-allow-shlib-undefined`;
GNU ld and lld; mold never checks, E-shlib 3.1a);
`--allow-shlib-undefined` turns it off. It judges every strong undefined
reference of every needed shared object and used closure file (GNU ld
checks closure files, 3.4a; an as-needed object that is not needed is not
judged: lld judges it and refuses a link that runs, 3.6). A reference is
satisfied by any definition of the program, of a needed shared object or
of a used closure file, with the version it requires (D-10). When a
closure library is **not found** (`needed-not-found`, input §8.6), the
references of the object that needs it are judged like the others, and
any that only the missing library would define are undefined: GNU ld's
rule (3.5, 4.1); lld exempts such an object. This **replaces** resolve
§5.2 L769's exemption (request R-R1): the conservative rule is the one
the default `gcc` line already applies, and `--allow-shlib-undefined` or
`-rpath-link` resolves it. Message: `undefined-symbol` with the note of
resolve §5.3 and the needing file as the place (GNU ld:
``d/libx.so: undefined reference to `nobody'``). *Tests:*
`shlib-undefined`, `shlib-undefined-closure`, `shlib-undefined-dropped`,
`shlib-undefined-missing-needed`.

**D-14 [M2] (versions read, versions not checked).** From a shared object
mortar reads its `versym`, its version definitions (names, `VER_FLG_BASE`
for the base entry, the hash it writes into a need, D-27) and nothing of
its version needs: a shared object's own needs of its dependencies are the
loader's to check (GNU ld, lld and mold do not check them; glibc does, at
load). `VER_FLG_WEAK` on an input definition has no meaning to the link.
*Test:* `version-needs-unchecked` (a library needing a version its
dependency lacks links with every reference and mortar).

**D-15 [M2] (`--copy-dt-needed-entries` refused).** Only GNU ld accepts it
(lld: `unknown argument`, mold: `unknown command line option`, E-shlib
2.10b), no census line passes it (§2), and GNU ld appends the closure
library after `libc.so.6`. It stays refused by name; input §8.6 L928-934
and input §15 L1645 ("supported (M2)") are withdrawn (request R-I1);
`--no-copy-dt-needed-entries` is the default and becomes `noop` with that
reason. *Test:* `copy-dt-needed-refused`.

**D-16 [M2] (TLS against a shared definition).** A TLS class relocation
against a key whose `def` is a non-`STT_TLS` shared definition, or a
non-TLS class against an `STT_TLS` shared definition, is `tls-mismatch`
(resolve §3.4 extended to shared definitions, request R-R4) [unverified
against the references]. *Test:* `tls-mismatch-shared`.

**D-17 [M2] (common against a shared definition).** resolve §3.2 rule 2
stands: GNU ld and mold give the shared data definition (`5 5`), lld the
common (`0 0`), E-shlib 5.4.

**D-18 [M2] (protected definitions).** A `COPY` or `CANON_PLT` demand on a
key whose shared definition is `STV_PROTECTED` is refused by name,
`copy-relocation-against-protected` (scan, phase 70): all three refuse
the copy (GNU ld: ``copy relocation against non-copyable protected symbol
`pv' in d/libprot.so``; lld: `cannot preempt symbol: pv`; mold: `cannot
create a copy relocation for protected symbol 'pv'; recompile with
-fPIC`), and GNU ld and lld refuse the canonical PLT (GNU ld x86-64:
``non-canonical reference to canonical protected function `pf'``; lld
`cannot preempt symbol: pf`); mold links it and the program breaks
pointer equality (prints `9 0` and glibc warns ``direct reference to
protected function``), a wrong output of mold, E-shlib 5.6d. A PIE that
reaches a protected datum through the GOT needs neither and links
(AArch64, 5.6a). *Tests:* `protected-copy`, `protected-canon-plt`.

**D-19 [M2] (input forms; confirmed).** `-Bstatic` before a shared object
named by path is `static-link-of-shared-object` (GNU ld and lld; mold
links it dynamically, 6.1f); an empty `.so` is ignored (input §2.3; GNU
ld and lld; mold refuses, 6.5); a non-ELF text `.so` is read as a script
and fails there; `.so` before `.a` within one directory, first directory
wins (6.1). `LD_LIBRARY_PATH` and `LD_RUN_PATH`, which a *native* GNU ld
searches for closure libraries ahead of `DT_RUNPATH` (and the cross GNU
ld does not, 4.4), are never read (input §4.7): a native GNU ld link that
finds a closure library only through them fails in mortar with
`needed-not-found` and the D-13 consequence, which the note names.

**D-19a [M2] (`DT_NEEDED` order).** `Slots.needed` holds the needed shared
objects in `FileId` order (the command line and scripts in order, objects'
positions irrelevant; one entry per soname). All three agree (E-shlib 1.3,
1.6a), except that lld refuses 1.6a for D-13's reason and mold writes the
found path for `INPUT(-lns)` of a soname-less library (6.3). *Test:*
`needed-order`.
## 6. `.dynsym`, `.dynstr`, hash tables, versions
## 7. `.dynamic`, `.interp`, `_DYNAMIC`
## 8. Dynamic relocations, copy relocations, canonical PLTs, TLS
## 9. Layout
## 10. Debuggers and tools
## 11. Requests to the architecture specs and `arch.md`
## 12. Requests to `plumb.md`
## 13. Where the references differ; wrong outputs
## 14. Hooks for M6 and M7
## 15. Open questions
## 16. Tests
## Appendix A. Experiments
## Appendix B. Not verified
