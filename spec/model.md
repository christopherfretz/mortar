# spec: the model (`mortar_core`)

Status: **draft** (WP-002). Frozen, and marked normative, after review
against the reader API of WP-004 and the `Plan` of WP-008. Design only: the
code is WP-020.

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
  Input bytes are borrowed (`'data`), nothing else is.

## 2. Ids

| id | one per | numbered |
|---|---|---|
| `FileId` | loaded file: the internal file, each relocatable object (given directly or an archive member), each shared object | §2.1 |
| `ArchiveId` | archive opened | in order of first appearance on the command line |
| `SectionId` | section-header-table entry of a relocatable object, index 0 included | `section_base[file] + sh_index` (§2.2) |
| `SymbolId` | distinct global symbol name (§2.3) | §2.3 |
| `ComdatId` | distinct COMDAT group signature | as `SymbolId`, over group signatures |
| `ChunkId` | unit of the output that layout places as a whole (§4.3) | creation order (§4.3) |
| `SegmentId` | program header of the output | output order |

A local symbol has no id of its own: it is `(FileId, st_index)`, written
`LocalSym`. Where a table is keyed by "a symbol, local or global"
(GOT-like slots, §4.2), the key is `SymKey = Global(SymbolId) |
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

`FileId` order is the **priority order** of symbol resolution
(`spec/resolve.md`) and the unit order of every per-file parallel phase.

### 2.2 Sections

A file's sections occupy one contiguous id range, `section_base[f] ..
section_base[f] + e_shnum(f)`, in section-header order, with no compaction:
`SectionId − section_base` *is* the ELF section index, so a symbol's
`st_shndx`, a relocation section's `sh_info` and a group's member list
address rows without a lookup. Rows of sections that carry no output
(symbol and string tables, relocation sections, groups) exist and are
marked (§3.2). Shared objects and the internal file have empty ranges.

Contiguity per file is what lets a parallel phase hand each worker a
disjoint mutable slice of a column (§6.1).

### 2.3 Global symbols

`SymbolId` names a **global symbol name**: one row per distinct name among
the non-local symbol-table entries of all files (lazy archive members and
shared objects included), with symbol versions distinguishing names as
`spec/resolve.md` specifies. It is not a definition: which definition a
name resolves to is a column of `Resolution`.

Numbering is defined sequentially: files in `FileId` order, each file's
non-local entries in symbol-table order, a name seen for the first time
taking the next id. A parallel implementation interns concurrently and
then numbers from each name's first mention `(FileId, st_index)`; the
result MUST equal the sequential definition (§6). `ComdatId` likewise,
over `SHT_GROUP` signatures in `(FileId, sh_index)` order.

## 3. Derived tables

A **derived table** is a pure function of the tables before it: it records
no choice. Recomputing it from the same inputs gives the same rows.

### 3.1 `Plan` (written by `mortar_cli`)

The command line as data: `spec/cli.md` §plan. Immutable. Each input keeps
its argument index; that index is the `unit` of diagnostics about the
command line and the root of load order.

### 3.2 `Inputs` (written by `mortar_input`)

Everything known from reading files, before any symbol is resolved.
Immutable once the phase ends.

| part | row | columns |
|---|---|---|
| `target` | one | machine, class, relocation form (RELA, or REL with implicit addends); the first object decides when `-m` did not, and every later file must agree or is diagnosed |
| `files[FileId]` | file | kind (internal, object, shared object); display name (`FileRef`: path, archive and member); `ArchiveId` and member offset, if any; the `Plan` input it came from and the positional state copied from it (as-needed, whole-archive, lazy, static, exclude-libs); the bytes (`&'data [u8]`); the parsed reader (`mortar_elf::Object` or the shared-object view); `section_base`, section count; the range of `sym_map`; identity and content hash (§7.1) |
| `sections[SectionId]` | section | `kind` (below); the relocation section(s) applying to it; the `ComdatId` it belongs to, if any; for hot paths, copies of size, alignment, flags and type widened once |
| `sym_map` | non-local symbol-table entry of each file, per file in table order | the `SymbolId` of its name |
| `symbols[SymbolId]` | name | name bytes, hash, version part |
| `comdats[ComdatId]` | signature | name bytes |
| `archives[ArchiveId]` | archive | path, kind |

`kind` classifies a section once, from its type, flags and name, for every
later phase: `Regular` (bytes copied and relocated), `Nobits`, `Merge`
(strings or fixed-size constants, §4.1), `EhFrame`, `Note`, `Debug`
(non-alloc, handled as data), `Metadata` (Mason records, `spec/meta.md`),
and the kinds that carry no output of their own: `Null`, `Symtab`,
`Strtab`, `Reloc`, `Group`, `Stack` (`.note.GNU-stack`), `Excluded`
(`SHF_EXCLUDE`). A section whose type or flags mortar does not implement
is not classified: the input phase refuses it by name.

