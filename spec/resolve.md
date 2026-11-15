# `mortar_resolve`: symbol resolution

Status: **accepted for M1** (WP-022; reviewed independently, implemented in `mortar_resolve`). Written into `spec/model.md` (normative):
this spec defines how every column of `Resolution` (§3.3 there) is
computed from `Inputs` (§3.2) and the `Plan`. The code is WP-022 stage 2.
Every "must" has a test (§15); §16 lists where the reference linkers
differ and which way mortar goes; §18 lists the changes this spec asks of
other specs.

Sources: the gABI ("Symbol Table": binding, visibility, `SHN_COMMON`,
`SHN_ABS`; "Section Groups"; "Archive File"), the GNU extensions
(`STB_GNU_UNIQUE`, `STT_GNU_IFUNC`, symbol versioning as in the Linux
Standard Base "Symbol Versioning"), GNU ld's manual 2.44 (by section name:
"Command-line Options", "VERSION", "PROVIDE", "Options Specific to ELF"),
and where those are silent, what GNU ld 2.44, `ld.lld-22` (22.1.8) and mold
2.37.1 were measured to do. The measurements, with the command lines that
reproduce them, are in `spec/landed/resolve/WP-022-spec.md` (written "E*n*"
below). A claim with neither a source nor an experiment says "not
verified".

Milestones (`docs/ROADMAP.md`): **M1** static executables; **M2** dynamic
executables (shared objects as inputs, versions, `--as-needed`, PIE);
**M4** shared-object output, version scripts, `-r`, `--wrap`, `--defsym`,
`--exclude-libs`, `--start-lib`, `-y`. Each rule is marked; a construct
whose milestone has not come is refused by name (§1.2).

## 1. Scope

### 1.1 What resolution decides

Given `Inputs` (every file loaded, every non-local symbol-table entry
interned to a `SymbolId`) and the `Plan`, resolution decides:

- which files take part (`file_live`, §4);
- for each global symbol key, which mention defines it and the properties
  merged over its mentions (`def`, `common`, `clash`, §3);
- which references are redirected (`redirect`, §7);
- which copy of each COMDAT group is kept (`comdat`, §6);
- the fate of every section of a live object (`section_base`,
  `sections`, §10);
- the verdict on unresolved references (§5), which the scan reports.

It reads no relocation (`spec/model.md` §5): every rule below is stated on
symbol-table entries, section headers and group sections. Diagnostics that
need a relocation's place (an undefined reference, a reference into a
discarded section, a `.gnu.warning`) are decided here and emitted by the
scan (`spec/arch.md`, phase 70), which reads the relocations of live
sections anyway; §14 says which.

Not here: finding and numbering files (`spec/input.md`), `--gc-sections`
(`spec/live.md`: this spec hands it the roots, §4.7), symbol values and
addresses (layout), the bytes of `.symtab` and `.dynsym` (`spec/synth.md`:
this spec gives the binding and visibility they show, §3.7), relocation
semantics (`spec/arch.md`).

### 1.2 Refused until their milestone

| construct | refusal (`spec/diag.md` §4) | until |
|---|---|---|
| a shared object among the inputs | `Refused{Feature, "shared objects as input"}` | M2 |
| `-shared` | (already refused as an option, `spec/cli.md`) | M4 |
| `-r` | `Refused{Feature, "relocatable output (-r)"}` (§11) | M4 |
| `--version-script`, `--dynamic-list`, `--export-dynamic-symbol[-list]`, `-Bsymbolic*`, `--exclude-libs` | option refusals | M4 (`--export-dynamic`, `--dynamic-list` for executables: M2) |
| `--wrap`, `--defsym`, `--start-lib`, `-y` | option refusals | M4 |
| `.gnu.linkonce.*` sections | `Refused{Feature, ".gnu.linkonce sections"}` (§6.6) | an input that needs it |
| a slim GCC LTO object (symbol `__gnu_lto_slim`) | `Refused{FileFormat, "GCC LTO object"}` (§10) | a later decision (`docs/ARCHITECTURE.md`, "Scope") |
| a weak common symbol (`STB_WEAK` with `SHN_COMMON`) | `Refused{SymbolBinding, "STB_WEAK common"}` | an input that needs it (no tool tested emits one; not verified) |
| a symbol binding other than `STB_GLOBAL`, `STB_WEAK`, `STB_GNU_UNIQUE` among non-local entries | `Refused{SymbolBinding, <name or 0x..>}` | never planned |
| a symbol type other than `NOTYPE`, `OBJECT`, `FUNC`, `SECTION`, `FILE`, `COMMON`, `TLS`, `GNU_IFUNC` | `Refused{SymbolType, ..}` | never planned |
| `--warn-backrefs-exclude` | option refusal | when asked for |

Everything else in this spec is **M1** unless marked.

## 2. Terms

- A **mention** is a non-local symbol-table entry `(FileId, st_index)` of a
  file: of a relocatable object's `.symtab`, of a shared object's
  `.dynsym` (M2), or of the internal file 0 (§8). `sym_map` gives its key.
- A **key** is a `SymbolId` (`spec/model.md` §2.3): the name before `@`
  and a version part. `foo`, `foo@@V2` are one key; `foo@V1` is another
  (§9.4).
- A mention is **defined** when its section is a section of the file
  (`Section(n)`, `elf.md` §3.5) or `Absolute`; **common** for `Common`
  (and, on x86-64, `Reserved(SHN_X86_64_LCOMMON)`, §3.3);
  **undefined** for `Undefined`. A `Reserved(n)` other than those this
  spec names is refused (`Refused{Feature, "SHN 0x.."}`).
- A **data definition** is a defined mention whose type is not `STT_FUNC`
  or `STT_GNU_IFUNC` (E3: GNU ld extracts a member for a common only for
  such a definition, §4.5).
- A file is **live** when `file_live` says so (§4). A **lazy** file is a
  relocatable object that is not live yet and may become live: an archive
  member, or an object in a `--start-lib` region (M4). Its definitions are
  **lazy candidates**.
- A mention in a live object whose section is dropped by COMDAT (§6) is
  **demoted**: it counts as an undefined mention of its key, with the
  binding it has (§6.2).
- `≺` is **command-line order** of mentions: `(FileId, st_index)`
  lexicographic. `FileId` is the priority order (`spec/model.md` §2.1).

## 3. The definition of a key

### 3.1 Candidates

Every mention of a key that is not undefined is a **candidate**, in one
class. The classes, strongest first:

| rank | class | mentions | milestone |
|---|---|---|---|
| 0 | `Assign` | file 0: a `--defsym`; a script assignment that is not `PROVIDE` | M4 |
| 1 | `Strong` | a defined `STB_GLOBAL` or `STB_GNU_UNIQUE` mention of a live object, not demoted | M1 |
| 2 | `Common` | a common `STB_GLOBAL` mention of a live object | M1 |
| 3 | `Weak` | a defined `STB_WEAK` mention of a live object, not demoted | M1 |
| 4 | `Shared` | a **visible** definition in a live shared object (§9.1) | M2 |
| 5 | `Provide` | file 0: a linker-defined symbol (§8) | M1 |

