# spec: layout (`mortar_layout`)

Status: draft (WP-024 stage 1), amended after the review of 2026-11-14 (WP-024-spec §5). The evidence for each non-obvious rule is
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
is ≥ `x`. "Rank" is the position class of an output section (§10). An
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
  two must agree modulo the page; the gABI states the congruence modulo
  `p_align`. mortar's outputs must load on 64 KiB page kernels on
  AArch64, so mortar's `p_align` is at least maxpage (L-67, L-70, L-74).
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
- **L-6 [C]** No `PT_LOAD` is both `PF_W` and `PF_X` unless a linker
  script asks for one (`-z execstack` concerns the stack, not a
  `PT_LOAD`). The references never make one in a default link. This is
  hardening, not correctness: the loader accepts RWX segments, so the
  verifier reports it on other linkers' outputs as a note only.
- **L-19 [G] (ET_DYN alignment)** In an `ET_DYN` output (PIE, shared
  object), every allocated section's `sh_addralign` is at most the
  `p_align` of its `PT_LOAD`. The kernel chooses the load bias of an
  `ET_DYN` file aligned to the largest `PT_LOAD` `p_align`, and no more.
  An `_Alignas(0x40000)` object in a PIE whose `p_align` was patched
  back to 0x10000 loaded at 0xffffb2590000 and 0xaaaab7830000, neither
  0x40000-aligned (review of 2026-11-14, finding 4). In `ET_EXEC` the
  addresses are absolute and L-5 suffices.

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
  from 0x710 to 0x1d8 and left `PT_GNU_RELRO` at 0x710 (E-14). The
  stripped file then breaks L-7: RELRO reaches memory no `PT_LOAD`
  maps, and glibc's `mprotect` of it fails. This is why lld and mold emit
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
- **L-23 [G]** `PT_TLS` has `p_vaddr ≡ 0 (mod p_align)`. The runtime
  computes the executable's TLS offsets from `p_align` alone (variant 1:
  TP + align_up(16, p_align) + offset, `spec/a64.md` §12.2), and the
  initialisation image is copied to a block aligned to `p_align`. A
  template whose start is not aligned shifts every TLS variable relative
  to the offsets the linker baked into code. lld and mold place the
  first TLS section at a file offset and an address that are both
  multiples of `p_align` (review finding 5); `check.rs` `tls()` already
  requires it.
- **L-12 [C]** `.tbss` takes no space in the address map of the
  `PT_LOAD`s. Its `sh_addr` is the TLS template address it would have,
  which overlaps the next section, so overlap rules exempt it (S4 and S6
  already do). All three references do this, but nothing requires it:
  a linker may give `.tbss` address space, which only wastes it.

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

- **L-16 [G]** At most one `PT_GNU_STACK` (S10). Which flags it has, and
  whether it is present, are choices (L-84).
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
| D. address (§11) | C, `Facts`, the patch set | `addr`, `offset`, `segments`, `tls`, `shndx`; the alignment gaps, the 843419 start padding (L-118) and the size of `.relro_padding` (L-64) | sequential over chunks (O(chunks)) |
| E. fixed point (§14) | D, the relocations through `spec/arch.md` | `patches`; then D again | sites, by section |
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
- **L-21 [C] (what depends on addresses)** The split is:
  - **Address-independent (stages A to C):** the assignment, the input
    order, which chunks exist (patch chunks included, empty or not), the
    body sizes and alignments of every chunk that is not a patch chunk,
    a `.relro_padding` or in `sized`, and the offsets of sections
    within chunks.
  - **Outputs of stage D:** every address and file offset, the
    alignment gaps between chunks, the 843419 start padding before a
    synthetic code chunk (L-118), and the size of `.relro_padding`
    (L-64). Each is computed in one sequential pass over `order` from
    the stage C output and the sizes of the patch chunks.
  - **Inputs of stage D that stage E changes:** the patch set. A patch
    chunk's size is the sum of its patches' sizes, with their
    alignments, in the island order of L-115, so it is a function of the
    patch set.

  Stage D is therefore a pure function of (stage C output, patch set),
  which is `spec/arch.md` §6's premise "addresses are a function of the
  patch set". Nothing in it needs to be monotone: a padding may shrink
  when a patch is added (arch.md §6, "Termination", gives the
  counterexample), and termination does not use distances. At M2,
  `.relr.dyn` (rank 4, before the code) has a size that depends on
  addresses. That makes the state (patch set, sizes in `sized`), with
  each `sized` size only growing (L-121). This is a request to arch.md
  §6 (§20.5); at M1 `sized` holds no allocated chunk before the end of
  the image, so the premise holds as stated.
- **L-22 [C] (parallel form)** The offsets of sections within a chunk
  are the aligned prefix sum of their sizes, in the order of stage B:
  offset₀ = 0, offsetᵢ₊₁ = align_up(offsetᵢ + sizeᵢ, alignᵢ₊₁). A
  chunk's alignment is the maximum alignment of its members, so offsets
  stay valid at any chunk address with that alignment. An aligned sum is
  not an additive prefix sum, so it is not `mortar_core::par`'s
  `Pool::prefix_sum` directly. The parallel steps are:
  - the gather of each output section's members (a stable partition,
    model §6 rules 3 and 4), parallel over files;
  - the per-chunk scan, sequential within a chunk and parallel across
    chunks;
  - the bucket offsets of the partition and the total size per output
    section, by `Pool::prefix_sum` over `OutSecId` order;
  - the writes of `place`, disjoint slices per chunk (model §6 rule 1).

  A blocked form of the in-chunk scan (per block: its size for each
  residue of the starting offset modulo the block's largest alignment,
  then a short sequential combine) makes a 10⁶-section `.text`
  parallel. It is an implementation option for WP-027, which measures
  it; the sequential scan is one pass over 16 bytes per section.

## 8. Output sections (stage A)

### 8.1 Sections that go nowhere

The table below is checked from the top; the first row that applies
wins.

| input section | outcome | rule |
|---|---|---|
| index 0, `SHT_NULL`, `SHT_SYMTAB`, `SHT_STRTAB`, `SHT_RELA`/`SHT_REL` (and `SHT_SYMTAB_SHNDX`, `SHT_GROUP`) | consumed by the reader, never assigned | L-30 |
| `SHF_EXCLUDE`, any type | dropped. Clang's `.llvm_addrsig` is the common case (E-11) | L-31 |
| `.note.GNU-stack` | consumed: decides `PT_GNU_STACK` (§11.6) | L-32 |
| `.gnu.warning`, `.gnu.warning.SYM` | consumed: no output section. The text is resolve.md's `gnu-warning` (§10 there): for `.gnu.warning.SYM`, once, when a live input other than the defining file references `SYM`; for plain `.gnu.warning`, when its file is linked in (GNU). Static glibc's `libc.a` carries `.gnu.warning.dlopen` and others in members every static link extracts. lld copies them into the output as orphans, mold into one `.gnu.warning`, GNU consumes them; none warns unless the symbol is used (E-17) | L-33 |
| `.note.gnu.property` | consumed by synthesis (`spec/arch.md` / `synth.md`), which builds the merged property note | L-34 |
| `SHT_AARCH64_ATTRIBUTES` (`.ARM.attributes`, AArch64) | accepted and consumed: no output section. Feature marking comes only from the AND-merged `.note.gnu.property` (L-34). Clang 22 emits both for every `-mbranch-protection` value but `none`; GCC 14 and Clang 19 emit only the note (E-18). GNU drops the section, lld copies it, mold drops it and emits no property note at all (E-18) | L-35 |
| `SHF_X86_64_LARGE` set | refused (`section-flag`, `SHF_X86_64_LARGE`) [M3] | L-36 |
| `.gnu.linkonce.*` | refused (`feature`, `.gnu.linkonce sections`): pre-COMDAT, and no current compiler emits it | L-37 |
| `.zdebug_*` | refused (`compression-type`, `.zdebug`). Only `SHF_COMPRESSED` is accepted | L-38 |
| a name that a synthetic section owns (`.got`, `.got.plt`, `.plt`, `.iplt`, `.rela.dyn`, `.rela.plt`, `.dynamic`, `.dynsym`, `.dynstr`, `.hash`, `.gnu.hash`, `.eh_frame_hdr`, `.relr.dyn`, `.interp`) | refused (`feature`, `input section <name>`). `.interp` becomes accepted at M2, when it decides the interpreter as in GNU | L-39 |
| `.ctors*`, `.dtors*`, non-empty | M1 refuses (`feature`, `.ctors section`); the rule from M2 is L-56 | L-40 |
| an `sh_type` or flag that `resolve.md` §10 ("The fate of sections") refuses | refused there (`section-type`, `section-flag`); layout never sees it. resolve.md owns the classification; this table only adds layout's outcome for the classes it accepts | L-41 |
| not live (`Liveness`), or in a discarded COMDAT group | given an output section like any other, but no place | L-42 |

