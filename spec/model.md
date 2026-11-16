# spec: the model (`mortar_core`)

Status: **normative** (WP-002): frozen after an independent design review
and review against the reader API of WP-004 and the `Plan` of WP-008. The
code is WP-020. `Demands` (§3.7), `Slots` (§4.3) and `patches` (§4.4) are
confirmed with `spec/arch.md` (WP-013), whose requested amendments M-1 to
M-8 are made (WP-020c, `spec/landed/model/WP-020c.md`); so are the
requests of `spec/input.md` §17 and `spec/resolve.md` §18 (WP-020d,
`spec/landed/model/WP-020d.md`). Any later amendment is made here first
and the code follows.

The model is what the phases of a link exchange: ids and tables. It holds
no algorithm. A phase is specified by the tables it reads and the table it
writes (`docs/ARCHITECTURE.md`, "Pipeline"), so this document is the
contract between every pair of phases, between a full and an incremental
link, and between the linker and its link state on disk.

## 1. Conventions

- **Widths.** Addresses, sizes, offsets and symbol values are `u64` in
  every table, whatever the ELF class; alignments are stored as a power of
  two (`u8`). The class is a property of the link (§3.2). Narrowing happens
  in the writers only, and a value that does not fit the class is an error
  there (an image larger than a 32-bit address space is diagnosed, never
  truncated).
- **Names are bytes.** Symbol, section and file names are `[u8]`, compared
  bytewise. Only rendering (`spec/diag.md` §5) decides how to show them.
- **Ids are `u32`.** Every id space is limited to 2^32 − 1 rows; a link that
  needs more is refused by name (`Refused{Feature, "more than 2^32 - 1
  <rows>"}`), never wrapped.
- **Rows are plain data.** A table holds fixed-width integers, ids and
  byte strings. No table holds a reference into another table, a pointer, a
  closure, or a container whose iteration order is unspecified (§6.5).
- **Input bytes belong to the input store.** A phase obtains the bytes of
  a file or a section through `Inputs` by `(FileId, range)` or
  `SectionId`: as a slice when the range lies in one resident part
  (`Inputs::resident`), otherwise copied into a buffer of the caller
  (`Inputs::copy_to`). The store is created before `mortar_input` runs,
  outlives every table, and is shared and immutable to its users (a
  `&'s` borrow, `StoreRef<'s>`); which ranges are resident is a function
  of the file's bytes and the store policy (`spec/input.md` §11). Whether
  the store reads a range again when asked (non-alloc debug sections at
  fill time) or keeps an owned buffer (a `SHF_COMPRESSED` input section
  is decompressed once into one) is the store's business and invisible to
  the tables.

## 2. Ids

| id | one per | numbered |
|---|---|---|
| `FileId` | loaded file: the internal file, each relocatable object (given directly or an archive member), each shared object | §2.1 |
| `ArchiveId` | archive mention | one per archive mention, in load order (`spec/input.md` §10.1) |
| `SymbolId` | global symbol row (§2.3) | §2.3 |
| `ComdatId` | distinct COMDAT group signature | as `SymbolId`, over group signatures |
| `SectionId` | section-header-table entry of a **live** relocatable object, index 0 included | `section_base[file] + sh_index` (§2.2) |
| `OutSecId` | output section (§3.4) | order of first assignment (§3.4) |
| `FragId` | distinct piece of mergeable content within one output section (§3.5) | order of first occurrence |
| `ChunkId` | unit of the output that layout places as a whole (§4.4) | creation order (§4.4) |
| `SegmentId` | program header of the output | output order |

A local symbol has no id of its own: it is `(FileId, st_index)`, written
`LocalSym`. Where a table is keyed by "a symbol, local or global"
(GOT-like slots, §4.3), the key is `SymKey = Global(SymbolId) |
Local(FileId, u32)`, ordered globals first, then by `(FileId, index)`.

Ids are dense and **per link**: the same file may have another `FileId` in
the next link. What survives between links is identity (§7.1).

### 2.1 Files

`FileId(0)` is the **internal file**: it has no sections and owns the
symbols the linker defines (`_end`, `__ehdr_start`, `__start_<name>`,
`--defsym`, script assignments).

The other files are numbered in **load order**, defined sequentially:
walk the `Plan`'s inputs in command-line order; an object takes the next
id; an archive contributes its members that are relocatable objects, in
archive order, each taking the next id whether or not it is ever
extracted; a shared object takes the next id; a linker script is replaced
by its input list, in place. `spec/input.md` owns the details (search,
the same file named twice, thin archives, `--start-lib`); this spec
requires that the numbering be a function of the `Plan` and of file
contents only, and equal to this sequential walk however files are read.

A file that is never extracted costs its `files` row and its entries in
`sym_map` and nothing else: no section rows (§2.2).

`FileId` order is the **priority order** of symbol resolution
(`spec/resolve.md`) and the unit order of every per-file parallel phase.

### 2.2 Sections

Section ids are assigned **after resolution**, to live files only:
`section_base` is the prefix sum of `e_shnum` over the live relocatable
objects in `FileId` order. A file's sections occupy one contiguous range
in section-header order, with no compaction inside the file:
`SectionId − section_base[f]` *is* the ELF section index, so a symbol's
`st_shndx`, a relocation section's `sh_info` and a group's member list
address rows without a lookup. Rows of sections that carry no output
(symbol and string tables, relocation sections, groups) exist and are
marked (§3.3); a wide column MAY be kept for output-carrying sections
only, behind a dense secondary index (§9).

Contiguity per file is what lets a parallel phase hand each worker a
disjoint mutable slice of a column (§6.1).

### 2.3 Global symbols

A `SymbolId` row is a **global symbol name**, not a definition. The key is
the name before any `@`, plus a version: an unversioned mention and a
default-version mention (`foo`, `foo@@V2`) are the same row; a
non-default version (`foo@V1`) is a row of its own. Which version a row's
definition carries, and versions assigned by a version script, are
columns of `Resolution`. `spec/resolve.md` specifies the rule in full; the
key shape is fixed here because interning happens before resolution.

One row exists per distinct key among the non-local symbol-table entries
of all files (lazy archive members and shared objects included).

The key is split at the **first** `@`: `foo@@V2` is the unversioned key
`foo`, `foo@V1` is `foo` with version `V1`, and `foo@` is `foo` with the
empty version, a key of its own, distinct from `foo`. A shared object's
version comes from its version tables, not from the name.

Numbering is defined sequentially: files in `FileId` order, each file's
non-local entries in symbol-table order, a key seen for the first time
taking the next id. `ComdatId` likewise, over `SHT_GROUP` signatures in
`(FileId, sh_index)` order, where a signature that is an `STT_SECTION`
symbol is the name of its section (the effective signature,
`spec/resolve.md` §6.1).

