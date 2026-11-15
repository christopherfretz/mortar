# spec: liveness, the scan driver, the image and the output file (`mortar_image`)

Status: **draft (WP-026 stage 1)**. Method, experiments and what was not
checked are in `spec/landed/image/WP-026-spec.md` (cited as **E1**–**E15**).
Every measurement here was taken on the WP-026 host: AArch64 (24 CPUs, 31
GB, Linux 6.12.76, transparent huge pages `always`), load 23–36 during the
runs, Rust 1.95.0, GNU ld 2.44, lld 19.1.7 and 22, mold 2.37.1, gdb 16.3,
lldb 19.1.7. Two file systems were measured: `/tmp` (overlay, local) and
the workspace mount (a host share). Numbers are medians of 3 runs with the
range in parentheses unless said otherwise; ratios matter more than
absolutes, because the host was shared and loaded.

This spec owns five phases of `docs/ARCHITECTURE.md`'s pipeline and the
file the pipeline produces:

| phase | ordinal (`spec/model.md` §8) | writes | this spec |
|---|---|---|---|
| liveness (and later folding) | 50 | `Liveness` (model §3.6) | §3 |
| relocation scan driver | 70 | `Demands` (model §3.7), the scan's diagnostics | §4 |
| fill | 100 | the output bytes, the fill's diagnostics | §5, §6 |
| sealing | 100 | the build id | §8 |
| output file | 100 | the file at `-o`, the map file | §7, §9 |

`mortar_arch` owns what one relocation does (`spec/arch.md`, `spec/a64.md`);
this spec owns how every relocation is reached, in what order, with what
memory, and what is done with the results. `spec/synth.md` (not yet
written) owns the content of synthetic chunks; this spec says how they are
asked for it (§13).

Rules are numbered **I-1**, **I-2**, …. Each carries the milestone where it
binds: **[M1]** static AArch64 (`docs/ROADMAP.md`), **[M2]** dynamic AArch64
with `--gc-sections`, **[M4]** compressed debug, `-r`, `.gdb_index`,
**[M7]** incremental linking, **[perf]** the performance package that
follows M2 (`docs/ROADMAP.md`). A rule marked for a later milestone is
specified so that nothing in the M1 design must be redone for it; its code
is not M1's. Rules marked **[C]** are choices, where the reference linkers
are not followed or disagree (§15).

## 1. Principles that bind this spec

- **The image is a function** (`spec/model.md` §5). Fill reads the tables
  of model §3 and §4 and the input bytes, and nothing else. A cache this
  spec builds (§5.2) is a function of those tables, lives only for the
  phase, and is never an input to another phase.
- **No per-relocation table** (model §5, §9). Scan and fill each decode
  every relocation record from the input bytes. A per-relocation byte of
  classification, cached between them, would cost 1 byte × 2.2 M records
  for `cxx-medium-g` and 100 MB for a link of 10^8 relocations; decoding
  costs 2.0–3.7 ns per record (E6), so the cache would save at most
  0.2–0.4 s of CPU per 10^8 relocations, spread over every thread, at a
  memory cost the model forbids. Rejected.
- **Determinism** (model §6): the output file's bytes, the map file and
  every diagnostic depend on the inputs, their order and the options. The
  thread count, `MORTAR_SCHED_SEED` and the file system are not inputs.
  Every partition this spec makes is by unit or by a fixed-size block
  whose size is a constant of this spec (rule 6).
- **Correct or refused by name.** An option this spec does not implement
  at a milestone is `refused` (`spec/cli.md` §4.2), never ignored.
- **Safe Rust.** `mortar_image` is `forbid(unsafe_code)`. It never maps a
  file. §10 measures what that costs.

## 2. Vocabulary

- **Unit**: model §5's: a placed input section, a live fragment run of a
  `Merged` chunk, a synthetic chunk or a record range of one. Plus, in
  this spec, the **structural units** the image writes itself from
  `Placement`: the ELF header, the program header table, the section
  header table (`ChunkKind::FileHeader`, `ProgramHeaders`,
  `SectionHeaders`), and **gaps** (bytes of the file covered by no unit,
  §5.5).
- **Fill plan**: the list of units with file bytes, sorted by file offset
  (§5.2).
- **Window**: a range `[k × W, min((k + 1) × W, file_size))` of the output
  file, with **`W = IMAGE_BLOCK = 2^20` bytes** (1 MiB), a constant of
  this spec and part of the build id's definition (§8.3). Windows are the
  unit of buffering, of writing and of build-id hashing.
- **Job**: a run of consecutive windows filled into one buffer by one
  worker (§5.3). Almost every job is one window.
- **Reference**: a relocation record that survives sequence recognition
  and still needs its symbol (`spec/arch.md` §3.4), identified by
  **`RefId = (FileId, section index, record index)`**, ordered
  lexicographically. This order is the order of the scan's diagnostic
  keys (model §8, phase 70).
- **Event**: what `arch.scan` reports about a reference instead of a
  finished diagnostic (§4.4).
- **Output file**: the file at `-o`; **temporary file**: the file the
  image is written to before it is renamed to the output file (§7).

## 3. Liveness (phase 50)

### 3.1 The table

`Liveness` (model §3.6) has the column `live[SectionId]`, the column
`frag_live[FragId]`, and the FDE rule. `mortar_live` writes it; it reads
`Inputs`, `Resolution`, `Assignment` and `Split`. Its `check` (WP-020)
requires `live ⇒ state == Kept`.

- **I-1 [M1]** Without `--gc-sections` (the default, and the only mode of
  M1): `live[s]` is true exactly when `sections[s].state == Kept`.
  `spec/resolve.md` §10 has already dropped consumed sections, excluded
  sections, the losing copies of COMDAT groups and stripped sections.
- **I-2 [M1]** Without `--gc-sections`, `frag_live[f]` is true for every
  fragment of every piece of a live `Merge` section: every piece of a
  kept mergeable section is in the output, referenced or not. This
  replaces model §3.6's "referenced from a live section" for this mode
  (request R-4): computing references would need a relocation walk at
  phase 50 for no change in correctness, and an unreferenced string
  costs a few bytes. That GNU ld, lld and mold also keep unreferenced
  pieces without `--gc-sections` was not verified (E14).
- **I-3 [M1]** An FDE piece is live exactly when the section its code
  range lies in is live (model §3.6; under folding, M5, also not folded
  away). A dead FDE is not written (`spec/layout.md` lays out live pieces
  only); a CIE is live when a live FDE uses it.
- **I-4 [M1]** Liveness emits no diagnostic in M1. Its key, when it does
  (M2, §3.3), is `(50, FileId, section index)`.
- **I-5 [M1]** `--no-gc-sections` is the default and is `supported` in M1
  (request R-6). `--gc-sections` is `refused` in M1 (`unsupported option:
  --gc-sections`). Evidence for refusing rather than implementing now:
  none of `gcc`, `g++`, `gfortran`, `clang`, `clang++` passes it by
  default (E1); of the bench workloads only `rust-self` (rustc passes it)
  uses it; the libc++ test suite's link lines are built by the M1 driver
  and do not need it. It is M2's (`docs/ROADMAP.md`).

### 3.2 `--gc-sections` (M2)

The design is fixed now so that the M1 tables need no change.

- **I-6 [M2] Roots.** A section is a root when any of these holds:
  - it defines `def` of a key rooted by `spec/resolve.md` §4.7 (the entry,
    `-u`, `--require-defined`, every key exported to `.dynsym`);
  - it has `SHF_GNU_RETAIN` (`spec/layout.md` L-97), or a script `KEEP`
    covers it;
  - its output section is `.init_array`, `.fini_array`,
    `.preinit_array`, `.init`, `.fini`, `.ctors`, `.dtors` (start-up
    tables run by name, not by reference: `spec/layout.md` §5);
  - its type is `SHT_NOTE` (notes are read by tools and loaders, not
    referenced; lld keeps them: not verified for GNU ld and mold, E14);
  - its name is a C identifier and `__start_NAME` or `__stop_NAME` is
    referenced from a live section (`spec/layout.md` L-96: GNU ld's
    `-z nostart-stop-gc`);
  - it is not `SHF_ALLOC` (every non-alloc section is kept whole: debug
    information is not collected; all three linkers, not separately
    verified, E14).
- **I-7 [M2] Edges.** A live `SHF_ALLOC` section `s` makes live:
  - the section of `def` of every key that a relocation record of `s`
    names, through `sym_map` (a `Section` definition; a `Common`
    definition makes the commons' output live; an `Internal` definition,
    nothing; a `Shlib` one, nothing);
  - the section of every local or section symbol a record of `s` names;
    for a reference into a split section, the piece and its fragment
    (`frag_live`, by offset through `Split`, as model §3.5 resolves a
    reference);
  - the sections in `s`'s `SHF_LINK_ORDER` dependents (a section whose
    `sh_link` is `s` is live when `s` is);
  - for a code section, its live FDEs (I-3) and, through each FDE's
    relocations, the personality routine's and the LSDA's sections
    (`.gcc_except_table` pieces): the FDE's edges are `s`'s edges.
  Relocations in non-alloc sections are not edges (debug information
  never keeps code alive); in `.eh_frame` they are edges only through
  the FDE rule. A group's members are not made live together: each is
  live by its own edges (lld's rule; GNU ld's not verified, E14).
- **I-8 [M2] The algorithm.** A least fixed point, computed by parallel
  rounds: round 0 is the roots; round `k + 1` is the set of sections first
  marked by the edges of round `k`'s sections. `live` is a column of
  atomic booleans marked with `swap(true)` (model §6 rule 2: idempotent,
  commutative); the section whose swap returned `false` is collected into
  the worker's buffer for the next round, and the buffers are
  concatenated in unit order (rule 3). The resulting *set* is the least
  set closed under I-7 containing the roots, whatever the schedule; the
  order of a round's list may differ between schedules and is never
  observed (no id, diagnostic or byte depends on it). Within a round,
  work is partitioned by section and, for a section with more than
  `SCAN_BLOCK` records, by `SCAN_BLOCK` records (rule 6).
- **I-9 [M2] Cost.** One walk of the relocation records of live alloc
  sections: 221,459 for `cxx-medium-g` (E5), about 0.8 ms at the 3.4–3.7
  ns per record of a scan-like walk (E6). It is a separate walk from the
  scan's (phase 70), because the scan must know liveness first: undefined
  references are reported only from live sections (`spec/resolve.md`
  §5.1), and whether a reference survives depends on liveness of its
  target (§6.1).