- **L-43** Dead and discarded sections still get an `out_of` entry, so
  the assignment never depends on liveness. Their output section can
  have zero live members. An output section with no live member and no
  synthetic content produces no chunk. The exceptions are a section that
  a script `KEEP`s and a section that a linker-defined symbol names
  (`__start_SEC`, §13): those get an empty chunk, so the symbol has a
  place.
- **L-35 detail.** Dropping `.ARM.attributes` is safe: the AArch64
  kernel and glibc read BTI and GCS from `PT_GNU_PROPERTY` only, and the
  build attributes duplicate the same feature bits (Tag_Feature_BTI,
  _PAC, _GCS in the `aeabi_feature_and_bits` subsection). What is lost
  for an object that carries only build attributes and no property note
  (no compiler on the matrix makes one, E-18): its BTI/PAC/GCS claim is
  not seen, so the AND-merge treats it as having no features, and the
  output is marked with fewer features. That is the safe direction: a
  feature is never claimed for code that did not claim it. Also lost:
  the attributes section in the output, which no loader reads. The full
  build-attributes merge (psABI "Build Attributes"), with the output
  section kept, is a later milestone (after M3); it needs `spec/a64.md`
  to define it. resolve.md §10 must change from refusing the type
  (§20.7).

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
  under one name is wrong with either choice. Only live inputs count:
  two dead sections, or a live and a dead one, never conflict. The
  assignment stays by name (L-43), so a dead section is still assigned
  to the output section of its name, and it contributes nothing to that
  section's type, flags or alignment (L-49 to L-51 read live inputs
  only). The review's library scan and E-19 found no such mixture in any
  compiler output or runtime library on the three targets.
- **L-48 [C] (orphans)** An orphan output section keeps the input name.
  Its rank (§10) comes from its type and flags: TLS, writable, exec,
  `NOBITS`, alloc. An orphan whose name is a C identifier gets
  `__start_`/`__stop_` symbols when they are referenced (§13.1, §13.2).
  Registration sections reached only through those bounds are the
  common use of orphans: systemd, the Linux tools, and older glibc
  (`__libc_atexit`, `__libc_subfreeres`; the glibc of the test host
  has none, E-17).

### 8.3 The output section's type and flags

- **L-49 [C]** An output section's `sh_type` is the type its live inputs
  share, with these compatible mixtures:
  - `SHT_PROGBITS` and `SHT_NOBITS` give `PROGBITS`, with the `NOBITS`
    inputs written as zeros;
  - the special array types (`SHT_INIT_ARRAY`, `SHT_FINI_ARRAY`,
    `SHT_PREINIT_ARRAY`) accept `PROGBITS` inputs named for them (old
    compilers emit `.init_array` as `PROGBITS`); the output type is the
    special type;
  - `.eh_frame` on x86-64: `SHT_X86_64_UNWIND` (Clang 19 and 22) and
    `SHT_PROGBITS` (GCC, its crt files and libgcc) are compatible. The
    output is `SHT_X86_64_UNWIND` only when every live input is, and
    `PROGBITS` otherwise. That is GNU 2.44's rule, observed: a clang-only
    link gives UNWIND, a clang link with GCC's crt files gives
    `PROGBITS`. lld and mold always give `PROGBITS` (E-19). All three
    run the mixed program (E-19);
  - `SHT_NOTE` stays `SHT_NOTE` (notes are not merged by name, L-45).

  Every other mixture is refused (`section-conflict`). The scan of E-19
  (210 objects: C, C++ and, on AArch64, Fortran, from GCC 14, Clang 19
  and Clang 22 in ten flag sets each, on AArch64, x86-64 and i386; with
  the crt files, libgcc, libgcc_eh and, on x86, libc.a)
  found no other mixture: the x86-64 `.eh_frame` was the only one. A
  refused mixture is therefore one no compiler on the matrix produces.
- **L-50 [C]** Output flags are the OR of the live inputs' `SHF_WRITE`,
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
  section. The grouping affects every `-O2` GCC link, not only
  `-ffunction-sections` ones: GCC puts `main` in `.text.startup` and
  cold paths in `.text.unlikely` at `-O2` (Q7, decided).
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
    segment. Only under `-z separate-code` (L-71) does that also mean
    that no code page is readable as data and no data page executable.
    By default, file offsets are contiguous (L-70), so the R segment's
    last page maps the first code bytes and the RX segment's first page
    maps the last rodata bytes as executable. lld and mold have the same
    property; the reviewer's pagesim confirmed it on spec-shaped
    binaries.
  - `.eh_frame` is last in rank 5, next to the code it describes,
    following lld. GNU puts `.gcc_except_table` and `.note.ABI-tag`
    after `.eh_frame`.
  - `.fini` is last in RX, so `etext` is the end of `.fini` as in GNU
    (E-6).
  - `.got.plt` in a static link holds only IRELATIVE slots. It stays
    writable (rank 8), as in all three references. glibc applies
    IRELATIVE before it protects RELRO, but that ordering was not
    confirmed for static start-up, so mortar does not depend on it
    (unverified, WP-024-spec §4).
  - `.preinit_array` comes before `.init_array`, as in GNU. lld puts it
    after; nothing depends on that.
  - `.tm_clone_table` is in RW because libgcc's `crtbegin` writes
    nothing to it but GNU puts it there. It is kept as is.
- **L-63 [C]** The section header table comes last. The `.symtab`,
  `.strtab` and `.shstrtab` order follows GNU, lld and mold.
