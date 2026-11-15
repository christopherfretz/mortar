# spec: layout (`mortar_layout`)

Status: draft (WP-024 stage 1). The evidence for each non-obvious rule is
in `spec/landed/layout/WP-024-spec.md` ("E-n" below names an experiment
there).

`mortar_layout` decides where everything goes. It reads the derived
tables of `spec/model.md` §3 and writes `Assignment` (§3.4 there) and
`Placement` (§4.4 there). This spec says what those tables contain when
there is no `SECTIONS` script (`spec/script.md` owns the script case).

It answers six questions:

1. which output section each input section goes to, and in what order
   the inputs sit inside it (§8, §9);
2. the order of the output sections, the segments, and the addresses
   (§10 to §12). The rules any linker's output must obey are kept apart
   from mortar's own choices: Part A (§2 to §6) against Part B;
3. the linker-defined symbols, with the exact value and section index of
   each (§13);
4. how the layout is computed as data: parallelism, the thunk and erratum
   fixed point, and validity rules 4 and 6 of `model.md` §4.4 (§7, §14,
   §15);
5. what incremental linking needs from the layout (§18);
6. which layout properties debuggers and tools rely on (§19).

## 1. Conventions

**Rule ids.** Every normative rule has an id `L-n`. Tests (`#@ spec:`)
cite the section; findings and review notes cite the id. Ids are never
reused. A withdrawn rule keeps its id and is marked "withdrawn".

**Classes.** Each rule has one of three classes:

- **[G] generic.** Any correct output obeys it, whichever linker made it.
  The rule cites the document that requires it: the gABI by chapter, a
  psABI, the Linux kernel's ELF loader, or glibc's static start-up.
  `mortar_plumb` checks [G] rules on every linker's output. A reference
  output that breaks one is a finding.
- **[T] tool.** No loader needs it, but a tool we must work with does:
  strip, objcopy, gdb, lldb, eu-elflint. The verifier checks it on
  mortar's output. On a reference output a breach is an anomaly, not a
  finding.
- **[C] choice.** mortar's own deterministic decision. Another linker may
  decide differently. The verifier checks it on mortar's output only, and
  from the map or the link state rather than from the bytes.

**Milestones.** Each rule is tagged with the milestone that must
implement it:

- M1: static non-PIE executables on AArch64;
- M2: dynamic executables, PIE and shared objects;
- M3: x86-64;
- M4: i386;
- M7: incremental linking.

A rule tagged with a later milestone binds the design now: an M1
implementation must not make that rule hard to add. A rule with no tag
applies from M1.

**Words.** *base* is the image base (`Facts.image_base`, `spec/arch.md`).
*maxpage* is the maximum page size (`Facts.max_page`, or
`-z max-page-size`). `align_up(x, a)` is the least multiple of `a` that
is ≥ `x`. "Rank" is the position class of an output section (§5). An
"exec section" has `SHF_EXECINSTR`.

The references are GNU ld 2.44 ("GNU"), lld 19.1 and 22 ("lld"), and
mold 2.37.1. They were observed on AArch64, and on x86-64 and i386 through
cross toolchains and qemu.

---

# Part A — Rules every output obeys

These rules are kept apart from mortar's choices so that the verifier can
apply them to any linker's output. Rules that `plumb.md` already states
as S1-S12 are not repeated; §17 maps each rule here to a verifier rule.

## 2. Loadability

- **L-1 [G]** Every `PT_LOAD` has `p_vaddr ≡ p_offset (mod p_align)`, and
  `p_align` is a multiple of the runtime page size. The Linux kernel
  (`load_elf_binary`) maps each `PT_LOAD` with `mmap` at
  `align_down(p_vaddr)` from file offset `align_down(p_offset)`, so the
  two must agree modulo the page. mortar's outputs must load on 64 KiB
  page kernels on AArch64, so mortar's own congruence modulus is maxpage
  (L-70, L-74).
- **L-2 [G]** `PT_PHDR`, if present, comes before every `PT_LOAD` and
  lies inside one `PT_LOAD`'s file image. `PT_INTERP`, if present, also
  comes before every `PT_LOAD`. Each of these occurs at most once (gABI
  "Program Header").
- **L-3 [G]** The program header table lies in the file image of a
  `PT_LOAD`. The kernel derives `AT_PHDR` from the loaded image, and
  glibc's static start-up reads the program headers through `AT_PHDR`
  (it finds `PT_TLS` and `PT_GNU_RELRO` that way). When the table is not
  in memory, a static program gets no TLS.
- **L-4 [G]** An allocated `SHT_NOBITS` section of non-zero size,
  `.tbss` aside, lies at or beyond `p_vaddr + p_filesz` of its
  `PT_LOAD`. Otherwise the loader maps file bytes over memory the program
  expects to be zero. (S12 checks that the memory reads as zero; this
  rule says where such sections may sit.)
- **L-5 [G]** Every section has `sh_addr ≡ 0 (mod sh_addralign)` when
  `sh_addralign > 1` (gABI "Sections").
- **L-6 [G]** No `PT_LOAD` is both `PF_W` and `PF_X` unless a linker
  script asks for one (`-z execstack` concerns the stack, not a
  `PT_LOAD`). The references never make one in a default link. This is
  hardening, not correctness: the loader accepts RWX segments, so the
  verifier reports it as a note.

## 3. RELRO

- **L-7 [G]** At most one `PT_GNU_RELRO` is present. Its memory lies
  inside the memory of one `PF_W` `PT_LOAD` or of several consecutive
  ones.
- **L-8 [G] (RELRO safety)** After relocation, glibc and musl make
  `[align_down(s, P), align_down(e, P))` read-only, where `[s, e)` is the
  `PT_GNU_RELRO` range and `P` is the runtime page size. glibc does this
  in `_dl_protect_relro`, documented behaviour. So for every page size
  `P` the program can run under (4 KiB up to the `p_align` of the
  `PT_LOAD`s), that range must not contain a byte of a writable section
  that is not RELRO. Writable non-RELRO sections are `.data`, `.bss`,
  `.got.plt` without `-z now`, and orphans. A breach makes the program
  fault on its first write to that data, and only on some kernels. That
  is the worst kind of bug, so the verifier checks this rule for every
  `P` in {4 KiB, 16 KiB, 64 KiB} that does not exceed the `PT_LOAD`
  `p_align`.
- **L-9 [T]** Every byte of every `PT_LOAD`'s memory, from `p_vaddr` to
  `p_vaddr + p_memsz`, lies in a section, the file header or the program
  header table. Gaps for alignment between sections are allowed. The end
  of the memory, however, must be the end of a section. strip and objcopy
  rebuild a `PT_LOAD`'s sizes from the sections they keep: when RELRO
  padding was not a section, strip shrank the RELRO segment's `p_memsz`
  from 0x710 to 0x1d8 and left `PT_GNU_RELRO` at 0x710 (E-14). That
  breaks L-8 on the stripped file. This is why lld and mold emit
  `.relro_padding`, and why mortar does (L-64).

## 4. Thread-local storage (generic)

- **L-10 [G]** At most one `PT_TLS`. Its `p_align` is a power of two at
  least the `sh_addralign` of every `SHF_TLS` section, and
  `p_filesz ≤ p_memsz`. Every `SHF_TLS` section lies in
  `[p_vaddr, p_vaddr + p_memsz)`. The `SHT_PROGBITS` TLS sections come
  first and the `SHT_NOBITS` ones after them. The initialisation image
  `[p_offset, p_offset + p_filesz)` is exactly the bytes of the
  `PROGBITS` ones, together with the padding between them (ELF Handling
  For Thread-Local Storage, "Thread-local storage section").
- **L-11 [G]** The `p_memsz` of `PT_TLS` is at least
  `end − p_vaddr`, where `end` is the end of the last `SHF_TLS`
  section. Its `p_filesz` is at least the end of the last `PROGBITS`
  TLS section minus `p_vaddr`.
- **L-12 [G]** `.tbss` takes no space in the address map of the
  `PT_LOAD`s. Its `sh_addr` is the TLS template address it would have,
  which overlaps the next section, so overlap rules exempt it (S4 and S6
  already do).

## 5. Start-up tables (generic for glibc static start-up)

glibc's static start-up (`csu/libc-start.c`, documented through its
symbol contract) runs the arrays between linker-defined bounds, and
applies `IRELATIVE` relocations between `__rela_iplt_start` and
`__rela_iplt_end`.

- **L-13 [G]** In a static executable, `__preinit_array_start`/`_end`,
  `__init_array_start`/`_end` and `__fini_array_start`/`_end` (when
  referenced) are the start and end of the output section of that name.
  When the section is absent, start equals end. The section's size is a
  multiple of the word size, and it is word-aligned.
- **L-14 [G]** In a static non-PIE executable, `__rela_iplt_start` and
  `__rela_iplt_end` (when referenced) bound exactly the `R_*_IRELATIVE`
  records, with no other relocation type between them. When there are
  none, start equals end.
- **L-15 [G]** `__ehdr_start` (when referenced) is the address of the
  file header in memory. That requires the file header to be in a
  `PT_LOAD`, at `p_offset` 0.