- **I-10 [M2]** Liveness under `--gc-sections` does not change
  `Resolution`: extraction is decided before liveness
  (`spec/resolve.md` §4.7). `discard <file> <section> gc` records go to
  the map (§9). `--print-gc-sections` is `refused` until M2 implements
  it; its line is then GNU ld's `removing unused section '{section}' in
  file '{file}'` (not measured against lld or mold, E14).
- **I-11 [M2]** `-r` with `--gc-sections` is refused (M4 decides).
  `--gc-sections` with `-shared` (M4) roots every exported key (I-6).

### 3.3 Liveness diagnostics (M2)

- **I-12 [M2]** A reference from a live non-alloc section into a
  collected section is not an error: it writes the tombstone of I-47 when
  the referring section has a debug name, and is
  `discarded-section-reference` otherwise (`spec/resolve.md` §6.3's
  table, applied to collected sections as to dropped COMDAT copies; GNU
  ld's treatment of collected sections from non-debug non-alloc sections
  not verified, E14). The scan reports it (§4.4); liveness reports
  nothing.

## 4. The scan driver (phase 70)

### 4.1 What the driver does

`mortar_arch` provides `Arch::scan(cx, RelocAt, DemandSink)` for one
relocation, and sequence recognition across neighbouring records
(`spec/arch.md` §3.4, §3.5). The driver:

1. builds the list of **scan units**: every live section with a
   relocation section, in `(FileId, section index)` order; a unit with
   more than `SCAN_BLOCK` (2^16, `mortar_core::par`) records is split into
   blocks of `SCAN_BLOCK` records (rule 6: never into one part per
   thread). A block boundary that would split a sequence the architecture
   recognizes is moved forward past the sequence's last member, by the
   architecture's sequence predicate, which reads only the records;
2. runs the units in parallel (`for_each_part`), each over its records
   in record order;
3. merges the results (§4.3, §4.5) and emits the diagnostics.

- **I-13 [M1]** The scan reads only live sections (`Liveness`), and of
  each only its own relocation records, through `Inputs` (resident in
  both stores: `spec/input.md` §11.1). It never reads section bodies
  except where `arch.scan` checks an instruction (`spec/arch.md` §3.4,
  through `RelocAt::site`); for a non-resident body (the staged store,
  [perf]) the site is read with `copy_to` into a 16-byte buffer of the
  worker's own.
- **I-14 [M1] Non-alloc fast path.** A relocation whose place is in a
  non-alloc section raises no demand and produces no dynamic relocation
  (`spec/a64.md` §13.2). For it the driver calls the architecture's
  non-alloc check only: the type is in the allowed list, the field lies
  in the section, and the target is classified as defined, undefined
  (an event, §4.4), or in a section that is not live (§6.1). This is
  where the cost is: 87% of `cxx-medium-g`'s 2.2 M relocations are in
  `.debug_*` sections (E5).
- **I-15 [M1]** The scan's outputs and only these: the `Demands` flags
  (atomic or, rule 2), the per-section dynamic-relocation counts
  (written by the unit that scans the section; a section split into
  blocks sums the blocks' counts, an integer sum, rule 2), the
  diagnostics of `spec/arch.md` §7 at their keys, and the events of
  §4.4. No other table is written. In particular the scan writes no
  "relocations are sorted" bit or other per-section fact for fill: fill
  computes its own (§5.2).

### 4.2 Cost

- **I-16 [M1]** The cost model for the scan is one decode, one `sym_map`
  lookup and one class-table lookup per record: 3.35–3.68 ns per record
  on one thread for a scan-like walk over `cxx-medium-g`'s 2.2 M records
  (E6), so about 0.35 s of CPU per 10^8 records. Parallel speed-up of the
  scan-like walk was nil in the measurement, because every record did an
  atomic `fetch_or` on a shared flag word; the driver therefore **reads
  a key's flags before or-ing** (`load`, and `fetch_or` only when a bit is
  missing), which keeps the cache line shared in the common case of a
  flag already set. The parallel scaling of that form is not measured
  (E14); the benchmark gate of §16.5 is.

### 4.3 Errors

- **I-17 [M1]** Each unit appends its diagnostics to a buffer of its own,
  with model §8's scan key `(70, FileId, section index × 2^32 + record
  index)`. The driver concatenates the buffers in unit order (rule 3) and
  hands them to the collector, which orders by key (`spec/diag.md` §6.2).
  A unit does not stop at its first error, nor when the error limit is
  reached elsewhere (`spec/diag.md` §6.1).

### 4.4 Events (supervisor ruling of 2026-11-14)

For four kinds, one diagnostic gathers many references. `arch.scan` does
not emit them; it reports an **event** for each surviving reference, and
the driver groups the events (§4.5).

| event class | when (`spec/arch.md` §3.4, `spec/resolve.md`) | group key | diagnostic |
|---|---|---|---|
| `Undefined` | a surviving reference to a key whose target kind is `Undefined` and whose verdict (`spec/resolve.md` §5.2) is an error or a warning | `SymKey` | `undefined-symbol` |
| `ComdatMismatch` | a surviving reference to a key defined only in dropped copies of a COMDAT group (`spec/resolve.md` §6.3 case (b)) | `SymKey` | `comdat-mismatch` |
| `DiscardedSection` | a relocation in a kept section of file `F` naming a local or section symbol of a dropped section of `F`, from a section for which `spec/resolve.md` §6.3 says error (case (a)); or, under `--gc-sections`, from a live section into a collected one where I-12 says error | `(FileId, referring section index, symbol index)` | `discarded-section-reference`, one per (symbol, referring section) |
| `GnuWarning` | a reference from a live section to a key `K` with a `.gnu.warning.K` section in a live file (`spec/resolve.md` §10) | `SymKey` | `gnu-warning` |

```rust
pub enum EventClass { Undefined, ComdatMismatch, DiscardedSection, GnuWarning }
pub struct RefId { pub file: FileId, pub section: u32, pub record: u32 }  // Ord: lexicographic
pub struct Event { pub class: EventClass, pub group: u64, pub at: RefId }
// group: SymKey as u64; for DiscardedSection, (referring section index << 32) | symbol index,
// the file being at.file
```

- **I-18 [M1]** `DemandSink` gains `reference(class, group)`; the sink
  fills in `at` from the record being scanned. It replaces
  `error(Key, Diagnostic)` for these four kinds (request R-1). `arch.scan`
  reports at most one event of each class per relocation record, and
  never both `Undefined` and `ComdatMismatch`: a key defined only in
  dropped copies of a group (its `def` is `Undefined` too) reports
  `ComdatMismatch` (`spec/resolve.md` §6.3 case (b)). A matched
  sequence reports the events of the members that still need their
  symbol, each at its own record.
- **I-19 [M1]** An event is only reported for a **surviving** reference:
  a relocation consumed by a matched sequence reports none
  (`spec/arch.md` §3.4); a relocation in a section that is not live is
  never scanned; an `Undefined` target whose verdict is "nothing"
  (`--unresolved-symbols=ignore-all`, `--ignore-unresolved-symbol`, weak)
  reports none. Whether the event is an error or a warning is the
  verdict's, fixed per key, and is recorded with the group, not per
  event.

### 4.5 The deterministic merge

The sequential meaning: take every event of the link, in `RefId` order;
for each `(class, group)`, the diagnostic lists the first three events'
places and counts the rest; its key is the first event's. The parallel
formulation:

- **I-20 [M1] Per unit.** A scan unit visits its records in record order,
  so its events arrive in increasing `RefId`. It keeps an accumulator
  per `(class, group)` it has seen, in a hash map used for lookup only:

  ```rust
  struct Acc { first: [RefId; 3], len: u8, count: u64 }  // count: every event, first included
  ```

  The first three events of a group in the unit fill `first`; every
  event adds 1 to `count`. At the end of the unit the map is turned into
  a vector sorted by `(class, group)` (rule 5: no hash order escapes).
  Memory: one `Acc` (40 bytes) per distinct group per unit, only for
  links that report these kinds; a unit referencing one undefined
  symbol 10^6 times holds one `Acc`.
- **I-21 [M1] Across units.** The vectors are concatenated in unit order
  (rule 3; units are in `RefId` order because blocks of a section are in
  record order and sections in `(FileId, index)` order), then stably
  sorted by `(class, group)` (rule 4, `par::sort_by_key`, which is
  stable). The run of one group is then in `RefId` order, and is reduced
  left to right: `first` takes the first three `RefId`s across the run's
  `Acc`s in order, `count` is the sum (a sum of integers, rule 2). The
  result is exactly the sequential meaning, whatever the schedule and
  the thread count.
- **I-22 [M1] The diagnostic.** Per group, in any order (the collector
  orders by key):
  - key `(70, first[0].file, first[0].section × 2^32 + first[0].record)`
    (model §8);
  - kind and head per `spec/diag.md` §3.1 (`undefined symbol: {name}
    (undefined reference to `{name}')` and the others);
  - one `>>> referenced by {file}:({section}+0x{r_offset})` context line
    per entry of `first` (`ReferencedBy`, `spec/diag.md` §3.1), and, when
    `count > 3`, the note `>>> referenced {count − 3} more times`;
  - the extra context of each kind: `spec/resolve.md` §5.3 (hidden
    visibility, demangling), §6.3 (`comdat-mismatch`'s kept and dropped
    copies and the ODR note).
  `{r_offset}` is read from the record when the diagnostic is built (the
  record is in a resident relocation section): `RefId` is enough.
- **I-23 [M1]** Two groups share a key only when their first references
  are the same record. A record names one symbol, so that happens only
  when two classes fire on it, `GnuWarning` with `Undefined` or
  `ComdatMismatch`; the collector orders the two by text
  (`spec/diag.md` §6.2).
- **I-24 [M1] Error limit.** Groups count as one diagnostic each against
  `--error-limit` (`spec/diag.md` §6.4); every group is still built,
  because which ones are printed depends on their keys.

The undefined references of shared objects (M2) are resolution's (phase
30, `spec/resolve.md` §5.2) and do not go through the scan.

### 4.6 What the scan driver needs from other specs

- `spec/arch.md` §3.4: events in place of diagnostics for the four kinds
  (R-1).
- `spec/arch.md` §3.5: a predicate `sequence_extent(records, i) ->
  usize` saying how many records from `i` a recognized sequence may span,
  so that block boundaries are moved past it (I-13's step 1). Requested
  as R-3.

## 5. Fill (phase 100)

### 5.1 Inputs and outputs

Fill reads every table of model §3 and §4, `Inputs` (the store) and the
options; it produces the bytes of the output file, written through §7's
writer, and the diagnostics of `spec/arch.md` §7 at phase 100. It runs
only when `has_errors()` is false after layout (`spec/diag.md` §7).

- **I-25 [M1]** Every byte of the file `[0, file_size)` belongs to exactly
  one unit or one gap (§5.5). `file_size` is the end of the last chunk
  with file bytes in `Placement` (the section header table, or the last
  non-alloc chunk). `NOBITS` chunks (`.bss`, `.tbss`) have no file bytes
  and are not in the plan.
- **I-26 [M1]** No byte is read back from the output: fill writes, the
  writer writes, and nothing reads the temporary file (model §5). The
  build id (§8) hashes window buffers before they are written.

### 5.2 The fill plan

- **I-27 [M1]** Before filling, the image builds the **fill plan**: one
  row per unit with file bytes, `(offset: u64, size: u64, unit: UnitRef)`
  (24 bytes), in increasing `offset`:
  - per `Body` chunk, its placed input sections in the chunk's member
    order of `Placement`, at `chunk.offset + place.offset`;
  - per `Merged` chunk, its live fragment runs (`MergeMap`): a run is a
    maximal sequence of fragments whose bytes are contiguous in the
    chunk, cut at `IMAGE_BLOCK` boundaries of the run's own extent so that
    no run is larger than one window;
  - per `Synthetic`, `Patches` and `Group`/`Relocs` (M4) chunk, one row,
    or one per record range where the synthetic chunk is divisible
    (§13);
  - the structural units: `FileHeader`, `ProgramHeaders`,
    `SectionHeaders`.
  Rows are built in parallel per chunk (`map_collect`, rule 3) and
  concatenated in chunk order, which is file-offset order for chunks with
  file bytes (`spec/layout.md` L-70, L-72). The plan is checked once:
  offsets increase and rows do not overlap (a failure is
  `internal-error`, `invalid fill plan`: validity rules 1 and 2 of model
  §4.4 promise it).
- **I-28 [M1]** The plan's size is 24 bytes per placed section plus small
  terms: 3.5 MB for `cxx-medium-g`'s 143,907 sections at most (the live
  ones are fewer, E5). It is model §9-conforming (per section, not per
  relocation) and lives only during the phase.
- **I-29 [M1] Sorted records.** For every placed input section larger
  than `W` that has relocations, the plan records one bit: whether its
  relocation records are in non-decreasing `r_offset` order. It is
  computed by fill itself (one read of the `r_offset` fields; the bit is
  fill's cache, model §4.5). In every object of `cxx-medium-g`, glibc's
  `libc.a`, GCC 14's `libstdc++.a` and `libgcc.a`, and Clang 19 and GCC
  14 `-g` objects, all 45,861 relocation sections are sorted (E7); GNU as
  and Clang's integrated assembler keep the order of `.reloc` directives,
  so an assembler source can produce unsorted records (E7).
- **I-30 [M1] Symbol values.** Fill computes, before the windows, the
  phase cache `sym_addr[SymbolId]: u64` (model §4.5: 8 bytes per global
  symbol) from `def`, `redirect`, `place`, `sym_place`, `defined`,
  `Split` and `MergeMap`, in parallel by `SymbolId` block (rule 1).
  Local symbols are computed on use from the file's symbol table (no
  per-entry row, model §9).

