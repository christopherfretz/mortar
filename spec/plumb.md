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
| 1 (this spec) | `ET_EXEC`, static, non-PIE, AArch64. Placement, relocations (the §4.2 table), the GOT slots of a static link, structure (§6), content, symbol values | — |
| 2 | TLS (the AAELF64 TLS types: TLSLE, TLSIE relaxed, TLSDESC relaxed), IFUNC/`IRELATIVE`, `.eh_frame` and `.eh_frame_hdr` contents, non-alloc (DWARF) relocations | `tls`, `ifunc`, `eh-frame`, `non-alloc` |
| 3 | PIE and shared objects: the dynamic sections, dynamic relocations, PLT | — |
| x | x86-64 and i386 tables (§4.5) | `machine` |

On the reference corpus, a stage-1 report may give only the stage-2
reasons in "not checked" (§9.1).

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
`archive(member)` names a member of a regular archive. The member is found
by name, and by header offset when the map gives one (mortar's format).
The member must be the only one with that name, or the record is refused
(I2). Thin archives are refused (I2, stage 3).

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
discard <file> <section> gc|comdat|script|unspecified
fold <file> <section> <into-file> <into-section>
chunk thunk|patch|synthetic|common <name> <addr>|- <size> [<target> <addend>]
define <name> <value>
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

The writer emits records in a fixed order: files by id, then places and
pieces, discards, folds, chunks and defined symbols, each group in the
order of the map's vectors.

Every syntax error is a `MapError { line, message }`. Line 0 means the map
was truncated, with no `end`. Tests:

- `map::mortar::tests::s3_2_round_trip`
- `s3_2_tokens_encode_every_byte`
- `s3_2_malformed_maps_are_errors`
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
| rewritten | `.eh_frame` placed whole with a size other than its input's (GNU ld) | not checked (`eh-frame`) |
| discarded | a `discard` record, or GNU's "Discarded input sections" and `/DISCARD/` | not in the output |
| absent | no record | not in the output. An absent alloc `.eh_frame` (mold lists none) counts as `eh-frame` |
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
types (512 to 573, and 1026 to 1031) are stage 2 (`tls`). Any other type is
not checked (`type`).

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

Test: `a64::tests::s4_3_fields_encode_as_the_instruction_set_says`,
`s4_3_checks`.

### 4.4 S: resolving a symbol

Symbol 0 has S = 0. An input symbol of type `STT_GNU_IFUNC` or `STT_TLS`
is stage 2 (`ifunc`, `tls`). Otherwise:

- **A local symbol** (`STB_LOCAL`, including `STT_SECTION`) defined in
  section k has S = place(k) + `st_value`, which depends on k's place:
  - **whole at a:** S = a + `st_value`;
  - **pieces:** found through the piece that covers `st_value`;
  - **merged:** §5.6;
  - **rewritten `.eh_frame`, or a dropped part of one:** `eh-frame`;
  - **ambiguous:** `section-not-locatable`;
  - **discarded or absent:** R5 (§5.7).

  An `SHN_ABS` symbol has S = `st_value`. Anything else is
  `symbol-unresolved`.
- **A global, weak or `STB_GNU_UNIQUE` symbol**, by name, takes the first
  of these that applies:
  1. **The output's global symbol of that name.** This is the first
     `STB_GLOBAL` or `STB_WEAK` symbol of the name in `.symtab`, and it
     decides which definition won. If it is IFUNC or TLS, the record is
     stage 2. If it is defined, S is its value. If it is undefined and weak
     (on either side), the symbol is an *undefined weak* and S = 0.
  2. **The unique placed definition in the inputs.** Take the one strong
     definition, else the one weak definition, whose section is placed,
     pieces or merged (or that is `SHN_ABS`). S follows from that section's
     place, as for a local symbol. This step covers hidden symbols, which
     lld and mold make local in the output.
  3. **The unique output symbol of that name**, of any binding.
  4. **An undefined weak** in the input: S = 0.

  If none applies, the record is `symbol-unresolved`.

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

### 4.6 Y1: symbol values

For each name that has a global, weak or unique definition in the inputs,
take the output symbol of that name: the global one, else the unique one.
If it is defined and not TLS, its value must equal the place of one of the
name's placed definitions, or the `st_value` of an `SHN_ABS` one.

- A name with IFUNC definitions is unchecked: the output symbol is the
  canonical PLT entry, which is stage 2.
- A name with no definition placed whole is unchecked.

Counts: `symbols_checked`, `symbols_unchecked`. Test:
`tests/mutation.rs s9_2b_moved_symbols_are_found`.

## 5. Checking a record

### 5.1 The procedure

The records of a section are checked in index order. For each record:

1. **Place.** Find P (§3.3). A record outside every piece of a sectioned
   place is not in the output.
2. **Trivial types.** Type 0 or 256 counts as `none`. TLS types go to
   `tls`, and types not in the table go to `type`.
3. **Place bounds.** The input's place, `width` bytes at `r_offset`, must be
   inside the section's data. If it is not, the finding is I2 (for
   `SHT_NOBITS`, the input word is taken as 0).
