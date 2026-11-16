# spec: liveness, the scan driver, the image and the output file (`mortar_image`)

Status: **accepted for M1** (WP-026 stage 1; reviewed independently, the review's findings applied). Method,
experiments and what was not checked are in
`spec/landed/image/WP-026-spec.md` (cited as **E1**–**E15**, and **E16**–
**E25** for the re-measurements made for the review). Every measurement
here was taken on the WP-026 host: AArch64 (24 CPUs, 31 GB, Linux
6.12.76, transparent huge pages `always`), load 23–41 during the runs,
Rust 1.95.0, GCC 14.2, Clang 19.1.7, GNU ld 2.44, lld 19.1.7 and 22.1.8,
mold 2.37.1, gdb 16.3 (and `gdb-multiarch` with `qemu-i386` for i386),
lldb 19.1.7. Two file systems were measured: `/tmp` (overlay, local) and
the workspace mount (a host share). Numbers are medians with the range in
brackets or parentheses and the load average beside them; ratios matter
more than absolutes, because the host was shared and loaded.

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
with `--gc-sections` (and `.gdb_index`, I-54), **[M4]** compressed debug, `-r`,
**[M7]** incremental linking, **[perf]** the performance package that
follows M2 (`docs/ROADMAP.md`). A rule marked for a later milestone is
specified so that nothing in the M1 design must be redone for it; its code
is not M1's. Rules marked **[C]** are choices, where the reference linkers
are not followed or disagree (§15).

## 1. Principles that bind this spec

- **The image is a function** (`spec/model.md` §5). Fill reads the tables
  of model §3 and §4 and the input bytes, and nothing else. A cache this
  spec builds (§5.2) is a function of those tables, lives only for the
  phase, and is never an input to another phase. The synthetic chunks'
  plans (`spec/synth.md`) are such functions too: each is made from the
  inputs, the command line and the final `Layout` (its `.eh_frame`
  records, slot lists, chunks, merged map and patches), never from a
  table the front computed on the way to it (WP-086). A link that
  computes its layout makes them from the tables it chose; a layout from
  elsewhere (a replay, IN-61; an update) gets the plans of its own
  tables, or fails by name where a plan holds a table to a rule (S-49,
  S-88).
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
  this spec in this mortar version (§8.3, I-75). Windows are the unit of
  buffering, of writing and of build-id hashing.
- **Job**: a run of consecutive windows filled into one buffer by one
  worker (§5.3): one window, or the windows a *whole* synthetic chunk
  larger than `W` covers (§13).
- **Reference**: a relocation record that survives sequence recognition
  and still needs its symbol (`spec/arch.md` §3.4), identified by
  **`RefId = (FileId, section index, record index)`**, ordered
  lexicographically. The section index is that of the **relocated**
  section (the relocation section's `sh_info`), not of the relocation
  section; the record index counts records of that section's one
  relocation section (`spec/resolve.md` refuses a second one). Scan units
  are formed in `(FileId, relocated section index)` order and visit
  records in record order, so **unit order is `RefId` order**. This order
  is the order of the scan's diagnostic keys (model §8, phase 70).
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
  costs a few bytes. GNU ld, lld and mold also keep unreferenced pieces
  without `--gc-sections` (verified, WP-031 Q11).
- **I-3 [M1]** An FDE piece is live exactly when the section its code
  range lies in is live (model §3.6; under folding, M5, also not folded
  away). A dead FDE is not written (`spec/layout.md` lays out live pieces
  only); a CIE is live when a live FDE uses it.
- **I-4 [M1]** Liveness emits no diagnostic in M1. Its key, when it does
  (M2, §3.3), is `(50, FileId, section index)`.
- **I-5 [M1]** `--no-gc-sections` is the default and is `supported` in M1
  (request R-6). `--gc-sections` was `refused` in M1 (WP-027d: by the
  row, at the command line, `spec/cli.md` §4.4); it is `supported` since
  WP-031 (§3.2), with `--print-gc-sections`, `--no-print-gc-sections` and
  `--gc-keep-exported`.

### 3.2 `--gc-sections` (M2)

The design is fixed now so that the M1 tables need no change.

- **I-6 [M2] Roots.** A section is a root when any of these holds:
  - it defines `def` of a key rooted by `spec/resolve.md` §4.7 (the entry,
    `-u`, `--require-defined`, every key exported to `.dynsym`); all three
    references root these (verified, WP-031 Q8; `-u` and
    `--require-defined` are refused by mortar's command line today;
    `--export-dynamic` on a static executable roots nothing in GNU ld and
    lld 22, everything in lld 19 and mold: nothing is exported, so
    nothing is rooted);
  - with `--gc-keep-exported` (GNU ld only), it defines a key that is
    defined, global or weak, of default or protected visibility (Q8);
  - it has `SHF_GNU_RETAIN` (`spec/layout.md` L-97; GCC 14 and Clang 19/22
    emit it for `__attribute__((retain))`, all three keep it, Q7), or a
    script `KEEP` covers it;
  - `Assignment.keep` holds it (`mortar_layout::assign::keep_by_rule`), its
    output section is `.init_array`, `.fini_array`, `.preinit_array`,
    `.init`, `.fini`, `.ctors`, `.dtors`, or its input name is one of
    those or one of those followed by `.` (`.init_array.00100`,
    `.ctors.65435`): start-up tables are run by name, not by reference
    (`spec/layout.md` §5); all three keep them and what they point to,
    in a COMDAT group too (Q5);
  - its type is `SHT_NOTE` or it is not `SHF_ALLOC`, and it is not in a
    section group with an allocated member (notes and non-alloc
    sections are read by tools and loaders, not referenced). All three
    keep notes, alloc or not, of every object (Q1). lld and mold keep
    every non-alloc section; GNU ld drops the non-alloc sections of an
    object none of whose allocated sections is marked, and every
    non-alloc non-debug section with relocations (Q2, Q13): mortar keeps
    them, the rule that cannot drop what a tool reads. A note or a
    non-alloc section in a group with an allocated member lives and dies
    with its group (I-7; GNU ld and lld drop such a note with its dead
    group, mold keeps it, Q1);
  - it is `.eh_frame` (its pieces are live by I-3), or it is kept but has
    no place of its own (`OutOf::Consumed`);
  - it is a section the mark could not describe (not of an object, or a
    header `mortar_elf` cannot read): the mark never drops what it has not
    looked at;
  - its name is a C identifier and `__start_NAME` or `__stop_NAME` is
    referenced from a live section (`spec/layout.md` L-96: GNU ld's
    `-z nostart-stop-gc`, its default; lld 19 and 22 default to
    `start-stop-gc`, under which an undefined `__stop_` is an error; mold
    roots every C-identifier section; all three collect an unreferenced
    one, Q6).
  A `SHF_LINK_ORDER` section is never a root (I-109).
- **I-109 [M2] `SHF_LINK_ORDER` follows its link target.** A section
  with `SHF_LINK_ORDER`, alloc or not, is live exactly when the section
  its `sh_link` names is live. This rule takes precedence over I-6's
  non-alloc root rule. Clang's `-ffunction-sections -fstack-size-section`
  emits a non-alloc `SHF_LINK_ORDER` `.stack_sizes` per function. If a
  collected function's `.stack_sizes` were kept, its relocation would be
  `discarded-section-reference`. Under `--gc-sections`, lld 22 and GNU ld
  remove it (`removing unused section '.stack_sizes'`); mold 2.37.1 keeps
  it (18 bytes in the output against 9 for lld and GNU ld, E25). M1 is
  unaffected: without `--gc-sections` only COMDAT drops sections, and a
  COMDAT `.stack_sizes` or `.llvm_bb_addr_map` is a member of its group.
- **I-7 [M2] Edges.** A live section `s` that is not `.eh_frame` and has
  no debug name (`mortar_arch::is_debug_name`) makes live:
  - the section of `def` of every key that a relocation record of `s`
    names, through `sym_map` and `redirect` (a `Section` definition; a
    `Common` definition, nothing: commons are never collected; an
    `Internal` definition, nothing, but a conditional `__start_NAME` or
    `__stop_NAME` the sections named `NAME`; a `Shlib` one, nothing).
    Every record is an edge, `R_*_NONE` included (`.reloc ., R_*_NONE,
    sym` asks for a dependency);
  - the section of every local or section symbol a record of `s` names;
    for a reference into a split section, the piece and its fragment
    (`frag_live`, by offset through `Split`, as model §3.5 resolves a
    reference: the symbol's value, plus the addend for a section symbol;
    under `SHT_REL`, whose addend is in the relocated bytes the mark does
    not read, every piece). GNU ld keeps a live merge section whole; lld
    and mold keep the referenced pieces (Q11);
  - the sections in `s`'s `SHF_LINK_ORDER` dependents, alloc or not (a
    section whose `sh_link` is `s` is live when `s` is, I-109);
  - for a code section, its FDEs (I-3) and, through the relocations of
    each FDE and of its CIE, the LSDA's (`.gcc_except_table`) and the
    personality routine's sections: the FDE's edges are `s`'s edges. A
    collected function's LSDA is collected (GNU ld and mold; lld keeps
    it), and a personality reached only through the CIE of collected
    FDEs is collected (GNU ld; lld and mold keep it) (Q4, Q15);
  - every other member of `s`'s section group: a group is live as a unit.
    GNU ld and lld keep a group whole when any member is live; mold
    collects members one by one (Q3; the earlier "each by its own edges"
    was lld's rule misread, refuted).
  Relocations in debug sections are not edges (debug information never
  keeps code alive, all three, Q14); in `.eh_frame` they are edges only
  through the FDE rule. Relocations in other non-alloc sections are: GNU
  ld marks through notes (`.note.stapsdt` keeps libstdc++'s
  `.stapsdt.base`, found by WP-031's tests), and a kept section's
  reference to a collected one would otherwise be refused (I-12).
- **I-8 [M2] The algorithm.** A least fixed point. `live` is a column of
  atomic booleans marked with `swap(true)` (model §6 rule 2: idempotent,
  commutative); the worker whose swap returned `false` owns the section
  and walks its edges, every live section exactly once. The work is
  handed out in rounds: round 0 is the roots; a round's units are the
  sections the round before left, partitioned by section and, for a
  section with more than `SCAN_BLOCK` records, by `SCAN_BLOCK` records
  (rule 6). A worker continues, depth first, on the sections its own
  swaps marked until it has walked `CONTINUE` records for the unit (no
  limit when the pool runs inline). It leaves the rest, and any section
  of more than `SCAN_BLOCK` records, to the next round, and the leftovers
  are concatenated in unit order (rule 3). The rounds only balance the
  load: the resulting *set* is the least set closed under I-7 containing
  the roots, whatever the schedule. Which round walks a section, and the
  order of a round's list, may differ between schedules and are never
  observed (no id, diagnostic or byte depends on them). A failure to read
  a section's relocations is reported after the closure: the least-keyed
  failure of the first breadth-first round (round `k + 1` = the sections
  first marked by round `k`'s edges) that has one, found by marking
  again breadth first (WP-032h).
- **I-9 [M2] Cost.** One walk of the relocation records of live alloc
  sections: 221,459 for `cxx-medium-g` (E5), about 0.8 ms at the 3.4–3.7
  ns per record of a scan-like walk (E6). Measured (WP-032h, static
  protoc, 1/8 threads): the phase is 15.7/9.8 ms (WP-031: 19/14), the
  rounds ~5.5/~5 ms in 3 rounds at 8 threads (20 before); lld 22's
  `markLive` is 5.3/4.7 ms and mold's `gc` 7/2 ms. The link is no slower
  in total (WP-031: layout has less to place: 79→56, 64→43, 66→41 ms).
  It is a separate walk from the
  scan's (phase 70), because the scan must know liveness first: undefined
  references are reported only from live sections (`spec/resolve.md`
  §5.1), and whether a reference survives depends on liveness of its
  target (§6.1).
- **I-10 [M2]** Liveness under `--gc-sections` does not change
  `Resolution`: extraction is decided before liveness
  (`spec/resolve.md` §4.7); a member every section of which is collected
  is still extracted, and its definitions still collide (all three, Q10).
  `discard <file> <section> gc` records go to the map (§9).
  `--print-gc-sections` writes GNU ld's lines to standard error, before
  the report: `<prog>: removing unused section '<section>' in file
  '<file>'`, `<file>` as `libx.a(y.o)` for a member, `<section>` as
  `name[signature]` for a member of a COMDAT group; one line per
  collected section that is kept by resolution, assigned an output
  section, not `.eh_frame`, and not of size 0; in `(FileId, section
  index)` order (`mortar_live::removed`). GNU ld prints the same lines
  in the same order (its program path where mortar prints its basename),
  and also lists `.group` sections, which mortar does not output; lld
  and mold print `removing unused section <file>:(<section>)` to
  standard output, size-0 sections included, mold in no stable order
  (Q9).
- **I-11 [M2]** `-r` with `--gc-sections` is refused (M4 decides).
  `--gc-sections` with `-shared` (M4) roots every exported key (I-6).

### 3.3 Liveness diagnostics (M2)

- **I-12 [M2]** A reference from a live non-alloc section into a
  collected section is not an error: it writes the tombstone of I-47 when
  the referring section has a debug name, and is
  `discarded-section-reference` otherwise (`spec/resolve.md` §6.3's
  table, applied to collected sections as to dropped COMDAT copies). The
  scan reports it (§4.4), or for a section deferred to fill fill's check
  does (I-14); liveness reports nothing. The mark reads every live
  section's relocation records itself (`mortar_live::read`), deferred or
  not: it never depends on the scan's walk. Under I-7 a live
  non-debug section's references are edges, so only a debug section can
  refer to a collected one. None of the three references diagnoses the
  non-debug case: GNU ld removes the referring section, lld and mold
  write `0 + A` (Q13). A key whose definition is in a collected section
  is `dead` to the scan (`mortar_scan::view`), as a local of one is.

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
  each only its own relocation records, through `Inputs`. Relocation
  records of an `SHF_ALLOC` section are resident in both stores; those
  of a non-alloc section are resident in the resident store only. In the
  staged store they are read in blocks of at most `SCAN_BLOCK` records
  with `copy_to`, into a buffer the worker owns (I-82, request R-13). The
  scan never reads section bodies except where `arch.scan` checks an
  instruction (`spec/arch.md` §3.4, through `RelocAt::site`). For a
  non-resident body the site is read with `copy_to` into a 16-byte
  buffer the worker owns.
- **I-14 [M1] Non-alloc relocations are checked by fill (supervisor
  ruling, WP-032c).** A relocation whose place is in a non-alloc section
  raises no demand and produces no dynamic relocation (`spec/a64.md`
  §13.2), except the types the architecture lists in
  `Arch::nonalloc_demand_types` (x86-64 `GOTOFF64`, X-53; AArch64
  `GOTREL64`/`GOTREL32`, M2), which need the GOT base (`spec/arch.md`
  §4.3). The scan **defers** a live relocated section `S` to fill
  (`RecIndex::deferred`) when all of these hold: `S` is not `SHF_ALLOC`,
  is `Regular` or `Debug`, and has a debug name (a reference from it to a
  section that is not live is the tombstone, §6.1:
  `mortar_arch::dead_reference` is `Tombstone`); its relocation section
  is `SHT_RELA` with the class's record size, whole records, `sh_info`
  `S` and `sh_link` the file's symbol table; and no global of `S`'s file,
  nor what `redirect` makes of one, is a key a reference to which makes
  an event (§4.4: target kind `Undefined`, a `comdat-mismatch` key, a
  `gnu-warning` key). A deferred section's records can make no event, so
  what is left of the scan's work on them is the checks of §4.3. The
  scan reads only a deferred section's records of the listed types, and
  calls `arch.scan` on those for their demands; it reads no other record
  of it.

  Fill checks each record of a deferred section as it applies it (§5.4),
  in the clip whose `[a, b)` holds its `r_offset` (the last clip: or any
  `r_offset ≥ b`): its symbol has a key; `classify` accepts the type;
  `arch.scan`, called as the scan calls it, reports nothing, which is
  the architecture's non-alloc check (the type is allowed at a non-alloc
  place, the field lies in the section, the target is defined, a weak or
  ignored undefined, or in a section that is not live, whose field is
  the tombstone of I-47) and raises no demand outside the listed types;
  and no two of the clip's windows overlap (`spec/arch.md` §3.4). A
  failed check fails the link, and its diagnostics are the scan's walk
  of the section (`Walker::deferred_report`): the same text as the
  scan's, keyed `(100, FileId, section index × 2^32 + record index)`,
  so after every scan diagnostic and among themselves by file, section
  and record. Fill reports nothing of its own for such a record and does
  not apply it. The walk also reports, so that no fault goes unseen,
  every deferred section when the link fails after the scan and before
  fill has run every job (a phase from 80 to 99, the image phase before
  fill, a job stopped by a failed read), and a deferred section fill does
  not reach (no input row with relocations). An `Undefined` from
  non-alloc sections only (I-110) is still the scan's: its file is not
  deferred. 87% of `cxx-medium-g`'s 2.2 M relocations are in `.debug_*`
  sections (E5).
- **I-15 [M1]** The scan's outputs and only these: the `Demands` flags
  (atomic or, rule 2), the per-section dynamic-relocation counts
  (written by the unit that scans the section; a section split into
  blocks sums the blocks' counts, an integer sum, rule 2), the
  diagnostics of `spec/arch.md` §7 at their keys, and the events of
  §4.4, and the **record index** of I-29. No other table is written.
  [M2] The per-section dynamic-relocation counts are also what places
  the `RELATIVE` records of `.rela.dyn`: each unit's slice starts at the
  exclusive prefix sum of the counts in address order of the placed
  sections and GOT slots, and the unit fills it with its records sorted
  by `r_offset` (`spec/dyn.md` D-40, synth S-4), so the counts must be
  exact per section, not only in total.

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
  (E14); the benchmark gate of §16.5 is. The records of the sections
  deferred to fill (I-14) are not part of this cost: fill pays one
  `arch.scan` and one window per such record, in the pass that applies
  it.

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
// group: SymKey as u64; for DiscardedSection, (referring section index << 32) | symbol index
pub struct GroupKey { pub class: EventClass, pub file: FileId, pub group: u64 } // Ord: lexicographic
// file: at.file for DiscardedSection (its group is file-relative); FileId 0 for the SymKey classes,
// whose groups span files. EventClass orders as declared.
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
  never scanned; a section deferred to fill (I-14) makes none, by the
  rule that defers it; an `Undefined` target whose verdict is "nothing"
  (`--unresolved-symbols=ignore-all`, `--ignore-unresolved-symbol`, weak)
  reports none. Whether the event is an error or a warning is the
  verdict's, fixed per key, and is recorded with the group, not per
  event.
- **I-110 [M1] [C] Undefined from non-alloc sections only.** A key
  referenced only from non-alloc sections (`.debug_info` naming an
  undefined global, for example) reports `Undefined` like any other
  reference: the link fails with `undefined-symbol`, as with GNU ld and
  mold; lld accepts the link (the review's measurement, §15). Debug
  information that names a symbol nothing defines describes code that is
  not in the program, which is the error the user needs to see.

### 4.5 The deterministic merge

The sequential meaning: take every event of the link, in `RefId` order;
for each `GroupKey` `(class, file, group)`, the diagnostic lists the
first three events' places **in `RefId` order** and counts the rest; its
key is the first event's. (`file` keeps two files' discarded-section
events with equal section and symbol indexes apart.) The parallel
formulation:

- **I-20 [M1] Per unit.** A scan unit visits its records in record order,
  so its events arrive in increasing `RefId`. It keeps an accumulator
  per `GroupKey` it has seen, in a hash map used for lookup only:

  ```rust
  struct Acc { first: [RefId; 3], len: u8, count: u64 }  // count: every event, first included
  ```

  The first three events of a group in the unit fill `first`; every
  event adds 1 to `count`. At the end of the unit the map is turned into
  a vector sorted by `GroupKey` (rule 5: no hash order escapes).
  Memory: one `Acc` (40 bytes) per distinct group per unit, only for
  links that report these kinds; a unit referencing one undefined
  symbol 10^6 times holds one `Acc`.
- **I-21 [M1] Across units.** The vectors are concatenated in unit order
  (rule 3; units are in `RefId` order because blocks of a section are in
  record order and sections in `(FileId, index)` order), then stably
  sorted by `GroupKey` (rule 4, `par::sort_by_key`, which is stable).
  **Ties** (equal `GroupKey`, from different units) keep unit order, which
  is `RefId` order (§2). The run of one group is then in `RefId` order, and is reduced
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
    per entry of `first`, in `RefId` order (`ReferencedBy`,
    `spec/diag.md` §3.1), and, when `count > 3`, the note `>>> referenced
    {count − 3} more times` (the references differ here, §15);
  - the extra context of each kind: `spec/resolve.md` §5.3 (hidden
    visibility, demangling), §6.3 (`comdat-mismatch`'s kept and dropped
    copies and the ODR note).
  `{r_offset}` is read from the record when the diagnostic is built (a
  resident record, or 24 bytes through `copy_to`): `RefId` is enough.
- **I-23 [M1]** Two groups share a key only when their first references
  are the same record. A record names one symbol, so that happens only
  when two classes fire on it, `GnuWarning` with `Undefined` or
  `ComdatMismatch`; the collector orders the two by text
  (`spec/diag.md` §6.2). The same shape (one diagnostic per group, the
  first three references, the count) applies to all four classes.
- **I-24 [M1] Error limit.** Groups count as one diagnostic each against
  `--error-limit` (`spec/diag.md` §6.4); every group is still built,
  because which ones are printed depends on their keys.

The undefined references of shared objects (M2) are resolution's (phase
30, `spec/resolve.md` §5.2) and do not go through the scan.

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
- **I-29 [M1] [C] The record index.** Fill finds a clip's relocations by
  binary search on `r_offset` (I-35), so it needs every relocation
  section in `r_offset` order. The scan already reads every record in
  record order (I-13), so it checks the order there, with one comparison
  per record and no extra pass. A section split into blocks combines its
  blocks' results: it is sorted when every block is and each block's last
  `r_offset` is at most the next block's first. Per relocation section,
  the scan's unit writes the phase cache `RecIndex` (model §4.5; a column
  per relocated section, rule 1):
  - `Sorted`, when `r_offset` never decreases. For a non-resident
    relocation section of more than `REC_SAMPLE = 2^12` records, it also
    keeps the `r_offset` of every `REC_SAMPLE`-th record (8 bytes per
    4096 records), so that fill can locate a clip's records with one
    `copy_to`;
  - `Permuted(Vec<u32>)`, otherwise: the record indexes stably sorted by
    `r_offset` (4 bytes per record of an unsorted section).
  The index is a `mortar_core` type (`mortar_core::records::{RecIndex,
  Order}`, re-exported by `mortar_scan`; supervisor ruling for WP-027b),
  and it also carries the scan's per-section `leads` bit (`RelocAt.leads`,
  `spec/arch.md` §3.5), which fill passes on unchanged. The plan's records
  are `RecRows::Rela` (the input's `SHT_RELA` table, read at fill time
  through the store for the clip, in `RecIndex` order; I-82's "read
  twice") or `RecRows::Decoded` (records already in memory, for tests and
  the bench); the site bytes a record reads are taken from the window
  buffer the clip is filled into, not read again.
  Unsorted sections are ordinary on x86. Per archive, unsorted
  relocation sections and records (E7, E22): x86-64 glibc `libc.a` 245 of
  3,962 sections (11,053 records; the largest 783; e.g. `PLT32` records
  appended out of order in `libc-start.o`); i386 `libc.a` 73 (6,167);
  x86-64 `libstdc++.a` 446 (6,584); i686 `libstdc++.a` 184. No AArch64
  library and no `cxx-medium-g` object has one. No relocation section
  anywhere has two records at one offset (the review). Building the
  permutation costs a sort of at most 783 records per section, and at
  most 44 KB of memory for a whole `libc.a`. The alternative, filling an
  unsorted section as one unclipped job when it fits in a window, needs
  the multi-window job for the rest. It also makes job boundaries depend
  on record order, and saves nothing measurable. Rejected. GNU as and
  Clang's integrated assembler keep the order of `.reloc` directives, so
  any target can see unsorted records (E7).