## 6. Stack and properties

- **L-16 [G]** At most one `PT_GNU_STACK` (S10). Its `PF_X` is set only
  when the link asked for an executable stack (§11.6).
- **L-17 [G]** `PT_GNU_PROPERTY`, when present, covers exactly the
  `.note.gnu.property` section. That section is a valid program-property
  note: 8-aligned in ELF64, 4-aligned in ELF32 (Linux gABI extensions,
  "Program Property"). The AArch64 kernel reads BTI from it. A wrong
  property note can turn on BTI for code that is not BTI-clean.
- **L-18 [G]** Non-allocated sections do not lie in any `PT_LOAD`'s file
  image. (They may be anywhere else in the file.)

---

# Part B — mortar's layout

## 7. The computation

The layout is a sequence of stages. Each stage is a function of the
tables before it.

| stage | reads | writes | parallel over |
|---|---|---|---|
| A. assign (§8) | `Inputs`, `Resolution`, `Plan` | `Assignment` | files |
| B. order (§9) | `Assignment`, `Liveness`, `Folds`, `Split` | per output section, the ordered live sections | output sections |
| C. chunk (§10, §12) | B, `MergeMap`, `Slots`, `Demands` | `chunks` (kind, flags, align, size), `order` | output sections |
| D. address (§11) | C, `Facts` | `addr`, `offset`, `segments`, `tls`, `shndx` | sequential over chunks (O(chunks)) |
| E. fixed point (§14) | D, the relocations through `spec/arch.md` | `patches`, `sized`; then D again | sites, by section |
| F. symbols (§13) | D/E | `defined`, `sym_place`, `entry` | none needed |

Stage A interns `OutSecId`s in first-appearance order: each file
collects the names it introduces (ordered collection, model §6 rule 3),
and ids are handed out after concatenation in `FileId` order. Stage B is
a partition of `SectionId`s by output section, in `SectionId` order,
followed by a stable sort only for the output sections §9 names. Most
output sections need no sort.

- **L-20 [C]** Every stage obeys `model.md` §6: the stage has one
  sequential meaning, and its parallel form uses only the six permitted
  patterns. No order in a table follows thread timing or hash iteration.
- **L-21 [C]** Stages A to C use no addresses. Everything they decide
  (assignment, input order, which chunks exist, chunk sizes and
  alignments, the offsets of sections within chunks) is
  address-independent. One exception: chunks in `sized` (L-121) may grow
  during E. That is what makes addresses cheap to recompute in E and in
  incremental links (§18).
- **L-22 [C]** The offsets of sections within a chunk are the aligned
  prefix sum of their sizes, in the order of stage B: offset₀ = 0,
  offsetᵢ₊₁ = align_up(offsetᵢ + sizeᵢ, alignᵢ₊₁). A chunk's alignment is
  the maximum alignment of its members, so offsets stay valid at any
  chunk address with that alignment. The sum is sequential within a
  chunk and parallel across chunks. It costs about 2-5 ms for 10⁶
  sections (one pass over 16 bytes per section). Measured in WP-027, not
  here.

## 8. Output sections (stage A)

### 8.1 Sections that go nowhere

The table below is checked from the top; the first row that applies
wins.

| input section | outcome | rule |
|---|---|---|
| index 0, `SHT_NULL`, `SHT_SYMTAB`, `SHT_STRTAB`, `SHT_RELA`/`SHT_REL` (and `SHT_SYMTAB_SHNDX`, `SHT_GROUP`) | consumed by the reader, never assigned | L-30 |
| `SHF_EXCLUDE`, any type | dropped. Clang's `.llvm_addrsig` is the common case (E-11) | L-31 |
| `.note.GNU-stack` | consumed: decides `PT_GNU_STACK` (§11.6) | L-32 |
| `.gnu.warning`, `.gnu.warning.SYM` | consumed: no output section. The text is a `link-warning`: for `.gnu.warning.SYM`, once, when a live input other than the defining file references `SYM`; for plain `.gnu.warning`, when its file is linked in (GNU). Static glibc's `libc.a` carries `.gnu.warning.dlopen` and others in members every static link extracts. lld copies them into the output as orphans, mold into one `.gnu.warning`, GNU consumes them; none warns unless the symbol is used (E-17) | L-33 |
| `.note.gnu.property` | consumed by synthesis (`spec/arch.md` / `synth.md`), which builds the merged property note | L-34 |
| `SHT_AARCH64_ATTRIBUTES` | refused (`section-type`) until the psABI build-attributes rules are specified | L-35 |
| `SHF_X86_64_LARGE` set | refused (`section-flag`, `SHF_X86_64_LARGE`) [M3] | L-36 |
| `.gnu.linkonce.*` | refused (`feature`, `.gnu.linkonce sections`): pre-COMDAT, and no current compiler emits it | L-37 |
| `.zdebug_*` | refused (`compression-type`, `.zdebug`). Only `SHF_COMPRESSED` is accepted | L-38 |
| a name that a synthetic section owns (`.got`, `.got.plt`, `.plt`, `.iplt`, `.rela.dyn`, `.rela.plt`, `.dynamic`, `.dynsym`, `.dynstr`, `.hash`, `.gnu.hash`, `.eh_frame_hdr`, `.relr.dyn`, `.interp`) | refused (`feature`, `input section <name>`). `.interp` becomes accepted at M2, when it decides the interpreter as in GNU | L-39 |
| `.ctors*`, `.dtors*`, non-empty | M1 refuses (`feature`, `.ctors section`); the rule from M2 is L-56 | L-40 |
| an `sh_type` that `elf.md` does not accept for its flags (`elf.md` owns the list) | refused (`section-type`), already by the reader; layout never sees it | L-41 |
| not live (`Liveness`), or in a discarded COMDAT group | given an output section like any other, but no place | L-42 |

- **L-43** Dead and discarded sections still get an `out_of` entry, so
  the assignment never depends on liveness. Their output section can
  have zero live members. An output section with no live member and no
  synthetic content produces no chunk. The exceptions are a section that
  a script `KEEP`s and a section that a linker-defined symbol names
  (`__start_SEC`, §13): those get an empty chunk, so the symbol has a
  place.

### 8.2 The name rule

- **L-44 [C]** An input section named `N` goes to the output section of
  the first row below that has a prefix `P` with `N == P`, or with `N`
  starting with `P` followed by `.`. If no row matches, it goes to an
  orphan output section named `N` (L-48). The rule looks at the name
  only, never at the file.

| output | prefixes `P` (in match order) | M |
|---|---|---|
| `.text` | `.text` (this covers `.text.unlikely`, `.text.hot`, `.text.startup`, `.text.exit`, `.text.sorted.*`) | M1 |
| `.rodata` | `.rodata` | M1 |
| `.data.rel.ro` | `.data.rel.ro` (`.data.rel.ro.local*` included) | M1 |
| `.data` | `.data` | M1 |
| `.bss.rel.ro` | `.bss.rel.ro` | M2 |
| `.bss` | `.bss`, and the commons of §12.4 | M1 |
| `.tdata` | `.tdata` | M1 |
| `.tbss` | `.tbss` | M1 |
| `.preinit_array` | `.preinit_array` | M1 |
| `.init_array` | `.init_array` (`.ctors` too from M2, L-56) | M1 |
| `.fini_array` | `.fini_array` (`.dtors` too from M2, L-56) | M1 |
| `.gcc_except_table` | `.gcc_except_table` | M1 |
| `.eh_frame` | `.eh_frame` | M1 |
| `.init`, `.fini`, `.tm_clone_table`, `.comment` | the name itself | M1 |

- Prefix order matters only for `.data.rel.ro` before `.data`, and
  `.bss.rel.ro` before `.bss`. The table lists them first.
- GNU's script differs on rare names, and in those cases mortar takes
  the uniform rule:
  - `.data1`, `.rodata1`, `.sdata*` and `.preinit_array.*` become
    orphans in mortar;
  - GNU folds `.gnu.linkonce.*` and `.text.*_unlikely`;
  - lld's list matches the table, with `.ldata`/`.lrodata`/`.lbss` added
    for x86-64 large sections (M3, refused by L-36).
- **L-45 [C]** Notes (`SHT_NOTE`) are not merged by name. Each distinct
  note name is its own output section, with the name kept:
  `.note.ABI-tag`, `.note.stapsdt` and the rest. The synthetic notes are
  `.note.gnu.build-id` and `.note.gnu.property`. Non-allocated notes
  (`.note.stapsdt` is allocated in some toolchains and not in others)
  follow L-47.
- **L-46 [C]** Each `.debug_*` name is its own non-allocated output
  section, with the name kept.
- **L-47 [C]** An allocated input and a non-allocated input never share
  an output section. A TLS input and a non-TLS input never share one
  either. When the name rule would put them together, the link fails
  with `section-conflict` (§20.4). GNU and lld silently merge the flags;
  mortar refuses, because a program that mixes `.tdata` and `.data`
  under one name is wrong with either choice.
