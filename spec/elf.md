# `mortar_elf`: ELF files

This spec covers the ELF readers and writer primitives of `mortar_elf`. §1 to §4
cover the constants and the reader for relocatable objects, and §6 the reader
for shared objects. §5 is reserved for the writer primitives, which are
specified separately, and `archive.md` covers archives.

The reference documents are the System V ABI generic ELF specification
("gABI", by chapter name: "ELF Header", "Sections", "String Table", "Symbol
Table", "Relocation"), the Linux gABI extensions ("Program Property"), the
processor supplements AAELF64 (ELF for the Arm 64-bit Architecture), the SysV
x86-64 psABI and the i386 psABI, and the GNU gABI extensions for
`SHT_GNU_*`, `STB_GNU_UNIQUE`, `STT_GNU_IFUNC` and `SHT_RELR`. §6 also uses
the gABI chapters "Program Header", "Dynamic Section" and "Hash Table", the
Linux Standard Base "Symbol Versioning" chapter, and the GNU hash table as
glibc and binutils define it (`DT_GNU_HASH`).

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
- read program headers or dynamic tables: that is the shared-object reader
  of §6, which reads `ET_DYN` files of the same three targets.

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

### 3.0 The parts view (WP-021b)

The reader reads a [`View`]: a file of a known length of which some
ranges, the **parts** (`Part { offset, bytes }`), are in memory
(`spec/input.md` §11.3). Parts are sorted, disjoint and not adjacent, so
a structure lies inside one part or in none. `View::get(start, size)`
gives the bytes exactly when one part holds them (an empty range within
the view always); `View::whole(bytes)`, or a view of one part covering
the file, is the whole file; `View::sub(start, size)` is a range of one
as a view of its own, offsets relative to it (an archive member).

- `Object::parse(&[u8])` is `Object::parse_view(View::whole(..))`, with
  no change of behaviour; `SectionTable::parse` likewise
  (`SectionTable::parse_view`).
- Bounds come first: every check of §3.2 to §3.9 against the file's size
  is made against the view's length, and gives the same error as over
  the whole file. Then residency: a structure within bounds that is not
  inside one part (the file header, the section header table, a string,
  symbol, relocation, group or note table, a compression header) is
  `Error::NotResident { offset, size }` (offsets in the view). The input
  store's rules (`spec/input.md` §11.1 (b)) make every structure the
  readers read resident, so the error is an internal one, never a panic.
- `Section::data()` of a section within bounds whose body is not resident
  is `NotResident { offset, size }` with the body's range; the caller
  copies it with the store's `copy_to`. `Section::is_resident()` says
  which, without the error.
- `Object::data()` and `SectionTable::file()` give the view (they gave
  `&[u8]`); `View::as_slice()` is the whole file when it is resident.
- Archives (`spec/archive.md`): `Archive::parse_view(View)` reads the
  magic and the special members through the view; `Archive::headers()`
  iterates the ordinary members' headers (`MemberHeader { header_offset,
  name, size, location }`) without their contents, so it works over a
  view that holds only the headers; `Archive::members()` reads each
  member's content and is `archive::Error::NotResident` at the first that
  is not resident. `archive::header_span(offset, raw, len)` is what a
  staged store keeps of the header at `offset`, decoded from its 60 bytes:
  the content's range, the bytes of it the reader decodes itself (a
  special or BSD-named member's whole content, else none), and the next
  header's offset; or the error the reader gives at that header.
- Shared objects keep the whole-slice reader (§6): the input store holds
  a shared object whole in both stores.

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

## 5. The writer primitives

Reserved: the writer primitives are specified by a later work package.

## 6. The shared-object reader

The shared-object reader reads what a linker needs from a shared object
(`ET_DYN`) of one of the three targets of §1: the program headers, the
dynamic table, the dynamic symbols and their versions, and the program
properties. Like the object reader, it is a set of `Copy` views over a
borrowed `&[u8]`:

- It does not copy or allocate (§6.10).
- `SharedObject::parse` costs time bounded by the section and program header
  tables (§6.3). Every part's contents are checked when the part is asked
  for, and an error in one part is the error of that part's accessor only.
- Every malformed shape is an `Error` that names what is wrong and where
  (§6.11). The reader never panics, and the work it does is bounded by the
  size of the file (§6.12).

It has no policy. What a linker does with a symbol's version, a `DT_SONAME`
or a segment's alignment is `resolve.md`'s; which files a link accepts as
shared objects is `input.md`'s.

What `mortar_input` reads of a shared object (`spec/input.md` §8.1,
§8.6), all of it provided here: `DT_SONAME`, `DT_FLAGS_1`, `DT_NEEDED`
in table order, `DT_RUNPATH` and `DT_RPATH` (§6.4); the dynamic symbols
with their versions (§6.5, §6.7); whether the file has section headers
(`SharedObject::entry`, §6.3). `SharedObject::parse`, whose entry path
follows the section headers when there are any, is the entry
`mortar_input` uses.

### 6.1 Identification

`SharedObject::parse` checks, in this order, and refuses the file with the
first error it finds:

1. Steps 1 to 5 of §3.1 (`FileHeader::parse`).
2. Steps 6 and 7 of §3.1: the machine and the machine's class, with the same
   errors.
3. **File type:** `ET_DYN`. Otherwise `NotSharedObject`, which names the type
   as `NotRelocatable` does ("not a shared object: e_type is ET_REL").
   A position-independent executable is an `ET_DYN` file and is read like any
   other; whether a link accepts one is `input.md`'s.
4. The section header table and its name table, exactly as §3.2.
5. The program header table (§6.2).
6. The parts are located (§6.3). Errors in locating a part are that part's.

`SharedObject` reports `class()`, `machine()`, `header()`, `data()` (the whole
file), `entry()` (§6.3), `program_headers()` and `section_table()`. The
object reader of §3 still refuses every `ET_DYN` file.

### 6.2 Program headers

**The table.** `ProgramHeaders::parse` locates it in constant time:

- `e_phoff` 0 means no table. With `e_phnum` not 0 as well, that is
  `MissingProgramHeaderTable`.
- `e_phnum` `PN_XNUM` (0xffff) means the count is `sh_info` of section
  header 0 (gABI "Program Header"). A file without section header 0 is
  `PhnumEscapeWithoutSections`.
- A count of 0 is an empty table, whatever `e_phentsize` says. Otherwise
  `e_phentsize` must be 32 (`ELFCLASS32`) or 56 (`ELFCLASS64`), or it is
  `BadPhentsize`.
- The whole table must lie within the file (overflow-checked), or it is
  `ProgramHeadersOutOfBounds`.

**Segments.** Program headers are numbered from 0 in table order. `get(i)`
returns segment `i`, and an index past the table is `SegmentIndexOutOfRange`.
`iter()` (also `IntoIterator` for the table and a reference to it) is
exact-size. A `Segment` exposes the widened `Phdr` as stored (`header()`),
`p_type()`, `flags()`, `is_readable()`, `is_writable()` and
`is_executable()`, and:

- `alignment()`: `p_align`, with 0 read as 1. A value that is not a power of
  two is `BadSegmentAlignment`.
- `contains(addr)`: whether the memory image, `p_vaddr` to `p_vaddr +
  p_memsz`, holds `addr`, without overflow.
- `file_range()` and `data()`: `p_filesz` bytes at `p_offset`, which must lie
  within the file, or it is `SegmentOutOfBounds`.
- `notes()`: §6.8.

**Lookups.** On the table:

- `unique(p_type)` is the one segment of a type a file has at most once
  (`PT_DYNAMIC`, `PT_TLS`, `PT_GNU_PROPERTY`, `PT_GNU_RELRO`,
  `PT_GNU_STACK`, `PT_INTERP`, ...), or `None`. A second is
  `MultipleSegments`, naming both.
- `load_containing(addr)` is the first `PT_LOAD` whose memory image holds
  `addr`: the segment a symbol of that value is in.
- `file_range(addr, size)` maps addresses to file offsets. It uses the first
  `PT_LOAD` whose **file** image, `p_vaddr` to `p_vaddr + p_filesz`, holds the
  whole range, and the result must lie in the file. Otherwise `None`.
- `file_bytes_from(addr)` gives the file bytes from `addr` to the end of the
  file image of the first `PT_LOAD` that holds it, and their file offset: for
  tables whose size the file does not state.

The reader keeps no segment types of its own: every type is reached by
`unique` or by iterating, and is named by `consts::pt_name`.

### 6.3 The entry paths and the rule

The parts of a shared object are the dynamic table, the dynamic symbols, the
version table, the version definitions and the version needs. There are two
ways to find them, `Entry::Sections` and `Entry::Dynamic`. Both give the same
views.

**By the section headers** (`Entry::Sections`):

- The dynamic table is the one `SHT_DYNAMIC` section. Its records are checked
  as §3.4 checks a record table (`sh_entsize` and size), and its string
  table is the `sh_link` section.
- The dynamic symbols are the `SHT_DYNSYM` table, read exactly as §3.5 reads
  a symbol table: `sh_info` is the first global, and `sh_link` is the string
  table.
- The version table is the one `SHT_GNU_versym` section. Its `sh_link` must
  be the `SHT_DYNSYM` section, or it is `VersymLink`. It must have one entry
  per dynamic symbol, or it is `VersymCountMismatch`.
- The version definitions and needs are the one `SHT_GNU_verdef` and
  `SHT_GNU_verneed` sections. `sh_info` is the number of records and
  `sh_link` the string table of their names.

A second section of any of these types is `MultipleSections`.

**By the dynamic table alone** (`Entry::Dynamic`):

- The dynamic table is the contents of the one `PT_DYNAMIC` segment. A file
  without one has no parts.
- An address that a `DT_*` entry gives is mapped to file bytes by the
  `PT_LOAD` segments (`file_range`, §6.2). An address whose bytes are not
  there is `AddressNotMapped`, naming the tag, the address and the size.
- The string table is `DT_STRSZ` bytes at `DT_STRTAB`. `DT_STRTAB` without
  `DT_STRSZ` is `MissingDynamicTag`.
- The dynamic symbols are at `DT_SYMTAB`. Their number is the count of §6.6.
  `DT_SYMENT`, when present, must be the class's symbol size, or it is
  `BadDynamicEntsize`. `DT_SYMTAB` needs `DT_STRTAB`. `DT_SYMTAB_SHNDX`, when
  present, holds one word per symbol.
- The version table is one 2-byte entry per dynamic symbol at `DT_VERSYM`.
  `DT_VERSYM` needs `DT_SYMTAB`.
- The version definitions are at `DT_VERDEF`, with `DT_VERDEFNUM` records;
  the needs are at `DT_VERNEED`, with `DT_VERNEEDNUM`. Each needs its count
  tag and `DT_STRTAB`. The bytes run from the address to the end of the
  `PT_LOAD` file image that holds it.
- **Two facts the dynamic table does not have.** The first global symbol is
  the first symbol whose binding is not `STB_LOCAL`, since there is no
  `sh_info`. Symbols' section indexes are not range-checked, since there is
  no section count. Errors about the string table name "the dynamic string
  table (DT_STRTAB)", and their `section()` is `None`
  (`strtab::DYNAMIC_STRINGS`).

**The rule.** `SharedObject::parse` finds the parts by the section headers
when the file has a section header table with any section in it, and by the
dynamic table when it has none. `SharedObject::parse_by(data, entry)` uses
the given path whether or not the file has sections. Through
`Entry::Sections`, a file without sections has no parts. `entry()` says which
path was taken. The reader does not cross-check the two paths.

This is what the three reference linkers do, and it goes no further. GNU ld
2.44, lld 22 and mold read a shared object's dynamic symbols, versions and
`DT_SONAME` through its section headers only. Given a shared object stripped
by `llvm-objcopy --strip-sections`:

- GNU ld refuses it: "file in wrong format";
- lld and mold accept it, but find no symbols in it.

So when the paths could disagree, the sections decide, as they do for every
reference linker. The dynamic-only path is first class: `input.md` decides
whether mortar links a section-less shared object, and the reader gives the
same view it would have given with sections (§6.13). Where the dynamic table
alone cannot give the view, the reader says so by name (§6.6).

### 6.4 The dynamic table

`DynamicTable` holds the entries up to and including the first `DT_NULL`.
The entries after it are padding, not entries. A table without `DT_NULL` is
`DynamicNotTerminated`. Each entry is a widened `Dyn`: `d_tag` is an `i64`
(sign-extended from `Elf32_Sword` in `ELFCLASS32`), and `d_val` is a `u64`.
Every tag is kept, known or not.

It offers:

- `len`, `is_empty`, `offset` (the table's file offset), `entries` (a
  `Table<Dyn>`) and `iter`, plus `IntoIterator` for a reference;
- `strings()`: the string table its string-valued tags index (§6.3), or
  `None`;
- `value(tag)`: the value of the one entry with that tag, or `None`. A tag
  that appears twice is `DuplicateDynamicTag`, naming both entries;
- `values(tag)`: every value of a tag that may repeat (`DT_NEEDED`,
  `DT_AUXILIARY`, `DT_FILTER`), in order;
- `string_at(tag, offset)` and `string(tag)`: a string of the string table,
  for an entry of `tag`. Without a string table, this is
  `MissingDynamicTag { DT_STRTAB, needed_by: tag }`;
- `soname()`, `rpath()` and `runpath()`: `DT_SONAME`, `DT_RPATH` and
  `DT_RUNPATH`, each refused if it repeats;
- `needed()`: the `DT_NEEDED` names in table order. An unreadable name is an
  error item, and iteration goes on after it;
- `flags()` and `flags_1()`: `DT_FLAGS` and `DT_FLAGS_1`, 0 when absent.

The address- and count-valued tags (`DT_STRTAB`, `DT_STRSZ`, `DT_SYMTAB`,
`DT_SYMENT`, `DT_HASH`, `DT_GNU_HASH`, `DT_VERSYM`, `DT_VERDEF`,
`DT_VERDEFNUM`, `DT_VERNEED`, `DT_VERNEEDNUM`) are read by `value`, and
interpreted by §6.3 and §6.6. A linker that needs the last `DT_SONAME` of a
file that repeats it (lld takes the last) uses `values`.

### 6.5 Dynamic symbols

`SharedObject::symbols()` is a `SymbolTable` (§3.5) over the dynamic
symbols, or `None` when there are none. Every accessor of §3.5 applies: names,
`first_global`, `locals` and `globals`, extended indexes and `section_of`.
§6.3 gives the two facts the dynamic-table path computes rather than reads.

### 6.6 The symbol count from the hash tables

Through the dynamic table, the number of dynamic symbols is stated only by
the hash tables:

- **`DT_HASH`** (gABI "Hash Table"): the count is `nchain`. The header and
  the `nbucket + nchain` words it states must lie in the file image of the
  `PT_LOAD` segment that holds the table.
- **`DT_GNU_HASH`**: the header is `nbuckets`, `symoffset`, `bloom_size` and
  `bloom_shift`. The bloom words are 4 bytes in `ELFCLASS32` and 8 in
  `ELFCLASS64`, and every other word is 4 bytes. The hashed symbols come
  last, so the count is one past the last symbol of the chain that starts at
  the highest bucket: the entry whose low bit is set. A bucket that is not 0
  but below `symoffset`, or a chain that does not end within the segment, is
  `BadHashTable`, as is a table that does not fit.
- **A GNU table that hashes nothing.** When every bucket is 0, no symbol is
  hashed, and the table says only that there are at least `symoffset`
  symbols. It is not a count: GNU ld writes `symoffset` 1 in a PIE whose 31
  dynamic symbols are all undefined, and so unhashed. The count is then
  `DT_HASH`'s, which must be at least `symoffset`. Without `DT_HASH` it is
  `UnhashedSymbolCount`, and the symbols cannot be read by the dynamic table.
  `readelf -D` cannot list them either, and `llvm-readobj` lists none.
- **Both tables.** They must give the same count (or `DT_HASH` must meet the
  GNU table's bound), or it is `SymbolCountMismatch`.
- **Neither table.** With `DT_SYMTAB`, that is `NoSymbolCount`.

The path by sections does not read the hash tables.

### 6.7 Symbol versions

The structures are those of the LSB "Symbol Versioning" chapter. They are the
same in both classes.

**The version table.** `versym()` is a `Table<Versym>`, one entry per dynamic
symbol. `Versym::index()` is the low 15 bits, and `hidden()` is bit 15
(`VERSYM_HIDDEN`).

**Definitions.** `version_definitions()` iterates the `Elf_Verdef` chain in
order (`declared()` is the record count). A `VersionDefinition` has
`record()`, `offset()` (its file offset), `index()` (`vd_ndx`), `flags()`,
`is_base()` (`VER_FLG_BASE`), `is_weak()` (`VER_FLG_WEAK`), `name()` (its
first `Elf_Verdaux`), and `names()`: the `vd_cnt` names in order, its own
first and then the versions it inherits from.

**Needs.** `version_needs()` iterates the `Elf_Verneed` chain, one record per
file. A `VersionNeed` has `record()`, `offset()`, `file()` (`vn_file`) and
`entries()`: its `vn_cnt` `Elf_Vernaux` records. A `VersionNeedEntry` has
`record()`, `offset()`, `index()` (`vna_other`), `flags()`, `is_weak()` and
`name()`.

**Walks.** Every chain is walked by its count, following the next-record
offsets from the start of the table (`vd_next`, `vda_next`, `vn_next`,
`vna_next`), and is bounded:

- the count must fit in the bytes from the chain's start, at the record's
  size (20, 8, 16 and 16 bytes), checked before the walk;
- a next-record offset of 0 with records left ends the chain;
- `vd_version` and `vn_version` must be 1;
- a definition must have at least one name.

These are `BadVersion { record, offset, fault }`. `record` is a
`VersionRecord` (`Definition`, `DefinitionName`, `Need` or `NeedEntry`).
`offset` is the record's file offset. `fault` is a `VersionFault`:
`Truncated`, `ChainEnds`, `Revision(v)`, `TooMany(count)` or `NoName`. The
error is the last item of its iteration.

**Which version a symbol carries.** `versions()` puts the three parts
together. `Versions::version(symbol, index)` is what an index names:

- 0 is `Version::Local`, and 1 is `Version::Global`;
- any other index must be the `vd_ndx` of exactly one definition or the
  `vna_other` of exactly one need entry, giving `Version::Defined(def)` or
  `Version::Needed { need, entry }`;
- an index no record has is `UnknownVersionIndex`, naming the symbol, and an
  index two records have is `AmbiguousVersionIndex`.

`Versions::of(symbol)` is the `SymbolVersion { versym, version }` of a
dynamic symbol, or `None` when the object has no version table.
`SymbolVersion::is_default()` is true for a defined version without the
hidden bit: `name@@VERSION`, the one an unversioned reference binds to. A
hidden one is `name@VERSION`. `name()` is the version's name, or `None` for
`Local` and `Global`. A definition's index is resolved by walking both
chains, so the cost is bounded by their size.

### 6.8 Notes and program properties

`Segment::notes()` iterates the notes of a `PT_NOTE` or `PT_GNU_PROPERTY`
segment, as §3.8 iterates a note section's. Names and descriptors are padded
to 8 bytes when `p_align` is 8, and to 4 bytes when it is 0 to 4. Any other
alignment is `SegmentNoteAlignment`. Another type of segment is
`WrongSegmentType`. A note that does not fit is `BadSegmentNote`, naming the
segment and the `NotePart`.

`SharedObject::gnu_property_note()` is the `GNU` note of type
`NT_GNU_PROPERTY_TYPE_0` in the one `PT_GNU_PROPERTY` segment, or `None`.
Its properties are read as §3.8 reads them.

### 6.9 Facts for copy relocations

A linker that makes a copy relocation for a shared object's data symbol needs
the symbol's alignment and whether its home is read-only. The reader gives
the facts and makes no decision:

- the symbol's value and size (§6.5);
- `ProgramHeaders::load_containing(st_value)`: its `PT_LOAD` segment, with
  `alignment()` and `is_writable()`;
- with sections, `SymbolTable::section_of` and the section's
  `alignment()` and flags (§3.4);
- `PT_GNU_RELRO` through `unique`.

How the facts become an alignment and a read-only answer is `resolve.md`'s.
lld, mold and GNU ld take the alignment from the section's `sh_addralign`,
bounded by the trailing zeros of the value. lld calls a symbol read-only when
its `PT_LOAD` has no `PF_W`.

### 6.10 Records and allocation

The widened records of §6 are `Phdr`, `Dyn`, `Versym`, `Verdef`, `Verdaux`,
`Verneed` and `Vernaux`. Each implements `Record` (§3.11). The version
records have one layout in both classes, so their two decoders agree.
Program headers and dynamic entries are read through `Table`.

The modules `dso`, `dynamic`, `hash`, `segment` and `version` are held to
§3.11's rule: no allocation, `core` only, no allocating type or method
outside their tests. The §3.11 source scan covers them.

### 6.11 Errors

The errors follow §4. An error about a segment starts with `segment N: `,
and `Error::segment()` returns `N`. Dynamic tags are named per §2.2 and
shown in hex when unnamed.

| group | variants |
|---|---|
| identification (§6.1) | `NotSharedObject` |
| program headers (§6.2) | `BadPhentsize`, `MissingProgramHeaderTable`, `PhnumEscapeWithoutSections`, `ProgramHeadersOutOfBounds`, `SegmentIndexOutOfRange`, `SegmentOutOfBounds`, `BadSegmentAlignment`, `WrongSegmentType`, `MultipleSegments` |
| locating the parts (§6.3) | `MultipleSections`, `VersymLink`, `VersymCountMismatch`, `AddressNotMapped`, `MissingDynamicTag`, `BadDynamicEntsize` |
| dynamic table (§6.4) | `DynamicNotTerminated`, `DuplicateDynamicTag` |
| hash tables (§6.6) | `NoSymbolCount`, `UnhashedSymbolCount`, `BadHashTable`, `SymbolCountMismatch` |
| versions (§6.7) | `BadVersion` (with `VersionRecord`, `VersionFault`), `UnknownVersionIndex`, `AmbiguousVersionIndex` |
| notes (§6.8) | `SegmentNoteAlignment`, `BadSegmentNote` |

The errors of §4.1 also arise, from the parts read as sections, symbols and
strings.

A well-formed shared object walks clean. Every accessor, applied to every
segment, dynamic entry, symbol, version, definition, need and note of the
test samples (by both paths, and stripped), returns `Ok`. The exceptions are
the errors the walk asks for on purpose: `unique(PT_LOAD)`, the notes of
segments that are not notes, and `value` of a repeated `DT_NEEDED`.

### 6.12 Hostile input

§4.2's contract holds for `SharedObject::parse` and every accessor of §6: no
panic, no overflow, and work bounded by the size of the file. No walk visits
more records than its bytes can hold. Version chains are checked against
their bytes before they are walked (§6.7), and a hash chain ends within its
segment (§6.6).

The tests take the sample shared object of each target, one hash style each.
Each sample is read three ways: by its sections, by its dynamic table, and
stripped of its sections. On each they test:

- every prefix;
- every field-sized slot of the file header, the program and section header
  tables and every decoded part (dynamic symbols, hash tables, version
  table, definitions, needs, notes, dynamic table), set to every edge value
  in every width;
- 20 000 rounds of combined pseudo-random mutations.

The walk's steps must stay under 16 × (file size + 64).

### 6.13 Agreement with the tools

The reader's view is compared field by field with GNU `readelf` (binutils
2.44, `-dW --dyn-syms -V -lW -n`) and with `llvm-readobj` (LLVM 22,
`--dynamic-table --dyn-syms --version-info --program-headers --notes`). It
must agree with both tools.

**What is compared:**

- every program header field;
- every dynamic entry's tag and value, with the strings of `DT_NEEDED`,
  `DT_SONAME`, `DT_RPATH` and `DT_RUNPATH` and the flag names of `DT_FLAGS`
  and `DT_FLAGS_1`;
- every dynamic symbol (value, size, type, binding, visibility, section and
  name) with its version, and every version table entry (index and hidden
  bit);
- every version definition (index, flags, hash and names) and need (file,
  and each entry's name, index, flags and hash);
- the program properties.

**The corpus.** Nothing binary is checked in.

- Every `ET_DYN` file of each target on the host, each inode once:
  `/usr/lib/aarch64-linux-gnu`, `/lib/aarch64-linux-gnu`,
  `/usr/x86_64-linux-gnu/lib`, `/usr/i686-linux-gnu/lib`, `/usr/lib/gcc*`
  and `/usr/lib/llvm-*/lib`. Every file that has sections is also read by
  its dynamic table, and the two views must be identical.
- A deterministic sample of it (every *n*th file by path, files over 64 MiB
  left out) stripped of its sections by `llvm-objcopy --strip-sections`. The
  view must be identical to the original's, and its symbols must agree with
  `readelf -D -W -s`.
- A versioned C library, built for each target by GNU ld, lld and mold with
  each of `--hash-style=sysv`, `gnu` and `both`. It has hidden and default
  versions of one name, an inherited version, a needed weak version, data,
  read-only data and TLS.

`MORTAR_REQUIRE_TOOLS=1` makes a missing tool, linker or corpus a failure.

**How the tools differ from the reader's view.** The test accounts for each
of these differences explicitly. None of them is a difference in the bytes
read.

- `readelf` prints version indexes in hex without `0x`, with an `h` suffix
  for a hidden one. `llvm-readobj` masks the hidden bit.
- `readelf` prints a needed version as `name@VERSION (n)`; `llvm-readobj`
  prints `name@VERSION`.
- `readelf` prints a version's own absolute symbol (named as the version)
  without its version; `llvm-readobj` prints `name@@name`.
- `readelf` prints no value for `DT_AARCH64_BTI_PLT`, `DT_AARCH64_PAC_PLT`
  and `DT_AARCH64_VARIANT_PCS`, on AArch64 only. The same number is another
  tag on other machines (0x70000001 is `DT_X86_64_PLTSZ`), and
  `llvm-readobj` names some processor tags after other machines.
- For an unnamed section symbol, `readelf` shows the section's name; a
  stripped file has none to show.
- A file whose GNU hash table hashes nothing and that has no `DT_HASH`
  (§6.6) has no symbol count by its dynamic table. The reader refuses with
  `UnhashedSymbolCount`. Once the file is stripped, `readelf -D` cannot list
  its symbols, and `llvm-readobj` lists none. Through its sections, it is
  compared like any other file.
- The linkers differ, not the tools: GNU ld records that a version inherits
  from its parent (`vd_cnt` 2), and lld and mold do not.

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

// §6: shared objects
pub struct Phdr, Dyn, Versym, Verdef, Verdaux, Verneed, Vernaux;  // widened
pub enum Entry { Sections, Dynamic }
pub struct SharedObject<'a>; // parse, parse_by, data, header, class, machine,
    // entry, program_headers, section_table, dynamic, symbols, versym,
    // version_definitions, version_needs, versions, gnu_property_note
pub struct ProgramHeaders<'a>; // parse, len, is_empty, class, file, headers,
    // get, iter, unique, load_containing, file_range, file_bytes_from
pub struct Segment<'a>;      // index, header, p_type, flags, is_readable,
    // is_writable, is_executable, alignment, contains, file_range, data, notes
pub struct DynamicTable<'a>; // len, is_empty, offset, entries, iter, strings,
    // value, values, string_at, string, soname, rpath, runpath, needed,
    // flags, flags_1
pub struct Needed<'a>;       // Iterator<Item = Result<&[u8]>>
pub struct Versions<'a>;     // versym, definitions, needs, version, of
pub enum Version<'a> { Local, Global, Defined(VersionDefinition),
                       Needed { need: VersionNeed, entry: VersionNeedEntry } }
pub struct SymbolVersion<'a> { pub versym: Versym, pub version: Version }
                             // is_default, name
pub struct VersionDefinitions<'a>; // declared; Iterator<Item = Result<..>>
pub struct VersionDefinition<'a>;  // record, offset, index, flags, is_base,
                                   // is_weak, name, names
pub struct VersionNeeds<'a>;       // declared; Iterator<Item = Result<..>>
pub struct VersionNeed<'a>;        // record, offset, file, entries
pub struct VersionNeedEntry<'a>;   // record, offset, index, flags, is_weak, name
pub enum VersionRecord { .. }  pub enum VersionFault { .. }
pub const DYNAMIC_STRINGS: u32;
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
| §6.1 | `dso.rs`: `s6_1_*` |
| §6.2 | `dso.rs`: `s6_2_*` |
| §6.3 | `dso.rs`: `s6_3_*`; `dso_tools.rs`: `s6_13_corpus_*`, `s6_13_stripped_*` |
| §6.4 | `dso.rs`: `s6_4_*` |
| §6.5 | `dso.rs`: `s6_5_*` |
| §6.6 | `dso.rs`: `s6_6_the_symbol_count_from_the_hash_tables`; `src/hash.rs`: `s6_6_*` |
| §6.7 | `dso.rs`: `s6_7_*` |
| §6.8 | `dso.rs`: `s6_8_*` |
| §6.9 | `dso.rs`: `s6_9_*` |
| §6.10 | `spec.rs`: `s3_11_the_reader_names_no_allocating_type` |
| §6.11 | `dso.rs`: `s6_11_every_accessor_of_the_samples_is_clean`, `s6_11_errors_name_their_place` |
| §6.12 | `dso_hostile.rs`: `s6_12_every_truncation_is_an_answer`, `s6_12_every_field_at_every_edge_is_an_answer_{aarch64,x86_64,i386}`, `s6_12_random_mutations_are_answers` |
| §6.13 | `dso_tools.rs`: `s6_13_corpus_{aarch64,x86_64,i386}`, `s6_13_stripped_{aarch64,x86_64,i386}`, `s6_13_versioned_library_{aarch64,x86_64,i386}` |