- **I-30 [M1] Symbol values.** Fill computes, before the windows, the
  phase cache `sym_addr[SymbolId]: u64` (model §4.5: 8 bytes per global
  symbol) from `def`, `redirect`, `place`, `sym_place`, `defined`,
  `Split` and `MergeMap`, in parallel by `SymbolId` block (rule 1).
  Local symbols are computed on use from the file's symbol table (no
  per-entry row, model §9).

### 5.3 Windows and jobs

- **I-31 [M1]** The file is filled window by window. A **job** is one
  window, except that a *whole* synthetic chunk touching more than one
  window (§13, I-104; a chunk smaller than `W` can straddle a boundary)
  makes one job of every window it touches, together with the windows
  already in the job. The jobs are formed by one sequential pass
  over the plan. Job boundaries are a function of the plan and `W` only
  (rule 6).
- **I-32 [M1]** Jobs run in parallel (`for_each_part` over job indices;
  under `MORTAR_SCHED_SEED`, permuted). A worker owns one buffer of `W`
  bytes and, in the staged store, one relocation buffer of `SCAN_BLOCK`
  records (1.5 MiB at 24 bytes per record), reused between jobs. The
  window buffer is grown only for a multi-window job and shrunk back to
  `W` after it.
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
  threads), plus the relocation buffers of I-32 in the staged store, plus,
  for each multi-window job in flight, its length; plus the plan (I-28),
  `sym_addr` (I-30) and `RecIndex` (I-29). Only a whole synthetic chunk
  larger than `W` makes a multi-window job.

### 5.4 Filling one unit, clipped

The unit's bytes are defined as if the whole unit were filled into one
buffer; the clip only selects which of them a job writes.

- **I-35 [M1] Placed input section.** For the clip `[a, b)` of section
  `s` (offsets relative to the section's start):
  1. copy the input bytes `[a, b)` of `s` with `Inputs::copy_to` into the
     buffer. In the staged store (the primary design, I-81) these are
     positioned reads straight into the window buffer. In the resident
     store they are a copy from memory: the same call, the same bytes. A
     `SHF_COMPRESSED` input is copied from the store's decompressed
     buffer (`spec/layout.md` L-52);
  2. apply every relocation record whose write window
     `[r_offset − before, r_offset + width + after)` intersects `[a, b)`,
     in record order, through a `SectionOut` whose valid range is
     `[a, b)`: bytes the relocation would write outside it are not
     written (request R-2: `apply`'s results for the bytes inside the
     clip are those of an unclipped call, which holds because `apply`
     never reads `out`, `spec/arch.md` §3.6). The records are those with
     `a − max_reach.after − 8 < r_offset < b + max_reach.before`, where 8
     is the widest field. They are found by binary search in `RecIndex`'s
     order (I-29: the records themselves when `Sorted`, the permutation
     when `Permuted`, the sample then one `copy_to` of the records between
     two samples when non-resident), and are then taken in record order;
     a sequence member finds its lead by record index (`spec/arch.md`
     §3.5);
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
- **I-111 [M1] `max_reach` is derived, never written by hand.**
  `Facts.max_reach = (before, after)` is the maximum, over every window
  the architecture states, of how far the window extends before the
  field's first byte and past its last. That covers each type's `Howto`
  window (`spec/arch.md` §3.2) and each matched form's member windows
  (`unit_window`, §3.5). `mortar_arch` computes it from those tables
  (request R-2), and a test checks that no `apply` writes outside its
  record's window. Values this gives:
  - AArch64 at least `(0, 4)`. The `CALL26` member of the GD→LE sequence
    (`spec/a64.md` §8.4) has its field at `o+8` and owns `[o+8, o+16)`,
    so it writes the `nop` 4 bytes past its field.
  - x86-64 `(4, 15)`: `before` 4 from the 16-byte `TLSGD` forms and
    `after` 15 from the 22-byte `TLSGD` and `TLSLD` forms (`spec/x64.md`
    §8.6).
  The draft's AArch64 `(0, 0)` was wrong; it worked only because of the
  8-byte slack of the search bound. The x86-64 large-model `TLSGD`
  sequence (`lea` / `movabs PLTOFF64` / `add` / `call`, 22 bytes,
  `[P − 3, P + 19)`, after = 15) and its `TLSLD` twin are relaxed forms
  (`spec/x64.md` X-25, X-27; request R-14, answered). GNU ld relaxes all
  22 bytes; lld 22 writes 16 bytes from `P − 4` and corrupts it (the
  review). `max_reach` follows from the tables.
- **I-37 [M1] `.ctors`/`.dtors` reversal** (`spec/layout.md` L-40, when it
  is lifted): the words of a `.ctors` input section are written in
  reverse word order; the relocation of the word at offset `o` is applied
  at offset `size − width − o`. The clip applies to the reversed
  positions. Until L-40 is lifted, layout refuses and fill never sees
  one.
- **I-38 [M1] Fragment runs.** A run writes each fragment's bytes (from
  its first occurrence, `Split` §3.5) at the fragment's chunk offset.
  A fragment's relocations, if its section has any, are applied from
  its first occurrence, as I-35 applies a section's. This was not
  observed in the inputs of `cxx-medium-g`, `libc.a` or `libstdc++.a`:
  no `SHF_MERGE` section in them has a relocation section (E7).