- **L-48 [C] (orphans)** An orphan output section keeps the input name.
  Its rank (§10) comes from its type and flags: TLS, writable, exec,
  `NOBITS`, alloc. An orphan whose name is a C identifier gets
  `__start_`/`__stop_` symbols when they are referenced (§13.1, §13.2).
  Registration sections reached only through those bounds are the
  common use of orphans: systemd, the Linux tools, and older glibc
  (`__libc_atexit`, `__libc_subfreeres`; the glibc of the test host
  has none, E-17).

### 8.3 The output section's type and flags

- **L-49 [C]** An output section's `sh_type` is the type its inputs share.
  `SHT_PROGBITS` and `SHT_NOBITS` together give `PROGBITS`, with the
  `NOBITS` inputs written as zeros. Other mixtures are refused
  (`section-conflict`). The special array types (`SHT_INIT_ARRAY`,
  `SHT_FINI_ARRAY`, `SHT_PREINIT_ARRAY`) also accept `PROGBITS` inputs
  named for them. Old compilers emit `.init_array` as `PROGBITS`; the
  output type is then the special type. `SHT_NOTE` stays `SHT_NOTE`.
- **L-50 [C]** Output flags are the OR of the inputs' `SHF_WRITE`,
  `SHF_ALLOC`, `SHF_EXECINSTR`, `SHF_TLS` and `SHF_GNU_RETAIN`.
  `SHF_MERGE`, `SHF_STRINGS` and `sh_entsize` are kept only when every
  live input has the same merge class `(strings, entsize)`. Otherwise all
  three are dropped (entsize 0). `SHF_INFO_LINK`, `SHF_LINK_ORDER`,
  `SHF_GROUP` and `SHF_COMPRESSED` never appear in an executable's output.
  Reason: lld keeps `SHF_MERGE` on a `.rodata` that holds several
  classes, with entsize 0, and eu-elflint reports it (E-12). mold splits
  each class into its own output section (`.rodata.str1.1`,
  `.rodata.cst16`). Neither matters to a loader.
- **L-51 [C]** An output section's alignment is the maximum alignment of
  its chunks (§12).

### 8.4 Compressed inputs

- **L-52 [C]** An input with `SHF_COMPRESSED` (zlib, and zstd when
  `elf.md` accepts it) is decompressed by the reader and placed by its
  decompressed size. The output is uncompressed unless
  `--compress-debug-sections` is given (`sized`, model §4.4). GNU, lld
  and mold all decompress (E-11).

## 9. Input order within an output section (stage B)

The default order is `SectionId` order: files in load order, sections by
index. That is lld's and mold's order and GNU's order within each script
pattern. The exceptions below are each a sort by a key that ends in
`SectionId`, so they are total and deterministic.

- **L-53 [C] (.text categories)** In `.text`, inputs are grouped into
  categories, in this order:
  1. `.text.unlikely`/`.text.unlikely.*`;
  2. `.text.exit`/`.text.exit.*`;
  3. `.text.startup`/`.text.startup.*`;
  4. `.text.hot`/`.text.hot.*`;
  5. `.text.sorted.*`, sorted by the name's suffix as GNU's
     `SORT(.text.sorted.*)` does: byte order of the name;
  6. everything else.
  Within a category the order is `SectionId`. This is GNU's default
  script (`ld --verbose`). lld and mold use plain `SectionId` order
  unless `-z keep-text-section-prefix` is given (E-3). mortar takes GNU's
  grouping: it puts cold code together and hot code together, which is
  what the compiler asked for by naming them, and it costs one key per
  section.
### 9.1 Initialization and termination

The start-up code runs `.preinit_array`, then `.init_array`, from the
first entry to the last, and at exit runs `.fini_array` from the last
entry to the first (gABI "Initialization and Termination Functions"). The
output order of the entries is therefore the run order of the
constructors, and the reverse of the run order of the destructors. The
order of the output sections themselves is in §10.

- **L-54 [C] (init priority)** In `.init_array` and `.fini_array`, an
  input named `.init_array.NNNNN` / `.fini_array.NNNNN` sorts by the
  number (decimal, 0 to 65535) before every unnumbered input. Ties go in
  `SectionId` order. A suffix that is not a decimal number below 65536
  is refused (`feature`, `init_array priority`). All three references
  sort this way (E-4).
- **L-56 [C] (.ctors/.dtors, M2)** A `.ctors` input goes into
  `.init_array`, and a `.dtors` input into `.fini_array`:
  - `.ctors.NNNNN` and `.dtors.NNNNN` take the priority 65535 − NNNNN
    (E-4); a plain `.ctors` or `.dtors` is unnumbered;
  - the words of each `.ctors`/`.dtors` input are written in reverse
    order;
  - the inputs sort into the array by L-54.
  Reason: `.ctors` ran from the last entry to the first, and
  `.init_array` runs from the first to the last. Observed run order with
  GNU and mold: A2 A1, then A-init_array B1, main, AD1 AD2 (E-5). lld
  instead makes a separate `.ctors` output that nothing runs (E-5):
  silently dropped constructors, the opposite of correct-or-refused. No
  crt file on any of the three targets has `.ctors` or `.dtors`. The
  words are reversed in `fill`, as a patch of the section's own bytes
  (`spec/image.md`), never by moving the section. Until then, L-40
  refuses.
### 9.2 Other output sections

- **L-55 [C]** Every other output section uses `SectionId` order, except
  `.eh_frame` (L-58) and `SHF_LINK_ORDER` sections (L-57).
- **L-57 [C] (SHF_LINK_ORDER)** An input with `SHF_LINK_ORDER` sorts by
  where its linked section is placed: (output section rank, output
  section's `OutSecId`, the linked section's position in stage B order),
  then `SectionId`. The key uses no addresses. `__patchable_function_entries`
  and Clang's `.llvm_bb_addr_map` are the common cases. A linked section
  that is not live makes the input dead (`Liveness`).
- **L-58 [C] (.eh_frame)** `.eh_frame` inputs keep `SectionId` order. The
  terminator that `crtend.o` provides (a zero-length CIE) must stay last,
  and it does, because `crtend.o` comes after the user's objects in every
  driver's line. Static start-up with `crtbeginT.o` walks
  `__EH_FRAME_BEGIN__` to that terminator without `.eh_frame_hdr`.
  `synth.md` owns CIE de-duplication and the removal of dead FDEs. Both
  keep this order for what remains.

## 10. Output section order

- **L-60 [C]** Chunks are ordered by (rank, the order within the rank
  below, then `OutSecId`). `OutSecId` order is first appearance in
  `SectionId` order (model §3.4). The table is the whole default order.
  The references' orders are in E-8.

| rank | segment class | output sections, in order | M |
|---|---|---|---|
| 0 | R | file header, program headers | M1 |
| 1 | R | `.interp` | M2 |
| 2 | R | `.note.gnu.property`, `.note.gnu.build-id`, `.note.ABI-tag`, other allocated notes in `OutSecId` order | M1 |
| 3 | R | `.hash`, `.gnu.hash`, `.dynsym`, `.dynstr`, `.gnu.version`, `.gnu.version_d`, `.gnu.version_r` | M2 |
| 4 | R | `.rela.dyn` (`.rel.dyn` on i386), `.relr.dyn`, `.rela.plt` (`.rel.plt`) | M1 for `.rela.plt` (IRELATIVE in a static link), M2 the rest |
| 5 | R | `.rodata`, read-only orphans (`PROGBITS`, alloc, not W, not X), `.gcc_except_table`, `.eh_frame_hdr`, `.eh_frame` | M1 |
| 6 | RX | `.init`, `.plt` (`.iplt` content in a static link), `.text`, exec orphans, `.fini` | M1 |
| 7 | RW-RELRO | `.tdata`, `.tbss`, TLS orphans (`PROGBITS` then `NOBITS`), `.preinit_array`, `.init_array`, `.fini_array`, `.data.rel.ro`, `.dynamic`, `.got`, `.got.plt` (only with `-z now`, M2), `.bss.rel.ro`, relro padding (L-64) | M1 (`.dynamic`, `.bss.rel.ro` at M2) |
| 8 | RW | `.got.plt`, `.data`, `.tm_clone_table`, writable `PROGBITS` orphans, `.bss`, writable `NOBITS` orphans | M1 |
| 9 | none | non-allocated sections other than the four below, in `OutSecId` order (`.comment`, `.debug_*`, notes) | M1 |
| 10 | none | `.symtab`, `.symtab_shndx`, `.strtab`, `.shstrtab` | M1 |
| 11 | none | section header table | M1 |

- **L-61 [C]** A read-only orphan with `SHT_NOBITS` (allocated, not
  writable) goes at the end of rank 5. An exec `NOBITS` orphan is
  refused (`section-conflict`).
