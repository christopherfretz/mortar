# spec: synthetic sections (`mortar_merge`, `mortar_synth`)

Status: **accepted for M1** (WP-025 stage 1; reviewed independently, the review's findings applied). The evidence for each non-obvious rule is
in `spec/landed/synth/WP-025-spec.md` ("X-n" below names an experiment
there).

This spec covers everything the linker writes that no input section holds
whole:

1. split and merge (phase 60): `SHF_MERGE` sections and `.eh_frame` cut
   into pieces, the fragments, `MergeMap` (§3, §4.1 to §4.3);
2. the synthetic sections of a static executable (phase 80 for their
   order and size, `mortar_image` for their bytes): `.eh_frame` after
   de-duplication (§4), `.eh_frame_hdr` (§5), `.got`, `.got.plt`, `.plt`
   and `.rela.plt` for IFUNC and TLS (§6), `.symtab`, `.strtab`,
   `.shstrtab` and `.symtab_shndx` (§7), the notes (§8), `.comment` (§9);
3. the outline of the dynamic tables (M2) with the decisions that
   constrain the M1 shapes (§10);
4. sizes as data, units, and what an incremental link redoes (§11);
5. what debuggers and tools rely on (§12).

It writes the `Split` (model §3.5), `MergeMap` (model §4.2) and `Slots`
(model §4.3) tables, and says what the image phase computes from them.
Layout (`spec/layout.md`) decides where the chunks go; this spec decides
what is in them and how big they are.

## 1. Conventions

**Rule ids.** Every normative rule has an id `S-n`. Tests (`#@ spec:`)
cite the section by its title; findings and review notes cite the id. Ids
are never reused. A withdrawn rule keeps its id and is marked "withdrawn".

**Classes**, as in `spec/layout.md` §1:

- **[G] generic.** Any correct output obeys it, whichever linker made it.
  The rule cites the document that requires it (the gABI, a psABI, the
  LSB's "Exception Frames", the DWARF 5 standard §6.4 "Call Frame
  Information", glibc's static start-up, libgcc's unwinder as
  documented). `mortar_plumb` checks [G] rules on every linker's output.
- **[T] tool.** No loader needs it, but a tool we must work with does
  (gdb, lldb, perf, nm, addr2line, objdump, valgrind, strip, eu-elflint).
- **[C] choice.** mortar's own deterministic decision. Another linker may
  decide differently.

**Milestones.** M1: static non-PIE AArch64 executables; M2: dynamic
outputs; M3: x86-64 (its static rules are M1 since `spec/x64.md` X-3 and
`docs/ROADMAP.md` M1); M4: i386; M7: incremental linking. A rule tagged
with a later milestone binds the design now: an M1 implementation must
not make that rule hard to add. A rule with no tag applies from M1. Rules
that name x86-64 are M1 (static) and M2 (dynamic) rules, those that name
i386 M4 rules; they are written now
because the shapes are shared.

**Words.** A **piece** is a part of a split input section (a string, a
constant, a CIE, an FDE). A **fragment** (`FragId`) is a set of equal
pieces of one output section; its first piece is its
**representative**. A **merge class** is the pair (`SHF_STRINGS` or not,
`sh_entsize`). `align_up(x, a)` is the least multiple of `a` that is
≥ `x`. *word* is 8 bytes on AArch64 and x86-64, 4 on i386.

**References.** GNU ld 2.44 ("GNU"), ld.lld 19.1 and 22 ("lld"), mold
2.37.1, observed on AArch64 natively and on x86-64 and i386 through the
cross binutils and GCC 14 (X-n). "All three" means the three agreed in
every experiment that tested the point.

**Diagnostics.** A refusal is `Refused { class, name }` (`spec/diag.md`
§4). A malformed input is `malformed-input` with the message given here
(lld's wording where lld has one). Both are errors. The phase ordinal of
a diagnostic is model §8's: 60 for split and merge (unit `FileId`, seq
`section index × 2^32 + piece`), 80 for synthetic sections.

## 2. Where this sits in the pipeline

- **S-1 [C] (phases).** The work is split by what it depends on:
  - **phase 60** (`mortar_merge`, after resolution and assignment, before
    liveness): cut every `Merge` and `EhFrame` section (resolve.md §10
    fates) into pieces, intern the merge pieces into fragments, and parse
    every `.eh_frame` record. It writes `Split`;
  - **after liveness** (`mortar_merge`, second stage): `frag_live` is
    known; give every live fragment its offset (`MergeMap`), and decide
    which `.eh_frame` records survive;
  - **phase 80** (`mortar_synth`, after the scan of model §3.7, before
    layout): give every slot its index, order the `.eh_frame` records,
    count the symbols, and so fix the size of every synthetic chunk that
    does not depend on addresses (`Slots.sizes`);
  - **image** (`mortar_image`, model §5): write the bytes of every
    synthetic chunk from the tables and the final addresses.
- **S-2 [C] (sizes before content).** Every synthetic chunk's size is a
  function of `Split`, `Liveness`, `Demands`, `Slots`, the `Plan` and the
  architecture's shapes (arch.md §3.7), with two exceptions that
  depend on addresses: `.relr.dyn` [M4] (model §4.4 `sized`; `spec/dyn.md` D-27, D-62), and the
  symbol tables when thunks or patches exist (S-80, S-88: their size is fixed
  after layout's fixed point, which no address depends on, because the
  symbol tables come after every allocated chunk, layout rank 10). No
  synthetic chunk's size depends on the bytes another chunk will hold.
- **S-3 [C] (the synthetic chunk list).** model §4.4 gives `ChunkId`s to
  synthetic chunks "in the order synth.md lists them". The list, with
  the milestone at which each can exist:

  | # | name | content | exists when | M |
  |---|---|---|---|---|
  | 1 | `.interp` | the interpreter path | an interpreter is named (`Exec`, `Pie`; never `StaticPie`, `spec/dyn.md` D-31) | M2 |
  | 2 | `.note.gnu.property` | merged properties (§8.2) | some property survives the merge | M1 |
  | 3 | `.note.gnu.build-id` | the build id (§8.1) | `--build-id` other than `none` | M1 |
  | 4 | `.gnu.hash` | hash table | dynamic output | M2 |
  | 5 | `.dynsym` | dynamic symbols | dynamic output | M2 |
  | 6 | `.dynstr` | dynamic strings | dynamic output | M2 |
  | 7 | `.gnu.version` | `versym` | `.gnu.version_r` exists (`spec/dyn.md` D-27) | M2 |
  | 8 | `.gnu.version_d` | version definitions, written by `spec/dyn.md` D-115 (S-128) | a version script (or a `.symver` definition) names a version node, `Shared` only (never in an executable, `spec/dyn.md` D-27) | M4 (WP-125) |
  | 9 | `.gnu.version_r` | version needs | a needed object's definition of some imported key is versioned (`spec/dyn.md` D-27) | M2 |
  | 10 | `.rela.dyn` (`.rel.dyn`) | dynamic relocations | some dynamic relocation | M2 |
  | 11 | `.relr.dyn` | packed relative relocations | `-z pack-relative-relocs` | M4 (`spec/dyn.md` D-27, D-62) |
  | 12 | `.rela.plt` (`.rel.plt`) | `JUMP_SLOT`, `IRELATIVE` | a PLT or IPLT entry exists | M1 |
  | 13 | `.eh_frame_hdr` | the FDE table (§5) | `--eh-frame-hdr` and some live FDE | M1 |
  | 14 | `.eh_frame` | the live records (§4) | some live record | M1 |
  | 15 | `.plt` | PLT and IPLT entries | a PLT or IPLT entry exists | M1 |
  | 16 | `.got` | GOT slots (§6.1) | a slot or the header word exists | M1 |
  | 17 | `.got.plt` | PLT and IPLT words (§6.2) | a PLT or IPLT entry, or the x86 header (S-67) | M1 |
  | 18 | `.dynamic` | the dynamic section | dynamic output | M2 |
  | 19 | `.symtab` | the symbol table (§7) | not `-s` | M1 |
  | 20 | `.symtab_shndx` | extended section indices | some `.symtab` entry needs one (S-85) | M1 |
  | 21 | `.strtab` | symbol names | with `.symtab` | M1 |
  | 22 | `.shstrtab` | section names | always | M1 |
  | 23 | `.comment` | mortar's identification (S-100) | always | M1 |
  | 24 | `.dynbss` | copy allocations of writable shared data (`spec/dyn.md` D-42); the first chunk of output section `.bss` | some such allocation | M2 |
  | 25 | `.bss.rel.ro` | copy allocations of read-only or RELRO shared data (D-42); output section `.bss.rel.ro` | some such allocation | M2 |
  | 26 | `.note.mason` | the one-definition-rule table (`spec/odr.md` O-50-O-52): `SHT_NOTE`, `SHF_ALLOC`, alignment 8; with `=full` a second note (WP-076) | `--odr-table=exported` or `=full` and a dynamic executable | M6 |
  | 27 | `.mason.meta` | the live inputs' Mason units, kept for tools (`spec/meta.md` MT-32): `SHT_PROGBITS`, no flags, alignment 8, not allocated | `--keep-mason-meta` and some live unit (WP-076) | M6 |
  | 28 | `.gdb_index` | gdb's name and address index, version 7 (§17): `SHT_PROGBITS`, no flags, alignment 4, not allocated | `--gdb-index` and some unit in the output's `.debug_info` (S-G1; WP-097) | M2 |

  There is no `.hash` chunk: an executable gets `.gnu.hash` only
  (`spec/dyn.md` D-20; `--hash-style` other than `gnu` is refused).

  A chunk that does not exist gets no `ChunkId`. The numbering is a
  name, not an order: `Placement.order` (layout §10) is the order.
  `.comment` was last so the earlier chunks kept their ids (WP-027e); the
  M2 rows 24-25 and M6's rows 26 (WP-075) and 27 (WP-076) follow it for
  the same reason; so does row 28 (WP-097).
- **S-4 [C] (division).** Each chunk declares its `Division`
  (`spec/image.md` §13, I-104) through the `SynthChunk` interface
  (`size`, `division`, `write(cx, range, out)`, `sealed`). `Divisible`
  chunks (`.eh_frame`, `.symtab`, `.strtab`, `.symtab_shndx`, `.got`,
  `.got.plt`, `.rela.plt`, `.plt`, and any other whose `write` meets
  I-104's bound) are filled by range; `Whole` chunks are written by one
  job into a buffer of their size, each with its size bound as a
  function of the tables:

  | whole chunk | bound |
  |---|---|
  | `.eh_frame_hdr` | `12 + 8 × live FDEs` (sorted by address: I-105's global order) |
  | `.shstrtab` | `1 + Σ (name + 1)` over output sections |
  | `.note.gnu.build-id` | `16 + align_up(n, 4)` |
  | `.note.gnu.property` | `16 + Σ(8 + align_up(datasz, a))` |
  | `.note.mason` | `40 + 24 × rows`, plus `40 + 32 × (definitions + types)` with `=full` (`spec/odr.md` O-51, O-52) |
  | `.mason.meta` | `Σ align_up(unit, 8)` over the live units kept (`spec/meta.md` MT-32) |
  | `.gdb_index` | its size (S-G7a) |

  `.strtab`'s range start is found through per-file prefix sums and the
  name offset of every 1024th symbol (input local, global id, or demoted
  row): a binary search over files, then over samples, then a walk of at
  most 1024 symbols; `.symtab` and `.symtab_shndx` likewise by entry. For
  any split of `[0, size)` the ranges' bytes concatenate to the same
  bytes (I-103).

  [M2] `.rela.dyn` is `Divisible`: its `RELATIVE` region (`spec/dyn.md`
  D-40 step 1) is the concatenation, in address order of the placed
  sections and GOT slots, of each unit's records sorted by `r_offset`;
  each unit's slice starts at an exclusive prefix sum of the per-section
  record counts the scan produced (model §3.7, image I-15), so the
  records are filled in parallel with no global sort. The records of
  D-40 step 2 (one per imported key and place) are few, collected and
  sorted by (`.dynsym` index, `r_offset`), and follow at the offset the
  `RELATIVE` count gives. A chunk's bytes depend only on the tables and
  addresses; no chunk reads another chunk's output, with one exception
  the writer orders explicitly: the build id's descriptor, the `sealed`
  range (S-92, I-107), computed over the finished image.

## 3. Split and merge

### 3.1 Which sections are split

- **S-5 [C].** A section is split exactly when its fate (resolve.md
  §10) is `Merge` or `EhFrame`: an `SHF_MERGE` section with
  `sh_entsize > 0`, allocated or not (`.rodata.str1.1`, `.rodata.cst8`,
  `.debug_str`, `.debug_line_str`, `.comment`), and every `.eh_frame`
  (§4). An `SHF_MERGE` section with `sh_entsize == 0` is `Regular` there
  and is placed whole; so is every section this subsection removes from
  merging. Merging is an optimization; placing a mergeable section whole
  is always correct, so every case below that is not certain to be safe
  falls back to placing it whole rather than refusing.
- **S-6 [C] (not merged).** A `Merge` section is placed whole, as
  `Regular`, when it:
  - is `SHF_WRITE` or `SHF_EXECINSTR` (merging writable data would
    alias two objects; the gABI gives `SHF_MERGE` no meaning for code);
  - is the target of a relocation section (its bytes are not final, so
    equal bytes need not be equal pieces);
  - is `SHF_TLS`;
  - is `SHF_COMPRESSED` (a non-alloc `.debug_str` of `-gz`: its bytes are
    not the pieces' until decompressed, and phase 60 reads the file's
    bytes);
  - has `sh_addralign` that is not a power of two (refused already by
    `elf.md`), or `sh_entsize` larger than 2^16;
  - is empty (`sh_size` 0; WP-136, `spec/resolve.md` §10.1 P-1): it has
    no piece, so no fragment could give a symbol defined in it a value.
    Placed whole it is an ordinary empty section of its output
    section's body (`spec/layout.md` L-159 when that section holds no
    bytes), or, after a merged input, at the end of the merged chunk
    (`spec/layout.md` L-162): where GNU ld and lld agree to put it,
    measured on the three targets; mold gives it an empty output
    section of its own.

  None of these occurs in the library scan (X-1); the rule exists so
  that a hand-written object cannot make a wrong merge. Such a section
  keeps `SHF_MERGE` out of its output section's flags (layout L-50).
  Resolution decides it (resolve.md §10: the row's kind becomes
  `Regular`, `mortar_resolve::sections::placed_whole`), so every later
  phase — merge, layout's `addr_in`, the symbol values — sees a plain
  section; phase 60 never cuts one (WP-027a; the test is
  `crates/mortar/tests/front_diff.rs` `s6_unsafe_merge_sections_are_placed_whole`).
- **S-7 [G] (malformed).** A `Merge` section whose size is not a
  multiple of `sh_entsize` is malformed (`elf.md` §3.10, `BadSize`):
  `malformed-input`, `section size is not a multiple of sh_entsize`.
- **S-8 [C] (unterminated strings).** An `SHF_STRINGS` section whose
  last `sh_entsize` bytes are not all zero is refused: `malformed-input`,
  `string is not null terminated` (lld's and mold's behavior; GNU copies
  the bytes and lets the last string run into whatever follows, X-4).
  The scan found none (X-1).

### 3.2 Pieces

- **S-9 [G] (string pieces).** An `SHF_STRINGS` section with entsize `e`
  is a sequence of `e`-byte characters (gABI "Section Attributes").
  A piece is a maximal run of characters ending with the first all-zero
  character, the terminator included. Characters are compared at
  positions that are multiples of `e` from the section start: a zero
  byte inside a non-zero character is not a terminator. Entsizes 1 and
  4 occur in the scan (X-1: `.rodata.str1.*`, and `.rodata.str4.*` from
  `wchar_t` literals); 2 (`char16_t` literals) and any other entsize
  follow the same rule. An empty string (one terminator) is a piece.
- **S-10 [G] (constant pieces).** A section without `SHF_STRINGS` with
  entsize `e` is a sequence of `e`-byte entries; each is a piece. Entsizes
  2, 4, 8, 16 and 32 occur (`.rodata.cst*`, X-1).
- **S-11 [G] (alignment).** Every piece of a section has that section's
  `sh_addralign`, which may exceed the entsize (`.rodata.str1.8`,
  `.rodata.cst16` with 32-byte alignment; X-1). A piece is placed at a
  multiple of its alignment, not only the section's first piece: all
  three references place every piece of `.rodata.str1.8` on an 8-byte
  boundary (X-7), because a compiler may assume any string it put there
  is 8-aligned (vectorized `memcpy` of a literal).
- **S-12 [C] (the Split row).** Per split section, the piece offsets in
  the section (`u32`) and the `FragId` of each piece (model §3.5): 8
  bytes per piece occurrence. A piece's size is the distance to the next
  piece offset (or the section end); it is not stored. A section with
  more than 2^32 − 1 pieces is refused (`feature`, `more than 2^32 - 1
  pieces in a section`; model §8).

### 3.3 Fragments

- **S-13 [C] (identity).** Two pieces belong to one fragment exactly when
  they go to the same output section, have the same merge class and the
  same alignment, and have equal bytes. The merge class is part of the
  identity: the same bytes in `.rodata.str1.1` and `.rodata.cst4` (a
  4-byte constant that happens to read as a 3-character string) are two
  fragments, and so are `abc\0` in a 1-aligned and in an 8-aligned string
  section (X-7: all three keep both; the string-against-constant case
  was not linked, and the rule follows from S-9 and S-10 cutting the two
  differently). Pieces of different output
  sections never merge (`.rodata.str1.1` and `.debug_str`).
- **S-14 [C] (which copy).** A fragment's representative is the piece
  with the least `(FileId, sh_index, piece index)` among its pieces: the
  first occurrence in link order. Its bytes and alignment are the
  fragment's. All pieces of a fragment have equal bytes, so the choice is
  visible only to symbols defined in a piece (S-22), which are valued
  through their own piece's fragment and so do not depend on the choice.
- **S-15 [C] (numbering).** `FragId`s number fragments by their
  representative's key, ascending: the numbering model §2.3 defines for
  symbols, computed by S-28's partitioned method rather than §2.3's
  atomic minimum (R-M7). The numbering is a function of the inputs and
  not of the thread count (model §6).
- **S-16 [C] (live fragments).** A fragment is live when any of its
  pieces lies in a live section and (with `--gc-sections`) is referenced
  or is in a non-allocated section (`spec/live.md` owns piece-level
  liveness; `frag_live` is its output). Without `--gc-sections` every
  piece of a live section is live: GNU keeps unreferenced strings, and
  so does mortar.

### 3.4 References into a merge section

A relocation or a symbol whose target is in a split `Merge` section is
resolved to a piece. The rule follows the gABI's two forms of target, a
section symbol with an addend, and a named symbol with an addend.

- **S-17 [G] (section symbol plus addend).** For an `STT_SECTION` symbol
  of a split section with addend `A` (RELA: `r_addend`; REL: the
  implicit addend read from the place, `arch.md` `read_addend`), let
  `r = st_value + A` (`st_value` is 0 for section symbols in every
  object seen). If `0 ≤ r < sh_size`, the target is the piece `p` that
  contains byte `r`, and the value is
  `out(frag(p)) + (r − start(p))`, where `out` is the fragment's output
  address. The x86 PC-relative bias (`A = offset − 4`) is inside `A`,
  and mortar, like all three references, finds the piece from `r`
  including the bias; producers avoid the ambiguity by referring through
  local symbols (GCC's `.LC*` on x86-64, X-1, X-5). This is the
  convention `plumb.md` §5.6 checks.
- **S-18 [C] (outside the section).** A section-symbol reference with
  `r < 0` or `r ≥ sh_size` is refused: `malformed-input`, `offset is
  outside the section; refer to it through a named symbol` (lld's
  behavior, with the fix named: S-19 links the same address through any
  symbol of the section); a known choice against GNU (§13). At `r ==
  sh_size` GNU silently writes the end of the *output* section, the end
  of some other object's string, and beyond it warns `access beyond end
  of merged section` and does the same; mold writes the end of the last
  piece, or an unrelated address (X-2). The scan has no such reference
  (X-1), and compilers emit one-past-the-end addresses through named
  symbols (X-5), i386 REL objects included: GCC 14 with GNU as and Clang
  19 (`-O2 -fmerge-all-constants`, `-fno-pic` and `-fPIC`) write
  `"abcdefgh" + 9` as `.LC1`/`.L.str` plus 9 and put constant arrays in
  plain `.rodata` (X-28).
- **S-19 [G] (named symbol plus addend).** For any other symbol defined
  in a split section, with value `v = st_value` and addend `A`, the
  value is `out(frag(p)) + (v − start(p)) + A`, where `p` is the piece
  containing byte `v` (or, when `v == sh_size`, the piece ending at `v`).
  The addend is applied after the mapping, so `sym + A` may leave the
  piece in either direction: all three references do this (X-3), and the
  i386 libc contains two such references (`gconv_simple.o` `sym − 2`,
  `plural.o` `sym − 16`, X-1) that only this rule links as compiled.
  The result is the address the producer meant whenever its expression
  stays within the piece or one past its end, which is all that C and
  C++ define.
- **S-20 [C] (non-allocated targets).** S-17 to S-19 apply unchanged to
  references from non-allocated sections (`.debug_info` into
  `.debug_str`); a reference from a non-alloc section to a piece of a
  section that is not live writes the tombstone (arch.md §3.11).
- **S-21 [C] (references by GOT).** A GOT slot for a symbol in a merge
  section holds the S-19 value (no `addend` participates, the slot is the
  symbol's address). A GOT-generating relocation against a section symbol
  of a merge section with a non-zero addend is refused (`feature`,
  `GOT reference to a section symbol plus addend in a mergeable
  section`): the slot would be per (piece, offset), which `Slots` does
  not key. None occurs in the scan; compilers use named symbols for GOT
  references.

### 3.5 Symbols defined in a merge section

- **S-22 [C].** A symbol defined in a split `Merge` section, at
  `st_value = v`, gets the output value of S-19 with `A = 0`. Such
  symbols are common: AArch64 mapping symbols `$d`, x86-64 `.LC*` and
  `.L.str*` locals, named statics such as `default_gconv_path`, and a
  few globals (`_IO_stdin_used` in `crt1.o`, `_nl_default_locale_path`;
  X-1). `st_size` is copied. A symbol whose piece is in a fragment that
  is not live is dropped from `.symtab` (S-76); a global so placed is
  undefined-after-gc, which `spec/live.md` reports.

### 3.6 Tail merging

- **S-23 [C].** mortar does not tail-merge: a string that is a suffix of
  another is its own fragment. GNU always tail-merges `SHF_STRINGS`
  sections; lld does at `-O2`; mold never does (X-6). The gain is small:
  on the protoc `-g` link (340 objects) 0.8% of `.debug_str` (10.21 MB
  against 10.13 MB) and 9.5% of a 32 KB `.debug_line_str`; X-8's 13% is
  a two-file program. The cost is a sequential sort of all strings of a
  class by reversed content (lld's `-O2` method) that cannot be sharded
  by hash, and it couples pieces (one string's bytes hold another's),
  which would make layout L-147's append-only merged chunk depend on
  which strings left. Final (Q1 closed): no tail merging at any `-O`.

### 3.7 Output order and offsets

- **S-24 [C] (order).** Within a `Merged` chunk (layout L-85), the live
  fragments are grouped by merge class (strings before constants, then
  entsize ascending, then alignment ascending) and ordered by `FragId`
  within a group; `frag_off(f) = align_up(end of the previous fragment,
  align(f))` in that order. This is first-occurrence order, GNU's for the
  strings its tail merge keeps (X-6); lld and mold order by hash, which
  makes a one-string edit move every string. It keeps strings in source
  order, and an incremental append (L-147) gives the full link's order
  when the new strings are in the last file.
- **S-25 [C] (alignment groups).** Sorting by alignment inside a merge
  class keeps 8-aligned strings together so that the padding between
  them is at most 7 bytes per fragment, not once per class change. This
  costs nothing: the group key is part of the class.
- **S-26 [C] (the chunk's alignment).** A `Merged` chunk's alignment is
  the largest alignment of its live fragments; its size is the end of
  its last fragment. A chunk with no live fragment does not exist.
- **S-27 [G] (string sections stay well-formed).** The bytes of a
  `Merged` chunk of a string class are whole strings, and padding between
  them is zero characters. A zero padding character is an empty string of
  that class, so the chunk is a valid `SHF_STRINGS` section with any
  `sh_entsize` (the gABI's definition), and a tool that walks it (strings,
  `readelf -p`) sees only extra empty strings. The output section keeps
  `SHF_MERGE`, `SHF_STRINGS` and `sh_entsize` only under layout L-50.

### 3.8 The parallel algorithm and its cost

- **S-28 [C] (method).** Phase 60 runs in five steps, each parallel:
  1. **cut**: per section, find piece boundaries (a vectorizable scan for
     zero characters, or a stride) and hash each piece's bytes with the
     class and alignment into one 64-bit hash;
  2. **partition**: split the split sections into `C` contiguous ranges
     in `SectionId` order (`C` = 8 × threads); per range, append each
     occurrence's `(SectionId, piece)` to the bucket of its shard (the
     high hash bits; 2^k shards with 2^k ≥ 8 × threads). Concatenating
     a shard's buckets in range order lists its occurrences in key order
     (`(FileId, sh_index, piece)`, since `SectionId`s are given in that
     order, model §2.2);
  3. **intern**: per shard, on one thread and with no lock, walk its
     occurrences in key order and look each up by `(hash, bytes)` in the
     shard's own table. A miss creates an entry whose representative is
     that occurrence, which is the first occurrence (S-14) because the
     walk is in key order: no atomic minimum and no pass that marks
     owners is needed. Each occurrence records its entry (shard and
     index, one `u32`). A shard's table belongs to one thread, so it
     grows as needed and needs no sizing pass;
  4. **number**: every shard's entry list is its representatives in key
     order. Bucket them by section (a pass over fragments, not
     occurrences), prefix-sum the counts in `SectionId` order, and give
     each section's representatives consecutive `FragId`s in piece order;
     store each entry's `FragId`;
  5. **resolve**: every occurrence reads its entry's `FragId` into its
     `Split` row.

  A shard depends only on bytes and its order is key order whatever the
  threads did, so entries, representatives and `FragId`s are functions of
  the inputs (model §6 rules 2, 3, 5); hashes only propose candidates,
  bytes decide. The prototype (X-29) gives the same `FragId`s at 1, 4 and
  8 threads as the previous draft's atomic-minimum method.
- **S-29 [C] (offsets).** `frag_off` is one sequential pass per output
  section over its live fragments in S-24 order (an aligned prefix sum);
  for 2.4 × 10^6 fragments it took 2 to 3 ms (X-29). A parallel prefix
  sum with alignment is possible (block sums computed per residue of the
  largest alignment) and is not needed at that cost.
- **S-30 [C] (cost, measured).** With the safe-Rust prototype (X-29), on
  the shared host at load 29 to 34, so upper bounds:

  | step | synthetic, 1 / 4 / 8 threads | protoc `.debug_str`, 4 threads |
  |---|---|---|
  | cut and hash | 423 / 230–251 / 302 ms | 20 ms |
  | partition | 87 / 26–31 / 50 ms | 7 ms |
  | intern | 1661 / 428–527 / 316 ms | 28 ms (92 at 1 thread, 17 at 8) |
  | number | 41 / 26–33 / 29 ms | 2 ms |
  | resolve | 33 / 10–15 / 12 ms | 1 ms |

  Synthetic: 10^7 string pieces, 529 MB, 2.45 × 10^6 fragments. protoc:
  the `.debug_str` of the `cxx-medium-g` workload, 990,853 pieces,
  64.2 MB, 95,321 fragments, 10.21 MB out; about 60 ms in all, against
  0.72 s for mold's whole link of that workload (the bench's median).
  The previous draft's locked table with an atomic minimum took 2.6 to
  3.3 s to intern the synthetic input, 117 ms for protoc.

  Memory: per occurrence, 8 bytes kept (the `Split` row, S-12) and 20
  held during phase 60 (hash 8, partition record 8, entry index 4); per
  fragment about 44 bytes (representative 8, a table slot of 12 at load
  ≤ 0.5, `FragId` 4, `frag_off` 8). The synthetic input then peaks at
  280 + 108 MB; the prototype, with fatter records and std's `HashMap`,
  measures 472 MB beyond its input. The bench harness
  (`docs/VALIDATION.md`) replaces these figures with mortar's.
- **S-31 [C] (what is taken from mold and lld).** Taken: sharding the
  table by hash bits (both); piece boundaries found per section in
  parallel (both); a per-shard pass on one thread (lld's per-shard
  finalisation). Rejected: output order by hash or shard (both; S-24); a
  concurrent table whose kept copy is decided by a race and repaired
  afterwards (mold's lowest-priority owner): S-28's key-ordered walk
  makes the first insert the first occurrence; lld's `-O0` no-merge mode
  (S-32).

### 3.9 Options

- **S-32 [C] (`-O`).** `-O LEVEL` (`-O0`, `-O1`, `-O2`, any integer) is
  accepted and changes nothing: mortar merges at every level, as GNU and
  mold do (X-6; GNU's output is the same at `-O0` and `-O2`). lld does
  not merge at `-O0`; no build depends on that, since it only grows the
  output.
- **S-33 [C] (no other merge options at M1).** No option turns merging
  off (GNU has none; lld's `-O0` is S-32); a switch for debugging the
  linker is a test hook. `-z` merge options (`-z nomerge` exists in none
  of the three) remain `unknown-option` (`spec/cli.md`).

## 4. .eh_frame

`.eh_frame` is a sequence of CIE and FDE records (LSB "Exception Frames",
DWARF 5 §6.4.1 with the LSB's augmentations). Every input `.eh_frame` is
split into records; the output is the live records after de-duplication,
with one terminator, and the pointers inside them rewritten.

### 4.1 Parsing

- **S-34 [G] (records).** A record is a 4-byte length `L` and `L` bytes
  of body. `L == 0` is a terminator. `L == 0xffffffff` would introduce a
  64-bit length; it is refused (`feature`, `64-bit DWARF .eh_frame
  record`): the LSB format has no such records and the scan has none
  (X-1). The second word is the CIE id: 0 for a CIE; otherwise the
  distance back from that word to the record's CIE (an FDE).
- **S-35 [C] (accepted CIEs).** A CIE is accepted when its version is 1
  or 3 (the scan has only 1; 3 is DWARF 3's `.debug_frame` version, which
  some assemblers emit; all three accept it, X-14); its augmentation is
  empty, or `z` followed by letters from `R`, `P`, `L`, `S`, `B`, `G`, each
  at most once (`S` a signal frame, `B` an AArch64 B-key frame, `G` an MTE
  stack frame; no data, passed through; the scan has `zR`, `zPLR`, one
  `zRS`, X-1); and its `R`, `P`, `L` encodings are ones S-37 relocates.
  An empty augmentation means the FDE's initial location is `absptr`.
- **S-36 [C] (refused CIEs).** Another version, an augmentation character
  outside S-35, or augmentation data past the record is `malformed-input`:
  `FDE version 1 or 3 expected, but got N`, `unknown .eh_frame
  augmentation string: STR` (lld's wording and cases, X-14). GNU warns
  and links without `.eh_frame_hdr`, silently losing the unwinder's fast
  path; mold copies. None in the scan.
- **S-37 [C] (pointer encodings).** A pointer is located by its encoding
  and relocated by the relocation at its place, if any. Formats
  `absptr`, `udata2/4/8`, `sdata2/4/8`, applications none and `pcrel`
  (`0x10`), optionally `indirect` (`0x80`), are accepted; `datarel`,
  `textrel`, `funcrel`, `aligned` and `uleb128`/`sleb128` in a relocated
  pointer are refused (`feature`, `.eh_frame pointer encoding 0xNN`): no
  producer emits them and their static base is not fixed by the LSB. The
  scan has FDE `0x1b`, personality `0x9b`, LSDA `0x1b` (X-1); hod's large
  model uses `0x1c` with `PREL64` (a64.md §13.1); GCC's non-PIC x86-64
  code (`-fno-pic -fno-PIE`) uses `0x03` (`udata4`) with `R_X86_64_32`
  for all three (WP-025m corpus).
- **S-38 [C] (FDE code range).** An FDE's code section is the section of
  the target of the relocation at its initial-location field (offset 8
  of the record). The relocation's class must be `Pc` for a `pcrel`
  pointer and `AbsWord` or `AbsNarrow` otherwise (arch.md §3.9), with the
  pointer's width, at the pointer's first byte; another is refused there
  (`feature`, `R_NAME at an .eh_frame pointer of another encoding`). The
  same holds at personality and LSDA pointers. A relocation of type
  `R_AARCH64_NONE`, `R_X86_64_NONE` or `R_386_NONE` anywhere in
  `.eh_frame` is ignored: a field whose only relocations are of that type
  has no relocation; `ld -r` outputs carry them (X-30). An FDE whose
  initial location has no relocation is dropped when the field is zero
  (lld and mold drop such FDEs; GNU keeps them and gives up on
  `.eh_frame_hdr`, X-14): it describes no code of this link, and the
  verifier cannot judge it either (`plumb.md` §6.4, R-P7). When the
  field is non-zero the FDE claims an absolute code address, which the
  verifier would expect in the output (E2) and no section of the link
  holds: it is refused (`feature`, `FDE without a relocation for its
  initial location`). Neither occurs in the scan. An FDE whose target is
  undefined weak, absolute, or in a section that is not live is dead
  (S-41).
- **S-39 [C] (malformed records).** These are `malformed-input`, with
  lld's wording:
  - a record whose length runs past the section end: `CIE/FDE ends past
    the end of the section`;
  - an FDE whose CIE pointer does not land on the start of a CIE of the
    same section: `invalid CIE reference`;
  - any byte after a terminator in the same input section: `relocation
    is not in any piece` when a relocation is there, else `data after
    the .eh_frame terminator`;
  - a relocation in the section whose place is not inside a record:
    `relocation is not in any piece`.

  GNU warns and drops the header table in each case; lld refuses each;
  mold accepts some and crashes on a truncated record (X-14). The scan
  has terminators only at the end of `crtend.o` and `crtendS.o` (X-1).
- **S-40 [C] (accepted oddities).** These are accepted, as by all three
  (X-14):
  - a record length that is not a multiple of 4: the output record is
    padded by S-47 (the scan has none, X-24);
  - a CIE that no FDE uses: accepted and dropped (S-42);
  - several CIEs in one section (up to three in the scan);
  - FDEs with the same initial location when no table is written (S-59
    refuses them with one).

### 4.2 Liveness and de-duplication

- **S-41 [C] (dead FDEs).** An FDE is live exactly when its code section
  is live and not folded away (model §3.6) and S-38 does not drop it: FDEs
  of losing COMDAT copies (resolve.md §6.3) and of sections removed by
  `--gc-sections` are dead. All three references agree on the count
  (X-10). A dead FDE is **absent**: mortar never writes a neutralised FDE
  (initial location or range 0), so every output FDE answers for one live
  input FDE (`plumb.md` §6.4 E2 to E4). S-48's terminator is always
  written.
- **S-42 [C] (live CIEs).** A CIE is live when a live FDE uses it. A CIE
  that no live FDE uses is dropped. (lld and mold drop unused CIEs; GNU
  keeps CIEs of sections it keeps. Nothing reads a CIE except through an
  FDE.)
- **S-43 [C] (CIE identity).** Two live CIEs are one when their bytes are
  equal and, at each relocated place, the relocations have the same type
  and addend and targets resolving to the same final symbol or section
  and offset. Two `zPLR` CIEs equal in bytes but with different
  personality targets are two CIEs (X-11). It is computed after
  resolution, so it is exact.
- **S-44 [C] (which copy).** Among equal live input CIEs the one with the
  least `(FileId, sh_index, record)` is kept, and every live FDE that used
  any of them points to it (the three references keep the same number,
  3 in X-10). The internal file's records (S-55 [M2]) take no part: no
  input FDE points to a linker CIE or the reverse, so S-46 keeps every
  CIE before its FDEs (libgcc reads the CIE pointer as an unsigned
  distance back).
- **S-45 [C] (piece row).** `Split` holds per record its offset (`u32`)
  and an `EhPiece` (model §3.5): `cie` (for an FDE, the index of its CIE
  among the section's records; `u32::MAX` for a CIE) and `code` (the
  FDE's code section). The CIE's identity is a `FragId`-like number
  computed after resolution by the same interning method (S-28) over
  live CIEs only; there are few (thousands), so this is a small table,
  not a per-record column.

### 4.3 Output order and the terminator

- **S-46 [C] (order).** The output records are every kept live CIE
  (S-44) and every live FDE, in `SectionId` order of the input
  `.eh_frame` sections (layout L-58) and record order within each. A kept
  CIE precedes its FDEs because the kept copy is the earliest and an FDE
  uses a CIE of its own section. This is GNU's order (X-10); lld groups
  each CIE with its FDEs, mold puts CIEs first. No unwinder depends on it
  (FDEs point to CIEs by offset; the hdr sorts), and `crtbeginT.o`'s walk
  starts at the output's start (S-50, S-51).
- **S-47 [C] (packing).** Records are written back to back, with no
  padding between them, at offsets that are the sum of the preceding
  record sizes. A record's output size is `4 + align_up(L, 4)`: a body
  whose length `L` is not a multiple of 4 gets zero bytes (`DW_CFA_nop`)
  appended and its length raised to match. Instructions run to the end
  of the record, so the padding changes nothing the record states, and
  every output record stays 4-aligned. GNU pads the same way, after
  removing trailing `DW_CFA_nop`s; lld and mold copy the bytes and leave
  later records unaligned (X-26). The verifier ignores trailing
  `DW_CFA_nop`s (`plumb.md` §6.4). The scan has only 4-byte multiples
  (X-24). The chunk's alignment is the largest input `.eh_frame`
  alignment (8 on the 64-bit targets, 4 on i386; X-24).
- **S-48 [G] (one terminator, last).** Input terminators are dropped,
  and the chunk ends with one 4-byte zero word, which libgcc's
  `__register_frame_info` walk (static start-up through `crtbeginT.o`)
  needs to stop. The result is then independent of where `crtend.o` is:
  with it before the user's objects GNU's output has the terminator in
  the middle and crashes in the unwinder; lld and mold write it last and
  run (X-13). A link with no `crtend.o` gets one too (lld, mold; GNU
  none). This changes layout L-58 (§14).
- **S-49 [C] (Slots).** `Slots.eh_records` lists the output records in
  order as `(SectionId, record index)`; the terminator is implied. It is
  a decision of the layout (`spec/layout.md` §4.3): S-41 to S-46 choose
  it in a link that computes its layout, and the output `.eh_frame` is a
  function of the list whoever chose it (WP-086). An FDE's CIE is its
  own input CIE when the list holds it, else the nearest earlier listed
  CIE of the same identity class (S-43); an FDE of the internal file
  takes only its own (S-55). The size is the sum of the records' output
  sizes (S-47) plus 4; offsets are a prefix sum, parallel by section
  (S-56). A list the rules do not admit fails the link by name:
  1. a row that names no record of a live `.eh_frame`;
  2. rows out of S-46's order, or one record twice;
  3. an FDE that is not live, or a live FDE not listed (S-41; checked
     by `Layout::check`);
  4. an FDE with no CIE to take by the rule above;
  5. a listed CIE that no listed FDE takes (S-42).

  So the only freedom a list has beyond S-41 to S-46's choice is to keep
  a CIE that S-44 would remove, which its section's FDEs then take
  (IN-34a keeps one per file this way). The plan and the bytes depend on
  the list only, never on how it was chosen.

### 4.4 References into .eh_frame

`.eh_frame` is referenced from inside (an FDE's CIE pointer, which S-52
rewrites and nothing resolves), from `crtbeginT.o`'s `frame_dummy` (a
relocation against the section at offset 0, where `__EH_FRAME_BEGIN__`, a
local symbol, also lies), from `.eh_frame_hdr`, and by symbols defined in
`.eh_frame`: `__EH_FRAME_BEGIN__`, `__FRAME_END__` (`crtend.o`'s
terminator) and AArch64 `$d` mapping symbols at record starts (355 in a
static C hello; 4396 in the reviewer's scan).

The `crtbeginT.o` reference differs by target. On x86-64 and i386 its
`.eh_frame` is empty, so the reference is offset 0 of a 0-byte section
(`R_X86_64_32 .eh_frame+0`, `R_386_32 .eh_frame`): every static x86 link
has it [M3, M4]. On AArch64 [M1] its `.eh_frame` holds a CIE and FDEs,
and the reference is an `ADRP`/`ADD` pair to offset 0, its CIE, which is
usually a duplicate of `crt1.o`'s and removed by S-44 (X-12, X-25).

- **S-50 [C] (the value of a reference into .eh_frame).** This rule gives
  the value of every relocation whose target is in an input `.eh_frame`
  section `E`, and of every symbol defined in one other than a mapping
  symbol. Let `r` be the target offset (`st_value + A` for a section
  symbol; `st_value` for a symbol):
  1. `r = 0`: the start of the output `.eh_frame`, whatever `E` holds
     (nothing, a kept first record, or a removed one);
  2. otherwise, `r` at the start of or inside a kept record: the
     record's output offset plus `r` minus its input offset;
  3. otherwise, `r` at the start of a removed record (a CIE de-duplicated
     away, a dead FDE, an FDE dropped by S-38, an input terminator), or
     `r = sh_size`: the output offset of the next kept record of `E`, or
     if none, of the next kept record after `E`'s records in output
     order, or the final terminator. This is where the removed record
     would have started;
  4. otherwise (`r` inside a removed record, not at its start): a
     relocation is refused, `malformed-input`, `reference into a removed
     .eh_frame record`. A symbol is never refused: a local symbol that no
     relocation refers to is dropped from `.symtab`; any other symbol
     takes rule 3's value for the record it lies in;
  5. `r < 0` or `r > sh_size`: `malformed-input`, `offset is outside the
     section` (S-18's message).

  A mapping symbol is kept, at its record's output place, exactly when
  its record is kept; otherwise it is dropped (S-76). A named symbol and
  a relocation through it always agree, since the symbol's value is
  computed once by these rules.

  Rule 1 is the one that does not follow the record. It holds for every
  input section, not for `crtbeginT.o` by name: a reference from code to
  the first byte of an input `.eh_frame` is the start of a frame list to
  walk (`__register_frame_info`), and only the output's start gives a
  walk that sees every FDE. With GNU's value (rules 2 and 3 at `r = 0`),
  a C++ `throw` through a function of an object placed before
  `crtbeginT.o` calls `std::terminate` on all three targets; with lld's
  and mold's (the start) it is caught (X-25). For offset 0 of a section
  whose first record is kept but not first in the output, GNU and lld
  give that record's place and mortar the start (§13); no input in the
  scan has such a reference outside the crt files. `__EH_FRAME_BEGIN__`
  is therefore the start of `.eh_frame` in `.symtab` too (lld's value;
  GNU's lies 16 to 40 bytes before the section; mold drops it).
- **S-51 [C] (the walk is complete).** With S-48 and S-50 rule 1, the
  walk from `__EH_FRAME_BEGIN__` covers every live FDE, wherever the crt
  files and other objects are on the line, including an object before
  `crtbeginT.o`, which GNU's output fails (§16).

### 4.5 Contents

- **S-52 [G] (rewriting).** The image phase writes each kept record's
  bytes from its input, then:
  - in an FDE, the CIE pointer: the distance from the pointer's own
    output address to the start of the kept CIE (S-44), as the LSB
    defines it;
  - every relocated field (initial location, personality, LSDA, and any
    other relocated place) by the ordinary relocation rules
    (`spec/image.md`), with `P` the field's output address;
  - nothing else. The CIE's and FDE's other bytes, the call frame
    instructions and the augmentation data that is not relocated are
    copied unchanged.
- **S-53 [G] (LSDA and personality).** An LSDA pointer refers into
  `.gcc_except_table`; one whose target section is not live (a dropped
  COMDAT copy) can only be in a dead FDE, which is not written. The
  personality pointer, in a kept CIE, refers to a `DW.ref.*` word (with
  `indirect`) or to the routine (without); both are ordinary relocations.
- **S-54 [C] (x86-64 type).** On x86-64 an input `.eh_frame` may be
  `SHT_X86_64_UNWIND` (Clang) or `SHT_PROGBITS` (GCC); both are split by
  this section, and the output type is layout L-49's.
- **S-55 [M2] (PLT unwind information).** A dynamic output's PLT has no
  FDE at M2 (lld writes none; GNU writes one for x86-64's lazy `.plt`);
  gdb and libgcc unwind through a PLT entry without one on all three
  targets (Q6). The shape allows linker-made records after the inputs'
  (`FileId` 0, ordered last for `.eh_frame` only): `Slots.eh_records`
  admits the internal file's sections. They carry their own CIE and are
  excluded from S-44, so no FDE points forward to its CIE.

### 4.6 Cost

- **S-56 [C].** For 10^6 FDEs (about 40 MB of input): parsing is linear
  and parallel by section; liveness is a lookup per FDE; identity interns
  only the CIEs (thousands); offsets are a per-section count and a prefix
  sum; `Slots.eh_records` is 8 MB; writing is a copy plus three or four
  field rewrites per record, parallel by range. Measured (WP-025's bench,
  10^6 input FDEs in 10^4 sections, 92% live, 22 MB written; median of
  5, load average 24, after the join with `mortar_merge`): plan
  12/15/13/12 ms and write 20/12/7.6/4.3 ms on 1/2/4/8 threads. Parsing
  and CIE identity (`mortar_merge`) are not included; the plan does not
  scale with threads (its per-record passes are sequential).

## 5. .eh_frame_hdr

- **S-57 [C] (when).** `.eh_frame_hdr` and `PT_GNU_EH_FRAME` exist
  exactly when `--eh-frame-hdr` is given and some FDE is live; the last
  of `--eh-frame-hdr` and `--no-eh-frame-hdr` wins. Without the option
  there is none, as with GNU and lld; mold always writes one
  (`tests/link/layout/eh-frame-hdr`). The GNU driver's behavior is the
  contract: GCC passes `--eh-frame-hdr` for every link except `-static`
  (`%{!static|static-pie:--eh-frame-hdr}`), so a static link has no table
  and libgcc finds the FDEs through `__register_frame_info` (S-50, S-51).
- **S-58 [G] (format).** The LSB format: version 1; `eh_frame_ptr`
  encoding `0x1b` (`pcrel|sdata4`), `fde_count` encoding `0x03`
  (`udata4`), table encoding `0x3b` (`datarel|sdata4`); then
  `eh_frame_ptr` (to the start of `.eh_frame`), `fde_count`, and
  `fde_count` pairs `(initial location, FDE address)`, each relative to
  the start of `.eh_frame_hdr` (arch.md §3.9). Size `12 + 8 × entries`,
  known in phase 80 (S-59). Alignment 4.
- **S-59 [C] (the entries).** One entry per live FDE, except:
  - an FDE with address range 0 is written but has no entry: it covers
    no address, and an entry for it would hide a real FDE with the same
    start from the binary search. This is what E5 calls neutralised.
    Reference note (supervisor's ruling after WP-030c; mortar's rule
    stands): GNU ld drops the range-0 FDE itself (glibc i386
    `clone3.o`), lld writes the FDE and no entry, and mold lists it in
    the table (plumb A19);
  - two live FDEs with the same initial location and non-zero ranges are
    refused when the table is planned (phase 80, where the entries are
    first known as `(code section, offset)` pairs; supervisor's ruling on
    WP-025), and again if two entries meet at one address when the table
    is written: `malformed-input`, `.eh_frame_hdr
    refers to overlapping FDEs` (GNU's message and behavior; lld keeps
    one entry, mold both, X-15). The table must be strictly sorted and
    list every FDE that is not neutralised (`plumb.md` §6.4 E5), and each
    output FDE answers for one input FDE (E3), so no output with both and
    a table passes the verifier. Without the table both are kept. The
    reviewer's scan has no such pair anywhere.

  The count is fixed in phase 80: initial locations are `(section,
  offset)` pairs before layout, and two FDEs share one exactly when they
  name the same section and offset (or the same folded target, model
  §4.1); distinct live sections never overlap (model §4.4).
- **S-60 [C] (overlapping ranges).** FDEs whose ranges overlap without
  sharing a start are accepted, all with entries (lld and mold; GNU fails
  the link, X-15). libgcc's binary search finds the FDE with the greatest
  start not above the address, whatever the linker does; the verifier
  checks only that starts strictly increase (E5).
- **S-61 [G] (sorted).** The table is sorted by initial location,
  ascending, as signed 32-bit offsets from the header (the same as by
  address: all code lies within ±2 GiB of the header in every M1 to M4
  code model; layout L-36 refuses the others). The image phase writes
  the pairs and sorts them (10^6 entries, about 10 ms on 8 threads); no
  two keys are equal after S-59.
- **S-62 [C] (an address the table cannot hold).** An offset that does
  not fit `sdata4` is `relocation-out-of-range` against `.eh_frame_hdr`;
  with layout's placement (rank 5, before `.text`) it cannot occur below
  2 GiB of code.

## 6. GOT, .got.plt, the IPLT and IRELATIVE (static, M1)

The per-architecture slot contents, entry shapes and relocation types
are in a64.md §10 (and `x64.md`, `x86.md`, planned). This section fixes
what is common: which chunks, the order of slots, the header words, and
which values are written where.

### 6.1 .got

- **S-63 [C] (regions).** `.got` holds the slot kinds in this order,
  each kind a contiguous region, its slots in `Slots` order:
  1. the header word, when present (S-65);
  2. `Addr` slots (`GOT`, AArch64 `GDAT`), in `Slots.got` order;
  3. `TpOff` slots (`GOT_TPREL`), `Slots.tls_ie` order;
  4. `TlsGd` pairs (`TLS_GD`), `Slots.tls_gd` order;
  5. the `TlsLd` pair (`TLS_LD`), when demanded;
  6. `TlsDesc` pairs (`TLS_DESC`), `Slots.tls_desc` order.

  A slot's offset is the region's base plus its index times the kind's
  size (`GotShape.slot_words × word`). The region bases are prefix sums
  of the region capacities (S-113); at M1 a region's capacity is its
  count. This is a64.md §10.1's order ("in Slots order", with the kinds
  as listed there).
- **S-64 [C] (slot order).** `Slots` lists each kind's keys in `SymKey`
  order (globals by `SymbolId`, then locals by `(FileId, index)`): a pure
  function of the ids, computed by a parallel collection of demands and a
  sort by key (model §6 rule 4). The demand scan already sorts by key, so
  this is a filter.
- **S-65 [C] (the header word).** On AArch64 `.got` starts with one
  header word when `GOT_BASE` is demanded (a reference to
  `_GLOBAL_OFFSET_TABLE_`, layout L-101), and in every output with
  `.dynamic` [M2] (a64.md §10.1, its value `_DYNAMIC`). In a static
  output the word is 0. On x86 `.got` has no header; the GOT base is
  `.got.plt` (S-67). At M7, an output linked under an incremental policy
  always has the header word (S-115), so that a later reference to
  `_GLOBAL_OFFSET_TABLE_` moves no slot.
- **S-66 [C] (contents, static).** Every `.got` word is written by the
  image phase; none carries a relocation in a static output except
  through `.rela.plt` (S-69). Static values (a64.md §10.1): `Addr` the
  target's address (for an IFUNC, its IPLT entry); `TpOff` the TP offset;
  `TlsGd` the pair (1, DTP offset); `TlsLd` (1, 0); `TlsDesc` relaxed
  away (none in a static output, a64.md §10.6). These values depend on
  addresses, so the chunk's content is the image phase's, and its size is
  phase 80's.

### 6.2 .got.plt, .plt and .rela.plt for IFUNC

- **S-67 [C] (.got.plt).** In a static output `.got.plt` holds:
  - on x86-64 and i386: three reserved zero words, then one word per IPLT
    entry (GNU and lld keep the three in a static link with only an
    IFUNC, X-22); `_GLOBAL_OFFSET_TABLE_` is its start (layout L-101).
    Keeping them whenever `.got.plt` exists means a new `GOTOFF`
    reference moves nothing (x86-64 [M1], `spec/x64.md` X-36); x86's crt files demand `GOT_BASE` anyway;
    under `-static-pie` on i386, word 0 is `_DYNAMIC`'s link-time
    address (the start of `.dynamic`, with no `RELATIVE`), words 1 and 2
    are 0: GNU ld, lld and mold all write this (`spec/x86.md` X86-47,
    R-16). glibc 2.41 does not read it; glibc before 2.35 did. [the rule
    now; the end-to-end gate M2] (x86-64 was not measured; x64.md X-36
    may want the same check);
  - on AArch64: one word per IPLT entry (a64.md §10.2; X-22).

  IPLT slot `n` is word `reserved + n`. A `.got.plt` with no words does
  not exist.
- **S-68 [C] (IPLT entries).** Each key in `Slots.iplt` has one IPLT
  entry of `PltShape.iplt_entry` bytes in `.plt`, after any PLT entries
  [M2], in `Slots.iplt` order, with no PLT header in a static output
  (a64.md §10.4). The entry bytes are `arch.write_plt(variant,
  Iplt(n), cx)` with `cx` holding the entry's address and its `.got.plt`
  word's address. The section names are GNU's: `.plt`, `.got.plt`,
  `.rela.plt` (`.rel.plt` on i386). lld uses `.iplt` and `.rela.dyn`,
  mold `.plt.got` and `.rela.dyn` (X-22); glibc reads only the symbols of
  S-70. This answers a64.md Q9.
- **S-69 [G] (IRELATIVE).** Each IPLT entry has one `IRELATIVE`
  relocation in `.rela.plt` (`R_AARCH64_IRELATIVE`,
  `R_X86_64_IRELATIVE`, `R_386_IRELATIVE`), in IPLT order, with
  `r_offset` the entry's `.got.plt` word, symbol 0, and:
  - RELA (AArch64, x86-64): `r_addend` the resolver's address; the word
    itself, per architecture (`Facts.iplt_word`, arch.md §3.1;
    `spec/dyn.md` §15 Q7, ruled; arch.md §3.10's `IRelative` row):
    - AArch64: `S+A`, the resolver's address (a64.md §6.2, §10.2's
      rule; glibc ignores the word under RELA and reads the addend, and
      the verifier accepts it);
    - x86-64: 0 (`spec/x64.md` X-39; plumb F5(3)).

    WP-041 implemented this, changing the AArch64 bytes (static outputs
    with an IFUNC included) before the M2 static baseline was cut. The
    references (not the reason for the rule): GNU ld 2.44 writes the
    start of `.plt` on AArch64 (measured by WP-041; static) and the
    entry's address + 6 (the lazy-PLT convention) on x86-64
    (`spec/x64.md` E3); lld writes 0 (X-22); glibc overwrites the word
    under RELA in every case;
  - REL (i386): the word itself holds the resolver's address, which is
    the implicit addend (GNU and lld, X-22; unchanged by Q7).

  On a REL target (i386) the `RELATIVE` addend, where synth places one
  (the static-pie GOT slots and address words [M2]), is likewise the
  initial content of the relocated word: synth writes the word, and the
  record has no addend field (`spec/x86.md` §6.2, R-4).

  `sh_link` is 0 in a static output, `sh_info` the index of `.got.plt`
  with `SHF_INFO_LINK` (gABI; GNU on all three targets; lld writes 0 on
  its `.rela.dyn`, X-22). Dynamic outputs put `IRELATIVE` in `.rela.plt`
  after the `JUMP_SLOT`s [M2].
- **S-70 [G] (`__rela_iplt_start`, `__rela_iplt_end`).** In a static
  output these hidden symbols (on i386 `__rel_iplt_start` and
  `__rel_iplt_end`) are the start and end of `.rela.plt`'s IRELATIVE
  entries; with no IFUNC they are equal (both at the address `.rela.plt`
  would have, layout §13). glibc's static start-up applies every entry
  between them. They are `STB_LOCAL` in `.symtab` (layout §13).

  Under `-static-pie` [the rule now; the end-to-end gate M2, `docs/ROADMAP.md`] every `IRELATIVE` is in `.rela.dyn`
  (`.rel.dyn`), applied by self-relocation, and there is no `.rela.plt`;
  `__rela_iplt_*` (`__rel_iplt_*`) are not defined, so glibc's weak
  references are 0, an empty range (resolve.md §8.1, WP-022 ruling 7; lld. GNU ld instead keeps the
  `IRELATIVE`s in `.rela.plt`, reached through `DT_JMPREL`, with one more
  in `.rela.dyn`; mold puts them in `.rela.dyn` and defines the symbols
  absolute 0; all three run; `spec/x64.md` X-56). Bounding the entries would make
  glibc apply each a second time, with an unrelocated addend.
- **S-71 [C] (the IFUNC's address and its symbol).** Every address of an
  IFUNC `f` with an IPLT entry that a relocation writes (a call, a GOT
  slot, an absolute or PC-relative address in code or data) is that
  entry's address (a64.md §10.4). This includes the canonical case,
  where non-PIC code or data takes `f`'s address and the IPLT entry
  becomes `f`'s one address for pointer comparison.

  Exception, i386 static-pie (`spec/x86.md` X86-50, R-14): the IPLT entry
  is `ff a3 <word − GOT>`, which needs `%ebx` = the GOT at the jump, so it
  cannot be the one address of `f`. Every address of `f` that a
  relocation writes is instead a word holding the resolved function, with
  an `IRELATIVE` whose implicit addend is the resolver (the `GDAT` slot,
  `32` in data); only `PLT32` calls go to the entry; `GOTOFF` and `PC32`
  against `f` are refused (`refused`, class `feature`, X86-50). lld 22
  makes the `ff a3` entry the canonical address and its output crashes
  when the pointer is called with `%ebx ≠ GOT` (`spec/x86.md` §15.2 W6).
  [the rule now; the end-to-end gate M2]

  The next paragraph is `.symtab`'s rule. In `.dynsym` [M2] an exported
  non-preemptible IFUNC is `STT_FUNC` at its IPLT entry (`spec/dyn.md`
  D-23 and D-45; a64.md §10.4): the loader must not run the resolver for
  a reference that binds to the executable.

  In `.symtab`, `f` keeps `STT_GNU_IFUNC` with the resolver's value,
  section and size in every case, the canonical one included: GNU's form
  on all three targets. lld writes a canonical IFUNC as `STT_FUNC` at its
  `.iplt` entry with size 0; mold writes `STT_FUNC` locals at `.plt.got`
  (`plumb.md` §10.2 A1, A2). With GNU's form gdb's `break f` stops in
  the implementation and `print f(3)` works; with lld's, `break f` stops
  in the IPLT stub and `print f(3)` fails (X-27). The one cost is that
  the address the program holds (the IPLT entry) has no symbol: `info
  symbol` on a pointer to `f` finds nothing (Q11). The verifier's Y3
  leaves IFUNC names unchecked, so both forms pass it.

### 6.3 Static TLS in the GOT

- **S-72 [C].** A static output's TLS GOT slots are `TpOff` slots holding
  the TP offset (IE not relaxed: `TLSIE_NORELAX`, arch.md §4.2) and, for
  GD or LD sequences that the architecture does not relax, the `TlsGd`
  and `TlsLd` pairs with module id 1 (the executable's, which is what
  glibc's static `__tls_get_addr` expects). AArch64 relaxes every
  TLSDESC in a static output (a64.md §10.6), so `TlsDesc` regions are
  empty at M1.

### 6.4 Undefined symbols

- **S-125 [C].** `mortar_synth` reports no undefined-symbol diagnostic.
  A strong reference to an undefined symbol is reported once per key by
  the scan driver (`spec/image.md`), whatever demands (GOT, PLT, IPLT)
  the reference makes; synth sees only the demands that survive, and a
  slot for an undefined weak key holds 0 (a64.md §10.1 `weak0`).

## 7. .symtab, .strtab, .shstrtab, .symtab_shndx

### 7.1 .symtab

The gABI "Symbol Table" requires the null entry first, every
`STB_LOCAL` symbol before every other, and `sh_info` one greater than the
last local's index. Beyond that, the content is mortar's choice, made for
gdb, lldb, perf, nm, addr2line, objdump and valgrind (§12).

- **S-73 [G] (structure).** `.symtab` is `SHT_SYMTAB`, `sh_entsize` 24
  (ELF64) or 16 (ELF32), alignment 8 (4), `sh_link` the index of
  `.strtab`, `sh_info` the number of local entries including the null
  entry. Entry 0 is all zeros.
- **S-74 [C] (the local part).** After entry 0, the locals are in blocks:
  1. the **internal file's block** (`FileId` 0): the linker-defined
     symbols local in the output (layout §13: `__ehdr_start`,
     `_GLOBAL_OFFSET_TABLE_`, ...) in `SymbolId` order, then the thunk and
     patch symbols (S-80) in `Placement.patches` order, before every
     `STT_FILE`, so no tool scopes them to a file;
  2. per input file in `FileId` order: its `STT_FILE` symbols and kept
     locals in table order, then its **demoted** symbols (S-77) in
     `SymbolId` order; a file with neither has no block.

  This is lld's order (X-17; GNU keeps hidden symbols global).
- **S-75 [C] (the global part).** After the locals: every global that the
  output keeps (resolve.md §3.7, its output binding not `STB_LOCAL`), in
  `SymbolId` order. `STB_GLOBAL`, `STB_WEAK` and `STB_GNU_UNIQUE` keep
  their binding (GNU and lld keep `STB_GNU_UNIQUE` in a static output,
  resolve.md §3.7, one rule with dyn.md D-23; `resolve/symtab-unique`,
  WP-059). mold makes every symbol local in a static
  output; GNU and lld do not (`tests/link/resolve/symtab`). A global
  defined in a section that is not live has no entry, as a local in one
  has none (S-76): at M1 `--gc-sections` is refused, so such a section is
  a discarded COMDAT member or a section discarded by the script
  (supervisor's ruling on WP-025).
  An undefined key whose combined visibility is `STV_HIDDEN` or
  `STV_INTERNAL` (a weak hidden reference no object defines, as glibc's
  `_nl_current_LC_*` and `_IO_cookie_*` in a static link) has no entry:
  the gABI ("Symbol Visibility") requires a hidden symbol to be removed
  or made `STB_LOCAL` in an executable, and an undefined key has no
  file block to be local in. This is GNU's form; lld writes it as an
  `STB_LOCAL` `SHN_UNDEF` entry (WP-027a, GCC 14 static C program:
  `_IO_cookie_close` absent in GNU 2.44, `LOCAL HIDDEN UND` in lld 19).
- **S-121 [G] (bindings are the inputs').** A global's output binding
  is resolve.md §3.7's and nothing else: `STB_LOCAL` only for a
  definition whose combined visibility is hidden or internal (S-77), and
  otherwise the binding of the chosen definition (`STB_WEAK` when every
  definition is weak). A static executable's default-visibility globals
  (`main`, `_start`, libc's) stay `STB_GLOBAL`. mold makes them all
  `STB_LOCAL` (`plumb.md` §10.2 anomaly A9; X-17), which `nm` and the
  debuggers then show as locals; mortar must not, and the verifier's Y3
  checks it. Type, size and visibility are the chosen definition's, the
  visibility being the most constraining over every non-local input
  entry of the name (gABI "Symbol Visibility"; Y3).
- **S-127 [C] (`st_other` above the visibility).** The bits of a
  global's `st_other` above the visibility (the gABI leaves them to the
  processor; of the three targets only AArch64 defines one,
  `STO_AARCH64_VARIANT_PCS`, 0x80: the function follows the vector or
  SVE calling convention) are, for a definition (demoted ones of S-77
  included), the defining entry's bits; for a common, the chosen common
  entry's; for an undefined entry (a referenced weak undefined key, S-75)
  on AArch64, `STO_AARCH64_VARIANT_PCS` when some live reference has it
  (resolve.md §3.7: `RefFlags::VARIANT_PCS`), and no other bit; for a
  linker-defined symbol (§8.1 of resolve.md), none. A local of an input
  file keeps its whole `st_other` (S-76). In every output kind (static,
  PIE, static-pie, `-no-pie`). Measured (WP-059 §2, GCC 14
  `aarch64_vector_pcs` functions: global, weak, protected, hidden and
  `static`): GNU ld 2.44, lld 19 and mold 2.37.1 keep the bit on every
  such entry in static, PIE and static-pie outputs; mold writes the
  entries `STB_LOCAL` (A9), GNU ld's demoted entry is `STV_DEFAULT` (its
  hidden form, S-77). Where they differ the rule is argued:
  - a definition without the bit and a reference with it: GNU ld ORs
    every entry's bits and writes it; lld and mold write the definition's
    (none). The bit describes the code at the symbol's address, which is
    the definition's; a debugger, `perf` and an unwinder reading
    `.symtab` need the convention that code follows, so the definition's
    bits are written (lld, mold; `spec/dyn.md` D-23's export row is the
    same rule for `.dynsym`);
  - an undefined weak entry: lld and mold write the first mention's bits
    (by input order: the bit or not depending on the command line), GNU
    ld has no entry in a static output and ORs in a PIE. An undefined
    entry describes no code; the OR is GNU's, does not depend on the
    input order, and is what the loader needs on a `.dynsym` import
    (`spec/dyn.md` D-23's `WeakDyn` row), so `.symtab` writes the same.
  Before WP-059 mortar wrote only the visibility (`STO_AARCH64_VARIANT_PCS`
  was lost on every global; WP-053 §4). *Test:*
  `tests/link/resolve/symtab-variant-pcs.test`.
- **S-76 [C] (which locals).** A local of an input file is kept when:
  - it is not `STT_SECTION` (S-79);
  - its section is live (or it is `SHN_ABS`), and, if its section is
    split: for a `Merge` section its piece's fragment is live (S-22); for
    `.eh_frame`, S-50 gives it a value (a mapping symbol in a removed
    record, and a local in the interior of one that no relocation refers
    to, are dropped; every other symbol in `.eh_frame` is kept, at S-50's
    value);
  - it is not discarded by the `-X`/`-x` rule (S-78).

  Locals in sections that are not live are dropped (GNU's
  `--strip-discarded`, its default, and lld). A section the link
  consumes (layout §8.1: `.note.gnu.property`, `.note.GNU-stack`,
  `.gnu.warning*`, `.ARM.attributes`) counts as not live here: it has
  no output section, so a symbol in it has no value. glibc's and
  libgcc's objects each carry a `$d` at the start of their
  `.note.gnu.property`; GNU and lld drop them (WP-027a: 386 such
  entries in a GCC 14 static C link, which also had no address to
  write). Locals in a folded section
  [ICF] take the place of the fold's target.
- **S-77 [C] (demoted globals).** A global whose output binding is
  `STB_LOCAL` (a hidden or internal definition, resolve.md §3.7) is a
  local of its defining file's block. Its `st_other` keeps its
  visibility (`STV_HIDDEN`). A linker-defined symbol that is local in the
  output is in the internal file's block. Layout writes only the values
  of linker-defined symbols (layout L-91); their binding and visibility
  are resolve.md §8.1's column, which this rule writes: "local, hidden"
  is `STB_LOCAL`, `STV_HIDDEN` in the internal block; "global" is
  `STB_GLOBAL`, `STV_DEFAULT`; `__start_`/`__stop_` are `STB_GLOBAL`,
  `STV_PROTECTED`. This is lld 22's `.symtab` exactly; GNU ld 2.44
  writes the same bindings with `STV_DEFAULT` on its locals and makes
  `__executable_start` global; mold makes all of them local
  (measured on AArch64, a static C program referencing each name).
  Y5 of the verifier judges the values only.
- **S-78 [C] (`-X`, `-x`, `--discard-none`).**
  - `-X` (`--discard-locals`): discard locals whose name begins
    with `.L` (the ELF temporary-label prefix; GNU and lld; mold keeps
    them; X-17). Mapping symbols (`$x`, `$d`, `$x.<n>`, `$d.<n>`) and
    `STT_FILE` are kept;
  - default (none of the three): discard the `.L` locals of split
    `Merge` sections only (S-5), keep the others. This is GNU's default
    (`discard_sec_merge`) and lld's: the assembler keeps a `.L` symbol in
    a mergeable section for its relocations, and the linker drops it.
    GCC's driver passes `-X`; Clang's does not, and lld 19 then keeps
    libstdc++'s `.LANCHOR0` (WP-027a, Clang 19 static C link: three
    `.LANCHOR` locals; the earlier text treated the default as `-X`);
  - `-x` (`--discard-all`): discard every local of every input file,
    `STT_FILE` and mapping symbols included (GNU and lld, X-17). Demoted
    globals (S-77) and the internal file's block are kept: they were not
    input locals (lld keeps a demoted hidden symbol under `-x`, X-17);
  - `--discard-none`: keep every local, `.L` names included (GNU, lld;
    mold rejects the option, X-17).

  The last of these on the command line wins. `-x` and `-X` do not
  affect globals.
- **S-79 [C] (section symbols).** mortar writes no `STT_SECTION`
  symbols. lld writes none; GNU writes one per output section on AArch64
  and none on x86-64 (X-17); mold writes one per output section. No tool
  needs them in an executable: section symbols exist for relocations,
  and an executable has none against them. (`-r` output, M4, writes
  section symbols, since its relocations need them; that is `-r`'s spec.)
- **S-80 [T] (thunks and patches).** Each range-extension thunk and
  erratum patch (layout §14) gets, in the internal file's block: on
  AArch64 a `$x` at its start and a `$d` at any literal word (AAELF64
  "Mapping symbols"); and an `STT_FUNC` local `__<target>_veneer` (GNU's
  AArch64 form; for a non-zero addend `+0x<addend>` (`-0x<magnitude>`
  for a negative one) in lower-case hex follows the target's name, before
  `_veneer`, as `__foo+0x10_veneer`;
  `@plt` for a `Plt` target, in the same place), `st_size` the thunk's
  size (`arch.thunk()`); a patch is `__erratum_843419_veneer`, `st_size`
  its shape's (`arch.patch(E843419)`). (Placement of the addend:
  supervisor's ruling on WP-025.) gdb and perf then attribute a thunk to a
  named function (lld names its thunks `__AArch64ADRPThunk_<target>` for
  the same reason). Both forms are in the verifier's not-judged list
  (`plumb.md` §6.5); the `$x` at an address no input occupies is Y4's
  second exception.
- **S-81 [G] (values).** `st_value` is the symbol's output address:
  `place(section) + st_value` for a symbol in a placed section; the S-19
  or S-50 value in a split section; `SHN_ABS` values unchanged; for
  `STT_TLS`, the offset from the start of the TLS template (`PT_TLS`'s
  `p_vaddr`), as all three write (X-17). A common symbol has the address
  of its `.bss` allocation (layout L-88) and is `STT_OBJECT` in `.bss`;
  `SHN_COMMON` never appears in an executable. An IFUNC keeps its
  resolver's value (S-71). `st_size` is the input's.
- **S-82 [G] (section indices).** `st_shndx` is the output section index
  of the symbol's chunk (layout L-86), `SHN_ABS` for absolute symbols
  and linker-defined absolute symbols, `SHN_UNDEF` for an undefined weak.
  An index ≥ `SHN_LORESERVE` (0xff00) is written as `SHN_XINDEX`, with
  the real index in `.symtab_shndx` (S-85).
- **S-122 [G] (the index names the section that holds the address).**
  For every defined symbol that is not `SHN_ABS`, `st_shndx` names an
  output section `X` with `X.addr ≤ st_value ≤ X.addr + X.size` (offsets
  for non-alloc symbols). The end is included: an end symbol (L-91) or a
  label after the last byte of a section takes that section's index. A
  value that ends one section and starts the next takes the section of
  the symbol's anchor chunk (L-91; else the defining input section's
  output section), which holds it once empty input sections are placed
  by R-L6. GNU breaks this for crtend.o's `__TMC_END__`, whose output
  section it removes (`plumb.md` §10.2 A10); the verifier's Y3 checks it.
  The one exception is the headers: a linker-defined value in the file
  header or the program headers (layout §13.1: `__ehdr_start`,
  `__executable_start`, an absent array's base) is below section 1 and
  takes index 1, as GNU ld, lld and mold all write it (WP-027c).
- **S-123 [C] (labels of empty input sections).** A symbol defined in a
  live input section of size 0 (`__TMC_LIST__`, `__TMC_END__` in the
  crtstuff `.tm_clone_table`; `__start_`-style markers in hand-written
  objects) is valued at the empty section's place (R-L6), and its
  `st_shndx` is the section that holds that place:
  - when its output section has content, the empty section is placed
    like any other, aligned, and the section's size covers it (a
    `-fgnu-tm` table then has `__TMC_LIST__` at its aligned start, X-31);
  - when its output section has no content, its labels take the address
    of the end of the chunk layout attaches it to, unaligned, and that
    chunk's section index: aligning there could put `__TMC_END__` past
    the end of the section that holds it (S-122). All labels of one
    absent output section get that one address, so `__TMC_LIST__ ==
    __TMC_END__`.

  crtstuff needs nothing more: on all three targets `deregister_tm_clones`
  and `register_tm_clones` compare the two labels and then test the weak
  `_ITM_*TMCloneTable`, which is 0 without libitm (X-31).
- **S-124 [G] (linker-defined symbols).** Every linker-defined symbol
  that is defined (layout L-90) gets layout §13.1's value and section
  index whether or not the section it is named after exists: with no
  `.bss`, `__bss_start` is `edata` in `edata`'s section (layout §13.1),
  never 0. mold writes `__bss_start = 0` when `--gc-sections` leaves no
  `.bss` (`plumb.md` §10.2 anomaly A11); a program that clears from
  `__bss_start` to `_end` would then write from address 0. Symbols that
  layout calls absolute are `SHN_ABS`; every other one follows S-122.
  Y5 admits layout §13.1's values on AArch64 (M1). On x86 it does not
  yet: it puts `_GLOBAL_OFFSET_TABLE_` at `.got` (L-101, S-67, GNU and lld
  use `.got.plt`) and knows only `__rela_iplt_*`/`SHT_RELA` (i386 has
  `__rel_iplt_*`/`.rel.plt`); R-P5 and R-P6 fix it before M3.
- **S-83 [C] (`STT_FILE`).** An input's `STT_FILE` entries are copied in
  place, so each starts its file's block, and a file with several (an
  object made by `ld -r`) keeps each in front of the locals that
  followed it in the input. gdb uses `STT_FILE` to scope static symbols
  of the same name in different files (layout L-155). A file with no
  `STT_FILE` and with kept locals gets none, as with lld: no symbol is
  invented. GNU inserts one named after the object or member (387
  entries against lld's 3 in a static C hello). The difference is
  cosmetic (X-32): gdb files libc's locals under the user's last source
  file in its minimal symbols, but `break`, `break file:func`, `info
  symbol`, `info line`, `info scope`, `list`, `print` and `info
  functions` answer the same on both outputs.
- **S-84 [C] (`-s`, `-S`).** `-s` (`--strip-all`) omits `.symtab`,
  `.strtab` and `.symtab_shndx`, and every non-allocated section that
  layout's `-S` rule removes. `-S` (`--strip-debug`) omits the debug
  sections (layout §8.1, a request in §14) and keeps `.symtab`.
  `--strip-discarded` is accepted and changes nothing (S-76 always
  applies); `--discard-none` does not undo it.

### 7.2 .symtab_shndx

- **S-85 [G].** When any `.symtab` entry has `SHN_XINDEX`, the output has
  `.symtab_shndx` (`SHT_SYMTAB_SHNDX`, entsize 4, alignment 4, `sh_link`
  the index of `.symtab`), with one word per `.symtab` entry: the real
  section index for an `SHN_XINDEX` entry and 0 for every other (gABI
  "Extended Section Indices"). It exists exactly when the output has more
  than `SHN_LORESERVE` (65280) sections, so that some section's index
  does not fit `st_shndx`; this is known once the number of output
  sections is known (layout stage C), before content and before which
  symbols land in the high sections (supervisor's ruling on WP-025: the
  approximation by the section count is the rule). The scan
  has no input with `SHN_XINDEX` (X-1), so this is reached only by an
  output with more than 65279 sections, which needs that many distinct
  orphan names.

### 7.3 .strtab

- **S-86 [C] (layout).** `.strtab` starts with a zero byte (the empty
  name), then a fixed table of common names, then one region per block
  of S-74 and one for the globals, in `.symtab` order. A name is written
  once per entry that uses it, except that names in the fixed table are
  shared: `$x`, `$d`, the empty string, `__erratum_843419_veneer`. No other
  de-duplication is done.
- **S-87 [C] (why little de-duplication).** On a static C++ program
  (X-18), de-duplicating all names saves 13% of `.strtab` (47.0 KB to
  41.0 KB) and tail merging 19% (to 38.1 KB, GNU's size); mold does
  neither; lld de-duplicates almost nothing. The mapping symbols are most
  of the repeats (1211 of 3643 entries); the fixed table takes them.
  Full de-duplication needs a global intern of every name before any
  offset is known, a sequential dependency across files; without it each
  file's region size is the sum of its kept names' lengths plus one,
  known per file in parallel, and every entry's `st_name` is its file's
  region base plus a prefix sum within the file.
- **S-88 [C] (sizes before content).** The sizes of `.symtab` and
  `.strtab` are known in phase 80 from per-file counts (kept locals,
  demoted symbols, and the bytes of their names), the internal block,
  and the kept globals, plus the thunk and patch symbols once layout's
  fixed point is done (S-2): they are computed in parallel per file and
  combined by a prefix sum over `FileId`. `Slots.symtab_locals` holds
  each file's `(first, count)`; demoted symbols are counted per defining
  file by a pass over globals (a parallel counting sort by `FileId`).
  The spans are a table of the layout that the symbol table is held to:
  a layout whose spans are not the plan's fails the link by name
  (WP-086). In state format version 1 (`spec/incr.md` IN-18a) the plan's
  are the only admissible value, since each file's block starts where
  the one before it ends (file 0's at entry 1) and holds exactly its
  kept locals and demoted symbols; the table exists so that an update
  can find an unchanged file's entries in the previous output (IN-49a),
  which needs the previous link's spans and cannot derive them. Q8's
  per-file ranges with null-entry holes would admit other spans (a block
  with room to grow); with them the plan would place each file's block
  at its span instead of checking it.
- **S-89 [C] (cost).** For 10^7 symbols (240 MB of `.symtab` on ELF64,
  and about 300 MB of names for a large C++ program): counting is one
  parallel pass over the kept symbols; filling is parallel by window
  (S-4). No table of 10^7 rows is built beyond the existing symbol
  columns: the output index of an entry is computed from its file's base
  and its position, and the write resumes from a checkpoint every 1024
  entries. Measured (`examples/synth_bench.rs`, WP-025; AArch64, shared
  host at load average 32, 10^7 symbols, half local over 2000 files;
  240 MB of `.symtab` and 248 MB of `.strtab` written by 4 MiB windows;
  median of 5):

  | threads | plan | write | peak RSS |
  |---|---|---|---|
  | 1 | 106 ms | 549 ms | 51 MiB |
  | 2 | 59 ms | 280 ms | 53 MiB |
  | 4 | 36 ms | 148 ms | 56 MiB |
  | 8 | 26 ms | 92 ms | 57 MiB |

  For 10^6 symbols: 4 ms plan and 12 ms write on 8 threads. The peak RSS
  is the bench's inputs plus the windows in flight; the plan itself is
  a 40-byte segment row per file block, 24 bytes per checkpoint (10^7
  entries: 230 KB) and 24 bytes per demoted global.

### 7.4 .shstrtab

- **S-90 [C].** `.shstrtab` is a zero byte followed by each output
  section's name in section index order, each once, including
  `.shstrtab` itself. Section names are de-duplicated (two output
  sections never have the same name, except notes of one name, which
  layout L-45 makes one section). Its size is known after layout stage C;
  it is the last section and changes no address.

## 8. Notes

The gABI "Note Section" format: `namesz`, `descsz`, `type`, the name
padded to the note's alignment, the descriptor padded likewise. The
descriptor starts at `align_up(12 + namesz, a)` from the note's start
(`elf.md` §3.8, as `mortar_elf` implements).

### 8.1 .note.gnu.build-id

- **S-91 [C] (size).** One note, name `GNU`, type `NT_GNU_BUILD_ID` (3),
  in an `SHT_NOTE` allocated section with alignment 4. `n` is the
  descriptor size of the style (`spec/image.md` I-69):

  | style | `n` | milestone |
  |---|---|---|
  | absent, `none`, `--no-build-id` | no note | M1 |
  | bare `--build-id` | 20 | M1 |
  | `sha1` | 20 (a different function from bare's) | M1 |
  | `sha256` | 32 | M1 |
  | `fast` | 8 | M1 |
  | `0xHEX` | the number of hex bytes | M1 |
  | `md5` | refused (`option value`) | — |
  | `uuid` | 16 | M2 (refused in M1, `option value`) |

  The row is `partial` (`spec/cli.md` §4.2): `=md5` and `=uuid` are
  refused at the command line, `unsupported option value:
  --build-id=md5`, with a note naming the styles mortar writes (WP-027d;
  `BuildIdPlan::new` keeps the same refusals for a plan that reaches it
  another way).

  `0xHEX` (I-70): `0x` is case-sensitive, then one or more pairs of
  hexadecimal digits of either case; an odd number of digits, none, or a
  non-hex digit is `invalid-option-value` with the note `expected an
  even number of hexadecimal digits`. The section's size is `16 +
  align_up(n, 4)`, zero-padded (`spec/image.md` I-71); the references
  write `16 + n` (a 3-byte `0xabcdef` gives `0x13`, X-21), which differs
  only for `0xHEX` of a length not a multiple of 4. synth.md needs only
  `n` before addresses.
- **S-92 [C] (content).** The descriptor is written in the sealing step,
  after every other byte (`spec/image.md` owns the styles, the hash
  functions and their order of input bytes); `0xHEX` is copied. Bare
  `--build-id` and `sha1` are different functions (image I-69: the fast
  tree and the SHA-1 tree). `fast` is an 8-byte non-cryptographic hash
  (lld's size): the first 8 bytes of the bare id (`spec/image.md`
  I-72). `uuid` (M2) sets the RFC 4122 version 4 and variant bits, as
  mold does.
- **S-93 [G] (position).** The note is in the first `PT_LOAD` (layout
  rank 2), so that a core dump, which contains the first page of each
  mapping, contains it; `PT_NOTE` covers it (layout §11).

### 8.2 .note.gnu.property

- **S-94 [G] (inputs).** An input `.note.gnu.property` holds one or more
  notes, each with name `GNU`, type `NT_GNU_PROPERTY_TYPE_0` (5), and a
  descriptor that is a sequence of `(pr_type u32, pr_datasz u32, data)`
  padded to 8 on ELF64 and 4 on ELF32 (gABI "Program Property", the Linux
  extensions). Every note of the section is read: libhwasan's
  `hwasan_interceptors_vfork.o` on AArch64 has two in one section (X-33).
  A property whose `pr_datasz` runs past the descriptor, or whose datasz
  is not 4 for the 32-bit-mask types below, is `malformed-input`
  (`program property is too short`, lld's wording; GNU warns `corrupt
  GNU_PROPERTY_TYPE`, mold crashes, X-19). Properties in a note are in
  ascending `pr_type` order; a note out of order is accepted and sorted.
- **S-95 [G] (merge rules).** The output property of each type, over all
  live input objects (an object without a property note has every
  property absent), by `arch.property_merge(pr_type)` in the processor
  range and by this list for the generic types (evidence: X-19, X-20,
  X-33):
  - **`And`**: the AND of every input's value, an absent input counting
    0; omitted if 0. AArch64 `FEATURE_1_AND` (0xc0000000: BTI 1, PAC 2,
    GCS 4), x86 `FEATURE_1_AND` (0xc0000002: IBT 1, SHSTK 2) and the rest of 0xc0000002..
    0xc0007fff (`UINT32_AND`), and the
    generic range 0xb0000000..0xb0007fff (`UINT32_AND`). GNU and lld
    agree, GCS included (GNU 2.44, lld 22); lld drops the generic range;
    mold writes no AArch64 property;
  - **`Or`**: the OR of the inputs that have it, present if any has it.
    x86 `ISA_1_NEEDED` (0xc0008002) and the other 0xc0008000..0xc000ffff types
    (binutils `GNU_PROPERTY_X86_UINT32_OR_HI`; GNU ld and mold OR
    0xc000f000, `spec/x64.md` E4), x86 `COMPAT_ISA_1_NEEDED`
    (0xc0000001, no `unknown-property`: GNU ld's merge; lld and mold drop
    it; `spec/x64.md` X-41), and the generic range 0xb0008000..0xb000ffff (`UINT32_OR`,
    whose first type is `GNU_PROPERTY_1_NEEDED`, emitted by
    `-mno-direct-extern-access`). GNU and mold agree; lld drops them;
  - **`OrAll`** (new, §14): the OR of the inputs, present only if every
    input has it. x86 `ISA_1_USED` (0xc0010002), `FEATURE_2_USED`
    (0xc0010001) and the other 0xc0010000..0xc0017fff types, and x86
    `COMPAT_ISA_1_USED` (0xc0000000, no `unknown-property`: GNU ld's
    merge; lld and mold drop it; `spec/x64.md` X-41) (GNU, mold;
    arch.md §3.13 calls `FEATURE_2_USED` `Or`, which X-20 refutes);
  - **`Max`**: `GNU_PROPERTY_STACK_SIZE` (1), the largest value over the
    inputs that have it; absent inputs do not count (GNU; lld and mold
    drop it);
  - **`Present`**: `GNU_PROPERTY_NO_COPY_ON_PROTECTED` (2), no data,
    present if any input has it (GNU; lld and mold drop it) [M2: it
    changes how a dynamic link treats protected symbols];
  - **`Option`**: `GNU_PROPERTY_MEMORY_SEAL` (3), no data, is a request
    of the output, not a fact about inputs: input instances are dropped
    without a diagnostic, and the output has it exactly when `-z
    memory-seal` is given (R-C3). GNU 2.44 has the option and treats
    input instances erratically; lld 22 does not know it;
  - **`EqualOrAbsent`**: AArch64 `FEATURE_PAUTH` (0xc0000001, 16 bytes,
    the platform and version of the pointer authentication ABI): copied
    when every input that has it has the same bytes (inputs without it do
    not count); refused when two disagree, `feature`, `inputs with
    different pointer authentication ABIs`. This is lld 22's behavior;
    GNU 2.44 does not know the type;
  - **unknown**: any other type (the rest of the processor range, the
    user range 0xe0000000 and up, generic types 4 to 0xafffffff) is
    dropped, with one warning per input and type, `unknown-property`:
    `{path}: unsupported GNU_PROPERTY_TYPE {pr_type:#x}; property dropped`
    (R-D2). It is never refused and never silent: its merge rule is
    unknown, so no output value could be claimed true. GNU warns
    `unsupported GNU_PROPERTY_TYPE` and then, depending on the order and
    number of inputs, copies one input's note unmerged or fails with
    `failed to create GNU property section`; lld and mold drop it
    silently.

  The scan has only `0xc0000000` (AArch64, values 3 and 7), and
  `0xc0000002` (value 3) and `0xc0008002` (value 1) on x86-64 (X-1).
- **S-126 [C] (several notes in one object).** An object's notes are
  combined first, each property by its own rule class of S-95 applied to
  the notes as if each were an object (an `And` property survives only
  if every note has it; `Or` is the OR of the notes that have it; `Max`
  the largest); the result then takes part in S-95 across objects
  (supervisor's ruling on Q12). A type repeated within one note is
  combined by the same rule (supervisor's ruling on WP-025). GNU 2.44 and lld 22 instead OR a repeated
  `FEATURE_1_AND` within an object and ignore a note that lacks it
  (X-33; Q12). mortar's rule is the conservative one: a BTI claim that
  some note did not make could fault at the first indirect branch. In
  libhwasan's object the notes are equal, so all rules agree there.
- **S-96 [G] (output).** One note in an allocated `SHT_NOTE` section
  `.note.gnu.property` with alignment 8 (ELF64) or 4 (ELF32), the
  surviving properties in ascending `pr_type`, each padded; no section
  when none survives. It is covered by a `PT_GNU_PROPERTY` segment and a
  `PT_NOTE` (layout §6). The input notes are consumed: they never reach
  the output as input bytes (plumb S12 excludes them).
- **S-97 [C] (decided once).** The merged properties are computed in
  phase 80 and are an input to the PLT variant (arch.md §3.7: BTI PLT
  when BTI survives) and to layout's `PT_GNU_PROPERTY`. `-z force-bti`,
  `-z bti-report`, `-z cet-report`, `-z force-ibt`, `-z shstk` and
  `-z pac-plt` stay refused at M1 (`spec/cli.md`). AArch64 build
  attributes (`SHT_AARCH64_ATTRIBUTES`) are dropped by layout L-35 and
  never feed the merge.

### 8.3 Other notes

- **S-98 [C] (concatenation).** Every other note section name (layout
  L-45: `.note.ABI-tag`, `.note.package`, `.note.stapsdt`, any unknown
  name) is an output section of the inputs' bytes, each input section
  placed whole at its own alignment in `SectionId` order, unchanged. The
  linker reads none of them. `.note.GNU-stack` is consumed by layout
  L-32 and `.gnu.warning.*` by layout L-33; neither reaches the output.
- **S-99 [C] (mixed alignment).** An output note section whose live
  inputs differ in alignment is refused: `feature`, `notes of different
  alignment in one output section`. A reader walks it with one alignment,
  and a 4-aligned note whose name size is not a multiple of 8 misparses:
  all three put a 4- and an 8-aligned `.note.foo` in one section and
  readelf reports `<corrupt - note is too small>` (X-23). The scan never
  mixes alignments within one name (X-1).

## 9. .comment

- **S-100 [C].** `.comment` (`SHT_PROGBITS`, `SHF_MERGE|SHF_STRINGS`,
  entsize 1, alignment 1, non-allocated) is the inputs' merged strings,
  then synthetic chunk 23: `mortar <version>` and its NUL, where the
  version is the crate's `MAJOR.MINOR.PATCH` with no hash, date, host or
  path, so output stays deterministic. The chunk joins the inputs'
  `.comment` output section after its merged chunk (layout's join of a
  synthetic chunk named like an output section), lld's place; with no
  input `.comment` it is the section. It exists in every output, as lld's
  (`Linker: LLD 22.1.8` after the inputs' strings) and mold's (`mold
  2.37.1 (compatible with GNU ld)` before them); GNU ld 2.44 adds
  nothing (X-9; measured on a static glibc link, WP-027e). `readelf -p
  .comment` then names the linker for bug reports and the differential
  harness. Test: `the_comment_names_mortar_after_the_inputs` in
  `crates/mortar/tests/pipeline.rs`. R-M6 is withdrawn: the internal
  file needs no merge piece.

## 10. Dynamic outputs (M2)

The M2 specification of the dynamic chunks is `spec/dyn.md` (§6-§8);
the rules below keep their ids and cite it (request R-S1).

- **S-101 [M2] (`.dynsym`).** `spec/dyn.md` D-21 (which symbols), D-22
  (order: null, the unhashed undefined entries in `SymbolId` order, the
  hashed entries by (bucket, `SymbolId`); `sh_info` 1; `Slots.dynsym`)
  and D-23 (entry fields).
- **S-102 [M2] (`.gnu.hash`).** `spec/dyn.md` D-20 (`.gnu.hash` only, no
  `.hash`; `--hash-style` other than `gnu` refused) and D-26 (the
  table, a closed form of the hashed count).
- **S-103 [M2] (versions).** `spec/dyn.md` D-27 (`.gnu.version`,
  `.gnu.version_r` in `Slots.needed` order; no `.gnu.version_d` in an
  executable) and D-24 (`.dynstr`).
- **S-104 [M2] (`.rela.dyn`).** `spec/dyn.md` D-40 (the set, a function
  of `Demands`, `Slots` and the target kinds; `RELATIVE` first by
  `r_offset`, counted by `DT_RELACOUNT`/`DT_RELCOUNT`; then the others by
  (`.dynsym` index, `r_offset`); the parallel fill of S-4) and D-41
  (`IRELATIVE` last in `.rela.plt`, S-69). `.relr.dyn` is M4 (D-27,
  D-62).
- **S-105 [M2] (`.dynamic`).** `spec/dyn.md` D-30 (the tags, their order
  and presence, six `DT_NULL`s); its size is known before addresses.
- **S-106 [M2] (`.interp`).** The `-dynamic-linker` path, NUL-terminated
  (`spec/dyn.md` D-31: a dynamic output without one is refused; "or the
  target's default" is withdrawn, request R-S2). It exists only when an
  interpreter is named (never in a `StaticPie`); first in the first
  `PT_LOAD` after the headers (layout L-60 rank 1).
- **S-107 [C] (what M1 must not preclude).** `.got` has a place for the
  `_DYNAMIC` header word (S-65); IPLT words follow PLT words (S-67) and
  `IRELATIVE` follows `JUMP_SLOT` (S-69); `.symtab` never takes its
  order from `.dynsym`; every S-3 chunk except `.relr.dyn` is sized
  before addresses, `.gnu.hash` and `.rela.dyn` included, and M2 keeps
  that. The static shapes are **not** the dynamic ones with no PLT
  entries on every target: a dynamic output adds PLT0 and the
  `.got.plt` header (three words on AArch64 too, a64 §10.2), and the
  lazy `JUMP_SLOT` words and `-z now` placement are `spec/dyn.md` D-49's
  (request R-S6).
- **S-128 [M4; planned: WP-118 (`.dynamic`, `DT_SONAME`, `DT_SYMBOLIC`),
  WP-125 (`.gnu.version_d`, `DT_VERDEF*`)] (the synthetic chunks of a
  `Shared` output).** Request R-S10 of `spec/dyn.md` §18.2. In a
  `Shared` output: row 8 (`.gnu.version_d`) exists and is written as
  `spec/dyn.md` D-115 says (entries, order, `.gnu.version` indices,
  section header); row 18 (`.dynamic`) holds D-30's tags with D-102's
  changes for `Shared`: `DT_SONAME` iff `-soname`, then `DT_SYMBOLIC`
  iff the line's last `-Bsymbolic` form is `-Bsymbolic` and it has no
  `--dynamic-list` (D-112), `DT_VERDEF`/`DT_VERDEFNUM` iff row 8 exists
  (before `DT_VERNEED`), `DT_VERSYM` also when only row 8 exists, no
  `DT_DEBUG`, `DT_FLAGS`/`DT_FLAGS_1` by D-103; row 1 (`.interp`) never
  exists (`spec/cli.md` §3.2: `-dynamic-linker` with `-shared` is
  accepted and writes nothing); rows 24-25 never exist (no copies,
  D-122). Every one of these is sized before addresses (S-107 stands).
  *Tests:* `dynamic-tags` (its `Shared` rows), `versions-verdef`.

## 11. Sizes as data, units and incremental linking

### 11.1 Sizes

- **S-108 [C].** `Slots.sizes` lists, for every synthetic chunk of S-3
  except `.relr.dyn` and the address-dependent cases of S-2, its size:

  | chunk | size |
  |---|---|
  | `.note.gnu.property` | `16 + Σ(8 + align_up(datasz, a))` over surviving properties, a = 8 (4) |
  | `.note.gnu.build-id` | `16 + align_up(n, 4)` (S-91) |
  | `.rela.plt` | `(PLT + IPLT entries) × entsize` |
  | `.eh_frame_hdr` | `12 + 8 × entries` (S-59) |
  | `.eh_frame` | Σ output sizes of the live records (S-47) + 4 |
  | `.plt` | `PltShape` header + entries + `iplt_entry × IPLT` |
  | `.got` | Σ region capacities × slot size (S-63) |
  | `.got.plt` | `(reserved + PLT + IPLT) × word` |
  | `.symtab` | `entsize × (1 + locals + globals)` (S-88) |
  | `.symtab_shndx` | `4 × .symtab entries` |
  | `.strtab` | `1 + fixed table + Σ name bytes + 1 per entry` |
  | `.shstrtab` | `1 + Σ (name + 1)` over output sections |
  | `.interp` [M2] | `len(path) + 1` (D-31) |
  | `.gnu.hash` [M2] | `16 + (W/8) × maskwords + 4 × nbuckets + 4 × n` for `n` hashed entries (D-26; `W` the word size in bits) |
  | `.dynsym` [M2] | `entsize × (1 + entries)` (D-21; entsize 24, ELF32 16) |
  | `.dynstr` [M2] | `1 + Σ (string + 1)` over the distinct strings of D-24 (no tail merging) |
  | `.gnu.version` [M2] | `2 × .dynsym entries` (D-27) |
  | `.gnu.version_r` [M2] | `16 × (Verneed + Vernaux entries)` (D-27) |
  | `.rela.dyn` [M2] | `records × entsize` (D-40; entsize 24, `.rel.dyn` 8) |
  | `.dynamic` [M2] | `(tags present + 6) × (16, ELF32 8)`: D-30's rows under their conditions, plus the six `DT_NULL`s (row 32) |
  | `.gdb_index` [M2] | `24 + 16 C + 20 R + 8 T + Σ (4 + 4 × vector) + Σ (name + 1)` (S-G7a) |
  | `.dynbss`, `.bss.rel.ro` [M2] | the end of the last allocation, each allocation placed at `align_up(end, its alignment)` in D-42's order (a function of the shared definitions' `st_size` and alignment) |

  Each is a function of counts. `Merged` chunk sizes come from
  `MergeMap` (S-24). None needs an address.
- **S-109 [C] (no cost by default).** A full link computes these sizes
  and no capacity (every capacity equals its size, model §4.4); nothing
  here costs a full link memory or time for incremental links' sake.

### 11.2 What an incremental link redoes [M7]

The rule for all of them is model §7.2: an incremental link recomputes
the decision tables that changed, keeps the rest, and refills whole units
through the same `fill`. The table says, per synthetic chunk, what one
edit changes.

- **S-110 [M7] (a string added or removed).** A new fragment is
  appended to its `Merged` chunk within capacity (layout L-147) with the
  next `FragId`; its referrers are the edited file's units, refilled
  anyway. An unused fragment keeps its bytes. A full link given these
  tables gives the same bytes (model §7.3): `MergeMap` is stored in
  identity terms (output section and content hash, model §7.1). An
  unused piece is a dead range (`spec/incr.md` IN-3a): recorded in the
  decision tables, never the target of a live reference, and exempt
  from fill, because a live reference may share its suffix.
- **S-111 [M7] (an FDE added or removed).** The edited file's records
  are a contiguous range of `.eh_frame` (S-46), refilled. In
  incremental mode `.eh_frame` is grouped in per-file ranges in S-46's
  order, CIEs are deduplicated within a file only, and a range's unused
  tail is 0 bytes, absorbed `DW_CFA_nop` padding of the range's last
  record (4, 8 or 12 bytes), or one padding CIE of at least 16 bytes
  (`spec/incr.md` IN-34a, with its bytes). A range that outgrows its
  capacity makes the whole section be rewritten in full-link order. The
  terminator is always the section's last word. `.eh_frame_hdr` is one
  unit, refilled whenever an FDE's address changes (a sort of 10^6
  entries, about 10 ms). Per-file capacity is `spec/incr.md`'s policy;
  the shape allows it because `Slots.eh_records` is a table and a hole
  can be a linker-made CIE that no FDE uses.
- **S-112 [M7] (a symbol added or removed).** `.symtab`, `.strtab` and
  `.symtab_shndx` are rewritten whole: they are non-allocated and last
  (layout rank 10), so their size changes no address and no other unit.
  Rewriting 10^7 symbols is about 0.12 s on 8 threads (S-89, measured). A per-file capacity in
  `.symtab` (with null local entries as holes) is possible and is not
  done unless the bench shows the rewrite matters; Q8 records it.
- **S-113 [M7] (a GOT or IPLT slot added or removed).** A new slot is
  appended to its kind's region within its capacity; a removed slot stays,
  written 0. A new IPLT entry appends to `.plt`, `.got.plt` and
  `.rela.plt`. `.rela.plt` is refilled whole with no holes (a hole would
  be an `IRELATIVE` that glibc applies): a removed entry's relocation is
  removed and the chunk compacted, while its `.plt` and `.got.plt` words
  stay as dead slots. `__rela_iplt_end` moves with the count, which
  changes only the start-up code's unit (refilled through the dependency
  index).
- **S-114 [M7] (properties, build id, `.comment`).** `.note.gnu.property`
  is recomputed whole when any input's notes change; if its size
  changes it grows within capacity, and if the merged value changes the
  PLT variant may change, which refills every PLT entry. The build id is
  recomputed over the final image on every link. `.comment` is a merged
  chunk and follows S-110.
- **S-115 [C] (fixed now for M7).** Every order is by ids (`FragId`,
  `SymKey`, `SectionId`), never hash or address, so tables extend by
  appending; the GOT has per-kind regions with capacities (S-63);
  `Slots.eh_records` lists records explicitly; `.symtab` is last and
  non-allocated; the AArch64 GOT header word is a policy (S-65); x86
  `.got.plt` always has its reserved words (S-67).

## 12. What tools rely on

- **S-116 [T] (gdb, lldb).** Names from `.symtab` without DWARF, statics
  scoped by `STT_FILE` (S-83), IFUNC breakpoints through
  `STT_GNU_IFUNC` (S-71); unwinding through `.eh_frame` (the table when
  present, not needed); `.debug_str` through `DW_FORM_strp`, which S-17
  keeps exact; thunks named by S-80.
- **S-117 [T] (perf).** `.symtab` with function sizes; the build id to
  find debug files; `.eh_frame_hdr` and `.eh_frame` for
  `--call-graph=dwarf` (drivers pass `--eh-frame-hdr` for dynamic links).
- **S-118 [T] (objdump, nm, addr2line).** AArch64 mapping symbols to tell
  code from data (S-78, S-80); `.symtab`; DWARF, falling back to
  `.symtab`.
- **S-119 [T] (valgrind).** `.symtab`, well-formed CIEs and FDEs (S-34 to
  S-39), the build id; no `.eh_frame_hdr`.
- **S-120 [T] (strip, objcopy, eu-elflint).** `.symtab` per S-73 (locals
  first, `sh_info` right).

## 13. Where the references differ

| point | GNU | lld | mold | mortar | why |
|---|---|---|---|---|---|
| merge order | first occurrence (with tail merge) | shard, then hash | hash | first occurrence, no tail merge (S-24) | stable under edits; GNU-like |
| tail merging | always | `-O2` | never | never (S-23) | measured gain small; cost and incrementality |
| `-O0` | merges | does not merge | merges | merges (S-32) | nothing depends |
| section symbol + A at `sh_size` | end of output section, silently | error | end of last piece | refused, the message naming the fix (S-18) | a known choice: GNU's value is some other object's string; no compiler emits it (X-28) |
| section symbol + A outside | warning, end of output section | error | silent garbage | refused (S-18) | |
| unterminated string section | copied | error | error | refused (S-8) | |
| `.eh_frame` order | input, CIE at first use | CIE then its FDEs | CIEs, then FDEs | input (S-46) | GNU; keeps crtbeginT's walk |
| input terminators | kept in place | dropped, one at end | dropped, one at end | dropped, one at end (S-48) | crtend.o position independent |
| no `crtend.o` | no terminator | terminator | terminator | terminator (S-48) | |
| reference to offset 0 of an input `.eh_frame` | the record's place, or the next kept record's (crtbeginT: misses earlier objects' FDEs) | the record's place; the start when that CIE was de-duplicated or the section is empty | the start plus the offset | the start of `.eh_frame` (S-50 rule 1) | the registry walk sees every FDE (X-25) |
| `__EH_FRAME_BEGIN__` in `.symtab` | 16 to 40 bytes before `.eh_frame` | `.eh_frame` start | dropped | `.eh_frame` start (S-50) | one rule for symbols and relocations |
| record length not a multiple of 4 | trailing nops removed, padded to 4 | copied | copied | padded to 4 (S-47) | records stay aligned |
| `R_*_NONE` in `.eh_frame` | ignored | ignored | ignored | ignored; a field with only `NONE` has no relocation (S-38) | `ld -r` outputs (X-30) |
| CIE version 2, 4; unknown augmentation | warning, no hdr | error | accepted | refused (S-36) | |
| truncated record, bad CIE pointer, data after terminator | warning, no hdr | error | crash or accepted | refused (S-39) | |
| FDE without pc relocation | kept, no hdr | dropped | dropped | dropped if zero, else refused (S-38) | verifier E2 |
| FDEs with the same start, with a table | link fails | one entry | all entries | refused (S-59) | verifier E3, E5 |
| dead FDEs | removed | removed | removed | absent, never neutralised (S-41) | verifier E4 |
| zero-length FDE | FDE dropped | FDE kept, no entry | FDE kept, entry (plumb A19) | FDE kept, no entry (S-59) | it would shadow |
| overlapping FDEs | link fails | accepted | accepted | accepted (S-60) | |
| `.eh_frame_hdr` without the option | none | none | always | none (S-57) | |
| static IPLT sections | `.plt`, `.rela.plt`, `.got.plt` | `.iplt`, `.rela.dyn`, `.got.plt` | `.plt.got`, `.rela.dyn` | GNU's (S-68) | |
| an IFUNC in `.symtab` | `IFUNC` at the resolver, always | `FUNC` at `.iplt`, size 0, when canonical; else `IFUNC` | `FUNC` locals at `.plt.got` (A1, A2) | `IFUNC` at the resolver, always (S-71) | gdb's `break f` works (X-27) |
| `__rela_iplt_*` under `-static-pie` | not defined | not defined | absolute 0 | not defined (S-70) | resolve.md §8.1 |
| IRELATIVE word (RELA) | AArch64: the start of `.plt` (WP-041); x86-64: the entry's address + 6 (`spec/x64.md` E3) | 0 | — | AArch64 `S+A`, the resolver; x86-64 0 (S-69; `spec/dyn.md` Q7) | glibc overwrites it |
| x86 static `.got.plt` reserved words | 3 | 3 | — | 3 (S-67) | |
| section symbols in `.symtab` | AArch64 yes, x86 no | none | all | none (S-79) | |
| `.L` locals | dropped | dropped | kept | dropped (S-78) | |
| `STT_FILE` for an input without one | one named after the member | none | none | none (S-83) | cosmetic in gdb (X-32) |
| hidden definitions | `STB_GLOBAL STV_HIDDEN` | local, in its file's block | local | local, in its file's block (S-77) | resolve.md §3.7 |
| globals in a static executable | global | global | local (A9) | global (S-75, S-121) | verifier Y3 |
| `STO_AARCH64_VARIANT_PCS` of a global | OR of every entry's | the definition's; an undefined entry the first mention's | as lld | the definition's; an undefined entry the OR of its references' (S-127) | the code's convention; input-order free |
| label of an empty input section (`__TMC_END__`) | index of a section below it (A10) | — | — | index of the section holding it (S-122, S-123) | verifier Y3 |
| `__bss_start` with no `.bss` | `edata` | `edata` | 0 under gc (A11) | `edata` (S-124) | verifier Y5 |
| `.strtab` de-duplication | full, tail merged | almost none | none | fixed table only (S-86) | |
| `.comment` | inputs only | adds `Linker: LLD <version>` after them | adds `mold <version> (compatible with GNU ld)` before them | adds `mortar <version>` after them (S-100) | |
| AArch64 `.note.gnu.property` | AND | AND | none written | AND (S-95) | BTI/PAC enforcement |
| x86 `ISA_1_NEEDED` | OR | dropped | OR | OR (S-95) | |
| x86 `*_USED` | OR if all have it | — | OR if all have it | OR if all have it (S-95) | |
| x86 `COMPAT_ISA_1_USED` / `_NEEDED` (0xc0000000, 0xc0000001) | OR if all have it / OR | dropped | dropped | GNU's (S-95) | `spec/x64.md` X-41 |
| several property notes in one object | OR within the object | OR within the object | — | as separate objects: AND for `And` types (S-126) | a BTI claim no note made could fault |
| generic AND/OR ranges (0xb0000000..) | AND / OR | dropped | dropped | AND / OR (S-95) | GNU |
| `STACK_SIZE`, `NO_COPY_ON_PROTECTED` | max / present if any | dropped | dropped | max / present if any (S-95) | GNU |
| `MEMORY_SEAL` | erratic; `-z memory-seal` | `-z` unknown | — | from `-z memory-seal` only (S-95) | |
| AArch64 PAUTH (0xc0000001) | warning, unsupported | copied if equal, error if not | dropped | copied if equal, refused if not (S-95) | lld |
| unknown property type | warning; copied unmerged or link fails | dropped | dropped | dropped, warning `unknown-property` (S-95) | never silent, never a false claim |
| notes of mixed alignment in one section | corrupt output | corrupt output | corrupt output | refused (S-99) | |

## 14. Requested changes to other specs

Each request gives the section, the current text, the proposed text, and
the reason. They are written to be applied mechanically.

### 14.1 model.md

**R-M1. §3.5, the identity of a fragment.**
*Applied by WP-027a.*
Current: "Two pieces have the same `FragId` exactly when they belong to
one output section and have equal bytes and alignment."
Proposed: "Two pieces have the same `FragId` exactly when they belong to
one output section and have the same merge class (`SHF_STRINGS` and
`sh_entsize`), equal alignment and equal bytes (`spec/synth.md` S-13)."
Reason: a constant and a string with equal bytes are cut and referred
to differently (S-9, S-10), so they must not share a fragment; two
alignments of one string stay apart in all three references (X-7).

**R-M2. §4.2, the reference to `spec/merge.md`.**
*Applied by WP-027a.*
Current: "The default order is by `FragId`, or by a content key
`spec/merge.md` gives (tail merging); never by a hash table's layout
(§6.5)."
Proposed: "The order is by merge class, then alignment, then `FragId`
(`spec/synth.md` S-24); never by a hash table's layout (§6.5). Fragments
never overlap: there is no tail merging (`spec/synth.md` S-23)."
Reason: there is no `merge.md`; synth.md owns merging, and does not
tail-merge.

**R-M3. §4.3, the `.symtab` order.**
*Applied by WP-027a.*
Current: "`.symtab` (and the symbol tables of `-r` output) is ordered by
rule, not by rows: locals per file in `FileId` and table order, then
globals in `SymbolId` order, so that the output index of `(FileId,
st_index)` is a prefix sum per file. Only the per-file `(first local,
count)` ranges are stored."
Proposed: "`.symtab` (and the symbol tables of `-r` output) is ordered by
rule, not by rows (`spec/synth.md` S-74, S-75): the internal file's
locals, then per file in `FileId` order its kept locals in table order
followed by its demoted globals (output binding `STB_LOCAL`) in
`SymbolId` order, then the kept globals in `SymbolId` order, so that the
output index of `(FileId, st_index)` is a prefix sum per file. Only the
per-file `(first local, count)` ranges are stored; `count` includes the
demoted globals."
Reason: hidden definitions become locals (resolve.md §3.7) and must be
in the local part (gABI); lld puts them in their file's block (X-17).

**R-M4. §4.3, GOT capacities.**
*Applied by WP-027a.*
Current: "Where the kinds lie relative to each other in the GOT, and
which `.got.plt` word belongs to a PLT or IPLT entry, follow from the
indices by `spec/synth.md`'s rule; they are not stored."
Proposed: "Where the kinds lie relative to each other in the GOT, and
which `.got.plt` word belongs to a PLT or IPLT entry, follow from the
indices and from each kind's capacity (`spec/synth.md` S-63) by that
spec's rule. A capacity equals the kind's count except in an incremental
link [M7], which stores it."
Reason: an incremental link appends a slot to one kind without moving
the slots of the next (S-113).

**R-M5. §4.4, `ChunkId` order.**
*Applied by WP-027a.*
Current: "`ChunkId`s are given in a fixed creation order: headers, then
synthetic chunks in the order `spec/synth.md` lists them, then merged
chunks and output-section bodies in `OutSecId` order."
Proposed: append "The list is `spec/synth.md` S-3; `.eh_frame` is one of
its synthetic chunks (its input sections are split and have no `place`)."
Reason: `.eh_frame` is synthesized from `Slots.eh_records`, not a body.

**R-M6. §3.5, the internal file's pieces.**
*Withdrawn (WP-027e): `.comment`'s piece is synthetic chunk 23 (S-3, S-100), not a merge piece.*
Current: (no text)
Proposed: add after the `EhFrame` bullet: "- the internal file (`FileId`
0) may contribute pieces: the `.comment` string of `spec/synth.md`
S-100, and at M2 linker-generated `.eh_frame` records (S-55)."
Reason: S-100 puts mortar's identification through the ordinary merge
path rather than a new chunk kind.

**R-M7. §3.5, the numbering method of `FragId`.**
*Applied by WP-027a.*
Current: "`FragId`s are numbered by first occurrence
`(FileId, sh_index, piece)`, by the method of §2.3;"
Proposed: "`FragId`s are numbered by first occurrence
`(FileId, sh_index, piece)`: the numbering §2.3 defines, computed by the
partitioned method of `spec/synth.md` S-28 (occurrences partitioned by
shard in key order, each shard interned on one thread; the first insert
is the first occurrence, so no atomic minimum is needed);"
Reason: S-28, S-30: the locked table with an atomic minimum costs 2.6 to
3.3 s on 10^7 pieces, the partitioned method 0.3 to 0.5 s at 4 to 8
threads, with the same `FragId`s (X-29).

### 14.2 layout.md

**R-L1. L-58.**
*Applied by WP-027a.*
Current: "The terminator that `crtend.o` provides (a zero-length CIE)
must stay last, and it does, because `crtend.o` comes after the user's
objects in every driver's line."
Proposed: "Input terminators (zero-length records, which `crtend.o`
provides) are dropped, and `spec/synth.md` writes one terminator at the
end of `.eh_frame` (S-48), wherever `crtend.o` is on the line."
Reason: X-13: with `crtend.o` early GNU's output crashes; lld and mold
write the terminator last and the program runs.

**R-L2. §20.6, replace the request list with its resolution.**
*Applied by WP-027a.*
Current: the four bullets under "### 20.6 `synth.md` (not yet written)".
Proposed: "### 20.6 `synth.md` — applied: `.relro_padding` [now `.pad.relro`, layout L-156] is a layout
chunk (synth.md does not list it, S-3); `.eh_frame` keeps input order
(S-46) and its type follows L-49 (S-54); the static IPLT is in `.plt`,
`.rela.plt` and `.got.plt` (S-68); `.note.gnu.property` is merged by
S-95, which keeps the AND rule and adds `Or` and `OrAll` for x86."
Reason: the requests are answered.

**R-L3. §8.1 (sections that go nowhere), `-S`.**
*Applied by WP-027a as L-158 (with the fix that also strips the mergeable `.debug_str`/`.debug_line_str`).*
Current: (no rule for `-S`/`--strip-debug`)
Proposed: add "- **L-n [C]** With `-S` (`--strip-debug`) or `-s`, the
`.debug_*` sections and `.gdb_index` go nowhere.
`.comment` and non-allocated notes are kept. (`spec/synth.md` S-84.)"
Reason: S-84 needs the rule to exist in the spec that decides which
sections are output.

**R-L4. §12.1 L-85, the merged chunk's order.**
*Applied by WP-027a.*
Current: "ordered by class (strings before constants, then entsize
ascending), then by `MergeMap` order."
Proposed: "ordered by class (strings before constants, then entsize
ascending), then by alignment ascending, then by `MergeMap` order
(`spec/synth.md` S-24, S-25)."
Reason: S-25.

**R-L5. §14.3, symbol-table sizes after the fixed point.**
*Applied by WP-027a (L-121).*
Current: L-121 lists `.relr.dyn`, patch islands and compressed non-alloc
chunks as sized chunks.
Proposed: add to L-121: "`.symtab`, `.strtab` and `.symtab_shndx` are
sized after the fixed point when thunks or patches exist, since each adds
symbols (`spec/synth.md` S-80); they are non-allocated and after every
allocated chunk, so their size moves no address."
Reason: S-2, S-80.

**R-L6. §9, empty input sections.**
*Applied by WP-027c as `spec/layout.md` L-159.*
Current: (no rule)
Proposed: add to §9 "- **L-n [C] (empty input sections)** A live input
section of size 0 is placed like any other, at its output section's
position in stage B order, aligned to its `sh_addralign`, and its output
section's size covers its place. When its output section has no other
content (so no chunk), it is placed at the end of the last chunk before
it in `order` within the same rank and segment class, or, when there is
none, at the start of the first chunk after it, with no alignment
applied, so that the place is at most that chunk's end; its labels take
that chunk's output section index. Every empty section that maps to one
absent output section gets that one address. (`spec/synth.md` S-122,
S-123.)"
Reason: `__TMC_LIST__`/`__TMC_END__` label empty `.tm_clone_table`
sections in every C and C++ link, and GNU's index for `__TMC_END__` fails
Y3 (A10). Aligning in the absent case could pass the chunk's end; not
aligning in the present case would put `__TMC_LIST__` before an aligned
table (X-31).

### 14.3 arch.md

**R-A1. §3.13, the merge rules of `property_merge`.**

*Applied by WP-023b.*

Current: "`property_merge(pr_type)` is `And` (an input without the
property counts as 0; AArch64 `FEATURE_1_AND`, x86 `FEATURE_1_AND`),
`Or` (an input without it counts as 0; x86 `ISA_1_NEEDED`,
`FEATURE_2_USED`) or `None` for a type the architecture does not know,
which is refused by name."
Proposed: "`property_merge(pr_type)` gives the rule for a processor-range
type (0xc0000000..0xdfffffff): `And` (an input without the property
counts as 0; the property is omitted when the result is 0; AArch64
`FEATURE_1_AND` with BTI, PAC and GCS, x86 `FEATURE_1_AND` and the rest
of `0xc0000002..0xc0007fff`), `Or` (the OR
of the inputs that have it, present if any has it; x86 `ISA_1_NEEDED`
and the other `0xc0008000..0xc000ffff` types, and `COMPAT_ISA_1_NEEDED`
0xc0000001), `OrAll` (the OR of the
inputs, present only if every input has it; x86 `ISA_1_USED`,
`FEATURE_2_USED` and the other `0xc0010000..0xc0017fff` types, and
`COMPAT_ISA_1_USED` 0xc0000000),
`EqualOrAbsent` (AArch64 `FEATURE_PAUTH`: copied when every input that
has it agrees, refused when two disagree) or `None` for a type the
architecture does not know, which `spec/synth.md` S-95 drops with the
warning `unknown-property`. The generic types and ranges (below
0xc0000000) are S-95's, not the architecture's."
Reason: X-20 (`*_USED` dropped when one input lacks it; `ISA_1_NEEDED`
kept), X-33 (GCS ANDs like BTI; PAUTH in lld 22; GNU drops or copies
unknown types with a warning and never refuses them).

**R-A2. §3.1, `Facts.got_plt_reserved`.**

*Applied by WP-023b.*

Current: "`got_plt_reserved: u8, // words reserved at the start of
.got.plt with .dynamic`"
Proposed: "`got_plt_reserved: u8, // words reserved at the start of
.got.plt: with .dynamic on every target; also in static outputs on x86
(synth.md S-67)`"
Reason: X-22; agrees with layout.md §20.5's request (x86 crt files
always demand `GOT_BASE`, L-101) and also covers a link without them.

### 14.4 a64.md

**R-A64-1. §13.1, the personality encoding.**

*Applied by WP-023b.*

Current: "personality `0x9c` (`indirect | pcrel | sdata4`) via a
`DW.ref.*` data word relocated by `ABS64`"
Proposed: "personality `0x9b` (`indirect | pcrel | sdata4`), the CIE's
pointer relocated by `PREL32` to a `DW.ref.*` data word that `ABS64`
relocates (GCC; every CIE with a personality in the libraries of all
three targets, `spec/landed/synth/WP-025-spec.md` X-1); `0x9c` is
`indirect | pcrel | sdata8`, with `PREL64`, where a producer uses it"
Reason: `0x9c` is `sdata8`, not `sdata4`; the scan has only `0x9b`.

**R-A64-2. Q9.** Mark as answered: "`.plt`/`.rela.plt`/`.got.plt`
(`spec/synth.md` S-68)."

*Applied by WP-023b.*

### 14.5 plumb.md

These requests are against `plumb.md` as WP-028 (verifier stage 2) leaves
it: §6.4 (E1 to E5, call frame information) and §6.5 (Y2 to Y5, the
symbol table) already check `.eh_frame`, `.eh_frame_hdr` and `.symtab`,
and this spec is written to pass them (S-41, S-59, S-121 to S-124).

**R-P1. §6, two new structure rules (stage 3).** Add:

*Applied by WP-028b, with new ids: S16 is P1 and P2 (`plumb.md` §6.6; S16 and S17 were taken by `layout.md` §20.3's requests), the merge per linker by map format; S17 is F1 (§4.8), which also requires each slot to be an aligned, writable, unoccupied word (F2), not a word of `.got.plt` by name, because lld and mold put the slots in `.got.plt` and `.got`.*

- "**S16 (.note.gnu.property).** The output's properties equal the merge
  of the inputs' by `synth.md` S-95 and S-126 (every note of an input
  section read, notes of one object combined first; unknown types
  absent; `MEMORY_SEAL` present exactly under `-z memory-seal`), and
  `PT_GNU_PROPERTY` covers the note."
- "**S17 (IRELATIVE).** In a static output every `IRELATIVE` entry lies
  between `__rela_iplt_start` and `__rela_iplt_end` (or `__rel_iplt_*`),
  every entry between them is an `IRELATIVE`, and each entry's
  `r_offset` is a word of `.got.plt`."
Reason: these are [G] rules of this spec that E1 to E5 and Y2 to Y5 do
not cover (Y5 checks only the bounds of the `SHT_RELA` section).

**R-P4. §6.5 Y3, the inclusive end.**

*Applied by WP-028b (the sentence; the check was already inclusive).*

Current: "Its `st_shndx` names an
allocated section whose range holds D's input section's place."
Proposed: append "The range includes its end address: a label at the end
of a section, or of an empty input section placed at a section's end,
is held by that section (`synth.md` S-122, S-123)."
Reason: S-122; without the clause a label of an empty `.tm_clone_table`
placed at the end of `.data` has no section that holds it.

**R-P2. §5.6 step 1, out-of-range section references.**

*Applied by WP-028b: on a reference output `piece-not-locatable`, not an anomaly id, since no corpus link has one; in mortar's, R6.*

Current: "The input offset is r = `st_value + A` for a section symbol,
with extra = 0."
Proposed: append "A section-symbol reference with r outside `[0,
sh_size)` is an anomaly on a reference output (GNU writes the end of the
output section) and cannot occur in mortar's (synth.md S-18)."
Reason: S-18.

**R-P5. §6.5 Y5, `_GLOBAL_OFFSET_TABLE_` on x86 (before M3).**

*Applied by WP-028b.*

Current: "| `_GLOBAL_OFFSET_TABLE_` | the start of `.got` |"
Proposed: "| `_GLOBAL_OFFSET_TABLE_` | AArch64: the start of `.got`;
x86-64 and i386: the start of `.got.plt` |"
Reason: layout L-101 and `synth.md` S-67 put the x86 GOT base at
`.got.plt`, as GNU and lld do (X-22); Y5 as written would report every
x86 link of all three references.

**R-P6. §6.5 Y5, the i386 IRELATIVE bounds (before M4).**

*Applied by WP-028b (`-static-pie` is `ET_DYN`, which S1 refuses until stage 3).*

Current: "| `__rela_iplt_start`/`_end` | the bounds of the only allocated
`SHT_RELA` section; with none, start = end |"
Proposed: "| `__rela_iplt_start`/`_end` (AArch64, x86-64),
`__rel_iplt_start`/`_end` (i386) | the bounds of the only allocated
`SHT_RELA` (i386: `SHT_REL`) section; with none, start = end; not
defined under `-static-pie` |"
Reason: i386 is REL (`.rel.plt`, `R_386_IRELATIVE`) and glibc's i386
start-up references `__rel_iplt_*` (`synth.md` S-70); under
`-static-pie` the relocations are in `.rela.dyn` and the symbols are
weak undefined (resolve.md §8.1).

**R-P7. §6.4 "The inputs' pointers", `R_*_NONE` and an initial location
of 0.**

*Applied by WP-028b (`R_*_NONE` counts under `none`; i386 frames are still `machine` until x86 CFI is judged).*

Current: "If there is no relocation, an absolute nonzero value is taken
as written, and anything else cannot be judged."
Proposed: "A relocation of type `R_*_NONE` is no relocation. If there is
no relocation, an absolute nonzero value is taken as written; an initial
location of 0 makes the FDE **dropped** (it describes no code of the
link, `synth.md` S-38); anything else cannot be judged."
Reason: lld's i386 `ld -r` output has such an FDE (only `R_386_NONE`,
field 0; X-30); "unknown" would turn off E4 for the whole output, though
every linker drops it.

**R-P3. Mutations (§9.2).**

*Applied by WP-028b in part: the property bit not in every input's AND and the property of an unknown type (§9.2 (k)), and the stray `IRELATIVE` outside `__rela_iplt_*` (§9.2 (i) (5)). Not applied: the `.eh_frame` terminator moved to the middle and `__EH_FRAME_BEGIN__`'s relocation retargeted, both E1/E4 mutations of crtbegin/crtend links (follow-up, `spec/landed/plumb/WP-028b.md`).*

Add those WP-028 lacks: move the
`.eh_frame` terminator to the middle; set a property bit not in every
input's AND; keep a property of an unknown type; leave a stray
`IRELATIVE` outside `__rela_iplt_*`; point `__EH_FRAME_BEGIN__`'s
relocation at a record after the first.

### 14.6 diag.md

**R-D1. §3.1, no new error kinds.** The malformed `.eh_frame`, merge and
note cases use `malformed-input` with the messages of S-7, S-8, S-18,
S-36, S-39, S-50 and S-94; the refusals use `feature` (S-12, S-21, S-34,
S-37, S-99, and S-95's disagreeing PAUTH properties). Add to §3.2: "The
`malformed-input` messages of `spec/synth.md` are lld's wording where lld
has one, and name the fix where there is one: `string is not null
terminated`, `offset is outside the section; refer to it through a named
symbol`, `CIE/FDE ends past the end of the section`, `invalid CIE
reference`, `FDE version 1 or 3 expected, but got N`, `unknown .eh_frame
augmentation string: S`, `program property is too short`."
Reason: one wording per case, tested.

**R-D2. §3.1, a new warning kind.** Add the row: "| `unknown-property`
| `UnknownProperty { path, pr_type }` | `{path}: unsupported
GNU_PROPERTY_TYPE {pr_type:#x}; property dropped` | GNU ld's phrase |",
a warning. Reason: S-95 drops a property whose merge rule it does not
know, and must say so (GNU warns the same; lld and mold are silent).

### 14.7 image.md (WP-026)

**R-I1. The build-id styles.** *Done*: `spec/image.md` I-69 to I-72
(accepted), applied to S-91, S-92 and S-108 above (image R-12). The
original request: image.md defines, per `--build-id=` style
(`sha1`, `md5`, `sha256`, `uuid`, `fast`, `0xHEX`, `none`), the
descriptor size `n` and how the bytes are computed; synth.md S-91 sizes
the note as `16 + n` from it. X-21 measured the references' sizes
(GNU: sha1 20, md5 16, uuid 16, no `sha256` or `fast`; lld: sha1 20,
md5 16, uuid 16, fast 8, refuses sha256; mold: sha1 20, md5 16, uuid 16,
sha256 32, fast 32). Reason: Q4 is image.md's (coordinator ruling).

### 14.8 cli.md and docs/status/options.md

**R-C1.** *Applied by WP-027a* (`--strip-discarded` and `-O` are
`noop`, the others `supported`; `--build-id=STYLE`'s check is cli.md
§3's). Status changes when WP-025's code lands (M1): `-s`, `-S`,
`--strip-discarded`, `-x`, `-X`, `--discard-none` (S-78, S-84),
`--build-id[=STYLE]` with every style of S-91, `--eh-frame-hdr`,
`--no-eh-frame-hdr` (S-57) become "implemented"; `-O LEVEL` becomes
"accepted, no effect" (S-32), with `plan.shaping.optimize` not stored.
`--build-id=STYLE` with an unknown style is `invalid-option-value`
(GNU warns and ignores; lld refuses).
Reason: these are the options this spec defines.

**R-C3. `-z memory-seal`, `-z nomemory-seal`.** *Applied by WP-027a
(`plan.z.memory_seal`; GNU ld 2.44 gives the same 0x18-byte note for a
static C program).* Add both as implemented
at M1, the last one winning, default `nomemory-seal`, feeding S-95's
`MEMORY_SEAL` property (GNU ld 2.44's options; lld 22 does not know
them).

**R-C2. §3.8.** *Applied by WP-027a, except "default `-X`": the
default is S-78's own (corrected by WP-027a).* Current: "`strip` (`-s`, `-S`, `--strip-*`);" Proposed:
"`strip` (`-s`, `-S`, `--strip-*`; the last wins, `--strip-discarded`
is accepted and has no effect, `spec/synth.md` S-84);" and current
"`discard` (`-x`, `-X`, `--discard-*`);" proposed "`discard` (`-x`,
`-X`, `--discard-none`; the last wins, default `-X`, S-78);".
Reason: S-78, S-84.

## 15. Open questions

Ranked by how much depends on each.

| # | question | default taken | what depends |
|---|---|---|---|
| Q12 | *Ruled*: each property by its own rule class within an object, then S-95 across objects (S-126). Several property notes in one object: S-126 combines them as separate objects (AND for BTI), where GNU 2.44 and lld 22 OR them (X-33). | S-126 (AND) | BTI marking of objects with disagreeing notes |
| Q3 | Whether a relocation into the interior of a removed `.eh_frame` record (S-50 rule 4) ever occurs (crtstuff variants, other libcs). | refused | static C++ links with other crt files |
| Q2 | Piece-level liveness of merge pieces under `--gc-sections` (lld marks pieces; GNU keeps whole sections). | `spec/live.md` decides; S-16 keeps all pieces without gc | size of `.rodata` with gc; symbol drops of S-22 |
| Q11 | *Ruled*: the default accepted (no symbol). A local symbol naming each IPLT entry (say `f@iplt`), so that gdb's `info symbol` on a function pointer to an IFUNC names it (S-71's one cost). No reference writes one. | none | gdb on canonical IFUNC pointers |
| Q6 | PLT and thunk unwind info (FDEs for `.plt`, thunks) for perf. | none (lld) | perf's unwinding through PLTs [M2] |
| Q8 | Per-file capacity in `.symtab` for incremental links. | rewrite whole (S-112) | M7 time for 10^7-symbol programs |