- **L-64 [C] (relro padding)** When rank 7 is non-empty, a NOBITS chunk
  named `.relro_padding` (L-156) ends rank 7. Its size makes the RELRO
  end a multiple of maxpage: `align_up(end, maxpage) − end`, where `end`
  is the end of the chunk before it. Stage D sets the size (L-21); it is
  the only chunk whose size stage D writes. The chunk exists whenever
  rank 7 is non-empty, with size 0 when the end is already aligned, so
  which chunks exist stays address-independent. It has
  `SHF_ALLOC|SHF_WRITE`, alignment 1, is part of the RELRO run, and
  takes no file space. lld pads only to the common page (4 KiB), which
  on a 64 KiB-page kernel leaves the last RELRO page writable (E-1).
  mold and GNU pad to maxpage. GNU does it by moving the RW segment's
  start instead, which costs file space. Padding to maxpage costs
  address space only, and buys full RELRO protection under 64 KiB pages
  (Q1, decided).

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
| `PT_LOAD` ×1-4 | L-65 | | `P` = max(maxpage, the largest alignment of a chunk in the segment) |
| `PT_DYNAMIC` | dynamic (M2) | `.dynamic` | word |
| `PT_NOTE` ×n | allocated notes | one per run of consecutive note chunks with the same alignment | that alignment |
| `PT_TLS` | a TLS chunk | §11.4 | max TLS alignment |
| `PT_GNU_PROPERTY` | `.note.gnu.property` | it | 8 / 4 |
| `PT_GNU_EH_FRAME` | `.eh_frame_hdr` | it | 4 |
| `PT_GNU_STACK` | always (L-84) | nothing (all zero but flags and `p_align`) | 16 |
| `PT_GNU_RELRO` | rank 7 non-empty | rank 7 including padding | 1 |

  - `PT_LOAD` `p_align` is `P`, so L-1 and L-19 hold for every section,
    and an `ET_DYN` output gets a load bias aligned for its most aligned
    section. All three references raise `p_align` the same way: an
    `_Alignas(0x40000)` object gives `p_align` 0x40000 for the segment
    that holds it, in GNU, lld and mold (E-21).
  - `PT_GNU_RELRO`: `p_offset`/`p_vaddr` are the first RELRO chunk's,
    `p_memsz` runs to the end of `.relro_padding` (a multiple of
    maxpage), and `p_filesz` runs to the end of the last RELRO chunk
    that is in the file (`p_filesz` < `p_memsz` when the run ends in
    NOBITS chunks; it may be 0). This is lld's and mold's value; GNU
    gives `p_filesz = p_memsz` in the usual static link, and less in the
    `.tbss`-only one (E-22).
    The loader reads only `p_vaddr` and `p_memsz`.
  - The order is GNU's (E-9). lld puts `PT_NOTE` last; mold puts it
    before the `PT_LOAD`s. Nothing reads the order beyond L-2.
  - GNU emits `PT_PHDR` only with an interpreter, which is why the rule
    is tied to `PT_INTERP`. lld and mold emit it always (E-9). glibc's
    `rtld` and the kernel need it only when there is an interpreter.
  - `PT_NOTE` per alignment run follows GNU. The gABI requires a note
    segment's notes to share one alignment for parsing (`plumb.md` S11).
  - The `PT_GNU_STACK` `p_align` of 16 follows GNU (lld 0, mold 1; E-20).
  - The `PT_GNU_RELRO` `p_align` of 1 follows all three.

### 11.2 Addresses and offsets

- **L-68 [C]** The first chunk (file header) has offset 0 and address
  `base` (non-PIE: `align_down(Facts.image_base, P₁)`, where `P₁` is the
  `p_align` of the first `PT_LOAD` (L-67); PIE and shared: 0, M2). With
  the default maxpage and no section aligned beyond it, `P₁` divides the
  table's bases, so the base is the table's. GNU rounds the same way
  with a larger `-z max-page-size` (E-21): i386 0x08048000 becomes
  0x08040000 at 64 KiB and 0x08000000 from 2 MiB; AArch64 and x86-64
  keep 0x400000 up to 4 MiB and get 0 at 8 MiB. A non-PIE base that
  rounds to 0 (a maxpage or a section alignment of 8 MiB or more on the
  64-bit targets) is refused (`feature`, `non-PIE image base 0`),
  because the kernel's `vm.mmap_min_addr` forbids a fixed mapping of
  page 0. GNU writes such a file; that it fails to load was not run
  (WP-024-spec §4). Table of bases (E-2):

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
  `addr = align_up(prev_end + pad_k, a_k)` and the same displacement in
  the file: `offset = prev_offset_end + (addr − prev_end)`. Here `a_k`
  is the chunk's *effective alignment*: its own alignment, except that
  the first TLS chunk's is `T` (L-76); `pad_k` is the 843419 start
  padding (L-118), 0 for every other chunk. NOBITS chunks advance the
  address only. The file never contains bytes for a NOBITS chunk, and a
  PROGBITS chunk never follows a NOBITS one within a segment: the rank
  table puts NOBITS last in each segment. `.tbss` is the exception,
  because it takes no address space (L-77). A layout with a PROGBITS
  chunk after a NOBITS one fails `Layout::check` (`internal-error`). A
  NOBITS section's `sh_offset` is `prev_offset_end`, undisplaced, as lld
  and mold give it (a displaced value works as well; no tool reads it).
- **L-70 [C] (next segment)** The first chunk of a new `PT_LOAD`, with
  effective alignment `a` and the segment's `p_align` `P` (L-67), gets
  the file offset `o = align_up(file_end, a)`, where `file_end` is the
  end of the previous chunk in the file, and the least address `A` with
  `A ≥ align_up(prev_segment_mem_end, maxpage)` and `A ≡ o (mod P)`.
  Since `a` divides `P` and `o`, `A ≡ 0 (mod a)` follows. File offsets
  stay contiguous, and no two segments share a page in memory at any
  page size up to maxpage. This is lld's and mold's rule, observed
  exactly (E-1): `0x227b60 → 0x237b80` with offset `0x27b80`. A model
  of L-67 to L-78 as amended, run on the review's test links (a
  0x40000-aligned section, TLS aligned to 64, TLS of `.tbss` only),
  gives layouts that satisfy L-1, L-5, L-8, L-23 and the RELRO end at
  every page size (WP-024-spec E-23).
- **L-71 [C] (separate code)** With `-z separate-code`, the RX segment's
  file offset and the next segment's offset are each rounded up to
  maxpage, and the gap is filled with zeros (exec: trap pattern, L-82).
  No page of the file then holds both code and other bytes. The default
  is `-z noseparate-code` on every target (Q2, decided). That is GNU's
  default on AArch64 and lld's and mold's everywhere; GNU defaults to
  separate-code on x86-64 and i386, with four segments R, R E, R, RW
  (E-1, review; §21).
- **L-72 [C]** Non-allocated chunks follow the last allocated chunk's
  file end, each at `align_up(offset, align)`, with no address (rank
  9-11). The section header table is aligned to the word size.
- **L-73 [C]** An address, an end address or an offset that does not fit
  the class (ELFCLASS32: an end beyond 2³²; ELFCLASS64: beyond 2⁶⁴) is
  refused by name, `output-too-large` (§20.4), never a wrapped value.
  The limit is the class's. A kernel's split of the address space (the
  3 GiB `TASK_SIZE` of a 32-bit kernel, the 48-bit user space of
  AArch64) is not the linker's concern: the same file runs on another
  kernel or under qemu. The review saw mold write an i386 `PT_LOAD`
  that wraps (`p_memsz` 0xffe03c60) and segfaults; GNU refuses with
  "section .bss VMA wraps around" and lld with "exceeds available
  address space".

### 11.3 Page sizes

- **L-74 [C]** maxpage defaults to `Facts.max_page`: 64 KiB on AArch64,
  4 KiB on x86-64 and i386. `-z max-page-size=N` sets it, and accepts
  what GNU 2.44 accepts (E-21) from 4096 up: any power of two, 2 MiB and
  4 MiB included, with no upper limit but L-68's base rule. `N = 0`
  means the default. A non-power of two, such as 0x3000, is
  `invalid-option-value` (GNU: "invalid maximum page size `0x3000'").
  GNU also accepts powers of two below 4096, down to 1 (its `p_align`
  then follows the largest section alignment). mortar refuses them
  (`invalid-option-value`): with contiguous file offsets the output
  would break L-1 under every Linux page size.
  The base then follows L-68's rounding, as GNU's does. lld accepts 2
  and 4 MiB; mold refuses 4 MiB because its base is not a multiple
  (review).
- **L-75 [C]** `-z common-page-size=N` is accepted and validated (a
  power of two ≤ maxpage). It changes nothing in mortar's layout, because
  every decision that GNU or lld takes at the common page, mortar takes
  at maxpage. That costs at most maxpage of address space per segment,
  and no file space.

### 11.4 Thread-local storage