### 5.3 Windows and jobs

- **I-31 [M1]** The file is filled window by window. A **job** is
  normally one window; the jobs are formed by one sequential pass over
  the plan: a placed section larger than `W` whose records are *not*
  sorted (I-29) makes one job of every window it touches, together with
  the windows already in the job. So a job is a run of windows that
  contains every unsorted large section it touches entirely. Job
  boundaries are a function of the plan and `W` only (rule 6).
- **I-32 [M1]** Jobs run in parallel (`for_each_part` over job indices;
  under `MORTAR_SCHED_SEED`, permuted). A worker owns one buffer of
  `W` bytes, reused between jobs, grown only for a multi-window job and
  shrunk back to `W` after it.
- **I-33 [M1]** For a job `[lo, hi)`, the worker:
  1. finds the first plan row ending after `lo` (binary search);
  2. for every row overlapping `[lo, hi)`, in order, fills the overlap
     `[max(lo, row.offset), min(hi, row.end))` of the unit (§5.4) into
     the matching slice of the buffer;
  3. fills each gap between rows (§5.5);
  4. per window of the job, in order: if a build id is computed, hashes
     the window's bytes (§8.3); then hands the window to the writer
     (§7.3).
  The buffer is disjoint per worker (rule 1), and a window's bytes
  depend only on the tables (model §5), so the file's bytes do not depend
  on which worker filled which window, or when.
- **I-34 [M1] Memory.** Fill's own memory is `threads × W` (8 MiB at 8
  threads), plus, for each multi-window job in flight, its length; plus
  the plan (I-28) and `sym_addr` (I-30). A link whose inputs have no
  unsorted section larger than `W` never makes a multi-window job.

### 5.4 Filling one unit, clipped

The unit's bytes are defined as if the whole unit were filled into one
buffer; the clip only selects which of them a job writes.

- **I-35 [M1] Placed input section.** For the clip `[a, b)` of section
  `s` (offsets relative to the section's start):
  1. copy the input bytes `[a, b)` of `s`: from the resident slice when
     the range is resident (`Inputs::resident`), otherwise with
     `Inputs::copy_to` into the buffer ([perf], staged store); a
     `SHF_COMPRESSED` input is copied from the store's decompressed
     buffer (`spec/layout.md` L-52);
  2. apply every relocation record whose write window
     `[r_offset − before, r_offset + width + after)` intersects `[a, b)`,
     through a `SectionOut` whose valid range is `[a, b)`: bytes the
     relocation would write outside it are not written (request R-2:
     `apply`'s results for the bytes inside the clip are those of an
     unclipped call, which holds because `apply` never reads `out`,
     `spec/arch.md` §3.6). `before`/`after` are bounded by the
     architecture's constant `Facts.max_reach` (R-2; AArch64 needs 0/0
     beyond the field, x86-64's `TLSGD` window `[P − 4, P + 12)` needs 4
     and 8 past a 4-byte field). The records are found, for a sorted
     section larger than `W`, by binary search on `r_offset` for `a −
     max_reach.after − 8` and a forward walk to `b + max_reach.before`;
     for other sections, by walking all of the section's records (the
     section is at most `W` long, or wholly inside this job);
  3. for a split `EhFrame` section, write only its live pieces at their
     output offsets (`spec/layout.md` §8), with the relocations of live
     pieces; the bytes between the input pieces do not exist in the
     output.
- **I-36 [M1] Relocation errors are reported once.** A relocation whose
  window straddles two jobs is applied by both, each writing its side.
  Its `FieldError` (`spec/arch.md` §3.6) is reported only by the job
  whose range contains `r_offset` (the first byte of its field), with
  the key `(100, FileId, section index × 2^32 + record index)`
  (`spec/arch.md` §7). A relocation's value does not depend on the job,
  so both jobs agree on whether it fails.
- **I-37 [M1] `.ctors`/`.dtors` reversal** (`spec/layout.md` L-40, when it
  is lifted): the words of a `.ctors` input section are written in
  reverse word order; the relocation of the word at offset `o` is applied
  at offset `size − width − o`. The clip applies to the reversed
  positions. Until L-40 is lifted, layout refuses and fill never sees
  one.
- **I-38 [M1] Fragment runs.** A run writes each fragment's bytes (from
  its first occurrence, `Split` §3.5) at the fragment's chunk offset.
  A fragment's relocations, if its section has any, are applied from
  its first occurrence as I-35 applies a section's (not observed in the
  inputs of `cxx-medium-g`, `libc.a` and `libstdc++.a`: no `SHF_MERGE`
  section in them has a relocation section, E7).
- **I-39 [M1] Synthetic chunks** are written by `spec/synth.md`'s
  `write(cx, range, out)` for the clip (§13). `Patches` chunks (islands,
  thunks, erratum patches) are synthetic chunks of `mortar_arch`'s
  `write_*` (`spec/arch.md` §6), with the key `(100, 0, ChunkId)`.
- **I-40 [M1] Structural units.** The ELF header, the program header
  table and the section header table are written by the image from
  `Placement` and the output kind: `e_entry` from `Placement.entry`;
  `e_shstrndx` the `.shstrtab` chunk's section index; a program header
  per `Placement.segments` row; a section header per output section
  (`sh_size` the section's last chunk's end minus its first chunk's
  start, `spec/layout.md` L-86). Their encodings are `spec/elf.md`'s
  writer half, and the byte layout is a function of the class and the
  rows; nothing in them is filled from input bytes.

### 5.5 Gaps

- **I-41 [M1]** A gap is a range of the file covered by no plan row. Its
  bytes are, by the first rule that applies:
  - inside an exec chunk, or between exec chunks of one segment: the
    trap pattern at the gap's addresses (`spec/layout.md` L-82), except
    between inputs of `.init` and `.fini`, where it is the no-op;
  - between segments under `-z separate-code`, in exec: the trap pattern
    (L-83);
  - otherwise zero (data padding, the space between segments, padding
    before non-alloc chunks and the section header table).
  The pattern comes from `Facts.code_fill` and is positioned by address
  modulo its length (AArch64: 4 zero bytes; x86: `0xCC`).
- **I-42 [M1]** Every gap is written explicitly. The output file has no
  hole the writer did not write: a sparse file would read the same zeros,
  but its blocks would be allocated by later writers (a debugger's
  in-place breakpoint patching of a copy, an incremental link) at a
  moment the user does not expect, and the zero-gap benefit is small
  (gaps are alignment padding).

### 5.6 Diagnostics of fill

- **I-43 [M1]** Fill reports `relocation-out-of-range` and
  `relocation-misaligned` (`spec/arch.md` §7) at their keys, into a
  per-job buffer, concatenated in job order (rule 3) and ordered by the
  collector (rule 4). A job does not stop at an error. When any is
  reported, the link fails after fill and the temporary file is removed
  (§7.4).
- **I-44 [M1]** A slot that `FillCx::slot` cannot find, a redirect that
  does not exist, or a plan check that fails is `internal-error`
  (`spec/arch.md` §3.6: §5.2 was violated), with the table and rule in
  `{what}`.

## 6. Non-alloc and debug sections

### 6.1 References into sections that are not in the output

A relocation in a live non-alloc section whose target lies in a section
that is not live (a dropped COMDAT copy, M1; a collected section, M2)
writes a value chosen by the referring section's name, as
`spec/resolve.md` §6.3's table sorts names:

- **I-45 [M1]** Referring section with **a GNU ld debug name**
  (`spec/resolve.md` §6.3: prefix `.debug`, `.zdebug`, `.line`, `.stab`,
  `.gnu.linkonce.wi.`, `.gnu.debuglto_.debug_`, exactly
  `.gnu_debuglink`): the **tombstone** of I-47, whatever the addend.
- **I-46 [M1]** Exactly `.gcc_except_table` (alloc or not): 0
  (`spec/resolve.md` §6.3). Any other non-alloc section: the scan
  reported `discarded-section-reference` (§4.4) and fill never runs.
- **I-47 [M1] [C] The tombstone.** `tombstone(section, width)`:
  - `.debug_ranges` and `.debug_loc`: **1**. These are DWARF ≤ 4 lists
    of address pairs; a pair `(0, 0)` ends the list early and a first
    word of all ones is a base-address selection entry, so neither 0 nor
    all ones is safe. `(1, 1)` is an empty range. All three references
    write 1 here (E9);
  - every other debug-named section: **all ones** in the field's width
    (`0xffffffffffffffff` for 64-bit fields, `0xffffffff` for 32-bit,
    `0xffff` for 16-bit), the value DWARF 6 reserves as the tombstone
    of an address and the value lld writes under
    `-z dead-reloc-in-nonalloc=<section>=0xffffffffffffffff`.
  This replaces `spec/arch.md` §3.11's 0 (request R-5). The evidence
  (E9, E10):
  - lld 22 and mold write 0 into `.debug_info`, `.debug_line`,
    `.debug_aranges`, `.debug_frame`, `.debug_loclists`,
    `.debug_rnglists` and `.debug_addr` for references into a dropped
    COMDAT copy; GNU ld writes the kept copy's address plus the addend
    when the two copies have the same size, and 0 otherwise. Measured
    for GCC 14 and Clang 19 objects, DWARF 4 and 5.
  - A zero tombstone is an address. When code is linked at a low address
    the dead function's DWARF claims `[0, size)` overlaps real code.
    With `-z max-page-size=4096 -z noseparate-code`, lld's and mold's
    `.text` starts at `0x1740`, inside the dead copy's claimed range
    `[0, 0x2a44)`: **lldb 19 loses the file and line of `big`, `helper`
    and `main`** (`big(int) + 8` with no `h3.h:3:16`), on lld's and
    mold's outputs, with GCC and Clang objects; GNU ld's redirected
    output is correct. gdb 16 shows identical `break`, `bt`, `info line`,
    `list` and `info scope` results for all three linkers in every
    layout tried.
  - The same link with lld and `-z dead-reloc-in-nonalloc=X=0xffff…`
    for `.debug_info`, `.debug_line`, `.debug_aranges`, `.debug_addr`,
    `.debug_rnglists`, `.debug_loclists` and `.debug_frame` gives lldb
    the correct file and line; gdb is unchanged;
    `llvm-dwarfdump-22 --verify` reports no errors and `addr2line` is
    correct for every variant.
  - mortar can produce low text addresses: x86-64's maxpage is 4 KiB
    (`spec/layout.md` L-74, M3), and `-z max-page-size` lowers it on
    AArch64. GNU ld's redirect was rejected: it states that the dropped
    copy's code is at the kept copy's address, which is true only if
    the two copies are the same code, and the linker does not know that
    (that is the ODR question `mortar_odr` exists for, M6).
- **I-48 [M1]** The tombstone is truncated to the field, never checked
  for range (it is not a value of `X`), and replaces `X` for every type
  allowed in a non-alloc section (`spec/a64.md` §13.2), including
  `PREL32` and `TLS_DTPREL64`.

### 6.2 Debug sections

- **I-49 [M1]** `.debug_*` sections are ordinary units: `Body` chunks of
  their own output section (`spec/layout.md` L-46), filled per §5.4, and
  `.debug_str` / `.debug_line_str` are `Merged` chunks when their inputs
  are `SHF_MERGE|SHF_STRINGS` (I-38). Debug information is data: the
  image does not parse DWARF in M1 (no `.debug_aranges` rebuild, no
  `.debug_line` rewriting): all three references copy and relocate it
  too (E9).