An undefined `name@V` is a row of its own like any non-default version;
resolution also binds it to a default definition of `name` whose version
is `V` (`spec/resolve.md` §9.2). The key shape does not change for it.

A parallel implementation MUST produce the same numbering. The intended
one:

0. **Sizing.** A concurrent table cannot grow while it is shared, so it is
   sized first from an estimate of the number of distinct keys (a pass over
   every key; WP-020b uses a HyperLogLog sketch). If the estimate falls
   short, pass 1 runs again with a larger table; the numbering is the same
   either way. Key extraction is therefore **repeatable**: the input phase
   hands the numbering each file's keys in order as a function it may call
   more than once, a read of the input bytes, not a buffer.
1. Per file in parallel, intern each key in the table, whose value holds the
   key's first mention, updated with an atomic minimum of `FileId << 32 |
   st_index`, or equally of `FileId << 32 | n` with `n` the entry's ordinal
   among the file's non-local entries (the same order, since those entries
   are in table order); leave the table slot in `sym_map`.
2. Per file, count the entries that are their key's first mention;
   prefix-sum the counts in `FileId` order.
3. Two steps with a join between them, because an id is written by its
   key's first-mention file and read by every file: (a) per file, give
   the first-mention entries their ids (the concatenation of pass 2 in
   `FileId` order is the list of slots by id); then (b) rewrite `sym_map`
   from slots to ids, in fixed-size blocks.

Every order that is observable then follows from ids, with no sort by
first mention later.

## 3. Derived tables

A **derived table** is a pure function of the tables before it: it records
no choice. Recomputing it from the same inputs gives the same rows.

### 3.1 `Plan` (written by `mortar_cli`)

The command line as data: `spec/cli.md` §3 specifies its fields.
Immutable. Each input keeps its argument index; that index is the `unit`
of diagnostics about the command line and the root of load order.

The `Plan` type and the types of its fields are defined in `mortar_core`
(module `plan`), so that every phase and the link state name them without
depending on the parser; `mortar_cli` writes a `Plan` and re-exports the
types. A `Plan` is what the user wrote, not what it means: values that
need the target or a file to be judged (`-z max-page-size=`, the options
of one target when no `-m` is given, `-R` naming a directory) are kept as
written and judged by the phase that uses them, which reports them under
its own phase ordinal (§8) with the argument index as the unit.

### 3.2 `Inputs` (written by `mortar_input`)

Everything known from reading files, before any symbol is resolved.
Immutable once the phase ends. The `DT_NEEDED` closure (`spec/input.md`
§8.6) is loaded by this phase; no later phase adds a file.

| part | row | columns |
|---|---|---|
| `target` | one | machine, class, relocation form (RELA, or REL with implicit addends); decided by `spec/input.md` §3.1 when `-m` did not; every ELF file must agree (§3.3), except candidates of a search, which are skipped (§4.4) |
| `files[FileId]` | file | kind (internal, object, shared object); display name (`FileRef`: path, archive and member); the found path; `ArchiveId`, member name, ordinal and offset, if any, members of one archive mention being contiguous in archive order; the mention that named it (`origin`: a `Plan` input and argument, the script entry, or the closure edge) and the positional state of its own mention (as-needed, whole-archive, lazy, static, copy-dt-needed, exclude-libs; a shared object: its first mention's); the parsed reader over the file's resident parts (`spec/input.md` §11.3); where its bytes are in the store; section count; the range of `sym_map`; identity and content hash (§7.1); the list of its `Meta` sections (Mason containers, `spec/meta.md` MT-6), so that `mortar_odr` finds the units without a rescan |
| `sym_map` | non-local symbol-table entry of each file, per file in table order | the `SymbolId` of its key |
| `symbols[SymbolId]` | key | name bytes, hash, version part |
| `comdats[ComdatId]` | signature | name bytes |
| `archives[ArchiveId]` | archive mention | found path, kind, the mention, its members (a `FileId` range) |
| `link` (`LinkInputs`, `spec/input.md` §10.4) | per link | each file's `--start-group` region (`group`: an ordinal from 1 in load order, or none); the soname units, one per loaded shared object, with every mention and the merged `as_needed` and `copy_dt_needed`; the closure edges `(needing, index)` and what satisfied each; `opened`, every file opened and what it became; `probes`, every candidate examined by a search (§7.1); `script_entries`; `directives` (`OUTPUT`, `ENTRY`, `EXTERN`); `rpath_from_r` |

Invariant **I-file**: the rows of a file (its `files` row, the keys it
mentions, and later its section rows of §3.3 relative to its base) are a
function of that file's bytes and its own mention (the `Plan` input or
script entry that named it, with that position's state) alone. This is
what an incremental link reuses for an unchanged file. Properties merged
over several mentions or numbered over the load order (`spec/input.md`
§10.4) are in `Inputs.link`, which I-file does not cover; an incremental
link recomputes it whole.

`Inputs::check` holds the invariants this table shows alone, numbered as
its messages are: (1) only `FileId(0)` is the internal file, with origin
`Internal`, no bytes, sections or symbols, and only objects have
sections, as many as their reader; (2) the `sym_map` spans are
contiguous in `FileId` order and cover it; (3) a member is an object of
an archive in range with that archive's path, inside its `members`
range, in increasing header offset, archives following one another in
`FileId` order; (4) `LAZY` (`--start-lib`) only on an object that is not
a member; (5) files of `Plan` inputs come in input order, closure files
after all others, each a shared object needed by an earlier shared
object; (6) `SymbolId`s are numbered by first mention (§2.3); (7)
`group` has a row per file, ordinals from 1 first appearing in
increasing order; (8) one soname unit per shared object in `FileId`
order, each with a mention; (9) closure edges sorted by `(needing,
index)`, between shared objects; (10) every index into `opened`,
`script_entries` and `closure` is in range and names a row of the right
kind.

Relocations and symbol-table entries are **not copied** into any table:
phases read them from the input bytes through the reader each time they
need them (§5).

### 3.3 `Resolution` (written by `mortar_resolve`)

| part | row | columns |
|---|---|---|
| `file_live[FileId]` | file | whether the file takes part in the link and why: `Dead`; `Always`; `Extracted` by the reference `(FileId, st_index)` that pulled the lazy object in (a root, `spec/resolve.md` §4.2: file 0 and the root's argument index); `Shared{needed, used}`, a loaded shared object (of the command line, a script, or the `DT_NEEDED` closure) that takes part: `needed` when it is `DT_NEEDED` of the output (the set N of `spec/dyn.md` D-11), `used` when it is not needed but is reached by a `DT_NEEDED` edge from a needed or used object, whatever loaded it (the set U of D-12), never both; every other loaded shared object (an unneeded as-needed object that nothing needed or used leads to, a closure file nothing reaches) is `Dead` [M2] |
| `def[SymbolId]` | key | the resolved definition: `Undefined{weak}`, `Defined{file, st_index}`, `Common{file, st_index}`, `Shared{file, st_index}`, `Internal(..)` (linker-defined: `Assign`, an assignment by its argument index or script position; `Alias(SymbolId)`, `--defsym A=B`; `Provide{conditional}`, a symbol of `spec/resolve.md` §8.1, `conditional` when its existence depends on output sections, §3.4); and the properties merged over every live mention (`spec/resolve.md` §3.6, §9.6): visibility, `ref_regular` (referenced from a relocatable object or a root), `ref_dynamic` (referenced from a needed or used shared object, a member of N ∪ U, computed after the needed/used fixed point: `spec/dyn.md` D-12), `ref_strong` (some reference not weak), `defined_dynamic` (a needed or used shared object defines it, D-12), `exported`, `preemptible`, binding for export, version, and the **verdict** of a surviving reference (`spec/resolve.md` §5.2: bound, error, warning, ignored) |
| `common[SymbolId]` | key with a common mention | size and alignment: the maximum over the live common mentions |
| `clash[SymbolId]` | key | whether two or more live definitions tie at the strongest rank (a duplicate definition, reported with both places) |
| `redirect[SymbolId]` | key | the row the **undefined mentions** of this key bind to (`--wrap`); for a `--defsym` alias, every mention. A defined mention binds to itself; itself otherwise. Every consumer of `sym_map` applies it once |
| `comdat[ComdatId]` | signature | the file whose group is kept: the least live `FileId` with the signature (`spec/resolve.md` §6.1) |
| `comdat_copies` | live copy of a group | `(ComdatId, FileId)` pairs, sorted; the keeper first; for `mortar_odr` (`spec/resolve.md` §6.7) |
| `section_base[FileId]` | live object | §2.2 |
| `exported`, `imported`, `ref_dynamic`, `defined_dynamic` | key (bit sets; empty in a static link) | the dynamic columns (`spec/dyn.md` D-11, D-12, D-21): `exported`, a program definition (`Defined`, `Common`, `Internal`) in `.dynsym`; `imported`, a key whose `def` is `Shared` that a live object mentions; `ref_dynamic`, referenced by a member of N ∪ U; `defined_dynamic`, defined by one. `def`'s `Shared` entry is `bind(k)` (D-11: the first member of N ∪ U in loader order with a visible definition, never `giver(k)`), read through `Resolution::bind` |
| `sections[SectionId]` | section | `kind` (below); the relocation section(s) applying to it; its `ComdatId`, if any; `state`: `Kept`, or `Dropped(reason)`: COMDAT group not kept (with the keeping file), excluded, consumed by the linker (the kinds without output), stripped by option |

`kind` classifies a section once, from its type, flags and name, for every
later phase: `Regular` (bytes copied and relocated), `Nobits`, `Merge`
(strings or fixed-size constants), `EhFrame`, `Note`, `Debug` (non-alloc,
handled as data), and the kinds that carry no output of their own:
`Null`, `Symtab`, `Strtab`, `Reloc`, `Group`, `Stack`
(`.note.GNU-stack`), `Meta` (a Mason container, `spec/meta.md` MT-6;
`Dropped(Consumed)`, read by `mortar_odr` through `mortar_meta`),
`Excluded` (`SHF_EXCLUDE`). A section whose type or flags mortar does not implement
is not classified: it is refused by name. Classification depends on the
file alone (I-file) and may be computed while the file is parsed and kept
per file until ids exist.

Every merge in this table is order-independent (§6.2): `def` is chosen
over the live candidate definitions of a key by `spec/resolve.md` §3.2
(rank, then `FileId`, then symbol index; not one total order over all
candidates: the common/shared rule there is an exception); `common` is a
maximum; `clash` a saturating count at the chosen rank; the properties
are unions or most-constraining values. Archive extraction is
reachability over a graph fixed by the inputs (`spec/resolve.md` §4.1:
`first(K)`, `cfirst(K)` over all files, live or lazy); `def` is then
computed over the live files only, and is `Undefined` when no live file
defines the key. Both are independent of the order in which they are
reached.

`Resolution::check` holds what the table shows alone: (1) per-key and
per-file columns of the right length; (2) section ranges contiguous in
`FileId` order, sections only for live files; (3) definitions,
extracting references and kept groups in live files, `redirect` and
aliases in range, `common` sorted; (4) no section of a kind without
output kept, a section dropped for its group in one; (5) `comdat_copies`
sorted and live, the keeper the least copy; (6) an undefined key with a
strong reference is not weak, a `Shared` definition is preemptible;
(7) each dynamic column empty or per key, an import's `def` `Shared`, an
export's the program's.
`Resolution::check_against(&Inputs)` adds: (1) one row per file, key and
signature; (2) `Shared` liveness only for a loaded shared object, with
exactly one of `needed` and `used`, `needed` only for a shared object of
the command line or a script (a closure file is never needed,
`spec/dyn.md` D-11), `Extracted` only for a lazy object, `Always` for a lazy object only under `--whole-archive`; (3) a
`Defined` or `Common` definition in an object, a `Shared` one in a
shared object.

### 3.4 `Assignment` (written by `mortar_layout`, first stage)

Which output section each kept section belongs to. It runs before
liveness and merging because both need it: a script's `KEEP` and
`/DISCARD/`, `__start_<name>`/`__stop_<name>` references keeping the
input sections of output section `<name>`, and merging "within one output
section".

| part | row | columns |
|---|---|---|
| `out_of[SectionId]` | section | `OutSecId`, or `Consumed` (a kept section whose contents layout or synth consume into a synthetic chunk, such as `.note.gnu.property`, `spec/layout.md` L-34), or `Discard` (a script's `/DISCARD/`: the section's state becomes `Dropped(by script)`), or none for sections without output |
| `outsecs[OutSecId]` | output section | name, type, flags, entry size class for merging; from a script: `KEEP` on the matching pattern, fill, constraints, the script position that orders it |
| `keep[SectionId]` | section | whether it is a root of liveness by rule or by `KEEP` |
| `withdrawn` | withdrawn symbol | the conditional `Provide` candidates of resolution withdrawn here, sorted |

Without a script the rule is the default mapping of `spec/layout.md`
(by name prefix, type and flags). A section dropped because its COMDAT
group is not kept may also carry the `OutSecId` its name maps to
(`spec/layout.md` L-42), so that `__start_`/`__stop_` and the output
section set do not depend on which copy of a group won; it has no place.
`Discard` is exactly the sections a script drops. `OutSecId`s are numbered in order of
first assignment `(FileId, sh_index)`, after any the script declares, in
script order. The order of output sections in the file and the order of
input sections inside one are **not** here: they are decisions (§4.4).

Assignment confirms or withdraws resolution's conditional `Provide`
candidates (`__start_`/`__stop_<sec>`, `_TLS_MODULE_BASE_`,
`__GNU_EH_FRAME_HDR`; `spec/resolve.md` §5.1, §8.1): a withdrawn one
reverts `def` to `Undefined{weak}` before the scan, weak unless a
reference to it is strong (`Assignment::def`). Every later reader of
`def` reads it through that function.

### 3.5 `Split` (written by `mortar_merge`, first stage)

Sections whose content is handled in pieces: `Merge` sections (strings,
fixed-size constants) and `EhFrame` sections (CIE and FDE records).

- per split section, a range into two columns with one entry per piece:
  the piece's offset in the section (`u32`) and, for `Merge`, its `FragId`;
- `frags[FragId]`: the output section, the content (as a place in some
  input section), size and alignment. Two pieces have the same `FragId`
  exactly when they belong to one output section and have the same merge
  class (`SHF_STRINGS` and `sh_entsize`), equal alignment and equal bytes
  (`spec/synth.md` S-13). `FragId`s are numbered by first occurrence
  `(FileId, sh_index, piece)`: the numbering §2.3 defines, computed by the
  partitioned method of `spec/synth.md` S-28 (occurrences partitioned by
  shard in key order, each shard interned on one thread; the first insert
  is the first occurrence, so no atomic minimum is needed);
- for `EhFrame`: per piece, CIE or FDE, and for an FDE the section its
  code range lies in (from its first relocation) and its CIE piece.

A reference into a split section (commonly an `STT_SECTION` symbol plus an
addend, which on a REL target is read from the section's bytes) is
resolved to a piece by offset through this table.

This is the largest table of a debug link; its row shape (8 bytes per
piece occurrence) is part of the contract (§9).

### 3.6 `Liveness` (written by `mortar_live`)

- `live[SectionId]: bool` after `--gc-sections` from the roots (entry,
  exported symbols, `keep`, start/stop references); all `Kept` sections
  when it is off;
- `frag_live[FragId]: bool`: referenced from a live section;
- an FDE is live exactly when the section its code lies in is live and
  not folded away (§4.1); this is a rule, not a column.

### 3.7 `Demands` (written by `mortar_arch` scan)

What the relocations of live sections require, with no placement:

- per `SymKey`, a set of flags: needs a GOT slot (`GOT`), a PLT entry
  (`PLT`), an IPLT entry (`IPLT`: a non-preemptible IFUNC, whose every
  address is then that entry), a TLS descriptor (`TLS_DESC`), a TLS
  module/offset pair (`TLS_GD`), a thread-pointer offset slot
  (`GOT_TPREL`), a copy relocation (`COPY`), a canonical PLT address
  (`CANON_PLT`), export to the dynamic symbol table (`EXPORT`); and the
  inhibitors of two relaxations: some use forbids the GOT relaxation
  (`GOT_NORELAX`), some use forbids the IE-to-LE relaxation
  (`TLSIE_NORELAX`);
- per output, a set of flags: the GOT base is referenced (`GOT_BASE`), the
  local-dynamic module slot (`TLS_LD`), static TLS (`STATIC_TLS`, written
  as `DF_STATIC_TLS`);
- per section, the number of dynamic relocations it will carry, by kind;
- the `WeakDyn` keys (`weak_dyn`, a bit set over `SymbolId`, empty in a
  static link): the undefined weak keys of a dynamic executable that get a
  `.dynsym` entry and a dynamic relocation (`spec/dyn.md` D-25, decided per
  key by the scan's `weak` step);
- errors found on the way (a relocation that cannot be satisfied in this
  kind of output), reported at phase 70 (§8) and not stored.

Every flag, per key and per output, is the union over all relocations that
raise it: a bitwise or (§6 rule 2), commutative and idempotent, never
cleared within a link. A flag means "some valid layout may use this"; for
the two inhibitors, "some use forbids this rewrite". `spec/arch.md` §4.2
names the flags and §4.3 says which relocation raises which.

The flags of one key follow from one target kind (`spec/arch.md` §2.1), so
some never occur together: `IPLT` with `PLT`, `CANON_PLT` or `COPY`;
`COPY` with `CANON_PLT`. A local symbol demands neither `COPY`,
`CANON_PLT` nor `EXPORT`.

**Demand is conservative**: a slot is demanded whenever some valid
layout would use it. A relaxation that depends on addresses (a distance,
an alignment, any value the layout gives), or on `Demands` themselves,
never removes a demand; one that depends only on `Resolution`, the output
kind, and the bytes and relocation records of its own input section may
(a rule of kind 1 in `spec/arch.md` §5.2; one of kind 2 raises the
demands of both forms).

## 4. Decision tables: the layout

A **decision table** records choices the inputs do not force: where
things go and in what order. Any value that satisfies the table's
validity rules yields a correct output; the default policy chooses one
deterministically, an incremental link chooses another (as much of the
previous one as still fits). Together the decision tables are **the
layout** of principle 7.

### 4.1 `Folds` (written by `mortar_live`, identical code folding)

`folded[SectionId]: SectionId`, the section a folded one is replaced by;
absent when folding is off. Valid when each folded section is equivalent
to its target by the rule of `spec/live.md` (checked again when a layout
is loaded: an edit can break an equivalence), and targets are not
themselves folded. It is a decision so that an incremental link is not
forced to refold, and so move, unrelated sections.

### 4.2 `MergeMap` (written by `mortar_merge`)

`frag_off[FragId]`: the offset of each live fragment in its merged chunk.
Valid when live fragments of one output section do not overlap and honor
their alignment. The order is by merge class, then alignment, then
`FragId` (`spec/synth.md` S-24); never by a hash table's layout (§6.5).
Fragments never overlap: there is no tail merging (`spec/synth.md`
S-23).

### 4.3 `Slots` (written by `mortar_synth`)

The content order of every synthetic table whose size does not depend on
addresses: the index of each slot of each `SymKey` that demands one, per
slot kind (address, `GOT`; thread-pointer offset, `GOT_TPREL`;
module/offset pair, `TLS_GD`; descriptor, `TLS_DESC`), and of the
output's local-dynamic module slot (`TLS_LD`, one per output); the PLT
index of each `SymKey` that demands `PLT` or `CANON_PLT`, and the IPLT
index of each that demands `IPLT`; the order of `.dynsym`; of dynamic
relocations; of version definitions and needs; of `DT_NEEDED` entries
(`spec/dyn.md` D-19a's order, computed by `mortar_resolve`); of
live `.eh_frame` records after deduplication of CIEs; copy-relocation
allocations, one per `(shared FileId, st_value)` with every `SymbolId`
aliasing that address mapped to it, each with its place (`.dynbss` or
`.bss.rel.ro`), alignment and record alias (`spec/dyn.md` D-42); and the
size each such chunk therefore has.

The dynamic lists (`spec/dyn.md`; all empty in a static link):

| field | content | rule |
|---|---|---|
| `needed` | the needed shared objects, in `DT_NEEDED` order | D-11, D-19a |
| `dynsym`, `dynsym_unhashed`, `gnu_hash_buckets` | `.dynsym` after the null entry (imports, exports, copy aliases, `WeakDyn` keys): the unhashed entries first in `SymbolId` order, then the hashed by (bucket, `SymbolId`); the unhashed count; `.gnu.hash`'s `nbuckets` | D-21, D-22, D-26, D-28 |
| `copies` | per allocation `(file, value)` (its identity and sort key), `place` (`DynBss` or `RelRo`), `align`, `alias` (the key the `COPY` record names) | D-42 |
| `verneed` | per needed object with a version an import binds to, in `needed` order: its versions by increasing definition index (`VerIndex`) | D-27 |
| `dynstr` | the `.dynstr` offset of every name the dynamic sections write (per `.dynsym` entry, per `needed` entry, per version of `verneed`, the run path) and the size | D-24 |

An index counts within its kind: one key can hold slots of several kinds
(`spec/a64.md` §10.1), each with its own index, and a key has a PLT index
or an IPLT index, never both. Where the kinds lie relative to each other
in the GOT, and which `.got.plt` word belongs to a PLT or IPLT entry,
follow from the indices and from each kind's capacity (`spec/synth.md`
S-63) by that spec's rule. A capacity equals the kind's count except in
an incremental link [M7], which stores it.

`.symtab` (and the symbol tables of `-r` output) is ordered by rule, not
by rows (`spec/synth.md` S-74, S-75): the internal file's locals, then
per file in `FileId` order its kept locals in table order followed by its
demoted globals (output binding `STB_LOCAL`) in `SymbolId` order, then
the kept globals in `SymbolId` order, so that the output index of
`(FileId, st_index)` is a prefix sum per file. Only the per-file `(first
local, count)` ranges are stored; `count` includes the demoted globals.

Valid when every demand of §3.7 has exactly one slot of its kind (the
module slot exists exactly when `TLS_LD` is demanded), no slot is given
twice, `.dynsym` holds exactly D-21's set (every import, export, `EXPORT`
demand, `WeakDyn` key and copied key, plus copy aliases) with the
unhashed block inside it, each copy allocation's `alias` maps to it and
its alignment is a power of two, `verneed` follows `needed` with
non-empty increasing versions, `.dynstr`'s offsets match the lists they
name and lie inside it, and the orders satisfy the format's own rules (the `.gnu.hash`
bucket order in `.dynsym`; ...), listed in `spec/synth.md`.

### 4.4 `Placement` (written by `mortar_layout`)

| part | row | columns |
|---|---|---|
| `chunks[ChunkId]` | chunk | kind: file header, program headers, section headers, part *k* of the body of an output section (`Body(OutSecId, k)`), the patch island after part *k* (`Patches(OutSecId, k)`), a merged section, a synthetic section (`spec/synth.md`) including the RELRO padding (`.pad.relro`, below), and for `-r` the group and relocation chunks of an output section; name, type, flags, alignment; `size`, `capacity ≥ size`; `addr` (none for a non-alloc chunk), `offset` (file); the output section index |
| `order` | — | the chunks in file order |
| `segments[SegmentId]` | program header | type, flags, offset, address, file size, memory size, alignment |
| `place[SectionId]` | section | `(ChunkId, offset in chunk)`, or none; a folded section has the place of its target |
| `sym_place` | symbol-anchored allocation: a common symbol, a copy-relocation allocation | `(key, ChunkId, offset, size)`, sorted |
| `defined` | linker-defined symbol | `(SymbolId, value or (ChunkId, offset))` |
| `patches` | code the linker adds because of addresses | `(ChunkId, offset, kind, target)`: a range-extension thunk to `Sym(SymKey, addend)` or `Plt(SymKey)`; an erratum patch (AArch64 843419), keyed by the `(SectionId, offset)` of the instruction it replaces; and the redirect list `(SectionId, offset in section) → patch`, sorted, at most one per site |
| `sized` | address-dependent synthetic content | the size, and the encoding inputs `spec/synth.md` names, of chunks whose content depends on addresses: `.relr.dyn`, patch chunks, a compressed non-alloc chunk (sized by a pass that produces its bytes; such chunks follow every alloc chunk) |
| `tls` | one | start, size and alignment of the TLS template |
| `entry` | one | the entry address |

`ChunkId`s are given in a fixed creation order: the file header, the
program headers, the section headers, then synthetic chunks in the order
`spec/synth.md` lists them, then the RELRO padding, then per `OutSecId` in
order its chunks: `Body(o, 0)`, `Patches(o, 0)`, `Body(o, 1)`,
`Patches(o, 1)`, …, and its merged chunk. Each part's island follows the
part (`spec/layout.md` L-110, L-111). `order` is the decision; the id is
only a name. The synthetic list is `spec/synth.md` S-3; `.eh_frame` is
one of its synthetic chunks (its input sections are split and have no
`place`). (Rust form: `ChunkKind::Body(Part)` and `Patches(Part)` with
`Part { out, part }`; the padding is `Synthetic(Synth::RELRO_PADDING)`.)

The RELRO padding is a `SHT_NOBITS` synthetic chunk named `.pad.relro`
with `relro: true`, whose size stage D writes so that the RELRO run ends on
a maxpage boundary (`spec/layout.md` L-64). When there is a RELRO run it is
its last chunk.

`capacity − size` is slack. The default policy leaves none; an incremental
policy leaves and consumes it. Nothing else distinguishes the two. Slack
lies in no section header and under no symbol: section sizes and symbol
values are computed from `size` (`spec/layout.md` L-86, L-92, L-141).

A `defined` row `(ChunkId, offset)` on a header chunk (file header or
program headers, as for `__ehdr_start`) is written with `st_shndx` 1, the
first output section, as GNU ld does (`spec/layout.md` L-91).

`entry` is none after the `entry-not-found` warning and for `-shared` or
`-r` with no entry symbol; the image then writes `e_entry = 0`
(`spec/layout.md` L-103).

Layout is a fixed point: patches move code, which may need more patches,
and change the sizes in `sized`. The table records the fixed point, not
the iteration. The iteration only adds patches (`spec/arch.md` §6), so a
patch can end up used by no site: such a patch stays, keeps its place and
its size, and is filled like any other.

**Validity** (`Layout::check`, run in tests on every layout mortar
computes and in production on every layout loaded from a link state,
before it is trusted). A layout mortar computed that violates a rule is a
bug of mortar's, reported as `internal-error` (`spec/diag.md` §3.2); a
loaded one that violates a rule is not reused (phase 15, `spec/incr.md`):

1. every live, output-carrying section has exactly one place; places
   inside a chunk do not overlap, lie within `size`, and honor the
   section's alignment relative to the chunk's address;
2. chunks do not overlap in the file or in memory; `addr ≡ offset` modulo
   the page size for loaded chunks; alignments hold; non-alloc chunks
   have no address;
3. `segments` equal what `derive_segments` (`mortar_core::layout`,
   `spec/layout.md` L-125) derives from the chunks, their order and the
   segment rules (class, maxpage, stack flags; the script's `PHDRS` when
   scripts land); a `PT_LOAD`'s `p_align` is `P` of `spec/layout.md` L-67,
   the larger of maxpage and its chunks' alignments; in particular the TLS
   initialized chunks are directly followed by the uninitialized ones,
   `tls` describes them, and `tls.start` is a multiple of `tls.align` (the
   loader computes the executable's thread-pointer offsets from the
   alignment alone, `spec/a64.md` §12.2), and so are `PT_TLS`'s `p_offset`
   and the first TLS chunk's file offset when it is `PROGBITS`
   (`spec/layout.md` L-76, L-128); the RELRO run is contiguous, ends with
   the RELRO padding and on a maxpage boundary (L-129); a copy-relocation allocation lies in
   writable memory with the alignment of the shared object's symbol;
4. (4a, needs the relocations) every site that needs a patch has one in range (a branch out of range
   of its target; an instruction at an address the erratum rule names);
   every redirect names a patch of its kind: a thunk redirect names a
   thunk of the key its site needs (and a `Plt(key)` thunk's key has a PLT
   or IPLT index), an erratum redirect names an erratum patch keyed by the
   redirect's own site; every erratum redirect names a site of that
   erratum at the layout's addresses; a patch no site uses is allowed;
   (4b, the island policy, from the tables) an erratum patch lies in the
   island `Patches(o, k)` of the part `Body(o, k)` its site is placed in,
   and one island holds at most one thunk per `(kind, key, addend)` and one
   erratum patch per site (`spec/layout.md` L-111, L-112, L-126);
5. `sym_place`, `defined`, `sized` and `entry` are present for exactly the
   symbols and features that need them, and a `defined` offset is at most
   the `size` of its chunk.

Rule 6 of earlier drafts (every relocated field holds its value at these
addresses) is not a rule of the layout: it is an obligation of `fill`,
which refuses with `relocation-out-of-range` (`spec/arch.md` §7) and never
writes a truncated value, and of the verifier (`spec/layout.md` L-127).

Rule 4a costs a pass over the relocations and, for the erratum sites,
over the code words. `fill` (§5) makes these checks anyway, on every field
it writes, so a full link pays nothing extra; a loaded layout is checked
for rules 1-3, 5, 4b and the part of rule 4a the tables show (a redirect
names an existing patch of its kind, keyed by its own site for an erratum,
inside a placed section; a `Plt` thunk's key has an entry), up front, and
for the rest of 4a and for `fill`'s obligation on the units that are
refilled, the rest being covered by the state's own integrity
(`spec/incr.md`).

### 4.5 What is derived from the layout

The address of anything is computed, not stored: a section's address is
its chunk's address plus its offset; a symbol's value follows from `def`
and `redirect` (§3.3) through `place`, `Split` and `MergeMap`,
`sym_place` or `defined`; a GOT slot's address from `Slots` and its
chunk. A phase MAY cache such values in a column of its own
(`sym_addr[SymbolId]` for the image phase); a cache is never stored in
the link state and never an input to another phase.

## 5. The image is a function

`mortar_image` computes the output bytes from the tables of §3 and §4 and
the input bytes, and from nothing else: not from a table of its own
making, not from bytes it wrote earlier, not from the previous output.

It does so per **unit**: one placed input section, one live fragment run
of a merged chunk, or one synthetic chunk (or a record range of one, where
`spec/synth.md` says a chunk is divisible). `fill(unit, out: &mut [u8])`
writes exactly the unit's bytes into a buffer the caller supplies. Units
do not overlap (validity rules 1 and 2), so:

- a full link fills units in parallel; the writer hands out disjoint
  slices of one buffer, or fills batches into buffers of its own and
  writes each at its file offset (positioned writes; safe, and the whole
  image need not be resident). Which, is `spec/image.md`'s to decide by
  measurement;
- an incremental link fills the units whose inputs or addresses changed;
- the verifier, and the test "a full link given this layout produces
  these bytes" (`docs/VALIDATION.md` L10), are meaningful.

Consequences that bind other specs:

- **`mortar_image` accepts every valid layout**, not only the ones
  `mortar_layout` computes by default. It may assume the validity rules of
  §4 and nothing more (no "sections of one file are adjacent", no "the GOT
  is in command-line order").
- **Relaxations are functions of the tables.** Whether an instruction
  sequence is rewritten (GOT load to address computation, a TLS model to a
  cheaper one) is decided by a rule in `spec/arch.md` from `Resolution`,
  `Demands`, the addresses the layout gives, and the bytes and relocation
  records of the same input section (a sequence of relocations,
  `spec/arch.md` §3.5, is recognized from neighboring records: input data,
  not a table), evaluated in `fill`.
  Because demand is conservative (§3.7), the unrelaxed form is always
  available: moving a section can turn a relaxation off without any table
  changing. A rewriting that changes a section's size is not a
  relaxation in this sense: it is a layout matter, recorded in
  `Placement`.
- **No value outside the tables.** `fill` reads no cache of an earlier
  pass, no previous output, and infers nothing from the order in which
  another phase happened to run.
- **No table has a row per relocation or per symbol-table entry** beyond
  `sym_map`. Scan and fill each read relocation records from the input
  bytes. The exceptions are sparse and listed: patch redirects (§4.4),
  dynamic relocations (`Slots`). For `-r`, the output index of a symbol
  is a function (§4.3), not a table.
- **One sealing pass.** Content that depends on the output's own bytes (the
  build id, `spec/image.md`) is computed after all units are filled, by a
  function of those bytes specified so that an incremental link can update
  it from the changed ranges.

## 6. Determinism in parallel phases

Principle 6: the output and every diagnostic depend on the inputs, their
order and the options only. Every parallel phase has the sequential
meaning its spec gives; the parallel implementation may reach shared
state in exactly these ways, each of which is independent of scheduling:

1. **Disjoint slices.** A column indexed by an id whose ranges are
   contiguous per unit (§2.2) is split into one mutable slice per unit;
   a worker writes its unit's slice only.
2. **Commutative, idempotent updates.** Shared cells updated with bitwise
   or, minimum, maximum or a saturating count over a total order
   (atomically). Flag sets (§3.7) and every merge of §3.3 are of this
   form. An update whose result depends on arrival order (a counter used
   as an id, "first writer wins", a sum of floats) is not.
3. **Ordered collection.** Each unit appends to a buffer of its own; the
   buffers are concatenated in unit order. Ids handed out to collected
   rows are assigned after concatenation.
4. **Sorting.** Rows gathered in any order and then sorted by a key that
   is total on them.
5. **No observable hash order.** Hash tables are for lookup. Nothing
   observable (output bytes, ids, diagnostics, the map file, the link
   state) follows a hash table's iteration order or its bucket layout;
   iteration happens over a sorted or id-ordered view. The hash functions
   are fixed (no per-process seed), so that sharding by hash is
   reproducible too.
6. **Thread count is not an input.** Work is partitioned by unit or by
   fixed-size block, never "into N parts for N threads", wherever the
   partition can be observed (block hashing, shard assignment, patch
   group boundaries).

Diagnostics follow rule 4 through their keys (§8).

`mortar_core` provides the helpers that embody rules 1 to 4 (for-each over
units, ordered map-and-collect, split-by-ranges, total-order sort), on
scoped threads, and a **scheduling seed** for tests: with
`MORTAR_SCHED_SEED=<n>` the helpers hand units to workers in a permuted
order and yield at unit boundaries, so that a test can run a phase under
many schedules and thread counts and require identical tables. Every
helper and every parallel phase has such a test.

How many workers a call uses is a schedule, never an input (rule 6
partitions by unit, never by worker): at most the thread count, at most
the number of items, at most `units × c / 200 µs` when the call site
states an estimated cost `c` per unit (a constant measured once, never a
timing taken during the link), after the input phase of a link run
without `--threads` or a seed at most one worker per 4 MiB of resident
input (the store's count, `spec/input.md` §11; WP-032h), and, without a
seed, a worker spawns another only while units are left untaken. With a seed every worker
runs, so that the tests exercise schedules (WP-032g).

`<n>` is a decimal integer below 2^64 (surrounding white space allowed).
Any other value is an error of the command-line phase, which the driver
reports before the first phase runs and with no output written:
`invalid-option-value` (`spec/diag.md` §3.1) with `{option}` the
variable's name, `MORTAR_SCHED_SEED`, and `{value}` the value as set (bytes
that are not UTF-8 shown as U+FFFD), a note `expected a decimal integer
below 2^64`, key `(10, 0, 0)` (§8). A seed is never ignored or replaced
by a default: a test that asked for a schedule and got another would pass
for the wrong reason.

## 7. What a link state stores

`spec/incr.md` (M7) owns the format. The model guarantees it can exist.

### 7.1 Identity

Ids do not survive a link; identities do, and a later link translates.

| thing | identity |
|---|---|
| file | the found path (`spec/input.md` §4.5) and the mention ordinal among files with that path (two mentions of one path are two files, `spec/input.md` §9.4; for a member, the ordinal of its archive mention among those with that path), plus member name and ordinal among same-named members for an archive member; with a content hash of the whole file (`spec/input.md` §11.6) and the probes that chose it (§4.6), re-checked by one `stat` each: a new candidate earlier in a search changes neither path nor hash, only a probe |
| section | file identity, then an **anchor** that survives edits elsewhere in the file: its COMDAT signature, else the first named non-local symbol defined at its start, else the first named local one; then its name and its ordinal among sections with the same anchor and name; with a content hash (which kiln and hod may supply, `docs/ROADMAP.md` "Cross-project") |
| global symbol | key bytes (name and version) |
| local symbol | file identity, name, ordinal among same-named locals of the file (never the symbol-table index, which shifts on any edit) |
| COMDAT group | signature bytes |
| output section, chunk | kind and name |
| fragment | output section and content hash |

### 7.2 Contents

- the `Plan`, normalized;
- per file: identity, hash, and the rows invariant I-file lets a later
  link reuse;
- `Resolution` and `Liveness` in identity terms;
- the decision tables (§4) in full, in identity terms: they are the
  previous layout, and the only part of a link state that cannot be
  recomputed;
- the **dependency index**: for each thing whose address or content can
  change (a section, a fragment, a slot, a symbol-anchored allocation, a
  symbol's definition, a patch and its redirects, and each per-symbol
  demand flag that another file's bytes depend on: `GOT_NORELAX`,
  `TLSIE_NORELAX`, `CANON_PLT`, `COPY`, `spec/arch.md` §4.2), the
  **units** (§5) whose bytes depend on it. For a patch, these are the
  units holding its redirected sites, and for each redirected site, its
  patch's chunk (`spec/arch.md` §6, "Incremental links"). A flag is a union
  over every file (§3.7): an edit in one file can set or clear it, which
  changes the bytes of a unit in a file that did not change (an AArch64
  GOT-load pair written back as a GOT load, an IE use kept IE, a
  function's address becoming its PLT entry, a symbol moving to its copy
  allocation); the other flags reach other files only through `Slots` and
  addresses, which the index already covers. An
  incremental link refills whole units through the same `fill`; it never
  patches a field in place, so relaxed sequences, sorted tables
  (`.eh_frame_hdr`, `.gnu.hash`), whole-chunk encodings (`.relr.dyn`)
  and symbol tables need no special case. The index is derived data,
  stored because recomputing it costs a scan of every relocation, and
  written by a streaming pass, never held as a table. `spec/incr.md`
  IN-25 implements it per unit (a section's bytes, not a relocation
  field), names its non-relocation targets (slots, the redirected sites
  of patches, the demand flags above) and adds beside it the **liveness
  graph** (IN-29), which a `--gc-sections` update walks instead of the
  relocations; IN-18a has the bytes of the parts stored so far.

A link state is untrusted input: it is read with the same care as an
object file, validated (§4.4, identities against the present inputs), and
any failure means a full link with the reason reported.

### 7.3 The two tested invariants

For every incremental link the harness makes: the verifier accepts the
output; and a full link *given the incremental link's decision tables*
produces the same bytes. The second needs an entry to the pipeline that
takes decision tables instead of computing them (a test hook, not an
option users see), which §5 makes well-defined. The second holds
exactly outside the **dead ranges** (`spec/incr.md` IN-3a: slack, holes,
dead fragments and records), which the decision tables record and which
are compared by the verifier's checks instead of byte for byte (IN-1
(B)). A third invariant is tested beside them, because the second alone
would accept stale tables that replay faithfully: (C) the update's
output is equivalent to a from-scratch full link of the same inputs
(`spec/incr.md` IN-1 (C), the items of IN-2). The hook is
`spec/incr.md` IN-61.

## 8. Diagnostic keys

`spec/diag.md` §6.1 leaves the phase ordinals to this table. Ordinals are
in pipeline order and spaced so that a phase can be added without
renumbering.

| phase | ordinal | `unit` | `seq` |
|---|---|---|---|
| command line | 10 | argument index | 0, or a count within the argument |
| incremental (state load, fallback) | 15 | 0 | a count |
| input (find, read, parse) | 20 | the `Plan` input's argument index | `k × 2^40 + offset` (`spec/input.md` §12.2): one input reaches several files (a script, a thin archive, the closure) |
| resolve | 30 | `FileId` of the reporting mention | symbol-table index |
| one-definition rule | 40 | `FileId` of the later definition | symbol-table index; for a row diagnostic without a symbol, `2^40 +` the row's index in its unit |
| assignment | 45 | `FileId` | section index |
| liveness, folding | 50 | `FileId` | section index |
| split, merge | 60 | `FileId` | section index × 2^32 + piece |
| scan | 70 | `FileId` | section index × 2^32 + relocation index |
| synthetic sections | 80 | 0 | `SymbolId`, or a count in a sequential step |
| layout | 90 | 0 | `ChunkId`, or a count in a sequential step |
| image | 100 | `FileId` (0 for synthetic chunks) | section index × 2^32 + relocation index; `ChunkId` for file 0 |
| verify (`--verify`) | 110 | 0 | output offset |

`FileId` is usable as a unit because it is a function of the inputs
(§2.1). A relocation or piece index is limited to 2^32 − 1 per section by
the key; a section with more is refused by name.

## 9. Memory

The model is the largest consumer of memory after the input bytes, so its
shape is part of principle 3:

- columns, not structs of everything: a phase touches the columns it
  needs (`place` is 12 bytes per section and is all the image phase needs
  to find a section);
- nothing per section for files that are not live (§2.2); wide columns
  only for output-carrying sections; `folded` only when folding is on;
- no per-relocation and no per-symbol-table-entry rows beyond `sym_map`
  (4 bytes per non-local entry);
- `Split`: 8 bytes per piece occurrence, 16 per fragment plus the
  interning table;
- names are slices of resident input bytes (`&'s [u8]` borrowed from the
  shared store, `spec/input.md` §11), hashed once; names always lie in
  resident parts;