- **L-76 [C]** TLS chunks are the first chunks of rank 7: `.tdata` (all
  TLS `PROGBITS`), TLS `PROGBITS` orphans, then `.tbss` and TLS `NOBITS`
  orphans. Let `T` be the maximum alignment of every TLS chunk. The
  first TLS chunk's effective alignment (L-69, L-70) is `T`, even when
  its own alignment is smaller, so its file offset and its address are
  both multiples of `T` (lld and mold do the same; for a `NOBITS` first
  chunk the offset is L-70's `o`, which becomes `PT_TLS` `p_offset`). Since the first TLS
  chunk is the first chunk of the RELRO segment, L-70 gives it
  `o = align_up(file_end, T)` and `A ≡ o (mod P)` with `T | P`. `tls.start`
  is that address, so L-23 holds (`check.rs` `tls()`, model §4.4
  rule 3).
- **L-77 [C]** `.tbss` gets `addr = align_up(tdata_end, align_tbss)` and
  `offset = tdata_offset_end`. It takes no address space in the
  `PT_LOAD`s, so the next chunk (`.preinit_array` or whichever comes
  next) starts at `tdata_end`, aligned to its own alignment, not after
  `.tbss`.
  - **TLS of `.tbss` only** (no TLS `PROGBITS`): `.tbss` is the first
    chunk of the RELRO segment, placed by L-70 with effective alignment
    `T`, and the next chunk starts at the same address, aligned to its
    own alignment. This is GNU's placement (E-22: `.tbss` and
    `.init_array` both at 0x41ffc0, `PT_TLS` offset 0xffc0, `p_filesz`
    0, `p_memsz` 0x18, `p_align` 0x40), and it satisfies L-10, L-23 and
    L-1. lld 22 puts `.tbss` at an address inside the RX range, mold
    after the start of `.init_array`; both are valid, neither is
    simpler. `PT_TLS` `p_offset` is L-70's `o`, congruent to the address
    as in GNU; the `.tbss` section header's `sh_offset` is the
    undisplaced file end (L-69).
- **L-78 [C]** `tls = {start, size, align}` with `size` = the end of the
  last TLS chunk − start, the exact extent. `align = T`. `PT_TLS` has
  `p_vaddr = start`, `p_memsz = size`, `p_filesz` = the end of the last
  TLS `PROGBITS` chunk − start (0 when there is none), and
  `p_align = T`. The section `.tdata` keeps `sh_addralign` = its own
  maximum (lld), not T (GNU raises it, E-7). The difference has no
  effect on the loader.
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
  - `.got.plt` has three reserved words (`arch.md`
    `got_base: GotPltStart`), in static links too, when it exists
    (L-101);
  - i386 uses REL, so `.rel.dyn`/`.rel.plt` replace the RELA names.
  No rule here names a machine except through `Facts`.

### 11.6 The stack — `PT_GNU_STACK`

- **L-84 [C]** mortar always emits one `PT_GNU_STACK`, `p_align` 16,
  everything else 0 but the flags. Its flags follow GNU ld 2.44, which
  behaves the same on AArch64, x86-64 and i386, with bare `ld` and
  through the driver (E-10, E-20):

  | inputs' `.note.GNU-stack` | no option | `-z execstack` | `-z noexecstack` |
  |---|---|---|---|
  | none in any input | RW (GNU: no `PT_GNU_STACK`) | RWX | RW |
  | all non-exec (`""`), or some non-exec and some absent | RW | RWX | RW |
  | one or more with `SHF_EXECINSTR` (`"x"`) | RWX, warning `executable-stack` | RWX, no warning | RW, no warning |

  - The input set is every object file linked in, live or not, as in
    GNU (the note is a property of the file, not of a section).
  - One warning per link, naming the first such file in load order, in
    GNU's words (GNU 2.44 does the same with two such files, E-20):
    `{file}: requires executable stack (because the .note.GNU-stack section is executable)`.
    GNU prints `ld: warning: x.o: requires executable stack (because the
    .note.GNU-stack section is executable)`.
  - The one difference from GNU: with no note in any input, GNU emits
    no `PT_GNU_STACK`, and the kernel then applies its default, which
    differs by architecture (an executable stack on i386 through
    `READ_IMPLIES_EXEC`; not on x86-64 since Linux 5.8 or on arm64; from
    the kernel source, not run). mortar emits RW, so the stack does not
    depend on the kernel. lld and mold also emit RW there. Through the
    driver the crt files carry `""` notes, so every driver link agrees
    with GNU.
  - GNU's `--warn-execstack` is its default; its `--error-execstack`
    family is refused until requested (`cli.md`). lld 22 errors on an
    exec note without `-z execstack`; lld 19 ignores the note; mold
    warns and keeps RW (§21).

## 12. Chunks (stage C) and padding

### 12.1 Which chunks

- **L-85 [C]** Each output section with live content becomes chunks in
  `order`:
  - `Body(OutSecId, part)` for the placed input sections, in stage B
    order, cut into parts (L-110). There is one part unless the output
    section is exec and larger than the part limit;
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
  runs from there to the end of the last chunk's `size`, never its
  capacity: no parser ever sees slack through a section header (L-141).
  Section indices are given densely in chunk order (`check.rs`), header
  chunks aside.
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
  This is a choice. GNU's AArch64 script fills `.init`, `.text` and
  `.fini` with `NOP` (`=0x1f2003d5` in `ld --verbose`), so a gap inside
  `.text` is a slide into the next function; GNU's x86 scripts give no
  fill (zeros, which decode as `add` on x86). lld's and mold's fill was
  not examined. mortar's trap fill turns a fall into a gap into a crash
  at the gap.
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
- **L-92 [C]** "End" means the end of the content, the last chunk's
  `size`, never its capacity: every end symbol (`etext`, `_edata`,
  `_end`, `__init_array_end`, `__stop_SEC`, ...) bounds exactly what the
  section header bounds (L-86). Slack is never inside any symbol's range
  or any header's (L-141). With the default policy size and capacity are
  equal, so the choice is invisible until M7. Growing a section at M7
  therefore can change an end symbol's value, and the dependency index
  records the symbol's users (Q4, open).

### 13.1 The table (static non-PIE, M1)

| symbol | value | section index | GNU | lld | mold |
|---|---|---|---|---|---|
| `__ehdr_start` | base (file header) | 1 | same | same | same |
| `__executable_start` | base | 1 | same | same, local hidden | same, local |
| `__dso_handle` | not defined (crtbegin defines it) | | crt (undefined reference without it) | defines it, local hidden, at base, index 1 | defines it, local, at the start of the first section (E-24) |
| `__data_start` | AArch64 only: start of `.data` (absent: `edata`) | `.data` | `PROVIDE` in the AArch64 script only | no | no |
| `__tdata_start` | start of `.tdata`, hidden; defined only when `.tdata` exists | `.tdata` | `PROVIDE_HIDDEN` in all three scripts; defined at the location counter even with no `.tdata` | undefined weak | undefined weak |
| `etext`, `_etext`, `__etext` | end of the last exec chunk (`.fini`) | that section | `etext`, `_etext`, `__etext` | no `__etext` | no `__etext` |
| `edata`, `_edata` | end of the last allocated non-`NOBITS` chunk in address order (rank 8 `PROGBITS`) | that section | the location counter before `.bss`, which equals `__bss_start` (E-6) | same as mortar | same as mortar |
| `__bss_start` | start of `.bss` (of the first rank 8 NOBITS chunk); if there is none, `edata` | `.bss` | as `_edata` | same as mortar | same as mortar |
| `end`, `_end` | end of the last allocated chunk in memory, `.tbss` aside | that section | align_up(…, 8) | unaligned | unaligned |
| `__bss_start__`, `__bss_end__`, `_bss_end__`, `__end__` | `__bss_start`; `end` (same section); AArch64 only | | yes (AArch64 script) | no | no |
| `__preinit_array_start`/`_end` | bounds of `.preinit_array`; when absent, both = base, index 1 | | init_array start | base, 1 | undefined |
| `__init_array_start`/`_end` | bounds of `.init_array` (absent: base, 1) | `.init_array` | same | same | same |
| `__fini_array_start`/`_end` | bounds of `.fini_array` (absent: base, 1) | `.fini_array` | same | same | same |
| `__rela_iplt_start`/`_end` (`__rel_iplt_*` on i386) | bounds of the IRELATIVE run in `.rela.plt`; absent: both = start of rank 4 or base | `.rela.plt` | same | same | 0 when there is no IRELATIVE; defined also in PIE |
| `_GLOBAL_OFFSET_TABLE_` | `Facts.got_base` (L-101): AArch64, the start of `.got`; x86, the start of `.got.plt`. A reference demands `GOT_BASE`, so the section exists: on AArch64 `.got` gets its header word (`spec/a64.md` §10.1), on x86 `.got.plt` its three reserved words, static links included | that section | AArch64: `.got` 8 bytes and `.got.plt` 0x18; x86: 3-word `.got.plt` (E-24) | AArch64: an empty `.got`, symbol local hidden; x86: 3 words | AArch64 as GNU; x86: the 3 reserved words only (IRELATIVE slots elsewhere) |
| `__start_SEC`, `__stop_SEC` | start and end (`size`, L-92) of output section `SEC`, `SEC` a C identifier. When no input section is named `SEC`, the symbol stays undefined weak (value 0); a strong reference is then `undefined-symbol` | `SEC` | protected; absent: undefined | protected; absent: undefined | local; absent: undefined |
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
  input section named `SEC` live under `--gc-sections`. This is GNU's
  default, `-z nostart-stop-gc`, and mold's. lld 19 defaults to `-z start-stop-gc`: it discards the
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
  a runtime file: `__dso_handle`, `data_start`, `_init`, `_fini`. If
  they are missing, `undefined-symbol` reports it, as GNU does. lld and
  mold define `__dso_handle` themselves (E-24), which hides a missing
  crtbegin. `__data_start` is the exception, on AArch64 only, because
  GNU's AArch64 script `PROVIDE`s it (table above): mortar defines it
  there when it is referenced and no input defines it. On x86 mortar
  does not define it.
