# `mortar_elf`: ELF files

This spec covers the ELF readers and writer primitives of `mortar_elf`. §1 to §4
cover the constants and the reader for relocatable objects. The shared-object
reader (§6) and the writer primitives are specified separately, and
`archive.md` covers archives.

The reference documents are the System V ABI generic ELF specification
("gABI", by chapter name: "ELF Header", "Sections", "String Table", "Symbol
Table", "Relocation"), the Linux gABI extensions ("Program Property"), the
processor supplements AAELF64 (ELF for the Arm 64-bit Architecture), the SysV
x86-64 psABI and the i386 psABI, and the GNU gABI extensions for
`SHT_GNU_*`, `STB_GNU_UNIQUE`, `STT_GNU_IFUNC` and `SHT_RELR`.

## 1. Scope

The object reader reads **relocatable** objects (`ET_REL`) in either class,
side by side:

| target | `e_machine` | class | relocation records |
|---|---|---|---|
| AArch64 Linux (LP64) | `EM_AARCH64` | `ELFCLASS64` | `SHT_RELA` |
| x86-64 Linux (LP64) | `EM_X86_64` | `ELFCLASS64` | `SHT_RELA` |
| i386 Linux | `EM_386` | `ELFCLASS32` | `SHT_REL` (`SHT_RELA` is read too) |

The data encoding is little-endian only (`ELFDATA2LSB`). The reader is a
set of views over a borrowed `&[u8]`:

- It does not copy, allocate or decompress (§3.11).
- Parsing an object costs constant time (§3.2); everything else is checked
  when it is asked for.
- Every malformed shape is an `Error` value that names what is wrong and
  where (§4). The reader never panics, and the work it does is bounded by the
  size of the file (§4.2).

What the reader does **not** do:

- interpret relocations: that is `arch.md`;
- resolve symbols, or decide what a group signature means: that is
  `resolve.md`;
- decompress sections;
- read program headers or dynamic tables: that is §6.

## 2. Constants and names

### 2.1 Conventions

- `consts` has every constant the readers, the writer and the diagnostics
  use. The Rust name of each constant is its name in the defining document,
  for example `SHT_SYMTAB_SHNDX`, `SHN_X86_64_LCOMMON` or
  `GNU_PROPERTY_AARCH64_FEATURE_1_AND`.
- `reloc` has the relocation type numbers in one module per machine:
  `reloc::aarch64`, `reloc::x86_64` and `reloc::i386`.
- Field types are the widened ones of §3.11. For example, `sh_flags` is a
  `u64` in both classes.

### 2.2 Value names

A name function maps a value to the name the specification gives it, or to
`None` when it does not name the value. The name functions are:

- `et_name`, `em_name`, `sht_generic_name`, `sht_name`;
- `stb_name`, `stt_name`, `stv_name`;
- `elfcompress_name`, `pt_name`, `dt_name`, `nt_gnu_name`.

**Every value has one name.** Where two names share a value (for example
`STB_GNU_UNIQUE` and `STB_LOOS`), the function returns the one a GNU/Linux
object means.

**Processor-specific section types are named per machine.**
`sht_name(e_machine, sh_type)` names:

- the gABI and GNU types;
- the processor range for that machine only: `0x70000001` is
  `SHT_X86_64_UNWIND` on x86-64 and is not named on AArch64, and
  `0x70000003` is `SHT_AARCH64_ATTRIBUTES`;
- the LLVM types (`SHT_LLVM_*`) on every machine.

`Machine::section_type_name` is the same function for a known machine.

### 2.3 Relocation type names

`reloc::name(e_machine, r_type)` and `Machine::reloc_name` give the name of a
type on that machine. The same number names a different thing on each machine
(type 4 is `R_X86_64_PLT32` and `R_386_PLT32`, and is not an AArch64 type). The
function returns `None` for an unknown machine and for a number the machine
does not define.

The names are the ones in the processor supplements:

- **AArch64:** AAELF64 "Relocation" covers the LP64 types and the
  `R_AARCH64_AUTH_*` types of the PAuth ABI extension (580 to 597 and 1041 to
  1044). `R_AARCH64_NULL` (256) is the withdrawn second code of `NONE`.
- **x86-64:** the psABI, including the APX `R_X86_64_CODE_{4,5,6}_*` types.
- **i386:** the i386 psABI.
- **All three:** the GNU `*_GNU_VTINHERIT` and `*_GNU_VTENTRY` types (250 and
  251).

Two ranges are deliberately left unnamed:

- the AArch64 ILP32 types (`R_AARCH64_P32_*`, 1 to 188), which occur only in
  `ELFCLASS32` objects, a class the reader refuses (§3.1);
- i386 type 200, which binutils calls `R_386_USED_BY_INTEL_200` and no ABI
  defines.

**Agreement with the tools.** Over every type number from 0 to 1100 (plus
4095, 4096, 0x10000 and 0xffffffff) on the 64-bit machines, and every number
from 0 to 255 on i386, the reader's name is the one binutils 2.44 `readelf`
and LLVM 22 `llvm-readobj` print. The two exceptions are the deliberately
unnamed types above. Where only one tool names a type, the reader uses that
name. The tools never give a type two different names. The types only one tool
names are pinned, so that a change in either tool shows up:

| named only by | AArch64 | x86-64 | i386 |
|---|---|---|---|
| binutils 2.44 | 256 | 38, 39, 40, 46-49, 51, 250, 251 | 38, 250, 251 |
| LLVM 22 | 314-317, 580-597, 1041-1044 | none | none |

## 3. The object reader

### 3.1 Identification

`Object::parse` checks the following, in this order, and refuses the file with
the first error it finds. The message names the refused thing.

1. **Magic:** `\x7fELF`. Otherwise `NotElf`, which includes the empty file and
   an archive.
2. **Class:** `ELFCLASS32` or `ELFCLASS64`. Otherwise `InvalidClass`.
3. **Encoding:** `ELFDATA2LSB`. `ELFDATA2MSB` is `BigEndian`, and anything else
   is `InvalidEncoding`.
4. **Length:** the file holds `e_ident` (16 bytes) and then the class's whole
   header (52 or 64 bytes). Otherwise `TruncatedHeader { len, expected }`.
5. **Version:** `EI_VERSION` and `e_version` are both `EV_CURRENT`. Otherwise
   `UnsupportedVersion`.
6. **Machine:** one of the three in §1. Otherwise `UnsupportedMachine`, shown
   with the machine's name when it has one ("unsupported machine EM_RISCV
   (243)") and as a number when it does not ("unsupported machine 4660").
7. **The machine's class:** `ELFCLASS32` with `EM_X86_64` (x32) or with
   `EM_AARCH64` (ILP32), and `ELFCLASS64` with `EM_386`, are each refused as
   `UnsupportedClass`. The message names the ABI: "x32", "ILP32", or "ELF64 is
   not a class of EM_386".
8. **File type:** `ET_REL`. Otherwise `NotRelocatable`, which names the type
   (`ET_DYN`, `ET_EXEC`, `ET_CORE`, `ET_NONE`) or shows it in hex ("e_type is
   0xff00").

`FileHeader::parse` runs steps 1 to 5 only. It reads any little-endian ELF
file of either class, whatever its type and machine, and exposes every field
of the header. Readers of other kinds of file start from it.

`Object` reports `class()`, `machine()` (a `Machine`) and `header()`.

### 3.2 The section header table

**Indexing.** Sections are numbered from 0 in table order, and section 0 is
counted. `Object::section(i)` and `SectionTable::get(i)` return a view of
section `i`. An index past the table is `SectionIndexOutOfRange { index, count
}`. `sections()` iterates over every section in order and knows its exact
length. A view exposes every header field as stored (§3.11).

**No table.** When `e_shoff` is 0, the file has no section header table and no
sections. In that case:

- a non-zero `e_shnum` is `MissingSectionTable`;
- a non-zero `e_shstrndx` is `BadShstrndx`.

**Checked when the object is parsed.**

- `e_shentsize` must be the class's `Shdr` size (40 or 64 bytes). Otherwise
  `BadShentsize`.
- The whole table must lie within the file. Otherwise
  `SectionTableOutOfBounds`.
- The name table index, when it is not 0, must be a section of type
  `SHT_STRTAB` whose contents lie within the file. An index past the table is
  `BadShstrndx`. A section of the wrong type is `WrongSectionType`, and a
  section outside the file is `SectionOutOfBounds`.
- An `e_shstrndx` of 0 means the file has no section names: every name is the
  empty string at offset 0, and any other offset is an error of the name
  (§3.3).

Parsing reads no section header other than header 0 and the name table's.

**Escapes for large counts** (gABI "ELF Header", "Sections"):

- **Count.** When `e_shnum` is 0 and the table exists, the count is `sh_size`
  of header 0. The reader takes that value as stated: a count of 0 there means
  a file without sections. A count that does not fit in 32 bits is
  `TooManySections`. A count whose table runs past the end of the file is
  `SectionTableOutOfBounds`. Nothing is allocated or walked to check it.
- **Name table index.** When `e_shstrndx` is `SHN_XINDEX` (0xffff), the index
  is `sh_link` of header 0. The reader follows this escape even when the
  index is below 0xff00.
- **Symbols.** Section indexes of symbols from 0xff00 up use
  `SHT_SYMTAB_SHNDX` (§3.5).

The L0 tests check each escape at 0xfeff, 0xff00, 0xff01 and 0xff06 sections,
and at 70 000 sections. The L1 tests (§4.3) check them at 70 000 sections.

### 3.3 String tables

A `StrTab` is the bytes of one `SHT_STRTAB` section and that section's index.
`get(offset)` returns the bytes from `offset` up to the next NUL, without the
NUL.

- An offset at or past the end of the table is `StringOutOfBounds { section,
  offset, size }`.
- A string that runs to the end of the table without a NUL is
  `StringNotTerminated { section, offset }`.

Both errors name the string table's section and belong to the accessor that
read the name. A bad name does not make the object unreadable: the other
sections' names still read.

`Section::strings()` returns the string table of a `SHT_STRTAB` section. Any
other type is `WrongSectionType`.

### 3.4 Sections

**Contents.** A section's contents are the `sh_size` bytes at `sh_offset`.

- `SHT_NOBITS` and `SHT_NULL` sections have no bytes, whatever their offset
  and size say.
- An empty range at the very end of the file is valid.
- A range that runs past the end of the file, or whose end overflows, is
  `SectionOutOfBounds { section, offset, size, file_len }`.

`file_range()` returns the range, or `None` for a section without bytes in the
file. `data()` returns the bytes. Contents are checked only when they are
asked for.

**Alignment.** `alignment()` is `sh_addralign`, with 0 read as 1. A value that
is not a power of two is `BadAlignment`. Values up to 2^63 are valid.

**Links.** `link()` reads `sh_link` as a section index, and `info_section()`
reads `sh_info` as one. Each is checked against the section count: `BadLink`
or `BadInfo`, with the count.

**Typed accessors.** The accessors that read a section's contents as records
(§3.5 to §3.8) check:

1. the type, which is `WrongSectionType { section, sh_type, expected }`
   otherwise;
2. that `sh_entsize` is the record's size **in the file's class**
   (`BadEntsize`);
3. that `sh_size` is a whole number of records (`BadSize`);
4. then the bounds.

The other class's entry size is wrong: an ELF32 symbol table with
`sh_entsize` 24 is an error.

### 3.5 Symbols

**The symbol table.** `Object::symbols()` returns the file's `SHT_SYMTAB` table,
or `None` when there is none. A file with more than one is
`MultipleSymbolTables { first, second }`. The table's shape is checked when it
is asked for:

- the entry size and size rules of §3.4;
- `sh_link` must be a section (`BadLink`) of type `SHT_STRTAB`
  (`WrongSectionType`);
- `sh_info`, the index of the first non-local symbol, may equal the count but
  not exceed it (`BadFirstGlobal`);
- the table must have fewer than 2^32 symbols (`TooManySymbols`).

`SectionTable::symbol_table(symtab, shndx)` reads a given table, either
`SHT_SYMTAB` or `SHT_DYNSYM`. `find_symbol_table(sh_type)` finds the one table
of a type.

**Records.** `Elf32_Sym` (16 bytes) and `Elf64_Sym` (24 bytes) order their fields
differently, and both decode to one `Sym`. Every field keeps its largest
value: an `st_value` or `st_size` of `u32::MAX` or `u64::MAX` survives.
`binding()`, `sym_type()` and `visibility()` split `st_info` and `st_other`
as the gABI says. The other bits of `st_other`, such as
`STO_AARCH64_VARIANT_PCS`, are kept.

**Access.**

- `get(i)` returns symbol `i`. An index past the table is
  `SymbolIndexOutOfRange`.
- `iter()`, `locals()` and `globals()` iterate over all symbols, the symbols
  below `sh_info`, and the rest. Each knows its exact length and yields
  `(SymbolIndex, Sym)` pairs.
- `name(&sym)` reads the name from the linked string table (§3.3).

**The section of a symbol.** `section_of(i, &sym)` returns:

- `Undefined` for `SHN_UNDEF`;
- `Absolute` for `SHN_ABS`;
- `Common` for `SHN_COMMON`;
- `Reserved(n)` for any other value from `SHN_LORESERVE` up, for example
  `SHN_X86_64_LCOMMON`. Its meaning is the machine's to give;
- `Section(n)` otherwise. The index must be a section of the file. Otherwise
  it is `SymbolSectionOutOfRange { symbol, index, count }`.

`SHN_XINDEX` takes the index from entry `i` of the table's `SHT_SYMTAB_SHNDX`
section. That section is the one of type `SHT_SYMTAB_SHNDX` whose `sh_link` is
the symbol table, and the rules are:

- An extended index of 0 means `Undefined`.
- A symbol that does not use the escape ignores its entry.
- The section must have exactly one 4-byte entry per symbol. Otherwise
  `ShndxCountMismatch`, or `BadEntsize` for a wrong entry size.
- The table may have at most one such section. A second is
  `MultipleShndxTables`.
- An `SHN_XINDEX` symbol in a table without one is `MissingShndx { symbol }`.

### 3.6 Relocations

`Section::rela()` reads a `SHT_RELA` section and `Section::rel()` reads a
`SHT_REL` section. Each returns a `RelocSection` with:

- `symtab()`, which is `sh_link` (checked: `BadLink`);
- `target()`, the section the records apply to, which is `sh_info` (checked:
  `BadInfo`);
- the records, which can be read with `get`, `iter` (from both ends) and
  `len`.

The type is checked first: `rela()` on a `SHT_REL` section is
`WrongSectionType`, and so is the reverse. The entry size must be the class's
size for that record type: 8 or 16 bytes for `REL`, 12 or 24 bytes for `RELA`.
The rules of §3.4 then apply.

**Decoding `r_info`** (gABI "Relocation"):

- `ELFCLASS32`: the symbol is `r_info >> 8` (24 bits) and the type is
  `r_info & 0xff` (8 bits).
- `ELFCLASS64`: the symbol is `r_info >> 32` and the type is
  `r_info & 0xffffffff`.

**Addend.** An `ELFCLASS32` addend is sign-extended to `i64`. Records keep file
order. `Rel` has no addend: for `REL` records the addend is stored in the place
being relocated, and `arch.md` reads it.

`SHT_RELR` is a named type (§2.2). Its records are not read: `rela()` and
`rel()` refuse it as `WrongSectionType`. Relocatable objects do not use it.

### 3.7 Groups

`Section::group()` reads a `SHT_GROUP` section (gABI "Section Groups"). The
contents are 4-byte words in both classes, and `sh_entsize` must be 4:

- **Flag word.** The first word is the flags. `is_comdat()` tests
  `GRP_COMDAT`, and unknown flag bits are reported, not refused. A section too
  short to hold the flag word is `EmptyGroup`.
- **Members.** The remaining words are the member section indexes.
- **Signature.** `sh_link` is the symbol table (checked: `BadLink`), and
  `sh_info` is the index of the signature symbol. The reader does not check
  that index: the caller reads the symbol through the symbol table, and
  `SymbolTable::get` checks it there.

`members()` checks each member as it reads it. A member must be a section of
the file other than section 0; otherwise it is `GroupMemberOutOfRange {
section, member, index, count }`. A bad member does not hide the ones after it:
iteration goes on.

A signature symbol may be an unnamed `STT_SECTION` symbol. GNU `readelf` then
shows the section's name and `llvm-readobj` shows the empty name (§4.3). The
reader gives the symbol as stored, and the meaning of such a signature is for
`resolve.md` to decide.

### 3.8 Notes and program properties

**Notes.** `Section::notes()` reads a `SHT_NOTE` section (gABI "Note Section")
as a sequence of notes, in order. Each note is a 12-byte header (`n_namesz`,
`n_descsz`, `n_type`, 4-byte words in both classes), then the name, then the
descriptor.

- **Padding.** The name and the descriptor are each padded to 4 bytes when the
  section's `sh_addralign` is at most 4, and to 8 bytes when it is 8. Any other
  alignment is `NoteAlignment`. The padding follows the section's alignment,
  not its class: the same bytes read with the other alignment are a different
  note.
- **Last note.** The padding after the last descriptor may be missing.
- **Name.** `name_bytes()` is the stored name with its terminator, and `name()`
  strips one trailing NUL.
- **Errors.** A note that does not fit is `BadNote { section, offset, part }`.
  `offset` is where the note starts in the section, and `part` is the part that
  does not fit: `Header`, `Name` or `Descriptor`. The error ends the iteration.
- **Large sizes.** Sizes near 2^32 are errors, not overflows or allocations.

**Program properties.** `Note::gnu_properties()` is `Some` only for a note
named `GNU` of type `NT_GNU_PROPERTY_TYPE_0` (Linux gABI extensions, "Program
Property"). It iterates over the properties.

- **Format.** Each property is an 8-byte header (`pr_type`, `pr_datasz`) and
  then its data. The data is padded to 4 bytes in `ELFCLASS32` and to 8 bytes
  in `ELFCLASS64`. The padding after the last property may be missing.
- **Errors.** A property that does not fit in the descriptor is `BadNote` with
  part `Property`, and the error ends the iteration. `offset` is where the
  property starts in the section.
- **Values.** `Property::as_u32()` reads 4-byte data, such as the
  `GNU_PROPERTY_*_FEATURE_1_AND` words. The reader does not interpret
  properties.

### 3.9 Compressed sections

`Section::compression()` returns `None` for a section without `SHF_COMPRESSED`.
For a section with the flag it returns the compression header and the bytes
after it (gABI "Compression Headers"). Nothing is decompressed.

- `Elf32_Chdr` is 12 bytes: `ch_type`, `ch_size`, `ch_addralign`.
- `Elf64_Chdr` is 24 bytes, with a reserved word after `ch_type`.
- Both decode to one `Chdr`.

**Errors.**

- A section shorter than its class's header is `CompressedTooShort`.
- A compressed `SHT_NOBITS` section is `CompressedNobits`.

A payload of zero bytes is valid. `ch_type` is reported as stored:
`ELFCOMPRESS_ZLIB` and `ELFCOMPRESS_ZSTD` are named by `elfcompress_name`.

### 3.10 Mergeable sections

`Section::merge()` returns `None` for a section without `SHF_MERGE`, even one
with `SHF_STRINGS`. For a section with the flag it returns `Merge { entsize,
strings }`.

- **Entry size 0.** It is reported as 0, not refused: what such a section
  means is for the merge phase to decide.
- **Size.** For a non-zero entry size, a `sh_size` that is not a multiple of
  it is `BadSize`.

### 3.11 Record tables and allocation

**Widened records.** Each on-disk record has one Rust type for both classes,
with every field at its `ELFCLASS64` width. The widened types are `Ehdr`,
`Shdr`, `Sym`, `Rel`, `Rela`, `Chdr`, `Nhdr` and `Word`. `Rel` and `Rela`
carry the decoded `r_sym` and `r_type`.

**The `Record` trait.** It gives the size of a record in each class (`SIZE32`,
`SIZE64`), a decoder for each layout (`decode32`, `decode64`), and
`size(class)` and `decode(class, bytes)`, which choose between them.

**Tables.** `Table<'a, T>` is a `Copy` view of a byte slice as an array of `T`
in one class. It offers `len`, `get(i)`, `slice(start, end)`, `bytes` and
`iter`. The iterator:

- is double-ended and exact-size;
- implements `nth` in constant time;
- branches on the class once in `fold` and `for_each`, and once per record in
  `next`.

Internal and external iteration give the same records. Trailing bytes too few
for a whole record are not a record. `get` and `slice` out of range return
`None`.

**No allocation.** The reader's modules do not allocate. Outside their tests
they use `core` only, and name no allocating type, macro or method (`Vec`,
`String`, `Box`, `format!`, `collect` and similar). A test scans the sources
for them. Every view borrows the file, and every error is `Copy`.

## 4. Errors

Every `Error` says what is wrong and where. Section and symbol numbers are
indexes into the file's own tables.

- `Display` gives one line: the text after the file name in a diagnostic.
- An error about a section starts with `section N: `, and `Error::section()`
  returns `N`. For the other errors it returns `None`, and the message names
  the symbol, index or header field instead.
- Section types in messages are named per §2.2 ("type is SHT_RELR, expected
  SHT_RELA"). An unnamed type is shown in hex.
- `Error` is `Copy`, `Eq` and `non_exhaustive`, and implements
  `std::error::Error`.

A well-formed object walks clean: every accessor, applied to every section,
symbol, relocation, group member, note and property of the test samples,
returns `Ok`.

### 4.1 The errors

| group | variants |
|---|---|
| identification (§3.1) | `NotElf`, `TruncatedHeader`, `InvalidClass`, `BigEndian`, `InvalidEncoding`, `UnsupportedVersion`, `UnsupportedMachine`, `UnsupportedClass`, `NotRelocatable` |
| section header table (§3.2) | `BadShentsize`, `MissingSectionTable`, `TooManySections`, `SectionTableOutOfBounds`, `BadShstrndx`, `SectionIndexOutOfRange` |
| one section (§3.3, §3.4) | `SectionOutOfBounds`, `WrongSectionType`, `BadEntsize`, `BadSize`, `BadAlignment`, `BadLink`, `BadInfo`, `StringOutOfBounds`, `StringNotTerminated` |
| symbols (§3.5) | `MultipleSymbolTables`, `MultipleShndxTables`, `TooManySymbols`, `BadFirstGlobal`, `ShndxCountMismatch`, `SymbolIndexOutOfRange`, `MissingShndx`, `SymbolSectionOutOfRange` |
| groups, notes, compression (§3.7 to §3.9) | `EmptyGroup`, `GroupMemberOutOfRange`, `NoteAlignment`, `BadNote` (with `NotePart`), `CompressedTooShort`, `CompressedNobits` |

### 4.2 Hostile input

The reader must survive any byte string:

- `Object::parse` and every accessor return `Ok` or `Err` and never panic;
- no arithmetic on a file's value overflows;
- the work done, and the records visited, are bounded by the size of the file.

A header that claims the largest possible tables in a file that is only a
header is refused at once, because nothing is sized by a count the file
states.

The tests that check this cover the sample objects of all three targets, in
two layouts: with the section header table at the end, and with the header
table and names first (so that a truncation meets the accessors that are
checked lazily). They also cover objects that use extended section indexes.
On each object they test:

- every prefix;
- every field-sized slot of the file header, the section header table and the
  record tables, set to every edge value in every width;
- a deterministic pseudo-random sweep of combined mutations.

### 4.3 Agreement with the tools

For every file of the L1 corpus, the reader's view is compared field by field
with the output of GNU `readelf` (binutils 2.44, `-W -h -t -g -r -s -n`) and of
`llvm-readobj` (LLVM 22, `--file-headers --sections --symbols --relocations
--expand-relocs --section-groups --notes`). The test must agree with both
tools; they are not alternatives.

**What is compared:**

- the file header;
- every section header field and name, and the compression header (readelf
  only, since `llvm-readobj` does not print it);
- every symbol: name, value, size, type, binding, visibility (including
  `VARIANT_PCS`) and section;
- every relocation: offset, symbol, type number and name, and addend (none
  for `REL`);
- every group: flags, signature and members;
- every note: owner, type and descriptor, and every program property of the
  kinds both tools decode.

**The corpus.** Nothing binary is checked in. The corpus is built on the host
when the test runs:

- the system's crt objects for each target;
- a deterministic sample of members of each target's `libc.a` and
  `libstdc++.a`;
- C, C++ and Fortran sources compiled by GCC and Clang for each target under
  several flag sets (`-ffunction-sections`, `-gz=zlib`, `-gz=zstd`, `-fPIC`,
  `-fcommon`, branch protection and CET, the x86-64 medium model's large
  common symbols);
- an object of 70 000 sections assembled for each target;
- the test builder's samples;
- a relocation-type sweep (§2.3).

`MORTAR_REQUIRE_TOOLS=1` makes a missing tool or driver a failure rather than a
skip.

**How the tools differ from the reader's view.** The test accounts for each of
these differences explicitly. None of them is a difference in the bytes
read.

- `readelf` truncates section names to 256 bytes without marking the cut. It
  does so wherever it prints a section name: section lists, group members,
  relocation and note headers, and section symbols.
- For a group whose signature is an unnamed `STT_SECTION` symbol, `readelf`
  shows the section's name and `llvm-readobj` the symbol's empty name.
- `llvm-readobj` labels `SHN_X86_64_LCOMMON` as "Processor Specific".
- `llvm-readobj` shows `NT_STAPSDT` and the FDO packaging note as `Unknown
  (0x…)`.
- The descriptors of `stapsdt` notes, which readelf prints on continuation
  lines, are not compared.

## Interface (outline)

```rust
pub enum Class { Elf32, Elf64 }                      // word_size(), name()
pub trait Record: Copy { const SIZE32: usize; const SIZE64: usize;
    fn decode32(&[u8]) -> Option<Self>; fn decode64(&[u8]) -> Option<Self>;
    fn size(Class) -> usize; fn decode(Class, &[u8]) -> Option<Self>; }
pub struct Ehdr, Shdr, Sym, Rel, Rela, Chdr, Nhdr, Word(pub u32);  // widened
pub struct Table<'a, T>     // Copy: new, empty, len, get, slice, bytes, iter
pub struct TableIter<'a, T> // DoubleEnded + ExactSize + Fused; fold per class

pub struct FileHeader { pub class: Class, .. }  // Deref<Target = Ehdr>; parse
pub enum Machine { AArch64, X86_64, I386 }      // of, e_machine, class, name,
                                                // reloc_name, section_type_name
pub struct Object<'a>;      // parse, data, header, class, machine,
                            // section_table, section_count, section, sections,
                            // symbols
pub struct SectionIndex(pub u32);  pub struct SymbolIndex(pub u32);
pub struct SectionTable<'a>; // parse, len, class, file, headers, names, get,
                             // iter, symbol_table, find_symbol_table
pub struct Section<'a>;      // index, header, class, sh_type, flags, size,
    // name, alignment, file_range, data, link, info_section, strings,
    // rela, rel, group, notes, compression, merge
pub struct RelocSection<'a, R>; pub type RelaSection, RelSection;
                             // section, symtab, target, records, len, get, iter
pub struct Group<'a>;        // section, flags, is_comdat, symtab, signature,
                             // len, members -> Result<SectionIndex> items
pub struct Compression<'a> { pub header: Chdr, pub payload: &'a [u8] }
pub struct Merge { pub entsize: u64, pub strings: bool }
pub struct StrTab<'a>;       // new, empty, bytes, section, get
pub struct SymbolTable<'a>;  // section, len, first_global, records, strings,
    // has_extended_indexes, get, iter, locals, globals, name, section_of
pub enum SymbolSection { Undefined, Absolute, Common,
                         Section(SectionIndex), Reserved(u16) }
pub struct Notes<'a>;        // Iterator<Item = Result<Note>>
pub struct Note<'a>;         // n_type, name_bytes, name, desc, gnu_properties
pub struct Properties<'a>;   // Iterator<Item = Result<Property>>
pub struct Property<'a> { pub pr_type: u32, pub data: &'a [u8] } // as_u32
pub enum Error { .. }  pub enum NotePart { Header, Name, Descriptor, Property }
pub mod consts; pub mod reloc;   // §2
```

## Tests

| section | tests (`crates/mortar_elf/tests/`) |
|---|---|
| §2.2 | `spec.rs`: `s2_2_values_have_their_specification_names` |
| §2.3 | `spec.rs`: `s2_3_relocation_types_are_named_per_machine`; `tools.rs`: `s2_3_relocation_names_agree_with_the_tools` |
| §3.1 | `spec.rs`: `s3_1_the_three_targets_are_accepted`, `s3_1_everything_else_is_refused_by_name`, `s3_1_the_file_header_reads_any_little_endian_file` |
| §3.2 | `spec.rs`: `s3_2_sections_are_indexed_and_iterated`, `s3_2_a_file_may_have_no_sections`, `s3_2_the_table_and_the_name_table_are_checked_eagerly`, `s3_2_large_section_counts_use_the_escapes`, `s3_2_the_count_escape_is_taken_at_its_word`; `tools.rs`: `s4_3_extended_section_numbering` |
| §3.3 | `spec.rs`: `s3_3_bad_section_names_are_errors_of_the_name` |
| §3.4 | `spec.rs`: `s3_4_contents_are_bounds_checked`, `s3_4_alignment_and_links_are_checked` |
| §3.5 | `spec.rs`: `s3_5_symbols_are_decoded_in_both_classes`, `s3_5_symbol_fields_are_widened_not_truncated`, `s3_5_a_malformed_symbol_table_is_an_error`, `s3_5_symbol_section_indexes_are_checked` |
| §3.6 | `spec.rs`: `s3_6_relocations_are_decoded_in_both_classes`, `s3_6_a_malformed_relocation_section_is_an_error` |
| §3.7 | `spec.rs`: `s3_7_groups_are_read`, `s3_7_a_malformed_group_is_an_error` |
| §3.8 | `spec.rs`: `s3_8_notes_follow_the_section_alignment`, `s3_8_malformed_notes_are_errors`, `s3_8_program_properties_are_read_for_each_machine` |
| §3.9 | `spec.rs`: `s3_9_compression_headers_are_decoded` |
| §3.10 | `spec.rs`: `s3_10_mergeable_sections_report_their_entry_size` |
| §3.11 | `spec.rs`: `s3_11_tables_iterate_the_same_either_way`, `s3_11_the_reader_names_no_allocating_type` |
| §4 | `spec.rs`: `s4_errors_say_what_and_where`, `s4_a_well_formed_object_walks_clean` |
| §4.2 | `hostile.rs`: `s4_2_every_truncation_is_an_answer`, `s4_2_every_field_at_every_edge_is_an_answer`, `s4_2_random_mutations_are_answers`, `s4_2_huge_claims_cost_nothing` |
| §4.3 | `tools.rs`: `s4_3_crt_objects_{aarch64,x86_64,i386}`, `s4_3_libc_members_*`, `s4_3_libstdcxx_members_*`, `s4_3_compiled_{c,cxx,fortran}`, `s4_3_built_samples`, `s4_3_extended_section_numbering` |