- every table states its row size in WP-020's landed record, and the bench
  harness's peak-memory figure is the check.

## 10. Rust shape (informative)

As landed by WP-020a (`mortar_core`; the record is
`spec/landed/model/WP-020a.md`):

```rust
#[repr(transparent)] pub struct FileId(pub u32);  // likewise the other ids
pub struct Column<I, T> { /* Vec<T> indexed by I */ }
impl<I: Id, T> Column<I, T> {
    pub fn split_by<'a>(&'a mut self, ranges: &[IdRange<I>])
        -> Result<Vec<&'a mut [T]>, SplitError>;   // §6.1: ascending, in bounds
}
pub struct SymKey(u64);  // packed so that integer order is §2's order
impl SymKey { pub fn kind(self) -> SymKind; }
pub enum SymKind { Global(SymbolId), Local(LocalSym) }
pub struct Inputs<'s> { pub target: LinkTarget, pub files: Column<FileId, File<'s>>,
                        pub link: LinkInputs<'s>, pub store: StoreRef<'s>, /* §3.2 */ }
pub trait InputStore: Sync {                                  // spec/input.md §1, §11
    fn resident(&self, blob: Blob, range: Range<u64>) -> Option<&[u8]>;
    fn copy_to(&self, blob: Blob, range: Range<u64>, out: &mut [u8]) -> Result<(), CopyError>;
}
// in mortar_input (WP-021):
// pub fn load<'s>(plan: &Plan, env: &LoadEnv, store: &'s Store, diag: &Collector) -> Option<Inputs<'s>>;
pub struct Resolution { /* §3.3 */ }
pub struct Demands { pub global: Column<SymbolId, DemandFlags>, pub local: Box<[(LocalSym, DemandFlags)]>,
                     pub output: GlobalFlags, pub dyn_relocs: Box<[(SectionId, DynCounts)]> }  // §3.7
pub struct Layout { pub folds: Folds, pub merge: MergeMap, pub slots: Slots, pub placement: Placement }
impl Layout {
    // the derived tables, the page size, the section shapes
    pub fn check(&self, cx: &CheckContext<'_, '_>) -> Result<(), Diagnostic>;
}
pub mod par { /* §6: for_each, map_collect, sort_by_key, the scheduling seed */ }
```