- **L-105 [C]** `__tdata_start` is defined (when referenced) as hidden,
  at the start of `.tdata`, only when `.tdata` exists. GNU's
  `PROVIDE_HIDDEN` defines it at the location counter even with no
  `.tdata`, a value that names no TLS data; lld and mold leave it
  undefined weak (E-24). mortar follows lld there.

### 13.5 x86 and dynamic additions [M2-M4]

- **L-100 [C] [M2]** In a shared object, `_end`, `_edata` and
  `__bss_start` are exported `STB_GLOBAL STV_DEFAULT` when they are
  defined (GNU and lld). `_DYNAMIC` is hidden.
- **L-101 [C]** `_GLOBAL_OFFSET_TABLE_` is tied to the `GOT_BASE`
  demand: a reference to the symbol demands `GOT_BASE` (`spec/arch.md`),
  and that demand creates the GOT header even when no GOT slot exists.
  - AArch64 (M1): `.got` with its one header word (`spec/a64.md`
    §10.1); a static `.got.plt` has no reserved words (§10.2 there). The
    symbol is the start of `.got`.
  - x86-64 and i386 (M3, M4): `.got.plt` with three reserved words, in
    static links as well; IRELATIVE slots follow them. The symbol is
    the start of `.got.plt`. GNU and lld do the same in static links
    (x86-64 `.got.plt` 0xc8 = 3 words + IRELATIVE slots; i386 0x40);
    mold gives 3 words only (E-24). x86's `crt1.o` and `crti.o`
    reference the symbol, so every x86 link has the header. Requested of
    `arch.md` (§20.5): the three reserved words in static links.
  Nothing else in §13 depends on the machine.

### 13.6 Entry point

- **L-102 [C]** `entry` is the value of the symbol named by `-e`/`--entry`
  (or `ENTRY` in a script), else `_start`. A number given to `-e` that
  is not a symbol name is accepted as an absolute address (GNU).
- **L-103 [C]** If the entry symbol is undefined in an executable,
  mortar warns with resolve.md's `entry-not-found`
  (`cannot find entry symbol {name}`, §5.5 there) and writes
  `e_entry = 0`; the link succeeds (exit status 0). `entry` is then
  none. This is lld's value and mold's (lld: "cannot find entry symbol
  _start; not setting start address"; mold: "entry symbol is not
  defined: _start"); GNU warns "cannot find entry symbol _start;
  defaulting to …" and uses the start of `.text`. mortar does not guess
  an address: a program with entry 0 faults at its first instruction,
  which is a clear failure, where a guessed start runs some code (Q6,
  decided: a warning, not an error). With `-shared` or `-r` and no
  `-e`, there is no entry symbol to find: `e_entry = 0`, silently.
  resolve.md §5.5 must change "the entry is layout's default (GNU's,
  the start of `.text`)" to `e_entry = 0` (§20.7).
- **L-104 [C]** A defined entry must lie in an exec chunk (`plumb.md`
  S7). An entry outside one is `internal-error` when mortar computed it,
  and a refusal (`feature`, `entry point outside code`) when `-e` names a
  data symbol. The carve-out: `e_entry = 0` after `entry-not-found`
  (L-103), and in `-shared` and `-r` outputs, is not an entry and is not
  checked; plumb.md S7 must say the same (§20.3).

## 14. Range extension and errata: the fixed point (stage E)

`spec/arch.md` §6 is the one normative text of the fixed point: its
state, its rounds (needs judged at the round's addresses `A_k`; a thunk
need reuses the in-reach thunk of its key with the lowest address; the
iteration stops when a round has no needs), its termination argument
(at most 2N + 1 rounds under condition (C)), its cap (more than 32
rounds is `refused`, class `feature`, name `range-extension fixed
point`, key (90, 0, 0)) and its determinism. Layout does not restate
them. This section adds only what arch.md §6 leaves to layout: the
island policy, which satisfies condition (C); the reach arithmetic,
with the island sizes; and the order of patches inside an island.

### 14.1 Islands

- **L-110 [C] (parts)** An exec output section is cut into body parts in
  stage B order. A new part starts before the input section that would
  make the current part larger than the part limit `S`:
  - AArch64: `S = 2²⁷ − 2²² − 32` bytes (124 MiB − 32), from L-112;
  - x86: no thunks, so one part.
  The cut uses sizes only, so it is address-independent (L-21). An input
  section larger than `S` is refused (`feature`, `code section larger
  than branch reach`).
