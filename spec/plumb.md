# `mortar_plumb`: the independent verifier

`mortar_plumb` takes a finished link and works out from the specifications
what the output must contain. It then reports every place where the
output differs. It checks mortar's links. It is also checked against the
links of GNU ld, lld and mold, which must verify clean (§9.1).

The verifier is independent of the linker. It shares only the ELF reader
(`mortar_elf`) and the diagnostic type (`mortar_diag`) with mortar. It is
written from public documents:

- the System V gABI, by chapter name ("ELF Header", "Program Header",
  "Sections", "Symbol Table", "Relocation", "Note Section");
- AAELF64, *ELF for the Arm 64-bit Architecture*, by section name
  ("Relocation operations", "Static AArch64 relocations", "Relocations for
  thread-local storage");
- the *Arm A64 Instruction Set Architecture* encodings;
- Arm's description of Cortex-A53 erratum 843419;
- the observed behavior of the reference linkers, recorded in §10.

The linker's code and the reference linkers' sources are not read
(`docs/AGENTS.md` rule 10). The output is deterministic (§7.1).

Stage 1 covers `-static` executables that are not PIE, on AArch64. §4.5
describes how x86-64 and i386 fit.

## 1. Scope and the stages

| stage | covers | not-checked reasons it removes |
|---|---|---|
| 1 | `ET_EXEC`, static, non-PIE, AArch64. Placement, relocations (the §4.2 table), the GOT slots of a static link, structure (§6), content, symbol values | — |
| 2 (WP-028) | Static TLS: every AAELF64 TLS type and the relaxations W9–W13 (§4.7); IFUNC/`IRELATIVE`, `.eh_frame` and `.eh_frame_hdr` contents (§6.4), the symbol table (§6.5), non-alloc (DWARF) relocations | `tls` (except the constructs §4.7 names), `ifunc`, `eh-frame` (except labels inside `.eh_frame`, §6.4), `non-alloc` |
| 3 | PIE and shared objects: the dynamic sections, dynamic relocations, PLT | — |
| x | x86-64 and i386 tables (§4.5) | `machine` |

On the reference corpus, a stage-1 report may give only the stage-2
reasons in "not checked" (§9.1).

Of stage 2, static TLS (§4.7), call frame information (§6.4), the
symbol table (§6.5), IFUNC (§4.8) and non-alloc relocations (§5.8) have
landed; the last two with WP-028b. The reasons `ifunc` and `non-alloc`
remain only for a CFI pointer to an IFUNC (§6.4) and for compatibility of
reports; compressed debug sections are named as not judged
(`compressed`, which is not a stage-2 reason). WP-028b also added the
layout rules S13–S19 (§6.2), program properties (§6.6) and mortar's
build id (§6.7). The x86 tables (stage x) are measured (§4.5), not
written.

`cargo xtask linktest` runs the verifier on every link (`spec/harness.md`
"L3").

## 2. Inputs

`verify(output, inputs, map) -> Report`:

```rust
pub struct Input<'a> { pub path: &'a str, pub bytes: &'a [u8] }
pub fn verify(output: &[u8], inputs: &[Input<'_>], map: &Map) -> Report;
```

`verify` never panics. Anything it cannot read becomes a finding or a
not-checked count (§9.4).

### 2.1 The output

The output is read the way a loader reads it:

- the file header, with `FileHeader::parse`;
- the program headers, read here (Elf64 entries are 56 bytes, Elf32
  entries 32);
- the section headers, with `SectionTable::parse`;
- `.symtab`.

Memory means the `PT_LOAD` segments. An address is readable when it lies
in one segment's `[p_vaddr, p_vaddr + p_memsz)`. Its bytes are file bytes
up to `p_filesz` and zero after that. If the output cannot be read, the
report has the single finding O1 (§6.1).

### 2.2 The inputs

Each input is identified by the path the map spells. `Input::path` must
match that spelling exactly. `xtask plumb` resolves paths against its
working directory and then reads them (§8). A map file of the form
`archive(member)` names a member of an archive: the member is the text
between the last `(` and the final `)`. The member is found
by name, and by header offset when the map gives one (mortar's format).
The member must be the only one with that name, or the record is refused
(I2). A member of a thin archive is the file at its stored path, from
the archive's directory when the path is relative (GNU ar manual,
"--thin"). That file must be among the inputs under that path, or the
record is I2. `thin_members(path, bytes)` lists those paths for a
caller, and `xtask plumb` reads them. A thin archive's member that is
itself a member of another archive is refused (I2).

An input object must be `ET_REL` of the output's machine. Otherwise the
finding is I2. A file the map names that is not among the inputs is I1,
reported once per path.

### 2.3 The map

The map lists where each input section went (§3). The output alone cannot
answer that, and recovering it by matching contents would be guesswork.
Each linker writes a map: mortar its own (§3.2), and the reference
linkers theirs (§3.4 to §3.6).

## 3. The placement map

### 3.1 The common form

Every reader produces the same `Map`:

| record | fields | meaning |
|---|---|---|
| `files[i]` | `path`, `member`, `member_offset` | an input file, referred to by index |
| `placed` | `file`, `section`, `input_offset`, `addr`, `size` | an input section placed whole at `addr` (`input_offset: None`), or bytes `[input_offset, +size)` of it placed at `addr` (a *piece*) |
| `discarded` | `file`, `section`, `reason` (`gc`, `comdat`, `script`, `unspecified`) | an input section not in the output |
| `folds` | `file`, `section`, `into_file`, `into_section` | identical code folding: the section has the place of `into` |
| `chunks` | `kind` (`thunk`, `patch`, `synthetic`, `common`), `name`, `addr`, `size`, `target` | memory the linker made: veneers, erratum patches, merged strings, COMMON |
| `defined` | `name`, `value` | a symbol the linker defined |

A section is named by its index in its file (mortar's format) or by its
name (the foreign formats). §3.3 resolves names.

### 3.2 mortar's format, version 1

The format is line-oriented UTF-8 text. Each record is one line of tokens
separated by single spaces, and the map ends with `end`:

```text
mortar-map 1
file <id> <path> [<member> <member-header-offset>]
place <file> <section> <addr> <size>
piece <file> <section> <input-offset> <addr> <size>
nplace <file> <section> <output-section> <offset> <size>
merged <file> <section> <output-section>
discard <file> <section> gc|comdat|script|unspecified
fold <file> <section> <into-file> <into-section>
chunk thunk|patch|synthetic|common <name> <addr>|- <size> [<target> <addend>]
define <name> <value>
output static|exec|pie|static-pie
interp <path>
runpath|rpath <paths>
bind lazy|now
relro on|off
arg <word>
needed <name>
copy <name> <addr> <size>
export <name>
plt <name>
iplt <name>
end
```

The token rules:

- **Ids.** `<id>` and `<file>` are decimal. `file` ids count up from 0,
  and a record may refer only to an earlier `file`.
- **Sections.** `<section>` is either the section's index in its file, in
  decimal, or `:` followed by the section's name as a string token. The
  name form is for converted foreign maps.
- **Numbers.** Addresses, sizes and offsets are lower-case hexadecimal
  without a prefix, at most 16 digits. `<addend>` is signed hex, for
  example `-1c`.
- **Strings.** `<path>`, `<member>`, `<name>` and `<target>` are string
  tokens. A byte from `!` to `~`, other than `%`, stands for itself. Every
  other byte is written `%XX` in upper-case hex. The empty string is a
  lone `%`. A decoded string must be UTF-8.
- **Lines.** No line is empty before `end`. After `end`, only empty lines
  may follow.
- **Versions.** The version is the integer in the header. A reader refuses
  any version other than its own.

`nplace` and `merged` are the non-alloc records (spec/image.md R-9,
applied by WP-028b). `nplace`: a non-alloc input section placed whole at
`<offset>` of the non-alloc output section named `<output-section>` (a
string token). `merged`: an `SHF_MERGE` non-alloc input section whose
pieces are in `<output-section>` and are not listed; references to them
are checked by content (§5.6). The output section must exist and be
non-alloc, and the input section must be a free non-alloc section of its
file (M1); an `nplace` size other than the input's is M3. A `place` of a
non-alloc section is also accepted, with `<addr>` read as the output
section's `sh_addr` (0) plus the offset: that is what a converted GNU ld
or lld map holds.

**The dynamic records** (`spec/dyn.md` P-D3; WP-040 step 0d) follow
`define`. A static link writes none of them; a dynamic link (`Exec`,
`Pie`, `StaticPie`) writes them, each package filling its own. Every
string is a string token.

| record | count | meaning |
|---|---|---|
| `output <kind>` | at most one | the output kind of `dyn.md` D-1: `static`, `exec`, `pie`, `static-pie` |
| `interp <path>` | at most one | the `-dynamic-linker` path |
| `runpath <paths>`, `rpath <paths>` | at most one of the two | the `-rpath` list joined by `:` as written (`$ORIGIN` kept); `rpath` under `--disable-new-dtags` (`DT_RPATH`) |
| `bind lazy`, `bind now` | at most one | `-z now` in force or not |
| `relro on`, `relro off` | at most one | `-z norelro` in force or not |
| `arg <word>` | any | the link's command line, one word per record, in order, with response files expanded and the program name left out |
| `needed <name>` | any | **hint**: a `DT_NEEDED` name, in the order written |
| `copy <name> <addr> <size>` | any | **hint**: a copy allocation, by the name its `COPY` record names |
| `export <name>` | any | **hint**: an exported name |
| `plt <name>` | any | **hint**: the key of a PLT entry, in PLT order (`name` or `name@version`) |
| `iplt <name>` | any | **hint**: the key of an IPLT entry, in IPLT order |

The first six records are facts of the command line: the verifier
recomputes each from the `arg` words (or, for a reference link, from its
response file) and reports a record that disagrees. The **hints** are
never the expected value: every decision they name (the needed set and
order, the copies, the export set, PLT order) is recomputed from the
inputs (`spec/dyn.md` §12), and a hint only names what is compared. A
second `output`, `interp`, `runpath`/`rpath`, `bind` or `relro` record
is a `MapError`.

The writer emits records in a fixed order: files by id, then places and
pieces, the non-alloc records, discards, folds, chunks and defined
symbols, each group in the order of the map's vectors, then the dynamic
records in the order of the grammar above.

Every syntax error is a `MapError { line, message }`. Line 0 means the map
was truncated, with no `end`. Tests:

- `map::mortar::tests::s3_2_round_trip`
- `s3_2_tokens_encode_every_byte`
- `s3_2_malformed_maps_are_errors`
- `s3_2_dynamic_records_round_trip`, `s3_2_malformed_dynamic_records_are_errors`
- `crates/mortar/tests/image.rs map_dynamic_records_read_by_the_verifier`
  (the writer side, `mortar_image::map::DynRecords`)
- `s3_7_truncations_are_answers`
- `tests/corpus.rs s3_2_reference_maps_round_trip_through_mortar_format`

### 3.3 Resolving a map

Each placed record names a section of its file:

- **By index.** A section index names that section, unless it is a section
  the linker consumes rather than places (`SHT_NULL`, `SHT_REL`,
  `SHT_RELA`, `SHT_SYMTAB`, `SHT_STRTAB`, `SHT_GROUP`,
  `SHT_SYMTAB_SHNDX`).
- **By name.** A name names every non-consumed section of the file with
  that name. If several match, the record goes to the one that:
  1. has not been placed yet;
  2. has the record's size, except for mergeable sections and
     `.eh_frame`, whose size the linker may change;
  3. otherwise, whose bytes outside its relocated ranges (§6.3) equal the
     output at the record's address;
  4. otherwise, the first one, if all the candidates are identical (bytes,
     type and relocation records).

  If none of these settles it, the candidates are *ambiguous*: their
  records are not checked (`section-not-locatable`).

Each input section ends up with one *place*:

| place | from | its relocation records |
|---|---|---|
| whole at `a` | a `place` record | checked, with `P = a + r_offset` |
| pieces | `piece` records (mortar's format), or lld's `.eh_frame+0xOFF` rows | checked where a piece covers `r_offset`. A record outside every piece is in a dropped part and is counted as not in the output |
| merged | an `SHF_MERGE` `SHF_ALLOC` section in a foreign map, placed, absent or ambiguous | not checked (`in-mergeable`). References *to* it are checked by content (§5.6) |
| rewritten | `.eh_frame` placed whole with a size other than its input's (GNU ld) | checked by CIE and FDE (§6.4) |
| discarded | a `discard` record, or GNU's "Discarded input sections" and `/DISCARD/` | not in the output |
| absent | no record | not in the output. An allocated `.eh_frame` is checked by CIE and FDE whatever its place (§6.4): mold lists none |
| ambiguous | see above | not checked (`section-not-locatable`) |

A fold gives a section its target's place.

The map rules. Each is a finding at the record's address, sited at the
input section:

- **M1.** A record names a section its file lacks, by index or by name.
  GNU ld is exempt: it names the first input file as the origin of the
  sections it makes itself (`.iplt`, `.rela.iplt`, `.got`, build-id; §10.1).
  A fold that does not name exactly one section on each side is also M1.
- **M2.** A section is placed whole more than once, or both whole and as
  pieces.
- **M3.** A section placed whole has a size other than its input's. The
  section still takes the place.
- **M4.** A section with bytes, other than `.tbss`, is not inside an
  allocated output section. Or that output section lacks its
  `SHF_EXECINSTR` or `SHF_WRITE`. Or the address is not a multiple of its
  `sh_addralign`.
- **M5.** Two input sections placed whole share bytes. A `.tbss` section
  occupies no memory and is exempt.

Each rule has a test: `tests/mutation.rs s9_2e_each_map_rule_is_enforced`.

### 3.4 GNU ld (`-Map`)

GNU ld's map is a sequence of blocks, each opened by a line at column 0:

1. **Preamble.** The archive members included, then "Allocating common
   symbols". Nothing in it is read.
2. **`Discarded input sections`.** Each entry becomes a `discarded`
   record with reason `unspecified`.
3. **`Memory Configuration`.** Nothing in it is read.
4. **`Linker script and memory map`.** It is required; without it the text
   is not a GNU map.

In the last block:

- **Column 0.** A line starting at column 0 is either an output section
  name or a command (`LOAD`, `START GROUP`, `END GROUP`, `OUTPUT(…)`,
  `TARGET`, `INPUT`, `GROUP`, "Address of section …"). Commands are
  skipped. An output section name becomes the current output section.
- **Input sections.** An input-section entry starts with one space, then
  the name, then `0xADDR 0xSIZE FILE`. The file is the rest of the line
  and may contain spaces.
- **Wrapped entries.** A name of 14 or more characters is followed by a
  line break. The next line starts with spaces and holds
  `0xADDR 0xSIZE FILE`. The entry takes the line number of its name.
  Anything else on that next line is an error.
- **Skipped lines.** These are not entries: lines starting with two
  spaces (symbols, assignments, `(size before relaxing)`), lines starting
  with ` *` (patterns, `*fill*`), and the data statements `BYTE`, `SHORT`,
  `LONG`, `QUAD`, `SQUAD` and `FILL`.
- **Special entries.** An entry whose file is `linker stubs` is a `thunk`
  chunk: veneers and erratum patches go in `.text.stub`. An entry named
  `COMMON` is a `common` chunk. An entry under `/DISCARD/` is discarded
  with reason `script`.
- **Member spelling.** A file `lib.a(m.o)` is split at the first `(` after
  the last `/`, provided it ends with `)`.

Ambiguities, and how they are resolved:

- **Merge sections.** Merge-section entries keep their input names. They
  overlap, and their sizes are the sizes after merging. They are therefore
  not places: §3.3 makes them *merged*.
- **`.eh_frame`.** Entries show the size after GNU ld edits the records,
  so the section is *rewritten*.
- **Spaces in names.** An input-section name never contains a space. A
  file name may.

Tests: `map::gnu::tests::s3_4_gnu_map_excerpt` and
`s3_4_malformed_gnu_maps_are_errors`.

### 3.5 lld (`-Map`)

lld's map is a table. The header is `VMA LMA Size Align Out In Symbol`.

Each row:

- **Numbers.** The row starts with four numbers: VMA, LMA and size in hex
  without `0x`, and the alignment in decimal.
- **Columns.** The text that follows starts at the column of `Out`, `In` or
  `Symbol` in the header. That column decides what the row is. Text in no
  header column is an error.
- **Input rows.** An `In` row is `file:(section)`, split at the last `:(`,
  with a final `)`. A section ending in `+0xOFF` is a *piece* at input
  offset OFF; lld writes `.eh_frame` this way.

Special rows:

- **`<internal>` rows.** `<internal>:(…)` is a chunk: `.text.thunk` is a
  thunk, `.text.patch` an erratum patch, and anything else (merged
  `.rodata.str*`, `.got`) is synthetic.
- **COMMON.** `file:(COMMON)` is a common chunk.

Ambiguities, and how they are resolved:

- **Garbage-collected sections.** lld does not list them, so they are
  *absent*.
- **Merge sections.** Their input rows give an address that is not the
  pieces' addresses, so §3.3 makes them *merged*.

Tests: `map::lld::tests::s3_5_lld_map_excerpt` and
`s3_5_malformed_lld_maps_are_errors`.

### 3.6 mold (`-Map`)

mold's map has the same column grammar as lld's, with the header
`VMA Size Align Out In Symbol`:

- the VMA is `0x` hex, or `0` for a non-alloc section;
- the size and alignment are decimal.

Ambiguities:

- **Paths.** mold normalizes them: `/usr/lib/gcc/…/../../../aarch64-linux-gnu/crt1.o`
  becomes `/usr/lib/aarch64-linux-gnu/crt1.o`. The inputs must be given by
  mold's spelling.
- **Symbol names.** mold demangles them in the `Symbol` column. They are
  not read.
- **Common symbols.** mold lists a file's common symbols as an input row
  `file:(.common)`. This is read as lld's `COMMON`: a chunk, not an input
  section (`resolve/common` in the link tests).
- **Unlisted inputs.** mold lists neither `.eh_frame` input sections nor
  mergeable ones. `.eh_frame` is *absent*, which counts as `eh-frame`, and
  mergeable sections are *merged*.

Tests: `map::mold::tests::s3_6_mold_map_excerpt` and
`s3_6_malformed_mold_maps_are_errors`.

### 3.7 Errors

Each reader returns `Ok(Map)` or `Err(MapError)`. It never panics. No
input, however truncated or corrupted, leads to any other outcome (tests:
`s3_7_truncations_are_answers`, `tests/hostile.rs s9_4_mutated_maps`).

## 4. Relocation values

### 4.1 Operations

The notation is AAELF64's:

- **S** is the symbol's value (§4.4).
- **A** is the addend: `r_addend` for RELA. For REL it is the field's
  value read from the input place (§4.5).
- **P** is the place's address (§3.3).
- **G(GDAT(S+A))** is the address of the GOT slot that holds S+A (§5.5).
- **GOT** is the value of `_GLOBAL_OFFSET_TABLE_` (the unique symbol of
  that name, else the global one), else the address of the output's
  `.got`.
- **Page(x)** is `x & ~0xfff`.

X, the value a type writes, is computed in 64-bit wrapping arithmetic. Its
overflow check then reads X as a signed 64-bit number.

### 4.2 The stage-1 table (AArch64)

The table is `a64::TABLE`. The test
`a64::tests::s4_2_table_is_sorted_and_named_as_the_reader_names_it` checks
it against this one and against `mortar_elf`'s names.

| type | name | X | field | check |
|---|---|---|---|---|
| 0, 256 | `NONE`, `NULL` | — | — | — |
| 257 | `ABS64` | S+A | data 8 | none |
| 258 | `ABS32` | S+A | data 4 | -2^31 <= X < 2^32 |
| 259 | `ABS16` | S+A | data 2 | -2^15 <= X < 2^16 |
| 260 | `PREL64` | S+A-P | data 8 | none |
| 261 | `PREL32` | S+A-P | data 4 | -2^31 <= X < 2^32 |
| 262 | `PREL16` | S+A-P | data 2 | -2^15 <= X < 2^16 |
| 263 | `MOVW_UABS_G0` | S+A | MOVZ/K imm16 = X[15:0] | 0 <= X < 2^16 |
| 264 | `MOVW_UABS_G0_NC` | S+A | X[15:0] | none |
| 265 | `MOVW_UABS_G1` | S+A | X[31:16] | 0 <= X < 2^32 |
| 266 | `MOVW_UABS_G1_NC` | S+A | X[31:16] | none |
| 267 | `MOVW_UABS_G2` | S+A | X[47:32] | 0 <= X < 2^48 |
| 268 | `MOVW_UABS_G2_NC` | S+A | X[47:32] | none |
| 269 | `MOVW_UABS_G3` | S+A | X[63:48] | none |
| 270 | `MOVW_SABS_G0` | S+A | signed group 0 | -2^16 <= X < 2^16 |
| 271 | `MOVW_SABS_G1` | S+A | signed group 1 | -2^32 <= X < 2^32 |
| 272 | `MOVW_SABS_G2` | S+A | signed group 2 | -2^48 <= X < 2^48 |
| 273 | `LD_PREL_LO19` | S+A-P | imm19 = X[20:2] | -2^20 <= X < 2^20 |
| 274 | `ADR_PREL_LO21` | S+A-P | ADR imm = X[20:0] | -2^20 <= X < 2^20 |
| 275 | `ADR_PREL_PG_HI21` | Page(S+A)-Page(P) | ADRP imm = X[32:12] | -2^32 <= X < 2^32 |
| 276 | `ADR_PREL_PG_HI21_NC` | Page(S+A)-Page(P) | X[32:12] | none |
| 277 | `ADD_ABS_LO12_NC` | S+A | imm12 = X[11:0] | none |
| 278 | `LDST8_ABS_LO12_NC` | S+A | imm12 = X[11:0] | none |
| 279 | `TSTBR14` | S+A-P | imm14 = X[15:2] | -2^15 <= X < 2^15 |
| 280 | `CONDBR19` | S+A-P | imm19 = X[20:2] | -2^20 <= X < 2^20 |
| 282 | `JUMP26` | S+A-P | imm26 = X[27:2] | -2^27 <= X < 2^27 |
| 283 | `CALL26` | S+A-P | imm26 = X[27:2] | -2^27 <= X < 2^27 |
| 284 | `LDST16_ABS_LO12_NC` | S+A | imm12 = X[11:1] | none |
| 285 | `LDST32_ABS_LO12_NC` | S+A | imm12 = X[11:2] | none |
| 286 | `LDST64_ABS_LO12_NC` | S+A | imm12 = X[11:3] | none |
| 287 | `MOVW_PREL_G0` | S+A-P | signed group 0 | -2^16 <= X < 2^16 |
| 288 | `MOVW_PREL_G0_NC` | S+A-P | X[15:0] | none |
| 289 | `MOVW_PREL_G1` | S+A-P | signed group 1 | -2^32 <= X < 2^32 |
| 290 | `MOVW_PREL_G1_NC` | S+A-P | X[31:16] | none |
| 291 | `MOVW_PREL_G2` | S+A-P | signed group 2 | -2^48 <= X < 2^48 |
| 292 | `MOVW_PREL_G2_NC` | S+A-P | X[47:32] | none |
| 293 | `MOVW_PREL_G3` | S+A-P | X[63:48] | none |
| 299 | `LDST128_ABS_LO12_NC` | S+A | imm12 = X[11:4] | none |
| 307 | `GOTREL64` | S+A-GOT | data 8 | none |
| 308 | `GOTREL32` | S+A-GOT | data 4 | -2^31 <= X < 2^31 |
| 309 | `GOT_LD_PREL19` | G-P | imm19 = X[20:2] | -2^20 <= X < 2^20 |
| 310 | `LD64_GOTOFF_LO15` | G-GOT | imm12 = X[14:3] | 0 <= X < 2^15 |
| 311 | `ADR_GOT_PAGE` | Page(G)-Page(P) | ADRP imm = X[32:12] | -2^32 <= X < 2^32 |
| 312 | `LD64_GOT_LO12_NC` | G | imm12 = X[11:3] | none |
| 313 | `LD64_GOTPAGE_LO15` | G-Page(GOT) | imm12 = X[14:3] | 0 <= X < 2^15 |
| 314 | `PLT32` | S+A-P | data 4 | -2^31 <= X < 2^31 |

All names take the `R_AARCH64_` prefix. Type 281 is not defined. The TLS
types are in §4.7. Any other type is not checked (`type`).

### 4.3 Fields

The bits each field covers come from the A64 encodings.

**Encoding X.** Writing X into a field replaces the field's bits and keeps
every other bit of the *input's* place:

- **Data** is the low `8w` bits of X, little-endian.
- **MOVW** (unsigned and `_NC`): bits [20:5] = X[s+15:s].
- **Signed MOVW groups.** If X < 0, the instruction becomes MOVN with
  `imm16 = (~X)[s+15:s]`. Otherwise it becomes MOVZ with
  `imm16 = X[s+15:s]`. Bits [30:29] are part of the field.
- **ADR, ADRP.** `immlo` (bits [30:29]) and `immhi` (bits [23:5]) receive
  X[1:0]:X[20:2] for ADR, and X[13:12]:X[32:14] for ADRP.
- **imm12** (bits [21:10]) = X[11:scale], and X[14:3] for the `LO15` forms.
- **imm19** (bits [23:5]), **imm14** (bits [18:5]), **imm26** (bits [25:0]).

**Checks.** The overflow check of the type must hold (R2). The low bits a
field drops must be zero: `scale` bits for imm12, 3 bits for `LO15`, 2 bits
for the branch fields (R3). A `_NC` type has no overflow check, but its
alignment is still checked.

- **HI12** (bits [21:10]) = X[23:12], for `ADD ..., LSL #12`.
- **Insn** (`TLSDESC_CALL`): no field; the type marks an instruction.

**R8: the instruction fits the type.** The input instruction at the place
must be of the class the type is defined for: a load or store of the
access size its `imm12` is scaled by (`LDST8` to `LDST128`, and the TLS
`LDST` forms), and a 64-bit `LDR` (unsigned offset) for the `LD64` GOT
forms (310, 312, 313, 542, 563). Otherwise the record is R8 and is not
otherwise judged. `ldr w0, [x0, :got_lo12:x]` is the case of review
finding 4: GNU ld writes the scale-8 offset into the scale-4 load, and lld
replaces the load with a 64-bit `ADR` (A5); the input is refused by name
whatever the linker made of it.

Test: `a64::tests::s4_3_fields_encode_as_the_instruction_set_says`,
`s4_3_checks`, `tests/tls.rs s4_3_r8_the_instruction_must_fit_the_type`.

### 4.4 S: resolving a symbol

Symbol 0 has S = 0. An IFUNC symbol is resolved by §4.8. A thread-local symbol (`STT_TLS`, or any symbol defined in an
`SHF_TLS` section, such as GCC's `.LANCHOR0` or a section symbol) resolves
as below to its address in the TLS template, which §4.7 turns into an
offset; through the output's `.symtab` that address is `PT_TLS` `p_vaddr`
+ `st_value` (gABI "Symbol Values"). Otherwise:

- **A local symbol** (`STB_LOCAL`, including `STT_SECTION`) defined in
  section k has S = place(k) + `st_value`, which depends on k's place:
  - **whole at a:** S = a + `st_value`;
  - **pieces:** for a section symbol, the reference's addend selects
    the piece (`synth.md` S-17): r = `st_value` + A, S + A is the address
    of byte r in the piece that holds it, and r outside `[0, sh_size)` is
    R6 under mortar's map (S-18) and `piece-not-locatable` under a
    foreign one. For any other symbol S is found through the piece that
    holds byte `st_value` (at `st_value = sh_size`, the piece ending
    there), and A is added after (S-19). The same order holds for every
    consumer of a resolved target: fields, GOT slots, tombstones and
    `.eh_frame` pointers (fixed by WP-028c; the verifier first found the
    piece of `st_value` for section symbols too);
  - **merged:** §5.6;
  - **in `.eh_frame`** (a label such as crtbeginT.o's
    `__EH_FRAME_BEGIN__`) when the section is rewritten, absent, or the
    label is in a dropped part: `eh-frame`;
  - **ambiguous:** `section-not-locatable`;
  - **discarded or absent:** R5 (§5.7).

  An `SHN_ABS` symbol has S = `st_value`. Anything else is
  `symbol-unresolved`.
- **A global, weak or `STB_GNU_UNIQUE` symbol**, by name, takes the first
  of these that applies:
  1. **The output's global symbol of that name.** This is the first
     `STB_GLOBAL` or `STB_WEAK` symbol of the name in `.symtab`, and it
     decides which definition won (an IFUNC is §4.8's; TLS is §4.7's).
     If it is defined, S is its value. If it is undefined and weak
     (on either side), the symbol is an *undefined weak*.
  2. **The unique placed definition in the inputs.** Take the one strong
     definition, else the one weak definition, whose section is placed,
     pieces or merged (or that is `SHN_ABS`). S follows from that section's
     place, as for a local symbol. This step covers hidden symbols, which
     lld and mold make local in the output.
  3. **The unique output symbol of that name**, of any binding.
  4. **An undefined weak** in the input, when the output has a `.symtab`.
     A stripped output (`-s`) does not say whether the linker defined the
     name (glibc's weak `__rela_iplt_start`), so the record is
     `symbol-unresolved` (`resolve/symtab` in the link tests).

  If none applies, the record is `symbol-unresolved`.

**An undefined weak** has S = 0, except for a PC-relative type that is
not a branch (`PREL64/32/16`, `LD_PREL_LO19`, `ADR_PREL_LO21`, the
`MOVW_PREL` groups), for which S = P, so X = A (AAELF64: an unresolved
weak reference is the place for a PC-relative relocation). GNU ld and lld
write X = A; mold refuses such a record as out of range (review finding
10; test: `tests/tls.rs s4_4_undefined_weak_pc_relative_is_the_place`).
Branches keep S = 0 and have W4 and W5.

Lookups by name go through hash maps. Nothing depends on their iteration
order (§7.1).

### 4.5 Other machines and REL

The engine dispatches on `Machine`. Stage 1 has the AArch64 table only,
so every record of another machine is not checked (`machine`). The
x86-64 and i386 tables will be modules shaped like `a64.rs`: a `Def` table
of operation, field and check, field encoders, and the rewritings of their
psABIs (GOTPCRELX relaxation, TLS relaxation). The rest stays as it is:
resolution (§4.4), places (§3.3), groups (§5.5, §5.6) and the report.

For `SHT_REL` (i386), A is read from the input's place through the type's
field. For data fields that is the place's value sign-extended to the
field's width. The output is compared with the encoding of X into the same
place, so REL and RELA follow one path.

Measured (WP-028b, the L3 run of `tests/link` for x86-64 and i386: 243
outputs, every one "not judged: machine"). The records of the tests' own
objects, by type:

| x86-64 | records | i386 | records |
|---|---|---|---|
| `PC32` | 177 | `PC32` | 194 |
| `PLT32` | 109 | `PLT32` | 91 |
| `64` | 19 | `GOTOFF` | 67 |
| `TPOFF32` | 8 | `GOTPC` | 58 |
| `REX_GOTPCRELX`, `32` | 3 each | `32` | 23 |
| `TLSGD`, `GOTTPOFF`, `GOTPCREL`, `DTPOFF32` | 2 each | `GOT32X` | 10 |
| `TLSLD`, `32S` | 1 each | `GOT32` | 9 |
| | | `TLS_LE` | 8 |
| | | `TLS_LDM`, `TLS_GOTIE`, `TLS_GD` | 2 each |
| | | `TLS_LDO_32` | 1 |

With the static C library the GOT forms dominate: x86-64's `libc.a` adds
1732 `GOTTPOFF`, 83 `GOTPCREL`, 72 `REX_GOTPCRELX` and some `GOTPCRELX`;
i386's adds 13158 `GOTOFF`, 2581 `GOTPC`, 1718 `TLS_GOTIE`, 884
`GOT32X` and some `GOT32`. A table that judges only the direct types
would leave most of a driver link's records `type`.

**x86-64 (WP-028c, request R-12 of `spec/x64.md`).** `src/x64.rs` is
`spec/x64.md` §6.1's M1 rows, in the shape of `a64.rs`; the record path
is `src/x64rec.rs`. Fields are little-endian data of 1, 2, 4 or 8 bytes
(`TLSDESC_CALL` has none). The rows, by operation (§4.1):

| operation | types |
|---|---|
| `Abs` (S + A) | `64` (none), `32` (u32), `32S` (s32), `16`, `8` (either: signed or unsigned) |
| `Prel` (S + A − P) | `PC32`, `PLT32` (s32), `PC16` (s16), `PC8` (s8), `PC64` (none) |
| GOT slot, `PrelA` (G + A − P) | `GOTPCREL`, `GOTPCRELX`, `REX_GOTPCRELX` (address slot), `GOTTPOFF` (TP-offset slot), `TLSGD`, `TLSLD`, `GOTPC32_TLSDESC` (s32; relaxed in a static output, §5.9) |
| GOT slot, `OffA` (G + A − GOT) | `GOT64` (none) |
| `GotRel` (S + A − GOT) | `GOTOFF64`; `PLTOFF64` (L + A − GOT, L the IPLT entry when there is one) |
| `GotPc` (GOT + A − P) | `GOTPC32` (s32), `GOTPC64` (none) |
| `TpRel` | `TPOFF32` (s32), `TPOFF64` |
| `DtpRel` | `DTPOFF32` (s32), `DTPOFF64`: the TP or DTP offset by section and linker (X-46, §5.9) |
| `Size` | `SIZE32` (u32), `SIZE64` |
| marker | `TLSDESC_CALL` (§5.9) |

GOT is the output's `_GLOBAL_OFFSET_TABLE_`. The later types of X-13 (`GOT32`, `GOTPCREL64`, `GOTPLT64`, the
APX `CODE_*` types 43–51) are not in the table: their records are not
checked (`type`). The dynamic-only types 5–8, 16 and 36–38 in an input
are R8 (X-14: they have no static value; GNU ld writes S + A, W3). `64` (and `32`) against a TLS symbol in `.debug_addr`
is `DtpRel(S + A)` (X-53); `GOTOFF64` in `.debug_info` is `S + A − GOT`
(X-53); every other non-alloc record is §5.8's. L3 judges x86-64 outputs
(`xtask/src/linktest/engine.rs` `not_judged`); the run-wide x86-64
default is mold's A13 (§10.2).

**i386 (WP-028e, request R-12 of `spec/x86.md`).** `src/x86.rs` is
`spec/x86.md` §6.1's M1 rows (21, `NONE` has no row); the record path
is `src/x86rec.rs`, and the `ModRM`/`SIB` reading both x86 tables share
is `src/x86op.rs`. Every addend is the field, sign-extended (X86-4).
X is computed modulo 2^32 and the narrow checks apply to the reduced
value read as an `i32` (x86.md §3.1); `TLS_LE` (s32, X86-38) and
`SIZE32` (u32, X86-12) are checked before the reduction. The rows:

| operation | types |
|---|---|
| `Abs` (S + A) | `32` (mod 2^32), `16`, `8` (either) |
| `Prel` (S + A − P) | `PC32`, `PLT32` (mod 2^32), `PC16` (s16), `PC8` (s8) |
| GOT slot, `OffA` (G + A − GOT) | `GOT32`, `GOT32X` (address slot) with a base register; `TLS_GOTIE` (TP-offset slot); `TLS_GD`, `TLS_LDM`, `TLS_GOTDESC` (relaxed in a static output, §5.11) |
| GOT slot, `AbsA` (G + A) | `GOT32`, `GOT32X` with no base register (X86-13); `TLS_IE` |
| `GotRel` (S + A − GOT) | `GOTOFF` |
| `GotPc` (GOT + A − P) | `GOTPC`, its symbol ignored (X86-30) |
| `TpRel` | `TLS_LE` (s32) |
| `DtpRel` | `TLS_LDO_32`: the TP or DTP offset by section and linker (§5.11) |
| `Size` | `SIZE32` (u32) |
| marker | `TLS_DESC_CALL` (§5.11) |

GOT is the output's `_GLOBAL_OFFSET_TABLE_` (the start of `.got.plt`),
except under mold's map, where it is the start of `.got`: mold's `GOTPC`
sets the base register to it, and its `GOTOFF` and based `GOT32` values
are relative to it (x86.md X86-47; WP-028e's `g.s`: `GOTPC` gives
`.got`'s address in `%ebx`). The operand of a `GOT32`/`GOT32X` decides its
form (X86-13): the byte before the field read as a `ModRM` (`(b & 0xc7)
== 5`: no base), the byte before that as a `ModRM` whose `rm` is 100 (a
`SIB` follows); two readings that disagree, a record at offset 0 or in a
section that is not executable are R8 ("the GOT32/GOT32X operand …"),
under every map (W1). `TLS_IE_32`, `TLS_LE_32` and the other numbered
types outside the table are not checked (`type`). The dynamic-only types
(5–8, 14, 35–37, 41, 42) in an input are R8 (X86-20, W14), and so is an
`SHT_RELA` section of an `EM_386` object (X86-2, W12: the references
disagree on it). Two records of one `SHT_REL` section at one
`r_offset`, or whose fields overlap (`TLS_DESC_CALL` has none), are R8
at the later one (x86.md R-13 (b): each implicit addend would be read
from bytes the other writes; every reference linker applies both,
WP-028e's `rel-overlap`, where the second write also leaves the first
field R1). `32` against a TLS symbol at a non-alloc place is
`DtpRel(S + A)` (X86-62; GNU ld and mold write the template address, A15).
A `GOTOFF` to a piece of a mergeable section (the PIC string idiom,
`leal .LC0@GOTOFF(%ebx)`) is grouped by its piece (§5.6) with the
evidence GOT + field. L3 judges i386 outputs (`not_judged` accepts
ELFCLASS32 `EM_386`).

### 4.6 Y1: symbol values

For each name that has a global, weak or unique definition in the inputs,
take the output symbol of that name: the global one, else the unique one.
If it is defined, its value must equal the place of one of the name's
placed definitions, or the `st_value` of an `SHN_ABS` one. For a TLS
symbol the value compared is `PT_TLS` `p_vaddr` + `st_value`; without a
usable `PT_TLS` the name is unchecked.

- A name with IFUNC definitions is judged by §4.8 ("Y1 of an IFUNC
  name").
- A name with no definition placed whole is unchecked.

Counts: `symbols_checked`, `symbols_unchecked`. Test:
`tests/mutation.rs s9_2b_moved_symbols_are_found`.

### 4.7 Thread-local storage (stage 2)

A static executable has one module, its TLS block is the `PT_TLS`
template, and on AArch64 (TLS variant 1) the block starts `tcb` bytes
after the thread pointer:

- `start` = `PT_TLS` `p_vaddr`, `tcb` = align_up(16, `p_align`);
- TPREL(v) = v − start + tcb, DTPREL(v) = v − start, the module index is 1.

On x86-64 and i386 (variant II, x64.md X-44, x86.md X86-57: the block
ends at the thread pointer, `%fs` and `%gs`), `tcb` = −align_up(`p_memsz`,
`p_align`); i386 values are compared modulo 2^32, `TLS_LE` as s32 before
the reduction (X86-38). The four linkers place an over-aligned i386 block
alike (x86.md E7); the corpus's TLS cases read every variable's offset.

More than one `PT_TLS`, an alignment that is not a power of two, or a
`p_vaddr` that is not a multiple of `p_align` is S9 (§6.2), and the TLS
records are then `tls`. A TLS record in an output without `PT_TLS` is
**T1**. A TLS type against a symbol that is not thread-local is **R8**.
`_TLS_MODULE_BASE_` (the TLSDESC ABI's name for the start of the module's
block) resolves to `start` whatever value a linker gives its symbol
(A3; review finding 6).

The types (AAELF64 "Relocations for thread-local storage"), with the
operation and field; checks are as AAELF64 states them:

| types | operation | field |
|---|---|---|
| 544–548 `TLSLE_MOVW_TPREL_G2/G1/G1_NC/G0/G0_NC` | TPREL(S+A) | signed MOVW (G2, G1, G0), MOVW (`_NC`) |
| 549 `TLSLE_ADD_TPREL_HI12`, 550/551 `_LO12(_NC)` | TPREL(S+A) | HI12; imm12 |
| 552–559, 570/571 `TLSLE_LDST{8,16,32,64,128}_TPREL_LO12(_NC)` | TPREL(S+A) | imm12, scaled |
| 523–527 `TLSLD_MOVW_DTPREL_*`, 528–538, 572/573 `TLSLD_ADD/LDST*_DTPREL_*` | DTPREL(S+A) | as the TPREL forms |
| 539/540 `TLSIE_MOVW_GOTTPREL_G1/G0_NC` | G − GOT, G holding TPREL(S+A) | signed MOVW; MOVW |
| 541 `TLSIE_ADR_GOTTPREL_PAGE21`, 542 `_LD64_GOTTPREL_LO12_NC`, 543 `_LD_GOTTPREL_PREL19` | Page(G) − Page(P); G; G − P | ADRP; imm12/8; imm19 |
| 513/514 `TLSGD_ADR_PAGE21`, `_ADD_LO12_NC` | G of a 16-byte entry (1, DTPREL(S+A)) | ADRP; imm12 |
| 518/519 `TLSLD_ADR_PAGE21`, `_ADD_LO12_NC` | G of a 16-byte entry (1, 0) | ADRP; imm12 |
| 562–564 `TLSDESC_ADR_PAGE21`, `_LD64_LO12`, `_ADD_LO12`; 569 `TLSDESC_CALL` | the sequence below | ADRP; imm12/8; imm12; none |

The types no reference linker or assembler produces for a static link
(512, 515–517, 520–522, 560, 561, 565–568, the `PREL21`/`PREL19` GD, LD and
TLSDESC forms) are `type`. The dynamic TLS types (1028–1031) are stage 3.

**Decisions.** A TPREL or DTPREL record is judged as in §5.2 with X as
above. A TLS GOT record is decided by W9 (IE), or by its sequence (GD, LD,
TLSDESC); when it is not rewritten, its output instruction must equal the
input outside the field (or be W7's ADR), else it is R1 ("rewritten into
a form no specified rewriting allows"), and it then joins the GOT groups
of §5.5 keyed by (kind, object, symbol, addend), whose valid entry holds
the TPREL word or the 16-byte pair. `TLSIE_MOVW` records give G − GOT
from the MOVZ/MOVN high half and the MOVK low half.

**Sequences.** Partners are found by offset in the same section:

- **TLSDESC**: 562, 563, 564, 569 at P, P+4, P+8, P+12, one symbol and
  addend. The input must be `ADRP x0; LDR Xt, [x0]; ADD x0, x0; BLR Xt`
  (else R8). A TLSDESC record outside a whole sequence is `tls-sequence`.
  W10 or W11 accepts all four; a sequence left as it is (for a dynamic
  loader) is `tls` (stage 3); anything else is R1 with TPREL as expected.
- **GD and LD**: 513+514 (or 518+519) at P, P+4, `CALL26 __tls_get_addr`
  at P+8, `NOP` in the input at P+12, input `ADRP x0; ADD x0, x0`. W12 or
  W13 accepts the three records (the word at P+12 has no record; W12's
  rewrite of it is exempt from S12). A sequence left as it is is decided
  per record (GOT pair, ordinary `CALL26`). Anything else is R1, on the
  call as well.

**Not judged** (`tls`): an undefined weak TLS symbol (glibc's
`_nl_current_LC_*`, referenced by `setlocale.o` through IE); a non-TLS
type against a TLS symbol; a TLSDESC sequence kept for a dynamic loader;
the TLS records of an output whose `PT_TLS` is unusable (S9).

Static PIE (`ET_DYN` with no `PT_INTERP`) is stage 3: on the reference
linkers its TLS takes the same forms and has no TLS dynamic relocation,
but S1 refuses `ET_DYN` today (§11).

Tests: `tests/tls.rs` (§9.1), `tests/mutation.rs
s9_2f_tls_mutations_are_found`.

### 4.8 IFUNC (stage 2, WP-028b)

An IFUNC (`STT_GNU_IFUNC`) symbol names a resolver; the program calls
the function the resolver returns. In a static output the linker makes,
for each IFUNC symbol it needs, a slot with an `IRELATIVE` record whose
addend (RELA) or slot contents (REL, i386) is the resolver's address, and
an *IPLT entry*: linker-made code that loads the slot and branches to what
it holds. The C runtime of a static executable applies the records from
`__rela_iplt_start` to `__rela_iplt_end` (`__rel_iplt_*` with REL; glibc
`csu/libc-start.c` `apply_irel`); under static-pie the symbols are weak
and undefined (spec/resolve.md) and the dynamic loader applies them.

**The tables** come from the output alone:

- the *records*: every entry of every allocated `SHT_RELA`/`SHT_REL`
  section. An `IRELATIVE` is type 1032 (AArch64), 37 (x86-64) or 42
  (i386);
- the *IPLT entries*: places outside every placed input section (§5.4),
  in an allocated executable section, that load the slot of an
  `IRELATIVE` record. On AArch64 the entry is `ADRP Xn, slot; LDR Xt,
  [Xn, :lo12:slot]` followed by `BR Xt` (mold's `.plt.got`), or by `ADD
  Xn, Xn, :lo12:slot; BR Xt` (GNU ld's `.plt`, lld's `.iplt`). In an
  output marked BTI, GNU ld and lld 22 put `BTI c` before the `ADRP`:
  the entry then starts at the `BTI` (a branch or address to it is to
  the entry; WP-028d, which removed the F4 the pipeline tests saw on
  `-mbranch-protection` links). On x86 it is
  `jmp *slot` (`FF 25`, RIP-relative on x86-64, absolute on i386), with an
  optional `ENDBR64`/`ENDBR32` and `BND` prefix before it. The entries of
  a resolver are those whose slot is a slot of a record of that resolver.

**Rules on the tables** (on every output; F1's first sentence on a static
executable only, `ET_EXEC` with no `PT_DYNAMIC` and no `PT_INTERP`):

- **F1.** Every record of a static executable is an `IRELATIVE` with
  symbol 0, since nothing else applies the others. When the output
  defines both bounds, the records from the start to the end symbol are a
  whole run of records, and every `IRELATIVE` lies inside it.
- **F2.** Each `IRELATIVE` slot is a word-aligned word of a writable
  `PT_LOAD`, outside every placed input section, and no two records share
  one. Its resolver is the place (§4.4) of an `STT_GNU_IFUNC` definition
  in the inputs, in an executable segment.

**S of an IFUNC.** A record's symbol is an IFUNC when it is an input
`STT_GNU_IFUNC` symbol (local, or a global this object defines), or a
global whose §4.4 step-2 definition is one, or, failing a placed
definition, when the output's symbol of the name is `STT_GNU_IFUNC` (its
value is then the resolver). The symbol's identity is its name (a
global) or its object and index (a local): two aliases of one resolver
(`gf`, `lf` in the corpus's `ifunc` case) get an entry each on every
reference linker, so pointer equality is per symbol. Then:

- **a branch** (`CALL26`, `JUMP26`, `CONDBR19`, `TSTBR14`) must reach an
  IPLT entry of the resolver, directly or through a veneer (W6): any
  entry calls the same function;
- **an address** (any other non-GOT type) has S = an IPLT entry E of the
  resolver, with W1;
- **a GOT-indirect pair** is W2/W3 to E, or reaches a slot (§5.5) that
  holds E + A, or is an `IRELATIVE` slot of the resolver (*resolved*: it
  holds the function itself once applied).

Every non-branch reference to one symbol must use one E, the *canonical
entry*: the one most references are right with (the lowest on a tie).

- **F3.** A reference that is right only with another entry, or that
  loads a resolved slot while other references take E, gives the
  function two addresses (`&f != &f`).
- **F4.** A reference that reaches no entry of the resolver, or a GOT
  pair that reaches no slot of either kind; also any reference to an
  IFUNC whose resolver has no IPLT entry. R2/R3 when every entry fails the
  check or the alignment.

Verified references count under `iplt` (§7.2); coverage also counts
`irelative_checked` and `iplt_entries`.

**Y1 of an IFUNC name** (§4.6). The output symbol is `STT_GNU_IFUNC` at
the resolver: GNU ld's form, and mortar's (synth.md S-71). lld and mold
make it `STT_FUNC` at the canonical IPLT entry instead, which keeps
`&f` in a debugger equal to `&f` in the program. That is accepted from
those two linkers as their convention, by format (a per-linker rule, not
an anomaly: no value is wrong), and for a local of the name under Y4 as
well. Y3 judges the GNU form like any symbol and leaves the other one to
Y1.

Observed (AArch64, static, `ifs.s` with a global and a local IFUNC, and
glibc's `memcpy`/`strlen`/... with `pick.c`):

| | records in | slots in | entries in | `.symtab` |
|---|---|---|---|---|
| GNU ld | `.rela.plt` | `.got.plt` | `.plt` (16 bytes) | `IFUNC` at the resolver |
| lld | `.rela.dyn` | `.got.plt` | `.iplt` (16 bytes) | `FUNC` at the entry, size 0 |
| mold | `.rela.dyn` | `.got` | `.plt.got` (`BR` then `BRK`) | `FUNC LOCAL` at the entry (A1, A2) |

GNU ld and mold give a GOT reference a slot holding the canonical entry;
lld rewrites it (W3) to `ADR` of the entry. No linker uses a resolved slot
for an address.

Tests: `tests/corpus.rs` (the `ifunc` case), `tests/mutation.rs
s9_2i_ifunc_mutations_are_found`.

**x86-64 (WP-028c).** IRELATIVE is 37; an entry is linker-made code that
loads a slot, `ff 25 rel32` (`jmp *slot(%rip)`), after an optional
`endbr64` and `bnd` prefix. Observed (static, `tests/x64/iplt-ibt.s` and
glibc's IFUNCs):

| | records in | slots in | entries in | slot word before start-up |
|---|---|---|---|---|
| GNU ld | `.rela.plt` | `.got.plt` | `.plt`, 16 bytes `ff 25; 68; e9`, or X-38's with IBT | entry + 6; the entry with IBT; 0 for a `.got` slot no entry loads |
| lld | `.rela.dyn` | `.got.plt` | `.iplt`, 16 bytes `ff 25; 68; e9`, or X-38's with IBT | 0 |
| mold | `.rela.dyn` | `.got` | `.plt.got`, 8 bytes `ff 25; cc cc`, with IBT too (W6) | the resolver |

- **F5 (x86-64).** (1) When the output's `GNU_PROPERTY_X86_FEATURE_1_AND`
  has IBT, every IPLT entry starts with `endbr64` (X-38); mold's do not
  (W6, A17). (2) mortar's entries are exactly X-37's 8 bytes (`ff 25 rel32
  66 90`, `rel32` = slot − (entry + 6)) or, with IBT, X-38's 16 (`f3 0f 1e
  fa ff 25 rel32 66 0f 1f 44 00 00`). (3) In a static executable each
  IRELATIVE slot holds, before start-up, what its linker writes there
  (X-39, the table's last column; mortar 0). The value is never read
  (glibc's `apply_irel` overwrites it), so (3) is a check of the linker's
  own convention: a different word means the slot is not the one the
  linker made.

  (2) now runs on mortar's output (WP-028d): `iplt-ibt.s` linked by
  mortar is clean, and its first entry's `endbr64` made a 4-byte NOP is
  F5 by (1) and (2), with F4 for the five references to the entry. The
  mutation harness's `iplt-entry` class (§9.5) makes the same mutant on
  every IPLT entry of every mortar output.

## 5. Checking a record

### 5.1 The procedure

The records of a section are checked in index order. For each record:

1. **Place.** Find P (§3.3). A record outside every piece of a sectioned
   place is not in the output.
2. **Trivial types.** Type 0 or 256 counts as `none`. Types not in the
   tables go to `type`. The input instruction must fit the type (R8, §4.3).
3. **Place bounds.** The input's place, `width` bytes at `r_offset`, must be
   inside the section's data. If it is not, the finding is I2 (for
   `SHT_NOBITS`, the input word is taken as 0).
4. **The output word.** Read the output's word at P. If P is outside the
   loaded memory, the finding is R7. An instruction field at an erratum
   843419 site is read through the patch (W8, §5.3).
5. **S.** Resolve S (§4.4). A TLS type is decided by §4.7.
6. **GOT types and merged targets.** These are grouped and decided after
   every section (§5.5, §5.6).
7. **Everything else.** Compute X and give a verdict (§5.2). A record that
   is not verified may still be accepted as a rewriting (§5.3).

### 5.2 Verdicts

Let E = `field.encode(input_word, X)`. The record is decided in this
order:

| outcome | condition |
|---|---|
| **R2** (overflow) | the type's check fails on X |
| **R3** (misaligned) | X has bits set that the field drops |
| **verified** | the output word equals E |
| **accepted W7** | the field is ADRP, P is an erratum `ADRP` address, and the output is `ADR` of the same register to `Page(P) + X` |
| **R1** (field) | otherwise; the finding gives the expected word E and the found word |

Only R1, R2 and R3 can be replaced by a rewriting (§5.3). The finding of
the verdict stands when no rewriting applies.

### 5.3 The specified rewritings

A linker may perform the rewritings below, and only these. Each is
accepted only when every stated condition holds. Registers are those of
the *input* instructions, and "partner" means the record at `r_offset ± 4`
in the same section with the same symbol and addend.

The branch rewritings apply to `JUMP26` and `CALL26`:

- **W4 `weak-branch-nop`.** S is an undefined weak, and the output is
  `NOP`.
- **W5 `weak-branch-next`.** S is an undefined weak, and the output is the
  same instruction (B or BL) to P+4.
- **W6 `veneer`.** The output is the same instruction (B or BL) to a
  target T. T must be loaded memory outside every placed input section,
  and the veneer emulator (§5.4) run from T must reach S+A.

The address rewritings:

- **W1 `adrp-add-to-adr`.** The input is `ADRP Xd` (275 or 276) at P with
  partner `ADD Xd, Xd, #lo12` (277) at P+4. The output is `NOP` at P and
  `ADR Xd, S+A` at P+4. Both records are accepted. The input ADD must have
  Rd = Rn = the ADRP's Rd. With a merged target (§5.6), the ADR's target
  is the evidence and must be a piece that matches.
- **W2 `got-to-adrp-add`.** The input is `ADRP Xd, :got:` (311) at P with
  partner `LDR Xt, [Xn, :got_lo12:]` (312). The registers must satisfy
  Xt = Xn = Xd. The output is `ADRP Xd, Page(S+A)` and
  `ADD Xd, Xd, #lo12(S+A)`. This is the GOT reference made direct.
- **W3 `got-to-adr`.** The input is as in W2. The output is `NOP` and
  `ADR Xd, S+A`.

The erratum rewritings (Cortex-A53 erratum 843419):

- **W7 `erratum-843419-adr`.** An `ADRP` at an address ending in `0xff8`
  or `0xffc` is made an `ADR` of the same page, with the same register.
- **W8 `erratum-843419-patch`.** All of these hold:
  - P ends in `0x000`, `0x004` or `0x008`;
  - the input word is a load or store with an unsigned 12-bit offset;
  - the output word is `B T`;
  - T is outside every placed input section;
  - T holds a load or store with an unsigned offset;
  - T+4 holds `B P+4`.

  The word at T is then the word the program executes, and it is checked
  in place of the branch. The same rule allows the branch in the content
  check (§6.2 S12).

The TLS rewritings (§4.7), static executables only. X is TPREL(S+A), and
the local-exec forms need 0 <= X < 2^32. `MOVZ x, #X[31:16], LSL #16` is
written `hi(x)` and `MOVK x, #X[15:0]` is written `lo(x)`:

- **W9 `tlsie-to-le`.** `ADRP Xd, :gottprel:S` (541) and
  `LDR Xt, [Xn, :gottprel_lo12:S]` (542), anywhere in the section with the
  same symbol and addend, made `hi(Xd)` and `lo(Xd)`, with Xt = Xn = Xd.
  With Xt ≠ Xn the rewriting leaves the value in no register (A8): it is
  not W9, and the changed instructions are R1.
- **W10 `tlsdesc-to-le`.** The four TLSDESC words made
  `hi(x0); lo(x0); NOP; NOP` (GNU ld, lld) or `NOP; NOP; hi(x0); lo(x0)`
  (mold).
- **W11 `tlsdesc-to-ie`.** The four TLSDESC words made
  `ADRP x0, G; LDR x0, [x0, :lo12:G]; NOP; NOP` or
  `NOP; NOP; ADRP x0, G; LDR x0, [x0, :lo12:G]` (mold), with G a GOT entry
  (§5.5 valid-slot conditions) holding X.
- **W12 `tlsgd-to-le`.** `ADRP x0; ADD x0, x0; BL __tls_get_addr; NOP` made
  `hi(x0); lo(x0); MRS x1, TPIDR_EL0; ADD x0, x1, x0`.
- **W13 `tlsld-to-le`.** The same LD sequence made
  `MRS x0, TPIDR_EL0; ADD x0, x0, #tcb; NOP; NOP`. The DTPREL records that
  follow keep their DTPREL values. GNU ld writes `#16` whatever `tcb` is
  (A7).

x86-64's rewritings are §5.9's, not W1–W13 (request R-12).

### 5.4 The veneer emulator

From T, at most 8 instructions are executed. Only these are followed:

- `BTI`, as the first instruction only (a `NOP` is not followed, so a
  branch to the padding before a veneer is a finding);
- `ADRP`, `ADR`, `ADD` (immediate, optionally `LSL #12`, or the shifted
  register form with `LSL #0` of two of x16 and x17), `LDR` (64-bit
  literal), and `MOVZ`/`MOVK`/`MOVN`, each writing x16 or x17;
- an end: `BR x16`, `BR x17`, or `B`.

The result is the destination. Anything else means the code is not a
veneer. Every reference linker's veneer shape is handled: GNU ld's
`ADRP; ADD; BR` and its 24-byte long veneer `LDR x16, .+16; ADR x17, .;
ADD x16, x16, x17; BR x16; .xword` (seen at 256 MiB, `spec/a64.md`
§8.7; WP-028b), lld's `LDR literal; BR; .xword`, and mold's
`ADRP; ADD; BR`.

**`spec/a64.md` §8.7's reference forms** (WP-028b) are decided so: GNU
ld's `nop` for a branch to an undefined weak is W4, and its branch to
itself for `CONDBR19`/`TSTBR14` is §4.4's X = A (a PC-relative
reference to an undefined weak resolves to the place), so it is a
direct verification, not a rewriting. The 16-byte veneer and lld's
`AbsLongThunk` are followed here; the long veneer too, now. lld's GOT
load to `nop; adr` is W3, mold's to `adrp; add` is W2, mold's TLSDESC
to IE in a static link is W11, and mold's unrelaxed traditional GD is
ordinary records (§4.7). IE to LE with `Rt ≠ Rn` is the anomaly A8;
mold's missing 843419 fix is A4; GNU ld's fix with an ADR is W7.

**Chained veneers and landing pads (WP-028d).** A veneer may end in a
branch to another veneer, and the emulator follows up to 4 of them: GNU
ld's `__x_veneer` (`ADRP; ADD; BR x16`) to a padless target in a BTI
output goes to `__x_bti_veneer` (`BTI c; B x`). A destination that is a
landing pad at the target minus 4 also reaches the target: lld 22 puts
its `__AArch64BTIThunk_x` (`BTI c`) just before a padless `x` and falls
through. A destination inside a placed input section other than the
target is not a veneer's.

- **R9.** In an output marked BTI (`GNU_PROPERTY_AARCH64_FEATURE_1_AND`
  bit 0, AAELF64 "Program Property"), every veneer hop that ends in `BR
  x16`/`BR x17` lands on a landing pad: `BTI c`, `BTI j`, `BTI jc`,
  `PACIASP` or `PACIBSP` (Arm ARM "BTI"; a plain `BTI` is no pad for an
  indirect branch). A direct `B` needs none. The finding is at the
  hop's start; the record itself is still judged by §5.2.

Observed: ld.lld 19 (the distribution's default `ld.lld`, not the
corpus's lld 22) ends its veneer to a padless `x` in a BTI output in
`LDR x16; BR x16` straight to `x`: R9 (it faults under BTI
enforcement). lld 22, GNU ld and mold do not.

Test: `a64::tests::s5_4_veneers_are_followed`,
`tests/mutation.rs s9_2d_retargeted_veneers_are_found`,
`s9_2n_a_veneer_to_no_landing_pad_is_found` (the `bti` case: `padded`'s
`BTI c` made `NOP`, in the output and the input, is exactly R9 for GNU
ld and lld; mold writes no property note on AArch64, so nothing is
guarded).

### 5.5 GOT slots (R4)

A static executable keeps its GOT, but nothing in the inputs says where
each slot is. The slot is therefore found from the fields and checked by
content:

1. **Group.** Records of GOT operations are grouped by (object, symbol
   index, addend). This happens after any W2/W3 check, and a record
   accepted by those leaves the group.
2. **Evidence.** Each record gives evidence for G:
   - `GOT_LD_PREL19` gives G exactly (P + the decoded field);
   - `LD64_GOTOFF_LO15` gives G exactly (GOT + the field);
   - `LD64_GOTPAGE_LO15` gives G exactly (Page(GOT) + the field);
   - `ADR_GOT_PAGE` gives the page (the ADRP's page, or a W7 ADR's);
   - `LD64_GOT_LO12_NC` gives the low 12 bits.
3. **Candidates.** The candidates are every exact value, and every page
   combined with every low part.
4. **Valid slots.** A candidate is a valid slot when it is 8-aligned,
   outside every placed input section, inside the output's `.got` when
   the output has one, and holds the 8 bytes S+A.
5. **Decision.** Each member is decided against the valid slots:
   - **verified (`got-slot`)** if, for some valid slot, the verdict (§5.2)
     is verified;
   - **R4** if there are no valid slots;
   - **R1, R2 or R3** if no valid slot verifies it;
   - **`got-unpaired`** if the group has pages and no low parts, or the
     reverse, and nothing exact.

Test: `tests/mutation.rs s9_2c_corrupted_got_slots_are_found`.

### 5.6 Mergeable pieces (R6)

A foreign map does not say where a merged piece went. For a reference into
a *merged* section:

1. **Name the piece.** The input offset is r = `st_value + A` for a
   section symbol, with extra = 0. For any other symbol, r = `st_value` and
   extra = A. The value referred to is T(r) + extra, where T(r) is the
   output address of input byte r. A section-symbol reference with r
   outside `[0, sh_size)` names no piece: on a reference output (GNU
   writes the end of the output section) it is `piece-not-locatable`; it
   cannot occur in mortar's (synth.md S-18 refuses it), so there it is R6
   (synth.md R-P2, WP-028b).
2. **Group.** Records are grouped by (defining object, section, r, extra).
3. **Evidence.** From each record's output field:
   - an exact value from `ABS` data, from `PREL` data and branches, from
     ADR, or from a W1 pair's ADR;
   - a page from ADRP;
   - low bits from `LO12`.

   The GOT operations and the MOVW groups give no evidence for a merged
   target (`piece-not-locatable`).
4. **Candidates.** As in §5.5.
5. **Valid candidates.** A candidate v is valid when the output at
   v − extra holds the piece from r to the end of the piece:
   - for `SHF_STRINGS`, the bytes up to and including the terminating NUL
     character (`entsize` bytes);
   - otherwise, the rest of the `entsize`-byte entry.

   This allows tail merging, where a string's suffix is shared.
6. **Decision.** Each member is decided as in §5.5:
   - **verified (`piece`)** if some valid candidate verifies it;
   - **R6** if there are no valid candidates;
   - **R1, R2 or R3** otherwise;
   - **`piece-not-locatable`** if there are no candidates at all.

### 5.7 Discarded targets (R5)

A relocation of a placed section whose symbol's section is discarded or
absent is R5. This does not apply to `.eh_frame`'s own records, which
§6.4 checks (an FDE whose function is discarded is dropped, not R5), nor
to non-alloc records (§5.8).

### 5.8 Non-alloc relocations (stage 2, WP-028b)

A non-alloc input section (`.debug_*`, `.comment`, …) is placed at an
offset of the non-alloc output section of its own name (every reference
linker keeps the names). Its place is `sh_addr` of that section (0 in an
executable) plus the offset:

- GNU ld's and lld's maps give the offset as the row's address;
- mold's map gives every non-alloc row the address 0. The offset is
  computed as mold lays the section out: the rows of an output section
  in map order, each at the next multiple of its input alignment. S12
  confirms it by content;
- mortar's map gives it with `nplace` (§3.2).

A mergeable non-alloc section (`.debug_str`, `.debug_line_str`) is
*merged* for a foreign map, as an allocated one is, and its references
are judged by content (§5.6) in the file bytes of the output section.

The records of a placed non-alloc section are judged like any other
(§5.1): S from §4.4 (a target in a non-alloc section is at its place
plus `st_value`, so a `.debug_info` reference to `.debug_abbrev` is an
offset), P is the place plus `r_offset`, and the output's field is read
from the output section's file bytes. S12 compares the section's bytes
outside its relocated ranges in the same way.

**Not judged.** A record whose input section, or whose output section,
is `SHF_COMPRESSED` (`--compress-debug-sections`) is not checked
(`compressed`), nor is its S12. mold's map lists no input rows for a
compressed output section (only the output section, at its compressed
size); a non-alloc input section absent from such a map whose output
section of the name is compressed counts as `compressed` too, not as
absent. An output with no non-alloc section of
the name (a stripped `-S` output) makes the records
`section-not-locatable`.

**A target not in the output.** A data record (`ABS64`, `ABS32`,
`PREL32`, …) of a non-alloc section whose symbol's section is discarded
or absent holds the linker's *tombstone*, compared in the field's width:

| referring section | GNU ld | lld, mold | mortar (`spec/image.md` I-47) |
|---|---|---|---|
| `.debug_ranges` | 1, or the redirect | 1 | 1 |
| `.debug_loc` | 0, or the redirect | 1 | 1 |
| `.debug_rnglists`, `.debug_loclists`, `.debug_addr` | 0, or the redirect | 0 | 0 |
| `.debug_aranges` | 0, or the redirect | 0 | all ones, and the tuple's length 0 (I-112) |
| `.debug_frame` | 0, or the redirect | 0 | all ones |
| other debug names | 0, or the redirect | 0 | all ones (ELFCLASS64), 0 (ELFCLASS32) |
| `.gcc_except_table` | 0, or the redirect | 0 | 0 |
| any other name | 0, or the redirect | 0 | R5: mortar refuses the link (I-46) |

- **GNU ld's redirect**: for a dead section of a group (`SHF_GROUP`, a
  dropped COMDAT copy), the place of a placed group section of the same
  name and size, plus the symbol's `st_value` and the addend: GNU ld's
  "kept copy when the sizes are equal". A collected section (no group)
  has no redirect.
- **Per-linker rules, not anomalies.** Each column is its linker's
  documented convention and is accepted from that linker only (by map
  format); none is a miscompile by the standards' text, since DWARF ≤ 5
  defines no tombstone. Mortar's column is image.md's exactly; the
  evidence for preferring it (debugger behaviour, E9, E10, E16, E17) is
  image.md's §6.1. GNU ld's 0 in `.debug_aranges` and elsewhere, and
  lld's and mold's 0, are the conventions image.md I-47 rejects for
  mortar, so they are recorded here and in §10.6 rather than accepted
  from mortar.
- **I-112** (mortar only): when the record is in `.debug_aranges`, its
  type is the class's absolute address-sized type of the output's
  machine (`R_AARCH64_ABS64` on `EM_AARCH64`, `R_X86_64_64` on
  `EM_X86_64`, `R_386_32` on `EM_386`), its target section is
  `SHF_ALLOC`, and no record has its field right after this one, the
  `width` bytes after the field (the field's own width: 8 in ELFCLASS64,
  4 in ELFCLASS32) are 0. S12 accepts those bytes. (Until WP-030d the
  acceptance was ELFCLASS64 only, and mortar's i386 output, written as
  I-112 says, was an S12 finding; test `tests/aranges.rs`.)
- A non-data field in a non-alloc section with a dead target, or a dead
  target from a non-tombstone name in a mortar link, is R5. A value
  other than the column's is R1.

Verified tombstones count under `tombstone` (§7.2).

Observed (AArch64, GCC 14 `-g -O0`, the corpus's `debug-comdat`, and
`-gdwarf-4 -O2 --gc-sections`, `debug-gc`): GNU ld redirects the dropped
COMDAT `big`'s `.debug_info`, `.debug_aranges`, `.debug_rnglists` and
`.debug_line` references to the kept copy (0x40073c), writes 1 in
`.debug_ranges` and 0 in `.debug_loc` for a collected function; lld and
mold write 0, and 1 in `.debug_ranges`/`.debug_loc`. No linker changes
the aranges length.

Tests: `tests/tls.rs s5_8_debug_references_verify_clean`,
`tests/mutation.rs s9_2j_debug_mutations_are_found`.

### 5.9 x86-64 rewritings (WP-028c)

A rewriting is matched on the bytes of the record's window in the input
section and the output (x64.md §8.6), and accepted only in the forms the
map's linker writes (x64.md §8.8, §14.1). Under mortar's map only
mortar's (x64.md §8). Every form below was linked by each reference
linker in `tests/x64/relax.s` (the `relax` case of `tests/x64.rs`,
which asserts each family is accepted):

| form | mortar | GNU ld 2.44 | lld 22 | mold 2.37 |
|---|---|---|---|---|
| X1 GOT `mov` → `lea` | `GOTPCRELX`/`REX_GOTPCRELX`, local | also plain `GOTPCREL`, local | as mortar | every `REX_GOTPCRELX` `mov`, also abs and undefined weak (X-22) |
| X4 `mov` → `mov $S` (`REX' c7 c0+r`) | — | `GOTPCRELX`/`REX_GOTPCRELX`, S fits | — | — |
| X2 `call`/`jmp *GOT` → `addr32 call` / `jmp; nop` | local | local, undefined weak | local | `rex call`/`rex jmp`, local or undefined weak |
| X3 binop/`test` → immediate (`REX' 81 /n`, `f7 c0+r`) | REX.W, local | REX.W or none; local, abs, undefined weak | REX.W, local | — |
| IE `mov` → `mov $T` (I1) | yes | yes | yes | per symbol, only when no reference to it is an `add` (otherwise all keep the GOT slot) |
| IE `add` → | `add $T` (I2) | `lea T(%r)`, `add $T` for `%rsp`/`%r12` | as GNU ld | kept (a GOT slot holding T) |
| TLSDESC `lea` → `mov $T`; `call *(%rax)` → `66 90` | yes | yes | yes | yes |
| GD → LE | X-25's 16 bytes `mov %fs:0,%rax; lea T(%rax),%rax`; large model the same and a 6-byte `nopw` | the same | the same, but W4 over the large-model 22-byte sequence (A12) | `add $T,%rax` for the `lea`; large model the same and `nopw` |
| LD → LE | `66 66 66 mov %fs:0,%rax` (four `66` for a `call *GOTPCREL`; 13-byte NOP first in the large model) | the same | the same; the large-model sequence refused | `xor %eax,%eax; mov %fs:(%rax),%rax; sub $tcb,%rax` (and the large-model variant) |
| `_TLS_MODULE_BASE_` descriptor T | 0 | 0 | 0 | TpRel(tls.start) |
| undefined weak TLS T | 0 | TpRel(0) | 0 | TpRel(0) |
| `DTPOFF*` in code / alloc data / non-alloc | TP / DTP / DTP | TP / DTP / DTP | TP / TP / DTP | DTP / DTP / DTP |

**X-48 (WP-028d).** In mortar's static `ET_EXEC` output (no
`PT_DYNAMIC`, no `PT_INTERP`), a `PLT32`, `PC32`, `GOTPCREL`,
`GOTPCRELX` or `REX_GOTPCRELX` record against `__tls_get_addr` in an
executable section is R8, "a call to __tls_get_addr survives in a
static output" (x64.md X-48: every GD/LD sequence is rewritten there).
The references are not judged (GNU ld keeps a defined
`__tls_get_addr`'s call). Test: `tests/x64.rs
x64_x48_no_tls_runtime_call_in_a_static_output`.

**x86-64 constructs, judged or not (WP-028d item 3).** Every construct
WP-029b's linker writes has a rule and a mutation that it catches
(§9.5's x86-64 sweep), except these, which are **not judged**:

- `.got` slots no reference reaches after relaxation (X1/X2/X3 keep the
  slot): their contents are dead, so a swapped pair is an equivalent
  mutant (§9.5); judging them needs the map to name each slot's owner.
- the non-`PT_LOAD` segments' `p_flags` and `p_align`, and
  `PT_GNU_RELRO`'s `p_filesz` (the loader reads `p_memsz`);
- `PT_GNU_STACK`'s `PF_X` (§11 item 11);
- the large-model sections' placement (`placement_not_judged`).

A record whose rewriting cannot be judged (its symbol unresolved, as in a
stripped output; an unusable `PT_TLS`) is not checked, and its rewrite
window (`GOTPCREL*` [P−3, P+5); `GOTTPOFF`/`GOTPC32_TLSDESC` [P−3, P);
`TLSDESC_CALL` [P, P+2); `TLSGD`/`TLSLD` [P−4, P+18)) is left out of S12.

### 5.10 Erratum 843419 by site (WP-028d)

W7 and W8 (§5.3) judge a fix's **meaning** where the words are judged:
the patch's first word is the record's relocated word (R1), or the input
word when the final word has no record (S12); the patch must return to
`F + 4` or it is not followed and the branch at `F` is R1/S12; an `ADR`
over an `ADRP` must compute the page the relocated `ADRP` computes (R1),
or, for an `ADRP` without a record, the page the input word computes at
its output address (S12). This section judges that the fixes are **where
the sites are** (`src/erratum.rs`, from `spec/a64.md` §11.3), on AArch64
outputs:

1. **The words.** A site reads the bytes `[P + 0xFF8, P + 0x100C)` of
   each page `P`: `W1` at `0xFF8` or `0xFFC` and the three words after
   it, which cross the page. The window can start in the section before
   the one that holds its last words (WP-032e found mortar reading such
   words unrelocated). Each word is taken from the input section placed
   there, with its class from the input's mapping symbols (a64.md §11.1
   condition 5: `$x`, `$d`, or unproved before the first). A word in no
   input section (padding, a synthetic chunk) breaks every candidate
   through it.
2. **Undo the fixes.** At `F` (page offset `0x000`, `0x004`, `0x008`), a
   `B` to a patch that §5.3's W8 conditions accept, over an input
   load/store with an unsigned offset, is a redirect: the patch's first
   word is the pre-fix word. At `Q` (`0xFF8`, `0xFFC`), an `ADR Xd` over
   an input `ADRP Xd` is an ADR fix: the input word is the pre-fix word.
   Other words are their output words (§8 rewritings applied, so a
   relaxed `ADRP` is not a `W1`).
3. **The sites** are a64.md §11.1's on the pre-fix words, both forms.
4. **The option.** No map format records `--fix-cortex-a53-843419`. The
   caller states it (`Map.fix_843419`: `xtask plumb
   --fix-cortex-a53-843419`/`--no-fix-cortex-a53-843419`, or read from
   the `--inputs-from` response file); otherwise any fix found shows it
   was given, and a link with no fix leaves its sites not judged
   (coverage `erratum_not_judged`). The map should carry it (request).

The rules (ids C1-C6):

- **C1** (every linker). A site with neither fix when the option is in
  effect. On a reference linker's output only sites both GNU ld 2.44 and
  lld 22 fix are judged (a64.md §11.4: `W2` a single-register load or
  store that does not load `Xn`, or a store pair; `W3` not a branch);
  the others, and unproved candidates, are not judged.
- **C2** (mortar). A site with both fixes, or an ADR fix at a `W1` that
  starts no site (§11.3 item 5).
- **C3** (mortar). A redirect at a word that is no site's `F` (§11.3
  item 4).
- **C4** (mortar). A candidate with an unproved word: mortar must have
  refused (§11.1 condition 5, §11.3 item 7).
- **C5** (mortar, the option in effect). A map chunk in an executable
  section starts at page offset `0xFFC`, `0x000`, `0x004` or `0x008`
  (§11.3 item 8, `spec/layout.md` L-118).
- **C6** (mortar). A fix in a link stated to be without the option.

Coverage counts the sites found with their fix (`erratum_sites`) and the
sites not judged.

**WP-032e's mislinks** (main 4afb6df's linker, eafc050 the fix): both
were already findings of §5.3's rules (R1 on the `ADRP` the `ADR`
replaced, R1 on the patch's load) when the verifier was run on them, as
was the `synthetic-sections` workload's output; the bench harness does
not run the verifier, which is how they went unreported. Reproducers
with each linker: `tests/mutation.rs` (`s9_2o_*`), and the stale patch
load, the ADR to the next page and an undone fix are mutation class
`erratum-fix` (§9.5).

### 5.11 i386 rewritings (WP-028e)

As §5.9, on `spec/x86.md` §8 and §9: a rewriting is matched on the
bytes of the record's window in the input and the output, and accepted
only in the forms the map's linker writes; under mortar's map only
mortar's (x86.md §8, §9). Every form was linked by each reference
linker in `tests/x86/relax.s` (the `relax` case of `tests/x86.rs`,
which asserts each family is accepted); the W rows are `tests/x86/wforms.s`.
"Static" is a static `ET_EXEC`; static-pie is outside stage 1 (S1).

| form | mortar | GNU ld 2.44 | lld 22 | mold 2.37 |
|---|---|---|---|---|
| G1 `GOT32X` `mov` → `mov $S` (`c7 c0+r`) | yes | yes, also absolute and undefined weak (`mov $0`) | — | — |
| G1 as `lea S − GOT(%reg)` (`8d`, base kept) | — | static-pie, local only | — | every based `mov` whose slot no other reference needs, any symbol (X86-27); in static-pie wrong for absolute and undefined weak (W16) |
| G2/G3 binop/`test` → immediate (`81 /n`, `f7 c0+r`) | yes | yes | — | — |
| G4/G5 `call`/`jmp *GOT` → `67 e8` / `e9 … 90` | defined target | defined target; in static also undefined weak, a direct call or jump to 0 (X86-24 says the GOT is kept) | — | — |
| `GOT32` (not `X`) | kept | kept | kept | kept |
| no-base `GOT32`/`GOT32X` (`-fno-pic -fno-plt`) | G + A | G + A | G + A | G − `.got` (W3, R4); a no-base `mov` becomes `lea` (R1) |
| I1–I3 `TLS_IE` → `b8 T`, `c7 c0+r T`, `81 c0+r T` | yes | yes | yes | kept (a GOT slot holding T) |
| I4/I5 `TLS_GOTIE` → `c7`/`81` | yes | yes | I4; I5 as `lea T(%r), %r` (`8d 80+r·9`) | kept |
| D1–D3 GD → LE (12 bytes) | `65 a1 0; 81 e8 −T` | the same | the same | `65 a1 0; 81 c0 T`; D3 written from P − 3 (W4, R1) |
| L1 / L2 LD → LE | `65 a1 0` + `90 8d 74 26 00` / `8d b6 0` | the same | the same | `65 a1 0; 2d Z`, Z the TP offset of the block start negated (L2: and `90`) |
| T1 `TLS_GOTDESC` `lea` → | `8d 05+r·8 T` | the same | the same | `90 b8+r T` |
| T2 `call *(%eax)` → `66 90` | yes | yes | yes | yes |
| `_TLS_MODULE_BASE_` descriptor T | 0 | 0 | 0 | TpRel(tls.start) |
| undefined weak TLS T | 0 | TpRel(0) | 0 | TpRel(0) |
| `TLS_LDO_32` | §5.9's `DTPOFF32` row | | | |

The GD and LD leads require `A` = 0 and the call's `A` of its form (−4
for `PLT32`/`PC32`, 0 for `GOT32`/`GOT32X`, `ModRM` `10 010 rm'` with
rm' ≠ 100), and T1 requires `A` = 0 (x86.md X86-35–X86-37 as ruled on
2026-11-15); another sequence is R8, "an unrecognized … TLS
sequence". A rewriting's window is left out of S12 when the record is
not judged, and so is the window of a slot record against a piece of a
mergeable section (§5.6 does not locate GOT slots of pieces): `GOT32X`,
`TLS_IE`, `TLS_GOTIE`, `TLS_GOTDESC` [P−2, P+4); `TLS_DESC_CALL` [P,
P+2); `TLS_GD`/`TLS_LDM` [P−3, P+10).

## 6. Structure and content

### 6.1 Output and input rules

- **O1.** The output cannot be read: the file header, the program header
  table, the section header table or `.symtab`. It is the only finding of
  its report.
- **I1.** A file the map names is not among the inputs.
- **I2.** An input cannot be read: it is not an object, it is the wrong
  machine, its member is missing or ambiguous, or its relocation section
  is unreadable or a place is out of bounds.

Test: `tests/mutation.rs s6_1_input_and_output_rules`.

### 6.2 Structure rules

These rules are checked from the output alone. Each one has a mutation
that breaks it (`tests/mutation.rs s9_2e_each_structure_rule_is_enforced`).

- **S1.** `e_type` is `ET_EXEC`, and the machine is supported in its
  class. `EI_OSABI` is `ELFOSABI_NONE` (0) or `ELFOSABI_GNU` (3); on
  `EM_386` and `EM_X86_64`, `e_flags` is 0 (the psABIs define none,
  x86.md X86-1). **(mortar)** `EI_OSABI` is 3 exactly when the output has
  an `STT_GNU_IFUNC` or `STB_GNU_UNIQUE` symbol or an `IRELATIVE` record
  in an allocated relocation section (image.md §5.4, a64.md Q11, x64.md
  X-49, x86.md X86-64; WP-028e). The references on i386: GNU ld writes 3
  for every static glibc output and for `ibt` (IFUNC), 0 for the
  `-nostdlib` ones without; lld and mold write 0 (x86.md §15.1).
- **S2.** There is at least one `PT_LOAD`. For each one:
  - `p_filesz <= p_memsz`;
  - `p_align` is 0, 1 or a power of two;
  - `p_vaddr ≡ p_offset (mod p_align)`;
  - its file image lies inside the file;
  - its memory does not wrap (in an `ELFCLASS32` output, it ends at or
    below 2^32);
  - in mortar's output (by map format), `p_align` is at least 4 KiB
    (`spec/layout.md` L-1; layout.md §20.3, WP-028b); on i386 it is
    0x1000, and a static executable's first `PT_LOAD` is at 0x08048000
    (x86.md X86-65, WP-028e).
- **S3.** The `PT_LOAD`s are in ascending `p_vaddr` order and do not
  overlap (gABI "Program Header").
- **S4.** Every allocated section with a size, other than `.tbss`, lies
  within one `PT_LOAD`'s memory. If it has bytes, they lie within that
  segment's file image, at `p_offset + (sh_addr − p_vaddr)`.
- **S5.** A section with `SHF_WRITE` is in a `PF_W` segment, and one with
  `SHF_EXECINSTR` is in a `PF_X` segment. A `PT_LOAD` that holds an
  allocated section is `PF_R`, unless (AArch64) every section in it is
  `SHF_AARCH64_PURECODE` (lld 22's execute-only `.iplt`): a text segment
  without `PF_R` is execute-only where the hardware allows it, and its
  literal pools fault. In mortar's output (by map format) a `PT_LOAD` is
  `PF_W` only if it holds a writable section and `PF_X` only if it holds
  an executable one (L-6 made a rule for mortar; WP-028d).
- **S6.** No two sections overlap. In memory this covers allocated
  sections other than `.tbss`. In the file it covers sections with bytes,
  the file header, the program header table and the section header table.
- **S7.** `e_entry` lies in an executable `PT_LOAD`, or is 0. A 0 is the
  gABI's "no associated entry point": a link with no entry symbol warns
  (`entry-not-found`) and writes 0 (`spec/layout.md` L-103, a binding
  ruling), and the verifier, which does not see the link's diagnostics,
  accepts the 0 whatever the reason (WP-028b). S7 judges `ET_EXEC` only:
  a shared object or `-r` output has no entry rule here (L-104). In
  mortar's output with a `.symtab` that has a function, a nonzero
  `e_entry` is the value of a symbol that is not a mapping symbol,
  `STT_SECTION`, `STT_FILE` or `STT_TLS` (L-103: the entry symbol's;
  WP-028d). The symbol may be local: glibc's `_start` is hidden, and a
  hidden symbol is local in the output. The references fall back to the
  start of `.text` when there is no entry symbol, so the clause is
  mortar's only; an `-e` given as an address with no symbol there would
  be its one false finding.
- **S8.** Every `.symtab` symbol defined in an allocated section, with
  `st_size > 0` and not `STT_SECTION`, lies within its section:
  `[st_value, st_value + st_size] ⊆ [sh_addr, sh_addr + sh_size]`. For a
  TLS symbol, `st_value + st_size <= p_memsz` of `PT_TLS`, and a TLS
  symbol requires a `PT_TLS`. Labels (`st_size == 0`) are checked by Y1.
- **S9.** There is at most one `PT_TLS`, its initialization image lies in
  a `PT_LOAD`'s file image, its `p_align` is a power of two, and its
  `p_vaddr` is a multiple of `p_align` (§4.7). Every
  `SHF_TLS` section lies inside it, and TLS sections require a `PT_TLS`.
  It is exactly the TLS sections (gABI "Thread-Local Storage"; WP-028d):
  `p_vaddr` is the first one's address, `p_filesz` reaches the end of the
  last one with bytes, `p_align` is their largest `sh_addralign`, and
  `p_memsz` is at least their span and at most the span rounded up to
  `p_align` (lld rounds it, since variant 2 puts the thread pointer after
  the block). A wrong `p_memsz` moves every x86-64 TP offset; a wrong
  `p_align` moves every AArch64 one (§4.7's `tcb`).
- **S10.** There is at most one `PT_GNU_STACK`. **(mortar, i386)** Its
  `p_align` is 16 (x86.md X86-66).
- **S11.** Every `PT_NOTE` lies in the file and is a sequence of whole
  notes (gABI "Note Section"), with padding to 4, or to 8 when `p_align`
  is 8. The padding after a descriptor is there "to ensure 4-byte
  alignment for the next note entry", so the last note of the segment
  may end at its descriptor: GNU ld, lld and mold all write a 6-byte
  `--build-id=0x…` so (`p_filesz` 0x16), and the verifier accepts it
  (WP-028d; before, it was an S11 finding). A `PT_NOTE` is its note
  sections: it starts at the first and ends at the end of the last,
  `p_memsz` = `p_filesz`, and `p_align` is their largest `sh_addralign`
  (at least 4): glibc ignores a property note under a `p_align` other
  than 8. `PT_GNU_PROPERTY` is exactly `.note.gnu.property` (address,
  offset, size, `p_memsz` = `p_filesz`, its alignment): the loader finds
  the properties through it (Linux "Program Property").
- **S12 (content).** Take every input section placed whole that is
  allocated, has a size, and is not mergeable, `.eh_frame`, `.tbss` or
  `.note.gnu.property`. Its output bytes must equal its input bytes
  outside its relocated ranges (§6.3). The exceptions are a word that is
  an erratum-patch branch (W8) and the last word of a W12 sequence. A `SHT_NOBITS` section's memory must read
  as zero. `.note.gnu.property` is excluded because linkers combine the
  property notes by definition (gABI "Program Property").

The rules S13 to S18 are `spec/layout.md` §20.3's requests (WP-028b),
renumbered where layout.md's ids were taken (its S17 is P2, §6.6; its
S18 is S17 here; its T1 is S18):

- **S13.** At most one `PT_PHDR` and one `PT_INTERP`, each before every
  `PT_LOAD`; `PT_PHDR` lies in a `PT_LOAD`'s file image, and so does the
  program header table (L-2, L-3: glibc's static start-up finds
  `PT_TLS` through `AT_PHDR`). `PT_PHDR` is the table: `p_offset` =
  `e_phoff`, `p_filesz` at least `e_phnum × e_phentsize` (lld leaves room
  for headers it does not write) and `p_memsz` = `p_filesz` (WP-028d).
- **S14.** An allocated `SHT_NOBITS` section with a size, `.tbss` aside,
  lies at or past `p_vaddr + p_filesz` of its `PT_LOAD` (L-4).
- **S15.** Every section with `sh_addralign > 1` has `sh_addr ≡ 0 (mod
  sh_addralign)` (L-5).
- **S16 (RELRO safety).** For each `PT_GNU_RELRO` `[s, e)` and each page
  size P in {4, 16, 64} KiB not above the largest `PT_LOAD` `p_align`
  (4 KiB always), `[align_down(s, P), align_down(e, P))` holds no byte of
  a writable allocated section outside `[s, e)` (L-8). Since
  `align_down(e, P) ≤ e`, that is: no writable byte in
  `[align_down(s, P), s)`. A writable section that `[s, e)` covers only in
  part is RELRO in that part by the linker's choice: GNU ld ends RELRO 24
  bytes into `.got.plt`, after its reserved words, which a static
  program never writes (observed, `c-hello`).
- **S17.** No non-allocated section with file bytes overlaps a
  `PT_LOAD`'s file image (L-18).
- **S18 (mortar only).** The end of every `PT_LOAD`'s memory is the end
  of a section, or of the headers (L-9). The reference linkers do not
  promise it (strip rebuilds `p_memsz` from the sections, E-14), so S18
  judges mortar's map format only.

- **S19 (x86, WP-028c).** The bytes between two placed input fragments
  of `.init` or `.fini` run at start-up and must be no-operation
  instructions (X-50): any NOP sequence for a reference linker, all 0x90
  for mortar. lld and mold pad with `int3` (W8, A16). (This S19 is the
  verifier's own id, not layout.md §20.3's S19 below.)

- **S20 (RELRO extent, WP-028d).** A section that starts inside a
  `PT_GNU_RELRO` ends inside it (`.got.plt` aside, S16), and `.got`,
  `.init_array`, `.fini_array`, `.preinit_array`, `.dynamic` and
  `.data.rel.ro*` lie inside it when there is one. Its end is the end of
  a section, a multiple of 4 KiB (GNU ld pads it to the page), or inside
  the reserved head of `.got.plt` (three words, S16). A RELRO cut short
  leaves the end of `.got` writable; one run long makes writable data
  read-only. (S16 judges the bytes below its start.)
- **S21 (WP-028d).** `PT_GNU_EH_FRAME` is exactly `.eh_frame_hdr`
  (address, offset, size, `p_memsz` = `p_filesz`): the unwinder finds the
  search table through it (LSB "Exception Frames").

Not taken from layout.md §20.3: its S19 (`ET_DYN` alignment) has no
output to apply to while S1 accepts `ET_EXEC` only, and is stage 3; its
S20 (`PT_TLS` alignment) is S9; its Y2 (the start-up symbol contract,
L-13 to L-15) is Y5's table; the RWX `PT_LOAD` note of L-6 and its notes
A7 and A8 are §10.2 notes, since they are never findings (their ids are
taken, §10.2).

### 6.3 Relocated ranges

The relocated ranges of a section are the bytes
`[r_offset, r_offset + width)` of each of its records, where width comes
from the type (§4.2). A TLS type counts as 4 bytes, and an unknown type
as 8.

### 6.4 Call frame information (stage 2)

The output's allocated `.eh_frame` is read as the LSB defines it
("Exception Frames": `.eh_frame` and `.eh_frame_hdr`), with DWARF 4 §6.4
("Call Frame Information") for the instructions. The inputs' allocated
`.eh_frame` sections are read the same way, whatever their place (§3.3):
the check is by record, not by address, so GNU ld's edited `.eh_frame`
(rewritten), lld's pieces and mold's unlisted sections are checked alike.
Test: `tests/eh.rs`, `src/eh.rs` unit tests.

**Reading.** A record is a 4-byte length (0xffffffff: an 8-byte length
follows; 0: the zero terminator), then an id, 0 for a CIE and otherwise
the distance back from the id field to the record's CIE. A CIE has:

- a version, 1 or 3;
- an augmentation string, empty or `z` followed by any of `R`, `P`, `L`,
  `S` (signal frame), `B` (AArch64 return addresses signed with the B
  key) and `G` (MTE-tagged frames);
- a code alignment (ULEB128), a data alignment (SLEB128) and a return
  address register (a byte in version 1, ULEB128 in 3);
- for `z`, the augmentation data: its length, then the `R` encoding of
  the FDE pointers, the `P` encoding and personality pointer, and the `L`
  encoding of the LSDA pointers, in the string's order;
- the initial instructions.

An FDE has the initial location (in the `R` encoding), the address range
(in its format, with no application), for `z` the augmentation data
length and the LSDA pointer when the CIE has `L`, and the instructions.
Pointer formats are `absptr`, `uleb128`, `udata2`, `udata4`, `udata8`,
`sleb128`, `sdata2`, `sdata4` and `sdata8`. Applications are absolute and
`pcrel` in `.eh_frame`, and also `datarel` (from the start of
`.eh_frame_hdr`) in the header. The `indirect` bit is kept, so a
personality is the address of the slot that holds it, which is what the
input's relocation gives too. Instructions are compared without their
trailing `DW_CFA_nop` padding. Anything else is unreadable.

**The inputs' pointers.** A pointer field of an input record is the value
of the relocation at its offset, resolved as §4.4 does (S + A for
`ABS64`, `ABS32`, `PREL64` and `PREL32`). A relocation of type
`R_*_NONE` is no relocation (it counts under `none`). If there is no
relocation, an absolute nonzero value is taken as written; an initial
location of 0 makes the FDE **dropped** (it describes no code of the
link, `synth.md` S-38); anything else cannot be judged (synth.md R-P7,
WP-028b). An input FDE is:

- **dropped** if its initial location resolves into a discarded or
  absent section, or its address range is 0 (it covers no code: GNU ld
  drops it, synth.md S-59; glibc's i386 `clone3.o` has one, WP-028e). Its records are counted with the records of sections not
  in the output, and the FDE in `fdes_dropped`;
- **unknown** if any of its pointers cannot be resolved (`eh-frame`, or
  the §4.4 reason). Its records are not checked, and E4 is not reported
  for the output (an unknown input could be the FDE it reports);
- **live** otherwise.

**The rules.** Each finding is at the output record's address, or the
`.eh_frame_hdr` address, and is sited at the input record when there is
one.

- **E1.** The output's `.eh_frame` cannot be read: a record runs past the
  section, a CIE pointer does not point at a CIE, there is an unknown
  version, augmentation or encoding, or a record follows the zero
  terminator. Also, an input ends its frames with a zero terminator
  (crtend.o's `__FRAME_END__`, the end of the list `__register_frame_info`
  walks) and the output has none.
- **E2.** A live input FDE has no output FDE with its initial location.
- **E3.** The output FDE at its initial location differs from it. The two
  must have the same address range, augmentation data length and
  instructions, the same LSDA value, and CIEs with the same version,
  augmentation string, alignments, return address register, initial
  instructions and personality value. The encodings may differ; the
  values may not. Each output FDE answers for one input FDE only, so a
  duplicate input FDE needs a duplicate output FDE.
- **E4.** An output FDE that no live input FDE gives, unless it is
  neutralised (initial location 0 or range 0, counted in
  `fdes_neutralised`).
- **E5.** `.eh_frame_hdr` (only when there is one, or a
  `PT_GNU_EH_FRAME`):
  - exactly one `PT_GNU_EH_FRAME` covers exactly the section;
  - the version is 1, and `eh_frame_ptr` is the address of `.eh_frame`;
  - `fde_count` is the number of output FDEs that are not neutralised;
  - the table is sorted strictly by initial location;
  - each entry gives an FDE's address and that FDE's initial location;
  - every FDE that is not neutralised is in the table.

  The `DW_EH_PE_omit` table that the LSB allows is accepted, without a
  table.

**Coverage.** The relocation records of an input `.eh_frame` are counted as
follows:

- the initial-location and LSDA records of a matched FDE are verified
  (`eh-frame`);
- a personality record is verified when one live FDE of its CIE is;
- E2 and E3 fail the FDE's records;
- a record at no pointer field of a CIE or FDE is I2;
- a CIE whose FDEs are all dropped has its personality record counted as
  not in the output.

`fdes_checked`, `fdes_dropped`, `fdes_neutralised` and
`eh_frame_hdr_entries` are reported.

**Not judged.** These are reported by name:

- relocations from other sections to a label inside an input `.eh_frame`
  (crtbeginT.o's `__EH_FRAME_BEGIN__`, 4 records per glibc link):
  `eh-frame`. Their value is where that input's frames begin in the
  output, which a rewritten or unlisted `.eh_frame` does not say;
- the instructions' meaning (they are compared byte for byte, not
  interpreted);
- the LSDA's contents (`.gcc_except_table` is checked by S12 and its
  relocations by §5);
- the frames of other machines (`machine`).

### 6.5 The symbol table (stage 2)

`.symtab` is what gdb, lldb, `nm`, `addr2line` and the profilers read. A
symbol that names the wrong section or has the wrong size misleads them
as surely as a wrong value. Y1 (§4.6) judges the values of globals. The
rules below judge the rest. Code: `src/symbols.rs`.

- **Y2. The tables.**
  - `e_shstrndx` (through `sh_link` of section 0 for `SHN_XINDEX`), when
    it is not 0, names an `SHT_STRTAB` section that begins and ends with a
    NUL.
  - `.symtab`, when there is one: `sh_entsize` is 24 (ELFCLASS64) or 16
    (ELFCLASS32), and `sh_size` is a multiple of it. `sh_link` names an
    `SHT_STRTAB` that begins and ends with a NUL. Entry 0 is all zeros.
    No local follows the first non-local entry. `sh_info` is the index of
    the first non-local entry (gABI "Symbol Table").
- **Y3. Globals.** This rule applies to each name with a global, weak or
  unique definition in the inputs whose output symbol (as Y1 chooses it)
  is defined. The definition D is the placed definition at the output
  value. A strong one comes before a weak one. When the value is no
  placed definition's, the name is Y1's finding and Y3 does not judge it.
  - When the name has a strong definition, the value is a strong
    definition's: a weak definition does not override a global one.
  - The visibility V is the most constraining one over every non-local
    input entry of the name, defined or undefined (gABI "Symbol
    Visibility"). If V is hidden or internal, the output symbol is
    `STB_LOCAL`, or keeps a hidden or internal visibility (GNU ld's form,
    §10.5). Otherwise its binding is `STB_WEAK` when every definition is
    weak, and else D's binding. `STB_GNU_UNIQUE` may become `STB_GLOBAL`.
    Its visibility is V.
  - Its type and size are D's.
  - Its `st_shndx` names an allocated section whose range holds D's input
    section's place. The range includes its end address: a label at the
    end of a section, or of an empty input section placed at a section's
    end, is held by that section (`synth.md` S-122, S-123, R-P4). For an
    `SHN_ABS` definition it is `SHN_ABS`.

  A name with an IFUNC definition, with no placed definition, or a TLS
  name without a usable `PT_TLS`, is counted in `globals_unchecked`.
- **Y4. Locals.** This rule applies to each defined output local, other
  than section and file symbols, whose name some input local has. The
  input locals of the name are found in this order:
  1. one at the output value, in the output's section, of the output's
     type and size;
  2. one at the output value, in the output's section;
  3. when every input local of the name is placed (§3.3), one at the
     output value.

  The output local must have the input local's type, size and section,
  as in Y3. When none is at the output value, that is a finding, with two
  exceptions, both counted in `locals_unchecked`:
  - some input local of the name has no known place (a `rewritten` or
    `merged` section, such as GNU ld's `.eh_frame`). A local in a
    non-alloc or discarded section has no address in the output and
    does not count (every object's `$d` in its debug sections; WP-028d:
    without this, no output `$x` or `$d` was judged);
  - the symbol is a mapping symbol (`$x…`, `$d…`, AAELF64 "Mapping
    symbols") at an address of an allocated section that no input section
    occupies: the linker's own, marking a veneer or a patch.

  A name the inputs also define globally, or a TLS local without a usable
  `PT_TLS`, is also counted in `locals_unchecked`.
- **Y5. Linker-defined symbols.** This rule applies to each defined output
  symbol whose name no input defines. The names below have these values.
  Here, "the section" means the allocated output section of that name;
  E is the highest end of the writable, non-TLS, non-`NOBITS` allocated
  sections; and M is the highest end of all the writable, non-TLS
  allocated sections.

  | name | value |
  |---|---|
  | `__ehdr_start` | `p_vaddr` of the `PT_LOAD` at file offset 0 |
  | `__executable_start` | the lowest `PT_LOAD` `p_vaddr` |
  | `__init_array_start`/`_end`, `__fini_array_…`, `__preinit_array_…` | the section's start and end; with no section, start = end |
  | `_GLOBAL_OFFSET_TABLE_` | AArch64: the start of `.got`; x86-64 and i386: the start of `.got.plt` (synth.md R-P5) |
  | `__GNU_EH_FRAME_HDR` | the start of `.eh_frame_hdr` |
  | `_edata`, `edata` | in [E, the next allocated section's start] |
  | `__bss_start`, `__bss_start__` | in [E, the first writable `NOBITS` section at or after E] |
  | `_end`, `end`, `__end__`, `_bss_end__`, `__bss_end__` | in [M, M rounded up to 8] |
  | `_etext`, `etext`, `__etext` | in [the end of `.text`, the next non-executable section's start] |
  | `__rela_iplt_start`/`_end` (AArch64, x86-64), `__rel_iplt_start`/`_end` (i386) | the bounds of the only allocated `SHT_RELA` (i386: `SHT_REL`) section; with none, start = end (synth.md R-P6; under `-static-pie` they are not defined, and S1 refuses an `ET_DYN` output) |
  | `_DYNAMIC` | the start of `.dynamic`, else 0 |
  | `_TLS_MODULE_BASE_` | 0, and `STT_TLS` (§4.7) |
  | `__start_X`, `__stop_X` (X a C identifier) | the bounds of section X |

  The data-end symbols allow a range, because the reference linkers place
  them differently within it (§10.5). Every other name is not judged. It
  is listed by name in `linker_defined_not_judged`.

**Coverage.** `globals_checked`, `globals_unchecked`, `locals_checked`,
`locals_unchecked`, `linker_defined_checked` and
`linker_defined_not_judged` (a list of names) are reported. The summary
line is "symbol attributes: …".

**Not judged.** These are reported by name or count:

- mold's own symbols (`$got`, `$gottp`, `$tlsgd`, `$pltgot`, `$thunk`,
  `__start_EHDR`/`__start_PHDR`, which mold sets to 0 even when they are
  referenced);
- `__dso_handle` and other names given by linker scripts, where no input
  defines them;
- veneer and patch names (`__*_veneer`, `__erratum_*`);
- the data-end symbols when there is no writable section;
- locals of archive members the map does not list, which are counted in
  `locals_unchecked` (lld under `--gc-sections`);
- the static-pie `.dynsym`, `.hash` and `.gnu.hash`. They are stage 3,
  with the rest of the dynamic sections (§11 item 7).

Tests: `tests/corpus.rs s9_1_reference_links_verify_clean` (on the glibc
cases: more than 1000 globals, more than 300 locals and at least 9
linker-defined symbols judged; not-judged names only from mold, §10.5) and
`tests/mutation.rs s9_2h_symbol_table_mutations_are_found`.

**x86-64 additions (WP-028c).** A common symbol (`SHN_COMMON`, or
x86-64's `SHN_X86_64_LCOMMON`, X-55) with no non-common definition is
Y3's when its output symbol is not in an allocated, writable
`SHT_NOBITS` section, with the largest size of its definitions and
aligned to the largest alignment (lld's absolute large common is A14,
W9). Y5's `_edata` and `_end` leave out an x86-64 large section
(`SHF_X86_64_LARGE`) placed after every small one (X-54): GNU ld's
`.ldata`/`.lbss` follow `.bss`. Where the large sections go differs by
linker (x64.md §14.1) and no rule judges it: their names are in the
coverage as *placement not judged* (§7.2).

### 6.6 Program properties (stage 2, WP-028b; x86 WP-028c)

`.note.gnu.property` is written from `spec/synth.md` S-94 to S-97 and
S-126, from the observations of synth.md §13, and for x86 from
`spec/x64.md` X-41. It is judged by two
rules; S12 does not compare the section's bytes (§6.2).

**The inputs.** Every input object the map names counts, in map order;
an object without `.note.gnu.property` has every property absent. Every
note of every such section of an object is read (S-94): name `GNU`,
type `NT_GNU_PROPERTY_TYPE_0`, properties `(pr_type, pr_datasz, data)`
padded to 8 (ELFCLASS64) or 4 (ELFCLASS32). Other notes are skipped. A
note or property that runs past its container is I2, and no property is
judged. An object's notes are combined first:

- mortar: S-126, each note as if it were an object (an `And` type
  survives only if every note has it);
- GNU ld and lld: an `And` type is the OR of the notes that have it
  (synth.md X-33); the other types as mortar;
- mold: not observed; an `And` type is not judged when an object has
  several notes.

**P1 (the merge).** For every type the output or an input has, the
output's property is the merge of the objects' by the rule of the
linker that wrote it (by map format):

| type | merge (S-95) | mortar | GNU ld | lld | mold |
|---|---|---|---|---|---|
| AArch64 `FEATURE_1_AND` 0xc0000000 | `And` | merged | merged | merged | dropped |
| AArch64 `FEATURE_PAUTH` 0xc0000001 | `EqualOrAbsent` | merged | not judged | merged | dropped |
| x86 0xc0000002..0xc0007fff (`FEATURE_1_AND`, `UINT32_AND`) | `And` | merged | merged | merged | merged |
| x86 0xc0008000..0xc000ffff (`ISA_1_NEEDED`, `FEATURE_2_NEEDED`, `UINT32_OR`) | `Or` | merged | merged | dropped | merged |
| x86 0xc0010000..0xc0017fff (`*_USED`, `UINT32_OR_AND`) | `OrAll` | merged | merged | dropped | merged |
| x86 `COMPAT_ISA_1_USED` 0xc0000000 | `OrAll` | merged | merged | dropped | dropped |
| x86 `COMPAT_ISA_1_NEEDED` 0xc0000001 | `Or` | merged | merged | dropped | dropped |
| generic 0xb0000000..0xb0007fff / 0xb0008000..0xb000ffff | `And` / `Or` | merged | merged | dropped | dropped |
| `STACK_SIZE` 1, `NO_COPY_ON_PROTECTED` 2 | `Max`, `Present` | merged | merged | dropped | dropped |
| `MEMORY_SEAL` 3 | an option | not judged | not judged | not judged | not judged |
| any other type, x86 | unknown | dropped | dropped (with a warning) | dropped | dropped |
| any other type, AArch64 | unknown | dropped | not judged | dropped | dropped |

The x86 ranges are binutils' `GNU_PROPERTY_X86_UINT32_AND_LO..` /
`_OR_LO..OR_HI` / `_OR_AND_LO..` (X-41; synth.md S-95 as amended by
R-7). WP-028c measured the x86 columns (two objects with 0xc0000000,
0xc0000001, 0xc0008002, 0xc0010002, the unknown 0xc0018000 and the
generic unknown 0x7000): GNU ld ORs 0xc0000001, keeps 0xc0000000 when
both objects have it, and drops both unknown types with "unsupported
GNU_PROPERTY_TYPE"; lld writes no property; mold keeps 0xc0008002 and
0xc0010002 and drops the rest.

- *merged*: the output has the type exactly when the merge gives it, with
  the merge's bytes. `And`: every object has it, ANDed, omitted if 0.
  `Or`: ORed over the objects that have it. `OrAll`: ORed, present only
  if every object has it. `Max`: the largest. `Present`: present if any
  object has it. `EqualOrAbsent`: the one value every object that has it
  has; two different values must refuse the link, so an output is P1.
- *dropped*: the output must not have the type.
- *not judged*: counted (`properties_not_judged`), never a finding.
  `MEMORY_SEAL` follows `-z memory-seal`, which the verifier does not
  see; on AArch64 GNU ld copies or fails on an unknown type depending
  on the inputs' order (synth.md §13), and does not know PAUTH.

These columns are per-linker rules, not anomalies: dropping a property
never claims what an input did not, and each column is accepted only
from its linker. The safety property is the `And` row: an output that
claims BTI, PAC, GCS, IBT or SHSTK that one input lacks is P1 from every
linker.

**P2 (the note and its segment).** When the output has
`.note.gnu.property`, it is the only section of the name, an allocated
`SHT_NOTE` aligned to 8 (ELFCLASS64) or 4, holding exactly one property
note that reads as above; exactly one `PT_GNU_PROPERTY` has its address
and size, and a `PT_NOTE` covers it. Without the section there is no
`PT_GNU_PROPERTY`. (S-96. The order of the properties is not judged:
S-94 accepts a note out of order.)

`synth.md` R-P1's S17 (IRELATIVE bounds) is F1 (§4.8).

Observed (AArch64, GCC 14 `-mbranch-protection=standard`, Debian's
`crt1.o` and `libc.a` all BTI and PAC): GNU ld and lld write `BTI, PAC`
when every object has it, `BTI` when one object has only BTI, and no
section and no `PT_GNU_PROPERTY` when one object has none. mold writes
no `.note.gnu.property` on AArch64.

Tests: `tests/tls.rs s6_6_properties_verify_clean`,
`tests/mutation.rs s9_2k_property_mutations_are_found`.

### 6.7 Mortar's build id (stage 2, WP-028b)

B1 (`spec/image.md` I-72, I-114, request R-8). When the map is mortar's
and the output has an `NT_GNU_BUILD_ID` note (`"GNU\0"`, type 3) in an
allocated `SHT_NOTE` section, with a descriptor of 8, 20 or 32 bytes, the
verifier recomputes I-72's function of the output: the file with the
descriptor's bytes read as zeros, in leaves of `2^20` bytes, with the tag
`mortar-build-id-2\0`. 8 bytes: the `L128` tree's prefix; 20: the `L128`
tree or the SHA-1 tree, either matching; 32: the SHA-256 tree. A
mismatch is a B1 finding at the descriptor's address. Other lengths
(`0xHEX`, `uuid`) are not checked, and the coverage says so. Other
linkers' ids are not checked (image.md §8.1: no tool recomputes them).

**The style from the map (WP-028d).** A `--build-id=0xHEX` of 8, 20 or
32 bytes is the user's, not a hash, and judging it as one is a false B1.
The map says which it is: the synthetic chunk of the note carries the
style as its target, `chunk synthetic .note.gnu.build-id <addr> <size>
<style> 0`, where `<style>` is `default` (`L128`, 20 bytes), `fast`
(`L128`, 8), `sha1` (20), `sha256` (32) or `given` (`0xHEX`, `uuid`).
With a style, only that tree and length are judged (a length other than
the style's is B1), `given` is not judged, and an unknown style is not
judged; the coverage says which. Without one (today's mortar maps,
`chunk synthetic .note.gnu.build-id 4001b0 18`) the lengths are judged as
above. The writer that must carry it is `crates/mortar_image/src/link.rs`'s
synthetic chunk loop, from `LinkOptions::build_id`: `Some(TreeStyle::…)`
gives the tree's name and `None` (a given id) gives `given`. This is a
request to the image package; the verifier reads both forms now.

SHA-1 and SHA-256 are written from FIPS 180-4 and `L128` from I-114's
text, in `src/buildid.rs`; none is shared with `mortar_digest`. The
definition is the one of this mortar version (I-75): a later tag is a
new definition, and B1 must change with it.

Tests: `src/buildid.rs` (FIPS 180-4's vectors, image.md §8.4's `L128`
and tree vectors), `tests/mutation.rs s9_2l_build_id_mutations_are_found`,
`s9_2l_build_id_style_from_the_map`.

## 7. The report

### 7.1 Findings

A finding has:

- the rule id: `O1`, `S1`–`S21`, `I1`, `I2`, `M1`–`M5`, `R1`–`R9`, `T1`,
  `Y1`–`Y5`, `E1`–`E5`, `F1`–`F5` (§4.8), `P1`–`P2` (§6.6), `B1` (§6.7)
  or `C1`–`C6` (§5.10);
- the output address, if it has one;
- the site (input file, archive member, section, offset);
- the relocation type's name and the symbol;
- the expected and found values;
- a one-line message.

Findings are sorted by address, with findings that have no address last,
then by rule, then by site, then by message. The report is a pure
function of its inputs: the same inputs give the same report (test:
`tests/corpus.rs s7_1_reports_are_deterministic`). No iteration over a
hash map decides any order.

### 7.2 Coverage

Every relocation record of a section that is in the output counts exactly
once:

`records = verified + accepted + not_checked + none + failed`

- **verified**, by how S was found: `direct`, `got-slot`, `piece`,
  `eh-frame` (a pointer field of a matched CIE or FDE, §6.4), `iplt`
  (an IFUNC reference, §4.8) or `tombstone` (§5.8);
- **accepted**, by rewriting (W1 to W13; on x86-64 §5.9's `got-to-lea`,
  `got-to-direct`, `got-to-imm`, `got-to-mov-imm` and the TLS ones);
- **not checked**, by reason: `tls`, `tls-sequence`, `ifunc`, `non-alloc`, `eh-frame`, `compressed`,
  `type`, `machine`, `section-not-locatable`, `piece-not-locatable`,
  `in-mergeable`, `symbol-unresolved`, `got-unpaired` or
  `input-unreadable`;
- **none**, for type 0 and type 256;
- **failed**, for records with a finding.

`checked_types` counts the verified and accepted records by type. The
records of sections not in the output are counted separately
(`absent_records`). So are the sections by place, the symbols Y1 checked
and left unchecked, the bytes S12 compared, and the FDEs and header
entries of §6.4, the symbols of §6.5, the IRELATIVE records and IPLT
entries of §4.8, and the property types §6.6 judged and did not judge, what B1 made of
the build id (§6.7), and the x86-64 large sections whose placement no
rule judges (`placement_not_judged`, §6.5).

The four stage-2 reasons are the only ones allowed on the reference
corpus.

### 7.3 Diagnostics

`Finding::to_diagnostic(output)` gives an error of kind `malformed-input`
(`spec/diag.md`) with the text `verifier rule <id>: <message>`, followed
by the relocation, symbol, address and expected/found values. It is placed
at the input section and offset when the finding has a site, and at the
output file otherwise. `Report::render` renders every finding with
`mortar_diag`'s plain style, under the program name `mortar-plumb`.

## 8. `cargo xtask plumb`

```text
cargo xtask plumb <output> --map <file> --map-format gnu|lld|mold|mortar
                  [--inputs-from <response file>|map]
```

The inputs are the files the map names, read from the paths it gives.
Relative paths are taken from the current directory.

With `--inputs-from <file>`, only the map's files whose canonical path
equals that of an entry in the response file are read; the others are I1.
The response file is parsed as GNU ld parses `@file`: words separated by
white space, with single and double quotes and backslash escapes. Words
starting with `-` are dropped, and so is the value of `-o`.

The command prints the rendered findings and the coverage summary. It
exits 0 when the report is clean, 1 on any finding, and 2 on a usage error
or an unreadable output or map.

Tests: `xtask/src/plumb.rs s8_arguments`, `s8_response_files`.

## 9. Validation

### 9.1 Reference links

The corpus is built at test time in a `Scratch` directory. Nothing binary
is checked in.

| case | built by | linked |
|---|---|---|
| `start` | gcc (asm): freestanding `_start`, `.tdata`, sized objects | `-static -nostdlib` |
| `c-hello` | gcc -O2 | `-static` (glibc) |
| `c-hello-clang` | clang-22 -O2 | `-static` |
| `cxx-eh` | g++ -O2: exceptions, iostream | `-static` |
| `fortran` | gfortran -O2 | `-static` |
| `gc-sections` | gcc `-ffunction-sections -fdata-sections` | `-static -Wl,--gc-sections` |
| `comdat` | g++ -O0, two units sharing inline functions and a template | `-static` |
| `veneer` | gcc (asm): a branch to `.fartext` at 0x20000000 | `--section-start`, plus `-Ttext=0x400000` for lld and mold |
| `got` | gcc (asm): GOT forms, relaxable and not, an undefined weak | `-static -nostdlib` |
| `erratum` | gcc (asm): an `ADRP` at `0xff8` and loads | `--fix-cortex-a53-843419` |
| `types` | gcc (asm): every §4.2 type each linker accepts | `-static -nostdlib` |
| `cxx-eh-hdr` | `cxx-eh`'s object | `-static -Wl,--eh-frame-hdr` |

Every case is linked by `ld.bfd` (GNU ld 2.44), `ld.lld-22` and mold 2.37,
each through the gcc driver with `-Wl,-Map`.

For each link (`tests/corpus.rs s9_1_reference_links_verify_clean`):

- there are no findings, other than the anomalies recorded in §10.2;
- `failed` counts only the records of A6, and the coverage sum holds;
- every not-checked reason is a stage-2 reason;
- §6.4: more than 100 FDEs are checked for the glibc C and C++ cases, an
  FDE is dropped for `comdat` and `gc-sections`, the search table has an
  entry for every FDE wherever there is one (mold always, GNU ld and lld
  for `cxx-eh-hdr`), and `cxx-eh-hdr` runs.

`s4_2_every_type_each_linker_accepts_is_checked` asserts that each type
listed in §10.3 is checked for its linker. `s5_3_observed_rewritings_are_accepted`
asserts the rewritings of §10.1.

The thread-local corpus (`Size::Tls`, `tests/tls.rs`) is linked the same
way, by the linkers that accept it, and its glibc programs are run:

| case | built by | linked | asserted |
|---|---|---|---|
| `tls-c` | gcc -O2 (LE, IE), gcc `-fPIC` TLSDESC and traditional GD (not for lld), clang `-fPIC` TLSDESC; alignments 4 to 64 | `-static` | exits 0; clean; the §10.1 TLS rewritings |
| `tls-types` | gcc (asm): every TLS type each linker accepts (§10.3), `_TLS_MODULE_BASE_` | `-static -nostdlib` | clean, nothing not checked, each type checked |
| `tls-ld-64`, `tls-ld-8` | gcc (asm): LD in a block aligned to 64 and to 8 | GNU ld, `-static` | A7: the 64 case exits wrong and has three R1; the 8 case exits 42, clean, W13 |
| `tls-ie-regs` | gcc (asm): `ADRP x4; LDR x5, [x4]` IE, TPREL >= 2^16 | `-static` | A8: GNU ld and lld exit wrong with R1 on both records; mold exits 42, clean |
| `got-ldr-w` | gcc (asm): `LDR w0, [x0, :got_lo12:]` | `-static -nostdlib` | R8 for every linker |
| `weak-prel` | gcc (asm): PC-relative data, `ADR` and `LDR` literal to an undefined weak | GNU ld, lld | clean (§4.4) |
| `eh-frames` | gcc (asm): CIEs `zR`, `zPLR` (personality through `DW.ref`, LSDA), `zRS`, `zRB`; one FDE in a collected section | `-static -nostdlib --eh-frame-hdr --gc-sections` | `tests/eh.rs`: exits 0; clean; nothing not checked; 5 FDEs checked, 1 dropped, 5 table entries, 7 records verified |

### 9.2 Mutations

A clean reference link is broken one way at a time. Each mutation must
produce a finding that is not a recorded anomaly (`tests/mutation.rs`):

- **(a)** The lowest and the highest bit of each relocated field is
  flipped, for every record of the `types`, `got`, `veneer`, `start` and
  `erratum` links, by every linker. All must be found. The places
  that already carry an anomaly (A6) are skipped.
- **(b)** A symbol's `.symtab` value is moved by 4. All must be found.
- **(c)** Each 8-byte word of `.got` is corrupted. A slot the linker's
  relaxation stopped using is not a defect, so the rate is printed. At
  least one slot must be found wherever a record went through one.
- **(d)** Each of the first 3 words of the veneer is corrupted. All must
  be found.
- **(e)** Each structure rule (S1–S12) and map rule (M1–M5) is broken.
  Each must produce its own finding.
- **(f)** Thread-local storage (`tls-types`, every linker): (1) bits 0, 5,
  10, 16, 22 and 31 of every TLS-relocated instruction, relaxed or not,
  are flipped, and all must be found (516/516); (2) `PT_TLS` is moved by
  its alignment, or its alignment multiplied by 4, and each must be found
  (6/6); (3) each `.got` word is corrupted as in (c), and at least one
  must be found where a TLS record went through an entry.
- **(g)** Call frame information (`eh-frames`, every linker; §6.4). The
  mutations are: bit 0 of every byte of every CIE and FDE after the
  length; bit 12 of every initial location and range; every record's
  length plus 4; bit 0 of every byte of `.eh_frame_hdr`. And for
  `tls-c`, which links crtend.o, the last FDE lengthened over the
  terminator. The verifier finds:
  - CIE bytes: 237/240;
  - FDE bytes: 318/318;
  - record lengths: 25/27;
  - header bytes: 156/156;
  - the terminator: 3/3.

  Every rule E1 to E5 finds some mutant. The 5 that are not found are
  equivalent, and the test lists them. The LSDA encoding `sdata4` made
  `sdata2` decodes the same LSDA. The last FDE lengthened over a
  terminator that lld and mold add, which no input asked for, only gains
  `DW_CFA_nop`s.

- **(h)** The symbol table (`tls-types`, `eh-frames` and `got-ldr-w`,
  every linker; §6.5). A mutant counts as found when it adds a finding
  that is not a recorded anomaly and that the clean link does not have.
  (`got-ldr-w` has R8 by design.) The mutations are:
  - Y2: `sh_info` ±1, `sh_entsize` 16, symbol 0's value, the last local
    swapped with the first global, and the first byte of `.strtab` and of
    `.shstrtab`;
  - Y3 and Y4: the size + 1, the type, a section that does not hold the
    value, the binding and the visibility of every symbol the inputs
    define. A local counts only if moving it by 4 gives a Y4 finding;
  - Y5: every judged linker-defined symbol moved by 0x100000.

  The verifier finds Y2 60/60, Y3 184/184, Y4 179/179 and Y5 78/78.
- **(i)** IFUNC (`ifunc`, every linker; §4.8): (1) the lowest and highest
  bit of every relocated field; (2) every IRELATIVE record's type,
  symbol, addend and slot; (3) the words of every IPLT entry that decide
  what it loads and where it branches; (4) the data word of `gf` made
  `lf`'s entry, which must be F3; (5) `__rela_iplt_start` and
  `__rela_iplt_end` each one record short, by every linker (an
  IRELATIVE record outside the bounds, synth.md R-P3), which must be F1.
  All must be found.
- **(j)** Debug information (`debug-comdat` and `debug-gc`, GNU ld and
  lld; §5.8): (1) the lowest and highest bit of every relocated
  non-alloc field, tombstones included, with `MORTAR_PLUMB_FULL=1`
  (588 flips; 371 s in a debug build), and by default one bit of 24
  places spread over each link; all must be found. (2) lld's
  `debug-comdat`, its map rewritten as mortar's (§3.2): every lld
  tombstone is R1 against I-47; the same output with the tombstones
  written as I-47 and I-112 say is clean; one `.debug_aranges` length
  restored is R1 (I-112).
- **(k)** Program properties (`prop-*`, GNU ld and lld; §6.6, synth.md
  R-P3): (1) a feature bit no input has (GCS) set, P1; (2) PAC set where
  one input has only BTI, P1; (3) `PT_GNU_PROPERTY` moved by 8, and made
  `PT_NULL`, P2; (4) read as mortar's link, the property's type made an
  unknown one (0xc0000003), P1; (5) `prop-two` read as mortar's link:
  the OR of one object's notes is P1 against S-126, and the value S-126
  gives is clean.
- **(l)** Mortar's build id (`start`, lld, read as mortar's link; §6.7,
  image.md R-8): (1) lld's own SHA-1 id, B1; (2) the id rewritten by
  I-72's bare and `sha1` trees is clean, and is not judged under lld's
  map; (3) the id's last byte changed, B1; (4) one byte changed at each
  of 16 places spread over the file outside the id, B1 (or O1 where the
  output no longer reads, or S11 where a note header changed; at least
  12 of the 16 by B1).

- **(m)** x86-64 (WP-028c, `tests/x64.rs`): (1) every field bit of
  every relocated place of the `relax` links flipped, each linker: 384/384
  found; (2) each opcode or ModRM byte of a rewritten window changed:
  161/161 (4 places inside an anomaly's window, A12, skipped); (3) a NOP
  byte of GNU ld's `.init` padding made `int3`: S19 1/1; (4) the large
  common `lc` made `SHN_ABS`: Y3 1/1; (5) a bit of a GNU ld C output's
  `ISA_1_NEEDED`: P1 1/1; (6) `PT_TLS` `p_memsz` grown by its alignment:
  1/1; (7) in the `w6` links of GNU ld and lld, an IPLT entry's `endbr64`
  made a 4-byte NOP, and the IFUNC word changed: F5 2/2 each.
- **(o)** (§5.10) The `erratum` link, by each linker: GNU ld's ADR made
  the next page's, and lld's patch load with its offset cleared, are R1;
  either fix undone is C1 with the option stated, and not judged
  without it; mold's unfixed site is C1, A4. Read as mortar's (the map
  rewritten): `W2` made a NOP in input and output is C2 (GNU ld's ADR)
  and C3 (lld's patch), the option stated off is C6, and a chunk at
  `.text + 0x1000` is C5.

### 9.3 Map readers

There are unit tests on excerpts captured from each linker (§3.4 to
§3.6). Every reference map is written in mortar's format and read back
with the same records (`s3_2_reference_maps_round_trip_through_mortar_format`).

### 9.4 Hostile input

These sweeps over the `Small` corpus must never panic (`tests/hostile.rs`):

- truncations of the output at every length up to 1 KiB, and at 400 lengths
  beyond;
- byte overwrites of the output (0x00, 0xff, and the high bit flipped) at
  every header byte and at 600 random positions;
- truncations and bit flips of every input;
- truncations and character substitutions of every map, in its own format
  and in mortar's.

A truncated output, or an input cut short, must also produce a finding.

### 9.5 Mutation coverage (WP-028d)

`src/mutate.rs` takes a correct output and makes single-field edits, one
mutant at a time, in 24 classes: a relocated field off by one, holding
another symbol's value, or another addend; a relaxed sequence put back
to its input bytes (`stale-relaxation`); two GOT words or two
IRELATIVE records swapped; `.eh_frame_hdr`'s table out of order or an
entry wrong; an FDE's `pc_begin`; a `.symtab` value or size; `e_entry`;
`EI_OSABI` or `e_flags` (`header-ident`, S1, WP-028e); a program header's `p_filesz`, `p_memsz`, `p_align` or flags; `PT_TLS`;
a property bit; a linker-defined symbol; two sections overlapping; a
thunk's target; an IPLT entry; an erratum fix (§5.10). A mutant is
**caught** when the report has a finding the unmutated output's report
does not. mortar's build id is resealed after each edit (B1 would
otherwise catch everything). Places that carry a baseline finding are
not mutated. The choice is a SplitMix64 sequence from a fixed seed: the
table is the same on every run. `cargo xtask plumb <out> --map <map>
--map-format F --mutate [--per-class N] [--seed S] [--threads T]`
prints the table and every missed mutant; `tests/mutation.rs
s9_5_mutation_coverage_on_the_small_corpus` runs the small corpus by
every linker and asserts the classes that must have no miss.

**mortar's outputs** (WP-028d, release build, seed default):

| class | AArch64 protoc (40/class) | x86-64 C++ threads (40/class) | x86-64 static C TLS (all) |
|---|---|---|---|
| field-off-by-one | 40/40 | 39/40 | 11446/11448 |
| field-wrong-symbol | 39/40 | 40/40 | 11428/11436 |
| field-wrong-addend | 40/40 | 40/40 | 11436/11448 |
| stale-relaxation | 10/10 | 40/40 | 268/268 |
| got-slot-swap | 40/40 | 7/40 | 13/28 |
| irelative-swap | 9/9 | 40/40 | 43/43 |
| eh-hdr-order, eh-hdr-entry | — (no `--eh-frame-hdr`) | 80/80 | 3254/3254 |
| fde-pc-begin | 40/40 | 40/40 | 1085/1085 |
| symtab-value | 36/40 | 40/40 | 1925/1940 |
| symtab-size | 40/40 | 40/40 | 1934/1940 |
| e-entry | 2/2 | 2/2 | 2/2 |
| phdr-filesz | 10/13 | 16/19 | 16/19 |
| phdr-memsz | 12/13 | 18/19 | 18/19 |
| phdr-align | 4/13 | 12/19 | 12/19 |
| phdr-flags | 12/21 | 12/30 | 12/30 |
| tls-segment | 3/3 | 3/3 | 3/3 |
| property-bit | — | 4/4 | 4/4 |
| linker-symbol | 9/9 | 10/10 | 10/10 |
| section-overlap | 20/20 | 22/22 | 22/22 |
| iplt-entry | 10/10 | 13/13 | 11/11 |
| erratum-fix | 4/4 | — | — |

**The misses, by kind:**

- **Equivalent mutants** (no observable difference): dead `.got` slots
  on x86-64 (relaxed references keep their slot); `GOTPCREL` against
  glibc's `_nl_current_*_used` (every slot holds 1, so another slot or
  symbol gives the same value); the non-`PT_LOAD` segments' flags and
  alignment; `PT_LOAD` `p_align` of `ET_EXEC`; `PT_GNU_STACK`'s and
  `PT_GNU_RELRO`'s `p_filesz`, `PT_GNU_STACK`'s `p_memsz`.
- **Holes, cheap, closed in WP-028d:** the W1 NOP-ADRP form (§5.3), S5
  (flags), S7 (entry), S9 (TLS), S11 (notes), S13 (`PT_PHDR`), S20
  (RELRO), S21 (`PT_GNU_EH_FRAME`), and Y4's mapping symbols (no `$x`
  or `$d` was judged before).
- **Holes, by cost:**
  1. `PT_GNU_STACK` `PF_X` flipped (the stack made executable): needs
     the link's `-z execstack` from the map (§11 item 11). Cheap once
     the map carries it.
  2. Linker-defined symbols' `st_size` (`_GLOBAL_OFFSET_TABLE_`,
     `__init_array_start`, … made 1): no spec states the size; a line
     in synth.md (size 0) makes it a Y5 clause.
  3. Locals in merged sections (`.rodata.str*`: `_nl_C_name`,
     `_itoa_lower_digits`) and in `.eh_frame` (`__EH_FRAME_BEGIN__`,
     `__FRAME_END__`), and `$d` in them: Y4 cannot place them; needs
     the map to carry merged and `.eh_frame` pieces (§11 item 1).
  4. A record into an `.eh_frame` mortar rewrote (crtbegin's `adrp`/
     `add` of `__EH_FRAME_BEGIN__`, AArch64; `R_X86_64_32` at the same
     place, x86-64): `not checked: eh-frame`; same map request.
  5. The x86-64 dead GOT slots, if they should be judged at all: the
     map naming each slot's owner (equivalent today).

**i386 (WP-028e).** `mutate.rs` reads the class's header layout
(`ELFCLASS32` symbol, program and section header offsets, 4-byte words)
and has the i386 forms: `stale-relaxation` puts back the 2 bytes before
a `GOT32X`, `TLS_IE`, `TLS_GOTIE` or `TLS_GOTDESC` field, or a GD/LD
lead's 3 bytes and its call's 6, or a `TLS_DESC_CALL`'s 2; `irelative-swap`
swaps or moves the resolver words at the `R_386_IRELATIVE` records'
places (REL: the addend is the word); `iplt-entry` moves an `ff 25`
word or makes an `endbr32` a NOP. mortar does not link i386 yet
(`tests/link/xfails`), so the table is the references' (`tests/x86.rs
x86_mutation_coverage`: `relax`, `ibt`, `tls-gd-ld`, `tls-desc`, `w5`,
`cxx` by the three linkers, 4 per class, seed default):

| class | applied | caught | | class | applied | caught |
|---|---|---|---|---|---|---|
| field-off-by-one | 72 | 72 | | e-entry | 36 | 18 |
| field-wrong-symbol | 72 | 72 | | header-ident | 54 | 36 |
| field-wrong-addend | 72 | 72 | | phdr-filesz | 72 | 59 |
| stale-relaxation | 60 | 60 | | phdr-memsz | 72 | 44 |
| got-slot-swap | 63 | 62 | | phdr-align | 72 | 20 |
| irelative-swap | 57 | 57 | | phdr-flags | 72 | 20 |
| eh-hdr-order, eh-hdr-entry | 32 | 32 | | tls-segment | 45 | 45 |
| fde-pc-begin | 48 | 48 | | property-bit | 12 | 12 |
| symtab-value | 72 | 57 | | linker-symbol | 67 | 56 |
| symtab-size | 72 | 57 | | section-overlap | 72 | 72 |
| iplt-entry | 40 | 40 | | | | |

Every relocation, relaxation, IFUNC, TLS, property and overlap class is
fully caught; the test asserts it. The misses are the rules that judge
mortar's map only: `e_entry` + 4 (S7's symbol clause), `EI_OSABI` 0 ↔ 3
(S1's mortar clause: the references disagree), `PT_LOAD` `p_memsz`
+ 0x10 and `p_align` (S18, L-1), the non-`PT_LOAD` segments, and as on
x86-64 the locals of merged sections, linker symbols' sizes and mold's
`$got` symbols. The one `got-slot-swap` miss is mold's `.got` words 0
and 1 in `ibt` (not examined).

## 10. The reference linkers, observed

### 10.1 The rewritings each linker performs

| linker | rewritings observed |
|---|---|
| GNU ld 2.44 | W4 (branch to an undefined weak made `NOP`); W6 (`__x_veneer` in `.text.stub`: `ADRP x16; ADD x16; BR x16`); W7 (erratum 843419: the `ADRP` made an `ADR` of the page). No GOT relaxation. With W7, an unused erratum stub is still emitted. |
| lld 22 | W1, only with addend 0, Rd equal, and the pair adjacent; W2 and W3 (GOT relaxation, only when Rt = Rn and the pair is adjacent); W5 (`bl .+4` to an undefined weak); W6 (`__AArch64AbsLongThunk_x`: `LDR x16, .+8; BR x16; .xword`); W8 (`__CortexA53843419_*`: the site becomes `B patch`, and the patch holds the load and `B back`) |
| mold 2.37 | W1, including with an addend; W2, including for an undefined weak (`ADRP` of page 0); W4; W6 (`x$thunkN`: `ADRP x16; ADD; BR x16`). Code padding is `BRK #0x3e8`. |

TLS (§4.7, static executables):

| linker | IE | TLSDESC | traditional GD | traditional LD |
|---|---|---|---|---|
| GNU ld 2.44 | W9, also with mismatched registers (A8); PREL19 and MOVW forms kept (GOT entry) | W10 (`hi; lo; NOP; NOP`) | W12 | W13 with `#16` (A7) |
| lld 22 | W9, also with mismatched registers (A8) | W10 (`hi; lo; NOP; NOP`) | refused (513, 514) | refused (518, 519, 528) |
| mold 2.37 | kept: a GOT entry holding TPREL | W10 (`NOP; NOP; hi; lo`) for local symbols, W11 (`NOP; NOP; ADRP; LDR`) for global ones | kept: a GOT pair (1, DTPREL), `BL __tls_get_addr` | refused |

x86-64 (WP-028c; §5.9 has every form): GNU ld relaxes the most (X4
`mov $S`, X3 for absolute and undefined weak symbols, plain `GOTPCREL`);
lld relaxes only what mortar does and writes W4 over the large-model
GD sequence; mold relaxes every `REX_GOTPCRELX` `mov` (even to an
absolute symbol), never an IE `add`, and keeps a symbol's IE GOT slot
for all its references once one of them is an `add`.

### 10.2 Anomalies

These are defects of a reference linker that the verifier reports. The
corpus test accepts them by name and nothing else.

- **A1 (mold).** The canonical symbol of an IFUNC (`memcpy`, `strlen`, …)
  has the value of its `.plt.got` entry. However, its `st_shndx` names
  `.text`, and its `st_size` is the resolver's (S8). A debugger attributes
  the address to the wrong section.
- **A2 (mold).** Other aliases of an IFUNC (`__libc_memcpy`, `__memchr`,
  `__strlen`, …) get the value `2^32 + x`, an address outside every
  segment (S8, and Y1 by §4.8). In an ELF32 output the value is
  truncated to `x`, which is 16 bytes before `.plt.got` (i386: `bcmp`,
  `__memchr`, `index`, … at `.plt.got` − 0x10, WP-028e).
- **A3 (mold).** `_TLS_MODULE_BASE_` is defined `STT_TLS` even with no
  `PT_TLS`. When there is one, its value is the segment's address rather
  than an offset (S8, and Y5 of §6.5).

- **A6 (GNU ld, lld, mold).** A `:got:` reference to a local symbol, which the
  assembler writes as a section symbol plus an addend
  (`ADR_GOT_PAGE .data+0x8`), gets a GOT slot that holds the section's
  address *without* the addend. In the corpus's `got` case, `lvar`'s slot
  holds `gvar`'s address, so the program loads the wrong address. This is a
  miscompile. lld and mold do the same: lld gives `lvar` the slot holding
  `.data`, and mold gives it `gvar`'s slot. Clang's assembler names `lvar`
  itself and is not affected. The verifier reports R4 on both records of
  the pair, for every linker. This is the only R-rule finding on the
  corpus.

Two more anomalies are reported here but do not reach the corpus:

- **A4 (mold).** mold accepts `--fix-cortex-a53-843419` and does not patch.
  The erratum sequence is left as it is. The fields are right. With the
  option stated to the verifier, each site is C1 (§5.10), classified A4;
  without it, no fix shows the option and the sites are not judged.
- **A18 (GNU ld).** GNU ld 2.44 leaves an erratum 843419 site unfixed in
  a large link: the `synthetic-sections` workload's static link has a
  4-word site (ADRP at `0x661ff8`, `W3` an `ADRP` of the same register,
  `F` at `0x662004`) with neither fix, after two of its own 16-byte
  erratum stubs earlier in `.text`. Consistent with the stubs moving
  code onto a page end after the scan; the same words at `0xFF8` in a
  small link are fixed. C1, classified A18 when an earlier site of the
  output is patched.
- **A5 (lld).** lld relaxes `LDR Wt, [Xn, :got_lo12:]` (a 32-bit load,
  ill-formed with `LD64_GOT_LO12_NC`) into a 64-bit `ADR`. GNU ld writes
  the scale-8 offset into the scale-4 load. The input is R8 (§4.3), for
  every linker (`got-ldr-w`).

Two TLS miscompiles, each asserted by `tests/tls.rs` on its own case and
absent from the other cases:

- **A7 (GNU ld).** Local-dynamic relaxed to local-exec (W13) adds 16 to
  the thread pointer whatever `PT_TLS` `p_align` is. With `p_align` 64 the
  block starts at TP + 64, so every local-dynamic access is 48 bytes off:
  `tls-ld-64` reads the variable before the one it names and exits 0
  instead of 42. The verifier reports R1 on the three records of the
  sequence, expecting `ADD x0, x0, #0x40`. With `p_align` <= 16 the
  rewriting is right.
- **A8 (GNU ld, lld).** IE relaxed to LE (W9) writes the MOVZ to the
  ADRP's register and the MOVK to the LDR's: `ADRP x4; LDR x5, [x4]`
  becomes `MOVZ x4, #hi; MOVK x5, #lo`, which leaves x5 without the high
  half (review finding 1). `tls-ie-regs` exits wrong; the verifier reports
  R1 on both records. mold keeps the GOT entry and is right.

Three symbol-table defects, found by §6.5 on the corpus:

- **A9 (mold).** Every global of a static executable is `STB_LOCAL` in
  `.symtab`, including `main`, `_start` and the default-visibility globals
  of libc. The symbols stay in the global part of the table (`sh_info` is
  right), but `nm` and debuggers show them as locals (Y3 binding, found 0).
- **A10 (GNU ld).** A label of an empty input section whose output
  section GNU ld removes gets the index of another section, one that does
  not hold the label's address. crtend.o's `__TMC_END__` labels the empty
  `.tm_clone_table`. Its `st_shndx` names a section that ends below it
  (Y3, "a label of an empty section").
- **A11 (mold).** With `--gc-sections` leaving no `.bss`, `__bss_start` is
  0 even when the program references it. A program that clears from
  `__bss_start` to `_end` would write from address 0. This was verified by
  hand with a 3-line program (Y5, found 0).

x86-64 (WP-028c; `spec/x64.md` §14.2 has the experiments, and each is a
case of `tests/x64.rs`):

- **A12 (lld).** lld writes its 16-byte local-exec form from P − 4 over
  the 22-byte large-model general-dynamic sequence (x64.md W4): the rest
  runs as garbage, and the write may reach the last byte of the sequence
  before it (Field, "(spec/x64.md W4)").
- **A13 (mold).** mold puts its own 4-aligned `.note.gnu.build-id` (and
  the inputs' 4-aligned notes, `.note.ABI-tag`, `.note.package`) in one
  `PT_NOTE` with the 8-aligned `.note.gnu.property`, with `p_align` 8: a
  reader that pads by 8 misreads every note after the first (S11, "only
  with padding to 4"). Run-wide on x86-64 whenever there is a property
  note and a 4-aligned note other than the last (WP-028d: in `w6`,
  `-nostdlib`, the build id is the last note and ends at its descriptor,
  which S11 accepts).
- **A14 (lld).** An x86-64 large common (`SHN_X86_64_LCOMMON`) becomes
  an absolute symbol whose value is its alignment, and nothing is
  allocated (W9): every store to it writes wherever that address is (Y3,
  found `SHN_ABS`).
- **A15 (GNU ld, mold).** GCC's `-gsplit-dwarf` `.debug_addr` entry of
  a TLS variable holds its template address, not its DTP offset (W10):
  a debugger reads the wrong thread's storage (Field, "(spec/x64.md
  X-53)").
- **A16 (lld, mold).** The padding between `.init` (or `.fini`)
  fragments is `int3`, which traps at start-up when a fragment is
  aligned above its predecessor's end (W8; S19, found 0xcc).
- **A17 (mold).** An output marked IBT has IPLT entries without
  `endbr64` (W6): an indirect call to an IFUNC faults under IBT
  enforcement (F5).
- **A19 (mold).** An FDE of address range 0 (glibc i386's `clone3.o`,
  in every static link that pulls it, e.g. Fortran's runtime) keeps an
  entry in `.eh_frame_hdr`'s table, so `fde_count` counts it (E5, "fde_count
  counts the FDEs of address range 0"). synth.md S-59: such an entry
  can hide a real FDE with the same start from the binary search. GNU
  ld drops the FDE; lld keeps it out of the table.

On i386 (WP-028e, `tests/x86.rs`; x86.md §15.2): W1 (two readings of a
`GOT32X` operand) is R8 under every map; W3 (mold's no-base GOT value
G − `.got`) is R4, and its no-base `mov` → `lea` R1; W4 (mold's LE form
from P − 3) is R1; W5 (GNU ld's `GOTOFF` of an IFUNC, the entry − 12)
is F4; W10 is A15 (GNU ld, mold); W11 (GNU ld's `16`/`8` truncation) is
R2; W12 (`SHT_RELA` in `EM_386`) is R8 under every map; W14 (GNU ld's
dynamic types) is R8. W2 reads as based (X86-13) and is accepted (an
accepted risk of the spec). W6–W9 and W16 are static-pie (S1, not
judged); W13 and W15 are false refusals, so there is no output.

W1 (GNU ld truncates `16`/`8` fields) and W2 (GNU ld writes `PC16` one
past either end) are violations, R2; W3 (GNU ld writes S + A for a
dynamic-only type) is R8; W5 (lld writes
a link-time value in a static-pie) is S1, since S1 accepts `ET_EXEC` only
(§11 item 7). GNU ld and mold refuse W5's input; lld and mold refuse
W1's, W2's and W3's.

**Notes, not anomalies** (`spec/layout.md` §20.3; never findings, so no
id):

- lld aligns the RELRO end to 4 KiB with a 64 KiB `p_align`, so under
  64 KiB pages the last part of RELRO stays writable: a loss of
  protection, not of correctness (S16 holds).
- With no `.note.GNU-stack` in any input, GNU ld writes no
  `PT_GNU_STACK` (L-84).
- A `PT_LOAD` both `PF_W` and `PF_X` (L-6) is hardening, not
  correctness; the verifier does not report it.

**Where the corpus expects them.** The anomalies that a reference linker
shows on essentially every link of a kind are run-wide defaults, listed
in `tests/link/anomalies` (format: `spec/harness.md` "L3"). It holds mold
A1 A2 A3 A9 (A1 and A2 come from the C runtime's IFUNCs, so the
`-nostdlib` links show only A3 and A9) and GNU ld A10 (crtbegin/crtend's
labels, on every driver link with the C runtime), and on x86-64 mold's
A13. On i386 GNU ld shows no A10 on any output of the L3 run or the corpus
(WP-028e), so the default is `bfd,aarch64,x86_64`; i386's defaults are
mold's A1 A2 A3 A9, and A19 is declared by the tests that pull glibc's
`clone3.o`. A test declares any
other anomaly itself with `#@ anomaly(<linker>): <id>...`. mortar never
has a default and never declares one.

### 10.3 Types each linker accepts

The `types` case links every §4.2 type its assembler can emit. It records
which types each linker refuses:

| types | GNU ld | lld 22 | mold 2.37 |
|---|---|---|---|
| `ABS64`, `PREL64/32/16`, `MOVW_UABS_*`, `LD_PREL_LO19`, `ADR_PREL_*`, `ADD_ABS_LO12_NC`, `LDST*_ABS_LO12_NC`, `CONDBR19`, `JUMP26`, `CALL26`, `ADR_GOT_PAGE`, `LD64_GOT_LO12_NC`, `LD64_GOTPAGE_LO15` | yes | yes | yes |
| `ABS32`, `ABS16`, `MOVW_SABS_*`, `MOVW_PREL_*`, `TSTBR14`, `GOT_LD_PREL19` | yes | yes | refused ("unknown relocation") |
| `LD64_GOTOFF_LO15` | yes | refused | refused |
| `GOTREL64`, `GOTREL32` | refused | refused | refused |
| `PLT32` | neither gcc's nor clang's assembler emits it here | | |

The table rows that no reference linker links (`GOTREL64/32`, `PLT32`) are
checked only by the unit tests of their encoding.

The `tls-types` case does the same for §4.7:

| TLS types | GNU ld | lld 22 | mold 2.37 |
|---|---|---|---|
| `TLSLE_MOVW_TPREL_*`, `TLSLE_ADD_TPREL_HI12`, `_LO12_NC`, `TLSIE_ADR_GOTTPREL_PAGE21`, `TLSIE_LD64_GOTTPREL_LO12_NC`, the TLSDESC sequence | yes | yes | yes |
| `TLSLE_ADD_TPREL_LO12` (checked), traditional GD (513, 514) | yes | refused | yes |
| `TLSLE_LDST*_TPREL_LO12_NC` | yes | yes | refused |
| `TLSLE_LDST*_TPREL_LO12` (checked), `TLSIE_LD_GOTTPREL_PREL19`, `TLSIE_MOVW_GOTTPREL_*`, traditional LD and every `DTPREL` type | yes | refused | refused |
| `LDST128` TLS forms | GAS cannot assemble them | | |
| `TLSDESC` `PREL21`/`LD_PREL19`, GD/LD `PREL21` | refused | refused | refused |

### 10.4 Call frame information

- **The search table.** In a `-static` link through the gcc driver, GNU ld
  and lld write no `.eh_frame_hdr` and no `PT_GNU_EH_FRAME`, because the
  driver passes `--eh-frame-hdr` only for dynamic links. mold always
  writes both. Unwinding in such a GNU ld or lld program goes through
  crtbeginT.o's `__register_frame_info`, which walks from
  `__EH_FRAME_BEGIN__` to crtend.o's terminator.
- **The terminator.** GNU ld keeps crtend.o's terminator last, and writes
  none when no input gives one (`-nostdlib`). lld and mold always end
  `.eh_frame` with a terminator of their own.
- **CIEs.** All three merge identical CIEs: a glibc `-static` hello
  world has 2 CIEs for 950 FDEs from each of them.
- **Dropped FDEs.** An FDE whose function is in a discarded COMDAT group
  or a collected section is absent from all three outputs. None of them
  writes a neutralised FDE on the corpus.

### 10.5 Symbol tables

Observed on the glibc `c-hello` links:

- **Hidden symbols.** lld and mold make a hidden global `STB_LOCAL`, as
  the gABI says (`_dl_argv`: `LOCAL HIDDEN`). GNU ld keeps the binding and
  the visibility (`GLOBAL HIDDEN`). Y3 accepts both forms.
- **Data ends.** GNU ld puts `_edata` at the end of `.data` (E) and
  `__bss_start` at the start of `.bss`, which is E rounded up to `.bss`'s
  alignment. mold puts `_edata` and `__bss_start` at E, which is also the
  start of its `.bss`. lld writes none of the three in this case, because
  nothing references them. All the values seen fall within Y5's ranges.
- **Mapping symbols.** All three keep the inputs' `$x` and `$d`. A
  linker's veneers and patches can carry mapping symbols of their own. A
  `$d` of the input `.eh_frame` lands in GNU ld's rewritten `.eh_frame`,
  at a place the map does not give, so Y4 counts it unchecked.
- **Not judged.** GNU ld and lld write no linker-defined symbol that §6.5
  does not judge. mold writes its `$` names and
  `__start_EHDR`/`__start_PHDR`.

### 10.6 Tombstones

Observed (AArch64; GCC 14 and Clang; the corpus's `debug-comdat`,
`debug-comdat-clang`, `debug-gc` and `debug-zlib`; §5.8):

- **GNU ld** (2.44): a reference from a dropped COMDAT copy is
  redirected to the kept copy when the two sections have the same name
  and size (`.debug_info`, `.debug_aranges`, `.debug_line`,
  `.debug_rnglists`). A reference to a section collected by
  `--gc-sections` holds 1 in `.debug_ranges` and 0 in `.debug_loc` and
  elsewhere. `spec/image.md` §15 says GNU ld writes "the redirect or 0"
  in `.debug_ranges`/`.debug_loc`; the experiment (`-gdwarf-4 -O2
  -ffunction-sections`, `--gc-sections`) shows 1 in `.debug_ranges`, so
  the table of §5.8 follows the observation.
- **lld** and **mold**: 1 in `.debug_ranges` and `.debug_loc`, 0
  elsewhere. Neither redirects. Neither changes the `.debug_aranges`
  tuple's length.
- **Compressed output** (`--compress-debug-sections=zlib`): all three
  compress the output's debug sections (`SHF_COMPRESSED`, `ELFCOMPRESS_ZLIB`).
  GNU ld's and lld's maps keep the input rows; mold's lists only the
  output sections, at their compressed sizes.

These are per-linker rules (§5.8), not anomalies: none is a defect by
DWARF's text, and each is accepted only from its own linker.

## 11. Open questions

1. `spec/model.md` should state that mortar's layout can write this map
   (§3.2) directly. Every input section would be placed by index, with
   pieces for merged sections and `.eh_frame`, and with chunks for
   veneers. With such a map, `merged`, `rewritten` and `absent` do not
   occur for mortar's links.
2. `spec/a64.md` (WP-013) should name the rewritings it performs from §5.3
   by id. A rewriting not listed there is a finding by design.
3. Whether mortar should fix erratum 843419 by W7 or W8, given that mold
   does neither (A4).
4. A6 (§10.2): all three reference linkers drop the addend of a
   GOT-generating relocation against a section symbol, and GAS writes such
   relocations for `:got:` references to local symbols. mortar must either
   give each (symbol, addend) its own slot holding S+A (AAELF64's
   GDAT(S+A)) or refuse the record by name. `spec/a64.md` should state
   which.
5. (Resolved by §6.4.) GNU ld's edited `.eh_frame` is checked record by
   record. mortar's map may still place `.eh_frame` by pieces. The check
   does not need it, but piece records would let crtbeginT.o's
   `__EH_FRAME_BEGIN__` be judged.
6. A7 and A8 (§10.2): mortar's TLS relaxations must use `tcb` =
   align_up(16, `p_align`) and must relax IE only when Xt = Xn = Xd (else
   keep the GOT entry, as mold does). `spec/a64.md` should state both.
7. Static PIE: S1 accepts `ET_EXEC` only, so a static-pie output is an S1 finding
   today. Its TLS forms are those of §4.7 with no TLS dynamic relocation;
   accepting `ET_DYN` without `PT_INTERP` needs the `RELATIVE` relocations
   of `.rela.dyn` judged first, which is stage 3. So are its `.dynsym`,
   `.hash` and `.gnu.hash` (§6.5).
8. A9 to A11 (§10.2): mortar's `.symtab` must keep the inputs' bindings,
   name the section that holds each label (or `SHN_ABS`), and define
   `__bss_start` within §6.5's range even when there is no `.bss`.
9. i386 (§4.5): the table and its rewritings need a spec of their own;
   until one exists every i386 record is `machine`.
10. X-48: resolved (WP-028d, §5.9): in mortar's static `ET_EXEC`
    output, a branch or GOT reference to `__tls_get_addr` is R8.
11. The map should state the link's options that decide a rule: the
    erratum option (§5.10) and `-z execstack`/`-z noexecstack` (with
    them `PT_GNU_STACK`'s `PF_X` could be judged against L-84's table;
    today a flipped `PF_X` there is not found, §9.5).