- **L-62 [C]** Reasons for the table:
  - Rodata comes before text, as in lld and mold. GNU puts it after.
    With this order the headers and the read-only data share one R
    segment, and no code page is readable as data.
  - `.eh_frame` is last in rank 5, next to the code it describes,
    following GNU and lld.
  - `.fini` is last in RX, so `etext` is the end of `.fini` as in GNU
    (E-6).
  - `.got.plt` in a static link holds only IRELATIVE slots. It stays
    writable (rank 8), as in all three references. glibc applies
    IRELATIVE before it protects RELRO, but that ordering was not
    confirmed for static start-up, so mortar does not depend on it
    (unverified, WP-024-spec §5).
  - `.preinit_array` comes before `.init_array`, as in GNU. lld puts it
    after; nothing depends on that.
  - `.tm_clone_table` is in RW because libgcc's `crtbegin` writes
    nothing to it but GNU puts it there. It is kept as is.
- **L-63 [C]** The section header table comes last. The `.symtab`,
  `.strtab` and `.shstrtab` order follows GNU, lld and mold.
- **L-64 [C] (relro padding)** When rank 7 is non-empty, a NOBITS chunk
  named `.relro_padding` (L-156, open question Q3 on the name) ends
  rank 7. Its size makes the RELRO end a multiple of maxpage:
  `align_up(end, maxpage) − end`. It has `SHF_ALLOC|SHF_WRITE`,
  alignment 1, and takes no file space. lld pads only to the common page
  (4 KiB), which on a 64 KiB-page kernel leaves the last RELRO page
  writable (E-1). mold and GNU pad to maxpage. GNU does it by moving the
  RW segment's start instead, which costs file space.

## 11. Segments

### 11.1 Which segments

- **L-65 [C]** The `PT_LOAD`s are, in this order, each present only when
  it has a chunk:
  1. **R**: ranks 0-5;
  2. **RX**: rank 6;
  3. **RW-RELRO**: rank 7, `PF_R|PF_W`;
  4. **RW**: rank 8, `PF_R|PF_W`.
  This is the lld/mold shape (E-1). GNU makes one R E segment for ranks
  0-6 (headers and rodata executable) and one RW for ranks 7-8 on
  AArch64. mortar keeps code pages out of the readable-data segment and
  makes the RELRO run its own segment, so that L-8 holds by construction
  at every page size up to maxpage.
- **L-66 [C]** Which segments exist depends only on which chunks exist,
  which is address-independent (L-21). Adding a thunk never creates or
  removes a segment.
- **L-67 [C]** The other program headers, in this order, each when its
  condition holds:

| type | when | covers | `p_align` |
|---|---|---|---|
| `PT_PHDR` | `PT_INTERP` is present (M2) | the program header table | 8 (ELF64) / 4 |
| `PT_INTERP` | dynamic, not `-shared` (M2) | `.interp` | 1 |
| `PT_LOAD` ×1-4 | L-65 | | maxpage |
| `PT_DYNAMIC` | dynamic (M2) | `.dynamic` | word |
| `PT_NOTE` ×n | allocated notes | one per run of consecutive note chunks with the same alignment | that alignment |
| `PT_TLS` | a TLS chunk | §11.4 | max TLS alignment |
| `PT_GNU_PROPERTY` | `.note.gnu.property` | it | 8 / 4 |
| `PT_GNU_EH_FRAME` | `.eh_frame_hdr` | it | 4 |
| `PT_GNU_STACK` | always | nothing (all zero but flags) | 16 |
| `PT_GNU_RELRO` | rank 7 non-empty | rank 7 including padding | 1 |

  - The order is GNU's (E-9). lld puts `PT_NOTE` last; mold puts it
    before the `PT_LOAD`s. Nothing reads the order beyond L-2.
  - GNU emits `PT_PHDR` only with an interpreter, which is why the rule
    is tied to `PT_INTERP`. lld and mold emit it always (E-9). glibc's
    `rtld` and the kernel need it only when there is an interpreter.
  - `PT_NOTE` per alignment run follows GNU. The gABI requires a note
    segment's notes to share one alignment for parsing (`plumb.md` S11).
  - The `PT_GNU_STACK` `p_align` of 16 follows GNU (lld 0, mold 1).
  - The `PT_GNU_RELRO` `p_align` of 1 follows all three.

### 11.2 Addresses and offsets

- **L-68 [C]** The first chunk (file header) has offset 0 and address
  `base` (non-PIE: `Facts.image_base`; PIE and shared: 0, M2). The base
  is a multiple of maxpage. Table of bases (E-2):

| target | mortar | GNU | lld | mold |
|---|---|---|---|---|
| AArch64 non-PIE | 0x400000 | 0x400000 | 0x200000 | 0x200000 |
| x86-64 non-PIE (M3) | 0x400000 | 0x400000 | 0x200000 | 0x200000 |
| i386 non-PIE (M4) | 0x08048000 | 0x08048000 | 0x400000 | 0x200000 |
| PIE, shared (M2) | 0 | 0 | 0 | 0 |

  mortar takes GNU's bases. Tools and users carry assumptions about
  them: crash triage that recognises 0x400000, gdb scripts. They are the
  bases of the compilers' native linker.
- **L-69 [C]** Within a segment, chunk *k* gets
  `addr = align_up(prev_end, align_k)` and the same displacement in the
  file: `offset = prev_offset_end + (addr − prev_end)`. NOBITS chunks
  advance the address only. The file never contains bytes for a NOBITS
  chunk, and a PROGBITS chunk never follows a NOBITS one within a
  segment: the rank table puts NOBITS last in each segment. `.tbss` is
  the exception, because it takes no address space (L-77). A layout with
  a PROGBITS chunk after a NOBITS one fails `Layout::check`
  (`internal-error`).
- **L-70 [C] (next segment)** The first chunk of a new `PT_LOAD` gets
  the least address `A` with
  `A ≥ align_up(prev_segment_mem_end, maxpage)`,
  `A ≡ offset (mod maxpage)` and `A ≡ 0 (mod align)`. Here `offset` is
  the next free file offset, aligned to the chunk's alignment. File
  offsets stay contiguous, and no two segments share a page in memory at
  any page size up to maxpage. This is lld's and mold's rule, observed
  exactly (E-1): `0x227b60 → 0x237b80` with offset `0x27b80`.
- **L-71 [C] (separate code)** With `-z separate-code`, the RX segment's
  file offset and the next segment's offset are each rounded up to
  maxpage, and the gap is filled with zeros (exec: trap pattern, L-82).
  No page of the file then holds both code and other bytes. The default
  is `-z noseparate-code` on every target. That is GNU's default on
  AArch64 and lld's default everywhere; GNU defaults to separate-code on
  x86-64 (E-1, open question Q2).
- **L-72 [C]** Non-allocated chunks follow the last allocated chunk's
  file end, each at `align_up(offset, align)`, with no address (rank
  9-11). The section header table is aligned to the word size.
- **L-73 [C]** An address or offset that does not fit the class (ELF32:
  2³²; ELF64: 2⁶⁴, and also the AArch64 kernel's 48-bit user space) is
  `output-too-large` (§20.4), never a wrapped value.

### 11.3 Page sizes

- **L-74 [C]** maxpage defaults to `Facts.max_page`: 64 KiB on AArch64,
  4 KiB on x86-64 and i386. `-z max-page-size=N` sets it. N must be a
  power of two between 4096 and 2²⁰ (at most 2¹⁶ is recommended), and
  the base must be a multiple of it. An invalid N is
  `invalid-option-value`.
- **L-75 [C]** `-z common-page-size=N` is accepted and validated (a
  power of two ≤ maxpage). It changes nothing in mortar's layout, because
  every decision that GNU or lld takes at the common page, mortar takes
  at maxpage. That costs at most maxpage of address space per segment,
  and no file space.

### 11.4 Thread-local storage

- **L-76 [C]** TLS chunks are the first chunks of rank 7: `.tdata` (all
  TLS `PROGBITS`), TLS `PROGBITS` orphans, then `.tbss` and TLS `NOBITS`
  orphans. Let `T` be the maximum alignment of every TLS chunk. The
  first TLS chunk is placed at an address that is a multiple of `T`,
  even when its own alignment is smaller. `tls.start` is that address
  (`tls.start ≡ 0 mod tls.align`, arch request M-3).
- **L-77 [C]** `.tbss` gets `addr = align_up(tdata_end, align_tbss)` and
  `offset = tdata_offset_end`. It takes no address space in the
  `PT_LOAD`s, so the next chunk (`.preinit_array` or whichever comes
  next) starts at `tdata_end`, aligned to its own alignment, not after
  `.tbss`.
- **L-78 [C]** `tls = {start, size, align}` with `size` = the end of the
  last TLS chunk − start, the exact extent. `align = T`. `PT_TLS` has
  `p_vaddr = start`, `p_memsz = size`, `p_filesz` = the end of `.tdata` −
  start, and `p_align = T`. The section `.tdata` keeps `sh_addralign` =
  its own maximum (lld), not T (GNU raises it, E-7). The difference has
  no effect on the loader.
- **L-79 [C]** A TLS symbol's value in `.symtab` is its offset from
  `tls.start` (gABI; all three, E-7). The TLS offsets that relocations
  use are `spec/arch.md`'s (AArch64 variant 1: TP + align_up(16, T) +
  offset).

### 11.5 Dynamic outputs (M2) and other targets (M3, M4)