- **I-39 [M1] Synthetic chunks** are written by `spec/synth.md`'s
  `write(cx, range, out)` for the clip (§13). `Patches` chunks (islands,
  thunks, erratum patches) are synthetic chunks of `mortar_arch`'s
  `write_*` (`spec/arch.md` §6), with the key `(100, 0, ChunkId)`.
  WP-027c writes them whole before fill (`link::patches`): each thunk by
  `write_thunk` with its target as the fixed point computed it, zero
  between patches. The erratum fixes (`spec/a64.md` §11.2: `b patch` at
  `F`, the patch's `W4'` and `b F + 4`, or the ADR at `Q`) are words
  written over fill's bytes (`Link::fixups`, applied in every window
  after its rows, so the build id covers them). The front computes them
  from the words fill writes at the final addresses, with the same scan
  as the fixed point (`LinkSites::erratum_fixes`). A thunk's target word
  is not proved at this point, so in a BTI output `thunk_target_ok`
  refuses it.
- **I-40 [M1] Structural units.** The ELF header, the program header
  table and the section header table are written by the image from
  `Placement` and the output kind: `e_entry` from `Placement.entry`;
  `e_shstrndx` the `.shstrtab` chunk's section index; a program header
  per `Placement.segments` row; a section header per output section
  (`sh_size` the section's last chunk's end minus its first chunk's
  start, `spec/layout.md` L-86). Their encodings are `spec/elf.md`'s
  writer half, and the byte layout is a function of the class and the
  rows; nothing in them is filled from input bytes.
  The fields (WP-027b, `link::structure`):
  - `e_ident[EI_OSABI]`: `ELFOSABI_GNU` when the output has an
    `STT_GNU_IFUNC` or `STB_GNU_UNIQUE` symbol or an `IRELATIVE` record,
    else `ELFOSABI_NONE`, on every target (a64.md Q11, ruled by WP-027c;
    x64.md X-49; x86.md X86-64; `spec/dyn.md` D-23 for `.dynsym`; plumb
    S1);
    `e_type` from the output kind (`spec/dyn.md` D-1: `ET_EXEC` for
    `Static` and `Exec`, `ET_DYN` for `Pie` and `StaticPie`);
    `e_machine` `Facts.e_machine`;
    `e_flags` `Arch::e_flags` over the live objects' `e_flags`;
  - `e_entry` is `Placement.entry`, or 0 when it is `None` (the gABI's
    "no entry point"; layout's `entry-not-found` warning, L-103);
  - `e_phoff`/`e_shoff` are the `ProgramHeaders`/`SectionHeaders` chunks'
    offsets; `p_align` is `1 << align` of the segment row;
  - `sh_name` from `.shstrtab`'s plan; `sh_type`, `sh_flags` and
    `sh_addralign` from the section's chunks (the largest alignment);
    `sh_addr` the first chunk's address (0 for non-alloc), `sh_offset`
    its offset;
  - `sh_entsize`: a synthetic chunk's from `SynthKind::header` (synth.md
    §3); an output section that keeps `SHF_MERGE` its merge class's
    entry size (L-50); `SHT_INIT_ARRAY`, `SHT_FINI_ARRAY` and
    `SHT_PREINIT_ARRAY` the word size; otherwise 0;
  - `sh_link`/`sh_info`: `.symtab` links `.strtab`, its `sh_info` the
    plan's first global (S-73); `.symtab_shndx` links `.symtab`;
    `.rela.plt` has `sh_link` 0 and `sh_info` `.got.plt` (S-69) in a
    static output; [M2] the dynamic sections' `sh_type`, `sh_flags`,
    `sh_link`, `sh_info`, `sh_entsize` and `sh_addralign` are
    `spec/dyn.md` D-34's table (`.rela.plt` then links `.dynsym`);
  - `.pad.relro` (layout's own `NOBITS` chunk, L-64, L-156) has a header
    and no synthetic plan;
  - extended numbering (gABI): when the section count or `.shstrtab`'s
    index reaches `SHN_LORESERVE`, `e_shnum` 0 and `e_shstrndx`
    `SHN_XINDEX`, with the values in section header 0;
  - each structural chunk's size is checked against its encoding
    (`internal-error` otherwise).

- **I-116 [M1] The phase's entry point** (`link::write`, WP-027b). It
  reads the front's tables (`Front` of `spec/landed/pipeline/
  FRONT-INTERFACE.md`) by reference, as `link::Link`, and in order:
  builds the fill tables (`LinkTables`: the global symbols' values in
  parallel by `SymbolId`, I-30, without `redirect`: the record keys are
  the scan's, already redirected); writes the structural units (I-40);
  builds the plan (`from_layout`) with the synthetic chunks of
  `Link.synth` (synth's own `SynthChunk`s and `FillCx`); creates the
  output (§7.2), fills it (§5.3), rechecks the inputs (I-118), and
  commits. `on_phase(100, "image")` and `on_phase(105, "output")` are
  called as those phases end, for `--time-phases` (§11.2). The target
  kind of each key is the scan's (`link::KeyFacts`, which the driver
  answers with the scan's own view), so fill and scan cannot disagree.
- **I-117 [M1] Values in non-allocated sections.** A symbol or fragment
  in a non-alloc chunk (`.debug_*`, `.debug_str`'s merged chunk) has the
  value of its offset from its output section's start (the gABI's
  `sh_addr` 0), and a relocation of a debug section against it is
  applied with that value (GNU ld, lld).
- **I-118 [M1] The end of the phase** (`spec/input.md` §11.5). After
  every window is written and before the rename, every input file the
  store still holds open is checked again (`Store::recheck`, through the
  driver's `Link.recheck` callback: `mortar_image` does not depend on
  `mortar_input`). A file whose size, `st_mtime` or `st_ctime` changed
  is `input-changed` at `(100, FileId, 0)`, and the link fails with no
  output file (I-65).
- **I-119 [M1] References into split sections** are resolved by the
  fill tables (`Tables::split_target`, which gives S + A): a section
  symbol of a `Merge` section by the piece at `st_value + A`
  (`spec/synth.md` S-17, S-18, `mortar_merge::refs::section_symbol`); a
  named symbol of one by its own piece, with A added after (S-19, S-22);
  a symbol in `.eh_frame` by `EhPlan::value` (S-50), and the relocations
  of `.eh_frame` itself at `EhPlan::place` (S-52, S-53).

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
  `.gnu_debuglink`): the **tombstone** of I-47, whatever the addend. Such
  a section is usually deferred to fill (I-14): the target's liveness is
  then read at fill (`SymValue::live`, from `Liveness`), which is where
  `arch.scan` would have found the target dead. Under `--gc-sections` a
  live `Merge` section can hold collected pieces (I-7, `frag_live`): a
  reference whose target is such a piece (a section symbol plus addend,
  or a named symbol in it) is a reference to a section that is not live,
  and writes the tombstone too (`Tables::split_target`'s `Ok(None)`; a
  named symbol's `SymValue::live` is false). The deferred check passes it
  (the target is defined), so it raises no diagnostic. Found by the
  WP-031 differential (`clang++ -O2 -g`: `.debug_info` naming an unused
  `.rodata.str` piece); test `tests/deferred.rs`
  `i14_dead_targets_are_the_tombstone_at_fill`.
- **I-46 [M1]** Exactly `.gcc_except_table` (alloc or not): 0
  (`spec/resolve.md` §6.3). Any other non-alloc section: the scan
  reported `discarded-section-reference` (§4.4) and fill never runs.
- **I-47 [M1] [C] The tombstone.** `tombstone(section, width, class)`.
  "All ones" means all ones in the field's width; it is the value DWARF 6
  reserves as an address's tombstone, and what lld writes under
  `-z dead-reloc-in-nonalloc=<section>=<all ones>`.

  | referring section | ELFCLASS64 | ELFCLASS32 |
  |---|---|---|
  | `.debug_ranges`, `.debug_loc` | 1 | 1 |
  | `.debug_rnglists`, `.debug_loclists`, `.debug_addr` | **0** | 0 |
  | `.debug_aranges` | all ones, **and the tuple's length 0** (I-112) | the same |
  | `.debug_frame` | all ones | all ones |
  | every other debug name (`.debug_info`, `.debug_line`, `.debug_str_offsets`, …) | all ones | **0** |

  Why each row (E9, E10, E16, E17, and WP-027e's matrix,
  `spec/landed/pipeline/WP-027e.md`):
  - `.debug_rnglists`/`.debug_loclists`/`.debug_addr` (DWARF 5): their
    entries pair an address with a length (`DW_RLE_start_length`,
    `DW_LLE_start_length`; through `.debug_addr`, Clang's
    `DW_RLE_startx_length` and `DW_LLE_startx_length`), so all ones makes
    `[2^64 − 1, len − 1)`, which wraps. gdb 16 then reports `Invalid
    .debug_rnglists data (inverted range)` and drops the CU's whole range
    list, and stepping into that CU's code says `pc … in read in CU, but
    not in symtab` (GCC 14 `-gdwarf-5`, its default, `-O0`). 0 is what
    GNU ld (without a kept copy), lld 19/22 and mold write: gdb skips an
    entry that starts at 0 when no section is there (`.debug_rnglists
    entry has start address of zero`), a subprogram whose
    `DW_FORM_addrx` `low_pc` is 0 is dropped by gdb and lldb 19, and
    lldb's breakpoints, lookups and steps are those of the all-ones
    output. It is sound because no code lies in `[0, len)`: the non-PIE
    base is at least 4 MiB (`spec/layout.md` L-68), so only a dead
    function of 4 MiB or more could reach it. In a PIE [M2] the rows
    stand (`spec/dyn.md` D-72, §15 Q6, ruled as lld): the 0 rows stay 0
    and `.debug_ranges`/`.debug_loc` stay 1, since no code is at 0 in a
    PIE either (the first `PT_LOAD` holds the headers). E16 measured
    lldb losing lines with the references' 0 in a low layout; a low
    layout stays as layout L-68 has it. `llvm-dwarfdump --verify` reports `DIE has overlapping
    ranges` for a CU's several `[0, len)` entries (lld and mold: the
    same); the debuggers are the oracle.
  - `.debug_ranges`/`.debug_loc` (DWARF ≤ 4 pairs): `(0, 0)` ends the
    list and a first word of all ones selects a base address; `(1, 1)` is
    empty. lld and mold write 1.
  - `.debug_aranges` (GCC emits it; Clang does not by default): all ones
    makes `[2^64 − 1, +len)`, which wraps, and lldb 19 then cannot map
    line-table addresses to a CU (`unable to resolve a line table file
    address`) and loses every other function's lines. A 0 address (lld,
    mold, and the coordinator's ruling) claims `[0, len)`, which fails
    the same way when code starts below `len`: GCC's `[0, 0x3d08)`
    covers `.text` at `0x1740` in the low layout. With the length 0 the
    tuple is empty and every row below passes. A tuple address of `2^63`
    also passed every 64-bit row; the empty tuple was chosen because it
    is empty in both classes.
  - ELFCLASS64 elsewhere (`DW_FORM_addr` in `.debug_info`,
    `DW_LNE_set_address`): gdb 16 and lldb 19 drop a subprogram whose
    `low_pc` is all ones (`llvm-dwarfdump` prints `dead code`). A 0 is an
    address: in the low layout lld's and mold's Clang DWARF 5 outputs
    lose lldb's lines because `.debug_addr` claims `[0, size)`.
  - ELFCLASS32 elsewhere: gdb 16 and lldb 19 do *not* treat `0xffffffff`
    as a tombstone (they add `size` in 64-bit arithmetic, so it does not
    wrap): `break big` gets a second location at `0xffffffff` in both. A
    0 is filtered by gdb, and lld writes 0. Residual: i386 Clang DWARF 5
    in the low layout still loses lldb's lines, with every linker (Q11).
  - `.debug_frame`: a dead FDE at all ones overlaps no code, even in
    64-bit arithmetic. i386 backtraces under `qemu-i386` were the same
    with 0 and all ones (E17); the row rests on that argument.
  - `.debug_str_offsets` has no relocation against code (its entries
    point into `.debug_str`), so the row never fired in E16.

  The 64-bit matrix (E16): the reviewer's two objects with a COMDAT
  inline `big`, the second copy dropped; GCC 14 and Clang 19, DWARF 4
  and 5, `-O0` and `-O2 -fno-asynchronous-unwind-tables` (which adds
  `.debug_loc`/`.debug_loclists` and Clang's `.debug_frame`); the default
  layout (`-no-pie`) and the low layout (`-pie -z max-page-size=4096 -z
  noseparate-code`); the reviewer's gdb and lldb scripts,
  `llvm-dwarfdump-22 --verify`, `addr2line`, `llvm-symbolizer-22`.
  "lldb ok" is 17 `at file:line` frames and no `unable to resolve`;
  failures give 6–10 and 5–7.

  | AArch64, 16 configurations per column | lld 22 | mold | GNU ld | all ones (draft) | ones, aranges 0 (ruling) | **I-47** |
  |---|---|---|---|---|---|---|
  | gdb transcript (addresses masked) | same | same | same | same | same | same |
  | lldb, GCC, default layout | ok | ok | ok | **fails** (wrap) | ok | ok |
  | lldb, GCC, low layout | fails | fails | ok | **fails** | **fails** | ok |
  | lldb, Clang 4, both layouts | ok | ok | ok | ok | ok | ok |
  | lldb, Clang 5, low layout | fails | fails | fails | ok | ok | ok |
  | `--verify` errors, DWARF 4 | 3 | 3 | 0–5 (by compiler and layout) | 0 | 0 | 0 |
  | `--verify` errors, Clang DWARF 5 | 0 | 0 | 6 (`.debug_str_offsets`) | 0 | 0 | 0 |
  | `addr2line`, function starts | right | right | `main` wrong (GCC `-O2`, low) | right | right | right |
  | `llvm-symbolizer` `big`, low layout | wrong (GCC, Clang 5) | wrong (GCC, Clang 5) | wrong (4 of 8) | right | `??` (GCC) | right |

  i386 (E17; static probes, and gdb under `qemu-i386`): with the
  ELFCLASS32 column, breakpoints and lines equal lld's everywhere, and
  the aranges rule fixes GCC in the low layout, which lld, mold and GNU
  ld all fail. `--verify` gives lld's counts.

  The draft's claim that `--verify` is clean for the references' outputs
  was wrong (the DWARF 4 `DIE address ranges are not contained in its
  parent's ranges`, GNU ld's `.debug_str_offsets` errors). `--verify` is
  reported, not the oracle; the debuggers are. Low text addresses are
  mortar's to produce: x86-64's maxpage is 4 KiB (`spec/layout.md` L-74,
  M3), and `-z max-page-size` lowers it on AArch64. GNU ld's redirect to
  the kept copy was rejected: it asserts that the two copies are the same
  code, which the linker does not know (the ODR question `mortar_odr`
  exists for, M6), and it gives `addr2line` a wrong line for `main`.