- **I-50 [M1] Volume.** Debug bytes dominate a debug link: in
  `cxx-medium-g`, 102 MB of the 132 MB of section bytes are `.debug_*`
  and 87% of the relocations are in them (E5); in lld's output (48.3 MB)
  `.debug_info` is 25.9 MB, `.debug_str` 8.1 MB (merged) and
  `.debug_line` 2.7 MB, against 3.1 MB of `.text` (E4).
- **I-51 [M1] `--compress-debug-sections`.** `=none` is `supported` (the
  default); `zlib`, `zlib-gnu`, `zlib-gabi` and `zstd` are `refused`
  (`option value`) until M4. None of the system archives and crt
  objects carry `SHF_COMPRESSED` sections, and `gcc -g` does not
  compress by default (E8). Input-side `SHF_COMPRESSED` is
  `spec/layout.md` L-52's (the store decompresses).
- **I-52 [M4] Compressed output design.** A compressed non-alloc output
  section is a `sized` chunk (`spec/layout.md` L-121) whose content is
  produced after the fixed point: its uncompressed bytes are filled in
  fixed blocks of `W` (as windows of a virtual file), each block
  compressed independently into a stream that concatenates (a zstd
  frame per block; for zlib, deflate blocks ended with a full flush and
  one Adler-32 combined from the blocks' checksums), so the compressed
  size and bytes are functions of the content and `W`, not of threads.
  The chunk's size is the compressed size plus the `Elf_Chdr`. Whether
  mortar may depend on a compression crate is the supervisor's decision
  (no shipped dependency without approval, `docs/ARCHITECTURE.md`).
- **I-53 [M2] `-S` / `--strip-debug`, `-s` / `--strip-all`.** `refused`
  in M1. Their effect on the image is nil: `spec/resolve.md` §10 drops
  `.debug_*` (`Dropped(Stripped)`) and `spec/synth.md` omits `.symtab`
  and `.strtab` under `-s`; references into stripped sections do not
  occur (stripped sections are referring sections, never targets of
  alloc code).
- **I-54 [M4]** `.gdb_index` and `.debug_names` are synthetic chunks of
  `spec/synth.md` that read DWARF; they are divisible by CU range (§13).
- **I-55 [M1]** The image adds no bytes of its own to any section other
  than synthetic and structural units. In particular it writes no
  identification string into `.comment` (lld and mold do, GNU ld does
  not; whether `spec/synth.md` adds one is its decision).

## 7. The output file

### 7.1 What the references do (E2)

Observed on this host for GNU ld 2.44, lld 22 and mold 2.37.1, each
linking a small static program over an existing output:

| situation | GNU ld | lld | mold |
|---|---|---|---|
| output exists (regular file) | new inode | new inode | new inode |
| another hard link to the old output | keeps the old bytes | keeps the old bytes | keeps the old bytes |
| output is a symbolic link | replaced by a regular file | replaced | replaced |
| mode of an existing output | not kept: `0777 & ~umask` (755 at umask 022, 700 at 077) | same | same |
| mode with `-r` / `-shared` | 644 / 755 | 644 / 755 | 644 / 755 |
| `-o DIR` | error, `Is a directory` | error | error |
| `-o /dev/null` | written | written | written |
| output is a running executable | replaced (new inode) | replaced | replaced |
| the link fails (undefined symbol, missing input) | **the old output is removed** | old output kept | old output kept |
| the link fails on a bad option | old output kept | kept | kept |
| directory not writable, output writable (as an unprivileged user) | succeeds, writes in place | fails: `cannot open output file …: Permission denied` | fails: `cannot open d/.x.<pid>` (a temporary name) |
| output is one of the inputs | refused: `input file 'in.o' is the same as output file` | succeeds | succeeds |

Opening a running executable for writing fails with `ETXTBSY` on Linux
6.12 (E2), which is why every reference replaces rather than rewrites.

### 7.2 Creating it

- **I-56 [M1]** The output path `P` is `-o`'s value, or `a.out`. The
  image creates the output file only when `has_errors()` is false after
  layout (`spec/diag.md` §7), before fill starts, so that an output that
  cannot be created is reported before the work of fill.
- **I-57 [M1]** If `P` names a directory (following symbolic links),
  the error is `cannot-open-output` with the reason `Is a directory`.
- **I-58 [M1]** If `P` exists and is neither a regular file nor a
  directory nor a symbolic link (a character device such as `/dev/null`,
  a FIFO, a socket), the image opens `P` for writing without creating or
  truncating it, and writes it directly (§7.3). A file that does not
  accept positioned writes (a FIFO: `ESPIPE`) fails with
  `cannot-write-output` and the system's reason (only `/dev/null` was
  observed, E2).
- **I-59 [M1] [C] Otherwise: a temporary file.** The image creates
  `T = dir(P)/.<base(P)>.mortar.<pid>` with `OpenOptions::create_new`
  (`O_CREAT|O_EXCL`), write-only, mode `0o777` for executables and shared
  objects and `0o666` for `-r` (the kernel applies the umask, which
  reproduces the references' modes of §7.1). If `T` exists (a stale file
  of a crashed process with the same pid), the names `T.1`, `T.2`, …,
  `T.63` are tried; then `cannot-open-output`. Every failure to create
  `T` is reported as `cannot-open-output` **naming `P`**, not `T`, with
  the system's reason: a read-only directory gives lld's line,
  `cannot open output file P: Permission denied`.
- **I-60 [M1]** After a successful fill and sealing (§8), the descriptor
  is closed and `T` is renamed to `P` (`std::fs::rename`). This gives
  §7.1's rows: a new inode, hard links unaffected, a symbolic link
  replaced, a running executable replaced. A failed rename is
  `cannot-write-output` and `T` is removed.
- **I-61 [M1]** No `fsync` of `T` or of the directory: a linker's output
  is a build artifact, re-creatable, and an `fsync` of a 2 GB debug
  output would wait for the device. (Whether the references sync was not
  measured, E14.)
- **I-62 [M1] [C] The output is an input.** Accepted (lld, mold). It is
  safe: the resident store has read every input whole before fill
  starts (`spec/input.md` §11.2), and the staged store ([perf]) reads
  through descriptors that pin the old inode, which the rename does not
  change (§11.4). GNU ld's refusal guards against writing in place, which
  mortar never does.

### 7.3 Writing

- **I-63 [M1]** Each window is written by the worker that filled it, at
  its file offset, with `FileExt::write_all_at` on the shared descriptor
  (a `&File` is `Sync`); there is no writer thread and no ordering
  between windows. The file is not pre-sized: every byte of `[0,
  file_size)` is written (I-25, I-42), so the final length is the end of
  the last window. Pre-sizing with `set_len` made no measurable
  difference (E3: 2 GB on `/tmp`, 1.134 s with against 1.159 s without).
- **I-64 [M1]** Any write error (`ENOSPC`, `EIO`, `EFBIG`, a short write
  that `write_all_at` reports) is `cannot-write-output` with the
  system's reason (request R-7). The worker records the error and stops
  writing; other workers finish their current job and stop (whether the
  link fails does not depend on timing: an error was reported). The
  reported error is the one of the lowest window that failed, so the
  diagnostic is a function of the failure, not of the schedule.

### 7.4 Failure

- **I-65 [M1]** If the link fails after `T` was created, `T` is
  removed.
- **I-66 [M1] [C]** If the link fails at any phase after the command
  line (ordinal ≥ 15), an existing **regular** output `P` is removed,
  unless it is the same file (device and inode) as one of the inputs.
  This is GNU ld's
  behaviour (§7.1); lld and mold keep the stale output. The reason: a
  build that ignores the status, or a developer who runs the program
  after a failed link, would run the previous program believing it is
  the new one; a missing file cannot be mistaken. A failure on the
  command line keeps the output, as all three do. An output that is also
  an input is never removed (it would destroy the input). A symbolic
  link at `P` is removed, not its target; a non-regular `P` is left
  alone. (Open question Q3.)
- **I-67 [M1]** A link killed by a signal leaves `T` behind (safe Rust
  installs no handler). A later link never removes another process's
  temporary files; I-59's retry only steps past them.

### 7.5 Keys

- **I-68 [M1]** `cannot-open-output` and `cannot-write-output` have the
  key `(105, 0, 0)`: a new phase ordinal, "output file", between image
  (100) and verify (110) (request R-4). At most one is reported per link
  (§7.3).

## 8. The build id

### 8.1 What the references do (E11, E12, E13)