- **L-80 [C] [M2]** PIE and shared objects use the same ranks and
  segment rules with base 0. RELRO then also holds `.dynamic`, `.got`,
  `.data.rel.ro` and, with `-z now`, `.got.plt`. `.bss.rel.ro` holds copy
  relocations of read-only data from shared objects.
- **L-81 [C] [M3, M4]** x86-64 and i386 change only the facts:
  - maxpage is 4 KiB;
  - the bases are those of L-68;
  - the code fill is `0xCC` (L-82);
  - `.got.plt` has three reserved words in dynamic outputs (`arch.md`
    `got_base: GotPltStart`);
  - i386 uses REL, so `.rel.dyn`/`.rel.plt` replace the RELA names.
  No rule here names a machine except through `Facts`.

### 11.6 The stack — `PT_GNU_STACK`

- **L-84 [C]** `PT_GNU_STACK` has `PF_R|PF_W`. It gets `PF_X` only if
  `-z execstack` is given, or if some live input has a `.note.GNU-stack`
  section with `SHF_EXECINSTR`. In the second case mortar warns with
  `executable-stack`. `-z noexecstack` overrides both, without a warning.
  An input without `.note.GNU-stack` does not make the stack executable:
  that is what lld and mold do (E-10). GNU warns and makes it RWX only
  when an input explicitly asks with an exec note. Without the note,
  GNU 2.44 on AArch64 keeps RW. All three agree when the note is absent.

## 12. Chunks (stage C) and padding

### 12.1 Which chunks

- **L-85 [C]** Each output section with live content becomes chunks in
  `order`:
  - `Body(OutSecId, part)` for the placed input sections, in stage B
    order, cut into parts (L-110). There is one part unless the output
    section is exec and larger than the island spacing;
  - `Merged(OutSecId)` after the body. It holds the live pieces of every
    merge class in the section, ordered by class (strings before
    constants, then entsize ascending), then by `MergeMap` order. One
    merged chunk per output section;
  - `Patches(OutSecId, part)` directly after each exec body part, for
    thunks (§14). It exists in the chunk list even when it is empty
    (size 0), so that a thunk never creates a chunk;
  - synthetic chunks (`spec/synth.md`) at the positions the rank table
    gives them;
  - for `.bss`, the commons (L-88) after the input bodies.
- **L-86 [C]** All the chunks of one output section share one section
  header. Its `sh_addr` is the first chunk's address, and its `sh_size`
  runs from there to the end of the last chunk's capacity. Section
  indices are given densely in chunk order (`check.rs`), header chunks
  aside.
- **L-87 [C]** A chunk's `capacity` is its `size` by default. Only an
  incremental policy (§18) leaves slack.

### 12.2 Commons

- **L-88 [C]** A common symbol that resolution chose is allocated in
  `.bss` (`sym_place`), after the `.bss` input sections, in `SymbolId`
  order, each aligned to its `st_value` (the common alignment). GNU
  orders commons by name within the `COMMON` pattern, lld by input
  order. The order has no observable effect. `SymbolId` is
  deterministic and needs no sort.
- **L-89 [C] [M1]** A TLS common (`STT_TLS` with `SHN_COMMON`) is
  refused (`feature`, `TLS common symbol`). GCC emits it only with
  `-fcommon` on TLS variables, which no current default does.

### 12.3 Fill

- **L-82 [C]** Padding inside an exec chunk, and between exec chunks of
  one segment, is the machine's trap pattern, positioned by address
  modulo the pattern length (`Facts.code_fill`, a request to `arch.md`):
  - AArch64: zero words, which decode as `UDF #0`;
  - x86: `0xCC` (`int3`).
  Inside `.init` and `.fini`, whose fragments run sequentially from
  crti through crtn, padding between inputs is the machine's no-op
  instead: AArch64 `NOP` (`0xd503201f`), x86 `0x90`. Any other padding
  inside a chunk is a trap, so falling into the gap from code is a crash
  and not a wrong answer. All other padding is zero. `.init` input
  sections are 4-aligned on AArch64, so the no-op case seldom arises;
  that does not make the rule optional.
- **L-83 [C]** The bytes between segments in the file, under
  separate-code, are zeros in data and the trap pattern in exec. Bytes
  covered by no chunk are not read by any tool.

## 13. Linker-defined symbols

- **L-90 [C]** A linker-defined symbol from the table below is defined
  only when an undefined (or weak undefined) reference to it remains
  after resolution, and no input defines it. This is lld's and mold's
  rule; GNU's `PROVIDE` is the same. GNU also defines `_edata`, `_end`,
  `__bss_start`, `__bss_start__`, `_bss_end__`, `__bss_end__` and
  `__end__` unconditionally. mortar does not: an unreferenced symbol
  changes only `.symtab`.
- **L-91 [C]** Each defined symbol is a `DefValue::At{chunk, offset}`,
  never an absolute number, except where the table says *absolute*. Its
  `st_shndx` is the index of the output section of that chunk. A value
  at the end of a section uses that section's index. When the anchor is
  a header chunk (file header, program headers), the index is 1, the
  first section (lld). Binding is `STB_GLOBAL` (GNU and lld). Visibility
  is `STV_HIDDEN` for the `PROVIDE_HIDDEN` names of GNU's script, and
  `STV_DEFAULT` otherwise. `__start_`/`__stop_` are `STV_PROTECTED`
  (GNU, lld; mold makes them local). Type: `STT_NOTYPE`, except
  `_TLS_MODULE_BASE_` (L-98).
- **L-92 [C]** "End" means the end of the chunk's capacity, not its
  size, so a symbol bounding a section still bounds it after slack is
  added. The exception is the packed sections of L-142 (arrays and
  `__start_`/`__stop_` sections), whose end symbols are at `size`,
  because the slack must not be part of the array (Q4). With the
  default policy, size and capacity are equal, so the choice is
  invisible until M7. (Request: `model.md` validity rule 5 compares a defined offset
  with capacity, §20.1.)

### 13.1 The table (static non-PIE, M1)

| symbol | value | section index | GNU | lld | mold |
|---|---|---|---|---|---|
| `__ehdr_start` | base (file header) | 1 | same | same | same |
| `__executable_start` | base | 1 | same | same, local hidden | same, local |
| `__dso_handle` | not defined (crtbegin defines it) | | crt | defines if undefined | crt |
| `etext`, `_etext`, `__etext` | end of the last exec chunk (`.fini`) | that section | `etext`, `_etext`, `__etext` | no `__etext` | no `__etext` |
| `edata`, `_edata` | end of the last allocated non-`NOBITS` chunk in address order (rank 8 `PROGBITS`) | that section | the location counter before `.bss`, which equals `__bss_start` (E-6) | same as mortar | same as mortar |
| `__bss_start` | start of `.bss` (of the first rank 8 NOBITS chunk); if there is none, `edata` | `.bss` | as `_edata` | same as mortar | same as mortar |
| `end`, `_end` | end of the last allocated chunk in memory, `.tbss` aside | that section | align_up(…, 8) | unaligned | unaligned |
| `__bss_start__`, `__bss_end__`, `_bss_end__`, `__end__` | `__bss_start`; `end` (same section); AArch64 only | | yes (AArch64 script) | no | no |
| `__preinit_array_start`/`_end` | bounds of `.preinit_array`; when absent, both = base, index 1 | | init_array start | base, 1 | undefined |
| `__init_array_start`/`_end` | bounds of `.init_array` (absent: base, 1) | `.init_array` | same | same | same |
| `__fini_array_start`/`_end` | bounds of `.fini_array` (absent: base, 1) | `.fini_array` | same | same | same |
| `__rela_iplt_start`/`_end` (`__rel_iplt_*` on i386) | bounds of the IRELATIVE run in `.rela.plt`; absent: both = start of rank 4 or base | `.rela.plt` | same | same | same, also in PIE |
| `_GLOBAL_OFFSET_TABLE_` | `Facts.got_base`: start of `.got` on AArch64; start of `.got.plt` on x86 | that section | same | same | same |
| `__start_SEC`, `__stop_SEC` | start and end (capacity) of output section `SEC`, `SEC` a C identifier | `SEC` | protected | protected | local |
| `__GNU_EH_FRAME_HDR` | start of `.eh_frame_hdr` when it exists, else undefined | `.eh_frame_hdr` | yes | no | always |
| `_TLS_MODULE_BASE_` | 0 relative to `tls.start`, `STT_TLS`, hidden | `.tdata` (or `.tbss`) | same | same | A3 |
| `_DYNAMIC` | M2 only: start of `.dynamic` | `.dynamic` | | | |
| `__global_pointer$` | never (not a target) | | | | |

- **L-93 [C]** "Absent" for the arrays means no output section of that
  name. Both bounds are then the same value, so the loops of L-13 run
  zero times. Base with index 1 is lld's choice. GNU's value is the
  location counter, which lands inside the TLS block (E-6). A defined
  value is safer than mold's undefined weak reference (zero), which is
  the same for a loop but not for a debugger.