4. **The output word.** Read the output's word at P. If P is outside the
   loaded memory, the finding is R7. An instruction field at an erratum
   843419 site is read through the patch (W8, §5.3).
5. **S.** Resolve S (§4.4).
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

### 5.4 The veneer emulator

From T, at most 8 instructions are executed. Only these are followed:

- `BTI`, as the first instruction only (a `NOP` is not followed, so a
  branch to the padding before a veneer is a finding);
- `ADRP`, `ADR`, `ADD` (immediate, optionally `LSL #12`), `LDR` (64-bit
  literal), and `MOVZ`/`MOVK`/`MOVN`, each writing x16 or x17;
- an end: `BR x16`, `BR x17`, or `B`.

The result is the destination. Anything else means the code is not a
veneer. Every reference linker's veneer shape is handled: GNU ld's
`ADRP; ADD; BR`, lld's `LDR literal; BR; .xword`, and mold's
`ADRP; ADD; BR`.

Test: `a64::tests::s5_4_veneers_are_followed`,
`tests/mutation.rs s9_2d_retargeted_veneers_are_found`.

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
   output address of input byte r.
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
absent is R5. This does not apply to `.eh_frame`'s own records, which are
stage 2, nor to non-alloc records.

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
  class.
- **S2.** There is at least one `PT_LOAD`. For each one:
  - `p_filesz <= p_memsz`;
  - `p_align` is 0, 1 or a power of two;
  - `p_vaddr ≡ p_offset (mod p_align)`;
  - its file image lies inside the file;
  - its memory does not wrap.
- **S3.** The `PT_LOAD`s are in ascending `p_vaddr` order and do not
  overlap (gABI "Program Header").
- **S4.** Every allocated section with a size, other than `.tbss`, lies
  within one `PT_LOAD`'s memory. If it has bytes, they lie within that
  segment's file image, at `p_offset + (sh_addr − p_vaddr)`.
- **S5.** A section with `SHF_WRITE` is in a `PF_W` segment, and one with
  `SHF_EXECINSTR` is in a `PF_X` segment.
- **S6.** No two sections overlap. In memory this covers allocated
  sections other than `.tbss`. In the file it covers sections with bytes,
  the file header, the program header table and the section header table.
- **S7.** `e_entry` lies in an executable `PT_LOAD`.
- **S8.** Every `.symtab` symbol defined in an allocated section, with
  `st_size > 0` and not `STT_SECTION`, lies within its section:
  `[st_value, st_value + st_size] ⊆ [sh_addr, sh_addr + sh_size]`. For a
  TLS symbol, `st_value + st_size <= p_memsz` of `PT_TLS`, and a TLS
  symbol requires a `PT_TLS`. Labels (`st_size == 0`) are checked by Y1.
- **S9.** There is at most one `PT_TLS`, its initialization image lies in
  a `PT_LOAD`'s file image, and its `p_align` is a power of two. Every
  `SHF_TLS` section lies inside it, and TLS sections require a `PT_TLS`.
- **S10.** There is at most one `PT_GNU_STACK`.
- **S11.** Every `PT_NOTE` lies in the file and is a sequence of whole
  notes (gABI "Note Section"), with padding to 4, or to 8 when `p_align`
  is 8.
- **S12 (content).** Take every input section placed whole that is
  allocated, has a size, and is not mergeable, `.eh_frame`, `.tbss` or
  `.note.gnu.property`. Its output bytes must equal its input bytes
  outside its relocated ranges (§6.3). The one exception is a word that is
  an erratum-patch branch (W8). A `SHT_NOBITS` section's memory must read
  as zero. `.note.gnu.property` is excluded because linkers combine the
  property notes by definition (gABI "Program Property").