Undefined (and demoted) mentions are not candidates; they are references
(§3.6). **Only live files have candidates**: a lazy file that §4 does not
make live contributes nothing to `def`, whatever it defines (review
finding 5: a key referenced only weakly whose only definer stays in an
archive is `Undefined{weak: true}`, not the archive's definition).

### 3.2 The order

`def[K]` is the minimum candidate of `K` under

```
(rank, FileId, st_index)          lexicographic
```

with these three rules on top, each a function of per-class minima (so
still a commutative merge, §12):

1. **Extraction is decided first.** `def` is computed after `file_live`
   (§4) is final, over the candidates of live files. Which archive member
   provides a key is §4's question, not this order's: the order never
   sees a lazy file.
2. **A shared data definition beats commons alone** (M2). If the minimum
   is `Common`, the key has no `Strong` and no `Weak` candidate, and the
   least `Shared` *data* definition by `≺` exists, then
   `def = Shared{..}` of that definition, and the commons count as
   references. (GNU ld and mold, E12; lld keeps the common. Reason: in a
   static link a real definition beats commons, §3.3, and gfortran's
   `BLOCK DATA` in a shared library must initialize the common block; see
   §16.)
3. **`Provide` beats a shared object's definition.** For a key that names
   a linker-defined symbol (§8), shared objects' definitions are not
   candidates. (lld, E20: GNU ld binds `__ehdr_start`, mold `end` and
   `etext`, to a shared object that defines them: a symbol describing this
   output must not come from another module.)

With no candidate, `def = Undefined{weak}`, where `weak` is true when
every live reference is weak (§3.6).

The consequences, each measured (E1-E6, E12):

- `Strong` beats everything but `Assign`: an object's `--defsym`d name
  takes the assigned value with no duplicate error (all three, E13).
- `Common` beats `Weak` (GNU ld, lld; gABI: a common is an `STB_GLOBAL`
  symbol, and "the link editor honors the global definition and ignores
  the weak ones"). mold takes the weak definition (E6): §16.
- `Weak` beats `Shared`: a weak definition in a live object is not
  replaced by a shared object's definition (all three, E12). Whether it
  is replaced by an archive member is §4.1: a weak definition **before**
  the archive keeps the member out (all three, E3); one **after** it does
  not (GNU ld and lld extract, run=2; mold does not, R1).
- Among weak definitions the first by `≺` wins (`tests/link/resolve/
  weak-strong.test`, all three).
- An archive before a shared object on the line provides the symbol, a
  shared object before an archive does (all three when the reference
  precedes both, E4): `first(K)` of §4.1 counts shared objects. Where a
  reference follows both, the three disagree and mortar is mold (§4.4,
  §16).

### 3.3 Commons

M1. For a key whose `def` is `Common`:

- `common[K].size` is the maximum `st_size` over the live common mentions
  (all three for size: `resolve/common.test`; mold's first-mention size is
  recorded there as a bug);
- `common[K].align` is the maximum alignment over them, where a common
  mention's alignment is its `st_value` (gABI), which must be a power of
  two, else `malformed-input` ("common symbol alignment is not a power of
  two"); `st_value` 0 is alignment 1 (GNU ld and mold place it at an odd
  address; lld refuses it; WP-022.md). GNU ld and lld take the maximum; mold 2.37.1 does not (E6: an
  alignment of 64 met in the second common gives a 32-aligned object):
  §16;
- `def` is `Common{file, st_index}` of the least common mention by `≺`
  (the identity; the size and alignment are the column's).
- `SHN_X86_64_LCOMMON` (large common, x86-64): accepted as a common
  symbol (`st_value` the alignment), resolved as `SHN_COMMON`; allocated
  in `.lbss` (`spec/x64.md` X-55, `spec/layout.md` L-36a); a `COMMON` and
  an `LCOMMON` of one name give a small common, allocated as `SHN_COMMON`
  is (`.lbss` only when every mention is `LCOMMON`; GNU ld 2.44,
  measured: `spec/x64.md` Q14). M1. On another machine
  the index 0xff02 means nothing and is refused as any other
  `Reserved(n)` (§2).

When a `Strong` definition wins over commons, its own size and alignment
stand. Warnings (GNU ld's, which gfortran users meet; mortar's wording is
§14):

- `common-larger-than-definition` (warning, default on): a live common
  mention is larger than the winning definition (`Strong`, `Weak`, or a
  shared definition by rule 2). GNU ld prints ``size of symbol `x' changed
  from 64 in c64.o to 8 in d8.o`` (E6, E19), only when the definition is
  met after the common. mortar warns in either order: the code compiled
  against the common may write past the definition (E19: a `BLOCK DATA`
  of 8 bytes against a common of 12 is what gfortran produces for a
  common block declared with different lengths).
- `common-alignment` (warning, default on): the winning definition's
  alignment (its section's, capped by the trailing zeros of its offset)
  is smaller than the largest common alignment. GNU ld prints ``alignment
  8 of normal symbol `x' in d8.o is smaller than 16 used by the common
  definition in c64.o``.
- Under `--warn-common` (M1): `multiple-common` for a key with commons in
  two files (GNU: ``common of `x' overriding smaller common``), and
  `common-overridden` when a definition beats a common (GNU: ``definition
  of `x' overriding common``). One warning per key, at the second mention
  by `≺`.

In a `-fno-common` world (GCC 10 and later, Clang 11 and later by default)
C produces no commons; gfortran still emits every named COMMON block as a
common symbol (E19: `C blk_`), so §3.3 and §4.5 are Fortran's rules.

### 3.4 Kinds that constrain the definition

M1 unless marked.

- **TLS.** GNU ld rejects a non-TLS reference to a TLS definition, an
  `STT_NOTYPE` one included (E6: ``TLS definition in t.o section .tdata
  mismatches non-TLS reference in m.o``), and a TLS reference to a non-TLS
  definition (``TLS reference in mt.o mismatches non-TLS definition in
  d1.o``), and a TLS reference against a non-TLS reference when nothing
  defines the key (R7: ``x: TLS reference in mt.o mismatches non-TLS
  reference in mn.o``, both weak undefined; lld and mold link). The rule
  is therefore: **a key whose live mentions (definitions, commons and
  references, in relocatable objects) are not all `STT_TLS` or all
  non-TLS is the error `tls-mismatch`**, naming the definition (or, with
  none, the first mention by `≺`) and the first mention of the other kind
  by `≺`. `STT_NOTYPE` undefined mentions count as non-TLS (GNU ld). A
  common mention is non-TLS. lld reports only a typed (`STT_OBJECT`)
  reference or a common against a TLS definition; mold warns only for an
  `STT_OBJECT` reference and is silent for `STT_NOTYPE` (R7): §16.
  Undefined weak TLS references stay a TLS `Undefined{weak}`
  (`spec/arch.md` gives them their value).
- **IFUNC.** `STT_GNU_IFUNC` is a `Strong` or `Weak` definition like any
  function. Resolution records the type in `def`; `spec/arch.md` decides
  the IRELATIVE and canonical-PLT consequences. An IFUNC definition in a
  shared object (M2) is a shared definition like any other.
- **`STB_GNU_UNIQUE`.** Ranks as `Strong`. Two live unique definitions are
  a duplicate (all three, E6), as in practice they are in one COMDAT group
  and only one copy survives (§6). The output binding is §3.7.
- **Absolute.** An `SHN_ABS` definition ranks by its binding. Two `Strong`
  absolute definitions **with the same value** are not a clash (GNU ld and
  lld, E6; mold reports a duplicate); with different values they are.
- **Section symbols, file symbols.** `STT_SECTION` and `STT_FILE` are local
  by the gABI; a non-local one is `malformed-input`.
- **Discarded COMDAT definitions** are demoted (§6.2).

### 3.5 Visibility

M1. The **visibility of a key** is the most constraining `st_other`
visibility over every live mention in a relocatable object (defined,
common, undefined, demoted), constraint order `STV_INTERNAL` >
`STV_HIDDEN` > `STV_PROTECTED` > `STV_DEFAULT` (gABI: "the most
constraining visibility attribute must be propagated to the resolving
symbol"). Mentions in shared objects and lazy files that stay lazy do not
contribute. `STV_INTERNAL` is treated as `STV_HIDDEN` in every rule below
(no processor supplement of the three targets defines it further).

- A hidden reference makes a default definition hidden: `x` referenced
  `.hidden` in one object and defined default in another is hidden in the
  output (all three, E6).
- A key that is hidden must be defined by a relocatable object or by file
  0. A hidden key whose `def` is `Shared` or `Undefined{weak: false}` is the
  undefined-symbol verdict "hidden" (§5.2, with a surviving reference): GNU ld ``hidden symbol `x'
  isn't defined``, lld ``undefined hidden symbol: x`` (E12). A hidden weak
  undefined key resolves to zero.
- `STV_PROTECTED` (M2): the definition is exported but not preemptible
  (§9.6).
- A key that is hidden and **referenced by a shared object** (M2) is the
  error `non-exported-symbol` (GNU ld: ``hidden symbol `cb' in mh.o is
  referenced by DSO``; lld: ``non-exported symbol 'cb' in 'mh.o' is
  referenced by DSO 'libl.so'``; mold accepts, E20), unless another live
  shared object defines the key with default visibility (then the shared
  object's reference binds there at run time: all three accept, E20).

### 3.6 References and the merged properties

Over the live mentions of a key, as unions (bitwise or):

- `ref_regular`: some live relocatable object has an undefined or demoted
  mention, or a root (§4.2) names the key;
- `ref_strong`: some such reference is not weak (`STB_GLOBAL` undefined; a
  root is strong); `Undefined{weak}` is `!ref_strong`;
- `ref_dynamic` (M2): some needed shared object has an undefined mention;
- `defined_dynamic` (M2): some live shared object defines the key (used
  for export, §9.6).

An undefined mention with `STB_GNU_UNIQUE` binding is malformed
(`malformed-input`).

### 3.7 Binding and visibility in the output

What `spec/synth.md` writes, decided here:

| `def` and visibility | `.symtab` (M1) | `.dynsym` (M2) |
|---|---|---|
| hidden (any def) | `STB_LOCAL`, visibility kept (gABI: "converted to STB_LOCAL"; lld, mold. GNU ld keeps `STB_GLOBAL STV_HIDDEN` in a static executable, E6: §16) | absent |
| `Strong` | `STB_GLOBAL` (`STB_GNU_UNIQUE` kept for a unique definition) | if exported (§9.6), as `.symtab` |
| `Common` | `STB_GLOBAL` `STT_OBJECT`, allocated by layout | as `Strong` |
| `Weak` | `STB_WEAK` | as `.symtab` |
| `Assign`, `Provide` | §8 | §8 |
| `Shared` | not in `.symtab` unless referenced (then `STB_GLOBAL`, or `STB_WEAK` if `!ref_strong`, undefined) | undefined entry with the binding of the references (`STB_WEAK` iff `!ref_strong`), version needed (§9.3) |
| `Undefined{weak}` with a live relocation | not checked (`resolve/weak-undef.test` records the three behaviors) | M2: `STB_WEAK` undefined entry in a dynamic link (§5.4) |

An undefined key that no live relocation uses is absent from both tables
(all three, E15).

### 3.8 Duplicates and `--allow-multiple-definition`

M1. Let `S` be the set of live `Strong` candidates; `clash[K]` is set when `|S| ≥ 2`, unless every
member of `S` is absolute and all have one value (any number of them; an
absolute definition and a section definition always clash, review finding
21). The definition is still the minimum by `≺`.

- Without `--allow-multiple-definition` / `-z muldefs`, a clash is the
  error `duplicate-symbol` (`spec/diag.md` §3.1): one diagnostic per key,
  `>>> defined in <place>` for the winning definition, then for each other
  clashing definition in `≺`, at most 3 places and a note `>>> defined N
  more times`. The places are symbol-anchored section places,
  `a.o:(.data+0x0)` (the defining section and `st_value`), and for an
  absolute definition the file place. Key: phase 30, `unit` = the `FileId`
  of the second definition by `≺`, `seq` = its `st_index` (GNU ld reports
  at the later definition; `spec/model.md` §8 gives the one-definition
  rule the same unit).
- With either option, no diagnostic: the first by `≺` is the definition
  (all three, E6: `-z muldefs d2.o d1.o` takes `d2.o`'s).

Wording (supervisor's policy of 2026-11-14, review finding 22): where GNU
ld has a phrase that build tools match, mortar's message for the same
condition contains it verbatim. The message is therefore
``duplicate symbol: x (multiple definition of `x')``: `spec/diag.md`'s
lld-style head and GNU ld's phrase (§14; requested of `spec/diag.md` in
§18).

A clash among a lazy member's definitions does not exist until the member
is live. Duplicates that arise because position-independent extraction
took a member GNU ld would not have taken are reported like any other;
under `--warn-backrefs` the GNU simulation (§4.6) adds a note.

## 4. Liveness of files

### 4.1 The rule

M1 (shared objects in it: M2). Liveness of files is **reachability over a
graph that is a function of the inputs**: no step looks at whether a
definer is live, so there is no "settled" test and nothing depends on the
order in which the closure is computed (supervisor's ruling A of
2026-11-14, after review findings 1, 4 and 5).

For each key `K`, two targets, computed once from the per-file symbol
tables (I-file, §13):

- **`first(K)`**: the file with the lowest `FileId`, live or lazy, that has
  a **defined or common** mention of `K` of any binding (`STB_GLOBAL`,
  `STB_WEAK`, `STB_GNU_UNIQUE`), counting a shared object's visible
  definitions (§9.1, M2) and file 0's `Assign` candidates (M4) but not its
  `Provide` candidates (§8). Commons count: a strong reference to a key
  whose lowest definer is a lazy common extracts that member (all three,
  R5: without it the reference is left undefined).
- **`cfirst(K)`**, the target of a common: the file with the lowest
  `FileId` among (a) files that are not lazy (objects named on the line,
  `--whole-archive` members, shared objects, file 0) with a **non-weak,
  non-common** definition of `K`, and (b) lazy files with a **non-weak data
  definition** of `K` (§2: not `STT_FUNC`, not `STT_GNU_IFUNC`; not
  common). A weak definition does not stop it (GNU ld: a common beats a
  weak definition, so its archive scan still sees a common and extracts
  the data definition, R6), nor does a lazy function or weak definition
  (§4.5).

The edges, from a file `F` to a lazy file `M`:

1. `F` has a **non-weak undefined** mention of `K` (after `--wrap`
   redirection, §7.1) and `M = first(K)`;
2. `F` has a **common** mention of `K` and `M = cfirst(K)`;
3. a root option names `K` (§4.2) and `M = first(K)`: an edge from file 0.

`file_live` is the set of files reachable from the roots of §4.2 (the
lazy files reached become live; every other file is live from the start
or never). For an extracted member it records the pulling reference: the
least `(FileId, st_index)` by `≺` among the live mentions with an edge to
it (a root: `FileId` 0 and the option's argument index).

**After the closure**, `def[K]` is the minimum of §3.2 over the candidates
of **live** files only; with none, `def[K]` is `Undefined{weak}`, `weak`
being true when every live reference is weak (§3.6). A lazy file that is
not reached contributes nothing (finding 5).

Consequences, each measured (R-numbers and E-numbers in the landed
record):

- **A definition before the archive keeps the member out** (all three, E3):
  `first(K)` is that file, not the member. This holds for a weak
  definition too (E3).
- **A definition after the archive does not** (finding 1): `t1m.o
  libt1k.a t1w.o`, the member defining `K` strongly and `t1w.o` weakly:
  `first(K)` is the member, which is extracted and wins (GNU ld and lld
  run=2, mortar 2; mold 1, R1). With a strong definition in `t1s.o` it is
  a duplicate (GNU ld, lld, mortar; mold links silently, R1).
- **Weak references never demand** (gABI: "the link editor does not
  extract archive members to resolve undefined weak symbols"; all three,
  `resolve/weak-undef.test`). A weak reference followed by a strong one
  from a later file demands from the strong one (R2: GNU ld fails when the
  strong reference follows the archive, a back-reference; lld, mold and
  mortar link).
- **Lazy commons and lazy weak definitions are extracted** for a strong
  reference when they are `first(K)` (all three, E3, R5). The member with
  the lowest `FileId` is taken even if it is weak and a later member is
  strong (all three: R3, `mk.o libKW.a libKS.a` gives the weak one's 1
  under GNU ld and lld; mold 2).
- **`--whole-archive` members are not lazy**: they block like named objects
  (R4: `mk.o libKS.a --whole-archive libKW.a`: the earlier lazy member is
  extracted and wins, GNU ld and lld 2; `--whole-archive libKW.a` first:
  none extracted, all three 1).
- **Among several lazy definers the lowest `FileId` wins**
  (`resolve/archive-order.test`: `-lone -ltwo` gives `one`, `-ltwo -lone`
  gives `two`, all three).
- **`--wrap` targets** are not roots: with `--wrap=f`, a member defining
  only `f` is not extracted unless `__real_f` is referenced (all three,
  E13).
- **COMDAT does not feed back.** Extraction treats a definition in a group
  section as a definition whichever copy is kept: the keeper is chosen
  after the closure (§6.1). A key defined only in copies that end up
  dropped is the error of §6.3.
- **Extraction reads the members' own symbol tables, never the archive's
  index** (finding 13): `spec/archive.md` reads every member anyway, a
  stale index cannot mislead mortar, and an archive without an index
  links (lld and mold link `ar rcS` archives; GNU ld refuses them with
  ``archive has no index; run ranlib to add one``, R8: §16).

**Rounds are an implementation, not the semantics.** An implementation
computes the closure breadth-first: round 0 is the roots; round `k+1` is
the set of lazy targets of the edges out of the files of round `k` not yet
live. Each round is a parallel map over the files of the round and a
union of target bits; the number of rounds is the depth of the graph.
Any order of visiting gives the same set (§12.2); tests compare a
sequential worklist with the parallel rounds.

### 4.2 Roots

M1 unless marked. The files live from the start:

- file 0;
- every relocatable object named on the command line outside a
  `--start-lib` region (M4) and outside an archive;
- every member of an archive under `--whole-archive` (its positional state,
  `spec/cli.md` §3.3);
- every shared object (M2): live for resolution whatever `--as-needed`
  says; whether it is *needed* is §9.5.

and the keys rooted by options, each an edge from file 0 (a strong
reference):

| option | key | M |
|---|---|---|
| none of `-e`, `ENTRY`, `-shared`, `-r` | the default entry `_start` (finding 14: all three extract a `_start` member from an archive with no `-e`, R9) | M1 |
| `-e SYM` / `ENTRY(SYM)` | `SYM` (all three extract it, E13); undefined afterwards: §5.5 | M1 |
| `-u SYM` | `SYM` (R4b: all three extract) | M1 |
| `--require-defined=SYM` | `SYM`; if still undefined after the closure: `required-symbol` (error; GNU: ``required symbol `x' not defined``, mold: `--require-defined: undefined symbol: x`; lld 22 has no such option, E13) | M1 |
| `--defsym A=EXPR` | the symbols `EXPR` names, wherever the option is on the line (finding 17: GNU ld extracts them only when the option precedes the archive, ``undefined symbol `k' referenced in expression`` otherwise; lld and mold in any position, R10: §16) | M4 |
| `--export-dynamic-symbol=GLOB` | each key matching the glob that some file mentions (lld; not verified for GNU ld) | M2 |
| a script's `EXTERN(SYM)` | `SYM` | M4 |

### 4.3 What GNU ld users see

GNU ld reads the command line once: an archive is searched when it is
reached, for the symbols undefined at that point, again until a pass adds
no member; `--start-group` repeats the archives of the group until a pass
adds nothing ("Command-line Options", `-l` and `--start-group`). lld and
mold remember every archive and extract on any later reference. mortar's
rule is position-independent like theirs (the supervisor's decision of
2026-11-14): a closure over a graph that is a function of the inputs is
parallel and order-independent by construction (§12) and is recomputable
from per-file summaries (§13); GNU's rule is a sequential walk with state.

**The claim.** Call a demand of `K` (an edge of §4.1 from a live file `F`)
a **back-reference** when GNU's walk has already passed `first(K)` (or
`cfirst(K)`) for the last time before it loads `F`: `first(K)` lies in an
archive before `F`'s position outside `F`'s archive and `--start-group`
region, or it lies in `F`'s own archive or group and GNU's current pass
meets another lazy definer of `K` after `F` first. **Without
back-references, mortar's live set, every `def`, and every COMDAT keeper
equal GNU ld's.** Proof sketch:

- *Every member GNU loads, mortar extracts.* GNU loads member `M` for key
  `K` when its walk reaches `M` with `K` undefined (or common, with `M`
  holding a non-weak data definition). No file before `M` that GNU loaded
  defines `K` (else `K` would not be undefined); no lazy file before `M`
  defines it either, or GNU would have passed it with `K` already
  demanded (it loads the first definer it meets), which is a
  back-reference. So `M = first(K)` (or `cfirst(K)`), and the demanding
  file is live in mortar by induction over GNU's load order: an edge.
- *Every member mortar extracts, GNU loads.* `M = first(K)` with a live
  demanding file `F`. Without a back-reference GNU loads `F` before it
  last considers `M`; then `K` is still undefined there, since no file
  before `M` defines it, and GNU loads `M`.
- *Definitions.* Over equal live sets, GNU's resolution among live files
  is §3.2's order (strong, then common, then weak, the first by
  `FileId`), measured case by case in E3, E6, E12.
- *COMDAT.* Without back-references GNU loads files in increasing `FileId`
  order (a later pass of a rescan loads a lower member only for a
  reference from a later member, a back-reference), so its first-loaded
  copy is the lowest live `FileId` (§6.1).

Back-references **inside one archive** are common in system archives
(members are not ordered by dependency) and are mostly harmless: GNU's
rescan reaches `first(K)` again and loads it, unless another definer of
`K` lies after the referring member. **Re-run on the reviewer's sixteen
system link lines** (gcc, clang, g++, clang++, gfortran, `-static-pie`,
`-lm -lpthread -ldl`, on AArch64, x86-64 and i386; landed record "Re-run
under ruling A"): mortar's member set equals GNU ld's on all sixteen
(381 to 653 members), as does the simulation of §4.6.

**Where mortar differs from GNU ld**, each requiring a back-reference
except the last, which is a COMDAT case:

1. **A different member** (silent, the link succeeds): GNU passed
   `first(K)` and takes a later definer. E1: `main.o libA.a libB.a libC.a`
   (`libB` needs `pick`, `libA` and `libC` define it): GNU ld runs `libC`'s
   (2), lld, mold, mortar `libA`'s (1); inside one archive `[a.o b.o c.o]`
   GNU ld takes `c.o` (its pass continues after it adds `b.o`); inside
   `--start-group` the same; and `m2.o libA.a libCB.a` with `libCB = [c.o
   b.o]` (finding 3's t2: GNU ld 3, mortar 1).
2. **A duplicate GNU never saw**: the member mortar extracts defines a
   symbol another live file defines (E1: `dup`; E2: `K`, where GNU ld links
   with 120 and lld and mold also report the duplicate: E2 is a
   back-reference from `n.o` to `libX.a`'s `K`).
3. **An undefined reference GNU never saw**: the member mortar extracts
   needs a symbol nothing defines.
4. **A different COMDAT copy, or a symbol lost with it** (finding 2,
   ruling C): GNU keeps the first loaded copy, mortar the lowest live
   `FileId` (§6.1). With a back-reference in the archive the two differ:
   `m10.o libG.a`, `libG = [gl.o: group g{g}, Z; gh.o: group g{g, hx}, Y
   → Z]`: GNU loads `gh.o` (for `hx`, `Y`) before `gl.o` (for `Z`) and keeps
   `gh.o`'s copy (GNU ld and lld run 140); mortar keeps `gl.o`'s, which
   does not define `hx`: the error `comdat-mismatch` (§6.3; mold reports
   `undefined symbol: hx`). Copies that differ in content are observable
   only as an ODR violation (§6.7); copies that differ in their symbol
   sets are this error. On the system lines the keepers differ from GNU
   ld's for 35 to 45 of 60 to 67 signatures per link, with byte-identical
   sampled copies and identical program output (the reviewer's table, in
   the landed record).

And the converse: links GNU ld rejects because of a back-reference link
with mortar (`resolve/archive-backref.test`; R2). `--start-group` and
`--end-group` change nothing in the closure; they are accepted and kept
in `Inputs` for the simulation of §4.6.

### 4.4 Shared objects and archives (M2)

All twelve cases of {shared object before the archive, after it} × {strong,
weak reference} × {`--as-needed` or not}, reference first on the line,
measured (E4; `libs.so` defines `f` returning 1, `liba.a` returning 2):

| line | strong ref | weak ref | weak ref, `--as-needed` |
|---|---|---|---|
| `m.o libs.so liba.a` | `libs.so` (all) | `libs.so` (all) | not needed, `f` = 0 (GNU ld, lld); `libs.so` needed (mold) |
| `m.o liba.a libs.so` | `liba.a` (all); with `--as-needed`, `libs.so` not needed (all) | `libs.so` (all): no extraction for a weak reference | not needed, `f` = 0 (GNU ld, lld); mold's output crashes |

mortar: as the table's majority in every cell (`first(K)` counts shared
objects, §4.1; a weak reference does not make a shared object needed,
§9.5). Re-run under ruling A (R11): the same four lines with a strong
reference give `libs.so`, `liba.a` (all three), and a weak reference
followed by a strong one after both (`mkw.o libs.so libKS.a uk.o`) gives
`libs.so` (all three; mortar: `first(K)` is the shared object).

With the reference **after** both (E4, R11), the three disagree:

| line | GNU ld | lld | mold | mortar |
|---|---|---|---|---|
| `liba.a libs.so m.o` | `libs.so` | `libs.so` | `liba.a` | `liba.a` (`first`) |
| `liba.a m.o libs.so` | `libs.so` (archive already passed) | `liba.a` | `liba.a` | `liba.a` |
| `--as-needed libs.so m.o` | undefined (GNU drops an as-needed library no earlier object needs, E4) | `libs.so` | `libs.so` | `libs.so` |

A shared object's undefined mentions are edges like an object's (an
archive member defining a symbol a shared object needs is extracted and
exported: all three, E4).

### 4.5 Commons and archives

M1. A live common mention of `K` is an edge to `cfirst(K)` (§4.1) when that
is a lazy file: the lowest lazy **non-weak data definition** of `K`, unless
a file that is not lazy and has a lower `FileId` defines `K` non-weakly
(GNU ld and mold, E3: a member defining `int v = 9;` replaces the common
`int v;`). lld does this only under `--fortran-common` (its `--help` calls
that the default; measured, lld 22's default does not extract, E3). The
rule is what makes a gfortran `BLOCK DATA` unit in an archive initialize
its COMMON block (E19: GNU ld and mold run the program correctly; lld
without `--fortran-common` leaves the block zero and the program fails).
Which lazy definitions are targets, after GNU ld (E3, R6):

| the lazy mention | target of a common |
|---|---|
| `STT_OBJECT` or `STT_NOTYPE`, in any section (data, rodata, text) | yes |
| absolute | yes |
| `STT_TLS` | yes, then `tls-mismatch` (GNU ld's error; §3.4) |
| `STT_FUNC`, `STT_GNU_IFUNC` | no (mold extracts and its program reads code as data, E3, R6) |
| weak | no |
| common | no (it is a target of a strong reference, §4.1, not of a common) |

What stops it: a non-weak definition in a non-lazy file with a lower
`FileId` (R6: `fv.o mc.o libLV.a`, `fv.o` defining `v` as a function: no
extraction in all three). What does not: a weak definition anywhere (R6:
`wv.o mc.o libLV.a` and `mc.o libLV.a wv.o`, GNU ld and mold extract,
GNU ld runs 9; lld keeps the common, 0). A key both strongly referenced
and common has both edges (to `first` and to `cfirst`); when they differ
(`first` a lazy function or common, `cfirst` a later data definition),
both members are extracted, as GNU ld does when the reference precedes
the archive (R5: `mv.o libLC.a libLV.a`, GNU ld extracts both and runs
9; lld only the common, 0; mold only the data definition, 9).

`--fortran-common` and `--no-fortran-common` (lld's options) are accepted:
the first is mortar's behavior (`supported`), the second is refused
(`spec/cli.md` §4.2; there is no use for it that is correct).

### 4.6 `--warn-backrefs`: simulating GNU ld

M1. Opt-in (ruling B, after finding 3: a predicate over `FileId` order is
not GNU's choice and gave both false and missing warnings). Under
`--warn-backrefs` mortar **simulates GNU ld's archive walk** after the
closure and reports, per key, where GNU ld would have failed or chosen
another file. Without the option nothing below runs: the default link
pays nothing.

The simulation walks the inputs in command-line order with a state per
key: `none`, `undefined` (a non-weak reference seen), `weak-undefined`,
`common`, `defined-weak`, `defined`.

- **Adding a file** (a named object, a `--whole-archive` member, a shared
  object, a member being loaded) applies its mentions in symbol-table
  order: a non-weak definition makes the key `defined`, unless it is
  already; a weak definition makes `none`, `undefined` or
  `weak-undefined` into `defined-weak`; a common makes anything below
  `common` (including `defined-weak`) into `common`; a non-weak undefined
  mention makes `none` or `weak-undefined` into `undefined`; a weak one
  makes `none` into `weak-undefined`. A shared object's definitions are
  `defined`.
- **An archive** (not under `--whole-archive`) is scanned in **passes**:
  each pass visits the members not yet loaded in member order and loads a
  member when one of its mentions is a definition or common of a key
  that is `undefined`, or a non-weak data definition of a key that is
  `common`; loading applies its mentions at once, so a later member of
  the same pass sees them. Passes repeat until one loads nothing.
- **A `--start-group` region**: its objects are added in order with its
  archives scanned as above; then the region's archives are scanned again,
  in order, until a full round over them loads nothing.
- Undefined references in shared objects are `undefined` mentions;
  `--as-needed` is ignored for loading (§4.4's last row is reported
  separately below).

The members' own symbol tables are read, as by the closure (§4.1), not
the archive index. The simulation needs from `Inputs`, per file, its
command-line position, its archive and member order, and its
`--start-group` region (`spec/model.md` §3.2 has the positional state but
not the region: finding 15, requested in §18 and being recorded by
`spec/input.md`).

It reports, for each key `K` whose `def` comes from an extracted member and
is not in a COMDAT group (group keys are §6's):

- **`archive-backref`** (warning): the simulation ends with `K`
  `undefined`: GNU ld would fail with ``undefined reference to `K'``. The
  message names `K`, the member mortar used and the first referring
  file. lld's own `--warn-backrefs` words this ``backward reference
  detected: pick in libB.a(b.o) refers to libA.a(a.o)``; it is silent when
  a later archive also defines the key (E1).
- **`archive-choice`** (warning): the simulation's definer of `K` is
  another file: names both. Not when the two files have the same file
  identity apart from the mention ordinal (`spec/model.md` §7.1: the same
  found path, member name and member ordinal), as when one archive is
  mentioned twice (`-lgcc ... -lc ... -lgcc`): they are the same
  definition.
- Under the same option, as notes on an existing `duplicate-symbol` or
  `undefined-symbol`: the member that caused it was loaded by mortar but
  not by the simulation.
- An as-needed shared object that mortar finds needed and whose every
  reference follows it (§4.4: GNU ld drops it): `archive-backref` naming
  the library.

**Cost.** One sequential pass per archive pass over the members' mention
lists; with the members' summaries already in memory (§13) it is a
fraction of resolution's reading cost, but it is not parallel. Checked
against GNU ld (landed record, "Ruling B"): the simulated member sets
equal `ld.bfd -t` on the sixteen system lines and on E1, E2, t1, t2, t2b,
t10 and the `--start-group` variants; it fires `archive-choice` for t2
(GNU 3, mortar 1) and nothing for t2b (all three 1), finding 3's two
cases.

### 4.7 What resolution hands to `--gc-sections`

The roots of liveness (`spec/live.md`, M2): the sections defining `def` of
every key rooted in §4.2 (`-u` included: GNU ld keeps a `-u` symbol's
section); the entry; every key exported to `.dynsym` (§9.6); every section
of `SHF_GNU_RETAIN`; `KEEP` (assignment's). `def` itself is the same with
and without `--gc-sections`: extraction is decided before liveness, so a
member whose sections are later collected still settled its keys (all
three; not separately verified).

### 4.8 `--start-lib` (M4)

The objects of a `--start-lib` … `--end-lib` region are lazy, each a member
of an archive of its own: exactly the archive rules apply (lld and mold;
GNU ld 2.44 has no such option, E13).

## 5. Undefined symbols

### 5.1 Who decides

Ruling D (2026-11-14, review findings 6, 8, 12): **resolution only computes
`def[K] = Undefined{weak}`**. Whether that is an error is decided **per
surviving reference** by the relocation scan (`spec/arch.md`, phase 70):
a reference survives when the architecture's sequence recognition leaves a
relocation that still needs the symbol's value or a slot for it. Three
consequences:

- **Relaxed-away references are not errors.** A TLS general-dynamic
  sequence relaxed to local-exec in a static link no longer calls
  `__tls_get_addr`; the call's relocation is consumed by the recognizer
  and does not survive. `libstdc++.a(eh_globals.o)` references
  `__tls_get_addr` (i386: `___tls_get_addr`) strongly and no `libc.a`
  member defines it: `g++ -static` links on x86-64 and i386 with all
  three references (finding 6; `x86_64-linux-gnu-objdump -dr` of the
  member shows `R_X86_64_PLT32 __tls_get_addr-0x4`, and
  `x86_64-linux-gnu-nm libc.a` has no definition: R15). mortar must link
  them too.
- **An unused undefined key is not an error**, whatever its visibility:
  `.globl zz` with no use links (all three, E15), and so does `.hidden zz`
  with no use (lld and mold; GNU ld reports ``hidden symbol `zz' isn't
  defined`` even under `--unresolved-symbols=ignore-all`, R14: §16).
- **References from sections `--gc-sections` drops are not errors** (all
  three, E15): the scan reads only live sections.

Layout symbols whose existence depends on output sections (§8.1:
`__start_<sec>`, `__stop_<sec>`, `_TLS_MODULE_BASE_`,
`__GNU_EH_FRAME_HDR`) are **conditional `Provide` candidates**
(finding 12): resolution records `def = Internal{Provide(name)}` with the
condition; the first layout stage (`Assignment`, `spec/model.md` §3.4),
which knows the output sections, confirms each or leaves it undefined
(`def` reverts to `Undefined{weak}` with the references' weakness), before
the scan runs. A conditional candidate never extracts a member and never
beats a file's definition (rank 5). This hand-over is requested of
`spec/model.md` §3.4 (§18).

### 5.2 The verdict for a surviving reference

From the output kind and the options (`spec/cli.md` §3.9, §3.7):

| situation | verdict |
|---|---|
| `Undefined{weak: true}` | resolves to zero; in a dynamic link (M2) it is exported as a weak undefined dynamic symbol where `spec/arch.md` says (a GOT entry or PLT) |
| `Undefined{weak: false}` in an executable | error, or warning under `--warn-unresolved-symbols` |
| … under `--unresolved-symbols=ignore-all` or `=ignore-in-object-files` | nothing (the value is zero, as weak) |
| … `--unresolved-symbols=ignore-in-shared-libs` | still the error for references from objects |
| `--ignore-unresolved-symbol=SYM` | nothing for `SYM` (M1) |
| `Undefined{weak: false}` in a shared object (M4) | nothing, unless `-z defs` / `--no-undefined`: then the error |
| a hidden key (§3.5) not defined in an object or file 0, with a surviving reference | error, whatever the options above say (it cannot be satisfied at run time); weak hidden: zero |
| undefined references **of shared objects** (M2) | an executable checks them by default (`--no-allow-shlib-undefined`; GNU ld and lld, E20; mold does not check, §16): a needed shared object's non-weak undefined mention of a key with `def = Undefined` is the error, unless `--allow-shlib-undefined` or the shared object has a `DT_NEEDED` that is not among the inputs (then its dependencies are unknown and it is not checked: lld's rule; not verified for GNU ld). These have no relocation in the link; resolution emits them (phase 30, unit = the shared object's `FileId`, seq = the `.dynsym` index) |

`--warn-unresolved-symbols` makes the error a warning; `--error-unresolved-
symbols` undoes it; the last one wins.

### 5.3 The diagnostic

`undefined-symbol` (`spec/diag.md` §3.1). Its message contains GNU ld's
phrase verbatim (the policy of §3.8): ``undefined symbol: f (undefined
reference to `f')``, requested of `spec/diag.md` (§18).

- **One diagnostic per key**, grouping its surviving references; its key
  is that of the least surviving reference by `(FileId, section index,
  relocation index)`: phase 70, `unit` that `FileId`, `seq` `section index
  × 2^32 + relocation index` (`spec/model.md` §8). The order of
  diagnostics is therefore the order of first references, independent of
  threads, as GNU ld's is; mold's is not (E15, `spec/diag.md` §8.1).
- `>>> referenced by <file>:(<section>+0x<offset>)` for the first **3**
  surviving references in that order, then `>>> referenced N more times`
  (a note) if there are more: lld's and mold's shape and count (E15).
  GNU ld prints every reference; with the error limit (`spec/diag.md`
  §6.4) counted per key, a link with thousands of references to one
  symbol does not drown.
- The name goes through the demangling hook (`spec/diag.md` §5.4): C++
  names are shown demangled with the `>>> mangled name:` line.
- For a hidden key: a note `>>> the symbol has hidden visibility`.
- For a reference from a shared object (§5.2): `>>> referenced by
  <shared object>` (the file place; lld's ``(disallowed by
  --no-allow-shlib-undefined)`` becomes the note `>>> a shared object's
  undefined symbol; --allow-shlib-undefined accepts it`).
- Under `--warn-backrefs`, for a member the simulation of §4.6 does not
  load: the note of §4.6.
- M1 prints section places. The enclosing function (lld's `m.o:(a1)`) and
  the source line (`m.c:3`, from DWARF) are `spec/diag.md` §9's planned
  place forms; mortar adds them when the DWARF line reader exists,
  as extra context lines, never instead of the section place.
- Hints, proposed for a later WP and not required now: lld's `>>> did you
  mean: f` (a key at edit distance 1, or the same name with different
  case), and `>>> did you mean to declare f(int) as extern "C"?` when the
  demangled base name of a C++ key equals a defined C key (and the
  converse). They are notes; they cost a pass over the keys only when
  there is an error.

Requested of `spec/arch.md` (§18): the scan emits `undefined-symbol` and
`discarded-section-reference` (§6.3) and `comdat-mismatch` with these keys,
after sequence recognition, from `Resolution`'s `def` and the dropped
sections.

### 5.4 Weak undefined values

Zero in a static executable, with no dynamic symbol. In a PIE or a dynamic
link (M2): `spec/arch.md` (GOT entry relocated against a weak undefined
dynamic symbol, or zero when `-z nodynamic-undefined-weak` / static-pie).
Resolution only records `Undefined{weak: true}` and `ref_dynamic`.

### 5.5 The entry symbol

`-e SYM` (or `ENTRY`, or the default `_start`) is a root (§4.2). If
`SYM` is undefined after the closure: GNU ld warns ``cannot find entry
symbol nosuch; defaulting to 0000000000400078`` (the start of `.text`),
lld ``cannot find entry symbol nosuch; not setting start address``, mold
``entry symbol is not defined: nosuch`` (E13). mortar warns
`entry-not-found` (once: resolve's, at the key of the table in §14;
layout does not repeat it) and `e_entry = 0` (`spec/layout.md` L-103,
lld's and mold's value). If `SYM` parses as a number and no file
mentions it, it is that address (GNU ld manual, `-e`). A shared object
(M4) without the option has no entry and no warning.

## 6. COMDAT groups

### 6.1 The keeper

M1. A group is an `SHT_GROUP` section with `GRP_COMDAT` in its flag word
(`elf.md` §3.7). Its **signature** is the name of the symbol `sh_info`
names, except that for an `STT_SECTION` symbol (whose name is empty) it is
the **name of the section** that symbol refers to (GNU ld and mold, E18:
two groups whose signatures are the section symbols of `.foo` and `.bar`
are kept both; lld takes the empty name for both and drops `.bar`, a bug:
§16). This settles the question WP-004 left. `ComdatId` interns signatures
so computed (§18: `spec/model.md` §2.3, `spec/input.md`).

**`comdat[G]` = the least `FileId` among the live files that have a group
with signature `G`.** If that file has several groups with signature `G`,
the first by section index is kept and the others are dropped like another
file's copies (not verified with any reference; no tool tested writes
it).

GNU ld and lld keep the **first loaded** copy; mold the lowest live file
(E10). They differ exactly when the file with the lowest `FileId` among
those with the group became live after one with a higher `FileId`: a
member extracted by a back-reference (§4.3), or a member of the same
archive extracted on a later scan, or a member of an earlier archive in a
`--start-group` region. E10 measured it with two copies of an inline
function returning 1 and 2: `main.o --start-group libm.a liblate.a
--end-group` gives 22 under GNU ld and lld (the later archive's copy) and
11 under mold and mortar; inside one archive `[m.o, l.o]` the same.
Observable in two ways (finding 2): when the copies' **contents** differ,
an ODR violation (different source, or the same source compiled
differently), the program behaves differently; when their **symbol sets**
differ, a symbol defined only in the copy GNU keeps is lost and a
reference to it is the error `comdat-mismatch` (§6.3; t10 of §4.3 case 4,
where GNU ld and lld link). Addresses differ in any case. On the `g++
-static` (three architectures) and `clang++ -static` lines GNU ld keeps
another file's copy than mortar for 35 to 45 of the 60 to 67 signatures
present in two or more live files, lld for 3 to 10, mold never; with user
templates in two archives, GNU ld 16 and lld 14 of 17 (the reviewer's
comparison, in the landed record); sampled copies were byte-identical and
the programs' outputs equal. The decision is
**kept** (ruling C): the keeper must be a
minimum over a set for the parallel resolution of §12 and for the
incremental link (§13: the keeper of `G` depends on the set of live files
with `G`, not on the history of the walk); GNU's "first loaded" depends on
the order of extraction, which mortar does not have. `mortar_odr`
(`spec/model.md` §8, M6) is where differing copies are reported; until
then, this spec records the gap (§6.7).

### 6.2 The other copies

Every section of a non-kept copy is `Dropped(Comdat{keeper})`, members
listed in the group and nothing else (a section not in the group stays,
even with the same name). Its symbols:

- a non-local mention defined in a dropped section is **demoted** (§2): it
  takes part in resolution as an undefined mention of its key with its
  binding, so it binds to the keeper's definition (all three);
- a local symbol of a dropped section is gone; a relocation against it, or
  against the section's `STT_SECTION` symbol, from a section that is kept
  is §6.3.

### 6.3 References into a dropped copy

A relocation in a kept section of file `F` whose symbol is (a) a local
symbol or section symbol of a dropped section of `F`, or (b) a demoted
mention whose key nothing live defines (`def` is `Undefined`, so the
keeper's copy lacks it): what each reference does (E9, R13):

| referring section | GNU ld | lld | mold | mortar |
|---|---|---|---|---|
| non-alloc, **GNU ld's debug names**: prefix `.debug` (any continuation: `.debug`, `.debug_info`, `.debugx`), `.zdebug`, `.line`, `.stab` (`.stab`, `.stabstr`, `.stab.foo`), `.gnu.linkonce.wi.`, `.gnu.debuglto_.debug_`, and exactly `.gnu_debuglink` | accepted (tombstone) | accepted | accepted | accepted: `spec/image.md` writes the tombstone |
| `.eh_frame` | the FDE is dropped (all) | | | the FDE is dead (`spec/model.md` §3.6 rule) |
| exactly `.gcc_except_table` (alloc or not; not in the group) | accepted, the field is 0 | alloc: **error**; non-alloc: accepted | accepted, the field is the addend | accepted, the field is 0 (GNU ld): the table is reached only from the dead FDE |
| any other non-alloc section (`.comment`, `.comment.x`, `.note.x`, `.gcc_except_table.foo`, `.gnu_debugaltlink`, `.debu`, `.lin`, `.gnu.debuglto_.debug`, `.gnu.linkonce.wix`) | error | accepted | accepted | error `discarded-section-reference` (GNU ld, finding 11) |
| an alloc section whose name is a debug name (`.debug_x`, `"a"`) | error | error | accepted | error |
| any other alloc section (`.text`, `.data`, `.rodata`, `.stapsdt.*`, `.gcc_except_table.foo`, …), case (a) | error ``` `lg' referenced in section `.text' of b.o: defined in discarded section `.text.g[g]' of b.o``` | error `relocation refers to a symbol in a discarded section: lg` | accepted silently, or crashes (E9: mold 2.37.1 segfaults after its own error) | error `discarded-section-reference` |
| case (b), any kept referring section | ``undefined reference to `hg'`` | `relocation refers to a symbol in a discarded section: hg` | `hg refers to a discarded COMDAT section probably due to an ODR violation` | error **`comdat-mismatch`** (below) |

**`comdat-mismatch`** (ruling C): a surviving reference to a key defined
only in dropped copies of group `G` whose kept copy does not define it.
The diagnostic names the symbol, the group signature, the keeper's file,
and the dropped copy's file that defines the symbol: ``symbol hx is
defined only in a discarded copy of COMDAT group g``, `>>> kept copy:
libG.a(gl.o)`, `>>> discarded copy defining it: libG.a(gh.o)`,
`>>> referenced by m10.o:(.text+0x4)`. It is also an ODR signal: two copies
of one group that define different symbols come from different sources
or different compilations, and the note says so (`>>> the copies of g
differ: a one-definition-rule violation, or objects compiled with
different options`). Key: phase 70 at the least surviving reference, as
§5.3. `mortar_odr` (M6) may later check symbol sets of all copies, used
or not (§6.7).

Resolution marks the dropped sections and the demoted mentions; the scan
(phase 70) finds the relocations and emits `discarded-section-reference`
one diagnostic per (symbol, referring section) and `comdat-mismatch` one
per symbol, each with the first 3 places, as §5.3. GNU ld also accepts
case (a) from an alloc section when the kept copy's corresponding section
has the same size (it redirects to it; measured only for a mix of sizes,
E9): not followed, the reference is to bytes of a copy that does not
exist.

### 6.4 Groups that are not COMDAT

A group whose flag word lacks `GRP_COMDAT` (flag 0) groups sections for
`-r` and `--gc-sections` only: never deduplicated (all three, E11). Unknown
flag bits (`elf.md` §3.7 reports them) are refused: `Refused{Feature,
"group flags 0x.."}`.

### 6.5 Members

A group member that is a relocation section applies to a member; a
relocation section that applies to a dropped section is dropped with it.
A section that is a member of two groups is `malformed-input`. A member
that is `SHF_LINK_ORDER` to a section of another group, or a non-member
`SHF_LINK_ORDER` section whose linked section is dropped, is dropped too
(GNU ld and lld; not verified here).

### 6.6 `.gnu.linkonce.*`

The pre-group form of vague linkage: sections named `.gnu.linkonce.<k>.<n>`
deduplicated by name. GNU ld deduplicates them; lld and mold do not and
report duplicate symbols (E11). No object in this machine's system
libraries and start files for the three targets has one (E11). **Refused
by name** until an input needs it; the refusal costs a prefix test per
section name.

### 6.7 Content (for the one-definition rule)

Nothing checks that the copies of a group agree: size, content, the set of
symbols they define. `mortar_odr` (M6) may compare the kept copy with each
dropped one (sizes and symbol sets cheaply; content by hash when kiln and
hod supply one). Resolution keeps, per group, the list of live files with
the group (`(ComdatId, FileId)` pairs in order) for that purpose.

## 7. Redirections

### 7.1 `--wrap=SYM` (M4)

GNU ld's manual: "Any undefined reference to SYM will be resolved to
`__wrap_SYM`. Any undefined reference to `__real_SYM` will be resolved to
SYM." Exactly that, and no more:

- `redirect[SYM] = __wrap_SYM` and `redirect[__real_SYM] = SYM`, **for
  undefined mentions only**. A reference in the file that defines `SYM`
  goes through the defined mention and is not wrapped: GNU ld and mold
  (E13: `g` calls `f` in `f`'s own object, and the call is not wrapped);
  lld wraps it as well (E13, exit 87 instead of 111): §16.
- Extraction follows redirection: an undefined `SYM` demands
  `__wrap_SYM` (a member defining it is extracted, E13), an undefined
  `__real_SYM` demands `SYM`. `SYM` itself is not a root (E13).
- An undefined `__wrap_SYM` is reported as such (all three, E13).
- Several `--wrap` options for one symbol are one; `--wrap` of
  `__wrap_X` or `__real_X` follows the same rule textually (not verified).
- LTO is refused (§1.2), so lld's interaction of `--wrap` with bitcode
  does not arise.

`redirect` is a column, not a rename of keys, because the key of a
mention is I-file (`spec/model.md` §3.2: interning before resolution, and
reused unchanged by an incremental link), the defining mentions of `SYM`
keep their key, and the redirection depends on the mention's kind. The
consumers (scan, fill, the verifier) resolve an undefined mention of key
`K` to `def[redirect[K]]`, a defined one to itself (§18 asks
`spec/model.md` §3.3 to say "undefined mentions").

### 7.2 `--defsym=SYM=EXPR` (M4)

A file-0 `Assign` candidate for `SYM` (rank 0, §3.1): it beats every
object definition of `SYM` without a duplicate error (all three, E13).
Expression forms:

| form | meaning | references |
|---|---|---|
| a number (`0x1234`, decimal, octal as GNU ld reads it) | absolute `SYM` | all three (E13) |
| `B` (a symbol) | `SYM` is an alias of `B`: `redirect[SYM] = B` for **every** mention of `SYM` (defined ones too: E13, the object's own `f` is replaced), and `def[SYM]` is `Internal{Alias(B)}` so that `.symtab` has `SYM` with `B`'s value, section and type | all three |
| `B + N`, `B - N` | `SYM` = `B`'s address plus `N`, in `B`'s section | lld (section-relative, `T`); GNU ld makes it absolute (`A`, the same value in a non-PIE link); mold refuses (E13). mortar: section-relative, which is also right in a PIE |
| anything else (`ALIGN`, `.`, two symbols, a script function) | `Refused{Feature, "--defsym expression"}`, named with the text | until `spec/script.md`'s expressions |

If `B` is a shared object's definition (M4): an alias redirects to it
(references get `B`'s GOT/PLT); `B + N` is refused
(`Refused{Feature, "--defsym offset from a shared object's symbol"}`).

### 7.3 Aliases

Two keys defined at the same place (`environ` and `__environ` in glibc,
one weak) are two rows with independent definitions; resolution does
not relate them. For copy relocations of a shared object's definition
(M2), aliases are the definitions with equal `(shared FileId, st_value)`:
one copy serves them all (lld and mold, E17; GNU ld makes a second copy
for a second strong alias, E17: two copies of one object). `Slots`
(`spec/model.md` §4.3) groups them; resolution supplies `def`.

## 8. The linker's own symbols (file 0)

### 8.1 Which exist

A `Provide` candidate (rank 5) exists for a name in the table when some
live mention references the key (lld's rule; GNU ld defines its script's
non-`PROVIDE` assignments always, so they appear in its `.symtab`
unreferenced: §16). It never extracts a member and never makes a
reference undefined. Measured (E14: weak references to each name in a
static executable; "lld/GNU/mold" columns: defined by that linker):

| name | defined when | binding, visibility (mortar) | GNU ld | lld | mold | M |
|---|---|---|---|---|---|---|
| `_GLOBAL_OFFSET_TABLE_` | always (a `.got` exists when referenced) | local, hidden | yes | yes | yes | M1 |
| `_DYNAMIC` | dynamic links only (the `.dynamic` section) | local, hidden | dynamic only | dynamic only | always | M2 |
| `__ehdr_start` | always | local, hidden | yes | yes | yes | M1 |
| `__executable_start` | executables | local, hidden | yes (global) | yes | yes | M1 |
| `_etext`, `etext`, `__etext` | always | global | all three | `_etext`, `etext` | `_etext`, `etext` | M1 |
| `_edata`, `edata` | always | global | yes | yes | yes | M1 |
| `__bss_start` | always | global | yes | yes | yes | M1 |
| `_end`, `end` | always | global | yes | yes | yes | M1 |
| `__data_start`, `__end__`, `__bss_start__`, `__bss_end__`, `_bss_end__` | AArch64 only (GNU ld's AArch64 script) | global | AArch64 | no | no | M1 |
| `__init_array_start/_end`, `__preinit_array_start/_end`, `__fini_array_start/_end` | always | local, hidden | yes | yes | yes | M1 |
| `__rela_iplt_start/_end` (AArch64, x86-64), `__rel_iplt_start/_end` (i386) | static executables that are not PIE (empty range when there is no IRELATIVE); under `-static-pie` not defined, so a reference is weak undefined and zero (GNU ld, lld; mold: absolute 0; all three on the three targets, WP-022.md): the `IRELATIVE` relocations are in `.rela.dyn` | local, hidden | yes | yes | only with IRELATIVE | M1 |
| `__start_<sec>`, `__stop_<sec>` | `<sec>` is a C identifier and an output section of that name exists | global, protected; `-z start-stop-visibility=` (M2) changes it | yes | yes | yes | M1 |
| `_TLS_MODULE_BASE_` | x86-64 (WP-029b; i386 with its back end): a TLS segment exists and the key is referenced (a conditional candidate; with no TLS chunk layout L-98 refuses it by name); `STT_TLS`, value 0 in the template (x64.md X-47: T = 0 by name, so only `.symtab` shows the value) | local, hidden | no | x86 only | yes | M1 (x86-64); M2 elsewhere |
| `__GNU_EH_FRAME_HDR` | `.eh_frame_hdr` exists | local, hidden | no | no | yes | M1 |
| `__dso_handle` | **not defined** (`crtbegin*.o` defines it) | — | no | yes (hidden) | yes | — |
| `_PROCEDURE_LINKAGE_TABLE_`, `__global_pointer$`, `_gp`, `__tls_get_addr`, `_init`, `_fini` | not defined (RISC-V, MIPS, or libc's) | — | no | no | no | — |

`__start_`/`__stop_` references keep the sections of `<sec>` live under
`--gc-sections` unless `-z start-stop-gc` (`spec/live.md`). Values are
layout's (`spec/model.md` §4.4 `defined`).

"Defined when" in the table is decided in two steps (§5.1, finding 12):
the rows whose condition is the output kind (`always`, `executables`,
`static executables`, `dynamic links only`, `AArch64 only`) are decided by
resolution; the rows whose condition is an output section or segment
(`__start_<sec>`/`__stop_<sec>`, `_TLS_MODULE_BASE_`,
`__GNU_EH_FRAME_HDR`) are **conditional candidates**, confirmed or reverted
to `Undefined` by the first layout stage (`Assignment`) before the scan.

### 8.2 When a file also defines it

E14 (an object defines the name strongly and references it; which value
does the reference get?):

| name | GNU ld | lld | mold |
|---|---|---|---|
| `_end`, `_edata`, `__bss_start` | **linker's** (its script assigns them without `PROVIDE`) | the object's | linker's |
| `_GLOBAL_OFFSET_TABLE_` | linker's | error `cannot redefine linker defined symbol` | linker's |
| `etext`, `__data_start` | the object's | the object's | the object's |
| every other name of §8.1 | the object's (`PROVIDE` in GNU's script) | the object's | linker's |

mortar: an object's definition wins (`Provide` is rank 5) for every name
where GNU ld and lld agree. For the names where GNU ld forces its own
value and lld takes the object's (`_end`, `_edata`, `__bss_start`, and on
AArch64 `__bss_start__`, `__bss_end__`, `_bss_end__`, `__end__`), and for
`_GLOBAL_OFFSET_TABLE_`, a live object definition is the error
`linker-symbol-redefined` (GNU ld silently gives references a value the
program did not define; lld gives a different one; refusing is the only
answer that agrees with one reference without contradicting the other
silently). mold's overriding of every name is §16.

`_GLOBAL_OFFSET_TABLE_` may be referenced from a shared object or a lazy
member like any key; only definitions are refused.

## 9. Shared objects and versions (M2; specified now)

### 9.1 What a shared object contributes

Read through the shared-object view (`spec/elf.md` §6, WP-006:
`SharedObject::{dynamic, symbols, versym, version_definitions,
version_needs, versions}`, `Versions::of(sym)`), from `.dynsym` only
(`.symtab` of a shared object is ignored, as by all three). Each entry
`i > 0`:

- `STB_LOCAL` entries are ignored (they should not be in `.dynsym` past
  `sh_info`; one there is `malformed-input`);
- undefined entries are **references** (they demand in round 0, §4.4, and
  set `ref_dynamic`);
- a defined entry is a **visible definition** (rank 4) when its visibility
  is `STV_DEFAULT` or `STV_PROTECTED` and its version (§9.2) does not hide
  it from the key; `STV_HIDDEN`/`STV_INTERNAL` entries in `.dynsym` are
  ignored (not verified with a reference);
- `SHN_COMMON` in a shared object is `malformed-input`; `SHN_ABS` is a
  definition;
- `STB_GNU_UNIQUE` is a definition like `STB_GLOBAL`.

### 9.2 `versym` and the key

`Versions::of(sym)` gives `Versym{index, hidden}` and `Version`:

| `versym` | meaning for the key |
|---|---|
| no `.gnu.version` section | unversioned: the mention is key `name` |
| `VER_NDX_GLOBAL` (1) | unversioned: key `name` |
| `VER_NDX_LOCAL` (0) on a defined entry | `malformed-input` (lld: `version definition index 0 for symbol foo is out of bounds`; GNU ld accepts and binds; mold ignores the definition, E16). Refused because no linker writes it and the three disagree; on an undefined entry it is the normal value and means unversioned |
| `index ≥ 2`, not hidden, a version definition `V` of this object | a **default** definition: key `name` (the same row as `name@@V`, `spec/model.md` §2.3) and the definition carries version `V` |
| `index ≥ 2`, hidden (`VERSYM_HIDDEN`, bit 15) | a **non-default** definition: key `name@V` only. An unversioned reference never binds to it: GNU ld, lld and mold all report `foo` undefined against a library whose only `foo` is `foo@V1` (E16) |
| `index ≥ 2` naming a version *need* (`Needed{..}`) on a defined entry | `malformed-input` |
| index past the version definitions and needs | `malformed-input` (GNU ld: ``foo: invalid version 9 (max 3)``; lld: `corrupt input file: version definition index 9 …`; mold 2.37.1 crashes, E16) |
| the base definition (`VER_FLG_BASE`, index 1) | the file's own name: as `VER_NDX_GLOBAL` |

A shared object may define both `foo@@V2` (default) and `foo@V1` (hidden):
two rows. An undefined entry carries its required version in its
`Needed` version: its key is `name@V` (a reference that requires `V`),
except that a requirement of the default version of the definition
binds there too (it is the row `name`, with the version recorded): the
key interner (`spec/input.md`) must therefore intern an undefined
versioned reference `name@V` as `name@V`, and resolution then looks for
a definition of `name@V` — a hidden one, or the default one whose version
is `V` (§18 asks `spec/model.md` §2.3 to state this second lookup).

The resolver builds, per shared object, its own index from version
index to name once (`Versions::version` walks the definition chains per
call, which costs symbols × versions; the resolver does not call it per
symbol).

### 9.3 Binding a reference to a versioned definition

- An unversioned reference from an object binds to the default definition
  (key `name`) and the output records a version need for its version: all
  three record `V2` for `foo` against `foo@V1`, `foo@@V2` (E16).
- A `.symver foo_old, foo@V1` reference in an object (an undefined
  `foo@V1`) binds to the hidden `foo@V1` and records `V1` (all three, E16).
- A versioned reference that finds no definition with that version is
  undefined (`undefined symbol: foo@V3`).
- Version needs are recorded per needed shared object for the versions the
  output's references bind to; `spec/synth.md` writes `.gnu.version_r`.

### 9.4 Versions in relocatable objects

`.symver` gives an object's entries names `foo@V` and `foo@@V`
(`foo@@@V` is GNU as's input syntax and appears in the object as one of
the two). For keys, `foo@@V` is `foo` (default) and `foo@V` its own row.
In an executable (M1/M2) versions mean nothing at run time. In a static
link (finding 9, R12; `mv2.o` references `foo@V2` through `.symver`):

| reference | definition in an object | GNU ld | lld | mold | mortar |
|---|---|---|---|---|---|
| `foo@V2` | `foo@V2` (non-default) | binds (4) | binds | binds | binds |
| `foo@V2` | `foo@@V2` (default) | binds (9) | `undefined symbol: foo@V2` | undefined `foo` | **binds** (GNU ld) |
| `foo` | `foo@@V2` | binds (9) | binds | binds | binds (same row) |
| `foo` | `foo@V2` only | undefined | undefined | undefined | undefined |

So the **second lookup** of §9.2 (a reference `name@V` also binds to a
default definition of `name` whose version is `V`) applies to definitions
in relocatable objects as well as in shared objects, in every link. For
extraction, `first(name@V)` (§4.1) is taken over the definitions of both
rows that would bind it (not verified against GNU ld for a lazy
`foo@@V2`). A versioned definition appears in `.symtab` under its full
name (not verified against the references).

What real static links need (survey of every `*.a` under
`/usr/lib/aarch64-linux-gnu`, `/usr/x86_64-linux-gnu/lib`,
`/usr/i686-linux-gnu/lib` and the GCC library directories of the three
targets, `nm -A | grep @`, R12): one versioned name, in `libmcheck.a`
(`__malloc_initialize_hook@GLIBC_2.17` on AArch64, `@GLIBC_2.0` on i386,
`@GLIBC_2.2.5` on x86-64), a non-default definition nothing references.
`libc.a`, `libstdc++.a` and `libgfortran.a` have none. Static links of
the system libraries therefore need only the plain row rule; the second
lookup matters for user objects built with `.symver`. In shared-object output (M4) a definition `foo@@V` exports `foo`
with version `V` and `foo@V` exports a hidden `foo@V`; `V` must be a
version node of the version script, else the error
`undefined-version` (GNU ld: ``version node not found for symbol
foo@V``; not verified here).

### 9.5 `--as-needed` and `DT_NEEDED`

A shared object is **needed** when it is not under `--as-needed`, or when
some definition of it is `def` of a key that has a **strong reference from
a live relocatable object** (`ref_strong` from objects), or from another
needed shared object that does not itself list it in `DT_NEEDED` (GNU ld's
manual, "--as-needed"). A weak reference does not make it needed: GNU ld
and lld, E4 (the weak reference then resolves to zero: `def` stays the
shared definition for resolution but the output treats the key as
`Undefined{weak: true}`; mold makes the library needed: §16). This is a
fixed point over needed shared objects (a needed one's references can make
another needed); it is computed after `file_live`, from minima (§12).

Unlike GNU ld, the position of an as-needed library relative to the
reference does not matter (§4.4: GNU drops `--as-needed libs.so m.o`;
lld and mold keep it; mortar keeps it).

`--copy-dt-needed-entries` (`--add-needed`): refused (M4); without it, a
symbol defined only by a shared object's own `DT_NEEDED` dependency that is
not on the command line is undefined (the default of GNU ld since 2.22,
and lld's, mold's; not verified here).

`--no-allow-shlib-undefined`: §5.2.

### 9.6 Export, preemption, copy relocations

What resolution marks for `.dynsym` (M2 executables; M4 shared output):

- **Exported from an executable**: a key defined by a live object (or file
  0) with default or protected visibility, when `--export-dynamic`; or it
  is referenced by a live shared object (`ref_dynamic`); or it is defined
  by a live shared object too (`defined_dynamic`: the executable's
  definition interposes; all three export `x` defined weak in the
  executable and defined by `libso.so`, E12); or it matches
  `--export-dynamic-symbol`, `--dynamic-list`. Hidden keys never
  (§3.5).
- **Exported from a shared object** (M4): every defined key of default or
  protected visibility not made local by a version script (§9.7), by
  `--exclude-libs` (defined by a member of a named archive, or every
  archive for `ALL`), or by `-Bsymbolic` variants' export rules (they do
  not change export, only preemption).
- **Preemptible** (M2 for executables: a definition in the executable is
  never preemptible; a `Shared` def always is): in a shared object (M4), a
  defined default-visibility key is preemptible unless `-Bsymbolic`,
  `-Bsymbolic-functions` (for `STT_FUNC`), `-Bsymbolic-non-weak-functions`
  (lld: non-weak `STT_FUNC`) or `-Bsymbolic-non-weak` applies, or it is
  version-local, or `--dynamic-list` is given and does not list it
  (`--dynamic-list` keeps the listed symbols preemptible under
  `-Bsymbolic`). `STV_PROTECTED` is never preemptible.
- **Copy relocations and canonical PLTs** are demands of the scan
  (`spec/model.md` §3.7). Resolution gives the scan, for a `Shared`
  definition: the aliases (§7.3); the **alignment** of the copy,
  `min(sh_addralign of the defining section, 2^ctz(st_value))` (the
  trailing zeros of the value, the alignment the object has at run time
  since shared objects load at page-aligned bases; `st_value` 0: the
  section's alignment; GNU ld uses the section's alignment, E17, which
  over-aligns; both are valid); whether it is **read-only**: the defining
  address lies in a `PT_LOAD` without `PF_W` (all three then put the copy
  in RELRO: GNU ld `.data.rel.ro`, lld `.bss.rel.ro`, mold
  `.copyrel.rel.ro`, E17); and its size `st_size`. A copy relocation
  against an `STV_PROTECTED` definition is an error of the scan (not
  verified; `spec/arch.md`).

### 9.7 Version scripts (M4; executables too)

Parsed by `spec/script.md`. Applied to each key defined by a live object,
by its name before `@` (versioned names `foo@V` from `.symver` are not
matched: they carry their version). The rule, measured (E16), all three
agreeing except where noted:

1. A pattern without wildcards (in any node, `global:` or `local:`)
   matches before any wildcard pattern: `V1 { global: f*; }; V2 { global:
   foo; };` puts `foo` in `V2`.
2. Among wildcard patterns, the **last node** that matches wins: `V1 {
   global: foo*; }; V2 { global: fo*; };` puts `foo` in `V2`; so does the
   reverse order with `foo*` in `V2`.
3. Inside one node, a `global:` match wins over a `local:` match of the
   same kind: `{ global: fo*; local: foo*; }` keeps `foo` global.
4. `local: *` (or any wildcard local) in a later node does not override an
   exact global; an exact `local:` overrides any wildcard `global:` (E16:
   `{ global: *; }; V2 { local: foo; };` hides `foo`).
5. An exact name in two nodes: GNU ld and lld keep the **first** (lld
   warns `attempt to reassign symbol 'foo' of version 'V1' to version
   'V2'`), mold the last. mortar: the first, with the warning
   `version-reassigned`.
6. `extern "C++" { "baz()"; }` and `extern "C++" { baz*; }` match the
   demangled name (all three, E16; demangling is `spec/diag.md` §9's
   later demangler, which this needs).
7. A key matched by no pattern keeps the base version (exported
   unversioned).
8. `local:` before `global:` in one node: GNU ld rejects it as a syntax
   error, lld and mold accept (E16). `spec/script.md` decides (accepting
   it costs nothing and loses nothing).
9. A name in a script that no file defines: an error under
   `--no-undefined-version` (lld's default since lld 16; GNU ld's default
   accepts it; not verified here), refused until decided (§17 Q6).

Version definitions written for shared output keep the script's parent
links (`V2 { … } V1;`): GNU ld writes them (`readelf -V`: `Parent 1:
V1`), lld and mold do not (E16). glibc's loader ignores parents; mortar
follows GNU ld so that `readelf -V` of a mortar library reads like GNU
ld's (`spec/synth.md`).

## 10. The fate of sections

M1. `kind` and `state` of `sections[SectionId]` (`spec/model.md` §3.3),
per section of a live object, by the first row that matches; the
classification is per file (I-file), the state needs `comdat`:

| match | kind | state | references |
|---|---|---|---|
| a **file** with the symbol `__gnu_lto_slim` (a common symbol in GCC 14's slim LTO objects; tested before any section row, finding 10: their `.gnu.lto_*` sections are `SHF_EXCLUDE` and would match that row first) | — | the file: `Refused{FileFormat, "GCC LTO object"}` (GNU ld: `plugin needed to handle lto object`; mold refuses; lld silently ignores the file and reports `main` undefined, E18) | |
| index 0 | `Null` | — | |
| `SHT_SYMTAB`, `SHT_SYMTAB_SHNDX`, `SHT_STRTAB` without `SHF_ALLOC` | `Symtab` / `Strtab` | `Dropped(Consumed)` | all |
| `SHT_REL`, `SHT_RELA` | `Reloc` | `Dropped(Consumed)`; the section it applies to records it. A second one for a kept section: `Refused{Feature, "two relocation sections for one section"}`, never a silent drop (GNU ld applies one of two `SHT_RELA` and both of `SHT_REL`+`SHT_RELA`; lld refuses; mold asserts on two `SHT_RELA` and drops one of `SHT_REL`+`SHT_RELA`; WP-022.md) | |
| `SHT_GROUP` | `Group` | `Dropped(Consumed)` | all |
| a member of a group not kept (§6) | its kind | `Dropped(Comdat{keeper})` | all |
| `SHF_EXCLUDE` (any type) | `Excluded` | `Dropped(Excluded)` | GNU ld, lld; mold keeps an allocated one (E18): §16 |
| name `.note.GNU-stack` | `Stack` | `Dropped(Consumed)` (layout reads its flags for `PT_GNU_STACK`) | all (E18) |
| name `.note.gnu.property`, `SHT_NOTE` | `Note` | `Kept`; `spec/synth.md` merges the properties into one note | GNU ld, lld, mold merge (not verified here) |
| `SHT_LLVM_ADDRSIG` (`.llvm_addrsig`) without `SHF_EXCLUDE` | `Excluded` | `Dropped(Consumed)` (a later `--icf=safe` reads it). Clang emits it with `SHF_EXCLUDE`, so the `SHF_EXCLUDE` row above matches first: `Excluded`, `Dropped(Excluded)` | all (E18) |
| `SHT_LLVM_CALL_GRAPH_PROFILE` | `Excluded` | `Dropped(Consumed)`: a section-order hint; ignoring it is correct | not verified |
| `SHT_LLVM_DEPENDENT_LIBRARIES` (`.deplibs`) | — | `Refused{SectionType, ..}`: lld adds the libraries it names, so doing nothing would differ | not verified |
| other `SHT_LLVM_*` (`ODRTAB`, `LINKER_OPTIONS`, `SYMPART`, `PART_EHDR`, `PART_PHDR`, `BB_ADDR_MAP`, `OFFLOADING`, `LTO`, `JT_SIZES`) | — | `Refused{SectionType, ..}` | |
| name `.gnu.lto_*` otherwise (a fat LTO object: GCC marks them `SHF_EXCLUDE`) | `Excluded` | `Dropped(Excluded)`: the object's real code links, as every linker does without a plugin | not verified |
| name `.gnu.warning` / `.gnu.warning.<sym>` | `Excluded` | `Dropped(Consumed)`; the text is a warning (`gnu-warning`): for `.gnu.warning` when the file is live, for `.gnu.warning.<sym>` per key `<sym>` referenced from a live section (one diagnostic per key, first 3 references, emitted by the scan) | GNU ld (E21); lld and mold neither warn nor drop: they copy the section into the output. glibc's `libc.a` has 112 such sections (E21) |
| name `.gnu.linkonce.*` | — | refused (§6.6) | |
| name prefix `.zdebug` | — | `Refused{Feature, ".zdebug compressed debug sections"}` | (GNU-style compression, superseded by `SHF_COMPRESSED`) |
| `SHF_COMPRESSED` (GCC and Clang `-gz`) | — | `Refused{Feature, "SHF_COMPRESSED input sections"}` until the store decompresses (`spec/layout.md` L-52); a non-alloc `.debug*` one under `-S`/`-s`: `Dropped(Stripped)` (WP-027c ruling: never copied compressed as if plain) | the references decompress |
| `SHT_X86_64_UNWIND` (x86-64) or name `.eh_frame` | `EhFrame` | `Kept` | |
| `SHF_MERGE` with entry size > 0 | `Merge`, or `Regular` when `spec/synth.md` S-6 places it whole (writable, code, TLS, relocated, `SHF_COMPRESSED`, odd shape) | `Kept`; a non-alloc `.debug*` one (`.debug_str`, `.debug_line_str`) under `-S`/`-s`: `Dropped(Stripped)`, as the `Debug` row (WP-027a: GNU ld, lld and mold drop them) | |
| `SHF_MERGE` with entry size 0 | `Regular` | `Kept` (not merged; lld) | not verified with GNU ld |
| non-alloc, name `.debug_*` | `Debug` | `Kept` (unless `-S`/`-s`: `Dropped(Stripped)`) | |
| `SHT_NOTE` other | `Note` | `Kept` | |
| `SHT_NOBITS` | `Nobits` | `Kept` | |
| `SHT_PROGBITS`, `SHT_INIT_ARRAY`, `SHT_FINI_ARRAY`, `SHT_PREINIT_ARRAY`, alloc `SHT_STRTAB` | `Regular` | `Kept` | |
| `SHT_AARCH64_ATTRIBUTES` (`0x70000003`, AArch64) | — | `Dropped(Consumed)` (`spec/layout.md` L-35, §20.7; applied in WP-027d): the feature marking is the property note's | Clang 22 emits it for every `-mbranch-protection` but `none`; GNU ld and mold drop it, lld copies it (E-18) |
| `SHT_DYNAMIC`, `SHT_DYNSYM`, `SHT_HASH`, `SHT_GNU_HASH`, `SHT_GNU_verdef/verneed/versym`, `SHT_RELR` in a relocatable object | — | `Refused{SectionType, ..}` | |
| any other type, or a flag mortar does not implement (`SHF_OS_NONCONFORMING`, unknown bits, a processor flag other than `SHF_AARCH64_PURECODE`/`SHF_X86_64_LARGE` where the arch spec admits them) | — | `Refused{SectionType or SectionFlag, ..}` (`spec/model.md` §3.3) | the references accept silently (`spec/diag.md` §8.1) |
| `SHF_GNU_RETAIN` | its kind | `Kept`, and a root of liveness (§4.7) | |

A refusal of this table (a type, a flag, a name) concerns a section's
content or fate, so it applies only to a section that would be kept: a
member of a COMDAT group that is not kept is `Dropped(Comdat)` and a
`.zdebug*` section under `-S`/`-s` is `Dropped(Stripped)`, without a
diagnostic, since they contribute nothing to the output. Structural
faults of a file (group flags §6.4, a section in two groups §6.5, symbol
faults §3) apply to every live file. (A survey of the 65 archives of the
three targets' libc and GCC directories finds no section any refusal
row matches, kept or dropped: WP-022.md.)

`.ARM.*`, `.riscv.*` and other machines' attribute sections cannot occur:
the file's machine is checked first (`spec/input.md`). `SHF_COMPRESSED`
does not change the kind once the store decompresses (`spec/model.md`
§1); until then the row above refuses it.

`section_base[f]` is the prefix sum of `e_shnum` over the live
relocatable objects in `FileId` order (`spec/model.md` §2.2): an exclusive
scan, computed after `file_live`.

## 11. `-r` (M4)

Refused until M4 (`Refused{Feature, "relocatable output (-r)"}`, already in
`spec/cli.md` as an option refusal). What changes then, so that this spec's
tables are ready for it: no undefined-symbol verdict (every undefined key
stays undefined); commons stay common unless `-d`/`-dc`/`-dp`; COMDAT
groups are kept per signature and written as groups (the keeper rule
still applies to duplicates); no linker-defined symbols (§8) except those
a script assigns; `--gc-sections` roots differ; versions are copied as
names. The rows of §3 are unchanged.

## 12. Determinism and the parallel formulation

### 12.1 Per column

| column | merge | order |
|---|---|---|
| `first(K)`, `cfirst(K)` | atomic minimum of `FileId` over the qualifying mentions of **all** files (§4.1), once, before the closure | `FileId` |
| `file_live` | reachability: per round, each file of the frontier ORs `live[M]` for the lazy targets of its edges; the next frontier is the newly set bits in `FileId` order; the pulling reference is an atomic minimum of `(FileId << 32) \| st_index` per member | `≺` |
| per-key class minima over live files (`Strong`, `Common`, `Weak`, `Shared`, `Shared` data, `Assign`, `Provide`) | atomic minimum of `(FileId << 32) \| st_index` per class, after the closure | `≺` |
| `def` | a pure function of the class minima (§3.2) | — |
| `common` size and alignment | atomic maximum of each, separately | integers |
| `clash` | after `def`: each `Strong` mention that is not `def` and not part of an all-absolute equal-value set ORs a flag; the diagnostic lists the mentions by an ordered collection (`spec/model.md` §6.3) | `≺` |
| visibility | atomic minimum over the constraint order (§3.5) | `INTERNAL < HIDDEN < PROTECTED < DEFAULT` |
| TLS agreement | atomic or of two bits (a TLS mention, a non-TLS mention) per key | — |
| `ref_*`, `defined_dynamic` | atomic or | — |
| `needed` (M2) | reachability as `file_live`, over shared objects | — |
| `comdat` | atomic minimum of `FileId` per `ComdatId` over live files | `FileId` |
| `section_base` | exclusive prefix sum in `FileId` order | — |
| `sections` | per file, disjoint slices (`spec/model.md` §6.1) | — |
| `redirect` | from the `Plan` only, before resolution | — |
| `archive-backref`, `archive-choice` (`--warn-backrefs` only) | the sequential simulation of §4.6, after the closure | command-line order |

`(FileId << 32) | st_index` fits a `u64`, so every minimum above is one
`AtomicU64::fetch_min`; the rank is the class, not part of the word.

### 12.2 The result equals the sequential definition

- **The graph is a function of the inputs.** `first` and `cfirst` are
  minima over all files' mentions, independent of liveness, computed
  before the closure: commutative, associative and idempotent, so any
  partition of the work gives the same values (`spec/model.md` §6.2).
  The edges of §4.1 are then fixed.
- **Reachability is unique.** The set of files reachable from the roots
  in a fixed graph does not depend on the order of the visit: any
  worklist, depth-first or breadth-first, sequential or parallel, ends
  with the same set (each step adds a target of an edge out of a reached
  file, and every such target is eventually added). The rounds of §4.1
  are one such visit; their number is bounded by the number of lazy files.
  The pulling reference is a minimum over the live mentions with an edge
  to the member, also order-independent.
- **`def` after the closure** is a function of class minima over the final
  live set.
- **COMDAT** does not feed back into extraction (§4.1), so the keeper,
  computed once over the final live set, cannot change `file_live`.
- **The `--warn-backrefs` simulation** is sequential by definition and
  reads only the inputs and the final `def`; it changes no table.
- **The sequential reference**: a worklist in `FileId` order. Tests run
  resolution under `MORTAR_SCHED_SEED` and thread counts 1, 2, 7 and 64
  and require identical tables and diagnostics (`spec/model.md` §6).

### 12.3 Diagnostics when several rules fire

All are emitted, each with its own key; none suppresses another, except
that a key with `tls-mismatch` gets no `common-*` warnings. Order follows
the keys (`spec/diag.md` §6.2): resolution's (phase 30: `duplicate-
symbol` at the second definition, `tls-mismatch` at the first mismatching
mention, `linker-symbol-redefined` at the object definition,
`required-symbol` at unit 0 and the argument index, the common warnings at
the second mention, `archive-*` at the referencing mention, a shared
object's undefined reference at its `.dynsym` entry) before the scan's
(phase 70: `undefined-symbol`, `comdat-mismatch`,
`discarded-section-reference`, `gnu-warning`).

## 13. Incremental consequences (for `spec/incr.md`, M7)

Stated as dependencies, so that M7 can tell what to recompute when a file
is replaced (with the same identity, `spec/model.md` §7.1) or added
(finding 16: the first draft's claims were too strong; these are the
honest ones).

- **Per-file summary.** Everything resolution reads of a file is its
  **summary**: for each non-local mention, its key, class (defined, weak,
  common, undefined), for a definition whether it is data (§2) and TLS and
  its absolute value, its visibility, common size and alignment; its
  groups' signatures, members, and the keys defined in each group; its
  section kinds. It is I-file.
- **The graph** (`first`, `cfirst`, the edges) depends on every file's
  summary and on `FileId` order. Replacing file `F` changes it only
  through `F`'s own entries: the targets `first(K)` and `cfirst(K)` for
  the keys `F` defines (before or after), and the edges out of `F`. A
  change of a definition's **type** between function and data, or of its
  **binding** between weak and non-weak, changes `cfirst` of that key even
  when the set of keys `F` defines is unchanged (finding 16).
- **`file_live`** is then recomputed by reachability over the stored
  summaries (no file is read). If neither any target nor `F`'s edges
  changed, `file_live` is unchanged; otherwise the closure is redone (it
  is cheap: no relocation, no section is read).
- **`def[K]`** depends on the mentions of `K` in live files and on which
  of their sections are dropped. Replacing `F` can change `def[K]` for the
  keys `F` mentions, for every key whose liveness of definers changed
  with `file_live`, and — when `F` gains or loses a COMDAT group or its
  file position makes it the new lowest live holder — for every key
  defined in a group whose keeper moved: other files' mentions in that
  group are demoted or restored (finding 16; the t10 case of §4.3).
- **`comdat[G]`** depends on the set of live files with group `G`; a
  file's `sections` state depends on `comdat` of its groups.
- **`common[K]`** depends on the live common mentions of `K`.
- **`redirect`** depends on the `Plan` only.
- **Verdicts and undefined diagnostics** depend on `def` and the scan of
  live sections; they are recomputed for the keys whose `def` changed and
  for the references in refilled units.
- **Identities**: `def` is stored as `(file identity, key)`, never as
  `st_index` (`spec/model.md` §7.1).

Two consequences the M7 design should know: adding or removing a file
changes `FileId`s after it and can change `first` of unrelated keys, so
every change of the input list is a full resolution (cheap) but not
necessarily a full layout; and a change that moves the COMDAT keeper of a
group moves code between files, which is a layout change of both.

## 14. Diagnostics

`unit`/`seq` per `spec/model.md` §8. Severity is the default; the
switches of `spec/diag.md` §6.3 apply. New kinds are proposed for
`spec/diag.md` §3.1 (area "symbols"); their wording follows §8.2 there,
and, by the policy of §3.8, where GNU ld has a phrase build tools match
for the same condition, mortar's message contains it verbatim (the
messages below in which a GNU phrase appears in parentheses).

| condition | id | severity | key | mortar's message | GNU ld's wording (for people who search for it) |
|---|---|---|---|---|---|
| a `Strong` clash (§3.8) | `duplicate-symbol` (exists) | error | 30, second definition | ``duplicate symbol: {name} (multiple definition of `{name}')`` + `>>> defined in` ×2..3 | ``multiple definition of `x'; a.o:(.data+0x0): first defined here`` |
| unresolved strong reference (§5) | `undefined-symbol` (exists) | error / warning | 70, first surviving reference | ``undefined symbol: {name} (undefined reference to `{name}')`` + `>>> referenced by` ×≤3 + note | ``undefined reference to `x'`` |
| TLS against non-TLS (§3.4) | `tls-mismatch` (new) | error | 30, first mismatching mention | `TLS attribute mismatch: {name}` + `>>> defined in`, `>>> referenced by` (lld's) | ``x: TLS definition in t.o section .tdata mismatches non-TLS reference in m.o`` |
| an object defines a forced linker symbol (§8.2) | `linker-symbol-redefined` (new) | error | 30, the object's mention | `cannot redefine linker-defined symbol: {name}` | (GNU accepts) lld: `u.o cannot redefine linker defined symbol` |
| `--require-defined` not satisfied | `required-symbol` (new) | error | 30, unit 0, seq = argument index | `required symbol not defined: {name}` | ``required symbol `x' not defined`` |
| entry symbol undefined (§5.5) | `entry-not-found` (new) | warning | 30, unit 0, seq = argument index (0 for the default) | `cannot find entry symbol {name}` | ``cannot find entry symbol x; defaulting to …`` |
| reference to a symbol only a dropped COMDAT copy defines (§6.3) | `comdat-mismatch` (new) | error | 70, first surviving reference | `symbol {name} is defined only in a discarded copy of COMDAT group {sig}` + `>>> kept copy:`, `>>> discarded copy defining it:`, `>>> referenced by` + the ODR note | ``undefined reference to `x'`` (GNU ld keeps the other copy and links) |
| reference into a dropped COMDAT copy (§6.3) | `discarded-section-reference` (new) | error | 70, first reference | `relocation refers to a symbol in a discarded section: {name}` + `>>> defined in`, `>>> referenced by` (lld's) | ``x referenced in section `.text' of b.o: defined in discarded section `.text.g[g]' of b.o`` |
| common larger than the definition (§3.3) | `common-larger-than-definition` (new) | warning | 30, the definition | `common symbol {name} is larger than its definition` + places + note with both sizes | ``size of symbol `x' changed from 64 in c.o to 8 in d.o`` |
| definition less aligned than a common (§3.3) | `common-alignment` (new) | warning | 30, the definition | `definition of {name} is less aligned than its common symbols` + note | ``alignment 8 of normal symbol `x' in d.o is smaller than 16 used by the common definition in c.o`` |
| `--warn-common` (§3.3) | `multiple-common`, `common-overridden` (new) | warning | 30, second mention | `multiple common symbols: {name}`; `common symbol {name} is overridden by a definition` | ``common of `x' overriding smaller common``; ``definition of `x' overriding common`` |
| `--warn-backrefs`: GNU ld's walk would define it from another file (§4.6) | `archive-choice` (new) | warning, opt-in | 30, the first referring mention | `GNU ld would choose another definition of {name}` + `>>> mortar uses`, `>>> GNU ld uses` | (none) |
| `--warn-backrefs`: GNU ld's walk would leave it undefined (§4.6) | `archive-backref` (new) | warning, opt-in | 30, the first referring mention | `backward reference to an archive member: {name}` + places | (lld: `backward reference detected: x in b refers to a`) |
| a needed shared object's undefined reference (§5.2, M2) | `undefined-symbol` | error | 30, the shared object, `.dynsym` index | as above, with the note of §5.3 | ``libl.so: undefined reference to `missing'`` |
| hidden key referenced by a shared object (§3.5) | `non-exported-symbol` (new) | error | 30, the definition | `non-exported symbol referenced by a shared object: {name}` + places | ``hidden symbol `x' in a.o is referenced by DSO`` |
| `.gnu.warning[.sym]` (§10) | `gnu-warning` (new) | warning | 70 (per symbol) / 30 (per file) | `{text}` with `>>> referenced by` | ``warning: foo is dangerous`` (the section's text) |
| exact version name twice (§9.7) | `version-reassigned` (new) | warning | 30, unit 0 | `{name} is assigned to version {first} and {second}; {first} is kept` | (accepted silently) |
| `foo@V` with no node `V` (§9.4) | `undefined-version` (new) | error | 30, the mention | `version {version} of {name} is not defined` | ``version node not found for symbol foo@V`` |
| malformed symbol or version data (§3.4, §9.2) | `malformed-input` (exists) | error | 20 (input) when the reader finds it; 30 when resolution does | the reader's statement | ``invalid version 9 (max 3)`` |
| constructs of §1.2 | `refused` (exists) | error | 30, the mention or section | `unsupported {noun}: {name}` | |

`-y SYM` (`--trace-symbol`, M4) prints `<file>: definition of SYM` /
`reference to SYM` lines; it is a report, not a diagnostic
(`spec/diag.md` has no informational severity): §17 Q7.

## 15. Tests

### 15.1 Behavioral tests (`crates/mortar_resolve/tests/`, built with `mortar_testutil`'s object builder)

Named `s<section>_<what>`; each checks the tables, not only that a link
succeeds:

- §3.2: `s3_2_strong_beats_weak_either_order`, `s3_2_first_weak_wins`,
  `s3_2_common_beats_weak`, `s3_2_weak_beats_shared`,
  `s3_2_lazy_files_have_no_candidates` (finding 5),
  `s3_2_shared_data_beats_commons_alone`,
  `s3_2_shared_function_does_not_beat_common`, `s3_2_provide_beats_shared`,
  `s3_2_assign_beats_strong_without_clash`, `s3_2_order_is_total` (a
  property test: random candidate sets, `def` equal to the brute-force
  minimum of the table, under permutations of the files' processing order).
- §3.3: `s3_3_size_and_alignment_are_maxima`,
  `s3_3_alignment_not_power_of_two_is_malformed`,
  `s3_3_larger_common_warns_in_both_orders`,
  `s3_3_less_aligned_definition_warns`, `s3_3_warn_common`.
- §3.4: `s3_4_tls_mismatch_each_direction` (TLS ref/non-TLS def, NOTYPE
  ref/TLS def, common/TLS def, TLS ref/non-TLS ref with no definition),
  `s3_4_unique_duplicates_clash`, `s3_4_equal_absolutes_do_not_clash`
  (three equal ones too), `s3_4_unequal_absolutes_clash`,
  `s3_4_absolute_and_section_definition_clash`,
  `s3_4_nonlocal_section_symbol_is_malformed`.
- §3.5: `s3_5_most_constraining_visibility`, `s3_5_hidden_needs_object_definition`,
  `s3_5_hidden_weak_undefined_is_zero`, `s3_5_lazy_mentions_do_not_contribute`.
- §3.7: `s3_7_output_binding_table` (one case per row).
- §3.8: `s3_8_duplicate_lists_three_places`, `s3_8_muldefs_takes_first`.
- §4.1: `s4_1_closure_is_order_independent` (generated archives with
  chains and cycles; a sequential worklist written from §4.1 in the test,
  breadth-first rounds and a random-order worklist give the same set),
  `s4_1_first_counts_live_and_lazy`, `s4_1_definition_before_archive_blocks`,
  `s4_1_weak_definition_after_archive_does_not_block` (finding 1, R1),
  `s4_1_weak_reference_does_not_extract`, `s4_1_lazy_common_is_first`
  (R5), `s4_1_weak_member_first_wins` (R3), `s4_1_wrap_target_is_not_a_root`,
  `s4_1_pulling_reference_is_least`, `s4_1_def_never_lazy` (finding 5),
  `s4_1_reads_member_tables_not_index` (R8).
- §4.2: `s4_2_each_root_option_extracts`, `s4_2_default_start_is_a_root`
  (R9), `s4_2_require_defined_errors`, `s4_2_whole_archive_members_are_roots`
  (R4).
- §4.3: `s4_3_system_lines_match_gnu` (the sixteen lines of the landed
  record, member sets against recorded `ld.bfd -t` lists).
- §4.5: `s4_5_common_targets` (the table of §4.5 row by row, and R6's
  weak and function blockers), `s4_5_common_and_strong_reference_both_edges`.
- §4.6: `s4_6_simulation_matches_bfd` (E1, E2, t1, t2, t2b, t10, the group
  variants, against recorded member lists), `s4_6_choice_fires_for_t2_not_t2b`,
  `s4_6_nothing_runs_without_option`.
- §5: `s5_1_relaxed_reference_is_not_undefined` (in `mortar_arch`: the
  x86-64 and i386 TLS GD sequences against an undefined `__tls_get_addr`),
  `s5_1_unused_hidden_undefined_is_accepted`, `s5_1_conditional_provide_reverts`,
  `s5_2_verdict_table`, `s5_3_one_diagnostic_three_places_and_count`
  (in `mortar_arch`'s scan tests, against resolution's `def`).
- §6: `s6_1_keeper_is_lowest_live_file`, `s6_1_section_symbol_signature_uses_section_name`,
  `s6_2_demoted_mentions_bind_to_keeper`, `s6_3_reference_table` (each row,
  each debug name of R13), `s6_3_comdat_mismatch_names_group_and_files`
  (t10),
  `s6_4_non_comdat_groups_are_not_merged`, `s6_6_linkonce_is_refused`.
- §7: `s7_1_wrap_undefined_mentions_only`, `s7_1_real_extracts_target`,
  `s7_2_defsym_forms`, `s7_2_defsym_alias_redirects_all_mentions`.
- §8: `s8_1_provided_only_when_referenced`, `s8_1_table` (one per row and
  architecture), `s8_2_object_definition_wins`, `s8_2_forced_names_refused`.
- §9: `s9_2_versym_table` (each row, with hand-built shared objects),
  `s9_4_second_lookup_in_objects` (R12),
  `s9_3_binding_and_needs`, `s9_5_as_needed_strong_only`,
  `s9_5_needed_is_a_fixed_point`, `s9_6_export_rules`, `s9_6_copy_alignment_and_readonly`,
  `s9_7_version_script_precedence` (the nine rules).
- §10: `s10_classification_table` (one per row), `s10_slim_lto_refused`
  (before the `SHF_EXCLUDE` row, finding 10).
- §12: `s12_same_tables_under_seeds_and_threads` (1, 2, 7, 64 threads,
  ten seeds, over the generated corpus of §4.1 and the system archives).
- §13: `s13_unchanged_graph_keeps_file_live`,
  `s13_type_change_moves_cfirst`, `s13_group_change_moves_keeper`.

### 15.2 Link tests to add (`tests/link/resolve/`, `tests/link/diag/`)

Each states the references' behavior it pins; until M1 each also carries
`xfail(mortar): no linking yet (M1)` (M2 ones `(M2)`).

| test | pins | markers |
|---|---|---|
| `resolve/archive-later-member.test` | E1 case 1, across archives: `-lA -lB -lC`; bfd exit 2, lld/mold/mortar exit 1; with `--warn-backrefs` on mortar's line, `diag(mortar): GNU ld would choose another definition of pick` (§4.6); without it, `not(mortar)` of that text | per-linker `run` expectations |
| `resolve/archive-same-member.test` | E1 in one archive `[a.o b.o c.o]`: bfd 2, lld/mold 1 | as above |
| `resolve/archive-group-member.test` | E1 under `--start-group`: bfd 2, lld/mold 1 | as above |
| `resolve/archive-extra-duplicate.test` | E1 case 2: bfd links; lld, mold fail `duplicate symbol: dup`; mortar fails with the note | `link(bfd)` and `link(lld,mold,mortar)` |
| `resolve/archive-backref-duplicate.test` | E2 `main1 libX libY`: bfd links (120, it never sees the back-reference); lld, mold and mortar fail `duplicate symbol: K` (§4.3 case 2); under `--warn-backrefs`, mortar's note that GNU ld would not load the member | `link(bfd)`, `link(lld,mold,mortar)` |
| `resolve/archive-choice-t2.test` | finding 3: `m2.o libA.a libCB.a` gives bfd 3, the others 1, and `archive-choice` under `--warn-backrefs`; `m2.o libAB.a libC.a` gives 1 everywhere and no diagnostic | per linker |
| `resolve/archive-dso-order.test` (M2) | E4, the twelve cases and the three disagreeing lines | per linker |
| `resolve/common-extract.test` | E3: common vs lazy data definition (bfd, mold 9; lld 0), vs lazy function (no extraction in bfd/lld) | per-linker expectations; lld differs by design |
| `resolve/fortran-block-data.test` | E19: `BLOCK DATA` in an archive initializes the common block (bfd, mold pass; lld fails) | `requires: gfortran`; `skip(lld)`: lld needs `--fortran-common`; `arch: aarch64 x86_64` |
| `resolve/common-weak.test` | E6: common beats weak (bfd, lld); mold takes the weak | `has(mold)` per linker |
| `resolve/common-align.test` | E6: alignment is the maximum (bfd, lld); mold under-aligns | `xfail(mold)` |
| `resolve/common-shared.test` (M2) | E12: shared data definition beats commons (bfd, mold 50; lld 0) | per linker |
| `resolve/visibility-hidden.test` | E6: hidden reference makes the definition local; `.symtab` LOCAL (lld, mold), GLOBAL HIDDEN (bfd) | `has(bfd)` per linker |
| `resolve/tls-mismatch.test` | E6: both directions; bfd errors both, lld one, mold none | `exit: fail` for bfd/mortar; per linker |
| `resolve/absolute-duplicates.test` | E6: equal absolutes link (bfd, lld), unequal fail (all) | per linker |
| `resolve/comdat-keeper.test` | E10: keeper with differing copies; bfd/lld 22, mold/mortar 11 | per linker; comment: an ODR violation on purpose |
| `resolve/comdat-section-signature.test` | E18: `.foo` and `.bar` section-symbol signatures both kept (bfd, mold); lld drops one | `xfail(lld)` |
| `resolve/comdat-discarded-ref.test` | E9: reference to a local of a dropped copy fails (bfd, lld); mold crashes | `skip(mold)`: mold 2.37.1 segfaults |
| `resolve/comdat-except-table.test` | E9: `.gcc_except_table` reference into a dropped copy links (bfd, mold) | `xfail(lld)` |
| `resolve/linkonce.test` | E11: bfd dedups, lld/mold duplicate; mortar refuses | `diag(mortar): unsupported feature: .gnu.linkonce sections` |
| `resolve/wrap.test` (M4) | E13: same-object reference not wrapped (bfd, mold 111; lld 87), archive extraction through `__wrap_`/`__real_` | per linker |
| `resolve/defsym.test` (M4) | E13: number, alias, alias overriding a definition, `k+4` (mold refuses) | per linker |
| `resolve/linker-symbols.test` | E14: the §8.1 table, per architecture, with weak references | `has(...)` per linker |
| `resolve/linker-symbols-user.test` | E14: user definitions of provided names win (bfd, lld); forced names: mortar refuses | per linker |
| `resolve/undefined-ignored.test` | E15: unused undefined symbol and a gc'd reference link | all |
| `resolve/as-needed-position.test` (M2) | E4: `--as-needed libs.so m.o` fails in bfd, links in lld/mold/mortar | per linker |
| `resolve/as-needed-weak.test` (M2) | E4: weak reference does not make an as-needed library needed (bfd, lld); mold does | `has(mold)` per linker |
| `resolve/shlib-undefined.test` (M2) | E20: executable fails on a shared object's undefined symbol (bfd, lld); mold links | per linker |
| `resolve/hidden-referenced-by-dso.test` (M2) | E20 | per linker; mold links |
| `resolve/version-bind.test` (M2) | E16: default and `.symver` references, hidden-only version undefined | all agree |
| `resolve/version-script-order.test` (M4) | E16: rules 1-5 of §9.7; rule 5 mold differs | per linker |
| `resolve/copy-reloc-alias.test` (M2) | E17: one copy for aliases (lld, mold); bfd two | per linker |
| `resolve/gnu-warning.test` | E21: `.gnu.warning.foo` warns (bfd) and is not in the output | `diag(bfd,mortar)`; `not(lld,mold)` per linker |
| `resolve/lto-slim.test` | E18: refused; bfd and mold refuse, lld fails differently | `requires`: gcc `-flto` only |
| `resolve/weak-after-archive.test` | R1: `t1m.o libt1k.a t1w.o` (weak definition after the archive): bfd, lld, mortar 2, mold 1; with a strong one (`t1s.o`) bfd, lld, mortar fail `multiple definition of`/`duplicate symbol: K`, mold links | written (§15.3) |
| `resolve/comdat-mismatch.test` | t10, ruling C: `m10.o libG.a`: bfd, lld run 140; mold `undefined symbol: hx`; mortar `comdat-mismatch` | written (§15.3) |
| `resolve/entry-default-root.test` | R9: `ld -static m0.o -lst`, no `-e`, `_start` only in an archive member: all three extract it | written (§15.3) |
| `resolve/tls-ref-mismatch.test` | R7: TLS reference vs non-TLS reference, no definition between them (bfd error, lld/mold link); mortar `tls-mismatch` | per linker |
| `resolve/hidden-unused.test` | R14: `.hidden zz` with no use: bfd error, lld/mold/mortar link | `link(bfd)` `exit: fail` |
| `resolve/versioned-static.test` | R12: the four rows of §9.4 | per linker |
| `resolve/archive-noindex.test` | R8: an `ar rcS` archive: bfd ``archive has no index``; lld, mold, mortar run 7 | per linker |
| `resolve/discarded-debug-names.test` | R13: each name of §6.3's table, one link per name, against a dropped copy | per linker; bfd's are the reference for mortar |
| `resolve/lazy-common.test` | R5: `mv.o libLC.a` extracts the common member (all); `mv.o libLC.a libLV.a`: bfd 9, lld 0, mold 9, mortar 9 | per linker |
| `resolve/common-weak-blocker.test` | R6: weak definition before or after `libLV.a` does not stop extraction (bfd 9, mortar 9; lld 0; mold 4); a function definition does | per linker |
| `resolve/defsym-position.test` (M4) | R10: `--defsym K=k` after `libk.a`: bfd fails ``undefined symbol `k' referenced in expression``; lld, mold, mortar link | per linker |
| `diag/undefined-many.test` | E15: one diagnostic per symbol, first 3 references, "referenced 3 more times" (lld, mold); bfd one line per reference | per linker |
| `diag/duplicate-places.test` | both places in the duplicate diagnostic | per linker |

### 15.3 Seed tests

`resolve/archive-backref.test` must change (§18). Written with this
revision (review of 2026-11-14), mortar rows `xfail(mortar): no linking
yet (M1)`: `resolve/weak-after-archive.test` (finding 1, R1),
`resolve/comdat-mismatch.test` (finding 2, t10) and
`resolve/entry-default-root.test` (finding 14, R9). The others
(`archive-extract`, `archive-order`, `comdat-inline`, `common`, `symtab`,
`thin-archive`, `weak-strong`, `weak-undef`, `diag/duplicate`,
`diag/undefined`) hold as written; their `spec:` directives name sections
of this spec by title ("Archives", "Weak symbols", "Common symbols",
"COMDAT groups"), which §18 asks to update to the section numbers.

## 16. Where the references differ

Every disagreement found; "E" is the experiment in the landed record.

| what | GNU ld 2.44 | lld 22.1.8 | mold 2.37.1 | mortar | E |
|---|---|---|---|---|---|
| back-reference to an archive (`-lhelper -luser`) | undefined | resolved | resolved | resolved (§4.3) | seed test |
| lowest definer passed before the reference (across archives, in one archive, in a group; t2) | later definer | lowest | lowest | lowest; `archive-choice` under `--warn-backrefs` (§4.6) | E1, finding 3 |
| member taken that defines an existing symbol | not taken: links | duplicate | duplicate | duplicate (note under `--warn-backrefs`) | E1 |
| a definer reached by a back-reference from a later member (`main1 libX libY`) | links (never sees it) | depends on member order | duplicate | duplicate (§4.3 case 2) | E2 |
| `--warn-backrefs` when a later archive also defines the key | (no option) | silent | (no option) | `archive-choice` (from the simulation of GNU's walk) | E1 |
| weak definition named after the archive, member strong | member extracted, 2 | 2 | not extracted, 1 | 2 | R1 |
| strong definition named after the archive, member strong | duplicate | duplicate | links silently | duplicate | R1 |
| two lazy definers, the lower weak (`libKW.a libKS.a`) | weak one, 1 | 1 | strong one, 2 | 1 | R3 |
| lazy member before a `--whole-archive` definer | extracted, 2 | 2 | not extracted, 1 | 2 | R4 |
| archive without an index (`ar rcS`) | error ``archive has no index; run ranlib to add one`` | links | links | links (member tables, §4.1) | R8 |
| `--defsym K=k` after the archive defining `k` | ``undefined symbol `k' referenced in expression`` | links | links | links (M4) | R10 |
| strong and common reference, lazy common before lazy data definition | both extracted, 9 | common only, 0 | data only, 9 | both, 9 | R5 |
| weak definition and a common, lazy data definition | extracted, 9 | not, 0 | extracted, 4 | extracted, 9 | R6 |
| common vs lazy data definition | extracts | does not (default; `--help` says it does) | extracts | extracts | E3 |
| common vs lazy function definition | no | no | extracts, program reads code | no | E3 |
| common vs weak definition | common | common | weak | common | E6 |
| common alignment | maximum | maximum | not the maximum | maximum | E6 |
| common sizes | maximum | maximum | first | maximum | seed `common` |
| common vs shared data definition | shared (copy) | common | shared (copy) | shared | E12 |
| common + weak + shared | depends on order, can fail | common | weak | common | E12 |
| equal absolute duplicates | accepted | accepted | duplicate | accepted | E6 |
| TLS reference to non-TLS definition | error | accepted | accepted | error | E6 |
| non-TLS reference to TLS definition | error | NOTYPE accepted, OBJECT error | NOTYPE silent, OBJECT warning (finding 7) | error | E6, R7 |
| TLS reference and non-TLS reference, definition elsewhere | error ``TLS reference in mt.o mismatches non-TLS reference in mn.o`` | accepted | accepted | error | R7 |
| `.hidden` undefined with no use | error ``hidden symbol `zz' isn't defined`` (even `ignore-all`) | accepted | accepted | accepted | R14 |
| reference `foo@V2`, object definition `foo@@V2` (static) | binds | `undefined symbol: foo@V2` | undefined `foo` | binds | R12 |
| hidden symbol in a static executable's `.symtab` | GLOBAL HIDDEN | LOCAL | LOCAL | LOCAL | E6 |
| executable's globals in `.symtab` | GLOBAL | GLOBAL | LOCAL | GLOBAL | seed `symtab` |
| lazy and shared, reference after both | shared | shared | lazy | lazy | E4 |
| archive passed, then reference, then shared | shared | lazy | lazy | lazy | E4 |
| weak reference, `--as-needed` | not needed | not needed | needed | not needed | E4 |
| `--as-needed` library before its reference | dropped | kept | kept | kept | E4 |
| executable and a shared object's undefined symbol | error | error | accepted | error | E20 |
| hidden definition referenced by a shared object | error | error | accepted | error | E20 |
| COMDAT keeper | first loaded | first loaded | lowest live | lowest live | E10 |
| kept copy lacks a symbol a dropped copy defines (t10) | links (keeps the other copy), 140 | 140 | `undefined symbol: hx` | `comdat-mismatch` (§6.3) | finding 2 |
| reference into a dropped copy from a non-alloc section that is not a debug name (`.comment`, `.note.x`, …) | error | accepted | accepted | error | R13 |
| the same from an alloc section with a debug name | error | error | accepted | error | R13 |
| section-symbol group signature | section name | empty name (merges unrelated groups) | section name | section name | E18 |
| reference into a dropped copy from `.gcc_except_table` | 0 | error | addend | 0 | E9 |
| reference to a local of a dropped copy from alloc code/data | error | error | accepted / crash | error | E9 |
| `.gnu.linkonce` | deduplicated | duplicate | duplicate | refused | E11 |
| `SHF_EXCLUDE` allocated section | dropped | dropped | kept | dropped | E18 |
| slim GCC LTO object | refused | ignored (then `main` undefined) | refused | refused | E18 |
| `.gnu.warning.<sym>` | warning, dropped | copied, no warning | copied, no warning | warning, dropped | E21 |
| `--wrap` and a reference in the defining object | not wrapped | wrapped | not wrapped | not wrapped | E13 |
| `--defsym a=b+4` | absolute | section-relative | refused | section-relative | E13 |
| `--require-defined` | supported | not an option | supported | supported | E13 |
| object defines `_end`, `_edata`, `__bss_start` | linker's value | object's | linker's | refused | E14 |
| object defines `_GLOBAL_OFFSET_TABLE_` | linker's | error | linker's | refused | E14 |
| object defines other linker symbols | object's | object's | linker's | object's | E14 |
| linker symbols defined by a shared object (`end`, `__ehdr_start`) | `__ehdr_start` binds to the DSO | linker's | `end`, `etext` bind to the DSO | linker's | E20 |
| `__dso_handle`, `_TLS_MODULE_BASE_`, `__GNU_EH_FRAME_HDR`, `_DYNAMIC` in a static link | not defined | `__dso_handle` (and `_TLS_MODULE_BASE_` on x86) | all defined | §8.1 | E14 |
| unreferenced `_end` etc. in `.symtab` | present | absent | — | absent | E14 |
| copy relocations for two aliases | two copies | one | one | one | E17 |
| copy alignment | section alignment | value-capped (as reported) | value-capped | `min(section, 2^ctz(value))` | E17 |
| verdef parent links | written | not written | not written | written | E16 |
| `VER_NDX_LOCAL` on a defined dynamic symbol | binds | corrupt input | ignored | malformed | E16 |
| versym index out of range | error | error | crash | malformed | E16 |
| exact name in two version nodes | first | first + warning | last | first + warning | E16 |
| `local:` before `global:` in a node | syntax error | accepted | accepted | `spec/script.md` | E16 |
| undefined diagnostics | one line per reference | one per symbol, 3 places | one per symbol, 3 places, not in reference order | one per symbol, 3 places, reference order | E15 |

Bugs of a reference found here, for the harness's list (`spec/harness.md`
"Where the references differ"): mold's common alignment, mold's crash on
a reference into a dropped copy (E9) and on an out-of-range versym (E16),
mold's crash of an `--as-needed` output (E4; not investigated), lld's
section-symbol signatures (E18), lld's stale `--fortran-common` help text
(E3).

## 17. Open questions (with recommendations)

1. **`archive-choice` on by default.** Resolved (ruling B): opt-in under
   `--warn-backrefs`, from a simulation of GNU's walk (§4.6).
2. **Errors for size mismatches.** `common-larger-than-definition` is a
   warning, as GNU ld's. A program that writes past an object is broken;
   recommend keeping the warning (drop-in) and offering
   `--fatal-warnings` as today; revisit with `mortar_odr`.
3. **The common/shared rule (§3.2 rule 2)** departs from a single total
   order to match GNU ld and mold on Fortran's case. Recommended as
   written; the alternative (lld's total order) loses `BLOCK DATA`
   initialization from shared libraries.
4. **Duplicate-symbol wording.** Resolved (policy of 2026-11-14): the
   message keeps lld's form and contains GNU's phrase verbatim, ``duplicate
   symbol: x (multiple definition of `x')`` (§3.8, §14); requested of
   `spec/diag.md` (§18).
5. **`.gnu.warning` for `-static` glibc links.** Recommended as written
   (GNU ld's behavior; 112 sections in `libc.a`); it is cheap (a per-key
   flag at resolution, a check in the scan).
6. **`--no-undefined-version` default** for version scripts (M4): lld's
   default errors, GNU ld's accepts. Recommended: accept with a warning
   (GNU ld's acceptance plus visibility), decided in `spec/script.md`.
7. **`-y` output** (M4): standard output text, not a diagnostic;
   needs a decision in `spec/diag.md` (an informational channel).
8. **Did-you-mean hints** (§5.3): propose a small WP after M1.
9. **`SHT_AARCH64_ATTRIBUTES`**: closed (`spec/layout.md` L-35, §20.7;
   applied in WP-027d): consumed, not merged.

## 18. Requested changes to other specs and tests

Not made by this WP (stage 1 writes only `spec/resolve.md`).

| file, section | current | proposed | reason |
|---|---|---|---|
| `spec/model.md` §3.3 `def` | "`Undefined{weak}`, `Defined{file, st_index}`, `Common{..}`, `Shared{..}`, `Internal{..}`" | add that `Internal` is `Assign(expr)`, `Alias(SymbolId)` or `Provide(name)`; add to the merged properties: `ref_strong`, `defined_dynamic`, `needed` (per shared file), `exported`, `preemptible`, and the verdict | §3.2, §3.6, §5.1, §9.6 need them; the scan and synth read them |
| `spec/model.md` §3.3 `redirect` | "the row references to this key bind to … Every consumer of `sym_map` applies it" | "the row **undefined mentions** of this key bind to (`--wrap`); for a `--defsym` alias, every mention. A defined mention binds to itself" | E13: GNU ld and mold do not wrap a reference in the defining object |
| `spec/model.md` §3.3, last paragraph | "Archive extraction is the least fixed point of …" | "Archive extraction is reachability over a graph fixed by the inputs (`spec/resolve.md` §4.1: `first(K)`, `cfirst(K)` over all files, live or lazy); `def` is then computed over the live files only, and is `Undefined` when no live file defines the key. `def` is not one total order over all candidates: the common/shared rule of §3.2 is an exception" | ruling A; findings 1, 4, 5, 20 |
| `spec/model.md` §3.2 `Inputs` | positional state per file | add each file's `--start-group` region (an index, or none) and its member order within its archive | the simulation of §4.6 needs them (finding 15) |
| `spec/model.md` §3.4 `Assignment` | — | Assignment confirms or withdraws resolution's conditional `Provide` candidates (`__start_`/`__stop_<sec>`, `_TLS_MODULE_BASE_`, `__GNU_EH_FRAME_HDR`); a withdrawn one reverts `def` to `Undefined{weak}` before the scan | §5.1, §8.1 (finding 12) |
| `spec/arch.md`, the scan (phase 70) | — | the scan emits `undefined-symbol`, `comdat-mismatch` and `discarded-section-reference`, per surviving reference after sequence recognition (a relaxed TLS GD/LD call to `__tls_get_addr` does not survive), keyed as `spec/resolve.md` §5.3; a hidden key with a surviving reference is an error whatever the options | ruling D; findings 6, 8, 12 |
| `spec/diag.md` §8.2 wording | lld's `duplicate symbol: x`, `undefined symbol: x` | the message contains GNU's phrase verbatim: ``duplicate symbol: x (multiple definition of `x')``, ``undefined symbol: x (undefined reference to `x')`` | policy of 2026-11-14 (finding 22) |
| `spec/model.md` §3.3 `comdat` | "the file whose group is kept" | add: "the least live `FileId` with the signature" and `(ComdatId, FileId)` pairs of the live copies for `mortar_odr` | §6.1, §6.7 |
| `spec/model.md` §2.3 | "`ComdatId` likewise, over `SHT_GROUP` signatures" | "… signatures, where a signature that is an `STT_SECTION` symbol is the name of its section" | E18; WP-004's open question |
| `spec/model.md` §2.3 | key = name before `@` plus version | add: an undefined `name@V` is its own row; resolution also binds it to a default definition of `name` whose version is `V` (`spec/resolve.md` §9.2) | a reference that requires the default version must find it |
| `spec/model.md` §3.3 `kind` | `Note` covers `.note.gnu.property` | optional: a kind `Property` | synth merges it by kind, not by name |
| `spec/model.md` §12 | archive semantics, version scripts, the COMDAT keeper: open | closed by `spec/resolve.md` §4, §9, §6.1 | |
| `spec/diag.md` §3.1 | 15 kinds | add the new kinds of §14 (area "symbols"), each with its test, among them `comdat-mismatch` (an ODR signal, §6.3) and the opt-in `archive-backref`, `archive-choice` | §14, ruling C |
| `spec/cli.md` §4 rows | `--fortran-common` "(default)" (lld's help) | the row's help: "mortar's behavior, always on"; status `supported` at M1; `--no-fortran-common` `refused` | E3: lld 22's default does not extract |
| `spec/cli.md` §4 rows | `--warn-backrefs`, `--warn-common`, `--require-defined`, `-u`, `-e`, `--allow-multiple-definition`, `-z muldefs`, `--unresolved-symbols`, `--warn-unresolved-symbols`, `--error-unresolved-symbols`, `--ignore-unresolved-symbol`, `--start-group`/`--end-group` (no-op for resolution) | `supported` when WP-022 stage 2 lands | M1 rules of this spec |
| `spec/input.md` (WP-021) | — | interns `ComdatId` by the effective signature (§6.1); refuses slim GCC LTO objects or leaves it to resolution (§10: one place, to be agreed) | §6.1, §10 |
| `spec/harness.md` "Where the references differ" | the archive back-reference row says "as bfd (open question)" | "resolved (as lld, mold): `spec/resolve.md` §4.3"; add the rows of §16 the new link tests meet | |
| `tests/link/resolve/archive-backref.test` | `link(bfd,mortar)` … `exit: fail`, `diag(mortar): helper`; `link(lld,mold)` … `run(lld,mold) prog` exit 7; comment "mortar follows GNU ld" | `link(bfd)` … `exit: fail` / `diag(bfd): undefined reference to `helper'`; `link(lld,mold,mortar)` … with `run(lld,mold,mortar) prog` `exit: 7`; with `--warn-backrefs` on mortar's line a `diag(mortar): backward reference to an archive member: helper`; comment: "mortar follows lld and mold (resolve.md §4.3); GNU ld rejects it" | the supervisor's decision, §4.3 |
| `tests/link/resolve/*.test`, `tests/link/diag/*.test` | `#@ spec: resolve.md "Archives"` etc., and `diag.md "Duplicate symbols"` / `"Undefined symbols"` (sections that do not exist) | `resolve.md §4.1`, `§3.2`, `§3.3`, `§6.1`, `§3.8`, `§5.2` | the sections now exist |