Invariant **I-file**: the rows of a file (its `files` row except the
bases, its `sections` rows, the names it mentions) are a function of that
file's bytes and its `Plan` input alone. This is what an incremental link
reuses for an unchanged file.

Relocations and symbol-table entries are **not copied** into any table:
phases read them from the input bytes through the reader each time they
need them (§5).

### 3.3 `Resolution` (written by `mortar_resolve`)

| part | row | columns |
|---|---|---|
| `file_live[FileId]` | file | whether the file takes part in the link (always; extracted archive member; needed shared object) and, for an extracted member, the reference that pulled it in `(FileId, st_index)` |
| `def[SymbolId]` | name | the resolved definition: `Undefined{weak}`, `Defined{file, st_index}`, `Common{file, st_index, size, align}`, `Shared{file, st_index}`, `Internal{..}` (linker-defined); and the properties merged over every live mention: visibility, whether referenced from a relocatable object, whether referenced from a shared object, binding for export, version |
| `comdat[ComdatId]` | signature | the file whose group is kept |
| `section_state[SectionId]` | section | `Kept`, or `Dropped(reason)`: file not live, COMDAT group not kept (with the keeping file), excluded, consumed by the linker (the kinds without output), stripped by option |

`def` is the minimum of a total order over the candidate definitions of a
name (`spec/resolve.md`: strength, then `FileId`, then symbol index), which
is what makes parallel resolution deterministic (§6.2).

### 3.4 `Liveness` (written by `mortar_live`)

`live[SectionId]: bool` after `--gc-sections` (all `Kept` sections when it
is off), and `folded[SectionId]: SectionId` for identical code folding
(itself when not folded). Whether folding is a derived table or a decision
(§4) is settled with `spec/live.md`; until then it is derived.

### 3.5 `Demands` (written by `mortar_arch` scan)

What the relocations of live sections require, with no placement:

- per `SymKey`, a set of flags (needs a GOT slot, a PLT entry, a TLS
  descriptor, a TLS module/offset pair, a thread-pointer offset slot, a
  copy relocation, a canonical PLT address, export to the dynamic symbol
  table);
- per section, the number of dynamic relocations it will carry, by kind;
- errors found on the way (a relocation that cannot be satisfied in this
  kind of output).

The flag set of a global is the union over all relocations that mention
it; union is order-independent (§6.2). `spec/arch.md` defines the flags.

## 4. Decision tables: the layout

A **decision table** records choices the inputs do not force: where
things go and in what order. Any value that satisfies the table's
validity rules yields a correct output; the default policy chooses one
deterministically, an incremental link chooses another (as much of the
previous one as still fits). Together the decision tables are **the
layout** of principle 7.

### 4.1 `MergeMap` (written by `mortar_merge`)

For each live `Merge` section, the split of its content into pieces and,
for each piece, the merged chunk and the offset it lands at. A reference
into a mergeable section resolves through this map. Valid when equal
pieces of one output section map to one offset and every piece is mapped.

### 4.2 `Slots` (written by `mortar_synth`)

The content order of every synthetic table: the GOT and PLT index of each
`SymKey` that demands one, the order of `.dynsym` and of `.symtab`, of
dynamic relocations, of version definitions and needs, of `DT_NEEDED`
entries, of `.eh_frame` records after deduplication of CIEs, and the size
each synthetic chunk therefore has. Valid when every demand of §3.5 has
exactly one slot, no slot is given twice, and the orders satisfy the
format's own rules (local before global in `.symtab`; the `.gnu.hash`
bucket order in `.dynsym`; ...), listed in `spec/synth.md`.

### 4.3 `Placement` (written by `mortar_layout`)

| part | row | columns |
|---|---|---|
| `chunks[ChunkId]` | chunk | kind: file header, program headers, section headers, an **output section** (assembled from input sections), a merged section, a synthetic section (`spec/synth.md`); name, type, flags, alignment; `size`, `capacity ≥ size`; `addr`, `offset` (file); the output section index |
| `order` | — | the chunks in file order |
| `segments[SegmentId]` | program header | type, flags, offset, address, file size, memory size, alignment |
| `place[SectionId]` | section | `(ChunkId, offset in chunk)`, or none; a folded section has the place of the section it is folded into |
| `sym_place` | symbol-anchored allocation: a common symbol, a copy-relocated symbol | `(SymbolId, ChunkId, offset, size)`, sorted by `SymbolId` |
| `defined` | linker-defined symbol | `(SymbolId, value or (ChunkId, offset))` |
| `thunks` | range-extension thunk | `(ChunkId, offset, target)`, and the redirect list `(SectionId, relocation index) → thunk`, sorted |
| `tls` | one | start, size and alignment of the TLS template |
| `entry` | one | the entry address |