### 6.3 Relocated ranges

The relocated ranges of a section are the bytes
`[r_offset, r_offset + width)` of each of its records, where width comes
from the type (§4.2). A TLS type counts as 4 bytes, and an unknown type
as 8.

## 7. The report

### 7.1 Findings

A finding has:

- the rule id: `O1`, `S1`–`S12`, `I1`, `I2`, `M1`–`M5`, `R1`–`R7` or `Y1`;
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

- **verified**, by how S was found: `direct`, `got-slot` or `piece`;
- **accepted**, by rewriting (W1 to W8);
- **not checked**, by reason: `tls`, `ifunc`, `non-alloc`, `eh-frame`,
  `type`, `machine`, `section-not-locatable`, `piece-not-locatable`,
  `in-mergeable`, `symbol-unresolved`, `got-unpaired` or
  `input-unreadable`;
- **none**, for type 0 and type 256;
- **failed**, for records with a finding.

`checked_types` counts the verified and accepted records by type. The
records of sections not in the output are counted separately
(`absent_records`). So are the sections by place, the symbols Y1 checked
and left unchecked, and the bytes S12 compared.

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

Every case is linked by `ld.bfd` (GNU ld 2.44), `ld.lld-22` and mold 2.37,
each through the gcc driver with `-Wl,-Map`.

For each link (`tests/corpus.rs s9_1_reference_links_verify_clean`):

- there are no findings, other than the anomalies recorded in §10.2;
- `failed` counts only the records of A6, and the coverage sum holds;
- every not-checked reason is a stage-2 reason.

`s4_2_every_type_each_linker_accepts_is_checked` asserts that each type
listed in §10.3 is checked for its linker. `s5_3_observed_rewritings_are_accepted`
asserts the rewritings of §10.1.

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

## 10. The reference linkers, observed

### 10.1 The rewritings each linker performs

| linker | rewritings observed |
|---|---|
| GNU ld 2.44 | W4 (branch to an undefined weak made `NOP`); W6 (`__x_veneer` in `.text.stub`: `ADRP x16; ADD x16; BR x16`); W7 (erratum 843419: the `ADRP` made an `ADR` of the page). No GOT relaxation. With W7, an unused erratum stub is still emitted. |
| lld 22 | W1, only with addend 0, Rd equal, and the pair adjacent; W2 and W3 (GOT relaxation, only when Rt = Rn and the pair is adjacent); W5 (`bl .+4` to an undefined weak); W6 (`__AArch64AbsLongThunk_x`: `LDR x16, .+8; BR x16; .xword`); W8 (`__CortexA53843419_*`: the site becomes `B patch`, and the patch holds the load and `B back`) |
| mold 2.37 | W1, including with an addend; W2, including for an undefined weak (`ADRP` of page 0); W4; W6 (`x$thunkN`: `ADRP x16; ADD; BR x16`). Code padding is `BRK #0x3e8`. |

### 10.2 Anomalies

These are defects of a reference linker that the verifier reports. The
corpus test accepts them by name and nothing else.

- **A1 (mold).** The canonical symbol of an IFUNC (`memcpy`, `strlen`, …)
  has the value of its `.plt.got` entry. However, its `st_shndx` names
  `.text`, and its `st_size` is the resolver's (S8). A debugger attributes
  the address to the wrong section.
- **A2 (mold).** Other aliases of an IFUNC (`__libc_memcpy`, `__memchr`,
  `__strlen`, …) get the value `2^32 + x`, an address outside every
  segment (S8).
- **A3 (mold).** `_TLS_MODULE_BASE_` is defined `STT_TLS` even with no
  `PT_TLS`. When there is one, its value is the segment's address rather
  than an offset (S8).

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
  The erratum sequence is left as it is. The fields are right, so this is
  not a finding: the verifier checks what the program computes, not
  whether the erratum workaround was applied.
- **A5 (lld).** lld relaxes `LDR Wt, [Xn, :got_lo12:]` (a 32-bit load,
  ill-formed with `LD64_GOT_LO12_NC`) into a 64-bit `ADR`. The corpus does
  not contain that input.

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
5. Stage 2 must specify the `.eh_frame` check against the LSB, so that
   GNU ld's edited `.eh_frame` (§3.4) can be checked.
