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