## 11. Tests (WP-020)

- §2: id arithmetic; load-order, symbol and fragment numbering equal to
  the sequential definition under every thread count and scheduling seed.
- §4.4: `Layout::check` accepts the layouts mortar computes and rejects a
  mutation of each rule (overlap, misalignment, a missing place, a missing
  patch, a wrong segment, split TLS, a misaligned TLS start, a redirect to
  a patch of the wrong kind or key, a field out of range), and accepts an
  unused patch.
- §3.7: the per-key and per-output flags merge by union; the flags that
  exclude each other are rejected by `Demands::check`.
- §5: filling units in a permuted order, or one at a time into a zeroed
  buffer, gives the bytes of a full fill.
- §6: each helper under seeds and thread counts 1, 2, 7 and 64; a
  malformed seed becomes the `invalid-option-value` diagnostic of §6.
- §3.2, §3.3, §3.4 (WP-020d): an accepting `Inputs` with every kind of
  file, mention and link row, and one violating table per way of breaking
  each of rules 1, 3, 4, 5, 7, 8, 9, 10; `Resolution` rules 5 and 6 and
  the three rules of `check_against`; `Assignment.withdrawn` and
  `Assignment::def`.
- §7.1 (WP-020d): mention ordinals of a path named twice and of an
  archive mentioned twice; the probe re-check list.
- §7: a table written in identity terms and read back in a link whose ids
  differ (an input added in front; a function added in the middle of a
  file) denotes the same rows; so does each dependee of the §7.2 index,
  patches, redirect sites and cross-file flags included.

## 12. Open, with owner

| question | owner |
|---|---|
| A section kind `Property` for `.note.gnu.property`, which synth merges by kind (optional, `spec/resolve.md` §18) | `spec/synth.md` |
| Divisible synthetic chunks, the sealing function for the build id, resident image or positioned writes | `spec/synth.md`, `spec/image.md` |
| `-r`: relocation and group chunks, REL addends of section symbols rewritten in the bytes | `spec/layout.md`, `spec/image.md` (M4) |

Closed (WP-020d): the mapping of `Plan` inputs to files (`spec/input.md`);
archive semantics, `file_live` being reachability, and the COMDAT keeper
being the least live `FileId` (`spec/resolve.md` §4, §6.1); version
scripts and `versym` against the key of §2.3 (`spec/resolve.md` §9).
