# spec: synthetic sections (`mortar_merge`, `mortar_synth`)

Status: draft (WP-025 stage 1). The evidence for each non-obvious rule is
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
outputs; M3: x86-64; M4: i386; M7: incremental linking. A rule tagged
with a later milestone binds the design now: an M1 implementation must
not make that rule hard to add. A rule with no tag applies from M1. Rules
that name x86-64 or i386 are M3 and M4 rules; they are written now
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
  depend on addresses: `.relr.dyn` [M2] (model §4.4 `sized`), and the
  symbol tables when thunks or patches exist (S-80, S-88: their size is fixed
  after layout's fixed point, which no address depends on, because the
  symbol tables come after every allocated chunk, layout rank 10). No
  synthetic chunk's size depends on the bytes another chunk will hold.
- **S-3 [C] (the synthetic chunk list).** model §4.4 gives `ChunkId`s to
  synthetic chunks "in the order synth.md lists them". The list, with
  the milestone at which each can exist:

  | # | name | content | exists when | M |
  |---|---|---|---|---|
  | 1 | `.interp` | the interpreter path | dynamic output | M2 |
  | 2 | `.note.gnu.property` | merged properties (§8.2) | some property survives the merge | M1 |
  | 3 | `.note.gnu.build-id` | the build id (§8.1) | `--build-id` other than `none` | M1 |
  | 4 | `.gnu.hash` | hash table | dynamic output | M2 |
  | 5 | `.dynsym` | dynamic symbols | dynamic output | M2 |
  | 6 | `.dynstr` | dynamic strings | dynamic output | M2 |
  | 7 | `.gnu.version` | `versym` | a version is defined or needed | M2 |
  | 8 | `.gnu.version_d` | version definitions | a version script defines one | M2 |
  | 9 | `.gnu.version_r` | version needs | a shared input has versions | M2 |
  | 10 | `.rela.dyn` (`.rel.dyn`) | dynamic relocations | some dynamic relocation | M2 |
  | 11 | `.relr.dyn` | packed relative relocations | `-z pack-relative-relocs` | M2 |
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

  A chunk that does not exist gets no `ChunkId`. The numbering is a
  name, not an order: `Placement.order` (layout §10) is the order.
  `.comment` is not in the list: it is a merged chunk with a piece of
  the internal file (§9).