`ChunkId`s are given in a fixed creation order: headers, then synthetic
chunks in the order `spec/synth.md` lists them, then merged chunks and
output sections in order of first contribution `(FileId, sh_index)`.
`order` is the decision; the id is only a name.

`capacity − size` is slack. The default policy leaves none; an incremental
policy leaves and consumes it. Nothing else distinguishes the two.

**Validity** (`Placement::check`, run in tests on every layout mortar
computes and in production on every layout loaded from a link state,
before it is trusted):

1. every live, output-carrying section has exactly one place; places
   inside a chunk do not overlap, lie within `size`, and honor the
   section's alignment relative to the chunk's address;
2. chunks do not overlap in the file or in memory; `addr ≡ offset` modulo
   the page size for loaded chunks; alignments hold;
3. segments cover exactly the chunks the program-header rules require
   (`spec/layout.md`), with the flags and order a loader needs;
4. every thunk redirect names a relocation whose target is out of range
   from its place and a thunk within range; every relocation that is out
   of range has a redirect;
5. `sym_place`, `defined`, `tls` and `entry` are present for exactly the
   symbols and features that need them.

### 4.4 What is derived from the layout

The address of anything is computed, not stored: a section's address is
its chunk's address plus its offset; a symbol's value follows from `def`
(§3.3) through `place`, `MergeMap`, `sym_place` or `defined`; a GOT
slot's address from `Slots` and its chunk. A phase MAY cache such values
in a column of its own (`sym_addr[SymbolId]` for the image phase); a cache
is never stored in the link state and never an input to another phase.

## 5. The image is a function

`mortar_image` computes the output bytes from the tables of §3 and §4 and
the input bytes, and from nothing else: not from a table of its own
making, not from bytes it wrote earlier, not from the previous output.

It does so per **unit**: one placed input section, or one synthetic chunk
(or a record range of one, where `spec/synth.md` says a chunk is
divisible). `fill(unit, out: &mut [u8])` writes exactly the unit's byte
range. Units do not overlap (validity rule 1 and 2), so:

- a full link fills all units in parallel, each through a disjoint
  `&mut [u8]` of the output, with no `unsafe`;
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
  cheaper one) is decided from `Resolution`, `Demands` and addresses by a
  rule in `spec/arch.md`, the same in scan (which must not demand a slot
  the rule will not use, or must demand it conservatively) and in fill.
  A relaxation that changes a section's size is a layout matter and is
  recorded in `Placement`.
- **No table has a row per relocation.** Scan and fill each read
  relocation records from the input bytes. The exceptions are sparse and
  listed: thunk redirects (§4.3), dynamic relocations (`Slots`). The
  incremental link state's referrer index (§7.2) is written to disk by a
  streaming pass and never held as a table.
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
   or, minimum or maximum over a total order (atomically). Flag sets
   (§3.5) and the choice of a definition (§3.3) are of this form. An
   update whose result depends on arrival order (a counter used as an id,
   "first writer wins", a sum of floats) is not.
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
   partition can be observed (block hashing, shard assignment, thunk
   group boundaries).

Diagnostics follow rule 4 through their keys (§8).

`mortar_core` provides the helpers that embody rules 1 to 4 (for-each over
units, ordered map-and-collect, split-by-ranges, total-order sort), on
scoped threads, and a **scheduling seed** for tests: with
`MORTAR_SCHED_SEED=<n>` the helpers hand units to workers in a permuted
order and yield at unit boundaries, so that a test can run a phase under
many schedules and thread counts and require identical tables. Every
helper and every parallel phase has such a test.

## 7. What a link state stores

`spec/incr.md` (M7) owns the format. The model guarantees it can exist:

### 7.1 Identity

Ids do not survive a link; identities do, and a later link translates.

| thing | identity |
|---|---|
| file | the path as resolved, plus member name and ordinal among same-named members for an archive member; with a content hash to detect change |
| section | file identity and section index; for matching across an edit of the file, the section's name, ordinal among same-named sections, and content hash (which kiln and hod may supply, `docs/ROADMAP.md` "Cross-project") |
| symbol | name bytes (with version) |
| COMDAT group | signature bytes |
| chunk | kind and name |

### 7.2 Contents

- the `Plan`, normalized;
- per file: identity, hash, and the rows invariant I-file lets a later
  link reuse;
- `Resolution` and `Liveness` in identity terms (a definition as file
  identity and symbol index);
- the decision tables (§4) in full: they are the previous layout, and the
  only part of a link state that cannot be recomputed;
- the **referrer index**: for each place that can move (a section, a
  slot, a symbol-anchored allocation), the output fields that depend on
  its address. It is derived data, stored because recomputing it costs a
  scan of every relocation.