- **L-111 [C] (islands, condition (C))** Each body part *k* is followed
  directly by its island `Patches(OutSecId, k)` (L-85). The islands are
  exactly these, one per exec body part; they exist from stage C on,
  before round 1, whether or not they ever hold a patch. When arch.md
  §6 step 3 needs a **new** thunk or erratum patch for a site in part
  *k*, it goes in island *k*. By L-112 island *k* is within reach of
  every site of part *k*, so:
  - a thunk need from part *k* never finds island *k*'s thunk of its key
    out of reach; a new thunk of a key is added to island *k* only when
    no thunk of that key is in reach, so island *k* holds at most one
    thunk per key;
  - an erratum site gets at most one patch in island *k*, and that
    patch never goes out of range, so the site is never re-patched;
  which is condition (C): `N ≤ islands × (thunk keys + erratum keys)`.
  Reuse is arch.md's alone: a site uses the in-reach thunk of its key
  with the lowest address, in any island (k−2, k+1, another output
  section's), not only island *k* or *k−1*.
- **L-112 [C] (reach arithmetic)** Let `C = 2²² = 4 MiB` be the island
  cap. Between a site in part *k* and any byte of island *k* lie at most
  the part (`≤ S`), the 843419 start padding before the island (L-118,
  ≤ 16 bytes), the alignment gap before the island (≤ 3 bytes, patch
  alignment 4) and the island (`≤ C`). So the distance from a site to a
  patch in its island is at most `S + 16 + 3 + C − 12 < 2²⁷ − 4`, the
  forward reach of `B`/`BL` (`s(28)`, `spec/a64.md` §9), and the
  patch's own branch back (`b F + 4`, a64.md §11.2) is within the same
  bound. An island whose patches would exceed `C` is refused (`feature`,
  `range-extension fixed point`, as arch.md §6's cap). With 12-byte
  thunks, `C` holds about 350 000 thunks per part. The thunk's own reach
  to its target (`s(33)` of its `ADRP`) is a64.md's
  (`relocation-out-of-range` beyond it).
- **L-113 [C] (fast path)** If the RX segment, with every island at its
  cap `C`, spans at most `2²⁷ − 4` bytes, no branch between two places
  in it is out of reach. Round 1 then collects branch needs only for
  targets outside the RX segment. This changes only the cost of
  collecting needs, never the result: a site the fast path skips is one
  arch.md §6 step 1 would not list. A typical link, under 120 MiB of
  code, has no range-extension need.

### 14.2 Patches

- **L-114** withdrawn (the iteration is arch.md §6's; review finding 7).
- **L-115 [C] (order in an island)** The patches of an island are in key
  order, a pure function of the island's patch set, never of the round
  that added them: first the thunks, by (kind, `SymKey`, addend) with
  `Sym` before `Plt`; then the erratum patches, by site key
  (`SectionId`, offset of the key instruction). Each patch is at
  `align_up(previous end, its alignment)` from the island's start. A
  patch added in a later round is inserted at its key's position, which
  may move the island's later patches; arch.md §6 step 1 catches any
  redirect that this moves out of reach. The island's size is the end of
  its last patch, so it too is a function of the patch set (L-21).
- **L-116 [C]** Unused patches are allowed (arch.md §6 "Monotone"; model
  §4.4 rule 4). Their bytes are a valid thunk or patch that nothing
  calls.
- **L-117 [C] (843419 sites)** With `--fix-cortex-a53-843419` (which
  Debian's GCC passes on every AArch64 link), the sites and the two
  fixes are `spec/a64.md` §11's: an `ADR` in place when the target is in
  `ADR` reach, else an erratum patch, which L-111 puts in the island of
  the site's own part.
- **L-118 [C] (843419 start padding)** With `--fix-cortex-a53-843419`,
  stage D never starts a synthetic code chunk of non-zero size (a patch
  island, `.plt`, the IPLT entries) at an address whose page offset
  (mod 4096) is 0xFFC, 0x000, 0x004 or 0x008 (a64.md §11.1, item 2).
  The chunk starts at the least address that is ≥ `prev_end`, a multiple
  of its alignment, and at an allowed page offset. For an alignment ≤ 16
  (islands 4, PLT 16) the padding is at most 16 bytes. It is filled with
  the trap pattern (L-82); it is not instructions, so it breaks every
  candidate sequence through it. The padding is an output of stage D
  (L-21) and enters the reach arithmetic (L-112). An empty island gets
  none. Without the option there is no padding.

### 14.3 Sized chunks

- **L-121 [C]** A chunk in `sized` whose size depends on addresses
  (`.relr.dyn` [M2], a compressed non-alloc chunk) grows monotonically:
  each round it gets the larger of its previous size and its new
  requirement. A patch island is in `sized` too (model §4.4), but its
  size is the function of its patch set that L-115 gives, and it grows
  because patches are only added. `.relr.dyn` is padded with RELR bitmap words that
  mark nothing, which is a valid encoding of the same set. Compressed
  non-alloc chunks come after every allocated chunk, so they do not move
  any address and are sized once, after the fixed point.

## 15. Validity: completing `model.md` §4.4

`model.md` §4.4 (after WP-020c) already has most of what this section
first asked for: rule 3 requires `tls.start` to be a multiple of
`tls.align` and the RELRO run to be contiguous and to end where the
loader's page rounding requires; rule 4 allows unused patches, keys an
erratum patch by its site, and separates the part the tables show
(checked up front on a loaded layout) from the part that needs the
relocations; a computed layout that breaks a rule is `internal-error`.
`check.rs` implements those (`tls()`, `relro()`, `patches()`). What
layout adds:

- **L-125 [C] (rule 3, derived segments)** `segments` is a function
  `derive_segments(chunks, order, Facts, Plan)`, implemented in
  `mortar_core` so that `Layout::check` and `mortar_layout` share one
  definition. `Layout::check` requires the stored segments to be equal
  to it. Segments then cannot disagree with chunks in a loaded layout.
  The function implements §11 and nothing else (L-65 to L-84, including
  `p_align` = `P`, L-67). This is a request (§20.1): `check.rs` today
  checks properties of the segments, not equality with a derivation.
- **L-126 [C] (rule 4)** The table part of rule 4 is `check.rs`
  `patches()` as it is, plus one layout fact: every patch lies in the
  island of its kind's policy, that is a thunk or erratum patch for a
  site in part *k* lies in `Patches(OutSecId, k)` or in an island that
  already held a thunk of its key (L-111). The reach part (every site
  that needs a patch has one; every patch reaches its target and is
  reached from its sites) needs the relocations: `fill` checks it on
  every field it writes, the verifier checks it independently, and an
  incremental link checks it on the units it refills (model §4.4).
  Layout runs it through `arch.md` in debug builds, after the fixed
  point.
- **L-127 [C] (rule 6)** Rule 6 (every relocated field holds its value
  at these addresses) is an obligation of `fill` and the verifier, as
  model §4.4 already says. `fill` fails with `relocation-out-of-range`
  (arch.md §7) and never writes a truncated value.
- **L-128 [C] (TLS)** Beyond `check.rs` `tls()`: the first TLS chunk's
  file offset, when it is `PROGBITS`, and `PT_TLS` `p_offset` are also
  multiples of `tls.align` (L-76), and `tls` equals
  what L-76 to L-78 derive (through L-125).
- **L-129 [C] (RELRO)** Beyond `check.rs` `relro()`: the page that the
  RELRO end must be a multiple of is maxpage (`CheckContext.page_size`),
  the last chunk of the run is the `.relro_padding` chunk (L-64), and
  `PT_GNU_RELRO`'s `p_filesz` is L-67's.

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
| L-1 | S2, extended: `p_align` ≥ 4096, and congruence modulo `p_align` | all |
| L-2, L-3 | new S13 | all |
| L-4 | new S14 | all |
| L-5 | new S15 | all |
| L-6 [C] | new A-class note: RWX segment | other linkers (note); mortar (M-rule) |
| L-7, L-8 | new S16 | all |
| L-9 | new T1 (anomaly for references) | mortar |
| L-10, L-11 | S9, extended | all |
| L-12 [C] | M-rule | mortar |
| L-19 | new S19: `ET_DYN` section alignment ≤ `p_align` | all, `ET_DYN` |
| L-23 | new S20: `PT_TLS` `p_vaddr ≡ 0 (mod p_align)` | all |
| L-13 to L-15 | new Y2 (symbol contract) | all, static |
| L-16 | S10 | all |
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
- **L-141 [C] [M7] (where slack lives)** Slack exists only as chunk
  `capacity − size` (after the chunk's content) and, in the
  hole-tolerant chunks of L-143, as holes between sections inside the
  content. No parser ever sees slack:
  - `sh_size` ends at the last chunk's `size` (L-86), so capacity beyond
    it is in no section header;
  - every end symbol is at `size` (L-92), so `__stop_SEC`,
    `__init_array_end` and the rest bound content only;
  - a `PT_NOTE` covers note chunks' `size` only; `.eh_frame`,
    `.eh_frame_hdr` and every `.debug_*` section have no holes (L-142);
  - merged-string and merged-constant chunks have the growth class of
    L-147, whose holes are valid pieces.
  The capacity beyond `size` lies between section headers, in the
  `PT_LOAD`'s memory but in no section; for the last chunk of a
  `PT_LOAD` that would break L-9 (strip would shrink the segment), so
  an incremental policy gives the last chunk of each `PT_LOAD` no slack
  and keeps its reserve in the chunks before it (the RELRO run ends with
  `.relro_padding`, which already covers its tail). The default policy
  leaves no slack, so a normal link pays nothing (L-87). An incremental
  policy is opted into (`--incremental`, `cli.md`, M7). Its reserve is a
  percentage per output section, as in gold. The percentage and its
  unit are open (Q8).
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
  Each is one chunk. Its slack is only its capacity beyond `size`, which
  is beyond `sh_size` (L-86, L-141), and an edit inside it relays the
  whole chunk. Growing such a section changes its end symbol's value,
  and the dependency index records the symbol's users (**Q4**, open).
- **L-143 [C] [M7] (hole-tolerant)** `.text` (holes filled with the trap
  pattern), `.rodata`, `.data`, `.bss`, `.data.rel.ro`, `.tdata`/`.tbss`
  (holes only at the end, since TLS offsets are baked into code) and
  `.gcc_except_table` accept holes between sections. A hole is inside
  `sh_size`, filled as L-82 says; nothing parses these sections as a
  sequence.
- **L-147 [C] [M7] (merged chunks, append-only)** A `Merged` chunk
  grows by appending: a new piece goes after the last piece, into the
  chunk's capacity, and `size` advances over it. A piece no longer used
  keeps its bytes, so every hole is a valid piece (a string or a
  constant of the class) and the chunk still parses as its class
  (`SHF_MERGE|SHF_STRINGS` with any `sh_entsize`). Pieces never move,
  so no reference to a kept piece changes. The chunk is repacked only by
  L-144 step 3.
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
- **L-145 [C] [M7]** Step 4 reruns stage D: a sequential O(chunks) pass
  that recomputes every address, alignment gap, 843419 start padding
  and the `.relro_padding` size from the stage C output and the patch
  set (L-21). Stages A to C rerun only for the output sections whose
  members changed. Then the fixed point (arch.md §6) starts from the
  previous patches, and round 1 re-evaluates every site against the new
  addresses: its cost is one pass over the branch and erratum sites of
  the code, as in a full link (the fast path of L-113 applies). Then
  stage F, and the refill of every unit that moved.
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
- **L-156 [T]** The padding section is named `.relro_padding`, lld's and
  mold's name, which llvm-objdump and lld-based tooling know (Q3,
  decided). eu-elflint `--gnu-ld` flags it: it takes any name beginning
  with `.rel`, `.rela` or `.relr` for a relocation section ("expected
  REL", E-12 and review; `_relro_padding` and `.re_ro_padding` pass).
  The harness therefore runs eu-elflint with that one message expected
  for that one section, on mortar's, lld's and mold's outputs alike.
- **L-157 [T]** gdb and lldb set breakpoints on static `-g` C outputs
  of all three references (E-16). lldb needs ASLR enabled to launch
  (`settings set target.disable-aslr false`) in the test container.
  That is a property of the container, not of the binary.

## 20. Requested changes to other specs

Rebased on `model.md` §4.3/§4.4 and `check.rs` after WP-020c, which
already allow unused patches, require `tls.start` to be a multiple of
`tls.align`, key erratum patches by `(SectionId, offset)`, and report an
invalid computed layout as `internal-error`. Those requests are dropped.

### 20.1 `model.md`

1. `ChunkKind::Body(OutSecId)` becomes `Body(OutSecId, u32 part)`, and
   `Patches(OutSecId)` becomes `Patches(OutSecId, u32 part)` (L-110,
   L-111). The `ChunkId` creation order puts each part's island after
   the part.
2. The RELRO padding is a chunk: `Synthetic(Synth { name:
   ".relro_padding", .. })` with `relro: true`, `SHT_NOBITS`, whose size
   layout's stage D writes (L-64). §4.4's list of chunk kinds names it,
   and states that it is the last chunk of the RELRO run.
3. Validity rule 3: segments are equal to `derive_segments(...)` in
   `mortar_core` (L-125); `PT_LOAD` `p_align` is `P` of L-67.
4. Validity rule 3, TLS: `PT_TLS` `p_offset`, and the first TLS
   chunk's file offset when it is `PROGBITS`, are multiples of
   `tls.align` too (L-76, L-128).
5. Validity rule 5: a `defined` offset is ≤ `size` of its chunk, and
   §4.4 says that `capacity − size` lies in no section header and under
   no symbol (L-86, L-92, L-141).
6. `DefValue::At` on a header chunk gives `st_shndx` 1 (L-91).
7. `entry` is none after `entry-not-found` and for `-shared`/`-r` with
   no entry symbol; the image writes `e_entry = 0` (L-103).

### 20.2 `cli.md`

| option | today | after M1 |
|---|---|---|
| `-z max-page-size=N` | refused | supported: a power of two ≥ 4096, or 0 for the default; else `invalid-option-value` (L-74) |
| `-z common-page-size=N` | refused | accepted and validated, no effect (L-75) |
| `-z separate-code`, `-z noseparate-code` | refused | supported (L-71); default `noseparate-code` on every target |
| `-z relro`, `-z norelro` | refused | supported. `norelro` removes rank 7 as a segment: its chunks join RW, and no padding is added |
| `-z execstack`, `-z noexecstack` | refused | supported (L-84) |
| `--warn-execstack` | refused | no-op (the default, L-84) |
| `--no-warn-execstack`, `--error-execstack` | refused | stay refused until requested |
| `--rosegment` | refused | accepted, no-op (always) |
| `--no-rosegment` | refused | stays refused |
| `-z start-stop-gc` | refused | stays refused (L-96) |
| `-z nostart-stop-gc` | refused | no-op (the default) |
| `-z start-stop-visibility=protected` | refused | no-op (the default); other values refused |
| `-z sectionheader` | refused | no-op |
| `--image-base=N`, `-Ttext-segment=N` | refused | M2, with a check that N is a multiple of `P₁` (L-68) |
| `-Ttext`, `-Tdata`, `-Tbss`, `--section-start` | refused | stay refused until scripts support them |
| `-z keep-text-section-prefix` | refused | stays refused (mortar's GNU-style categories already apply) |
| `--fix-cortex-a53-843419` | as `arch.md` | as `arch.md`, with L-117 and L-118 |
| `-e`, `--entry` | as is | L-102 to L-104 |

### 20.3 `plumb.md`

- **S2:** congruence modulo `p_align` (already), plus `p_align ≥ 4096`
  for mortar's outputs (L-1).
- **S7:** `e_entry` lies in an executable `PT_LOAD`, except that
  `e_entry = 0` is allowed when the link reported `entry-not-found`
  (L-103), and in `ET_DYN` (`-shared`) and `ET_REL` outputs (L-104).
- **S13:** `PT_PHDR` and `PT_INTERP` come before all `PT_LOAD`s, at
  most one of each, and are covered by a `PT_LOAD`. The phdr table is in
  a `PT_LOAD` (L-2, L-3).
- **S14:** a NOBITS allocated section (not `.tbss`) lies beyond
  `p_filesz` (L-4).
- **S15:** `sh_addr ≡ 0 (mod sh_addralign)` (L-5).
- **S16:** RELRO safety (L-8) for P ∈ {4, 16, 64} KiB up to the
  `PT_LOAD` `p_align`.
- **S17:** `PT_GNU_PROPERTY` covers exactly `.note.gnu.property` (L-17).
- **S18:** non-allocated sections are outside `PT_LOAD` file images
  (L-18).
- **S19:** in `ET_DYN`, every allocated section's `sh_addralign` ≤ its
  `PT_LOAD`'s `p_align` (L-19).
- **S20:** `PT_TLS` `p_vaddr ≡ 0 (mod p_align)` (L-23).
- **T1:** the end of every `PT_LOAD`'s memory is a section's end (L-9).
  An anomaly for references.
- **Y2:** the start-up symbol contract (L-13 to L-15).
- **A7 (lld), a note:** the RELRO end is aligned to 4 KiB with a
  64 KiB `p_align`, so under 64 KiB pages the last part of RELRO stays
  writable. A loss of protection, not of correctness: S16 still holds,
  because L-70 puts the next segment in a later page. Never a finding.
- **A8 (GNU), a note:** with no `.note.GNU-stack` in any input, GNU
  emits no `PT_GNU_STACK` (L-84).
- The RWX `PT_LOAD` note of L-6, for other linkers' outputs.

### 20.4 `diag.md`

New kinds (§3.1 table rows):

| id | variant | message | wording from |
|---|---|---|---|
| `section-conflict` | `SectionConflict { name, first, second }` | `section {name}: {first} and {second} inputs cannot be combined` | own (lld: `section type mismatch for {name}`) |
| `output-too-large` | `OutputTooLarge { what, limit }` | `output too large: {what} exceeds {limit}` | own (lld: `output file too large`) |
| `executable-stack` (warning) | `ExecutableStack { file }` | `{file}: requires executable stack (because the .note.GNU-stack section is executable)` | GNU ld |

`first`/`second` in `section-conflict` are the class words "TLS",
"non-TLS", "allocated", "non-allocated", or the `SHT_` names.
`internal-error` exists (`diag.md` §3.2). The `.gnu.warning` text is
resolve.md's `gnu-warning`, and the missing entry resolve.md's
`entry-not-found`: no new kinds for them.

New refusal names (class `feature` unless noted):

- `input section <name>`;
- `.ctors section`;
- `init_array priority`;
- `TLS common symbol`;
- `code section larger than branch reach`;
- `entry point outside code`;
- `_TLS_MODULE_BASE_ without TLS`;
- `non-PIE image base 0`;
- class `section-flag`: `SHF_X86_64_LARGE`.

(`.gnu.linkonce` and `.zdebug` are resolve.md's refusals already.)

### 20.5 `arch.md`

*Applied by WP-023b: all four bullets (arch §3.1, §6; a64 §11.1).*

- §6 "Addresses are a function of the patch set": at M2, the state is
  (patch set, the sizes in `sized`), with each `sized` size only
  growing, because `.relr.dyn` precedes the code (L-21, L-121). The
  termination argument then counts size increases as well as patches.
- `Facts.code_fill: &[u8]` (the trap pattern) and `Facts.code_nop:
  &[u8]` (L-82).
- x86-64 and i386: a static `.got.plt` has its three reserved words
  when `GOT_BASE` is demanded (L-101).
- a64.md §11.1 item 2's padding is specified as L-118; a64.md's "at
  most 16 bytes" holds for chunk alignments ≤ 16.

### 20.6 `synth.md` (not yet written)

- `.relro_padding` is a layout chunk, not synthetic content.
- `.eh_frame` keeps input order (L-58); its type follows L-49.
- The IPLT goes in `.plt`/`.rela.plt`/`.got.plt` for static links (arch
  Q9).
- `.note.gnu.property` is the AND-merge of the inputs' notes; an input
  without one has no features (L-35).

### 20.7 `resolve.md`

- §10: `SHT_AARCH64_ATTRIBUTES` becomes accepted and consumed
  (`Dropped(Consumed)`), not refused; §17 item 9 is closed (L-35).
- §5.5: "the entry is layout's default (`spec/layout.md`: GNU's, the
  start of `.text`)" becomes "`e_entry = 0`" (L-103).

## 21. Where the references differ

| point | GNU | lld | mold | mortar | why |
|---|---|---|---|---|---|
| PT_LOAD shape | R E + RW | R, RX, RW-RELRO, RW | same as lld | lld | L-65 |
| RELRO end alignment | maxpage (by moving RW start, file gap) | common page (4 KiB) | maxpage (padding) | maxpage (padding) | L-8, L-64 |
| next segment address | page gap | L-70 | L-70 | L-70 | contiguous file |
| separate-code default | AArch64 no; x86-64 and i386 yes (R, R E, R, RW) | no | no | no | L-71, Q2 |
| base, AArch64 | 0x400000 | 0x200000 | 0x200000 | 0x400000 | L-68 |
| rodata vs text order | text first | rodata first | rodata first | rodata first | L-62 |
| `.text` input order | categories | input order | input order | categories | L-53 |
| `.ctors` | into init_array, reversed | separate, not run | as GNU | as GNU (M2), refused (M1) | L-56 |
| `.preinit_array` vs `.init_array` | pre first | after | pre first | pre first | L-62 |
| merged sections | one output | one output, MERGE kept | one per class | one output, MERGE dropped if mixed | L-50 |
| PT_PHDR | with INTERP | always | always | with INTERP | L-67 |
| `PT_LOAD` `p_align` | max(maxpage, section align) | same | same | same | L-67 |
| PT_GNU_STACK p_align | 16 | 0 | 1 | 16 | L-67 |
| no `.note.GNU-stack` in any input | no `PT_GNU_STACK` | RW | RW | RW | L-84 |
| exec `.note.GNU-stack` | RWX, warning | lld 19: RW, silent; lld 22: error | RW, warning | RWX, warning | L-84 |
| `-z max-page-size` | power of two (also < 4096), base rounded down | 2M, 4M accepted | 4M refused | GNU's, ≥ 4096 | L-74 |
| TLS of `.tbss` only | RELRO segment start | inside RX range | after `.init_array` start | GNU | L-77 |
| `PT_GNU_RELRO` `p_filesz` | = `p_memsz` (but `.tbss` case) | file part | file part | file part | L-67 |
| x86-64 `.eh_frame` type | UNWIND iff all inputs are | PROGBITS | PROGBITS | GNU | L-49 |
| `.ARM.attributes` | dropped | copied | dropped (and no property note) | dropped | L-35 |
| code fill (AArch64) | NOP | not examined | not examined | trap (zero) | L-82 |
| padding section name | none | `.relro_padding` | `.relro_padding` | `.relro_padding` (eu-elflint flags it) | L-156 |
| `_end` | aligned 8 | exact | exact | exact | L-95 |
| `__preinit_array_*` absent | location counter (in TLS) | base, index 1 | undefined | base, index 1 | L-93 |
| `__dso_handle` | crt | defines, local hidden, base | defines, local, first section | crt | L-99 |
| `__data_start` | AArch64 `PROVIDE` | no | no | AArch64 only | L-99 |
| `__tdata_start` without `.tdata` | location counter | undefined | undefined | undefined | L-105 |
| `_GLOBAL_OFFSET_TABLE_`, no GOT, AArch64 | `.got` 8 + `.got.plt` 0x18 | empty `.got` | as GNU | `.got` header word | L-101 |
| x86 static `.got.plt` | 3 words + IRELATIVE | same | 3 words | GNU | L-101 |
| `__start_`/`__stop_` gc | keep | discard | keep | keep | L-96 |
| `__start_` visibility | protected | protected | local | protected | L-91 |
| `__rela_iplt_*` in PIE | undefined | undefined | defined (0 without IRELATIVE) | undefined | L-94 |
| `.tdata` alignment | raised to TLS max | own | own | own | L-78 |
| undefined entry | start of .text, warning | 0, warning | 0, warning | 0, warning | L-103 |
| i386 address overflow | error | error | wraps silently | error | L-73 |
| `.init`/`.fini` vs `.text` | `.init`, `.plt`, `.text`, `.fini` | `.text`, `.init`, `.fini`, `.iplt` | `.init`, `.text`, `.fini` | GNU | L-62 |
| `.gnu.warning*` | consumed | copied as orphans | one `.gnu.warning` | consumed | L-33 |

## 22. Open questions

Decided in the review of 2026-11-14 (WP-024-spec §5):

- **Q1.** RELRO is padded to maxpage (L-64).
- **Q2.** Separate-code is off by default on every target (L-71); GNU
  differs on x86-64 and i386 (§21).
- **Q3.** The padding section keeps lld's name `.relro_padding`
  (L-156).
- **Q5.** GNU's bases (L-68).
- **Q6.** An undefined entry is a warning, with `e_entry = 0` (L-103).
- **Q7.** GNU's `.text` categories (L-53). This affects every `-O2`
  link, not only `-ffunction-sections` ones: GCC puts `main` in
  `.text.startup` at `-O2`.
- **Q9.** An exec `.note.GNU-stack` gives RWX with GNU's warning
  (L-84).

Still open, for M7:

1. **Q4. End symbols and growth.** End symbols are at `size` (L-92), so
   growing a packed section changes their values. Does the dependency
   index record symbol users, or is the growth refused into a full
   relayout?
2. **Q8. The incremental reserve:** the percentage, and whether it is
   per output section (gold) or per chunk (L-141).

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