- **L-94 [C]** `__rela_iplt_start`/`_end` are defined only in a static
  non-PIE output. In PIE, glibc applies IRELATIVE through
  `_dl_relocate_static_pie`, and both must stay undefined weak (zero).
  mold defines them anyway, as equal values. That gives the same loop
  count but a different symbol table.
- **L-95 [C]** `edata` and `end` exclude non-allocated chunks and
  `.tbss`. `end` is unaligned. GNU aligns `_end` to 8 through its script;
  lld and mold do not. No libc relies on the alignment: glibc's `brk`
  start is page-rounded.

### 13.2 Garbage collection

Liveness is `spec/live.md`'s (`mortar_live`). Layout needs these facts
from it:

- **L-96 [C]** A reference to `__start_SEC` or `__stop_SEC` keeps every
  input section named `SEC` live, as GNU's `-z start-stop-gc` default
  under `--gc-sections`. This is GNU's default (`-z nostart-stop-gc`)
  and mold's. lld 19 defaults to `-z start-stop-gc`: it discards the
  sections, and the link then fails with "undefined symbol: __stop_myro"
  (E-13). mortar takes GNU's default, because a program that names a
  section's bounds wants its contents.
- **L-97 [C]** `SHF_GNU_RETAIN` makes an input section a liveness root,
  and the flag is kept on the output section (L-50). Sections kept by
  `KEEP` in a script are roots too (`script.md`).

### 13.3 TLS symbol

- **L-98 [C]** `_TLS_MODULE_BASE_` is defined (when referenced) as
  `STT_TLS`, `STB_GLOBAL`, `STV_HIDDEN`, value 0 (`tls.start` relative),
  in `.tdata`'s index, or `.tbss`'s if there is no `.tdata`. A reference
  with no TLS chunk is refused (`feature`, `_TLS_MODULE_BASE_ without
  TLS`).

### 13.4 Symbols mortar does not define

- **L-99 [C]** mortar never defines a symbol whose definition belongs to
  a runtime file: `__dso_handle`, `data_start`, `__data_start`, `_init`,
  `_fini`. If they are missing, `undefined-symbol` reports it, as GNU and
  mold do. lld defines `__dso_handle` itself, which hides a missing
  crtbegin.

### 13.5 x86 and dynamic additions [M2-M4]

- **L-100 [C] [M2]** In a shared object, `_end`, `_edata` and
  `__bss_start` are exported `STB_GLOBAL STV_DEFAULT` when they are
  defined (GNU and lld). `_DYNAMIC` is hidden.
- **L-101 [C] [M3]** `_GLOBAL_OFFSET_TABLE_` on x86 is the start of
  `.got.plt`, through `Facts.got_base`. Nothing else in §13 depends on
  the machine.

### 13.6 Entry point

- **L-102 [C]** `entry` is the value of the symbol named by `-e`/`--entry`
  (or `ENTRY` in a script), else `_start`. A number given to `-e` that
  is not a symbol name is accepted as an absolute address (GNU).
- **L-103 [C]** If the entry symbol is undefined, an executable fails
  with `undefined-symbol` for it. mortar does not fall back to the start
  of `.text` as GNU does with a warning, or to 0 as lld does. A program
  that starts at a guessed address is the definition of an untrusted
  link. A shared object (M2) with no entry symbol gets `e_entry = 0`.
- **L-104 [C]** The entry must lie in an exec chunk (`plumb.md` S7). An
  entry outside one is `internal-error` when mortar computed it, and a
  refusal (`feature`, `entry point outside code`) when `-e` names a data
  symbol.

## 14. Range extension and errata: the fixed point (stage E)

### 14.1 Islands

- **L-110 [C]** An exec output section is cut into body parts in stage B
  order. A new part starts before the input section that would make the
  current part larger than `spacing = reach − margin`:
  - AArch64: `reach` = 128 MiB (`B`/`BL`, `s(28)`), `margin` = 4 MiB,
    so `spacing` = 124 MiB;
  - x86: no thunks, so one part.
  The cut uses sizes only, so it is address-independent. An input
  section larger than `spacing` is refused (`feature`, `code section
  larger than branch reach`).
- **L-111 [C]** Each body part *k* is followed by its patch island
  `Patches(OutSecId, k)`. A thunk for target `t` that a site in part *k*
  needs goes in island *k*. There is one thunk per (target, island). A
  site uses island *k−1*'s thunk instead when that thunk exists and is in
  range of the site; this keeps thunk counts down when a part ends near
  a hot target. The check is in output-address order, so it is
  deterministic.
- **L-112 [C]** An island larger than `margin` is refused
  (`range-extension fixed point`, `arch.md` §6). With 12-byte AArch64
  thunks, 4 MiB holds about 350 000 thunks per 124 MiB part.
- **L-113 [C] (fast path)** If the whole RX segment (sizes only, islands
  empty) spans less than `reach − margin`, no direct branch between
  exec chunks can be out of range. Stage E then runs only for absolute
  and undefined-weak targets (`arch.md` rules) and for errata. A typical
  link, under 124 MiB of code, never iterates on addresses for range
  extension.

### 14.2 The iteration

- **L-114 [C]** Stage E repeats until nothing changes:
  1. compute addresses (stage D) from the current sizes;
  2. collect the needs: branches out of range, sites the erratum rule
     names (`arch.md` §6, collected in output-address order, by section
     in parallel);
  3. add their patches.
  Patches are only added, never removed, and a `sized` chunk only grows,
  so each round's set contains the previous one. The iteration stops
  when a round adds nothing; at most 32 rounds, then
  `range-extension fixed point`.
- **L-115 [C]** Within a round, patches go into islands in (island,
  target address, kind) order, and each gets its offset in that order.
  The table records the result, not the rounds (model §4.4).
- **L-116 [C]** Unused patches are allowed (arch M-4). A patch that a
  later round no longer needs, because an address moved, stays, so the
  sequence is monotone. Its bytes are a valid thunk that nothing calls.
- **L-117 [C] (843419)** With `--fix-cortex-a53-843419`, each ADRP
  at an offset of 0xff8 or 0xffc within a 4 KiB page that starts the
  erratum sequence is rewritten as ADR when its target is in ADR range,
  else redirected to a patch (`arch.md`). Patches for a site in part *k*
  go in island *k*. Because an island is placed after its part, adding
  one shifts only what follows it, but that may move sites in later
  parts across a 4 KiB boundary. The iteration covers that.

### 14.3 Sized chunks

- **L-121 [C]** A chunk in `sized` whose size depends on addresses
  (`.relr.dyn` [M2], patch islands, a compressed non-alloc chunk) grows
  monotonically: each round it gets the larger of its previous size and
  its new requirement. `.relr.dyn` is padded with RELR bitmap words that
  mark nothing, which is a valid encoding of the same set. Compressed
  non-alloc chunks come after every allocated chunk, so they do not move
  any address and are sized once, after the fixed point.

## 15. Validity: completing `model.md` §4.4

- **L-125 [C] (rule 3, derived segments)** `segments` is a function
  `derive_segments(chunks, order, Facts, Plan)`, implemented in
  `mortar_core` so that `Layout::check` and `mortar_layout` share one
  definition. `Layout::check` requires the stored segments to be equal
  to it. Segments then cannot disagree with chunks in a loaded layout.
  The function implements §11 and nothing else.
- **L-126 [C] (rule 4 split)**
  - **4a (tables, in `Layout::check`):** every redirect names an
    existing patch; every patch lies in a `Patches` chunk in its own
    island's range; the patch's target is a valid `SymKey`; unused
    patches are allowed.
  - **4b (reach, not in `Layout::check`):** every site that needs a
    patch has one, and every patch reaches its target and is reached
    from its sites. That needs the relocations, which `Layout::check`
    does not read. Layout checks 4b through `arch.md` in debug builds,
    after the fixed point. `fill` checks it on every field it writes,
    and the verifier checks it independently. An incremental link checks
    it on the units it refills (model §4.4 already says so).