A link state is untrusted input: it is read with the same care as an
object file, validated (§4.3 validity, identities against the present
inputs), and any failure means a full link with the reason reported.

### 7.3 The two tested invariants

For every incremental link the harness makes: the verifier accepts the
output; and a full link *given the incremental link's decision tables*
produces the same bytes. The second needs an entry to the pipeline that
takes decision tables instead of computing them (a test hook, not an
option users see), which §5 makes well-defined.

## 8. Diagnostic keys

`spec/diag.md` §6.1 leaves the phase ordinals to this table. Ordinals are
spaced so that a phase can be added without renumbering.

| phase | ordinal | `unit` | `seq` |
|---|---|---|---|
| command line | 10 | argument index | 0, or a count within the argument |
| input (find, read, parse) | 20 | the `Plan` input's argument index | byte offset of the construct in the file as given (an archive member: its offset in the archive plus the offset within) |
| resolve | 30 | `FileId` of the reporting mention | symbol-table index |
| one-definition rule | 40 | `FileId` of the later definition | symbol-table index |
| liveness, folding | 50 | `FileId` | section index |
| merge | 60 | `FileId` | section index, then piece |
| scan | 70 | `FileId` | section index × 2^32 + relocation index |
| synthetic sections | 80 | 0 | `SymbolId`, or a count in a sequential step |
| layout | 90 | 0 | `ChunkId`, or a count in a sequential step |
| image | 100 | `FileId` (0 for synthetic chunks) | section index × 2^32 + relocation index; `ChunkId` for file 0 |
| verify (`--verify`) | 110 | 0 | output offset |
| incremental (state load, fallback) | 5 | 0 | a count |

`FileId` is usable as a unit because it is a function of the inputs
(§2.1). A relocation index is limited to 2^32 − 1 per section by the key;
a section with more is refused by name.

## 9. Memory

The model is the largest consumer of memory after the input bytes, so its
shape is part of principle 3:

- columns, not structs of everything: a phase touches the columns it
  needs (`place` is 12 bytes per section and is all the image phase needs
  to find a section);
- no per-relocation and no per-symbol-table-entry rows beyond `sym_map`
  (4 bytes per non-local entry);
- names are slices of input bytes, hashed once;
- every table states its row size in WP-020's landed record, and the bench
  harness's peak-memory figure is the check.

## 10. Rust shape (informative)

```rust
#[repr(transparent)] pub struct FileId(u32);      // likewise the other ids
pub struct Column<I, T> { /* Vec<T> indexed by I */ }
impl<I: Id, T> Column<I, T> {
    pub fn split_by<'a>(&'a mut self, ranges: &[IdRange<I>]) -> Vec<&'a mut [T]>; // §6.1
}
pub enum SymKey { Global(SymbolId), Local(FileId, u32) }
pub struct Inputs<'data> { pub target: Target, pub files: Column<FileId, File<'data>>, /* §3.2 */ }
pub struct Resolution { /* §3.3 */ }
pub struct Layout { pub merge: MergeMap, pub slots: Slots, pub placement: Placement }
impl Placement { pub fn check(&self, /* derived tables */) -> Result<(), Diagnostic>; }
pub mod par { /* §6: for_each, map_collect, sort_by_key, the scheduling seed */ }
```

## 11. Tests (WP-020)

- §2: id arithmetic; load-order and symbol numbering equal to the
  sequential definition under every thread count and scheduling seed.
- §4.3: `Placement::check` accepts the layouts mortar computes and rejects
  a mutation of each rule (overlap, misalignment, a missing place, a
  missing thunk, a wrong segment).
- §5: filling units in a permuted order, or one at a time into a zeroed
  buffer, gives the bytes of a full fill.
- §6: each helper under seeds and thread counts 1, 2, 7 and 64.
- §7: a table written in identity terms and read back in a link whose ids
  differ (an input added in front) denotes the same rows.

## 12. Open, with owner

| question | owner |
|---|---|
| The mapping of `Plan` inputs to files: the same path twice, shared objects by `DT_SONAME`, thin archives, `--start-lib` | `spec/input.md` (WP-021) |
| Archive semantics: GNU ld's left-to-right extraction against lld's and mold's order-independent one, and which one `file_live` implements | `spec/resolve.md` (WP-022) |
| Symbol versions in the key of `SymbolId` (`foo@V`, `foo@@V`, shared-object `versym`) | `spec/resolve.md` |
| Folding as derived or decision | `spec/live.md` |
| Divisible synthetic chunks and the sealing function for the build id | `spec/synth.md`, `spec/image.md` |
| Whether `Plan` moves into `mortar_core` or is re-exported from `mortar_cli` | this spec, at freeze, with WP-008's handback |