- **S-4 [C] (divisible chunks).** The image phase may fill these
  synthetic chunks in parallel by sub-range (model §5 "a record range
  of one"): `.eh_frame` by record range, `.symtab` and `.strtab` by
  file, `.got` by slot range, `.rela.plt` and `.plt` by entry range. The
  others are one unit each. A unit's bytes depend only on the tables and
  addresses; no unit reads another unit's output, with one exception the
  writer orders explicitly: the build id (S-92), computed over the
  finished image.

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
  - has `sh_addralign` that is not a power of two (refused already by
    `elf.md`), or `sh_entsize` larger than 2^16.

  None of these occurs in the library scan (X-1); the rule exists so
  that a hand-written object cannot make a wrong merge. Such a section
  keeps `SHF_MERGE` out of its output section's flags (layout L-50).
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
  representative's key, ascending, by the method of model §2.3 (atomic
  minimum, then a prefix count over owner marks). The numbering is a
  function of the inputs and not of the thread count (model §6).
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
  `r < 0` or `r ≥ sh_size` is refused: `malformed-input`,
  `offset is outside the section` (lld's wording and behavior). At
  `r == sh_size` GNU silently writes the end of the *output* section,
  which is the end of some other object's string, and for `r > sh_size`
  or `r < 0` it warns `access beyond end of merged section` and does the
  same; mold maps `r == sh_size` to the end of the last piece and writes
  an unrelated address beyond it (X-2). No such reference occurs in the
  library scan (X-1), and the compilers emit one-past-the-end and
  before-the-start addresses through named symbols (X-5), which S-19
  handles.
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
  sections; lld does at `-O2`; mold never does (X-6). Measured gains are
  small in loaded data and moderate in debug strings (X-8): on a static
  C++ program, 64 bytes of 114 KB of `.rodata` (lld `-O1` against `-O2`),
  and 13% of `.debug_str` and 4% of `.debug_line_str`. The cost is a sort
  of all strings of a class by reversed content (lld's `-O2` method), a
  sequential step over all strings of an output section that cannot be
  sharded by hash, and it couples pieces (one string's bytes then hold
  another's), which would make the append-only merged chunk of layout
  L-147 depend on which strings left. Q1 asks whether to add it later
  behind `-O2`; the `MergeMap` validity rule (model §4.2) already allows
  overlapping fragments, so adding it changes no table.

### 3.7 Output order and offsets

- **S-24 [C] (order).** Within a `Merged` chunk (layout L-85), the live
  fragments are grouped by merge class (strings before constants, then
  entsize ascending, then alignment ascending) and ordered by `FragId`
  within a group. `frag_off` is assigned by one pass in that order,
  aligning each fragment to its alignment: `frag_off(f) =
  align_up(end of the previous fragment, align(f))`. The order is the
  order of first occurrence, which is GNU's order for the strings that
  survive its tail merge (X-6); lld and mold order by hash (lld by shard,
  then hash), which no tool relies on but which makes a one-string edit
  move every string. First-occurrence order keeps a program's strings in
  source order and makes an incremental append (L-147) the same order a
  full link would give when the new strings are in the last file.
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

- **S-28 [C] (method).** Phase 60 runs per section in parallel:
  1. **cut**: find piece boundaries (a vectorizable scan for zero
     characters, or a stride) and hash each piece's bytes with the class
     and alignment, one 64-bit hash;
  2. **intern**: insert `(hash, bytes)` into a table sharded by the high
     hash bits, 2^k shards for k with 2^k ≥ 8 × threads; each entry keeps
     the least key of the pieces that inserted it, the key being
     `SectionId × 2^32 + piece` in one `u64` (`SectionId`s are given in
     `(FileId, sh_index)` order, model §2.2, so this is the
     `(FileId, sh_index, piece)` order), kept by `AtomicU64::fetch_min`
     (safe Rust; no lock is held across the minimum);
  3. **mark owners**: every piece whose key equals its entry's key is a
     representative;
  4. **number**: count representatives per section, prefix-sum in
     section order (`SectionId` order), and give each section's
     representatives consecutive `FragId`s in piece order; store each
     entry's `FragId`;
  5. **resolve**: every piece occurrence reads its entry's `FragId`.

  No step depends on the order threads run in or on the table's layout;
  the output is a function of the inputs (model §6 rules 2, 3 and 5).
  Hashes decide only equality candidates; equal hashes compare bytes.
- **S-29 [C] (offsets).** `frag_off` is one sequential pass per output
  section over its live fragments in S-24 order (an aligned prefix sum);
  for 10^7 fragments it is about 10 ms on one core. A parallel prefix sum
  with alignment is possible (block sums computed per residue of the
  largest alignment) and is not needed at that cost.
- **S-30 [C] (cost, stated).** For 10^7 string pieces of mean length 32
  (320 MB of input strings, a debug build of a large C++ program) on 8
  threads: cutting and hashing is bound by memory bandwidth (about 40 ms);
  interning is 10^7 lookups into a table of about 4×10^6 entries
  (3×10^6 to 5×10^6 distinct strings is typical), about 0.1 s with
  sharded locks; numbering and resolving are linear passes (about 20 ms
  each). Memory: 8 bytes per occurrence (80 MB), 16 bytes per fragment
  plus the table entry (about 32 bytes at load 0.5): 200 MB for that
  link. The numbers are estimates from the table sizes, to be replaced by
  the bench harness's figures (`docs/VALIDATION.md`).
- **S-31 [C] (what is taken from mold and lld).** Taken: interning in a
  concurrent, sharded table (both); piece boundaries found per section in
  parallel (both). Rejected: output order by hash or shard (both; S-24);
  choosing the kept copy by a race decided afterwards (mold's
  "lowest-priority owner" is the same idea as S-14 and is taken); lld's
  `-O0` no-merge mode (S-32).

### 3.9 Options

- **S-32 [C] (`-O`).** `-O LEVEL` (`-O0`, `-O1`, `-O2`, any integer) is
  accepted and changes nothing: mortar merges at every level, as GNU and
  mold do (X-6; GNU's output is the same at `-O0` and `-O2`). lld does
  not merge at `-O0`; no build depends on that, since it only grows the
  output.
- **S-33 [C] (no other merge options at M1).** No option turns merging
  off. GNU ld has none; lld's `-O0` is S-32. An option to disable it
  (for debugging the linker) is a test hook, not a user option. `-z`
  options naming merge behavior (`-z nomerge` does not exist in any of
  the three) remain `unknown-option` per `spec/cli.md`.

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
- **S-35 [C] (accepted CIEs).** A CIE is accepted when:
  - its version is 1 or 3 (the scan has only 1; version 3 is DWARF 3's
    `.debug_frame` CIE, which some assemblers emit in `.eh_frame`; all
    three accept it, X-14);
  - its augmentation string is empty, or starts with `z` and continues
    with letters from `R`, `P`, `L`, `S`, `B`, `G` (each at most once).
    `S` is a signal frame, `B` an AArch64 B-key frame, `G` a memory-tagged
    stack frame (AArch64 MTE); they have no data in the augmentation and
    the linker passes them through. The scan has `zR`, `zPLR` and one
    `zRS` (X-1);
  - its `R`, `P` and `L` pointer encodings (the low nibble a format, the
    high nibble an application) are ones S-37 can relocate.

  An empty augmentation means the FDE's initial location is `absptr`.
- **S-36 [C] (refused CIEs).** A CIE with version other than 1 or 3, or
  with an augmentation character outside S-35, or one whose augmentation
  data runs past the record, is refused: `malformed-input`, `FDE version
  1 or 3 expected, but got N` and `unknown .eh_frame augmentation string:
  STR` (lld's wording; lld refuses exactly these, X-14). GNU warns
  `error in FILE(.eh_frame); no .eh_frame_hdr table will be created` and
  links without a table, which silently loses the unwinder's fast path
  for every frame in the program; mold accepts and copies. None occurs in
  the scan.
- **S-37 [C] (pointer encodings).** A pointer in a CIE or FDE is located
  by its encoding and relocated by the relocation at its place, if any.
  The formats `absptr`, `udata2/4/8`, `sdata2/4/8` and the applications
  none and `pcrel` (`0x10`), optionally `indirect` (`0x80`), are
  accepted; `datarel` (`0x30`), `textrel` (`0x20`), `funcrel` (`0x40`),
  `aligned` (`0x50`) and `uleb128`/`sleb128` formats in a pointer that
  carries a relocation are refused (`feature`, `.eh_frame pointer
  encoding 0xNN`): none is emitted by GCC, Clang or hod and their meaning
  in a static executable needs a base the LSB does not fix. The scan has
  FDE encoding `0x1b` (`pcrel|sdata4`), personality `0x9b`
  (`indirect|pcrel|sdata4`) and LSDA `0x1b` (X-1); hod's large model
  uses `0x1c` with `PREL64` (a64.md §13.1).
- **S-38 [C] (FDE code range).** An FDE's code section is the section of
  the target of the relocation at its initial-location field (offset 8
  of the record). The relocation's class must be `Pc` or `AbsWord`
  (arch.md §3.9); another is refused there. An FDE whose initial
  location has no relocation is dropped (lld and mold drop it; GNU keeps
  it and gives up on `.eh_frame_hdr`, X-14): it describes no code of
  this link. None occurs in the scan. An FDE whose target is undefined
  weak, absolute, or in a section that is not live is dead (S-41).
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
  (X-14): a record length that is not a multiple of 4 (records are
  copied byte for byte; the scan has none, X-24); a CIE that no FDE uses
  (it is kept if live, S-42); several CIEs in one section (up to three in
  the scan); FDEs with the same initial location (§5 handles the table).

### 4.2 Liveness and de-duplication

- **S-41 [C] (dead FDEs).** An FDE is live exactly when its code section
  is live and not folded away (model §3.6), and the FDE is not dropped
  by S-38. FDEs whose code section was discarded as a losing COMDAT copy
  are therefore dead (resolve.md §6.3), as are those of sections removed
  by `--gc-sections`. All three references agree on the count of live
  FDEs (X-10).
- **S-42 [C] (live CIEs).** A CIE is live when a live FDE uses it. A CIE
  that no live FDE uses is dropped. (lld and mold drop unused CIEs; GNU
  keeps CIEs of sections it keeps. Nothing reads a CIE except through an
  FDE.)
- **S-43 [C] (CIE identity).** Two live CIEs are one when their bytes are
  equal and, at each relocated place, the relocations have the same
  type, the same addend, and targets that resolve to the same final
  symbol or the same section and offset. The personality routine is
  usually such a target (`DW.ref.__gxx_personality_v0` through
  `0x9b`): two `zPLR` CIEs equal in bytes but with different personality
  targets are two CIEs (X-11). Identity is the equality of the
  triple (bytes, relocations by place, resolved targets); it is computed
  after resolution, so it is exact.
- **S-44 [C] (which copy).** Among equal live CIEs the one with the least
  `(FileId, sh_index, record)` is kept; every live FDE that used any of
  them points to it. With C++ COMDAT and `--gc-sections` the three
  references keep the same number of CIEs (3 in X-10).
- **S-45 [C] (piece row).** `Split` holds per record its offset (`u32`)
  and an `EhPiece` (model §3.5): `cie` (for an FDE, the index of its CIE
  among the section's records; `u32::MAX` for a CIE) and `code` (the
  FDE's code section). The CIE's identity is a `FragId`-like number
  computed after resolution by the same interning method (S-28) over
  live CIEs only; there are few (thousands), so this is a small table,
  not a per-record column.

### 4.3 Output order and the terminator

- **S-46 [C] (order).** The output records are, in `SectionId` order of
  the input `.eh_frame` sections (layout L-58) and record order within
  each, every live CIE that is kept (S-44) and every live FDE. A kept CIE
  therefore precedes every FDE that uses it unless an earlier file's FDE
  uses a CIE that a later file keeps, which cannot happen: the kept copy
  is the earliest (S-44), and an FDE uses a CIE of its own section in the
  input. This is GNU's order (X-10). lld groups each CIE with its FDEs;
  mold puts all CIEs first. No unwinder depends on the order (each FDE
  points to its CIE by offset; `.eh_frame_hdr` sorts), but the static
  start-up of `crtbeginT.o` walks from `__EH_FRAME_BEGIN__` (S-50), and
  input order keeps that walk covering what GNU's output covers.
- **S-47 [C] (packing).** Records are written back to back, with no
  padding between them, at offsets that are the sum of the preceding
  record sizes: input records are 4-byte multiples (X-24), so every
  output record is 4-aligned. The chunk's alignment is the largest input
  `.eh_frame` alignment (8 on the 64-bit targets, 4 on i386; X-24).
- **S-48 [G] (one terminator, last).** Input terminators are dropped, and
  the chunk ends with one 4-byte zero word. The terminator is required by
  libgcc's `__register_frame_info` walk (static start-up through
  `crtbeginT.o`), which stops at the first zero length. Dropping the
  input terminators and writing one at the end makes the result
  independent of where `crtend.o` sits on the command line: with
  `crtend.o` before the user's objects GNU's output has the terminator in
  the middle, and the program crashes in the unwinder; lld and mold write
  one at the end and the program runs (X-13). A link with no `crtend.o`
  (bare `ld`) gets a terminator too (lld and mold; GNU writes none,
  X-13). This changes layout L-58's sentence about `crtend.o` (§14).
- **S-49 [C] (Slots).** `Slots.eh_records` lists the output records in
  output order as `(SectionId, record index)`; the terminator is implied.
  The chunk's size is the sum of the listed records' sizes plus 4. Each
  record's output offset is a prefix sum over the list, computed in
  parallel by section (S-56).

### 4.4 References into .eh_frame

`.eh_frame` is referenced from inside (an FDE's CIE pointer), from
`crtbeginT.o`'s `frame_dummy` (a relocation against `__EH_FRAME_BEGIN__`,
a local symbol at offset 0 of `crtbeginT.o`'s `.eh_frame`), from
`.eh_frame_hdr`, and through `__EH_FRAME_BEGIN__`/`__FRAME_END__` in
`.symtab`.

- **S-50 [C] (a reference to a record).** A relocation or symbol whose
  target is input byte `r` of an `.eh_frame` section resolves to the
  record containing `r`:
  - a kept record: its output offset plus `r` minus its input offset;
  - a dropped record (a CIE de-duplicated away, a dead FDE, an input
    terminator), with `r` at the record's start: the output offset of the
    next kept record of the same input section, or if none, of the next
    kept record after it in output order, or the final terminator. This
    is the place the dropped record would have started had it been kept,
    and it is what GNU's relocation for `frame_dummy` computes (X-12): it
    makes `__register_frame_info(__EH_FRAME_BEGIN__)` walk the records
    that follow `crtbeginT.o`'s position to the terminator;
  - a dropped record with `r` inside it (not at its start): refused,
    `malformed-input`, `reference into a removed .eh_frame record`.
    Nothing but an FDE's own fields lies inside a record, and those are
    rewritten (S-52), not resolved.
  GNU's `.symtab` value for `__EH_FRAME_BEGIN__` in the same link is 16
  bytes before `.eh_frame` (a value in no section), while its relocation
  is right (X-12); lld gives both the start of `.eh_frame`; mold drops the
  symbol. mortar gives the symbol the value of its relocation: one rule
  for symbols and relocations.
- **S-51 [C] (the walk is complete enough).** With S-46, S-48 and S-50,
  the walk from `__EH_FRAME_BEGIN__` covers every live FDE of the files
  after `crtbeginT.o` on the command line (all user code and libraries,
  in every GCC driver's order), and misses the FDEs of `crt1.o` and
  `crti.o` before it (`_start`, which no exception unwinds through), as
  in GNU's output. Static C++ programs that throw run with all three
  references and must run with mortar (`tests/link/lang/cxx-exceptions`).

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
  FDE at M2 (lld writes none; GNU writes a CIE and FDE for x86-64's lazy
  `.plt`). gdb and libgcc unwind through a PLT entry from the caller's
  frame without one on all three targets. Q6 asks whether M2 adds them
  for perf's benefit. The chunk shape must allow linker-generated records
  appended after the inputs' (internal-file records, `FileId` 0, ordered
  last by S-46's rule applied with file 0 as last for `.eh_frame` only):
  `Slots.eh_records` already allows a record whose `SectionId` belongs to
  the internal file.

### 4.6 Cost

- **S-56 [C].** For 10^6 FDEs (about 40 MB of `.eh_frame` input): parsing
  is linear per section and parallel by section; liveness is a lookup per
  FDE; CIE identity interns only the CIEs (thousands). The output offset
  of each record is a per-section count followed by a prefix sum over
  sections: two parallel passes and one sequential pass over sections.
  `Slots.eh_records` is 8 bytes per record (8 MB). Writing is a copy plus
  three or four field rewrites per record, parallel by record range. The
  whole is a few tens of milliseconds on 8 threads; the largest cost is
  reading the input bytes.

## 5. .eh_frame_hdr

- **S-57 [C] (when).** `.eh_frame_hdr` and `PT_GNU_EH_FRAME` exist
  exactly when `--eh-frame-hdr` is given and some FDE is live. Without
  the option there is none, as with GNU and lld; mold always writes one
  (`tests/link/layout/eh-frame-hdr`). `--no-eh-frame-hdr` cancels an
  earlier `--eh-frame-hdr`; the last one wins. GCC's driver passes
  `--eh-frame-hdr` for every link except `-static` (on these Debian
  toolchains), so a static GCC link has no table and libgcc finds the
  FDEs through `__register_frame_info` (S-50) or through
  `_dl_find_object`/`dl_iterate_phdr` with a linear search.
- **S-58 [G] (format).** The LSB format: version 1; `eh_frame_ptr`
  encoding `0x1b` (`pcrel|sdata4`), `fde_count` encoding `0x03`
  (`udata4`), table encoding `0x3b` (`datarel|sdata4`); then
  `eh_frame_ptr` (to the start of `.eh_frame`), `fde_count`, and
  `fde_count` pairs `(initial location, FDE address)`, each relative to
  the start of `.eh_frame_hdr` (arch.md §3.9). Size: `12 + 8 ×
  fde_count`, known in phase 80 from the count of table entries (S-59).
  Alignment 4.
- **S-59 [C] (the entries).** One entry per live FDE, except:
  - an FDE with address range 0 has no entry (GNU; lld and mold keep it,
    X-15). It covers no address, and when it shares its initial location
    with a real FDE that follows it, an entry for it would hide the real
    one from the binary search;
  - of several FDEs with the same initial location, only the first in
    output order has an entry (lld; GNU fails the link with
    `.eh_frame_hdr refers to overlapping FDEs`; mold keeps all, so its
    search finds an arbitrary one, X-15).

  The count of entries is fixed in phase 80: initial locations are
  `(section, offset)` pairs before layout, and two FDEs share one exactly
  when they name the same section and offset (or the same folded target,
  model §4.1), which phase 80 can decide. FDEs of distinct sections
  never share an initial location, because distinct live sections never
  overlap (model §4.4 validity).
- **S-60 [C] (overlapping ranges).** FDEs whose ranges overlap without
  sharing an initial location are accepted and all have entries (lld and
  mold; GNU fails the link, X-15). A lookup for an address in the overlap
  finds the FDE with the greater initial location not above it, which is
  the libgcc binary search's answer whatever the linker does. Overlap
  arises only in hand-written assembly; the verifier reports it as an
  anomaly, not an error.
- **S-61 [G] (sorted).** The table is sorted by initial location,
  ascending, as signed 32-bit offsets from the header (which is the same
  as by address, since all code lies within ±2 GiB of the header on every
  M1 to M4 code model; layout L-36 refuses the others). The sort is over
  final addresses, so it is the image phase's work: the image phase
  writes the pairs and sorts them (10^6 entries: a parallel sort of
  8-byte keys, about 10 ms on 8 threads). The order of equal keys never
  matters, because S-59 left none.
- **S-62 [C] (an address the table cannot hold).** An offset that does
  not fit `sdata4` is refused: `relocation-out-of-range` against
  `.eh_frame_hdr`. With layout's placement (rank 5, before `.text`) it
  cannot occur below 2 GiB of code.

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
  - on x86-64 and i386: three reserved words (zero), then one word per
    IPLT entry. GNU and lld both keep the three words in a static link
    with only an IFUNC (X-22), and `_GLOBAL_OFFSET_TABLE_` (the x86 GOT
    base) is `.got.plt`'s start (layout L-101). Keeping them whenever
    `.got.plt` exists, rather than only when `GOT_BASE` is demanded,
    means a new `GOTOFF` reference in an incremental link moves nothing
    [M3]; x86's `crt1.o` and `crti.o` demand `GOT_BASE` in every driver
    link anyway (L-101);
  - on AArch64: one word per IPLT entry, no reserved words (a64.md §10.2;
    GNU and lld agree, X-22).

  Slot `n` of the IPLT is word `reserved + n`. A `.got.plt` with no
  entries and no reserved words does not exist.
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
  - RELA (AArch64, x86-64): `r_addend` the resolver's address, and the
    word itself written 0 (lld; GNU writes the PLT entry's address on
    AArch64 and 0 on x86-64; glibc overwrites the word, X-22);
  - REL (i386): the word itself holds the resolver's address, which is
    the implicit addend (GNU and lld, X-22).

  The `.rela.plt` chunk's size is `count × entsize`; its contents depend
  on addresses (image phase). `sh_link` is 0 in a static output (no
  `.dynsym`), `sh_info` the section index of `.got.plt` and `SHF_INFO_LINK`
  set (gABI: the section the relocations apply to). GNU writes exactly
  this on all three targets; lld writes `sh_info` 0 and no
  `SHF_INFO_LINK` on its `.rela.dyn` (X-22). Dynamic outputs use `.rela.plt` for `JUMP_SLOT` and put
  `IRELATIVE` there too, after the `JUMP_SLOT`s [M2].
- **S-70 [G] (`__rela_iplt_start`, `__rela_iplt_end`).** In a static
  output these hidden symbols (on i386 `__rel_iplt_start` and
  `__rel_iplt_end`) are the start and end of `.rela.plt`'s IRELATIVE
  entries; with no IFUNC they are equal (both at the address `.rela.plt`
  would have, layout §13). glibc's static start-up applies every entry
  between them. They are `STB_LOCAL` in `.symtab` (layout §13).
- **S-71 [C] (the IFUNC's address).** Every address of an IFUNC `f` with
  an IPLT entry is that entry's address (a64.md §10.4), including its
  `.symtab` value, typed `STT_FUNC` there.

### 6.3 Static TLS in the GOT

- **S-72 [C].** A static output's TLS GOT slots are `TpOff` slots holding
  the TP offset (IE not relaxed: `TLSIE_NORELAX`, arch.md §4.2) and, for
  GD or LD sequences that the architecture does not relax, the `TlsGd`
  and `TlsLd` pairs with module id 1 (the executable's, which is what
  glibc's static `__tls_get_addr` expects). AArch64 relaxes every
  TLSDESC in a static output (a64.md §10.6), so `TlsDesc` regions are
  empty at M1.

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
- **S-74 [C] (the local part).** After entry 0, the locals are in blocks,
  in this order:
  1. the **internal file's block** (`FileId` 0): the linker-defined
     symbols that are local in the output (layout §13: `__ehdr_start`,
     `__rela_iplt_start`, `_GLOBAL_OFFSET_TABLE_`, ...), in `SymbolId`
     order, then the thunk and patch symbols (S-80), in `Placement.patches`
     order. They precede every `STT_FILE`, so no tool scopes them to a
     file;
  2. for each input file in `FileId` order that contributes a local: the
     file's `STT_FILE` symbols and kept locals in the file's symbol table
     order, then the file's **demoted** symbols (S-77) in `SymbolId`
     order.

  This is lld's order (X-17: lld puts a demoted hidden symbol after its
  file's locals; GNU keeps hidden symbols global). A file with no kept
  local and no demoted symbol has no block.
- **S-75 [C] (the global part).** After the locals: every global that the
  output keeps (resolve.md §3.7, its output binding not `STB_LOCAL`), in
  `SymbolId` order. `STB_GLOBAL`, `STB_WEAK` and `STB_GNU_UNIQUE` keep
  their binding (GNU and lld keep `STB_GNU_UNIQUE` in a static output,
  X-17; resolve.md §3.7). mold makes every symbol local in a static
  output; GNU and lld do not (`tests/link/resolve/symtab`).
- **S-76 [C] (which locals).** A local of an input file is kept when:
  - it is not `STT_SECTION` (S-79);
  - its section is live (or it is `SHN_ABS`), and, if its section is
    split, its piece is kept (S-22, S-50);
  - it is not discarded by the `-X`/`-x` rule (S-78).

  Locals in sections that are not live are dropped (GNU's
  `--strip-discarded`, its default, and lld). Locals in a folded section
  [ICF] take the place of the fold's target.
- **S-77 [C] (demoted globals).** A global whose output binding is
  `STB_LOCAL` (a hidden or internal definition, resolve.md §3.7) is a
  local of its defining file's block. Its `st_other` keeps its
  visibility (`STV_HIDDEN`). A linker-defined symbol that is local in the
  output is in the internal file's block.
- **S-78 [C] (`-X`, `-x`, `--discard-none`).**
  - default, `-X` (`--discard-locals`): discard locals whose name begins
    with `.L` (the ELF temporary-label prefix; GNU and lld both drop
    them by default, mold keeps them; X-17). Mapping symbols (`$x`, `$d`,
    `$x.<n>`, `$d.<n>`) and `STT_FILE` are kept;
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
- **S-80 [T] (thunks and patches).** Each range-extension thunk and each
  erratum patch (layout §14) gets, in the internal file's block:
  - on AArch64, a `$x` mapping symbol at its start (AAELF64 "Mapping
    symbols"), so that objdump disassembles it as code; and a `$d` at
    any literal word inside it;
  - an `STT_FUNC` local named `__thunk_` followed by the target's name
    (and `+0x<addend>` in lower-case hex when the addend is not 0, and
    `@plt` for a `Plt` target), with `st_size` the thunk's size; an
    erratum patch is named `__erratum_843419`. gdb and perf then attribute
    samples and frames in a thunk to a named function rather than to the
    end of the preceding one; lld names its thunks
    `__AArch64ADRPThunk_<target>` and `__AArch64AbsLongThunk_<target>`
    for the same reason.
- **S-81 [G] (values).** `st_value` is the symbol's output address:
  `place(section) + st_value` for a symbol in a placed section; the S-19
  or S-50 value in a split section; `SHN_ABS` values unchanged; for
  `STT_TLS`, the offset from the start of the TLS template (`PT_TLS`'s
  `p_vaddr`), as all three write (X-17). A common symbol has the address
  of its `.bss` allocation (layout L-88) and is `STT_OBJECT` in `.bss`;
  `SHN_COMMON` never appears in an executable. An IFUNC with an IPLT
  entry is S-71. `st_size` is the input's.
- **S-82 [G] (section indices).** `st_shndx` is the output section index
  of the symbol's chunk (layout L-86), `SHN_ABS` for absolute symbols
  and linker-defined absolute symbols, `SHN_UNDEF` for an undefined weak.
  An index ≥ `SHN_LORESERVE` (0xff00) is written as `SHN_XINDEX`, with
  the real index in `.symtab_shndx` (S-85).
- **S-83 [C] (`STT_FILE`).** An input's `STT_FILE` entries are copied in
  place, so each starts its file's block, and a file with several (an
  object made by `ld -r`) keeps each in front of the locals that
  followed it in the input. gdb uses `STT_FILE` to scope static symbols
  of the same name in different files (layout L-155). A file with no
  `STT_FILE` and with kept locals gets none.
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
  "Extended Section Indices"). Whether it exists is known once the number
  of output sections is known (layout stage C), before content. The scan
  has no input with `SHN_XINDEX` (X-1), so this is reached only by an
  output with more than 65279 sections, which needs that many distinct
  orphan names.

### 7.3 .strtab

- **S-86 [C] (layout).** `.strtab` starts with a zero byte (the empty
  name), then a fixed table of common names, then one region per block
  of S-74 and one for the globals, in `.symtab` order. A name is written
  once per entry that uses it, except that names in the fixed table are
  shared: `$x`, `$d`, the empty string, `__erratum_843419`. No other
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
- **S-89 [C] (cost).** For 10^7 symbols (240 MB of `.symtab` on ELF64,
  and about 300 MB of names for a large C++ program): counting is one
  parallel pass over the kept symbols; filling is parallel by file and
  bound by memory bandwidth (about 0.1 s on 8 threads for 540 MB written).
  No table of 10^7 rows is built beyond the existing symbol columns: the
  output index of an entry is computed from its file's base and its
  position.

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
  in an `SHT_NOTE` allocated section with alignment 4. The descriptor
  size by style:

  | `--build-id=` | bytes | GNU | lld | mold |
  |---|---|---|---|---|
  | (none given, or `sha1`) | 20 | 20 | 20 | 20 |
  | `md5` | 16 | 16 | 16 | 16 |
  | `sha256` | 32 | unrecognized, ignored with a warning | refused | 32 |
  | `fast` | 8 | unrecognized | 8 | 32 |
  | `uuid` | 16 | 16 | 16 | 16 |
  | `0xHEX` | the hex bytes | same | same | same |
  | `none` | no note | no note | no note | no note |

  The section's size is `16 + n` with no padding after the descriptor
  (all three: a 3-byte `0xabcdef` gives `0x13`, X-21). `--build-id` alone
  is `sha1` (`tests/link/layout/build-id`). An odd number of hex digits,
  or none, is `invalid-option-value`.
- **S-92 [C] (content).** The descriptor is a hash of the output file
  with the descriptor's bytes zero, computed after every other byte is
  written (the sealing step; `spec/image.md` owns the hash and its
  order of input bytes); `0xHEX` is copied; `uuid` is 16 random bytes
  (RFC 4122 version 4). `uuid` is the one output byte range that is not
  a function of the inputs (model §6): the user asked for it. `fast` is
  an 8-byte non-cryptographic hash (lld's size); its function is
  `image.md`'s.
- **S-93 [G] (position).** The note is in the first `PT_LOAD` (layout
  rank 2), so that a core dump, which contains the first page of each
  mapping, contains it; `PT_NOTE` covers it (layout §11).

### 8.2 .note.gnu.property

- **S-94 [G] (inputs).** An input `.note.gnu.property` holds one note,
  name `GNU`, type `NT_GNU_PROPERTY_TYPE_0` (5), whose descriptor is a
  sequence of `(pr_type u32, pr_datasz u32, data)` padded to 8 on ELF64
  and 4 on ELF32 (gABI "Program Property", the Linux extensions). A
  property whose `pr_datasz` runs past the descriptor, or whose datasz
  is not 4 for the 32-bit-mask types below, is `malformed-input`
  (`program property is too short`, lld's wording; GNU warns `corrupt
  GNU_PROPERTY_TYPE`, mold crashes, X-19). Properties in an input are in
  ascending `pr_type` order; an input out of order is accepted and sorted.
- **S-95 [G] (merge rules).** The output property of each type, over all
  live input objects (an object without a property note has every
  property absent), by `arch.property_merge(pr_type)`:
  - **`And`**: present with value the AND of every input's value, an
    absent input counting 0; omitted if the result is 0. AArch64
    `GNU_PROPERTY_AARCH64_FEATURE_1_AND` (0xc0000000: BTI 1, PAC 2,
    GCS 4) and x86 `GNU_PROPERTY_X86_FEATURE_1_AND` (0xc0000002: IBT 1,
    SHSTK 2). GNU and lld agree (X-19: `3 & 1 = 1`, `3 & 7 = 3`, an object
    without a note removes the property; X-20); mold writes no AArch64
    property;
  - **`Or`**: present with value the OR of the inputs that have it, if
    any input has it; omitted if none does. x86
    `GNU_PROPERTY_X86_ISA_1_NEEDED` (0xc0008002) and the other
    `0xc0008000..0xc0008fff` "needed" types. GNU and mold agree (X-20:
    baseline OR v2 gives both; an input without the property does not
    remove it); lld drops `ISA_1_NEEDED` altogether;
  - **`OrAll`** (new, §14): present with value the OR of the inputs, only
    if every input has it. x86 `GNU_PROPERTY_X86_ISA_1_USED`
    (0xc0010002), `GNU_PROPERTY_X86_FEATURE_2_USED` (0xc0010001) and the
    other `0xc0010000..0xc0017fff` "used" types: GNU and mold drop the
    property when any input lacks it (X-20). arch.md §3.13 calls
    `FEATURE_2_USED` `Or`, which is wrong by this experiment;
  - a type the architecture does not know is refused (`feature`, the hex
    `pr_type`; a64.md Q13). GNU warns `unsupported GNU_PROPERTY_TYPE`
    and copies it; lld refuses `0xc0000001` on AArch64
    (`GNU_PROPERTY_AARCH64_FEATURE_PAUTH`, which needs a matching ABI
    tag; X-19). `GNU_PROPERTY_STACK_SIZE` (1) and
    `GNU_PROPERTY_NO_COPY_ON_PROTECTED` (2) are known generic types:
    `STACK_SIZE` is the maximum, `NO_COPY_ON_PROTECTED` is `OrAll`
    [M2]. The scan has only `0xc0000000` (AArch64, values 3 and 7),
    `0xc0000002` (value 3) and `0xc0008002` (value 1) on x86-64, and no
    properties on i386 (X-1).
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
  inputs have different alignments is refused: `feature`, `notes of
  different alignment in one output section`. A reader walks a note
  section with one alignment (`PT_NOTE`'s `p_align`, or the section's),
  and a 4-aligned note whose name size is not a multiple of 8 then
  misparses: all three references put a 4-aligned and an 8-aligned
  `.note.foo` in one 8-aligned section and readelf reports
  `<corrupt - note is too small>` (X-23). No input in the scan mixes
  alignments within one name (X-1: `.note.gnu.property` 8, `.note.ABI-tag`
  4, `.note.package` 4, `.note.stapsdt` 4).

## 9. .comment

- **S-100 [C].** `.comment` is a `Merged` chunk (`SHF_MERGE|SHF_STRINGS`,
  entsize 1, non-allocated) of the inputs' `.comment` strings,
  de-duplicated by §3, plus one piece of the internal file: the string
  `mortar <version>`, where `<version>` is the crate version
  (`MAJOR.MINOR.PATCH` from the package manifest), with no commit hash,
  date or host. The internal file has `FileId` 0, so its piece is the
  first fragment (S-15) and comes first. The section exists in every
  output (lld and mold always add their identification:
  `Linker: Debian LLD 19.1.7`, `mold 2.37.1 (compatible with GNU ld)`;
  GNU adds nothing, X-9). It makes a binary's linker identifiable by
  `readelf -p .comment`, which bug reports and the differential harness
  use. The string is a function of the linker's version only, so output
  stays deterministic across hosts and runs.

## 10. Dynamic outputs (M2 outline)

This section names what M2 adds and the decisions that constrain the M1
shapes. It is not the M2 specification.

- **S-101 [M2] (`.dynsym`).** The null entry, then the local section
  symbols needed by dynamic relocations (none: mortar writes no
  `STT_SECTION` dynamic symbols, as lld), then the exported and imported
  symbols in `Slots.dynsym` order. With `--hash-style=gnu`, the symbols
  that the hash covers come after those it does not, sorted by bucket
  (`hash % nbuckets`), and within a bucket by `SymbolId` (a total,
  deterministic order). `.dynsym`'s `sh_info` is 1 (no locals but the
  null entry). The number of buckets is a function of the count only.
- **S-102 [M2] (`.gnu.hash`).** The GNU hash table: `nbuckets`,
  `symoffset`, the Bloom filter (size a power of two, a function of the
  count), the buckets and the chain. `--hash-style=sysv` and `both` are
  refused at M2 (`option-value`), as `spec/cli.md` already says;
  `DT_HASH` is not written.
- **S-103 [M2] (versions).** `.gnu.version` one `u16` per `.dynsym`
  entry; `.gnu.version_d` from the version script; `.gnu.version_r` per
  needed shared object in `Slots.needed` order, versions in the order of
  first reference by `SymbolId`.
- **S-104 [M2] (`.rela.dyn`).** Relative relocations first, sorted by
  `r_offset`, then the others sorted by (symbol index, `r_offset`); the
  count of relative relocations is `DT_RELACOUNT` (`DT_RELCOUNT` on
  i386). With `-z pack-relative-relocs` the relative relocations that
  qualify (arch.md §3.10) go to `.relr.dyn` (a sized chunk, layout L-121)
  and the rest stay. `IRELATIVE` relocations of a dynamic output go to
  `.rela.plt` after the `JUMP_SLOT`s (S-69). The relocation *set* is a
  function of `Demands` and `Slots`; its order is a sort, so it is
  independent of the scan's thread count.
- **S-105 [M2] (`.dynamic`).** Entries in a fixed tag order (`DT_NEEDED`
  in `Slots.needed` order, `DT_SONAME`, `DT_RUNPATH`, the table
  addresses and sizes, `DT_FLAGS`, `DT_FLAGS_1`, `DT_DEBUG` for
  executables, `DT_NULL`); its size is a function of the tags present,
  known before addresses.
- **S-106 [M2] (`.interp`).** The `--dynamic-linker` path, or the
  target's default, NUL-terminated; first in the first `PT_LOAD`.
- **S-107 [C] (what M1 must not preclude).**
  - the `.got` regions of S-63 already have a place for the header word
    with `_DYNAMIC` (S-65);
  - `.got.plt` already puts IPLT words after PLT words (S-67), and
    `.rela.plt` IRELATIVE after `JUMP_SLOT`s (S-69), so the static shapes
    are the dynamic ones with zero PLT entries;
  - `Slots.dynsym` and `.symtab` are independent: `.symtab` never takes
    its order from `.dynsym`;
  - every synthetic chunk of S-3 except `.relr.dyn` has its size before
    addresses, including `.gnu.hash` (its sizes depend on counts) and
    `.rela.dyn` (one entry per demand); M2 must keep that property.

## 11. Sizes as data, units and incremental linking

### 11.1 Sizes

- **S-108 [C].** `Slots.sizes` lists, for every synthetic chunk of S-3
  except `.relr.dyn` and the address-dependent cases of S-2, its size:

  | chunk | size |
  |---|---|
  | `.note.gnu.property` | `16 + Σ(8 + align_up(datasz, a))` over surviving properties, a = 8 (4) |
  | `.note.gnu.build-id` | `16 + n` (S-91) |
  | `.rela.plt` | `(PLT + IPLT entries) × entsize` |
  | `.eh_frame_hdr` | `12 + 8 × entries` (S-59) |
  | `.eh_frame` | Σ live record sizes + 4 |
  | `.plt` | `PltShape` header + entries + `iplt_entry × IPLT` |
  | `.got` | Σ region capacities × slot size (S-63) |
  | `.got.plt` | `(reserved + PLT + IPLT) × word` |
  | `.symtab` | `entsize × (1 + locals + globals)` (S-88) |
  | `.symtab_shndx` | `4 × .symtab entries` |
  | `.strtab` | `1 + fixed table + Σ name bytes + 1 per entry` |
  | `.shstrtab` | `1 + Σ (name + 1)` over output sections |

  Each is a function of counts. `Merged` chunk sizes come from
  `MergeMap` (S-24). None needs an address.
- **S-109 [C] (no cost by default).** A non-incremental link computes
  exactly these sizes and no capacity: every capacity equals its size
  (model §4.4). Nothing in this spec is kept only for incremental links
  in a full link's memory or time, except S-65's header word under an
  incremental policy, which a full link does not have.

### 11.2 What an incremental link redoes [M7]

The rule for all of them is model §7.2: an incremental link recomputes
the decision tables that changed, keeps the rest, and refills whole units
through the same `fill`. The table says, per synthetic chunk, what one
edit changes.

- **S-110 [M7] (a string added or removed).** A new fragment is
  appended to its `Merged` chunk in the capacity (layout L-147) with the
  next `FragId` after the stored ones; its referrers are the edited
  file's units, which are refilled anyway. A fragment no longer used
  keeps its bytes (a valid piece). No other unit changes. A full link
  given these tables produces the same bytes (model §7.3) because
  `MergeMap` is a decision table, stored in identity terms (fragment =
  output section and content hash, model §7.1).
- **S-111 [M7] (an FDE added or removed).** The edited file's records
  are a contiguous range of `.eh_frame` (S-46); the range is refilled.
  When it grows past its capacity, `.eh_frame` grows by layout L-144;
  records after it move, and the units that refer to them (the FDEs'
  CIE pointers of moved FDEs are in the moved records themselves; the
  hdr table) are refilled. A CIE that becomes unused is kept in place; a
  CIE needed for the first time is appended in the edited file's range.
  `.eh_frame_hdr` is one unit, refilled whenever any FDE's address
  changes (a sort of 10^6 entries, about 10 ms); its capacity covers a
  growth in entries. Per-file capacity in `.eh_frame` is a policy
  decision of `spec/incr.md`; the shape allows it because records are
  listed in `Slots.eh_records` (a decision table) and a hole is
  expressible as a padding CIE no FDE uses (a valid CIE with no FDE,
  S-40).
- **S-112 [M7] (a symbol added or removed).** `.symtab`, `.strtab` and
  `.symtab_shndx` are rewritten whole: they are non-allocated and last
  (layout rank 10), so their size changes no address and no other unit.
  Rewriting 10^7 symbols is about 0.1 s (S-89). A per-file capacity in
  `.symtab` (with null local entries as holes) is possible and is not
  done unless the bench shows the rewrite matters; Q8 records it.
- **S-113 [M7] (a GOT or IPLT slot added or removed).** A new slot is
  appended to its kind's region in the region's capacity, with the next
  index; a removed slot stays (its word is written 0, no relocation
  refers to it). A new IPLT entry appends to `.plt`, `.got.plt` and
  `.rela.plt`. `.rela.plt` is one unit (a few entries) refilled whole,
  with no holes: a hole would be an `IRELATIVE` that glibc applies, so a
  removed entry's relocation is removed and the chunk compacted, while
  its `.plt` and `.got.plt` words stay as dead slots. `__rela_iplt_end`
  moves with the count, which changes only `.rela.plt`'s referrer (the
  start-up code's unit, refilled through the dependency index).
- **S-114 [M7] (properties, build id, `.comment`).** `.note.gnu.property`
  is recomputed whole when any input's notes change; if its size
  changes it grows within capacity, and if the merged value changes the
  PLT variant may change, which refills every PLT entry. The build id is
  recomputed over the final image on every link. `.comment` is a merged
  chunk and follows S-110.
- **S-115 [C] (fixed now for M7).**
  - every order in this spec is by ids (`FragId`, `SymKey`,
    `SectionId`), never by hash or address, so a stored table can be
    extended by appending with the next id;
  - the GOT has per-kind regions with capacities (S-63), not a single
    interleaved order;
  - `Slots.eh_records` lists records explicitly (a table, not a rule),
    so that an incremental link can keep a record's position;
  - `.symtab` is last and non-allocated, so it never forces relayout;
  - the AArch64 GOT header word is a policy, not only a demand (S-65);
  - x86 `.got.plt` always has its reserved words (S-67).

## 12. What tools rely on

- **S-116 [T] (gdb, lldb).** Use `.symtab` for function names when there
  is no DWARF and `STT_FILE` to scope statics (S-83); unwind with
  `.eh_frame` (and `.debug_frame`) through the CIE/FDE format, and use
  `.eh_frame_hdr` when present but do not need it; read `.debug_str`
  through `DW_FORM_strp` offsets, which S-17 keeps exact. gdb's `info
  symbol` on a thunk names it by S-80.
- **S-117 [T] (perf).** Symbolizes with `.symtab`; needs `st_size` on
  functions; reads the build id (S-91) to find debug files; unwinds with
  `.eh_frame_hdr` and `.eh_frame` (`--call-graph=dwarf`), so a dynamic
  output should have the header (compiler drivers pass
  `--eh-frame-hdr`).
- **S-118 [T] (objdump, nm, addr2line).** objdump uses AArch64 mapping
  symbols to decide code from data (S-78, S-80); nm lists `.symtab`;
  addr2line uses DWARF and falls back to `.symtab`.
- **S-119 [T] (valgrind).** Reads `.symtab`, `.eh_frame` (it requires
  well-formed CIEs and FDEs, S-34 to S-39) and the build id; it needs no
  `.eh_frame_hdr`.
- **S-120 [T] (strip, objcopy).** `strip` removes `.symtab` and
  `.strtab` and rewrites section indices; it requires that `.symtab`
  follow S-73 (locals first, `sh_info` right), which eu-elflint also
  checks.

## 13. Where the references differ

| point | GNU | lld | mold | mortar | why |
|---|---|---|---|---|---|
| merge order | first occurrence (with tail merge) | shard, then hash | hash | first occurrence, no tail merge (S-24) | stable under edits; GNU-like |
| tail merging | always | `-O2` | never | never (S-23) | measured gain small; cost and incrementality |
| `-O0` | merges | does not merge | merges | merges (S-32) | nothing depends |
| section symbol + A at `sh_size` | end of output section, silently | error | end of last piece | refused (S-18) | GNU's value is wrong |
| section symbol + A outside | warning, end of output section | error | silent garbage | refused (S-18) | |
| unterminated string section | copied | error | error | refused (S-8) | |
| `.eh_frame` order | input, CIE at first use | CIE then its FDEs | CIEs, then FDEs | input (S-46) | GNU; keeps crtbeginT's walk |
| input terminators | kept in place | dropped, one at end | dropped, one at end | dropped, one at end (S-48) | crtend.o position independent |
| no `crtend.o` | no terminator | terminator | terminator | terminator (S-48) | |
| `__EH_FRAME_BEGIN__` in `.symtab` | 16 bytes before `.eh_frame` | `.eh_frame` start | dropped | its relocation's value (S-50) | one rule |
| CIE version 2, 4; unknown augmentation | warning, no hdr | error | accepted | refused (S-36) | |
| truncated record, bad CIE pointer, data after terminator | warning, no hdr | error | crash or accepted | refused (S-39) | |
| FDE without pc relocation | kept, no hdr | dropped | dropped | dropped (S-38) | |
| FDEs with the same start | link fails | one entry | all entries | one entry (S-59) | |
| zero-length FDE | no entry | entry | entry | no entry (S-59) | it would shadow |
| overlapping FDEs | link fails | accepted | accepted | accepted (S-60) | |
| `.eh_frame_hdr` without the option | none | none | always | none (S-57) | |
| static IPLT sections | `.plt`, `.rela.plt`, `.got.plt` | `.iplt`, `.rela.dyn`, `.got.plt` | `.plt.got`, `.rela.dyn` | GNU's (S-68) | |
| IRELATIVE word (RELA) | AArch64: PLT address; x86-64: 0 | 0 | — | 0 (S-69) | glibc overwrites it |
| x86 static `.got.plt` reserved words | 3 | 3 | — | 3 (S-67) | |
| section symbols in `.symtab` | AArch64 yes, x86 no | none | all | none (S-79) | |
| `.L` locals | dropped | dropped | kept | dropped (S-78) | |
| hidden definitions | `STB_GLOBAL STV_HIDDEN` | local, in its file's block | local | local, in its file's block (S-77) | resolve.md §3.7 |
| globals in a static executable | global | global | local | global (S-75) | |
| `.strtab` de-duplication | full, tail merged | almost none | none | fixed table only (S-86) | |
| `.comment` | inputs only | adds `Linker: LLD` | adds `mold` | adds `mortar <version>` (S-100) | |
| `--build-id=sha256` | not recognized | refused | 32 bytes | 32 bytes (S-91) | |
| `--build-id=fast` | not recognized | 8 bytes | 32 bytes | 8 bytes (S-91) | |
| AArch64 `.note.gnu.property` | AND | AND | none written | AND (S-95) | BTI/PAC enforcement |
| x86 `ISA_1_NEEDED` | OR | dropped | OR | OR (S-95) | |
| x86 `*_USED` | OR if all have it | — | OR if all have it | OR if all have it (S-95) | |
| unknown property type | warning, copied | error (PAUTH) | ignored | refused (S-95) | |
| notes of mixed alignment in one section | corrupt output | corrupt output | corrupt output | refused (S-99) | |

## 14. Requested changes to other specs

Each request gives the section, the current text, the proposed text, and
the reason. They are written to be applied mechanically.

### 14.1 model.md

**R-M1. §3.5, the identity of a fragment.**
Current: "Two pieces have the same `FragId` exactly when they belong to
one output section and have equal bytes and alignment."
Proposed: "Two pieces have the same `FragId` exactly when they belong to
one output section and have the same merge class (`SHF_STRINGS` and
`sh_entsize`), equal alignment and equal bytes (`spec/synth.md` S-13)."
Reason: a constant and a string with equal bytes are cut and referred
to differently (S-9, S-10), so they must not share a fragment; two
alignments of one string stay apart in all three references (X-7).

**R-M2. §4.2, the reference to `spec/merge.md`.**
Current: "The default order is by `FragId`, or by a content key
`spec/merge.md` gives (tail merging); never by a hash table's layout
(§6.5)."
Proposed: "The order is by merge class, then alignment, then `FragId`
(`spec/synth.md` S-24); never by a hash table's layout (§6.5). Fragments
may overlap only if tail merging is added (`spec/synth.md` S-23, Q1)."
Reason: there is no `merge.md`; synth.md owns merging, and does not
tail-merge.

**R-M3. §4.3, the `.symtab` order.**
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
Current: "`ChunkId`s are given in a fixed creation order: headers, then
synthetic chunks in the order `spec/synth.md` lists them, then merged
chunks and output-section bodies in `OutSecId` order."
Proposed: append "The list is `spec/synth.md` S-3; `.eh_frame` is one of
its synthetic chunks (its input sections are split, not placed, and have
no `place`)."
Reason: makes explicit that `.eh_frame` is synthesized from
`Slots.eh_records`, not a body.

**R-M6. §3.5, the internal file's pieces.**
Current: (no text)
Proposed: add after the `EhFrame` bullet: "- the internal file (`FileId`
0) may contribute pieces: the `.comment` string of `spec/synth.md`
S-100, and at M2 linker-generated `.eh_frame` records (S-55)."
Reason: S-100 puts mortar's identification through the ordinary merge
path rather than a new chunk kind.

### 14.2 layout.md

**R-L1. L-58.**
Current: "The terminator that `crtend.o` provides (a zero-length CIE)
must stay last, and it does, because `crtend.o` comes after the user's
objects in every driver's line."
Proposed: "Input terminators (zero-length records, which `crtend.o`
provides) are dropped, and `spec/synth.md` writes one terminator at the
end of `.eh_frame` (S-48), wherever `crtend.o` is on the line."
Reason: X-13: with `crtend.o` early GNU's output crashes; lld and mold
write the terminator last and the program runs.

**R-L2. §20.6, replace the request list with its resolution.**
Current: the four bullets under "### 20.6 `synth.md` (not yet written)".
Proposed: "### 20.6 `synth.md` — applied: `.relro_padding` is a layout
chunk (synth.md does not list it, S-3); `.eh_frame` keeps input order
(S-46) and its type follows L-49 (S-54); the static IPLT is in `.plt`,
`.rela.plt` and `.got.plt` (S-68); `.note.gnu.property` is merged by
S-95, which keeps the AND rule and adds `Or` and `OrAll` for x86."
Reason: the requests are answered.

**R-L3. §8.1 (sections that go nowhere), `-S`.**
Current: (no rule for `-S`/`--strip-debug`)
Proposed: add "- **L-n [C]** With `-S` (`--strip-debug`) or `-s`, the
`.debug_*` sections and `.gdb_index` go nowhere.
`.comment` and non-allocated notes are kept. (`spec/synth.md` S-84.)"
Reason: S-84 needs the rule to exist in the spec that decides which
sections are output.

**R-L4. §12.1 L-85, the merged chunk's order.**
Current: "ordered by class (strings before constants, then entsize
ascending), then by `MergeMap` order."
Proposed: "ordered by class (strings before constants, then entsize
ascending), then by alignment ascending, then by `MergeMap` order
(`spec/synth.md` S-24, S-25)."
Reason: S-25.

**R-L5. §14.3, symbol-table sizes after the fixed point.**
Current: L-121 lists `.relr.dyn`, patch islands and compressed non-alloc
chunks as sized chunks.
Proposed: add to L-121: "`.symtab`, `.strtab` and `.symtab_shndx` are
sized after the fixed point when thunks or patches exist, since each adds
symbols (`spec/synth.md` S-80); they are non-allocated and after every
allocated chunk, so their size moves no address."
Reason: S-2, S-80.

### 14.3 arch.md

**R-A1. §3.13, the `Or` rule and a new `OrAll` rule.**
Current: "`property_merge(pr_type)` is `And` (an input without the
property counts as 0; AArch64 `FEATURE_1_AND`, x86 `FEATURE_1_AND`),
`Or` (an input without it counts as 0; x86 `ISA_1_NEEDED`,
`FEATURE_2_USED`) or `None` for a type the architecture does not know,
which is refused by name."
Proposed: "`property_merge(pr_type)` is `And` (an input without the
property counts as 0; the property is omitted when the result is 0;
AArch64 `FEATURE_1_AND`, x86 `FEATURE_1_AND`), `Or` (the OR of the inputs
that have it, present if any has it; x86 `ISA_1_NEEDED` and the other
`0xc0008000..0xc0008fff` types), `OrAll` (the OR of the inputs, present
only if every input has it; x86 `ISA_1_USED`, `FEATURE_2_USED` and the
other `0xc0010000..0xc0017fff` types) or `None` for a type the
architecture does not know, which is refused by name (`spec/synth.md`
S-95)."
Reason: X-20: GNU and mold drop a `*_USED` property when one input lacks
it, and keep `ISA_1_NEEDED` when one input lacks it.

**R-A2. §3.1, `Facts.got_plt_reserved`.**
Current: "`got_plt_reserved: u8, // words reserved at the start of
.got.plt with .dynamic`"
Proposed: "`got_plt_reserved: u8, // words reserved at the start of
.got.plt: with .dynamic on every target; also in static outputs on x86
(synth.md S-67)`"
Reason: X-22. layout.md §20.5 requests the same fact for static x86
links "when `GOT_BASE` is demanded"; x86's `crt1.o` and `crti.o` always
demand it (layout L-101), so the two requests agree, and this wording
also covers a link without the crt files.

### 14.4 a64.md

**R-A64-1. §13.1, the personality encoding.**
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

### 14.5 plumb.md

**R-P1. §6, new structure rules (stage 2).** Add:
- "**S13 (.eh_frame).** `.eh_frame` parses as records (synth.md S-34)
  ending with exactly one terminator, at its end; every FDE's CIE
  pointer lands on a CIE of the section; every FDE's initial location
  lies in an executable section of the output."
- "**S14 (.eh_frame_hdr).** When present: version 1, the encodings of
  arch.md §3.9, `eh_frame_ptr` equal to `.eh_frame`'s address,
  `fde_count` entries strictly increasing by initial location, each
  pointing at an FDE whose initial location equals the entry's, and no
  live FDE with a non-zero range missing from the table unless another
  FDE with the same initial location precedes it."
- "**S15 (.symtab).** Entry 0 zero; every `STB_LOCAL` entry before every
  other; `sh_info` one past the last local; every `st_shndx` that is not
  reserved names an existing section and `st_value` lies in it (or at its
  end) unless `STT_TLS`; `SHN_XINDEX` entries have a `.symtab_shndx`."
- "**S16 (.note.gnu.property).** The output's properties equal the merge
  of the inputs' by synth.md S-95."
- "**S17 (IRELATIVE).** In a static output, `__rela_iplt_start` and
  `__rela_iplt_end` (or `__rel_iplt_*`) bound exactly the `IRELATIVE`
  entries of `.rela.plt`, and each entry's `r_offset` is a `.got.plt`
  word."
Reason: these are the [G] rules of this spec; plumb.md lists
`.eh_frame` as stage 2.

**R-P2. §5.6 step 1, out-of-range section references.**
Current: "The input offset is r = `st_value + A` for a section symbol,
with extra = 0."
Proposed: append "A section-symbol reference with r outside `[0,
sh_size)` is an anomaly on a reference output (GNU writes the end of the
output section) and cannot occur in mortar's (synth.md S-18)."
Reason: S-18.

**R-P3. Mutations (§9.2).** Add: drop the `.eh_frame` terminator; move it
to the middle; swap two `.eh_frame_hdr` entries; point an FDE's CIE
pointer 4 bytes off; put a global before a local in `.symtab`; set
`sh_info` one too small; set a property bit not in every input's AND.

### 14.6 diag.md

**R-D1. §3.1, no new kinds.** The malformed `.eh_frame`, merge and note
cases use `malformed-input` with the messages of S-7, S-8, S-18, S-36,
S-39, S-50 and S-94; the refusals use `feature` (S-12, S-21, S-34, S-37,
S-99) and the hex `pr_type` (S-95). Add to §3.2: "The `malformed-input`
messages of `spec/synth.md` are lld's wording where lld has one: `string
is not null terminated`, `offset is outside the section`, `CIE/FDE ends
past the end of the section`, `invalid CIE reference`, `FDE version 1 or
3 expected, but got N`, `unknown .eh_frame augmentation string: S`,
`program property is too short`."
Reason: one wording per case, tested.

### 14.7 cli.md and docs/status/options.md

**R-C1.** Status changes when WP-025's code lands (M1): `-s`, `-S`,
`--strip-discarded`, `-x`, `-X`, `--discard-none` (S-78, S-84),
`--build-id[=STYLE]` with every style of S-91, `--eh-frame-hdr`,
`--no-eh-frame-hdr` (S-57) become "implemented"; `-O LEVEL` becomes
"accepted, no effect" (S-32), with `plan.shaping.optimize` not stored.
`--build-id=STYLE` with an unknown style is `invalid-option-value`
(GNU warns and ignores; lld refuses).
Reason: these are the options this spec defines.

**R-C2. §3.8.** Current: "`strip` (`-s`, `-S`, `--strip-*`);" Proposed:
"`strip` (`-s`, `-S`, `--strip-*`; the last wins, `--strip-discarded`
is accepted and has no effect, `spec/synth.md` S-84);" and current
"`discard` (`-x`, `-X`, `--discard-*`);" proposed "`discard` (`-x`,
`-X`, `--discard-none`; the last wins, default `-X`, S-78);".
Reason: S-78, S-84.

## 15. Open questions

Ranked by how much depends on each.

| # | question | default taken | what depends |
|---|---|---|---|
| Q1 | Tail merging behind `-O2` for `.debug_str`-class sections (13% of `.debug_str` measured, X-8). | none | output size of debug builds; `MergeMap` validity already allows it |
| Q2 | Piece-level liveness of merge pieces under `--gc-sections` (lld marks pieces; GNU keeps whole sections). | `spec/live.md` decides; S-16 keeps all pieces without gc | size of `.rodata` with gc; symbol drops of S-22 |
| Q3 | Whether a reference into a removed `.eh_frame` record other than at its start (S-50 refusal) ever occurs (crtstuff variants, other libcs). | refused | static C++ links with other crt files |
| Q4 | `--build-id=fast`: 8 bytes (lld) or 32 (mold), and its function. | 8 bytes, image.md's hash | build-id consumers only see the size |
| Q5 | `.eh_frame_hdr` for `-static` links: GCC's driver does not pass `--eh-frame-hdr`; adding it by default would speed up static unwinding (mold does). | not by default | static C++ throw performance |
| Q6 | PLT and thunk unwind info (FDEs for `.plt`, thunks) for perf. | none (lld) | perf's unwinding through PLTs [M2] |
| Q7 | The `.comment` string's exact form (`mortar X.Y.Z` against `Linker: mortar X.Y.Z`, lld's style). | `mortar X.Y.Z` | tools that grep `.comment` |
| Q8 | Per-file capacity in `.symtab` for incremental links. | rewrite whole (S-112) | M7 time for 10^7-symbol programs |
| Q9 | `GNU_PROPERTY_AARCH64_FEATURE_PAUTH` (0xc0000001) and GCS-only inputs. | refused / AND | AArch64 pointer-authentication ABI binaries |

## 16. Tests

Existing link tests cite these sections: ".symtab" (§7.1,
`tests/link/resolve/symtab`), ".eh_frame_hdr" (§5,
`tests/link/layout/eh-frame-hdr`), ".note.gnu.build-id" (§8.1,
`tests/link/layout/build-id`), ".eh_frame" (§4,
`tests/link/lang/cxx-exceptions`), "IRELATIVE" (§6.2,
`tests/link/ifunc/irelative`). This draft adds no tests (WP-025 stage 2).
The plan per family:

- **merge** (`tests/link/merge/`): piece mapping of S-17 and S-19 with a
  string referred to by section symbol, by local symbol with addends
  −1, +len, and crossing (the X-2, X-3 inputs); S-18 refusal at
  `sh_size`, beyond and negative (`refused` by message); S-8; S-11 with
  `.rodata.str1.8`; S-13 (`abc` in `str1.1`, `str1.8`, `cst4`); S-24
  order (strings appear in first-occurrence order, checked by
  `readelf -p`); S-32 (`-O0` output equals `-O2`); S-6 (a writable
  `SHF_MERGE` section placed whole). Each runs against all references
  with `skip` where §13 says they differ.
- **eh_frame** (`tests/link/eh/`): the X-14 matrix as one test per row
  (accepted rows check the program unwinds through `_Unwind_Backtrace`;
  refused rows check the message); X-13 (crtend first, program throws
  and catches); X-12 (`__EH_FRAME_BEGIN__` value equals the
  `__register_frame_info` argument, read by gdb); X-11 (two personality
  CIEs survive); the X-15 hdr cases.
- **symtab** (`tests/link/symtab/`): S-74 order (`readelf -s`: locals
  per file, demoted hidden after its file's locals); S-78 for each of
  `-X`, `-x`, `--discard-none`; S-79 (no `SECTION` entries); S-81 TLS
  value; S-80 a thunk symbol in a link with a far call; S-85 with a
  generated object of 65300 sections (marked slow).
- **notes** (`tests/link/notes/`): the X-19 and X-20 property matrices;
  S-99 refusal; S-91 sizes for every style; S-100 `.comment` string.
- **GOT/IPLT** (`tests/link/ifunc/`, `tests/link/tls/`): S-67 to S-70 per
  architecture (sections, words, `readelf -r`), run under qemu for x86.
- **verifier**: the plumb S13 to S17 rules (R-P1) with the mutations of
  R-P3; every link test's output passes them.
- **unit tests** (`mortar_merge`, `mortar_synth`): S-28 determinism (the
  same `Split` and `MergeMap` with 1, 2, 8 and 64 threads, and with the
  input files' contents permuted among equal names); S-46 and S-49 on
  synthetic record lists; S-88 symbol counts against a model.