- GCC's and Clang's drivers pass bare `--build-id` on every link,
  `-static` included (GCC's spec has `%{!r:--build-id}`), and none of
  them passes a style (E1). Every bench workload's link line has it. So
  the build id's cost is paid by every link a developer makes.
- Bare `--build-id` gives 20 bytes in GNU ld, lld 19, lld 22 and mold
  (SHA-1-sized). `md5` gives 16 bytes in all three. `fast` gives 8 bytes
  in lld, 32 in mold, and GNU ld 2.44 rejects it with a warning
  (`unrecognized --build-id style ignored`) and writes no note; `sha256`
  gives 32 bytes in mold, is a warning in GNU ld and an error in lld 19
  and 22. `uuid` gives 16 random bytes. `0xHEX` gives the bytes
  (§8.2's table for its edge cases).
- None of the three references' ids equals the SHA-1 (or MD5) of the
  output with the id field zeroed, nor a two-level tree of it with 64
  KiB, 1 MiB or 4 MiB leaves (E12). No tool can recompute any of them,
  and none tries: the id is compared, not verified.
- Lookup of separate debug files by `/usr/lib/debug/.build-id/xx/rest.debug`
  works in gdb 16 for ids of at least 2 bytes and in lldb 19 for ids of
  at least 8 bytes (an id of 1 or 2 bytes is not found by lldb; 1 byte
  not by gdb); `eu-unstrip -n` lists ids of every length tried, from 8
  bytes up (E13).

### 8.2 Styles

- **I-69 [M1]** `--build-id[=STYLE]`, `--no-build-id`:

  | spelling | mortar | length | milestone |
  |---|---|---|---|
  | absent, `--build-id=none`, `--no-build-id` | no note | — | M1 |
  | `--build-id` (bare, `Default`) | `sha1` (Q1) | 20 | M1 |
  | `--build-id=sha1` | SHA-1 tree (§8.3) | 20 | M1 |
  | `--build-id=fast` | XXH64 tree (§8.3) **[C]** | 16 | M1 |
  | `--build-id=md5` | MD5 tree | 16 | M2 (refused in M1) |
  | `--build-id=sha256` | SHA-256 tree **[C]** (mold accepts it; GNU ld and lld do not) | 32 | M2 (refused in M1) |
  | `--build-id=0xHEX` | the bytes of `HEX` | `len(HEX)/2` | M1 |
  | `--build-id=uuid` | refused (`option value`) **[C]** (Q4) | — | — |
  | any other | `invalid-option-value` | — | M1 |

  The last of these options on the line wins (`spec/cli.md` §2.10).
- **I-70 [M1] [C] `0xHEX`.** `HEX` is one or more pairs of hexadecimal
  digits, either case; `0x` is case-sensitive. An odd number of digits,
  no digits, or a non-hex digit is `invalid-option-value` with the note
  `expected an even number of hexadecimal digits`. Observed (E11):

  | value | GNU ld | lld 22 | mold |
  |---|---|---|---|
  | `0xABCD` | `ab cd` | `ab cd` | `ab cd` |
  | `0x00` | `00` | `00` | `00` |
  | `0xabc` (odd) | warning, no note | `ab 0c` | error `invalid build-id` |
  | `0x0` | warning, no note | `00` | error |
  | `0x` | warning, no note | a note with no bytes | error |
  | `0xzz` | warning, no note | error `not a hexadecimal value` | error |

  mortar follows mold: an id that is not what was written is worse than
  an error.
- **I-71 [M1] [C] The note** is `spec/synth.md`'s chunk
  `.note.gnu.build-id` (`spec/layout.md` rank 2): `namesz = 4`, `descsz
  = L`, `type = NT_GNU_BUILD_ID (3)`, name `GNU\0`, the descriptor, and
  zero padding to a multiple of 4 bytes (`size = 16 + align_up(L, 4)`).
  The references write `size = 16 + L` (18 bytes for `0xABCD`, which
  `readelf -n` 2.44 warns about: `note with invalid namesz and/or descsz`,
  E11); the padding follows the ELF note format. That `readelf` accepts
  the padded form was not verified (no mortar output exists yet).

### 8.3 The function

- **I-72 [M1]** For a hash style with hash function `H` and length `L`:
  let `F` be the output file's bytes with the descriptor `[d, d + L)`
  replaced by zeros, `n = file_size`, `B = IMAGE_BLOCK = 2^20`, and
  `m = ⌈n / B⌉`. Then

  ```text
  leaf_i = H(F[i·B, min((i+1)·B, n)))                    for 0 ≤ i < m
  root   = H("mortar-build-id-1\0" ‖ le64(n) ‖ le64(B) ‖ leaf_0 ‖ … ‖ leaf_{m-1})
  id     = root[0 .. L]
  ```

  `H` is SHA-1 (`sha1`, 20 bytes), MD5 (`md5`), SHA-256 (`sha256`), or,
  for `fast`, `X(b) = le64(XXH64(b, seed 0)) ‖ le64(XXH64(b, seed 1))`
  (16 bytes). The tag names the version: a change of the function changes
  the tag. The definition is public so that `mortar_plumb` can recompute
  and check it (request R-8).
- **I-73 [M1]** The leaves are window digests: a job hashes each of its
  windows after filling and before writing it (I-33 step 4), into
  `leaves[i]` (a column indexed by window: rule 1). The window holding
  the descriptor is hashed with the descriptor's zeros, as the synthetic
  chunk wrote it. After the last job, the root is computed on one thread
  (`m × 20` bytes: 41 KB for a 2 GB output) and the id is written with
  one `write_all_at` of `L` bytes at `d` into `T`, before the rename
  (the sealing pass of model §5).
- **I-74 [M1] Why a tree** (E12, E6): a whole-file SHA-1 is sequential:
  at 267–279 MB/s per thread (a from-scratch scalar implementation in
  safe Rust), a 2 GiB debug output would take 7.7–8.0 s after the fill,
  longer than every other phase of the link. The tree's leaves are
  hashed in parallel, by the workers that already hold the window in
  cache:

  | 2 GB of random bytes, 1 MiB leaves | 8 threads | 16 threads |
  |---|---|---|
  | SHA-1 tree | 1.10 s | 0.66 s |
  | SHA-256 tree | 1.53 s | 1.06 s |
  | XXH64 tree | 0.022 s | 0.017 s |

  For comparison, writing 2 GB from 8 workers to `/tmp` takes 1.06–1.75
  s (E3), and writing does not scale with workers (§10.2), so the
  hashing may largely overlap the writes; the combined figure is not
  measured (E14). For `cxx-medium-g`'s 48 MB output, a SHA-1
  tree costs about 0.18 s of CPU in all, about 25 ms on 8 threads
  (estimated from the per-thread rate, not measured in a link). Since
  no tool recomputes any linker's id (§8.1), the tree costs no
  compatibility. A faster scalar SHA-1 would need unrolled rounds (an
  estimated factor up to 2, not measured), and hardware SHA instructions
  need `unsafe` intrinsics (`docs/ARCHITECTURE.md` principle 4: a
  measured case, later).
- **I-75 [M1]** The block size `B` is part of the function, not a tuning
  knob: it equals `W` so that a window is a leaf. Changing `W` changes
  every id (and requires a new tag).
- **I-76 [M7]** An incremental link keeps `leaves` in the link state
  (20 bytes per MiB of output), rehashes the windows it refills, and
  recomputes the root: its cost is proportional to the refilled bytes,
  plus `m` digests. A change of `n` changes the root through `le64(n)`.

## 9. The map file

### 9.1 What exists

- `-Map=FILE` (or `-Map FILE`) and `-M` (`--print-map`) are GNU ld's;
  the text formats of GNU ld, lld and mold differ (`spec/plumb.md` §3.4
  parses all three). None of the bench workloads asks for a map (E1). No
  build tool on the host parses one: the only one installed is CMake
  3.31, which passes map options only for TI and ARM embedded compilers'
  linkers (E8).

### 9.2 mortar's map

- **I-77 [M1] [C]** `-Map=FILE` writes `spec/plumb.md` §3.2's
  `mortar-map 1` format to `FILE`; `-M` writes it to standard output
  after a successful link (a failed link prints nothing on standard
  output, `spec/diag.md` §7). GNU ld's text format is not produced until
  a consumer needs it (Q5). `--cref` is `refused`.
- **I-78 [M1] Records.**
  - `file` per live input file, in `FileId` order; the map's file ids
    count from 0 in that order (`FileId` 0, mortar's own file, has no
    record); `<path>` the found path (`spec/input.md` §4.5), with
    `<member>` and the member header's offset for an archive member;
  - `place` per placed input section of an `SHF_ALLOC` output section,
    `<addr>` its address (`chunk.addr + place.offset`), `<size>` its
    size; `NOBITS` sections included (they have addresses);
  - `piece` per live piece of an alloc split section (`Merge` and
    `EhFrame`), with its input offset, address and size;
  - `discard` per section of a live file that has bytes and is not in
    the output: `comdat` (`Dropped(Comdat)`), `gc` (not live under
    `--gc-sections`, M2), `script` (`/DISCARD/`, M4), `unspecified`
    (`Dropped(Excluded)`, `Dropped(Stripped)`). Consumed sections
    (symbol tables, relocations, groups) are not listed;
  - `chunk` per synthetic chunk with bytes (`synthetic`), patch island
    or thunk (`patch`, `thunk`), and the commons' chunk (`common`);
  - `define` per symbol of file 0 that layout defined
    (`spec/layout.md` §13), with its value;
  - non-alloc input sections: none until `spec/plumb.md` has records for
    them (request R-9: `nplace` for a section at an offset of a non-alloc
    output section, `merged` for a merged non-alloc section whose pieces
    are not listed). `mortar_plumb` stage 1 does not check non-alloc
    relocations (`spec/plumb.md` §1), so nothing is lost in M1.
- **I-79 [M1]** The map is a function of the tables and the file names,
  written after layout and independent of fill. Each record group is
  formatted in parallel per file (`map_collect`, rule 3) and the buffers
  are concatenated in `spec/plumb.md`'s group order. Its size is about 40
  bytes per placed section and per alloc piece. It is written with the
  same writer as the output (temporary file, rename, §7), only for a
  successful link; a failed link removes nothing at `FILE` (not
  compared with the references, E14).

## 10. I/O: the measured design

### 10.1 The candidates

Every candidate was written as a throwaway program (E3) that produces an
output of `S` MiB of pseudo-random bytes from 8 workers in 1 MiB blocks,
into a temporary file renamed at the end, and reports wall time and the
growth of `VmHWM`:

| name | how | memory beyond the inputs | safe Rust |
|---|---|---|---|
| `vec` | the whole image in one `vec![0; n]`, filled in parallel, one `write_all` | the output's size | yes |
| `vecpar` | the same buffer, written by parallel `write_all_at` of 4 MiB | the output's size | yes |
| `chunk` | each worker fills a reused 1 MiB buffer and writes it with `write_all_at` | workers × 1 MiB | yes |
| `chunk+setlen` | `chunk` after `set_len(n)` | workers × 1 MiB | yes |
| `stream` | `chunk`, but the bytes come from a positioned read of a source file (the staged store's path) | workers × 1 MiB | yes |
| `cfr` | per block, `std::io::copy` from a `take` of the source file to the output, which std performs with `copy_file_range` on Linux | none | yes |
| `mmap` | `ftruncate`, `MAP_SHARED`, `memcpy` (lld's and mold's method), in C | the output's size | no |
| `mmapf` | `mmap` after `posix_fallocate` | the output's size | no |
| `mmapin` | the source mapped too | twice the output's size | no |

### 10.2 Results (E3; 8 workers; total seconds, median of 3; load 28–36)

On `/tmp`:

| MiB | vec | vecpar | chunk | chunk+setlen | stream | cfr | mmap | mmapf | mmapin |
|---|---|---|---|---|---|---|---|---|---|
| 10 | .006 | .008 | .005 | .005 | .010 | .006 | .006 | .005 | .007 |
| 200 | .123 (.108–.278) | .132 | .085 (.077–.096) | .090 | .085 | .085 | .085 | .073 | .094 |
| 2048 | 1.597 (1.47–6.21) | 1.864 | 1.159 (1.06–1.75) | 1.134 | 1.225 | 1.529 (.99–2.15) | 1.148 | .855 | 1.072 |

On the workspace mount (a host share):

| MiB | vec | vecpar | chunk | chunk+setlen | stream | cfr | mmap | mmapf | mmapin |
|---|---|---|---|---|---|---|---|---|---|
| 10 | .085 | .082 | .090 | .083 | .095 | .030 | .048 | .040 | .068 |
| 200 | 1.63 | 1.58 | 1.53 | 1.70 | 1.49 | .33 | 4.09 | 1.76 | 1.91 |
| 2048 | 16.1 | 17.2 | 16.6 | 16.1 | 14.9 | 4.07 | 8.5 | 6.15 | 11.5 |

Memory, measured as the growth of `VmHWM`: `chunk` and `stream` about 7
MiB; `vec`, `vecpar` and `mmap` the output's size; `mmapin` twice; `cfr`
nothing. On the workspace mount the growth for 2 GB varied from run to
run: 599–1454 MiB for `mmap` and 65–2048 MiB for `mmapf` (dirty shared
pages awaiting writeback count as resident).

Scaling on `/tmp`, 2 GB: `chunk` 0.67–0.88 s on 1 worker, 0.74–1.19 on 4,
1.06–1.75 on 8, 0.85–1.08 on 16; `mmap` 0.80–0.85 on 1, 0.83–0.92 on 4,
1.02–1.10 on 16; `mmapf` 0.81–0.94 on 1, 0.70–0.77 on 4, 1.22–1.54 on
16. At 200 MB, `chunk` 0.073–0.078 s on 1 worker and 0.078–0.122 on 16.
Block sizes of 64 KiB, 256 KiB and 4 MiB gave the same times as 1 MiB
within the noise.

### 10.3 What the numbers say

- **Writing one file does not scale with threads**, by any method: about
  2–3 GB/s on `/tmp` from 1 worker or 16, for positioned writes and
  for a shared mapping alike. The likeliest cause is the per-inode lock
  on buffered writes and page-cache insertion; it was not traced (no
  `strace` or `perf` on the host, E14). So the write's floor is the
  file system's, and parallelism buys only the filling, hashing and
  relocating, which must happen anyway.
- **Safe positioned writes cost nothing against plain `mmap`** on the
  local file system: `chunk` / `mmap` = 1.00 at 200 MB and 1.01 at 2 GB,
  with 7 MiB of extra memory instead of the output's size. Against
  `mmap` with `posix_fallocate` (which std does not offer), `chunk` is
  1.16× slower at 200 MB and 1.36× at 2 GB. On the host share, `mmap` is
  erratic: 2.7× *slower* than `chunk` at 200 MB (4.09 against 1.53 s)
  and 1.95× faster at 2 GB (8.5 against 16.6 s), and its dirty pages
  count as resident memory (599–1454 MiB at 2 GB).
- **The whole-image buffer is worse on every count**: `vec` / `chunk` =
  1.45 at 200 MB and 1.38 at 2 GB on `/tmp`, and the output's size in
  memory (the kernel must also fault in and zero the 2 GB first).
- **`copy_file_range`** is 3.7–5.2× faster than every other method on the
  host share (it avoids moving bytes through the process), and no faster
  on `/tmp`. It can only copy bytes the linker does not touch: no
  relocation, and no build-id hashing, because the bytes never reach the
  process. Since every driver passes `--build-id` (§8.1) and debug
  sections are dense with relocations (I-50), its applicability in real
  links is close to nil. Not adopted; recorded for the performance
  package (Q8).
- **For scale**: lld 22 links `cxx-medium-g` in 0.72–0.75 s on `/tmp`,
  mold (`--no-fork`) in 0.37–0.43 s, GNU ld in 1.90–2.36 s (E4). Writing
  its 48 MB output at 2–3 GB/s is 16–24 ms: about 5% of mold's link. On
  the host share the same links take 1.00–1.44 s (lld), 0.82–0.97 s
  (mold) and 5.3–6.8 s (GNU ld): the file system dominates GNU ld's.

### 10.4 The M1 design

- **I-80 [M1]** Fill by windows into per-worker buffers, written with
  positioned writes (`chunk`), as §5.3 and §7.3 specify; inputs from the
  resident store. No mapping, no whole-image buffer, no
  `copy_file_range`.
- **I-81 [M1]** The expected cost of writing is the file system's floor
  (§10.3), the same as lld's and mold's method on a local file system,
  with `threads × 1 MiB` of buffers instead of the output's size.

### 10.5 The performance design ([perf], after M2)

- **I-82 [perf]** The staged store (`spec/input.md` §11.1 (b)): large
  section bodies (`.debug_*`, large `.text`) are never resident; fill
  reads each clip with `copy_to` straight into the window buffer
  (positioned reads from the page cache). The `stream` measurement is
  this path: 1.06× the time of `chunk` at 2 GB and equal at 200 MB on
  `/tmp`, while removing the debug bytes from resident memory: 102 MB
  of `cxx-medium-g`'s 201 MiB of inputs (E5). The budget of §11.3 then
  drops by that amount.
- **I-83 [perf]** The window plan already gives each worker a
  contiguous output range; with the staged store, the reads of one
  window come from a few input files in increasing offsets, which the
  kernel's readahead serves.
- **I-84 [perf]** Mapping inputs or the output is not part of the
  design. The measurement it would have to beat is §10.2's: parity with
  `mmap`, and 1.16–1.36× against `mmap` with `fallocate`, on the local
  file system. A request for `unsafe` (`docs/ARCHITECTURE.md` principle
  4) would need a link-level measurement showing that difference, with
  the `SIGBUS` handling `spec/input.md` §11.1 describes.

### 10.6 The benchmark's own scratch directory

The bench harness writes its outputs to `.build/bench-work/scratch`, on
the workspace mount (`spec/harness.md`, Measurement). Writing 200 MB
there takes 18× as long as on `/tmp` with `chunk` (1.53 against 0.085
s), and the references' link times rise 1.4–2.9× (§10.3). A benchmark of
mortar's I/O on that mount measures the host share. Request R-10 asks
the harness to write its scratch outputs to a local file system and to
record which.

## 11. Threads, `--time-phases`, memory

### 11.1 `--threads`

- **I-85 [M1]** `--threads=N`, `--thread-count N` (`N` from 1 to 1024),
  `--threads` alone (all), `--no-threads` (1): `spec/cli.md` §3.10's
  `threads`. The default and "all" are
  `std::thread::available_parallelism()` (which follows the CPU affinity
  mask and, on Linux, the cgroup CPU quota). `N` outside 1..=1024 is
  `invalid-option-value`. The rows become `supported` (R-6).
- **I-86 [M1]** The count sizes `mortar_core::par::Pool`; one thread runs
  every helper inline (`Pool::is_inline`). The output, the map and every
  diagnostic are identical for every count (rule 6, §16.2).
- **I-87 [M1]** mortar is not a client of make's jobserver in M1: it
  uses its thread count whatever `MAKEFLAGS` says, as linkers commonly
  do. Whether to read `--jobserver-auth` is Q9.

### 11.2 `--time-phases`

- **I-88 [M1]** `--time-phases` prints, to standard error after the link
  (successful or failed), one line per phase that ran, in pipeline
  order, then a total:

  ```text
  mortar: time-phases: 20 input: wall 112.4 ms, cpu 640 ms, rss 203.1 MiB, peak 204.0 MiB
  mortar: time-phases: total: wall 401.7 ms, cpu 1920 ms, peak 251.3 MiB
  ```

  - `wall` from `std::time::Instant`, to 0.1 ms;
  - `cpu`: the change of `utime + stime` (fields 14 and 15 of
    `/proc/self/stat`, all threads of the process) over the phase, in
    ms; its resolution is the clock tick, 10 ms (`CLK_TCK` = 100 on this
    host, E15);
  - `rss`: `VmRSS` of `/proc/self/status` at the phase's end;
  - `peak`: `VmHWM` at the phase's end, the process's peak so far, so a
    phase that raised the peak shows it as an increase.
  A value that cannot be read is `-`. The phase names are model §8's
  (`command line`, `input`, `resolve`, `assignment`, `liveness`, `split`,
  `scan`, `synthetic`, `layout`, `image`, `output file`, `verify`).
- **I-89 [M1]** The lines are not diagnostics: no key, not counted by
  `--error-limit`, not hidden by `-w`, not part of any determinism test.
  They appear after the diagnostics.
- **I-90 [M1] The peak is never reset.** Writing `5` to
  `/proc/self/clear_refs` resets `VmHWM`, which would give per-phase
  peaks, but it also lowers what the parent sees: a process that peaked
  at 308 MB and reset before exiting reported 1.6 MB to
  `/usr/bin/time` (`ru_maxrss`, E15). The bench harness measures memory
  that way, so a reset would falsify it.
- **I-91 [M1]** The calls are safe std file reads; they cost three small
  reads per phase and are made only under `--time-phases`.

### 11.3 The peak-memory budget

- **I-92 [M1]** mortar's peak resident memory is bounded by

  ```text
  peak ≤ R + D + T + P + threads × W + J + E
  ```

  - `R`: resident input bytes (`spec/input.md` §11.7; the resident store:
    every input byte read);
  - `D`: decompressed `SHF_COMPRESSED` inputs (the store's buffers);
  - `T`: the tables of model §3–§4 at the row sizes WP-020's landed record
    states, and the phase caches (`sym_addr`, 8 B per global symbol;
    the fill plan, 24 B per placed section; build-id leaves, 20 B per MiB
    of output);
  - `P`: synthetic chunks' non-divisible working sets (§13: a sorted
    `.eh_frame_hdr` table, 8 B per live FDE; string tables are divisible
    and hold no copy);
  - `threads × W`: fill's window buffers (I-34);
  - `J`: the multi-window jobs in flight, at most `threads` × the
    largest unsorted section larger than `W` (0 in every measured input,
    E7);
  - `E`: the event accumulators (§4.5), only in links that report them.
  There is no term in the output's size: the image is never resident.
- **I-93 [M1]** For `cxx-medium-g`, `R` is 201.1 MiB (`spec/input.md`
  §11.8). The references' peaks for this link (E4, `/usr/bin/time`
  `ru_maxrss`): GNU ld 217 MB, lld 22 332 MB, mold (`--no-fork`)
  409–423 MB. The bench gate (§16.5) checks mortar's measured peak
  against the formula's value with `T` from the landed row sizes, plus a
  slack of 32 MiB for allocator overhead and stacks, and reports the
  ratio to lld's and mold's peaks.
- **I-94 [perf]** With the staged store, `R` loses the non-resident
  section bodies (§10.5).

## 12. Incremental linking constraints (M7)

`spec/incr.md` (M7) owns the link state; `spec/layout.md` §18 owns where
things move. The image's part:

- **I-95 [M7]** An incremental link refills whole units (model §7.2):
  the units the dependency index names, plus the units that moved. The
  **dirty windows** are the windows their file ranges overlap. Only
  dirty windows are filled, hashed and written; every byte of a dirty
  window is refilled from the plan (a window's bytes are a function of
  the tables, I-33), so neighbours of a changed unit in the same window
  are rewritten with their unchanged bytes.
- **I-96 [M7] Writing in place.** The output is updated in place only
  when all of these hold; otherwise the whole output is written as in
  §7:
  - the link state records `(device, inode, size, mtime, ctime)` of the
    output it wrote, and `P` still has them;
  - `P` can be opened for writing: a running executable gives `ETXTBSY`
    (E2). Then mortar copies `P` to `T` (with `std::io::copy`, which on
    Linux uses `copy_file_range` and so shares extents on file systems
    that support it), updates `T`, and renames it;
  - the link state is first marked "output being updated" and the mark
    cleared after the last write: a crash in between leaves a mark that
    makes the next link write the whole output.
- **I-97 [M7]** A smaller output is truncated with `set_len` after the
  writes; a larger one is extended by them.
- **I-98 [M7]** The invariant of model §7.3 is checked on the file: the
  incremental output's bytes equal those of a full link given the same
  decision tables, compared byte for byte by the harness.
- **I-99 [M7]** The build id's leaves are in the link state (I-76). The
  map file is rewritten whole (it is about 40 bytes per section).
- **I-100 [M7]** The scan's events: a link state is written only by a
  successful link, which had no error groups; warning groups
  (`gnu-warning`, `--warn-unresolved-symbols`) are kept in the link
  state as each file's `Acc` vector (§4.5), so that a rescan of the
  changed files merges with the unchanged files' vectors by I-21.
- **I-101 [M7]** Liveness under `--gc-sections` is recomputed in full (a
  walk of the alloc relocations, I-9), and its difference from the
  stored `live` column gives the units that appear or disappear.
- **I-102 [M7]** `IMAGE_BLOCK` is fixed (I-75), so a link state's
  windows and leaves are those of the next link.

## 13. What the image needs from `spec/synth.md`

`spec/synth.md` is not written. The image needs this interface for every
synthetic chunk (`ChunkKind::Synthetic`, the GOT, PLT, `.symtab`,
`.strtab`, `.shstrtab`, `.eh_frame_hdr`, the notes, `.rela.*`, `.relr.dyn`,
`.dynamic`, `.dynsym`, `.gnu.hash`, `.gdb_index`):

```rust
pub trait SynthChunk {
    /// The chunk's size, known at layout (stage C) or, for a `sized` chunk, after the fixed point.
    fn size(&self, cx: &SizeCx<'_>) -> u64;
    /// The chunk's record boundaries: none (written whole), fixed records of `n` bytes,
    /// or variable records found through `record_start`.
    fn division(&self) -> Division;
    /// Write the bytes [range.start, range.end) of the chunk into `out` (out.len() == range.len()).
    fn write(&self, cx: &FillCx<'_>, range: std::ops::Range<u64>, out: &mut [u8]) -> Result<(), SynthError>;
    /// Ranges filled with zeros now and written by the sealing pass (the build id's descriptor).
    fn sealed(&self) -> Option<std::ops::Range<u64>>;
}
```

- **I-103** `write` is a function of the tables and the range only: for
  every split of `[0, size)` into ranges, the concatenation of the ranges'
  outputs is the same bytes (the clip rule of I-35, which the image's
  tests check by filling each chunk whole and by windows).
- **I-104** A chunk larger than `W` is **divisible**: `write` of a range
  costs `O(range + log chunk)`, not `O(chunk)`. `.symtab` (fixed
  records) is trivially so; `.strtab` needs, per input file, the prefix
  sum of the sizes of its names in the output (a column per file, which
  model §9 allows), and then walks that file's symbols from its start
  (`O(file's symbols)` per window). In `cxx-medium-g`'s lld output
  `.symtab` is 1.26 MB and `.strtab` 3.58 MB (E4).
- **I-105** `write` allocates nothing proportional to the chunk. A chunk
  whose content needs a global order (`.eh_frame_hdr`'s table sorted by
  address, `.gnu.hash`'s buckets) builds it once, in a prelude of phase
  100 that synth owns, with the parallel sort of rule 4, and states its
  memory in §11.3's `P`.
- **I-106** Errors of `write` are `relocation-out-of-range` /
  `relocation-misaligned` at `(100, 0, ChunkId)` (`spec/arch.md` §7), or
  `internal-error`.
- **I-107** The build-id note's `sealed` range is its descriptor; the
  image zeroes it for hashing and writes it last (I-73).
- **I-108** `address_dependent` (`Placement.chunks`' `Synth`) is the
  chunk's own statement that its bytes depend on addresses; the image
  uses it for M7 dirtiness (I-95): an address-dependent chunk is dirty
  whenever any address it reads moved, which the dependency index
  records.

## 14. Diagnostics of this spec

| kind | phase / key | emitted by | rule |
|---|---|---|---|
| `undefined-symbol` | `(70, first reference)` | scan driver, from events | I-20–I-22 |
| `comdat-mismatch` | `(70, first reference)` | scan driver | same |
| `discarded-section-reference` | `(70, first reference)` | scan driver | same |
| `gnu-warning` (per key) | `(70, first reference)` | scan driver | same |
| `relocation-incompatible` | `(70, FileId, section × 2^32 + record)` | `arch.scan` | I-17 |
| `relocation-out-of-range`, `relocation-misaligned` | `(100, FileId, section × 2^32 + record)`; `(100, 0, ChunkId)` | fill, synth | I-36, I-43 |
| `internal-error` | none | fill | I-27, I-44 |
| `cannot-open-output` | `(105, 0, 0)` | output file | I-57, I-59 |
| `cannot-write-output` (new, R-7) | `(105, 0, 0)` | output file | I-58, I-60, I-64 |
| `invalid-option-value` | `(10, argument, 0)` | command line | I-70, I-85 |
| `refused` | `(10, argument, 0)` | command line | I-5, I-51, I-53, I-69 |

## 15. Where the references differ

| subject | GNU ld 2.44 | lld 22 | mold 2.37.1 | mortar | rule |
|---|---|---|---|---|---|
| debug reference into a dropped COMDAT copy, `.debug_info`/`line`/`aranges`/`frame`/`addr`/`rnglists`/`loclists` | the kept copy's address + addend when the sizes are equal, else 0 | 0 | 0 | all ones | I-47 |
| the same in `.debug_ranges`/`.debug_loc` | redirect or 0 (GC case: 0 in `.debug_loc`, `spec/a64.md` §13.2) | 1 | 1 | 1 | I-47 |
| existing output after a failed link | removed | kept | kept | removed, unless it is an input | I-66 |
| read-only directory, writable output | written in place | error | error (names its temporary) | error naming the output | I-59 |
| output is an input | refused | accepted | accepted | accepted | I-62 |
| `--build-id=fast` | warning, no note | 8 bytes | 32 bytes | 16 bytes (XXH64 ×2 tree) | I-69 |
| `--build-id=sha256` | warning, no note | error | 32 bytes | 32 bytes (M2) | I-69 |
| `--build-id=uuid` | 16 random bytes | 16 random bytes | 16 random bytes | refused | I-69 |
| `--build-id=0xabc` (odd) | warning, no note | `ab 0c` | error | error | I-70 |
| `--build-id=0x` | warning, no note | empty descriptor | error | error | I-70 |
| build-id note size for a 2-byte id | 18 (unpadded) | 18 | 18 | 20 (padded to 4) | I-71 |
| what the hash covers | not the whole file, not a 64 KiB/1 MiB/4 MiB tree (E12) | same | same | a public tree over 1 MiB blocks | I-72 |
| `-Map` / `-M` format | GNU text | lld text | mold text | `mortar-map 1` | I-77 |
| `--gc-sections` in M1 | supported | supported | supported | refused until M2 | I-5 |
| identification string in `.comment` | none | `Linker: LLD …` | `mold …` | none from the image | I-55 |
| default thread count | one thread | all (not measured here) | all (not measured here) | `available_parallelism()` | I-85 |
| `--time-phases` | — | `--time-trace` (a different format) | `--perf` (a different format) | mortar's lines | I-88 |

## 16. Tests

### 16.1 Unit tests (`crates/mortar_image/tests/`, built with `mortar_testutil`)

- `plan_rows_sorted_and_disjoint`: plans from hand-made `Placement`s;
  an overlap is `internal-error` (I-27).
- `clip_equals_whole`: for every unit kind (placed section with
  relocations, fragment run, `EhFrame` pieces, a synthetic chunk, the
  structural units), filling the unit whole equals the concatenation of
  its fills clipped at every split into ranges of 1, 3, 7, 4096 and
  `W` bytes (I-35, I-103).
- `straddling_relocation_reported_once`: a relocation whose field
  crosses a window boundary, with an out-of-range value: one diagnostic,
  at the job holding `r_offset` (I-36).
- `unsorted_large_section_is_one_job`: a section larger than `W` with
  `.reloc`-ordered records gives the same bytes as the sorted
  equivalent, and the job covers every window it touches (I-31).
- `gap_patterns`: trap, no-op and zero gaps per I-41, positioned by
  address for odd starts.
- `tombstone_by_name_and_width`: I-45–I-48 for each name class of
  `spec/resolve.md` §6.3 and each allowed type (`ABS64`, `ABS32`,
  `ABS16`, `PREL32`, `TLS_DTPREL64`).
- `events_merge_like_sequential`: random event multisets (up to 10^5
  events, 1–50 groups, 1–200 units), merged by I-20/I-21 under 50
  scheduling seeds and thread counts 1, 2, 7 and 16, equal the sequential
  definition of §4.5 (first three, count, key).
- `build_id_vectors`: SHA-1, SHA-256, MD5 and XXH64 test vectors; the
  tree of I-72 for `n` = 0, 1, `B − 1`, `B`, `B + 1`, `3B + 5`; the
  descriptor zeroed (I-73).
- `build_id_hex_values`: every row of I-70's table.
- `time_phases_line_format`: the line of I-88 from fixed `/proc` texts.

### 16.2 Determinism

- Each parallel phase of this spec (liveness rounds, scan, event merge,
  plan, fill, map) under `MORTAR_SCHED_SEED` and thread counts 1, 2, 3,
  8 and 24 gives identical tables, output bytes, map and diagnostics
  (model §6).

### 16.3 Link tests (`tests/link/image/`, `tests/link/diag/`)

- Each row of §7.1's table, for mortar's column: new inode, hard link
  unaffected, symbolic link replaced, mode under umask 022 and 077,
  `-o DIR`, `-o /dev/null`, a running executable replaced, a failed link
  removing the old output, a command-line failure keeping it, an output
  that is an input (success, and a failure that keeps it), a read-only
  directory (run unprivileged: `setpriv --reuid=65534`; root ignores the
  mode), a stale `T` from another pid (I-59).
- `undefined-symbol` with 5 references in 3 files: one diagnostic, three
  `referenced by` lines in `RefId` order, `referenced 2 more times`, and
  the key of the first; 10^5 references to one symbol: one diagnostic;
  undefined and `comdat-mismatch` groups interleaved: report order by
  first reference.
- `--build-id` styles: the note's length and bytes; `none` gives no
  note; the id changes when one input byte changes; the id is the same
  for `--threads=1` and `--threads=24`.
- `--time-phases`: the lines' shape (not their numbers), on a failed and
  a successful link.

### 16.4 Debugger tests (`tests/debug/`)

The E9/E10 matrix as a test: two objects with one COMDAT inline
function, linked so that the second copy is dropped, compiled by GCC 14
and Clang, DWARF 4 and 5, linked at the default layout and at
`-z max-page-size=4096 -z noseparate-code`:

- the probe script of E10 (`break`, `bt`, `info line`, `list`, `info
  scope` in gdb; `breakpoint set`, `thread backtrace`, `source list`,
  `image lookup` in lldb) gives, with mortar's output, the transcript of
  GNU ld's output after addresses are masked;
- `llvm-dwarfdump --verify` reports no errors;
- the tombstone bytes are those of I-47 (read by the E9 script);
- a separate debug file found through `.build-id/xx/rest.debug` by gdb,
  lldb and `eu-unstrip`, for `sha1` and `fast` ids.

### 16.5 Benchmark gates (`spec/harness.md`)

- The image phase's time (from `--time-phases`) on the bench workloads,
  and on synthetic 200 MB and 2 GB outputs, reported with the load
  average, against `chunk`'s figure on the same file system; a ratio
  above 1.2 fails.
- Peak memory (`ru_maxrss`) at most I-92's bound with `T` from the
  landed row sizes plus 32 MiB, and its ratio to lld's and mold's peaks,
  on each workload (I-93).

### 16.6 Verifier

- `mortar_plumb` accepts every output of §16.3 and §16.4; with R-8, it
  recomputes the build id; every `-Map` file round-trips through
  `spec/plumb.md` §3.2's reader and resolves to the output's sections
  (I-78, I-79).

### 16.7 Incremental (M7)

- I-95–I-98: after each edit of the incremental test series, the output
  equals a full link given the same tables, byte for byte; a running
  executable takes the copy path; a crash injected between writes (a
  test hook) leads the next link to a full write.

## 17. Requested changes to other specs

Each request gives the section, the current text, the proposed text and
the reason. Text is quoted from `main` at the merge of WP-020d.

### R-1 `spec/arch.md` §3.4 (the scan reports events)

1. The `DemandSink` comment. Current:
   `/* raise(SymKey, DemandFlags); raise_global(GlobalFlags); count(DynRole, n) for this section; error(Key, Diagnostic) */`.
   Proposed:
   `/* raise(SymKey, DemandFlags); raise_global(GlobalFlags); count(DynRole, n) for this section; error(Key, Diagnostic); reference(EventClass, group: u64) for the record being scanned */`.
2. The bullet **What is reported.** Current: "For every live relocation
   against an `Undefined` target that still needs its symbol, scan
   reports `undefined-symbol` once. The message contains GNU ld's phrase
   `undefined reference to '<name>'`, and gives the place
   `file:(section+0xoff)`." Proposed: "For every live relocation against
   an `Undefined` target that still needs its symbol, scan reports the
   event `reference(Undefined, key)` and no diagnostic. The scan driver
   (`spec/image.md` §4.4, §4.5) turns the events into one
   `undefined-symbol` per key: GNU ld's phrase `undefined reference to
   `<name>'` in the head, the first three references in `(FileId,
   section index, record index)` order as `>>> referenced by
   file:(section+0xoff)`, then `>>> referenced N more times`.
   `comdat-mismatch` (event `ComdatMismatch`, per key),
   `discarded-section-reference` (event `DiscardedSection`, per symbol
   and referring section) and the per-key `gnu-warning` (event
   `GnuWarning`) are reported the same way."
3. The bullet **The key**. Current: "**The key** is `(70, FileId, section
   index × 2^32 + record index)`." Proposed: "**The key** of each grouped
   diagnostic is its least reference's `(70, FileId, section index ×
   2^32 + record index)`." (The rest of the bullet is unchanged.)
4. The last bullet. Current: "Scan sees only live sections. A relocation
   in a live section against a symbol in a dead section is not an error
   at scan; `fill` writes a tombstone (§3.11) or `spec/image.md` reports
   it." Proposed: "Scan sees only live sections. A relocation in a live
   section against a symbol in a section that is not live is classified
   by `spec/resolve.md` §6.3's table of referring sections: from a debug
   name it is accepted and `fill` writes the tombstone (§3.11); from
   `.gcc_except_table` the field is 0; from any other section scan
   reports the event `DiscardedSection`."

Reason: the supervisor's ruling of 2026-11-14; a per-reference
diagnostic cannot be grouped by the collector, which orders but does
not merge (`spec/diag.md` §6.2).

### R-2 `spec/arch.md` §3.6 and §3.1 (clipped output)

1. `SectionOut`'s comment. Current: `/* the unit's output bytes,
   pre-filled with the input bytes; P of offset 0 */`. Proposed: `/* the
   unit's output bytes in the clip [a, b) of section offsets,
   pre-filled with the input bytes; P of offset 0; writes outside the
   clip are discarded */`.
2. Add to the **Contract** bullet: "`out` may hold only a clip of the
   section (`spec/image.md` I-35): `apply` writes through `SectionOut`'s
   methods, and the bytes it writes inside the clip are those of an
   unclipped call."
3. Add to §3.1's facts: "`max_reach: (u8, u8)`: the largest `before` and
   `after` of any write window (§3.6) of the architecture, over every
   type and sequence: AArch64 `(0, 0)`; x86-64 `(4, 8)` (the `TLSGD`
   window `[P − 4, P + 12)` around a 4-byte field); i386 as Appendix B
   gives."

Reason: fill by windows (I-33) clips sections at window boundaries;
`apply` already never reads `out`, so the change costs nothing.

### R-3 `spec/arch.md` §3.5 (sequence extent)

Add: "`sequence_extent(records, i) -> u32`: the number of records from
`i` that a sequence recognized at `i` may consume (1 for a record that
starts no sequence). It reads only the records. The scan driver does not
split a section's records between blocks inside an extent
(`spec/image.md` §4.1)." Reason: blocks of `SCAN_BLOCK` records must not
cut a sequence.

### R-4 `spec/model.md`

1. §3.6, the `frag_live` bullet. Current: "`frag_live[FragId]: bool`:
   referenced from a live section;". Proposed: "`frag_live[FragId]:
   bool`: under `--gc-sections`, referenced from a live section; without
   it, every fragment of a live section (`spec/image.md` I-2);".
2. §5, "One sealing pass": append "`spec/image.md` §8.3 gives the
   function: a hash tree over the file's 1 MiB blocks with the
   descriptor zeroed, whose leaves an incremental link updates."
3. §8, add a row after `image`: "| output file | 105 | 0 | 0 |".
4. §12: remove "the sealing function for the build id, resident image or
   positioned writes" from the row of divisible synthetic chunks (they
   are decided: `spec/image.md` §8, §10); the row keeps "Divisible
   synthetic chunks" with owner `spec/synth.md`, its interface given by
   `spec/image.md` §13.

Reason: I-2, I-68, I-72; model §12's open item is closed.

### R-5 `spec/arch.md` §3.11 and `spec/a64.md` §13.2 (tombstone)

1. `spec/arch.md` §3.11. Current: "A relocation in a non-alloc section
   whose target is in a section that is not live writes
   `tombstone(name)` truncated to the field instead of `X`: 1 for
   `.debug_ranges` and `.debug_loc`, 0 for every other section, on all
   three targets (a64.md §13.2 for the experiment)." Proposed: "A
   relocation in a non-alloc section with a debug name (`spec/resolve.md`
   §6.3) whose target is in a section that is not live writes
   `tombstone(name)` truncated to the field instead of `X`: 1 for
   `.debug_ranges` and `.debug_loc`, all ones for every other debug
   name, on all three targets (`spec/image.md` I-47 for the experiment;
   `.gcc_except_table` and other names: `spec/resolve.md` §6.3)."
2. `spec/a64.md` §13.2, **Discarded targets**. Current: "…writes a
   tombstone instead of `X`, independent of `A`: 1 in `.debug_ranges`
   and `.debug_loc` (where 0 would end the list), 0 elsewhere." Proposed:
   "…writes a tombstone instead of `X`, independent of `A`: 1 in
   `.debug_ranges` and `.debug_loc` (where 0 would end the list and all
   ones would select a base address), all ones in the field's width
   elsewhere (`spec/image.md` I-47: a 0 is an address, and lldb
   attributes low code to the dead copy)." The experiment paragraph
   stays; append: "For dropped COMDAT copies (`spec/image.md` E9) GNU ld
   instead redirects to the kept copy when the sizes are equal."

Reason: E9/E10 (lldb loses line information on lld's and mold's
outputs at low text addresses; all ones fixes it; gdb unaffected).

### R-6 `spec/cli.md` §4.2 (rows whose status the image changes)

Append to the list of rows the M1 and M2 packages change: "WP-026 stage 2
(`mortar_image`): `supported` in M1: `--no-gc-sections`, `--build-id`
(bare, `none`, `sha1`, `fast`, `0xHEX`; `md5` and `sha256` refused
until M2; `uuid` refused, `option value`), `--no-build-id`, `-Map`,
`-M`/`--print-map`, `--threads`, `--thread-count`, `--no-threads`,
`--time-phases` (its help loses `(reserved)`),
`--compress-debug-sections` with the value `none` (others refused,
`option value`, until M4); `noop(mortar never maps the output)`:
`--mmap-output-file` and `--no-mmap-output-file`. `--gc-sections`, `--print-gc-sections`,
`-s`, `-S` and `--cref` stay `refused` (M2)." And in §2.3, the
build-ID value kind: "`0x` followed by an even, non-zero number of
hexadecimal digits; anything else is `invalid-option-value`
(`spec/image.md` I-70)." Reason: I-5, I-51, I-69, I-70, I-77, I-85,
I-88.

### R-7 `spec/diag.md` §3.1, §3.2, §7

1. §3.1, files area, after `cannot-open-output`, a row: "|
   `cannot-write-output` | `CannotWriteOutput { path, reason }` |
   `cannot write output file {path}: {reason}` | own |".
2. §3.2, a note: "`cannot-open-output` and `cannot-write-output` name
   the output path as the user gave it, never the temporary file
   (`spec/image.md` §7); key `(105, 0, 0)`."
3. §7, the bullet "The phase that creates the output file … creates it
   only when `has_errors()` is false, and removes it if an error is
   reported afterwards." Append: "A failure at any phase after the
   command line also removes an existing regular output file that is not
   an input (`spec/image.md` I-66)."

Reason: I-64 needs a kind for write failures; I-66.

### R-8 `spec/plumb.md` (build-id check)

Add a check: "When the map is mortar's (`mortar-map 1`) and the output
has an `NT_GNU_BUILD_ID` note of 16, 20 or 32 bytes, the verifier
recomputes `spec/image.md` I-72's function (16 bytes: MD5 or `fast`,
either matching; 20: SHA-1; 32: SHA-256) and reports a mismatch as a
finding at the note's address." Reason: the id is the one sealed value
that no other check covers.

### R-9 `spec/plumb.md` §3.2 (non-alloc records), M2

Add two records, written after `piece` records:

```text
nplace <file> <section> <output-section> <offset> <size>
merged <file> <section> <output-section>
```

`nplace`: a non-alloc input section placed whole at `<offset>` of the
non-alloc output section named `<output-section>` (a string token).
`merged`: an `SHF_MERGE` non-alloc input section whose pieces are in
`<output-section>` and are not listed (references to them are checked
by content, §5.6). Map rules M2/M3/M5 apply to `nplace` within one output
section. Reason: I-78; non-alloc pieces (`.debug_str`) number in the
millions and have no address.

### R-10 `spec/harness.md` (Measurement)

Current: the bench writes its scratch output to
`.build/bench-work/scratch`. Proposed: "The bench writes its scratch
outputs under a local file system (`$TMPDIR`, default `/tmp`), in a
directory unique to the run, removed at its end, and records the file
system type of that directory with every result." Reason: §10.6 (18×
slower writes and 1.4–2.9× slower reference links on the workspace
mount).

### R-11 `spec/resolve.md` §5.3, §6.3, §4.7 and `spec/layout.md` §13.2

1. `spec/resolve.md` §6.3's table, mortar's cell of the debug-names row.
   Current: "accepted: `spec/image.md` writes the tombstone". Proposed:
   "accepted: `spec/image.md` I-47 writes the tombstone (1 in
   `.debug_ranges`/`.debug_loc`, all ones elsewhere)".
2. `spec/resolve.md` §6.3, the paragraph after `comdat-mismatch`.
   Current: "the scan (phase 70) finds the relocations and emits
   `discarded-section-reference` one diagnostic per (symbol, referring
   section) and `comdat-mismatch` one per symbol, each with the first 3
   places, as §5.3." Proposed: "the scan (phase 70) reports each such
   relocation as an event, and the scan driver emits
   `discarded-section-reference` one diagnostic per (symbol, referring
   section) and `comdat-mismatch` one per symbol, each with the first 3
   places, as §5.3 (`spec/image.md` §4.5)."
3. `spec/resolve.md` §4.7 ("The roots of liveness (`spec/live.md`,
   M2)") and `spec/layout.md` §13.2 ("Liveness is `spec/live.md`'s"):
   `spec/live.md` becomes `spec/image.md` §3, until folding (M5) gives
   liveness a spec of its own.

### R-12 `spec/synth.md` (to be written)

The interface of §13 (I-103–I-108): `size`, `division`, `write(cx,
range, out)`, `sealed`; divisibility for chunks larger than 1 MiB; no
allocation proportional to the chunk in `write`; the build-id note's
layout (I-71) and its sealed descriptor.

### R-13 `spec/input.md` §11.3 (confirmation)

Add to `copy_to`'s bullet: "`copy_to` takes `&self` and may be called
by every worker at once (positioned reads on a shared descriptor)."
Reason: fill's workers call it concurrently ([perf], I-82); the
current text implies it but does not say it.

## 18. Open questions for the supervisor (most consequential first)

1. **Q1 — What bare `--build-id` computes.** Recommended for M1: the
   SHA-1 tree (20 bytes, the length every reference gives). The
   alternative is a 20-byte fast tree (three XXH64 lanes): about 16×
   cheaper in CPU (E6: 13.1–13.5 GB/s per lane against 267–279 MB/s), no tool can tell the difference (§8.1), and every link pays the
   hash (every driver passes `--build-id`). Decide after the M1 bench
   shows the SHA-1 tree's share of link time; until then the
   conservative choice. Depends on it: every link's time; the frozen
   definition of I-72.
2. **Q2 — Freezing `W` = 1 MiB in the build id's definition (I-75).** It
   ties the fill's window, the hash's leaf and M7's dirty unit together.
   A later change of `W` changes every id and needs a new tag; an id is
   not stored anywhere that would break, so this is cheap to revisit
   before M7 and expensive after.
3. **Q3 — The all-ones tombstone (I-47)** instead of lld's and mold's 0.
   Every C++ debug link with a COMDAT duplicate has such references.
   Evidence: lldb loses line information with 0 at low addresses; gdb
   and `llvm-dwarfdump --verify` accept both. Risk: a consumer that
   treats all ones as a real address (none found among gdb 16, lldb 19,
   `addr2line`, `llvm-dwarfdump-22`, `eu-unstrip`).
4. **Q4 — Removing a stale output after a failed link (I-66)**: GNU ld's
   behaviour; lld and mold keep it. The safety argument is above;
   the cost is that a developer loses the last working binary on a
   failed relink.
5. **Q5 — `-Map` writes `mortar-map 1`, not GNU's text (I-77).** No
   installed tool parses either; a user script that greps GNU's map
   would break.
6. **Q6 — `--build-id=uuid` refused (I-69).** It is nondeterministic by
   definition; GNU ld, lld and mold accept it. The alternative is to
   accept it and exempt it from the determinism tests.
7. **Q7 — `fast` = 16 bytes and `sha256` accepted (I-69).** lld's `fast`
   is 8 bytes (lldb finds separate debug files from 8 bytes, E13); 16
   is safer against collisions in debuginfod-sized collections.
8. **Q8 — Accepting `-o` equal to an input (I-62)**, which GNU ld
   refuses; with I-66, a failed link must then never remove the output
   (it does not).
9. **Q9 — The jobserver.** mortar uses `available_parallelism()` threads
   even under `make -jN`; joining the jobserver (`--jobserver-auth` in
   `MAKEFLAGS`) would avoid oversubscription in parallel builds but
   adds a protocol to the linker.
10. **Q10 — `copy_file_range` (§10.3).** Rejected for now because the
    build id needs the bytes; worth revisiting if the host share (or
    NFS) is where developers link, where it is 3.7–5.2× faster.