- **I-112 [M1] The aranges length.** For a relocation in `.debug_aranges`
  whose target is a section that is not live, whose type is the class's
  address-sized absolute type (`ABS64`, `R_X86_64_64`, `R_386_32`), and
  whose target is `SHF_ALLOC`, fill also writes 0 into the `width` bytes
  that follow the field: that is the tuple's length in every DWARF
  version (segment selectors are 0 on Linux). The rule applies unless a
  relocation record has its field there, in which case those bytes are
  left as the input has them. It is a function of the record and the
  section bytes, so clipping applies as for any write (`max_reach.after`
  is raised to at least 8 for `.debug_aranges` in I-35's search). No DWARF is
  parsed.
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
- **I-49a [M2] Split DWARF** (WP-080, `spec/landed/image/WP-080.md`).
  An object built with `-gsplit-dwarf` (DWARF 4's GNU extension or
  DWARF 5) is linked by I-49 with nothing added: its skeleton units
  (`DW_UT_skeleton`, or DWARF 4's `DW_AT_GNU_dwo_name` unit),
  `.debug_addr`, `.debug_str_offsets`, `.debug_rnglists`/`.debug_ranges`,
  `.debug_aranges`, `.debug_gnu_pubnames`/`.debug_gnu_pubtypes` and a
  producer's per-unit `.debug_names` are ordinary units, concatenated in
  input order and filled per §5.4 (no index is merged or rebuilt: I-54);
  `.debug_str` and `.debug_line_str` merge per I-38. A dead target's
  entry gets I-47's row (`.debug_addr` 0, `.debug_ranges` 1); a TLS
  variable's `.debug_addr` entry is its DTP offset (`spec/x64.md` X-53,
  `spec/x86.md` X86-62). The `.dwo` sections Clang's
  `-gsplit-dwarf=single` leaves in the object carry `SHF_EXCLUDE` and are
  dropped (`spec/layout.md` L-31). The `.dwo` files are not opened, read
  or rewritten, and `DW_AT_dwo_name`/`DW_AT_comp_dir` reach the output as
  the producer wrote them: a debugger finds the `.dwo` files (or a `.dwp`
  that `dwp`/`llvm-dwp` makes from the output) as for GNU ld's output.
  All three references do the same with every section (WP-080's census,
  256 links each).
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
- **I-54 [M2] `.gdb_index`** (WP-097; amended from "[M4] `.gdb_index`
  and `.debug_names` are synthetic chunks of `spec/synth.md` that read
  DWARF; they are divisible by CU range"). `--gdb-index` makes the
  synthetic chunk of `spec/synth.md` §17 (S-3 row 28): complete for
  every live unit, or the link is refused by name
  (`gdb-index-incomplete`, S-G3). Its plan reads the inputs' unit
  headers, root DIEs, range lists and GNU name sets in phase 80; the
  image writes its header, CU list and address area from the layout and
  copies the rest from the plan; it is `Whole` (S-G7a), not divided by
  CU range until a measurement asks for it. With the option the input
  `.debug_gnu_pubnames`/`.debug_gnu_pubtypes` are consumed, not copied
  (S-G6); everything else of I-49 and I-49a is unchanged, and the other
  debug sections are still not parsed by the image. A merged
  `.debug_names` is not written: `--debug-names` stays refused (S-G9);
  the inputs' per-unit tables are concatenated (I-49a).
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
- **I-58 [M1] [C]** If `P` exists and, following symbolic links (WP-087),
  is neither a regular file nor a directory, it is written in place only
  if it is a character device (`/dev/null`, a terminal): the image opens it for writing without
  creating or truncating it and writes it directly (§7.3). A FIFO or a
  socket at `P` is `cannot-open-output` with the note `output is a FIFO`
  (or `a socket`): the image writes by position (I-63), which a FIFO
  rejects (`ESPIPE`), and streaming it in order would serialize fill. This
  is a choice: lld and mold write a FIFO (they build the image in memory),
  GNU ld fails with `Illegal seek` (§15). A block device is refused the
  same way (`output is a block device`).
- **I-58a [M1] [C] A descriptor alias (WP-087).** If `P` is a symbolic
  link whose chain passes through a directory `/proc/<pid>/fd` or
  `/proc/<pid>/task/<tid>/fd` (`/dev/stdout`, `/dev/stderr`,
  `/dev/fd/N`) and ends at a regular file, `P` is opened for writing
  through the link, truncated, and written in place: `-Map=/dev/stdout >
  a.map` writes `a.map`. Without this the writer of I-59 would create
  `/dev/.stdout.mortar-tmp` and rename it over the `/dev/stdout` link
  (as root in a container, where `/dev` is writable: observed before
  WP-087), or fail as `cannot-open-output` elsewhere. A FIFO or a socket
  behind such a link is I-58's case for the output and I-79a's for the
  other files; a terminal is a character device.
- **I-59 [M1] [C] Otherwise: a temporary file.** Let `T0 =
  dir(P)/.<base(P)>.mortar-tmp`. The image opens `T0` with `create_new`, or,
  if it exists, opens the existing file, write-only, mode `0o777` for executables and shared
  objects and `0o666` for `-r` (the kernel applies the umask, which
  reproduces §7.1's modes), and then calls `File::try_lock` (an exclusive
  `flock`, stable Rust, no `unsafe`):
  - It first checks that the file system enforces the lock: a second
    descriptor of `T0`, opened in the same process, must fail `try_lock`
    with `WouldBlock`. On `/tmp` (tmpfs) it does; on this host's
    workspace share (a FUSE `fakeowner` mount) it does not: `try_lock`
    succeeds on both descriptors, and across processes (E25).
  - Enforced and acquired, and `T0` still names the locked inode: no
    running link holds `T0`, so a `T0` that existed is a stale file of a
    killed link. The image removes it and creates `T0` again with
    `create_new` (a truncated stale file would keep the killed link's
    mode), locking the new inode before releasing the old one; if another
    link wins the new name, the image falls back as for `WouldBlock`. A
    lock acquired on an inode that `T0` no longer names (another link
    replaced it meanwhile) is treated as `WouldBlock`. A `T0` this link
    created on a file system that does not enforce the lock is removed
    again. This is how a later link removes an
    interrupted link's litter (G3): the name is derivable, and the lock
    says whether its owner is alive. The kernel drops the lock when the
    owner dies, `kill -9` included (E25).
  - Enforced and `WouldBlock`: another link to `P` is running. The image
    falls back to `T = T0.<pid>` created with `create_new` (then `.1`
    … `.63` if a stale one exists), and does not touch `T0`.
  - Not enforced, or `try_lock` returns `Unsupported`: the image never
    reuses a name. It creates `T = T0.<pid>` with `create_new` as above.
    A killed link can leave `T` behind on such a file system, and no later
    link removes it (deleting a file whose owner cannot be shown dead is
    unsafe). The link works without locks.

  `create_new` alone (the draft) was rejected: it never collides, but
  every interrupted link leaves litter that no one removes, and
  `.gitignore` patterns cannot predict a pid. Every failure to create `T`
  is `cannot-open-output` **naming `P`**, with the system's reason: a
  read-only directory gives lld's line, `cannot open output file P:
  Permission denied`. The lock is held until the rename (I-60). NFS and
  SMB shares were not available to test; `flock` on NFSv4 is emulated
  with byte-range locks, and the probe decides at run time either way.
- **I-60 [M1]** After a successful fill and sealing (§8), `T` is renamed
  to `P` (`std::fs::rename`) and then the descriptor is closed: the lock
  of I-59 covers the rename, so no other link can take `T0` for stale
  between the close and the rename. (A close error cannot be observed
  through `std::fs::File`; it is not reported.) This gives
  §7.1's rows: a new inode, hard links unaffected, a symbolic link
  replaced (its target untouched), a running executable replaced. A failed
  rename is `cannot-write-output`, and `T` is removed.
- **I-113 [M1] [C] The old output is set aside.** Renaming over an
  existing `P` makes the kernel free the old inode's blocks inside
  `rename`, on the linker's critical path. On `/tmp` that costs 36 ms
  for a 220 MB output and 0.73 s for 2 GB (E20). So, after `T` is created
  and before fill, if `P` is a regular file or a symbolic link and is not
  the same file (device and inode, following the link) as any input, the
  image renames `P` to `A = dir(P)/.<base(P)>.mortar-old`, replacing any
  stale `A`. A helper thread then unlinks `A`, overlapped with fill, and
  is joined before the process exits. Measured on `/tmp` (td.rs; load
  32–36):

  | output | fresh | over an existing one, plain rename | set aside |
  |---|---|---|---|
  | 220 MB | 100 ms | 140 ms | 94–114 ms |
  | 2 GB | 0.91 s | 2.40 s (1.67–4.05) | 0.82–1.11 s |

  Holding a descriptor of the old inode open until exit does not help
  (2.10 s): the free then happens at exit, still inside the link's wall
  time. A hard link to the old output keeps it alive, so the unlink frees
  nothing and the other name keeps the old bytes (§7.1). A symbolic link
  at `P` is itself moved, and its target is untouched. If `P` is an input,
  nothing is set aside: since WP-087 the facade never reaches this (I-62
  refuses such a link before `T` is created); the skip stays in
  `Output::create`, whose `OutputOptions::inputs` callers other than the
  facade may still pass, as a defensive guard that no link can now
  trigger. If the rename to `A` fails (it is in the same directory, so
  only a permission or a race can fail it), the image proceeds without it. On the
  workspace share the write itself dominates (0.8–1.2 s for 220 MB), and
  the set-aside changes nothing measurable.
- **I-61 [M1]** No `fsync` of `T` or of the directory: a linker's output
  is a build artifact, re-creatable, and an `fsync` of a 2 GB debug
  output would wait for the device. (Whether the references sync was not
  measured, E14.)
- **I-62 [M1] [C] The output is an input: refused (WP-087).** An output
  `P` that is, following symbolic links, the same file (device and inode)
  as a file the link read (an object, an archive, a thin archive's member,
  a linker script, a response file, a shared object) is
  `written-file-clash` by the one rule of every written file
  (`spec/cli.md` §3.14), decided after the front and before `T` is
  created: nothing is written and the input is byte-equal afterwards
  (I-66 does not remove it). A non-regular `P` (`-o /dev/null`) is outside
  the rule (I-58). GNU ld 2.44 refuses it by name or symbolic link (not
  by hard link: `input file 'a.o' is the same as output file`); lld 19
  and 22 accept it and replace the input; mold 2.37.1 writes in place and
  fails with the input destroyed (`ld -r -o a.o a.o b.o`: an assertion
  after replacing it, measured by the coordinator).
  *History.* Before WP-087 mortar accepted it, as lld and mold do (Q8):
  it was safe for the link itself because mortar never writes in place
  (the resident store has read every input whole before fill,
  `spec/input.md` §11.2; the staged store reads through descriptors
  opened before `T` is created, which pin the old inode), and I-113
  skipped such a `P`. It changed because the input is still replaced
  when the link succeeds: a prebuilt object or archive named by a typo in
  `-o` cannot be rebuilt, a refusal by name is recoverable, and GNU ld,
  the reference of record, already refuses it, so no build written for
  it depends on the output being an input.

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
  removed, and so is `A` (I-113). Neither is renamed back: a failed link
  leaves no output, consistent with I-66.
- **I-66 [M1] [C]** If the link fails at any phase after the command
  line (ordinal ≥ 15), an existing **regular** output `P` is removed,
  unless it is the same file (device and inode) as one of the inputs.
  This is GNU ld's
  behaviour (§7.1); lld and mold keep the stale output. The reason: a
  build that ignores the status, or a developer who runs the program
  after a failed link, would run the previous program believing it is
  the new one; a missing file cannot be mistaken. A failure on the
  command line keeps the output, as all three do. An output that is also
  an input is never removed (it would destroy the input); since WP-087
  "an input" is every file the link read (`spec/cli.md` §3.14: the
  response files included). A symbolic link at `P` is removed, not its
  target, unless its target is not a regular file or it is a descriptor
  alias (I-58a: `-o /dev/stdout`); a non-regular `P` is left alone. Accepted by the coordinator (Q4).
- **I-67 [M1]** A link killed by a signal leaves `T` (and possibly `A`)
  behind: safe Rust installs no handler. On a file system that enforces
  `flock`, the next link to the same `P` reuses `T0` (I-59) and replaces
  `A` (I-113), so litter is bounded to two files per output and lasts
  until the next link. Elsewhere, a killed link's `T0.<pid>` stays.
  For comparison (the review's observation, not re-run): after `SIGINT`,
  lld leaves nothing, mold leaves `.out.<pid>`, and GNU ld leaves a
  4 KiB partial output at `P` (§15).

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
  bytes up (E13). Re-checked for 8, 16, 20 and 32 bytes: gdb and lldb
  find the debug file for all four (E24).
- Producers and consumers of a style (E24): every driver on the host
  passes bare `--build-id`, and no CMake module passes a style (rpm,
  debhelper and meson are not installed). Nothing found checks more than
  the length: `file` labels 8 bytes `xxHash`, 16 `md5/uuid`, 20 `sha1`.

### 8.2 Styles

- **I-69 [M1]** `--build-id[=STYLE]`, `--no-build-id`:

  | spelling | mortar | length | milestone |
  |---|---|---|---|
  | absent, `--build-id=none`, `--no-build-id` | no note | — | M1 |
  | `--build-id` (bare, `Default`) | the fast tree (§8.3) **[C]** | 20 | M1 |
  | `--build-id=sha1` | the SHA-1 tree | 20 | M1 |
  | `--build-id=sha256` | the SHA-256 tree **[C]** (mold accepts it; GNU ld and lld do not) | 32 | M1 |
  | `--build-id=fast` | the fast tree, truncated **[C]** (lld's length) | 8 | M1 |
  | `--build-id=md5` | refused (`option value`, note `use sha1`) **[C]** | — | — |
  | `--build-id=uuid` | 16 bytes from `/dev/urandom`, RFC 4122 version 4 bits set (mold's form) | 16 | M2 (refused in M1) |
  | `--build-id=0xHEX` | the bytes of `HEX` | `len(HEX)/2` | M1 |
  | any other | `invalid-option-value` | — | M1 |

  The last of these options on the line wins (`spec/cli.md` §2.10).
  Bare `--build-id` is what every driver passes, so it gets the function
  that is cheapest at the length every tool expects (20, `sha1`-sized).
  `md5` is refused because no producer on the host asks for it, and it
  would be a fourth function to implement, test and verify for nothing;
  refused by name, it fails loudly if a build does ask. `uuid` makes the
  output depend on something other than the inputs, so the determinism
  gate (`docs/VALIDATION.md`) excludes links that use it. mold sets the
  version and variant bits; lld and GNU ld do not (one id each, E24).
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
  zero padding to a multiple of 4 bytes: `size = 16 + align_up(L, 4)`.
  The references write `size = 16 + L` (18 bytes for `0xABCD`). Every
  style but `0xHEX` has `L` a multiple of 4, so the difference is only
  there. The reviewer saw no `readelf -n` warning for the padded form;
  this spec's E11 saw one (`note with invalid namesz and/or descsz`) for
  the unpadded 18-byte form. Request R-12 asks `spec/synth.md` for the
  same formula.

### 8.3 The function

- **I-72 [M1]** For a hash style with leaf function `H`, root function
  `R` and length `L`: let `F` be the output file's bytes with the
  descriptor `[d, d + L)` replaced by zeros, `n = file_size`, `B =
  IMAGE_BLOCK = 2^20`, and `m = ⌈n / B⌉`. Then

  ```text
  leaf_i = H(F[i·B, min((i+1)·B, n)))                    for 0 ≤ i < m
  root   = R("mortar-build-id-2\0" ‖ le64(n) ‖ le64(B) ‖ leaf_0 ‖ … ‖ leaf_{m-1})
  id     = root[0 .. L]
  ```

  | style | `H` (leaf bytes) | `R` | `L` |
  |---|---|---|---|
  | bare, `fast` | `L128` (16) | SHA-256 | 20, 8 |
  | `sha1` | SHA-1 (20) | SHA-1 | 20 |
  | `sha256` | SHA-256 (32) | SHA-256 | 32 |

  `fast` is the bare id's first 8 bytes. SHA-1 and SHA-256 are FIPS
  180-4's, written from the standard (mortar ships no dependency for
  them). Neither uses hardware instructions, which would need `unsafe`
  intrinsics. The tag names the definition's version (I-75).

  Bytes exactly (WP-026h): the tag is the 17 ASCII bytes
  `mortar-build-id-2` and one zero byte (18 bytes); `le64` is 8 bytes
  little-endian; each `leaf_i` is its function's digest as bytes (SHA-1's
  and SHA-256's in FIPS 180-4's big-endian word order, `L128`'s as I-114
  ends), concatenated in index order with nothing between them. For `n =
  0` there are no leaves, and the bare and `sha256` roots are equal (both
  SHA-256 of the same 34 bytes); no ELF output is empty. Vectors: §8.4.
- **I-114 [M1] `L128`**, defined here so that `mortar_plumb` and any
  re-implementation compute it bit for bit. All arithmetic is on `u64`,
  wrapping; `rotl` rotates left; words are little-endian.

  ```text
  P1 = 0x9E3779B185EBCA87   P2 = 0xC2B2AE3D27D4EB4F
  P3 = 0x165667B19E3779F9   P4 = 0x85EBCA77C2B2AE63
  round(v, w) = rotl(v + w·P2, 31) · P1
  fmix(x)     = x ^= x >> 33; x ·= P2; x ^= x >> 29; x ·= P3; x ^= x >> 32
  L128(d), n = len(d):
    v = [P1 + P2, P2, 0, 0 − P1]
    for each whole 32-byte stripe s of d: v[k] = round(v[k], le64(s[8k .. 8k+8])), k = 0..3
    if 0 < n mod 32: pad the rest of d with zeros to 32 bytes, and do the same once
    a = fmix(rotl(v0,1) ^ rotl(v1,7) ^ rotl(v2,12) ^ rotl(v3,18) ^ n·P4)
    b = fmix(round(round(round(round(n, v3), v2), v1), v0) ^ a·P3)
    L128(d) = le64(a) ‖ le64(b)
  ```

  Reading the definition (WP-026h): `·` binds tighter than `+`, and both
  tighter than `^`; `n` in the last two lines is the byte length as a
  `u64`; `0 − P1` is `2^64 − P1`; `round(round(…(n, v3)…), v0)` passes `n`
  as `v` and each lane as the word `w`; the stripes are taken from offset
  0 in order, and the zero-padded tail is one more stripe, after the
  whole ones; `a` is computed before `b`, which uses it. The lanes, their
  initial values and `round` are XXH64's (seed 0); the finish is not.
  Vectors: §8.4.

  It is our own multiply-rotate design, in the family of the published
  64-bit multiplicative hashes (the constants are their usual ones). It
  is not cryptographic, and does not need to be: a build id must tell
  apart outputs that differ, and nobody forges a linker's id to collide
  with another. The argument:
  - For a fixed word, `round` is a bijection of `v` (`P1` is odd and
    rotation is a bijection), so two leaves can only collide if their
    lanes do, and the length is mixed into both halves. Inputs whose
    differences all fall in one lane collide with probability about
    2^-64, not 2^-128. That bounds the bare id at about 2^-64 per pair of
    different outputs.
  - Measured (`bh quality`, E23): flipping any one input bit flips each
    output bit with probability 0.5000 on average (worst output bit
    |p − 0.5| = 0.0205, over 64 seeds and lengths 8 to 4096). For 2^20
    structured 4 KiB leaves that differ in one 4-byte counter, the low
    32 bits collide 91 times (128 ± 11 expected for random 32-bit
    values) and the 128-bit values never do. For 81,602 sparse leaves
    (all zeros of each length 0 to 65,536, and single nonzero bytes)
    there are no collisions. Re-measured on `mortar_digest`
    (`tests/l128_quality.rs`, WP-026h): 118,272 single-bit flips
    (lengths 8, 31, 64, 100, 4096; 64 random inputs each; every bit, or
    512 sampled) give mean 0.5002 and worst |p − 0.5| = 0.0041 (the
    sampling noise is about 0.0015 per output bit); 2^20 4 KiB zero
    leaves with a 4-byte counter at offset 2048 collide 138 times in the
    low 32 bits (128 ± 11 expected), never in 128; 81,857 sparse leaves
    (zeros of each length 0 to 65,536; each byte value 1–255 at each
    offset of a 64-byte zero leaf) do not collide.
  - The root is SHA-256 over all leaves and parameters, so the root adds
    no weakness of its own.
- **I-73 [M1]** The leaves are window digests: a job hashes each of its
  windows after filling and before writing it (I-33 step 4), into
  `leaves[i]` (a column indexed by window: rule 1). The window holding
  the descriptor is hashed with the descriptor's zeros, as the synthetic
  chunk wrote it. After the last job, the root is computed on one thread
  (`m × 16` bytes for the bare id: 32 KB for a 2 GB output) and the id is
  written with one `write_all_at` of `L` bytes at `d` into `T`, before
  the rename (the sealing pass of model §5). For `uuid` and `0xHEX` no
  leaves are computed.
- **I-74 [M1] The cost** (E23: 1 GiB of generated bytes, 1 MiB leaves,
  load 34; ids equal across thread counts):

  | function | 1 thread | 4 threads | 8 threads |
  |---|---|---|---|
  | `L128` tree (bare, `fast`) | 85 ms (12.6 GB/s) | 28 ms | 17 ms |
  | SHA-256 tree (`sha256`) | 5.51 s (195 MB/s) | 1.41 s | 0.85 s |
  | SHA-1 tree (`sha1`), from E12's 2 GB run | ≈ 4 s | — | ≈ 0.55 s |

  Re-measured on `mortar_digest` itself (WP-026h,
  `examples/digest_bench.rs`; 1 GiB of splitmix64 bytes, 1 MiB leaves,
  AArch64 host, load 46; best of 3 for `L128`, one run otherwise; ids
  equal across thread counts). `L128` agrees with E23; both SHA trees are
  faster than E23's straightforward loops (the 16-word rolling schedule
  and rotating variable roles of WP-026h), SHA-1 about 3×:

  | function | 1 thread | 2 | 4 | 8 |
  |---|---|---|---|---|
  | `L128` tree | 82 ms (13.1 GB/s) | 43 ms | 24 ms | 19 ms |
  | SHA-1 tree | 1.39 s (775 MB/s) | 0.72 s | 0.49 s | 0.29 s |
  | SHA-256 tree | 3.74 s (287 MB/s) | 1.99 s | 0.99 s | 0.55 s |

  So `sha1` costs about 17×, and `sha256` about 45×, the bare id's time
  on one thread; the bare id remains the only one cheap enough to be the
  default.

  The review measured mold's id at 107 ms and lld's at about 95 ms on a
  1 GB output (`fast` in lld: about 0); not re-run here. In E18's
  `cxx-medium-g` link (48 MB), mold spends 7–14 ms in `write_build_id`
  (load 39–41), and dropping `--build-id` moved neither linker's wall
  time beyond run-to-run noise (lld 133–160 against 145–176 ms, mold
  93–100 against 95–96 ms; load 32–40). A whole-file SHA-1 at 260–279
  MB/s per thread would take 3.6–3.9 s at 1 GB (1.3 s at `mortar_digest`'s
  775 MB/s), longer than every other phase. The tree's leaves are hashed by the workers that already hold
  the window in cache, so the bare id costs about 1–2% of a 1 GB write.
  No tool recomputes any linker's id (§8.1), so the tree costs no
  compatibility.
- **I-75 [M1] [C]** The function, `B` included, is defined per mortar
  version. `B` equals `W` so that a window is a leaf. It is not frozen:
  a later version may change `W`, `L128` or the root, and must then
  change the tag. Two mortar versions may give different ids for the
  same output. `mortar_plumb` checks an id with the definition of the
  mortar version that wrote it (it ships in the same release; request
  R-8).
- **I-76 [M7]** An incremental link keeps `leaves` in the link state
  (16 bytes per MiB of output for the bare id), rehashes the windows it
  refills, and recomputes the root: its cost is proportional to the
  refilled bytes, plus `m` digests. A change of `n` changes the root
  through `le64(n)`.

### 8.4 Vectors (WP-026h)

`pattern(n)` is the `n` bytes whose byte `i` is `i mod 251`. `B =
2^20`. Every value below was computed twice, by `mortar_digest` and by a
separate implementation written from the text of I-72 and I-114 alone
(Python, its `hashlib` for SHA-1 and SHA-256); they agree. They are
checked by `crates/mortar_digest/tests/vectors.rs`.

`L128` (I-114), as the 16 bytes in hexadecimal:

| input | `L128` |
|---|---|
| empty | `166e5d750a9987ba700b8ae8169c08f1` |
| `abc` | `34d9933929a69d420194f31701b62a6e` |
| `pattern(1)` | `a9b2c13446ed2ae78d9276aa7e6a453c` |
| `pattern(7)` | `d8ba3acb3828bf91099bcd2dc39f4672` |
| `pattern(8)` | `8b8a2b39f5629b5caaa295deca45fa41` |
| `pattern(31)` | `30de9b5bf64bf64bc7885eaab7d8ddd5` |
| `pattern(32)` | `cf8b6fc0c63e046e161c68c0466a00e5` |
| `pattern(33)` | `70094889eb36eb1e8e4f9a079d66511b` |
| `pattern(63)` | `5bf246f31e37031982482cf3c6f97f75` |
| `pattern(64)` | `53d570ba718049ac4882965680303b32` |
| `pattern(65)` | `b15eb71a90c7abc48e43144b543f840e` |
| `pattern(4096)` | `f570b658e524275c4140b619d155cab9` |
| `pattern(B − 1)` | `cb9e12add796f4ed147b7108555b66f7` |
| `pattern(B)` (1 MiB) | `c6e1dfc9cc045a72cf3a72032a298d32` |
| `pattern(B + 1)` | `1234f10fe0f80d6c59c2724939331490` |

The roots of I-72 over `F = pattern(n)` (the id is the first `L` bytes:
20 for bare, 8 for `fast`, all of it for `sha1` and `sha256`); the last
row zeroes `[B − 6, B + 14)`, a descriptor across a leaf boundary:

| `n` | `L128` tree (SHA-256 root) | SHA-1 tree | SHA-256 tree |
|---|---|---|---|
| 0 | `598ec1d4dd3c72cee90920b43faf73a539ad84fd3eb148f8b1c5f4362be50bcb` | `e17559e60537aa8405a216c63d1680f54fe53601` | `598ec1d4dd3c72cee90920b43faf73a539ad84fd3eb148f8b1c5f4362be50bcb` |
| 1 | `2df0b93caf760623c40f3781b8e01397e19f7230094ebf68d67a4fa922ca5082` | `f78bec69d0b07408559e32b98a419a7b111caed9` | `5b94dbbfe23e7efbfd1ccc5a410d926b54a06d828aa57ba9efa5cc4de7e0ed62` |
| `B − 1` | `8cea389497e938a07c9945e7e8e7e43f0c6098838f993d330e0e963b2fced212` | `add0d032914772ae22c9296d55dfd93ae2297cb2` | `c6b45677bcf57dffff98e8adebf7554fbd34e1ea80259f0a1436cb3e289839d0` |
| `B` | `059fe2c890954621d038b090876901341dcdaf92703e0a0bd27fef29ad98147f` | `0e2d57f84202376f665ed443e840c6e47b5c276e` | `99a6c7fa4d6cfed58f895c66aab64909f814a11ccc9406bfbe21ba0b5a3352e8` |
| `B + 1` | `be75a9986f7fe460609c23a5432a777d0c5c9106721d52b09522cfd3a5162b09` | `9c9df507ac24501067432862ecf325e57b4aa0a4` | `2b2777fb577eeeed88d735f2577239e248be71cbf97b4170d1ccf34a91aad675` |
| `3B + 5` | `4570d21375cf658f56f2d1c4f6c81b52e44c63ddc657cb18604b04470513b883` | `0b5813d4604084d007aefb48b83fa4530f15931e` | `a5d24a056a18d9f63ec9f63f3aa6589b8abcd847a7167b101d12069d30d28e9d` |
| `3B + 5`, zeroed | `31ff17b9820dd9f21410ccd27c9bcd3993f325613f8c070cad73c613c8faed25` | `756ee545d2b48c55d0a776066dcaf96c8c02a8c6` | `80b19e7d7a46828ca58fe5e4f7368db79c755c9e90207f9d34b752db211a860e` |

`uuid` (M2): `uuid_v4(16 × 0xff) = ffffffffffff4fffbfffffffffffffff`,
`uuid_v4(16 × 0x00) = 00000000000040008000000000000000` (byte 6's high
nibble is 4, byte 8's two high bits are `10`, every other bit is the
random input's).

## 9. The map file

### 9.1 What exists

- `-Map=FILE` (or `-Map FILE`) and `-M` (`--print-map`) are GNU ld's;
  the text formats of GNU ld, lld and mold differ (`spec/plumb.md` §3.4
  parses all three). None of the bench workloads asks for a map (E1). No
  build tool on the host parses one: the only one installed is CMake
  3.31, which passes map options only for TI and ARM embedded compilers'
  linkers (E8).

### 9.2 mortar's map

- **I-77 [M1] [C]** `--mortar-map=FILE` writes `spec/plumb.md` §3.2's
  `mortar-map 1` format to `FILE` after a successful link, for the
  verifier and tools. `-Map` and `-M` write GNU's text instead (§9.3,
  I-120; WP-027e replaced Q5's "`-Map` is `mortar-map 1`"). A failed link
  writes neither and prints nothing on standard output (`spec/diag.md`
  §7). `--cref` is `refused`.
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
  - non-alloc input sections (R-9, applied by WP-028b; written by
    WP-027b): `nplace` per non-alloc input section placed whole, with
    its output section's name and its offset from that section's start;
    `merged` per `SHF_MERGE` non-alloc input section (its pieces are not
    listed).
  - Within a file, `place` and `piece` records are in address order
    (pieces of one fragment, such as the NUL padding of a string
    section, share an address; ties by section, then input offset).
  - `.eh_frame`'s pieces are its kept records (S-49), each at its output
    address with its output size.
  - `thunk` and `patch` chunks: one record per `Placement.patches` row,
    named as `.symtab` names it (S-80), a thunk with its target's name
    (`@plt` appended for a PLT target) and addend (WP-027c).
  - Not yet written: `fold` (M2). `common` is not a
    chunk of its own in M1 (commons are placed in `.bss`, layout §8).
- **I-79 [M1]** The map is a function of the tables and the file names,
  written after layout and independent of fill. Each record group is
  formatted in parallel per file (`map_collect`, rule 3) and the buffers
  are concatenated in `spec/plumb.md`'s group order. Its size is about 40
  bytes per placed section and per alloc piece. It is written with the
  same writer as the output (temporary file, rename, §7), only for a
  successful link; a failed link removes nothing at `FILE` (not
  compared with the references, E14).
- **I-79a [M1] [C] Streams (WP-087).** The files written whole (the
  maps, the dependency file: `mortar_image::output::write_whole`) are
  written in place, without a temporary name, when `FILE` is, following
  symbolic links, a character device or a FIFO (`-Map=/dev/stdout` on a
  terminal or a pipe, `-Map=/dev/null`), or a descriptor alias of a
  regular file (I-58a, truncated): they are written in order, so a FIFO
  is no obstacle. A FIFO with no reader blocks the link until one opens
  it, as it does for every program. Every written path is first checked
  against the files the link read and the other written paths
  (`spec/cli.md` §3.14).

### 9.3 The GNU-style map (WP-027d specified, WP-027e implemented)

- **I-120 [M1]** `-Map=FILE`, `-Map FILE` and `-M` (`--print-map`, to
  standard output) write a human-readable map in GNU ld's text layout;
  `--mortar-map=FILE` writes `mortar-map 1` (I-77, I-78) for the
  verifier and tools. Every consumer of the machine map uses
  `--mortar-map`: the pipeline and options tests
  (`crates/mortar/tests/pipeline.rs` `mortar()`, `x64.rs`,
  `debuggers.rs` `map_option()`) and `cargo xtask linktest` (its L3
  verifier run adds `--mortar-map` to every mortar link, whatever `-Map`
  the test gives). Both may be given; each is written as I-79 says.
- **I-121 [M1] The text.** The headings and record shapes of GNU ld
  2.44 (checked against its output for a static C link with libc, and a
  C++ link with COMDAT groups), so that people and the scripts that
  grep GNU maps read it; not GNU's content (addresses, order, fill and
  which COMDAT copy is kept are mortar's). In order:
  1. `Archive member included to satisfy reference by file (symbol)`, a
     blank line, and per taken member in `FileId` order
     `ARCHIVE(MEMBER)`; then, at column 30 (on the next line after 30
     spaces when the member text is 30 columns or longer, else padded
     to column 30 on the same line), `FILE (SYMBOL)`: the file and
     symbol whose reference took it (resolve's first taker,
     `spec/resolve.md` §4.3, which is not always GNU's), for a root
     (`-u`, the entry) no file, so the line is 30 spaces and
     `(SYMBOL)`, as GNU writes it; a blank line;
  2. `Discarded input sections`, a blank line, per dropped section of a
     live object (COMDAT copy, `/DISCARD/`, excluded, gc'd; zero-sized
     ones included, as GNU does) ` NAME`, address 0, size and file,
     in the column layout of item 4; a blank line. Sections the link
     consumes (`.note.GNU-stack`, `.note.gnu.property`, `.group`, layout
     §8.1) are not listed (GNU lists them: its script discards them);
  3. `Memory Configuration`, a blank line,
     `Name             Origin             Length             Attributes`,
     `*default*        0x0000000000000000 0xffffffffffffffff` (8 digits
     each for ELFCLASS32), a blank line;
  4. `Linker script and memory map`, a blank line, `LOAD PATH` per input
     file in `FileId` order (an archive once, at its first member), then
     per output section in section-header order a blank line, `NAME`
     padded to column 16 (on a line of its own and the columns on the
     next line after 16 spaces when the name is 15 columns or longer),
     the address `0x%016x` (`0x%08x` for ELFCLASS32; 0 for a non-alloc
     section, whose lines then hold offsets in it), a space and the size
     `0x%x` right-aligned to 10. Inside it, in address order: per placed
     input section ` NAME` with the same wrapping rule (counting the
     leading space), address, size and ` FILE` or ` ARCHIVE(MEMBER)`;
     after it, per global symbol defined there, 16 spaces, address, 16
     spaces, name (GNU's default: locals are its `--print-map-locals`,
     not written); ` *fill*` with address and size for each gap in an
     allocated section, and in an executable one the fill pattern of
     §4 in hex (`00000000` on AArch64, `cc` on x86; GNU writes its NOP
     word). A chunk mortar makes is one input line of file `mortar`
     (`spec/synth.md` S-3 names: `.got`, `.eh_frame`, `.symtab`, ...),
     and so are the merged bytes of an output section with mergeable
     inputs (GNU lists each input section); a symbol in those bytes is
     listed under that line. An island of thunks and erratum patches is
     an input line of file `linker stubs` with a symbol line per patch,
     named as `.symtab` names it (S-80, `__TARGET_veneer`);
  5. `OUTPUT(FILE ELF-TARGET)`, the output as the command line named it
     and GNU's BFD target name (`elf64-littleaarch64`, `elf64-x86-64`,
     `elf32-i386`), then `LOAD linker stubs` (GNU's last line).
  GNU's script lines (` *(.text.unlikely ...)`, `PROVIDE (...)`,
  assignments) are not written: mortar has no script for default
  placement, and the linker-defined symbols they show are not listed.
  `--cref` stays refused. The test is
  `crates/mortar/tests/pipeline.rs`
  `the_text_map_matches_the_output_and_gnus_map`: one parser reads
  mortar's and GNU's maps of one line; every section line is the
  section header, every input line lies in its section, every symbol
  line is its `.symtab` value, and the (section, size, file) sets of
  placed input sections and the taken members are GNU's, but for what
  item 4 says mortar makes itself and the kept COMDAT copy.
- **I-122 [M1] Determinism and cost.** The text is a function of the
  same tables as `mortar-map 1` (I-79) and the output's name, and of
  nothing else; it is formatted per output section in parallel and is
  written only when asked. The determinism matrix
  (`the_output_is_a_function_of_the_inputs_alone`) covers it like the
  machine map. Its cost on protoc (static, `-g`) is in
  `spec/landed/pipeline/WP-027e.md`.

## 10. I/O: the measured design

### 10.1 Candidates and results (E3)

Throwaway programs wrote `S` MiB from 8 workers in 1 MiB blocks into a
renamed temporary file: `vec` (one whole-image buffer, one `write_all`),
`vecpar` (the same, parallel `write_all_at`), `chunk` (a 1 MiB buffer
per worker, `write_all_at`; `+setlen` after `set_len`), `stream`
(`chunk` filled by positioned reads: the staged store's path), `cfr`
(`copy_file_range` through `std::io::copy`), and, in C, `mmap`, `mmapf`
(after `posix_fallocate`) and `mmapin` (source mapped too).
Total seconds, median of 3, load 28–36:

| file system, MiB | vec | vecpar | chunk | +setlen | stream | cfr | mmap | mmapf | mmapin |
|---|---|---|---|---|---|---|---|---|---|
| `/tmp`, 200 | .123 | .132 | .085 | .090 | .085 | .085 | .085 | .073 | .094 |
| `/tmp`, 2048 | 1.597 | 1.864 | 1.159 | 1.134 | 1.225 | 1.529 | 1.148 | .855 | 1.072 |
| share, 200 | 1.63 | 1.58 | 1.53 | 1.70 | 1.49 | .33 | 4.09 | 1.76 | 1.91 |
| share, 2048 | 16.1 | 17.2 | 16.6 | 16.1 | 14.9 | 4.07 | 8.5 | 6.15 | 11.5 |

Memory growth (`VmHWM`): `chunk` and `stream` about 7 MiB; `vec`,
`vecpar` and `mmap` the output's size; `mmapin` twice; `cfr` nothing.
On the share, dirty mapped pages count as resident (599–1454 MiB for
`mmap` at 2 GB). Block sizes from 64 KiB to 4 MiB gave the same times.

### 10.2 What the numbers say

- **Writing one file scales to 2–4 threads and no further.** 2 GiB with
  `chunk` on `/tmp` (E21, load 26–27): 0.80 s on 1 thread, 0.51 on 2,
  0.59 on 4, 0.64 on 8, 0.80 on 16; the peak is 4.2 GB/s at 2 threads.
  E3's `mmap` runs gave the same shape. The likeliest cause is the
  per-inode lock on buffered writes; it was not traced (no `perf`).
  **mortar promises no speed-up of the write beyond 4 threads** (P5).
  Threads beyond that pay for filling, relocating and hashing, which
  scale with the work.
- **Safe positioned writes cost nothing against `mmap`** on `/tmp`:
  `chunk` / `mmap` = 1.00 at 200 MB and 1.01 at 2 GB, with 7 MiB instead
  of the output's size. Against `mmap` with `fallocate` (which std does
  not offer), 1.16× at 200 MB and 1.36× at 2 GB. On the share, `mmap` is
  erratic (2.7× slower at 200 MB, 1.95× faster at 2 GB).
- **The whole-image buffer is worse on every count**: `vec` / `chunk` =
  1.45 at 200 MB and 1.38 at 2 GB, and the output's size in memory.
- **`copy_file_range`** is 3.7–5.2× faster on the share and no faster on
  `/tmp`. It copies only bytes the process never sees: no relocation and
  no hashing. With `--build-id` on every link and debug sections dense
  with relocations (I-50), it hardly applies. Recorded for the
  performance work (Q10).
- **The budget** (E18: `cxx-medium-g`, 201 MB of inputs and a 48 MB
  output, all on `/tmp`; 3 runs; load 32–40): mold 93–100 ms on 8
  threads (108–135 on 4, 298–305 on 1), lld 22 133–160 ms (155–189 on
  4, 251–274 on 1), GNU ld 797 ms. mold's `--perf` attributes 35–45% of
  its time to `copy` (33–49 ms of 71–167 ms), 7–14 ms to the build id
  and 12–31 ms to reading inputs (load 39–41). So the image phase is
  about 37% of the time mortar must beat. Writing 48 MB at 2–4 GB/s is
  12–24 ms of it, the rest is copying and relocating. Peak RSS: mold
  323 MB, lld 325 MB, GNU ld 212 MB. The draft's figures (lld 0.72 s,
  mold 0.37 s) had inputs on the workspace share and overstated the
  budget about fourfold.
- **Replacing an existing output** costs the old inode's teardown inside
  `rename`: 36 ms at 220 MB and 0.73 s at 2 GB on `/tmp`, which I-113
  moves off the critical path (E20).

### 10.3 The design

- **I-80 [M1] [C]** Fill by windows into per-worker buffers, written with
  positioned writes (`chunk`), as §5.3 and §7.3 specify. No mapping, no
  whole-image buffer, no `copy_file_range`.
- **I-81 [M1] [C] The staged store is primary.** `spec/input.md` §11.1
  (b): large section bodies are never resident; fill reads each clip
  with `copy_to` (`read_exact_at`) straight into the window buffer. The
  `stream` measurement is this path: 1.06× `chunk` at 2 GB and equal at
  200 MB. Reading 201 MB from the page cache takes 54 ms on 1 thread,
  16.8 ms on 4 and 12.7 ms on 8 (15.8 GB/s; load 27). The resident store
  (every input read whole) remains: it is M1's first pipeline and the
  fallback for inputs that cannot be re-read (a pipe, standard input).
  WP-021b implements the staged store.
- **I-82 [M1] [C] Relocations.** What the store keeps resident for
  `cxx-medium-g` (E19, 202 MB of inputs):

  | part | MB | share |
  |---|---|---|
  | relocations, alloc targets | 6.87 | 3.4% |
  | relocations, non-alloc targets (debug) | 45.93 | 22.7% |
  | symbol tables and string tables | 15.54 | 7.7% |
  | headers, section headers, `.shstrtab` | 15.20 | 7.5% |
  | archive indexes | 7.16 | 3.5% |
  | small (≤ 64 KiB) alloc / non-alloc bodies | 7.63 / 6.86 | 7.2% |
  | groups | 0.56 | 0.3% |
  | large alloc bodies / large non-alloc bodies | 0.73 / 95.5 | 47.6% |

  Relocations of alloc sections are resident: liveness, scan and fill
  all read them, and they are small. Relocations of non-alloc sections
  are read twice: once by the scan in `SCAN_BLOCK` blocks (I-13), once by
  fill for the records of its clip (I-35). The second read is 45.9 MB:
  about 2.9 ms of wall time on 8 threads and 12 ms of CPU at the
  measured rate, against 45.9 MB of resident memory (23% of the input).
  With them resident, the staged store keeps 105.8 MB (52% of the
  input); read twice, 59.9 MB (29.7%). Read twice is chosen. The largest
  relocation section is 1.36 MB, so a whole section never needs a buffer
  larger than `SCAN_BLOCK`'s.
- **I-83 [M1]** The window plan gives each worker a contiguous output
  range; with the staged store, a window's reads come from a few input
  files at increasing offsets, which readahead serves. Cold-cache
  behaviour was not measured.
- **I-84 [perf]** Mapping inputs or the output is not part of the
  design. It would have to beat parity with `mmap`, and 1.16–1.36×
  against `mmap` with `fallocate`, on the local file system. A request
  for `unsafe` (`docs/ARCHITECTURE.md` principle 4) would need a
  link-level measurement of that difference, with `spec/input.md`
  §11.1's `SIGBUS` handling.
- **I-115 [M1]** The bench harness writes outputs under
  `.build/bench-work/scratch`, on the workspace share
  (`spec/harness.md`). Writing 200 MB there takes 18× as long as on
  `/tmp`, and the references' link times rise 1.4–2.9×. Request R-10
  asks the harness to write scratch outputs to a local file system and
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

- **I-92 [M1] [C]** mortar's peak resident memory is bounded by

  ```text
  staged store:   peak ≤ R_s + D + T + P + threads × (W + S) + X + E
  resident store: peak ≤ R_r + D + T + P + threads × W + X + E
  ```

  - `R_r`: every input byte (the resident store reads each file whole);
  - `R_s`: the staged store's resident parts: headers, section headers,
    `.shstrtab`, symbol and string tables, groups, archive indexes,
    bodies of at most 64 KiB, and the relocations of alloc sections
    (I-82; `spec/input.md` §11.1 (b) as R-13 amends it);
  - `D`: decompressed `SHF_COMPRESSED` inputs (the store's buffers);
  - `T`: the tables of model §3–§4 at the row sizes WP-020's landed record
    states, and the phase caches (`sym_addr`, 8 B per global symbol; the
    fill plan, 24 B per placed section; the record index, 4 B per record
    of an unsorted section plus 8 B per `REC_SAMPLE` records of a sorted
    non-resident one (I-29); build-id leaves, 16 B per MiB of output);
  - `P`: synthetic chunks' non-divisible working sets (§13);
  - `W`: fill's window buffer (I-34); `S` = `SCAN_BLOCK` = 1.5 MiB, the
    staged store's relocation buffer, used by the scan and by fill;
  - `X`: the buffers of whole chunks in flight (§13): at most `threads`
    of the largest declared whole chunk larger than `W`;
  - `E`: the event accumulators (§4.5), only in links that report them.

  There is no term in the output's size: the image is never resident.
- **I-93 [M1]** For `cxx-medium-g` (E19), `R_r` = 202 MB and `R_s` =
  59.9 MB. With 8 threads the window and relocation buffers add 20 MiB.
  The references' peaks for this link (E18): mold 323 MB, lld 325 MB, GNU
  ld 212 MB. The bench gate (§16.5) checks mortar's measured peak against
  the formula's value, with `T` from the landed row sizes plus a slack
  of 32 MiB for allocator overhead and stacks, and reports the ratio to
  lld's and mold's peaks.
- **I-94** Merged into I-92, which now gives both stores.

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
    /// `Divisible` (any range, I-104's bound) or `Whole { max_size }` (one job, one buffer).
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
- **I-104 [C]** Every chunk declares its `Division`, one of two kinds:
  - **Divisible**: `write` of any range costs `O(range + log chunk +
    2^10)` and allocates nothing proportional to the chunk. `.symtab`
    (fixed records) is so trivially. `.strtab` keeps, per input file, the
    prefix sum of its names' sizes in the output, and the output offset
    of every 1024th symbol's name: a range start is found by binary
    search over files and then over samples, and then by walking at most
    1024 symbols. (The draft walked from the file's first symbol, which
    is `O(file's symbols)` per window and violated the bound it stated.)
    In `cxx-medium-g`'s lld output, `.symtab` is 1.26 MB and `.strtab`
    3.58 MB (E4).
  - **Whole**: one job writes the whole chunk into a buffer of the
    chunk's size, then writes the windows it covers. Allowed only for the
    kinds `spec/synth.md` declares whole, each with a size bound as a
    function of the tables (for example 8 bytes per live FDE for
    `.eh_frame_hdr`). The buffers in flight are I-92's `X`. A whole chunk
    of at most `W` bytes costs nothing extra: it fits one window's job.
  Request R-12 asks synth for the list.
- **I-105** `write` of a divisible chunk allocates nothing proportional
  to the chunk. A chunk whose content needs a global order (`.gnu.hash`'s
  buckets, a sorted table) builds it once, in a prelude of phase 100 that
  synth owns, with the parallel sort of rule 4, and states its memory in
  I-92's `P`.
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
| `undefined-symbol` | `(70, first reference)` | scan driver, from events; also from non-alloc sections | I-20–I-22, I-110 |
| `comdat-mismatch` | `(70, first reference)` | scan driver | same |
| `discarded-section-reference` | `(70, first reference)` | scan driver | same |
| `gnu-warning` (per key) | `(70, first reference)` | scan driver | same |
| `relocation-incompatible` | `(70, FileId, section × 2^32 + record)` | `arch.scan` | I-17 |
| `relocation-out-of-range`, `relocation-misaligned` | `(100, FileId, section × 2^32 + record)`; `(100, 0, ChunkId)` | fill, synth | I-36, I-43 |
| `internal-error` | none | fill | I-27, I-44 |
| `cannot-open-output` | `(105, 0, 0)` | output file (a directory, FIFO, socket or block device; `T` not creatable) | I-57–I-59 |
| `cannot-write-output` (new, R-7) | `(105, 0, 0)` | output file | I-58, I-60, I-64 |
| `invalid-option-value` | `(10, argument, 0)` | command line | I-70, I-85 |
| `refused` | `(10, argument, 0)` | command line (`md5`; `uuid` until M2) | I-5, I-51, I-53, I-69 |

## 15. Where the references differ

| subject | GNU ld 2.44 | lld 22 | mold 2.37.1 | mortar | rule |
|---|---|---|---|---|---|
| debug reference into a dropped COMDAT copy, ELFCLASS64 (`.debug_info`, `line`, `frame`) | the kept copy's address + addend when the sizes are equal, else 0 | 0 | 0 | all ones | I-47 |
| the same in `.debug_rnglists`, `.debug_loclists`, `.debug_addr` | redirect or 0 | 0 | 0 | 0 | I-47 |
| the same, ELFCLASS32 | redirect or 0 | 0 | 0 | 0 (`.debug_frame`: all ones) | I-47 |
| the same in `.debug_aranges` | redirect or 0 | 0 | 0 | all ones, length 0 | I-47, I-112 |
| the same in `.debug_ranges`/`.debug_loc` | redirect or 0 (GC case: 0 in `.debug_loc`, `spec/a64.md` §13.2) | 1 | 1 | 1 | I-47 |
| `SHF_LINK_ORDER` non-alloc section (`.stack_sizes`) of a dead section, `--gc-sections` | removed | removed | kept | removed (M2) | I-109 |
| undefined symbol referenced only from non-alloc sections | error | accepted | error | error | I-110 |
| `referenced by` lines of a grouped undefined symbol (the review's order c, a, b) | one line per reference | c, b, b | c, a, a | first three in `RefId` order | I-22 |
| output is a FIFO | `Illegal seek` | written | written | refused by name | I-58 |
| litter after `SIGINT` (the review) | a 4 KiB partial output | none | `.out.<pid>` | `T0`, reused by the next link | I-59, I-67 |
| teardown of a replaced output | in `rename`, on the critical path | hidden | hidden | on a helper thread | I-113 |
| existing output after a failed link | removed | kept | kept | removed, unless it is an input | I-66 |
| read-only directory, writable output | written in place | error | error (names its temporary) | error naming the output | I-59 |
| output is an input | refused (by name or symbolic link; a hard link passes) | accepted, input replaced | written in place: an object fails with the input destroyed, an archive or script is replaced | refused by name, any spelling (accepted before WP-087) | I-62 |
| `-Map` or `--dependency-file` names an input (WP-087) | overwritten (an archive: truncated before it is read, the link fails) | overwritten | `SIGBUS`, overwritten | refused by name | `spec/cli.md` §3.14 |
| `-Map` names the output | the output overwritten by the map | the same | the same | refused by name | `spec/cli.md` §3.14 |
| `-Map=/dev/stdout` redirected to a file | written | written | written | written in place | I-58a, I-79a |
| bare `--build-id` | 20 (hash not reproducible) | 20 | 20 | 20, the `L128` tree | I-69 |
| `--build-id=fast` | warning, no note | 8 bytes | 32 bytes | 8 bytes (the bare id's prefix) | I-69 |
| `--build-id=sha256` | warning, no note | error | 32 bytes | 32 bytes | I-69 |
| `--build-id=md5` | 16 bytes | 16 bytes | 16 bytes | refused | I-69 |
| `--build-id=uuid` | 16 random bytes | 16 random bytes | 16, RFC 4122 v4 bits | 16, v4 bits (M2) | I-69 |
| `--build-id=0xabc` (odd) | warning, no note | `ab 0c` | error | error | I-70 |
| `--build-id=0x` | warning, no note | empty descriptor | error | error | I-70 |
| build-id note size for a 2-byte id | 18 (unpadded) | 18 | 18 | 20 (padded to 4) | I-71 |
| what the hash covers | not the whole file, not a 64 KiB/1 MiB/4 MiB tree (E12) | same | same | a public tree over 1 MiB blocks | I-72 |
| `-Map` / `-M` format | GNU text | lld's own text | mold's own text | GNU's layout (`mortar-map 1` is `--mortar-map`) | I-77, I-121 |
| `--gc-sections` in M1 | supported | supported | supported | supported since WP-031 | I-5 |
| COMDAT group with one live member, `--gc-sections` | whole group kept | whole group kept | members one by one | whole group kept | I-7 |
| non-alloc sections of an object with no marked section; non-alloc non-debug with relocations | removed | kept | kept | kept | I-6 |
| note in a dead group with an alloc member | removed | removed | kept | removed | I-6 |
| personality reached only from the CIE of collected FDEs | removed | kept | kept | removed | I-7 |
| collected function's `.gcc_except_table` | removed | kept | removed | removed | I-7 |
| unreferenced pieces of a live string section, `--gc-sections` | kept (whole section) | removed | removed | removed | I-7 |
| `__start_`/`__stop_` default | `nostart-stop-gc` | `start-stop-gc` | every C-identifier section kept | `nostart-stop-gc` | I-6 |
| `--print-gc-sections` | stderr, `'sec' in file 'f'`, no size 0 | stdout, `f:(sec)` | stdout, `f:(sec)`, unordered | GNU's text and order | I-10 |
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
- `unsorted_records_index`: a section larger than `W` with records out
  of `r_offset` order (as in E22's x86-64 `libc.a` members) gives the
  same bytes as the sorted equivalent; the scan marks it `Permuted`, a
  sorted one `Sorted` without an extra pass (I-29).
- `window_boundary_sequences`: the AArch64 GD→LE sequence placed so that
  a window boundary falls at `o+4`, `o+8` and `o+12`, and the x86-64
  `TLSGD` form with a boundary at each byte of `[P − 4, P + 12)`: the
  bytes equal an unclipped fill (I-35, I-111); the same for the 22-byte
  large-model `TLSGD` and `TLSLD` forms at each byte of `[P − 3, P + 19)`
  (`spec/x64.md` §8.6).
- `max_reach_derived`: `Facts.max_reach` equals the maximum over the
  tables, and no `apply` writes outside its record's window (I-111).
- `gap_patterns`: trap, no-op and zero gaps per I-41, positioned by
  address for odd starts.
- `tombstone_by_name_width_class`: I-45–I-48 for each row of I-47's
  table, both classes, each allowed type (`ABS64`, `ABS32`, `ABS16`,
  `PREL32`, `TLS_DTPREL64`); I-112's length write, and its absence when
  a record has its field at the length.
- `events_merge_like_sequential`: random event multisets (up to 10^5
  events, 1–50 groups, 1–200 units), merged by I-20/I-21 under 50
  scheduling seeds and thread counts 1, 2, 7 and 16, equal the sequential
  definition of §4.5 (first three, count, key).
- `build_id_vectors`: SHA-1 and SHA-256 FIPS 180-4 vectors; `L128`
  vectors and the trees of I-72 of §8.4 (landed in `mortar_digest`'s
  `tests/vectors.rs`, WP-026h); the image's own test checks that a
  linked output's id equals `mortar_digest::tree_root` over the file with
  the descriptor zeroed; `fast` equal to the bare id's prefix; the
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
  mode).
- The output file (I-58, I-59, I-113): a FIFO and a socket at `P`
  refused by name; a stale `T0` with no lock holder is reused; a `T0`
  locked by a second process makes the link use `T0.<pid>` and leaves
  `T0` alone; on a file system whose `flock` is not enforced (simulated by
  a test hook), only `create_new` names are used; an existing output is
  set aside and gone after the link; a hard link to it keeps the old
  bytes; a symbolic link is replaced and its target untouched; an output
  that is an input is not set aside; a failed link leaves neither `P`
  nor `A` nor `T`.
- `undefined-symbol` referenced only from `.debug_info`: an error
  (I-110).
- `--gc-sections` (M2): a `.stack_sizes` section linked to a dead
  `.text` section is removed, and one linked to a live one kept (I-109).
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

E16's and E17's matrix as a test: two objects with one COMDAT inline
function, the second copy dropped, compiled by GCC 14 and Clang 19,
DWARF 4 and 5, at `-O0` and `-O2 -fno-asynchronous-unwind-tables`,
linked in the default layout and the low layout (`-pie -z
max-page-size=4096 -z noseparate-code`), for AArch64 and i386 (under
`qemu-i386` for gdb):

- gdb's transcript of the probe script (`break`, `bt`, `info line`,
  `list`) equals GNU ld's after addresses are masked, and has one
  location per breakpoint;
- lldb's probe (`breakpoint set`, `thread backtrace`, `image lookup`)
  gives every breakpoint a file and line and no `unable to resolve`
  line, in every configuration but i386 Clang DWARF 5 in the low layout
  (Q11), which is recorded and not gated;
- `llvm-dwarfdump --verify` errors are reported, not gated;
- the tombstone bytes are those of I-47 and I-112 (read by E16's script);
- a separate debug file is found through `.build-id/xx/rest.debug` by
  gdb, lldb and `eu-unstrip`, for bare, `sha1`, `sha256` and `fast` ids.

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
  recomputes the build id of the same mortar version; every `-Map` file round-trips through
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

*Applied by WP-023b (`reference(record, EventClass, SymKey)`: the driver forms the group; `spec/landed/a64/WP-023b.md`).*

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

*Applied by WP-023b (`max_reach: Window`; `spec/landed/a64/WP-023b.md`).*

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
   `after` of any write window of the architecture, over every `Howto`
   window (§3.2) and every member window of a recognized form
   (`unit_window`, §3.5). `mortar_arch` computes it from those tables;
   it is never written by hand. AArch64 gives at least `(0, 4)` (the
   GD→LE `CALL26` member, `spec/a64.md` §8.4); x86-64 `(4, 8)` for the
   A.3 forms."

Reason: fill by windows (I-33) clips sections at window boundaries;
`apply` already never reads `out`, so the change costs nothing. I-111:
the draft's hand-written AArch64 `(0, 0)` was wrong.

### R-3 `spec/arch.md` §3.5 (sequence extent)

*Applied by WP-023b.*

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

*Applied by WP-023b (the image writes the `.debug_aranges` length, I-112).*

1. `spec/arch.md` §3.11. Current: "A relocation in a non-alloc section
   whose target is in a section that is not live writes
   `tombstone(name)` truncated to the field instead of `X`: 1 for
   `.debug_ranges` and `.debug_loc`, 0 for every other section, on all
   three targets (a64.md §13.2 for the experiment)." Proposed: "A
   relocation in a non-alloc section with a debug name (`spec/resolve.md`
   §6.3) whose target is in a section that is not live writes
   `tombstone(name, class)` truncated to the field instead of `X`
   (`spec/image.md` I-47): 1 for `.debug_ranges` and `.debug_loc`; all
   ones for `.debug_aranges` and `.debug_frame`; for every other debug
   name all ones in ELFCLASS64 and 0 in ELFCLASS32. In `.debug_aranges`
   the tuple's length is also written 0 (I-112). `.gcc_except_table` and
   other names: `spec/resolve.md` §6.3."
2. `spec/a64.md` §13.2, **Discarded targets**. Current: "…writes a
   tombstone instead of `X`, independent of `A`: 1 in `.debug_ranges`
   and `.debug_loc` (where 0 would end the list), 0 elsewhere." Proposed:
   "…writes a tombstone instead of `X`, independent of `A`: 1 in
   `.debug_ranges` and `.debug_loc` (where 0 would end the list and all
   ones would select a base address), all ones elsewhere, with the
   aranges length written 0 (`spec/image.md` I-47, I-112: a 0 is an
   address, and lldb attributes low code to the dead copy)." The
   experiment paragraph stays; append: "For dropped COMDAT copies
   (`spec/image.md` E9) GNU ld instead redirects to the kept copy when
   the sizes are equal." `spec/x64.md` (WP-029) takes the same text for
   x86-64 and the ELFCLASS32 column for i386.

Reason: E16, E17. lldb loses line information with 0 at low text
addresses and with an unbounded all-ones arange; gdb is unaffected;
ELFCLASS32 debuggers do not treat `0xffffffff` as a tombstone.

### R-6 `spec/cli.md` §4.2 (rows whose status the image changes)

Append to the list of rows the M1 and M2 packages change: "WP-026 stage 2
(`mortar_image`): `supported` in M1: `--no-gc-sections`, `--build-id`
(bare, `none`, `sha1`, `sha256`, `fast`, `0xHEX`; `md5` refused,
`option value`; `uuid` refused until M2), `--no-build-id`, `-Map`,
`-M`/`--print-map`, `--threads`, `--thread-count`, `--no-threads`,
`--time-phases` (its help loses `(reserved)`),
`--compress-debug-sections` with the value `none` (others refused,
`option value`, until M4); `noop(mortar never maps the output)`:
`--mmap-output-file` and `--no-mmap-output-file`. `--gc-sections`,
`--print-gc-sections`, `-s`, `-S` and `--cref` stay `refused` (M2)."
And in §2.3, the build-ID value kind: "`0x` followed by an even,
non-zero number of hexadecimal digits; anything else is
`invalid-option-value` (`spec/image.md` I-70)." Reason: I-5, I-51,
I-69, I-70, I-77, I-85, I-88.

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

*Applied by WP-028b (`plumb.md` §6.7, rule B1).*

Add a check: "When the map is mortar's (`mortar-map 1`) and the output
has an `NT_GNU_BUILD_ID` note of 8, 20 or 32 bytes, the verifier
recomputes `spec/image.md` I-72's function as defined by the same mortar
version (8 bytes: the `L128` tree's prefix; 20: the `L128` tree or the
SHA-1 tree, either matching; 32: the SHA-256 tree) and reports a
mismatch as a finding at the note's address. Other lengths (`0xHEX`,
`uuid`) are not checked." Reason: the id is the one sealed value no
other check covers; I-75 makes the definition per version.

### R-9 `spec/plumb.md` §3.2 (non-alloc records), M2

*Applied by WP-028b (`plumb.md` §3.2, §5.8; I-47 and I-112 judged as tombstones).*

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
system type of that directory with every result." Reason: I-115 (18×
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

### R-12 `spec/synth.md` (WP-025's draft)

1. S-91's table and text. Current: "| (none given, or `sha1`) | 20 |…",
   "| `md5` | 16 |…", "| `uuid` | 16 |…", and "The section's size is
   `16 + n` with no padding after the descriptor". Proposed: bare
   `--build-id` and `sha1` both 20 bytes, but different functions
   (`spec/image.md` I-69: the `L128` tree and the SHA-1 tree); `md5`
   refused, `option value`; `uuid` 16 bytes, M2; "The section's size is
   `16 + align_up(n, 4)`, zero-padded (`spec/image.md` I-71); the
   references write `16 + n`, which differs only for `0xHEX` of a length
   not a multiple of 4."
2. S-108's row. Current: "| `.note.gnu.build-id` | `16 + n` (S-91) |".
   Proposed: "| `.note.gnu.build-id` | `16 + align_up(n, 4)` (S-91) |".
3. S-92. Current: "`fast` is an 8-byte non-cryptographic hash (lld's
   size); its function is `image.md`'s." Proposed: append "It is the
   first 8 bytes of the bare id (`spec/image.md` I-72). `uuid` sets the
   RFC 4122 version 4 and variant bits, as mold does."
4. S-4. Current: "…`.eh_frame` by record range, `.symtab` and `.strtab`
   by file, `.got` by slot range, `.rela.plt` and `.plt` by entry range.
   The others are one unit each." Proposed: "Each chunk declares its
   `Division` (`spec/image.md` §13, I-104): `Divisible` chunks
   (`.eh_frame`, `.symtab`, `.strtab`, `.got`, `.rela.plt`, `.plt`, and
   any other whose `write` meets I-104's bound) are filled by range;
   `Whole` chunks are written by one job into a buffer of their size,
   and S-4 lists each with its size bound as a function of the tables.
   `.strtab`'s range start is found through per-file prefix sums and
   the name offset of every 1024th symbol."

Reason: I-69, I-71, I-72, I-104 (C1, C3, C4 of the review).

### R-13 `spec/input.md` §11.1 (b), §11.3, §11.7

1. §11.1 (b). Current: "**(b) `Staged`** (specified here for a later
   performance package; not M1). Each file's **resident parts** are read
   into owned buffers: … `SHT_GROUP` sections, relocation sections, and
   every other section of at most 64 KiB". Proposed: "**(b) `Staged`**
   (the primary store; WP-021b). Each file's **resident parts** are read
   into owned buffers: … `SHT_GROUP` sections, the relocation sections
   of `SHF_ALLOC` sections, and every other section of at most 64 KiB.
   Relocation sections of non-alloc sections are not resident: the scan
   and fill read them with `copy_to` (`spec/image.md` I-13, I-82)." And
   (a): "**(a) `Resident`** (M1's first pipeline, and the fallback for
   an input that cannot be re-read)."
2. §11.3, `copy_to`'s bullet. Append: "`copy_to` takes `&self` and may
   be called by every worker at once (positioned reads on a shared
   descriptor, `read_exact_at`)."
3. §11.7. Append: "With non-alloc relocations not resident, the staged
   store keeps 59.9 MB of `cxx-medium-g`'s 202 MB (29.7%), against
   105.8 MB (52%) with them (`spec/image.md` E19). The peak formula for
   both stores is `spec/image.md` I-92's."

Reason: P2 of the review; the measurement in I-82 (re-reading 45.9 MB
costs about 2.9 ms of wall time on 8 threads).

### R-14 `spec/x64.md` (WP-029)

*Answered by `spec/x64.md` X-25 and X-27: both 22-byte forms are
relaxed, with their windows (applied by WP-029r).*

The x86-64 large-model `TLSGD` sequence (`lea` with `R_X86_64_TLSGD`,
`movabs` with `R_X86_64_PLTOFF64`, `add`, `call`; 22 bytes,
`[P − 3, P + 19)`) is not one of `spec/arch.md` A.3's forms. Request:
either add it as a form, with its member windows (then `max_reach.after`
rises to 15, derived), or state that it is left unrelaxed (each member
relocated as its own type), with a test that links it and runs it. GNU ld
relaxes the 22 bytes; the review reports that lld 22 writes 16 bytes
from `P − 4` and corrupts it. Reason: I-111.

## 18. Open questions for the supervisor (most consequential first)

1. **Q11 — i386 Clang DWARF 5 in the low layout (I-47).** lldb loses
   line information there with every linker (`.debug_addr` holds 0 for
   the dead copy and claims `[0, size)`), and all ones makes gdb and
   lldb add a breakpoint location at `0xffffffff`. No tombstone fixes
   both. Options: accept (the default layout of i386 executables is
   unaffected), or pick all ones for `.debug_addr` only in ELFCLASS32
   and accept the extra location. Native i386 gdb was not run (only
   under `qemu-i386`).
2. **Q12 — `flock` on network file systems (I-59).** The probe decides
   at run time, so correctness does not depend on it, but on a file
   system where the probe fails a killed link's `T0.<pid>` stays forever.
   NFS and SMB were not available to test.
3. **Q9 — The jobserver** (later performance work). mortar uses
   `available_parallelism()` threads even under `make -jN`.
4. **Q10 — `copy_file_range` (§10.2)** (later performance work). It
   cannot be used for bytes the build id must hash.

Resolved by the review (2026-11-15):

- Q1: bare `--build-id` is the `L128` tree truncated to 20 bytes (I-69,
  I-72).
- Q2: the function is defined per mortar version, not frozen (I-75).
- Q3: the tombstone, as I-47 (the coordinator's `.debug_aranges` = 0 was
  replaced by all ones with the length 0, on E16's evidence).
- Q4: removing a stale output after a failed link (I-66), accepted.
- Q5: `-Map` writes `mortar-map 1` (I-77), accepted; in §15. Replaced
  by WP-027e: `-Map` is GNU's text, `--mortar-map` the machine map
  (I-120).
- Q6: `uuid` accepted in M2, excluded from the determinism gate (I-69).
- Q7: `fast` is 8 bytes; `sha256` accepted in M1 (I-69).
- Q8: `-o` equal to an input, accepted (I-62). Reversed by WP-087's
  review: refused (`written-file-clash`).