Closed by the coordinator's rulings on the review: Q1 (no tail merging,
S-23); Q4 (the build-id styles are image.md's, R-I1); Q5 (no
`.eh_frame_hdr` unless `--eh-frame-hdr` is given: the GNU driver's
behavior is the contract, S-57); Q7 (`.comment` is `mortar <version>`,
S-100); Q9 (GCS is ANDed like BTI; PAUTH is copied when the inputs that
have it agree and refused when they do not, S-95); Q10 (the reviewer's
scan of all three targets' libraries and corpus has no two live FDEs
with one initial location, and no FDE initial-location relocation
against a global symbol, so S-59's refusal stands untriggered).

## 16. Tests

Existing link tests cite these sections: ".symtab" (§7.1,
`tests/link/resolve/symtab`), ".eh_frame_hdr" (§5,
`tests/link/layout/eh-frame-hdr`), ".note.gnu.build-id" (§8.1,
`tests/link/layout/build-id`), ".eh_frame" (§4,
`tests/link/lang/cxx-exceptions`), "IRELATIVE" (§6.2,
`tests/link/ifunc/irelative`). This draft adds no tests (WP-025 stage 2).
Each family runs against all references, with `skip` where §13 says they
differ. Required, per family:

- **merge** (`tests/link/merge/`): S-17 and S-19 (a string by section
  symbol, by local symbol with addends −1, +len and crossing; X-2, X-3);
  S-18 at `sh_size`, beyond and negative (the message); S-8; S-11
  (`.rodata.str1.8`); S-13 (`abc` in `str1.1`, `str1.8`, `cst4`); S-24
  (first-occurrence order, `readelf -p`); S-32 (`-O0` equals `-O2`); S-6;
  and the S-18 corpus case (X-28): `ec.c` built for i386 by GCC with GNU
  as and by Clang at `-O2 -fmerge-all-constants`, `-fno-pic` and
  `-fPIC`, asserting that no relocation is a merge section's symbol
  with `r ≥ sh_size` and that the program prints the C addresses.
- **eh_frame** (`tests/link/eh/`): the X-14 matrix, one test per row
  (accepted rows unwind through `_Unwind_Backtrace`, refused rows check
  the message); X-13 (crtend first); X-11 (two personality CIEs); the
  X-15 hdr cases; and:
  - `before-crtbegint` (X-25), per target: a static C++ `throw` through
    an object placed before `crtbeginT.o` is caught, and the
    `__register_frame_info` argument (read by gdb) is the start of
    `.eh_frame`. M1 is AArch64's non-empty `.eh_frame` whose CIE S-44
    removes; M3 and M4 are the empty x86 `.eh_frame` that every static
    x86 link has, so each target's ordinary static C++ link is required
    too (`skip(gnu)` for the placed-before case);
  - `removed-record-symbols`: `a64-cfi-undefined.s` assembled twice, so
    a `$d` lies inside a de-duplicated CIE: the link succeeds and the
    `$d` is absent; a global inside the removed CIE takes the next kept
    record's address; a relocation into its interior is refused;
  - `ld-r-inputs` (X-30): the `-r` outputs of GNU, lld and mold for
    i386, AArch64 and x86-64 of two C++ objects sharing an inline
    function and a template (`cxx.cc`, `cxx2.cc`), linked statically
    into a program that throws and catches, and judged by the verifier
    with no FDE `unknown`;
  - `odd-length` (X-26): an FDE of length 17 and a following one; the
    output records are 4-aligned and both unwind.
- **symtab** (`tests/link/symtab/`): S-121 (`main` and libc globals
  `GLOBAL` in a static link; `skip(mold)`); S-122 and S-123
  (`__TMC_END__`'s index holds it; `__TMC_LIST__ == __TMC_END__`; a
  `-fgnu-tm` program's table); S-124 (`__bss_start` with
  `--gc-sections` and no `.bss`); S-74 order; S-78 for `-X`, `-x`,
  `--discard-none`; S-79; S-81 TLS values; S-80 a thunk symbol; S-85
  with 65300 generated sections (slow).
- **notes** (`tests/link/notes/`): the X-19, X-20 and X-33 matrices
  (`STACK_SIZE` with an input without it, `NO_COPY_ON_PROTECTED`, the
  generic AND and OR ranges, `MEMORY_SEAL` with and without `-z
  memory-seal`, GCS, PAUTH equal, different and absent, unknown generic,
  processor and user types with the `unknown-property` warning); two
  notes in one section, equal (libhwasan's `hwasan_interceptors_vfork.o`)
  and disagreeing (S-126); S-99; S-91 sizes per style; S-100.
- **GOT/IPLT** (`tests/link/ifunc/`, `tests/link/tls/`): S-67 to S-70 per
  architecture (sections, words, `readelf -r`; qemu for x86); S-71 with
  X-27's program: `IFUNC` at the resolver in `readelf -s`, the program's
  pointer is the IPLT entry, gdb's `break f` stops in the implementation.
- **verifier**: every output passes `plumb.md` §6.4 E1 to E5 and §6.5 Y2
  to Y5, and the S16 and S17 rules of R-P1 with the mutations of R-P3.
- **unit tests** (`mortar_merge`, `mortar_synth`): S-28 determinism (one
  `Split` and `MergeMap` at 1, 2, 4, 8 and 64 threads and with equal-named
  inputs permuted, equal to a sequential first-occurrence numbering);
  S-46, S-47 and S-49 on synthetic record lists (S-49: `select` then
  `from_records` is the plan; a kept equal CIE is taken by its section's
  FDEs; each of the five refusals by name); S-50's five cases; S-88
  counts against a model; S-95 and S-126 against a table of cases.

## 17. `.gdb_index` (WP-097)

*Added by WP-097 from WP-080 stage B's proposal
(`spec/landed/image/WP-080-stage-b.md` §4, S-G1 to S-G9); the evidence
is in `spec/landed/image/WP-097.md`. The numbering of §13 to §16 is
kept, since other specs and the verifier cite them: the brief's
"section 13" is this section.*

`--gdb-index` asks for a `.gdb_index` section: a name and address index
that gdb reads instead of scanning every unit at start-up. gdb trusts it:
a name the index does not list is a name gdb does not find, and an
address the index does not map is an address gdb cannot attribute to a
unit (stage B §2.1 measured both with lld's and mold's output). So the
index mortar writes is **complete**, or the link is refused by name.
Rules that only gdb's reader defines cite it: gdb's manual, appendix
"`.gdb_index` section format", and the reader in gdb 16.3
(`gdb/dwarf2/read-gdb-index.c`), with what was observed in
`spec/landed/image/WP-097.md`.

### 17.1 When there is one

- **S-G1 [M2] (the chunk).** With `--gdb-index` in force
  (`plan.shaping.gdb_index == Some(true)`: of `--gdb-index` and
  `--no-gdb-index` the last wins) and at least one unit in the output's
  `.debug_info` (S-G2), the link has one synthetic chunk `.gdb_index`,
  S-3 row 28: `SHT_PROGBITS`, no flags, alignment 4, `sh_entsize` 0, not
  allocated; layout places it in rank 9 (`spec/layout.md` L-60) after
  the `.debug_*` output sections, before `.symtab`. Its content is
  version 7 of gdb's format (§17.5), the version lld 22 and mold write;
  gdb 16 reads it (its `maint print objfiles` names `.gdb_index:
  version 7`). Versions 8 and 9 (what `gdb-add-index` writes; 9 adds a
  shortcut table for the main function and its language) add nothing a
  linker's index needs. Without the option nothing of this section
  runs: no input is read, no table is built, and every byte of the
  output is what it was before WP-097 (S-109; the bench's static rows
  and the byte-identical lane show it).
- **S-G8 [M2] (with other options).** `--gdb-index` with
  `--incremental` is one of `spec/incr.md` IN-12's incompatible modes
  (IN-58, unchanged): the warning `incremental-unsupported` names
  `--gdb-index` and the link is plain, with its index and without a
  state; `--incremental=update` is an error. With `-S`/`--strip-debug` or `-s`
  the output has no `.debug_info`, so S-G1 makes no chunk and nothing is
  read or refused. `-r` is refused as a whole today (`spec/cli.md`);
  when it lands, a relocatable output gets no `.gdb_index` (an index is a
  property of the final link: lld and gold write none with `-r`).
- **S-G9 [M2] (`--debug-names`).** `--debug-names` (lld's merged
  `.debug_names`) stays refused, with the reason in the diagnostic's
  note: gdb 16.3 ignores every `.debug_names` it did not write
  ("`.debug_names` not created by gdb; ignoring", stage B §2.1 item 1),
  so a merged table would give gdb nothing; `.gdb_index` is the index
  mortar writes. `--no-debug-names` is a no-op (it asks for mortar's
  behaviour; refusing it was a defect).

### 17.2 Units

- **S-G2 [M2] (the units; the CU list).** A *unit* is a unit header in
  an input section named `.debug_info` that is in the output: kept by
  resolution (not dropped as a COMDAT copy, not stripped) and live
  (`spec/image.md` I-6: a non-allocated section is a root unless it is
  in a section group with an allocated member, with which it lives and
  dies). A unit is *live* exactly when its section is. The CU list is
  every unit, in output order: the `.debug_info` input sections in the
  order layout places them in output section `.debug_info` (`SectionId`
  order, which is input order), and the units of one section in section
  order. Units are read from offset 0 of their section to its end; a
  section's bytes that are not a whole number of units are
  `gdb-index-malformed-unit`. Each unit is accepted when:
  - the unit length is not `0xffffffff` (DWARF64 is refused,
    `gdb-index-malformed-unit`);
  - the version is 2, 3, 4 or 5;
  - the address size is the output's word size (8, or 4 on i386);
  - its type is a compile unit: DWARF 5 `DW_UT_compile` (1) or
    `DW_UT_skeleton` (4), a DWARF 2-4 unit whose root DIE is
    `DW_TAG_compile_unit` (0x11). `DW_UT_type` (2) and `DW_UT_split_type`
    (6) are `gdb-index-type-unit`, and so is any live input section named
    `.debug_types` (DWARF 4 type units): the TU list stays empty until a
    package tests one (stage B S-G2). `DW_UT_partial` (3),
    `DW_UT_split_compile` (5), a root `DW_TAG_partial_unit` (0x3c) and
    every other value are `gdb-index-malformed-unit`.

  The CU list entry is (the unit's offset in the output's `.debug_info`,
  its length including the 4-byte length field), both `u64`.
- **S-G3a [M2] (debug-bearing).** A unit is *debug-bearing* when it is a
  skeleton (`DW_UT_skeleton`, or a root DIE with `DW_AT_GNU_dwo_name`
  (0x2130) or `DW_AT_dwo_name` (0x76)) or its root DIE has children (its
  abbreviation's `DW_CHILDREN_yes`). A unit that is neither (what an
  assembler writes for `gcc -g x.S`: a root DIE with a line table and an
  address range, nothing to name) needs no name set; it is in the CU
  list and the address area all the same.
- **S-G3 [M2] (complete, or refused).** Every live debug-bearing unit
  must be named by exactly one GNU pubnames set and exactly one GNU
  pubtypes set (S-G4a). The first live debug-bearing unit, in CU-list
  order, that has none of one of them refuses the link as
  `gdb-index-incomplete`, placed at its file (`{path}` or
  `{archive}({member})`), with a note giving the unit's offset in its
  section, which set is missing, and the fix: compile it with
  `-ggnu-pubnames` (GCC and Clang; GCC's `-gsplit-dwarf` implies it) or
  link without `--gdb-index`. When the file has a `.debug_names` section
  (Clang's `-gpubnames`) or a `.debug_pubnames` section (DWARF's own
  `.debug_pubnames`, which has no attribute byte and cannot give gdb its
  kinds), the note says it is not read (S-G4a). This rule is one
  decision in one place (`mortar_synth::gdbindex::INCOMPLETE`): the
  operator may later make it "warn and write no index", which changes
  that constant, this paragraph and the tests of the refusal, nothing
  else.

### 17.3 Name sets

- **S-G4a [M2] (the sources).** The names come from the sets of the
  input sections named `.debug_gnu_pubnames` and `.debug_gnu_pubtypes`
  that are not dropped as COMDAT copies (with the option they are read
  and not copied, S-G6). A section holds sets one after another, from
  offset 0 to its end. A set is:

  | field | size | rule |
  |---|---|---|
  | `unit_length` | 4 | not `0xffffffff` (DWARF64 refused); the set ends `4 + unit_length` after its start, within the section |
  | `version` | 2 | 2 |
  | `debug_info_offset` | 4 | relocated (below): names the unit |
  | `debug_info_length` | 4 | not read (for a skeleton it is the `.dwo` unit's length) |
  | entries | | `(u32 die_offset, u8 attributes, name)` until a `die_offset` of 0; the name is NUL-terminated and may be empty (Clang 22 names a string literal's unnamed `DW_TAG_variable` with `""`; lld and mold index it as the name `""`, and so does mortar) |

  The terminator is the set's last 4 bytes; a set that ends before it,
  bytes after it, a name that runs past the set, or an attribute byte
  whose bits 0-3 are not 0 or whose kind (bits 4-6) is 5, 6 or 7 are
  `gdb-index-malformed-name-set`. Its `debug_info_offset` field has a
  relocation of absolute class (`R_AARCH64_ABS32`, `R_X86_64_32`,
  `R_386_32`) against a symbol in an input `.debug_info` section of the
  same file; the target is that section at `st_value + addend` (REL:
  the implicit addend). A field without such a relocation, or a target
  that is not the start of a unit of its section, is
  `gdb-index-malformed-name-set`; a target section that is not in the
  output (a dropped COMDAT copy) is `gdb-index-orphan-name-set`. Two
  pubnames sets (or two pubtypes sets) naming one unit are
  `gdb-index-malformed-name-set`. A set naming a unit that is not
  debug-bearing is read as any other.

  Not read: `.debug_names` (Clang's `-gpubnames`, DWARF 5's per-unit
  name index). Its entries carry `DW_IDX_die_offset` and an abbreviation
  tag, from which a kind could be derived (a tag of a type is
  `TYPE`, a subprogram `FUNCTION`, a variable `VARIABLE`), but no
  static bit: gdb's `.gdb_index` reader treats a name without the static
  bit as global, so a file-local `static int x` in two units would be
  indexed as two globals. And stage B's observation that lld and mold
  build 30 symbols from such inputs is not what lld 19 and 22 and mold
  2.37 write here: each writes an empty symbol table for Clang
  `-gsplit-dwarf -gpubnames` objects
  (`spec/landed/image/WP-097.md`). So a unit named only by `.debug_names`
  has no set (S-G3). Input `.debug_names` sections are copied as
  before (I-49a; lldb reads them): gdb 16.3 ignores a `.debug_names` it
  did not write (stage B §2.1) and uses `.gdb_index`. Old-style
  `.debug_pubnames`/`.debug_pubtypes` (no attribute byte) are not read
  either and are copied as before.
- **S-G4 [M2] (names and attributes).** A unit's names are the entries
  of its pubnames set in set order, then those of its pubtypes set.
  Each entry gives the name (its bytes, without the NUL, compared byte
  for byte: `Foo` and `foo` are two names) and the CU-vector value
  `cu | (attributes << 24)`, where `cu` is the unit's index in the CU
  list and `attributes` the entry's attribute byte unchanged: bits 28-30
  of the value are the kind (gdb's `GDB_INDEX_SYMBOL_KIND`: 0 none, 1
  type, 2 variable, 3 function, 4 other) and bit 31 is static, as the
  pubnames attribute byte has them in bits 4-6 and 7 (gdb manual,
  "Index Section Format", the CU vector; GCC's `dwarf2out.cc` writes the
  byte with gdb's macros). A name's CU vector is the set of the values
  of all its entries, sorted ascending as `u32` with equal values
  written once (gdb's own writer does this; lld 22 writes duplicates,
  `spec/landed/image/WP-097.md`). `cu` must be below 2^24 (24 bits);
  more units than that are `gdb-index-too-large`.
- **S-G6 [M2] (the input sets are consumed).** With `--gdb-index` in
  force and neither `-S` nor `-s`, resolution gives the input
  `.debug_gnu_pubnames` and `.debug_gnu_pubtypes` sections that it would
  keep the state `Dropped(Consumed)` (`spec/resolve.md` §10): phase 80
  reads them and they are not copied (lld and mold drop them too). A
  COMDAT copy stays `Dropped(Comdat)` and a stripped one
  `Dropped(Stripped)`. Without the option I-49a is unchanged: they are
  ordinary debug sections, copied and relocated.

### 17.4 Address ranges

- **S-G5 [M2] (the address area).** A unit's ranges are read from its
  root DIE only (what gdb's reader of the unit's addresses uses when
  there is no index; `.debug_aranges` is not read):
  - `DW_AT_low_pc` with `DW_AT_high_pc`: one range `[low, high)`, or
    `[low, low + n)` when `DW_AT_high_pc` is a constant (`DW_FORM_data1`
    to `data8`, `udata`, `implicit_const`);
  - `DW_AT_ranges`: the range list it names. DWARF 5:
    `DW_FORM_sec_offset` into `.debug_rnglists`, or `DW_FORM_rnglistx`
    through the offsets table at `DW_AT_rnglists_base`. DWARF 2-4:
    `DW_FORM_sec_offset` (`data4` before DWARF 4) into `.debug_ranges`.
    `DW_AT_GNU_ranges_base` is not added: in a DWARF 4 skeleton it
    applies to the `.dwo` unit's `DW_AT_ranges`, never the skeleton's
    own (LLVM's reader says so; GCC 14's skeletons agree);
  - neither: no range.

  An address is read through the relocation at its field: `DW_FORM_addr`
  in `.debug_info`, a `.debug_addr` entry for `DW_FORM_addrx`,
  `addrx1`-`addrx4` and `DW_FORM_GNU_addr_index` (at `DW_AT_addr_base`
  or `DW_AT_GNU_addr_base` plus index × address size; the base is a
  relocated section offset into the file's `.debug_addr`), or a range
  list entry. Range list entries: `.debug_rnglists` (DWARF 5) codes 0
  `end_of_list`, 1 `base_addressx`, 2 `startx_endx`, 3 `startx_length`,
  4 `offset_pair`, 5 `base_address`, 6 `start_end`, 7 `start_length`;
  `.debug_ranges` pairs `(begin, end)` with a begin of all ones (in the
  address size) selecting a base and a pair of two zeros **without
  relocations** ending the list. The base address starts as the unit's
  `DW_AT_low_pc` (0 if absent). A section offset (`DW_AT_ranges`,
  `DW_AT_rnglists_base`, `DW_AT_addr_base`, `DW_AT_GNU_addr_base`, the
  unit header's `debug_abbrev_offset`) is read through its relocation,
  whose target names the section (the file's `.debug_rnglists`,
  `.debug_ranges`, `.debug_addr`, `.debug_abbrev`) and the offset in it
  (`st_value + addend`). An offset without a relocation, or one whose
  target has another name, is `gdb-index-malformed-unit`.

  An address is a pair (section or none, offset): with a relocation of
  absolute class against a symbol defined in an input section, that
  section and `st_value + addend` (plus a constant added to it, such as
  an `offset_pair`'s); without one, no section and the field's value. A
  range is `[start, end)` whose two ends have the same section (or both
  none); its length is `end − start` in input terms, so it is known
  before layout. Then:
  1. a range of length 0 is left out (lld and mold leave them out; gdb
     ignores them);
  2. a range whose section is not in the output (a COMDAT copy dropped
     for another unit's, a section collected by `--gc-sections`) is left
     out: its addresses are the tombstone in the output (`spec/image.md`
     I-45, I-47) and gdb finds no code there either;
  3. a range with no section and a length above 0 is
     `gdb-index-malformed-unit` (no code of a relocatable object is at an
     absolute address), and so are ends in two sections, a negative
     length, a relocation of another class at an address field
     (`R_*_NONE` counts as none), a symbol that is not defined in a
     section of the link, a form or code not listed here in the attributes
     read, and any read past the end of a section.

  The address area lists, for each unit in CU-list order, its kept
  ranges in the order its DIE and range list state them, each as
  `(u64 low, u64 high, u32 cu)` with `low` the output address of the
  range's start (`addr_in(section, offset)`, model §4.3, merge-aware) and
  `high = low + length`, exclusive. Nothing is sorted, merged or
  de-duplicated (lld does the same; gdb's reader builds its own map).
  A unit with no kept range has no entry: gdb finds it by name only (a
  data-only unit, or one whose code was all collected).

  The other attributes of the root DIE are skipped by their form's size:
  every DWARF 5 form, `DW_FORM_indirect`, and GNU's `0x1f01`
  (`GNU_addr_index`), `0x1f02` (`GNU_str_index`), `0x1f20` and `0x1f21`
  (`GNU_ref_alt`, `GNU_strp_alt`, 4 bytes in 32-bit DWARF).
- **S-G5a [M2] (a dead unit).** A unit stays in the CU list while its
  `.debug_info` section is in the output, even when `--gc-sections`
  collected every function it describes: its names are indexed (S-G4)
  and its ranges are all left out (S-G5 rule 2). gdb then finds such a
  name, expands the unit and reads the DIEs, whose addresses are the
  tombstone: what it reads without an index (the session test of
  `crates/mortar/tests/debuggers.rs` compares both). A unit whose
  `.debug_info` section is dropped (a COMDAT group not kept) is not in
  the output, so it has no entry and its names, if a set of a kept
  section names it, are `gdb-index-orphan-name-set`.

### 17.5 The bytes

- **S-G7 [M2] (the format, version 7, little-endian).** All integers are
  little-endian, whatever the target (gdb's format). With `C` units, `R`
  kept ranges, `N` distinct names and `T` slots:

  | offset | size | field |
  |---|---|---|
  | 0 | 4 | version: 7 |
  | 4 | 4 | CU list offset: 24 |
  | 8 | 4 | TU list offset: `24 + 16 C` |
  | 12 | 4 | address area offset: the TU list offset (the TU list is empty) |
  | 16 | 4 | symbol table offset: `24 + 16 C + 20 R` |
  | 20 | 4 | constant pool offset: the symbol table offset `+ 8 T` |
  | 24 | `16 C` | the CU list (S-G2): `(u64 offset, u64 length)` per unit |
  | | `20 R` | the address area (S-G5) |
  | | `8 T` | the symbol table: `T` slots of `(u32 name, u32 vector)`, both offsets from the start of the constant pool; an empty slot is `(0, 0)` |
  | | | the constant pool: first the CU vectors of the `N` names in name order, each `u32 count` then `count` values; then the names in name order, each its bytes and a NUL |

  **Name order** is the order of first occurrence: the units in CU-list
  order, each unit's names in S-G4's order (pubnames, then pubtypes,
  each in set order); a name takes the next number the first time it
  is seen. Its vector holds every value of S-G4, wherever it occurs.

  **The hash** is gdb's `mapped_index_string_hash` for versions 5 and
  later: `h = 0`, then for each byte `c` of the name (unsigned, without
  the NUL) `h = h × 67 + lower(c) − 113` modulo 2^32, where `lower` maps
  ASCII `A`-`Z` to `a`-`z` and leaves every other byte (gdb uses
  libiberty's `TOLOWER`, which is ASCII-only).

  **The table's size** `T` is `max(1024, P)`, where `P` is the smallest
  power of two strictly greater than `⌊4N/3⌋` (lld 22's rule; it keeps
  the load at most 3/4, and gdb's reader needs a power of two). For `N`
  up to 767, `T` is 1024.

  **The probe sequence** (gdb's writer and reader): with `m = T − 1`,
  the first slot is `h & m` and the step `((h × 17) & m) | 1` (modulo
  2^32 before the mask); the next slot is `(slot + step) & m`. Names are
  inserted in name order, each into the first empty slot of its
  sequence. Every name is distinct, so a slot is never shared.

  Every offset and count fits its `u32` field; a section of more than
  `2^32 − 1` bytes is `gdb-index-too-large`. The bytes are a pure
  function of the CU list and the address area (the layout's offsets and
  addresses) and of the names and their values (the input sets): no
  hash order, thread count or address enters anything but the fields
  that hold them.
- **S-G7a [M2] (sizes and division).** The size is
  `24 + 16 C + 20 R + 8 T + Σ (4 + 4 × |vector|) + Σ (|name| + 1)`, a
  function of counts known after phase 80 (S-G5's lengths are input
  terms), so the chunk's size never depends on an address (S-1). The
  symbol table and the constant pool are built in phase 80 (they need no
  address); the header, the CU list and the address area are written at
  fill, from the layout. The chunk is `Whole` (S-4), bound by its size:
  one job writes it. A later package can make it `Divisible` (the CU
  list and the address area by entry, the rest by copying), which the
  bench does not ask for yet (`spec/landed/image/WP-097.md`, cost).
  The plan holds the distinct names once (in the pool it will write) and
  one `(name, value)` pair per entry before the vectors are built; it
  reads each input set once; nothing is quadratic.

### 17.6 The refusals

All are `Refused { Feature, name }` errors (`spec/diag.md` §4), placed
at the file (and the section and offset when one is known), with the
detail in a note. Each stops the link in phase 80.

| name | when | rule |
|---|---|---|
| `gdb-index-incomplete` | a live debug-bearing unit has no pubnames set or no pubtypes set | S-G3 |
| `gdb-index-type-unit` | a type unit in `.debug_info`, or a live `.debug_types` | S-G2 |
| `gdb-index-malformed-unit` | a unit, its root DIE, an abbreviation, a range list or an address field that S-G2/S-G5 cannot read | S-G2, S-G5 |
| `gdb-index-malformed-name-set` | a set that does not parse, does not name the start of a unit, or names a unit a second time | S-G4a |
| `gdb-index-orphan-name-set` | a set naming a unit whose section is not in the output | S-G4a, S-G5a |
| `gdb-index-too-large` | `2^24` units or more, or a section above `2^32 − 1` bytes | S-G4, S-G7 |

`--gdb-index` with `--incremental` is one of IN-12's incompatible modes
(IN-58): the warning `incremental-unsupported` names it and the link is
plain, with the index and without a state; with `--incremental=update`
it is an error. `--debug-names` is `unsupported option: --debug-names`
with S-G9's note.

### 17.7 What the verifier reads (WP-098)

Everything S-G1 to S-G7 use is in the inputs and the map
(`spec/plumb.md` §3.2) already: the `.debug_info` input sections'
output offsets are their `nplace` records, the code sections' addresses
their `place` records, and the consumed sets are input sections of
known names whose state the verifier recomputes from the option (`arg`
records give `--gdb-index`, in every map since WP-102, `spec/plumb.md`
§3.2a). The chunk is named by its `chunk synthetic
.gdb_index - <size>` record. No map record is added.

### 17.8 Where the references differ

| point | lld 19 and 22 | mold 2.37 | mortar |
|---|---|---|---|
| a unit with no name set | indexed without names: gdb misses its types and globals | the same | refused, `gdb-index-incomplete` (S-G3) |
| Clang `-gpubnames` (`.debug_names` only) | empty symbol table | empty symbol table | refused, the note naming `.debug_names` (S-G4a) |
| duplicate values in a CU vector | written | removed | removed, sorted (S-G4) |
| table size | `max(1024, next power of two above 4N/3)` | smaller (64 slots for 34 and for 37 names) | lld's, which is also `gdb-add-index`'s rule (S-G7) |
| alignment | 1 | 4 | 4 (S-G1) |
| type units | TU list | TU list | refused (S-G2) |
| input pubnames | dropped | dropped | dropped (S-G6) |
| an empty name (Clang's unnamed variables) | indexed | indexed | indexed (S-G4a) |
| ranges | from the unit DIE, empty ones left out | from the unit DIE | from the unit DIE, empty and dead ones left out (S-G5) |

### 17.9 Tests

- `crates/mortar_synth`: unit tests of `gdbindex` (the hash, the size
  rule, the probe, sorted and unique vectors) and of `gdbread` (LEB128,
  every form's size, abbreviations); `tests/gdb_add_index.rs` holds the
  hash, the probe and the size rule to the bytes `gdb-add-index` (gdb
  16.3) wrote for 1303 names (`tests/data/gdb-add-index-v9.bin`);
- link tests, on the three architectures with lld and mold beside
  mortar: `tests/link/split/gdb-index-gcc.test` (GCC DWARF 5 and 4
  split, `--gc-sections`, the last of `--gdb-index`/`--no-gdb-index`,
  `-S`), `gdb-index-clang.test` (Clang 22 split DWARF 5 and 4, non-split
  `-ggnu-pubnames`, `--gc-sections`), `gdb-index-pubnames.test`
  (non-split `-g -ggnu-pubnames`: GCC DWARF 5 and 4 at `-O0` and `-O2`,
  Clang at `-O2`), `gdb-index-refused.test` (each refusal by name: a
  mixed build at the object without a set, Clang `-gpubnames`, DWARF 4
  and 5 type units, a malformed set, a partial unit, an orphan set; lld's
  and mold's index of the mixed build without its names; the
  incremental warning; `--debug-names` and `--no-debug-names`), each
  with `readelf --debug-dump=gdb_index` and `llvm-dwarfdump --gdb-index`;
  their mortar links are rows of the byte-identical lane;
- `crates/mortar/tests/gdb_index.rs`: mortar's index against lld's and
  mold's on the three architectures, eight GCC and Clang forms, plain
  and with `--gc-sections`, by a reader that shares nothing with the
  writer (§17.8's differences only);
- `crates/mortar/tests/debuggers.rs`: `gdb_index_is_used_and_answers_the_same`
  (gdb's and lldb's transcripts with and without the index equal; gdb
  reports `.gdb_index: version 7`) and
  `gdb_index_of_a_mixed_build_is_refused` (stage B's failure case: lld's
  index hides the third unit's type and variable from gdb; mortar
  refuses the link at that object).