- **L-127 [C] (rule 6)** Rule 6 (every relocated field holds its value
  at these addresses) is an obligation of `fill` and the verifier, not
  of `Layout::check`. `fill` must fail with `relocation-out-of-range`
  (arch's kind) and never write a truncated value. An incremental link
  meets it by refilling every unit that the dependency index names for a
  moved thing. Units that were not moved need no re-check.
- **L-128 [C] (TLS)** `Layout::check` requires `tls.start ≡ 0 (mod
  tls.align)` (arch M-3), and that `tls` equals what L-76 to L-78
  derive.
- **L-129 [C] (RELRO)** `Layout::check` requires the RELRO run (rank 7
  chunks, padding included) to be contiguous and to end on a maxpage
  boundary. The padding chunk is the last chunk of the run (model kind
  `RelroPad`, §20.1).

## 16. Static executables

- **L-130 [C]** For `-static -no-pie` on AArch64 with glibc, the
  default output is:
  - `PT_LOAD` R: headers, `.note.gnu.property`, `.note.gnu.build-id`,
    `.note.ABI-tag`, `.rela.plt` (IRELATIVE), `.rodata`, RO orphans,
    `.gcc_except_table`, `.eh_frame`;
  - `PT_LOAD` RX: `.init`, `.plt`, `.text` (islands), exec orphans,
    `.fini`;
  - `PT_LOAD` RW (RELRO): `.tdata`, `.tbss`, `.init_array`,
    `.fini_array`, `.data.rel.ro`, `.got`, `.relro_padding`;
  - `PT_LOAD` RW: `.got.plt`, `.data`, `.tm_clone_table`, writable
    orphans, `.bss`;
  - `PT_TLS`, `PT_NOTE`(s), `PT_GNU_PROPERTY`, `PT_GNU_STACK`,
    `PT_GNU_RELRO`;
  - non-alloc: `.comment`, `.debug_*`, `.symtab`, `.strtab`, `.shstrtab`.
  This matches lld's shape for the same link (E-1), with GNU's bases,
  order categories and symbols. It loads under the 4 KiB kernel of the
  test host. The 64 KiB-page property is L-8, checked statically,
  because no 64 KiB kernel was available.

## 17. What the verifier checks (`plumb.md`)

| rule here | verifier rule | applies to |
|---|---|---|
| L-1 | S2, extended: `p_align` ≥ 4096 and congruence | all |
| L-2, L-3 | new S13 | all |
| L-4 | new S14 | all |
| L-5 | new S15 | all |
| L-6 | new A-class note: RWX segment | all (warning) |
| L-7, L-8 | new S16 | all |
| L-9 | new T1 (anomaly for references) | mortar |
| L-10 to L-12 | S9, extended | all |
| L-13 to L-15 | new Y2 (symbol contract) | all, static |
| L-17 | new S17 | all |
| L-18 | new S18 | all |
| §8 to §15 [C] | M-rules over the map and link state (`plumb.md` §3) | mortar |

---

# Part C — Incremental linking, tools, and decisions

## 18. Incremental linking constraints (M7)

`spec/incr.md` owns the procedure. This section fixes what the layout
must make possible, so that M1 does not foreclose it.

### 18.1 What other linkers did

| linker | mode | how | what we take |
|---|---|---|---|
| gold | `--incremental`, `--incremental-update` | Reserves a percentage of free space per output section (`--incremental-patch=PERCENT`). New and grown sections go into free space, and the linker falls back to a full link when they do not fit. The previous link's inputs and layout are recorded in extra sections of the output itself. Little used; gold is now deprecated by binutils | per-section reserve, explicit fallback, "full link when in doubt"; not: state inside the output (mortar keeps a separate link state, model §7) |
| MSVC link.exe | `/INCREMENTAL` | An incremental link table (ILT): every function is called through a jump thunk, and functions are padded so they can grow. The output differs from a non-incremental link: function addresses are thunk addresses, and the indirection stays in the program | rejected: changes the program |
| Zig self-hosted | default in debug | Calls go through an offset table (GOT-like), and each atom gets surplus capacity (about a third). Moving an atom updates one table slot | the capacity idea; rejected: the indirection changes code generation, which mortar does not control for GCC or Clang inputs |
| lld, mold | none | mold's author has argued that a fast enough full link makes an incremental mode unnecessary | the bar: an incremental link must beat a full mortar link, or it is not used |
| Apple ld64 | none | | |

### 18.2 The constraints

- **L-140 [C] [M7]** An incremental link produces a layout that is valid
  (§15) and a full link given that layout produces the same bytes (model
  §7.3). There is no indirection that a full link would not have, and no
  "incremental flavour" of code.
- **L-141 [C] [M7]** Slack exists only as chunk `capacity − size` and as
  holes between sections inside a chunk. The default policy leaves none,
  so a normal link pays nothing (L-87). An incremental policy is opted
  into (`--incremental`, `cli.md`, M7). Its reserve is a percentage per
  output section, as in gold. The percentage is an open question (Q8).
- **L-142 [C] [M7] (packed sections)** Some output sections must not have
  holes, because something reads them as a dense array or a
  self-delimiting sequence:
  - `.init_array`, `.fini_array`, `.preinit_array`, `__start_`/`__stop_`
    sections (arrays bounded by symbols);
  - `.init`/`.fini` (code that falls through);
  - `.eh_frame` (a walk to the terminator), `.eh_frame_hdr` (sorted);
  - notes (a sequence);
  - `.tm_clone_table`;
  - `.debug_*`, where a hole is read as a unit header.
  Their slack is only at the end, beyond `sh_size`, and an edit inside
  them relays the whole chunk. Their end symbols (`__stop_SEC`,
  `__init_array_end`, ...) are at `size`, not `capacity` (L-92), so the
  slack is never part of the array. Growing such a section therefore
  changes a symbol value, and the dependency index must record the
  symbol's users (**Q4**).
- **L-143 [C] [M7] (hole-tolerant)** `.text` (holes filled with the trap
  pattern), `.rodata`, `.data`, `.bss`, `.data.rel.ro`, `.tdata`/`.tbss`
  (holes only at the end, since TLS offsets are baked into code) and
  `.gcc_except_table` accept holes between sections.
- **L-144 [C] [M7] (growth)** When a section changes size, the
  incremental link tries, in order, and takes the first that works:
  1. in place, if it fits the same offset (shrinking leaves a hole);
  2. move it into slack of the same chunk, and refill its referrers
     through the dependency index;
  3. relayout the chunk (stage C for that chunk only), which keeps the
     chunk's address if its capacity holds;
  4. a full relayout, keeping every decision that the new inputs do not
     invalidate.
  Each step yields a valid layout or reports why it gave up.
- **L-145 [C] [M7]** Because stages A to C are address-independent
  (L-21), step 4 costs stages D-F only: O(chunks), plus the fixed point
  over branch sites near moved code.
- **L-146 [C] [M7]** Islands are addressed by (output section, part), so
  they survive an edit elsewhere. An incremental link may add patches to
  an existing island within its capacity.

## 19. What debuggers and tools rely on

Each property below was checked against gdb 16.3, lldb 19, strip,
objcopy and eu-elflint (E-14 to E-16).

- **L-150 [T]** Section headers are present, and every allocated section
  has a header with the right address and size. gdb and lldb find
  `.text` and the debug sections by name, not by segment.
- **L-151 [T]** Every `PT_LOAD` byte up to `p_memsz` is covered by
  sections (L-9). strip and objcopy then keep the program headers as
  they are. Otherwise they rebuild them, wrongly for RELRO (E-14).
- **L-152 [T]** Non-allocated sections come after all allocated content
  in the file, so strip can drop them without moving anything that is
  loaded.
- **L-153 [C]** Section headers are in address order. This is mortar's
  choice, not a tool requirement: a GNU output with two allocated
  headers swapped still runs, and gdb, strip and objcopy handle it
  (E-15).
- **L-154 [T]** `.debug_*` sections are uncompressed by default (L-52).
  The DWARF in them is copied with relocations applied (`arch.md`).
  `.debug_*` sections of dead code keep their bytes, with relocations to
  dead sections resolved as `arch.md`/`plumb.md` R5 say (tombstone).
- **L-155 [T]** `.symtab` keeps locals and `STT_FILE` entries in input
  order (`synth.md`). gdb uses `STT_FILE` to scope static symbols.
- **L-156 [T]** `.relro_padding` is a `NOBITS` section named so that
  eu-elflint `--gnu-ld` does not take it for a `.relr*` section: it
  rejects any name beginning with `.relr` with "expected RELR" (E-12).
  That is the case for lld's and mold's name. **Q3**: keep lld's name
  (tool compatibility with llvm-objdump, which knows it) or use another
  (eu-elflint clean).
- **L-157 [T]** gdb and lldb set breakpoints on static `-g` C outputs
  of all three references (E-16). lldb needs ASLR enabled to launch
  (`settings set target.disable-aslr false`) in the test container.
  That is a property of the container, not of the binary.

## 20. Requested changes to other specs

### 20.1 `model.md`

1. `ChunkKind::Body(OutSecId)` becomes `Body(OutSecId, u32 part)`, and
   `Patches(OutSecId)` becomes `Patches(OutSecId, u32 part)` (L-110).
2. A new `ChunkKind::RelroPad`: NOBITS, the last chunk of the RELRO run
   (L-64, L-129).
3. Validity rule 3: segments are equal to `derive_segments(...)` in
   `mortar_core` (L-125).
4. Validity rule 4 is split into 4a (`Layout::check`) and 4b
   (fill/verifier), and unused patches are allowed (L-126; arch M-4).
5. Validity rule 5: a `defined` offset must be ≤ `capacity`, not ≤ size
   (L-92).
6. Validity rule 6 is an obligation of `fill` and the dependency index
   (L-127), not of `Layout::check`.
7. A `tls.start ≡ 0 (mod tls.align)` check (arch M-3, L-128).
8. `DefValue::At` on a header chunk gives `st_shndx` 1 (L-91).
9. The `ChunkId` creation order adds the patches part chunks after each
   body part.

### 20.2 `cli.md`

| option | today | after M1 |
|---|---|---|
| `-z max-page-size=N` | refused | supported (L-74) |
| `-z common-page-size=N` | refused | accepted and validated, no effect (L-75) |
| `-z separate-code`, `-z noseparate-code` | refused | supported (L-71) |
| `-z relro`, `-z norelro` | refused | supported. `norelro` removes rank 7 as a segment: its chunks join RW, and no padding is added |
| `-z execstack`, `-z noexecstack` | refused | supported (L-84) |
| `--rosegment` | refused | accepted, no-op (always) |
| `--no-rosegment` | refused | stays refused |
| `-z start-stop-gc` | refused | stays refused (L-96) |
| `-z nostart-stop-gc` | refused | no-op (the default) |
| `-z start-stop-visibility=protected` | refused | no-op (the default); other values refused |
| `-z sectionheader` | refused | no-op |
| `--image-base=N`, `-Ttext-segment=N` | refused | M2, with a check that N is a multiple of maxpage |
| `-Ttext`, `-Tdata`, `-Tbss`, `--section-start` | refused | stay refused until scripts support them |
| `-z keep-text-section-prefix` | refused | stays refused (mortar's GNU-style categories already apply) |
| `--fix-cortex-a53-843419` | as `arch.md` | as `arch.md` (L-117) |
| `-e`, `--entry` | as is | L-102 to L-104 |

### 20.3 `plumb.md`

New structure rules S13-S18 and T1, and Y2 (§17). In detail:

- **S13:** `PT_PHDR` and `PT_INTERP` come before all `PT_LOAD`s, at
  most one of each, and are covered by a `PT_LOAD`. The phdr table is in
  a `PT_LOAD`.
- **S14:** a NOBITS allocated section (not `.tbss`) lies beyond
  `p_filesz`.
- **S15:** `sh_addr ≡ 0 (mod sh_addralign)`.
- **S16:** RELRO safety (L-8) for P ∈ {4, 16, 64} KiB up to the
  `PT_LOAD` `p_align`.
- **S17:** `PT_GNU_PROPERTY` covers exactly `.note.gnu.property`.
- **S18:** non-allocated sections are outside `PT_LOAD` file images.
- **T1:** the end of every `PT_LOAD`'s memory is a section's end. This
  is an anomaly for references.
- **Y2:** the start-up symbol contract (L-13 to L-15).

A new note, **A7 (lld):** the RELRO end is aligned to 4 KiB with a
64 KiB `p_align`. Under 64 KiB pages the last part of RELRO stays
writable. That is a loss of protection, not of correctness: S16 still
holds, because L-70 puts the next segment in a later page (WP-024-spec
§3). A7 is reported as a note, never as a finding (Q1).

### 20.4 `diag.md`

New kinds (§3.1 table rows):

| id | variant | message | wording from |
|---|---|---|---|
| `section-conflict` | `SectionConflict { name, first, second }` | `section {name}: {first} and {second} inputs cannot be combined` | own (lld: `section type mismatch for {name}`) |
| `output-too-large` | `OutputTooLarge { what, limit }` | `output too large: {what} exceeds {limit}` | own (lld: `output file too large`) |
| `executable-stack` (warning) | `ExecutableStack { file }` | `{file}: requires executable stack (because the .note.GNU-stack section is executable)` | GNU ld |
| `internal-error` | `InternalError { what }` | `internal error: {what}; please report this` | own (WP-020 follow-up: replaces `Malformed` for an invalid layout) |
| `link-warning` (warning) | `LinkWarning { text }` | `{text}` (the `.gnu.warning` text, at the referencing place) | GNU ld |

`first`/`second` in `section-conflict` are the class words "TLS",
"non-TLS", "allocated", "non-allocated", or the `SHT_` names.

New refusal names (class `feature` unless noted):

- `.gnu.linkonce sections`;
- `input section <name>`;
- `.ctors section`;
- `init_array priority`;
- `TLS common symbol`;
- `code section larger than branch reach`;
- `entry point outside code`;
- `_TLS_MODULE_BASE_ without TLS`;
- class `compression-type`: `.zdebug`;
- class `section-type`: `SHT_AARCH64_ATTRIBUTES`;
- class `section-flag`: `SHF_X86_64_LARGE`.

### 20.5 `arch.md`

- A `Facts.code_fill: &[u8]` (the trap pattern) and a
  `Facts.code_nop: &[u8]` (L-82).
- The island spacing comes from `reach` and `margin` of the branch kind
  (L-110). arch states `margin`.

### 20.6 `synth.md` (not yet written)

- `.relro_padding` is a layout chunk, not synthetic content.
- `.eh_frame` keeps input order (L-58).
- The IPLT goes in `.plt`/`.rela.plt`/`.got.plt` for static links (arch
  Q9).

## 21. Where the references differ

| point | GNU | lld | mold | mortar | why |
|---|---|---|---|---|---|
| PT_LOAD shape | R E + RW | R, RX, RW-RELRO, RW | same as lld | lld | L-65 |
| RELRO end alignment | maxpage (by moving RW start, file gap) | common page (4 KiB) | maxpage (padding) | maxpage (padding) | L-8, L-64 |
| next segment address | page gap | L-70 | L-70 | L-70 | contiguous file |
| separate-code default | AArch64 no, x86-64 yes | no | no | no | Q2 |
| base, AArch64 | 0x400000 | 0x200000 | 0x200000 | 0x400000 | L-68 |
| rodata vs text order | text first | rodata first | rodata first | rodata first | L-62 |
| `.text` input order | categories | input order | input order | categories | L-53 |
| `.ctors` | into init_array, reversed | separate, not run | as GNU | as GNU (M2), refused (M1) | L-56 |
| `.preinit_array` vs `.init_array` | pre first | after | pre first | pre first | L-62 |
| merged sections | one output | one output, MERGE kept | one per class | one output, MERGE dropped if mixed | L-50 |
| PT_PHDR | with INTERP | always | always | with INTERP | L-67 |
| PT_GNU_STACK p_align | 16 | 0 | 1 | 16 | L-67 |
| exec `.note.GNU-stack` | RWX, warning | ignored | warning, RW | RWX, warning | L-84 |
| `_end` | aligned 8 | exact | exact | exact | L-95 |
| `__preinit_array_*` absent | location counter (in TLS) | base, index 1 | undefined | base, index 1 | L-93 |
| `__dso_handle` | crt | defines | crt | crt | L-99 |
| `__start_`/`__stop_` gc | keep | discard | keep | keep | L-96 |
| `__start_` visibility | protected | protected | local | protected | L-91 |
| `__rela_iplt_*` in PIE | undefined | undefined | defined | undefined | L-94 |
| `.tdata` alignment | raised to TLS max | own | own | own | L-78 |
| undefined entry | start of .text, warning | 0, warning | not observed | error | L-103 |
| `.init`/`.fini` vs `.text` | `.init`, `.plt`, `.text`, `.fini` | `.text`, `.init`, `.fini`, `.iplt` | `.init`, `.text`, `.fini` | GNU | L-62 |
| `.gnu.warning*` | consumed | copied as orphans | one `.gnu.warning` | consumed | L-33 |

## 22. Open questions (ranked by how much depends on them)

1. **Q5. GNU's bases or lld's?** This draft says GNU's (L-68). It
   affects every expected address in tests.
2. **Q2. Separate code by default.** GNU defaults to separate-code on
   x86-64, as hardening against gadget pages; lld and mold do not. The
   choice affects every x86-64 output's size and layout (M3). This draft
   says no on every target.
3. **Q7. `.text` categories.** GNU's grouping (this draft) or lld's plain
   order? It changes `.text` order in every link with `-ffunction-sections`.
4. **Q3. The padding section's name.** `.relro_padding` (lld/mold, known
   to llvm-objdump) breaks eu-elflint `--gnu-ld`. The alternatives are
   another name, or reporting the bug to elfutils. It affects tool
   checks in the harness.
5. **Q4. Bounds at size or capacity** for packed sections under
   incremental linking (L-142). It decides the M7 dependency index for
   symbol values.
6. **Q6. Undefined entry: error or GNU's warning?** (L-103.) It affects
   tests that link without crt files.
7. **Q9. Exec `.note.GNU-stack`:** warn and make the stack RWX (GNU,
   this draft), or refuse? mold warns and stays RW, which can crash
   programs that need it.
8. **Q1. RELRO padding to maxpage.** mortar pads RELRO to 64 KiB on
   AArch64 (L-64), as GNU and mold do and lld does not. It costs address
   space only. Is full protection under 64 KiB pages worth that, or
   should mortar follow lld? Only RELRO protection depends on it, never
   correctness (A7).
9. **Q8. The incremental reserve percentage**, and whether it is per
   output section (gold) or per chunk.

## 23. Tests

The existing link tests cite this spec by section title: "Segments"
(§11), "Thread-local storage" (§11.4), "Initialization and
termination" (§9.1), "Garbage collection" (§13.2), "Entry point"
(§13.6) and "Static executables" (§16). This draft adds no tests (WP-024 stage 2).
The test plan per rule:

- for each [G] rule, a mutation in `plumb.md` §9.2;
- for each [C] rule, a link test that checks `readelf` output, run with
  every reference that agrees and with `skip` for those that differ (the
  table in §21);
- `Layout::check` unit tests for L-125 to L-129.
