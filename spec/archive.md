# `mortar_elf::archive`: archives

An archive (`libfoo.a`) is the `ar` container a linker reads static
libraries from: a magic string, then members, each a 60-byte text header
and its bytes. `mortar_elf::archive` reads one from the bytes of its file.
It says what the archive contains and nothing about what a link does with
it.

The format has no single owner. This spec cites:

- **ar(5)**: the manual page "ar - archive (library) file format" of the
  BSDs, for the header, the `#1/` names and `__.SYMDEF`;
- **gABI**: the System V ABI, "Archive File" in "Formats and Protocols",
  for the `/` symbol table and the `//` string table;
- **binutils**: GNU `ar`'s manual (the `T`, `s`, `S` modifiers) and the
  format comments of BFD's `archive.c` and `archive64.c`, for thin
  archives, `/SYM64/` and what GNU `ar` writes;
- **LLVM**: the "File Format" part of the `llvm-ar` manual and the reader
  `llvm/Object/Archive.h`, for what `llvm-ar` writes and refuses.

Where those are silent or disagree, what GNU `ar` 2.44 and `llvm-ar` (19.1
and 22.1, which behave alike) do was observed and is written down here; §10 lists where the two differ and
which way mortar goes. Every "must" below has a test in
`crates/mortar_elf/tests/archive/` whose name starts with the section it
checks (`s4_2_` for §4.2); "Tests" at the end maps them.

## 1. Scope

In scope: regular and thin archives; GNU short names, the GNU long-name
table, BSD `#1/` names; the GNU symbol index, its 64-bit form and the BSD
symbol index; the members in file order and by header offset.

Out of scope, by decision: what a member contains (`spec/elf.md`); which
members a link takes (`spec/resolve.md`); writing archives; reading files
(the reader is given bytes, and for a thin archive gives back paths).

Not supported and refused by name: COFF archives (§5). Not recognised, so
"not an archive" (§2): AIX big archives (`<bigaf>\n`) and any other magic.

## 2. The file

An archive starts with one of two 8-byte magic strings (ar(5); binutils
for the second):

| magic | kind | members |
|---|---|---|
| `!<arch>\n` | regular | each member's bytes follow its header |
| `!<thin>\n` | thin | each ordinary member is a path to a file (§7.2) |

`Archive::sniff(bytes)` gives the kind from the magic alone, `None` for
anything else. `Archive::parse` must fail with `NotAnArchive` for bytes
that start with neither, including bytes shorter than a magic.

An archive that is the magic and nothing else is valid and empty: no
index, no members. GNU `ar rcs` and `llvm-ar rcs` both write it for an
empty member list (glibc installs `libpthread.a` and `libdl.a` so).

## 3. Member headers

After the magic the file is a sequence of members. A member starts with a
60-byte header of ASCII fields, each padded with spaces on the right
(ar(5)):

| bytes | field | read as |
|---|---|---|
| 0..16 | name | §4 |
| 16..28 | modification time | not interpreted |
| 28..34 | owner | not interpreted |
| 34..40 | group | not interpreted |
| 40..48 | mode (octal) | not interpreted |
| 48..58 | size | decimal: the number of bytes that follow the header |
| 58..60 | terminator | `` `\n `` (0x60 0x0a) |

- The four fields that are not interpreted may hold anything: GNU `ar`
  leaves them blank in the header of the long-name table.
- The size must be one or more decimal digits followed only by spaces.
  Anything else (no digits, a leading space, a sign, another base) is the
  error `BadSize`.
- The terminator must be `` `\n ``, else `BadHeaderTerminator`.
- Fewer than 60 bytes where a header starts is `TruncatedHeader`. That
  includes any bytes after the last member.
- In a regular archive, and for the special members (§5) of a thin one,
  the `size` bytes after the header are the member's; if the archive ends
  before them the error is `TruncatedMember`. For an ordinary member of a
  thin archive nothing follows the header and the size is that of the
  file it names (§7.2).
- A header starts at an even offset. When a member's bytes end at an odd
  offset, one padding byte follows (writers put `\n`; its value is not
  examined). The padding byte of the last member may be missing: every
  byte of the member is present, and GNU `ar` reads such a file.

## 4. Member names

The name field has four forms. A field that starts with `/` or `#1/` is
read up to its first space and the rest of the field is ignored; GNU `ar`
leaves a stray `/` in the last byte of such a field for a thin-archive
member whose name is 15 bytes long (`/79            /`), and both tools
read past it.

### 4.1 Short names

A field that does not start with `/` or `#1/` holds the name itself.

- If the field contains `/`, the name is the bytes before the first `/`.
  This is how GNU `ar` and `llvm-ar --format=gnu` write names of up to 15
  bytes; the name may contain spaces (`sp ace.o/`).
- Otherwise the name is the field without its trailing spaces: the
  traditional BSD form, and the whole field for a 16-byte name. If a
  space remains inside that name the field is refused (`AmbiguousName`):
  GNU `ar` ends such a name at the first space and `llvm-ar` at the last
  non-space, and no writer produces it.
- A NUL byte anywhere in such a field is refused (`NulInName`), for the
  same reason.
- The name may be empty (a field of spaces), as both tools read it.

Names are bytes. Several members may have the same name; each is a member
of its own (§7.1).

### 4.2 Long names

A field `/<decimal>` refers to the long-name table (§5): the name starts
at that byte offset of the table and ends before the first `/\n` at or
after it (gABI; LLVM). The name may contain `/` and spaces. A reference
may point into the middle of an entry and names the rest of it.

- GNU `ar` and `llvm-ar` put a name there when it is longer than 15
  bytes, and every name of a thin archive (a path, §7.2).
- No long-name table: `NoLongNameTable`.
- An offset at or past the end of the table: `LongNameOutOfRange`.
- No `\n` at or after the offset, or a `\n` not preceded by a `/` at or
  after the offset: `LongNameUnterminated`.
- A field that starts with `/` and is none of `/`, `//`, `/SYM64/` (§5),
  `/<decimal>` or `/<decimal>:<decimal>` (§7.3): `BadLongNameReference`.

A reference is resolved when the member is read, not by `parse`.

### 4.3 BSD names

A field `#1/<decimal>` says that the name is the first `<decimal>` bytes
after the header, and the member's content is what follows them (ar(5)).
The size field counts both. The name ends at its first NUL: writers pad
it with NULs (`llvm-ar --format=bsd` so that the content starts at a
multiple of 8 bytes). The member's `size()` is the size field less the
length, and its content offset is after the stored name.

- `llvm-ar --format=bsd` and `--format=darwin` write every name this way.
- `#1/` not followed by decimal digits: `BadBsdNameLength`.
- A length larger than the member's size: `BsdNameTooLong`.
- A BSD name in a thin archive is refused (`BsdNameInThinArchive`): no
  tool writes it and where the name's bytes would be is not defined.

## 5. Special members

These members are not ordinary members: the walk and `member_at` (§7)
never report them.

| name field | member |
|---|---|
| `/` | the GNU symbol index (§6.1) |
| `/SYM64/` | the 64-bit GNU symbol index (§6.2) |
| `//` | the long-name table (§4.2) |
| a name `__.SYMDEF`, `__.SYMDEF SORTED`, `__.SYMDEF_64` or `__.SYMDEF_64 SORTED` | a BSD symbol index (§6.3) |

A BSD index is recognised by its name in any form of §4.1 (the 16-byte
field `__.SYMDEF SORTED` included) or §4.3, in either kind of archive.

They are at the front of the archive, in this order: at most one symbol
index, then at most the long-name table, then the ordinary members. Either
may be absent. A special member anywhere else is `MisplacedSpecialMember`,
reported when the walk reaches it.

A `/` member directly after a GNU symbol index is the second linker member
of a COFF archive (LLVM), whose long names and index have other formats:
`parse` must fail with `CoffArchive`.

The special members of a thin archive are stored in it like those of a
regular one, with their padding.

`Archive::parse` examines the magic, the symbol index and the long-name
table, and the header that follows them when it has to look at it to know
that it is not one of them: that is, the first header, and the header
after a symbol index. It reports every error of those headers except the
lookup of a long name (§4.2). A malformed symbol index does not fail
`parse`: the errors of §6.5 marked "`symbol_index`" are returned by
`Archive::symbol_index()` as `Some(Err(BadIndex))`, and the members are
read as if the index were well formed. A linker does not need the index
(`spec/input.md` §7.1), and GNU ld, lld and mold link such archives.
It reads no other header, so its cost does not depend on the number of
members or symbols; the errors of every other header are reported by the
walk or `member_at` (§7.1).

## 6. The symbol index

The index lists symbols that members define, each with the offset in the
archive of the defining member's header. It is written by `ar s` and
`ranlib` from the members' symbol tables; the reader does not check it
against them. An entry is `(name, member header offset)`; entries are
reported in the order of the index. A symbol may be listed more than once,
for one member or for several.

`SymbolIndex::iter` yields `Result<Symbol, Error>` and ends after the
first error. `len()` is the number of entries the index announces and
`member_offset(i)` the offset of entry `i` in constant time. The name of
entry `i` is reached by iterating (the GNU forms store names
consecutively).

### 6.1 GNU (`/`)

gABI. All words are 4 bytes, big-endian:

```
count
count words:   member header offsets
count strings: names, each terminated by NUL, in the same order
```

Bytes after the last name are ignored (writers pad the member with NUL to
an even size, inside the size). An index with a count of zero is an index
with no entries, not the absence of one.

### 6.2 GNU, 64-bit (`/SYM64/`)

The same with words of 8 bytes (binutils `archive64.c`). Written when an
offset does not fit in 32 bits. It may be followed by GNU or BSD member
names: `llvm-ar --format=bsd` writes `/SYM64/` and `#1/` names together.

### 6.3 BSD (`__.SYMDEF`)

ar(5); the macOS `ranlib.h`. All words are 4 bytes for `__.SYMDEF` and
`__.SYMDEF SORTED`, 8 bytes for `__.SYMDEF_64` and `__.SYMDEF_64 SORTED`,
and little-endian: the format uses the byte order of the target, and
mortar's targets are little-endian (`docs/ARCHITECTURE.md`, "Scope").

```
table size in bytes (a multiple of two words)
entries:  { name offset, member header offset }, two words each
names size in bytes
names:    NUL-terminated strings, addressed by the entries' name offsets
```

Bytes after the names are ignored. Entries are reported in table order.
The reader decodes this form like the others; `SymbolIndex::kind` tells a
caller which form it has.

### 6.4 No index

An archive without a symbol index (`ar rcS`, or an archive in which no
member defines a symbol: both tools then write none) is a normal state:
`Archive::symbol_index()` is `None`. It is not an error of the reader.

### 6.5 A malformed index

All are `BadIndex`, with the offset of the index's header, its form and
one of:

| problem | when | reported by |
|---|---|---|
| `NoCount` | the member is smaller than one word | `symbol_index` |
| `TableOutOfRange` | the table the first word announces (GNU: entries; BSD: bytes) does not fit in the member, or its size overflows | `symbol_index` |
| `TableNotWhole` | BSD: the table size is not a multiple of two words | `symbol_index` |
| `NoNamesSize` | BSD: no word follows the table | `symbol_index` |
| `NamesOutOfRange` | BSD: the names do not fit in the member | `symbol_index` |
| `NameUnterminated` | an entry's name has no NUL before the names end | iteration, at that entry |
| `NameOutOfRange` | BSD: an entry's name offset is at or past the end of the names | iteration, at that entry |

The framing problems (`symbol_index`) are found by `parse`, which keeps
the result; `symbol_index()` returns it on every call, without reading
the member again. A member offset of the index is not checked until it is
given to `member_at` (§7.1).

### 6.6 Cost

Neither `parse` nor iteration allocates, and `parse` does not scan the
names: opening an archive with a hundred thousand symbols costs what
opening an empty one does, and the index is read in one pass over its
bytes.

## 7. Members

### 7.1 The walk and `member_at`

`Archive::members()` yields the ordinary members in file order as
`Result<Member, Error>` and ends after the first error. Each step reads
one header. `Archive::member_at(offset)` reads the member whose header
starts at `offset`, which is what the symbol index gives, and yields the
same `Member` the walk does.

A `Member` has: `header_offset()`; `name()` (§4), borrowed from the
archive; `size()`, the size of its content; `content()`.

`member_at` must refuse an offset that cannot be a member's header:

- before the first ordinary member (so: the magic and the special
  members), or at or past the end of the archive: `NoMemberAt`;
- odd: `OddMemberOffset` (§3).

An even offset in range that is not the start of a header is read as one
and fails with whichever error of §3 or §4 the bytes there produce; like
the tools, the reader cannot tell a header from member bytes that look
like one.

In a regular archive `content()` is `Content::Inline { offset, data }`:
the `size()` bytes at `offset` of the archive.

### 7.2 Thin archives

binutils ("thin archives"). An ordinary member's header is followed by the
next header: no content, no padding. Its name is a path, as stored:
relative to the directory that holds the archive unless it starts with
`/`. `content()` is `Content::External { path }` and `size()` the size
the header gives for that file; the reader does not open it. `name()` is
the same path, which is what `ar t` lists when run in the archive's
directory; run elsewhere, `ar t` joins it with the archive's directory
(`sub/objs/f.o` for `sub/libt.a`).

Both tools store every path in the long-name table. A short name (§4.1)
is read as a path too.

### 7.3 Archives inside thin archives

What the tools do when an archive is added to a thin archive:

| added | GNU `ar` | `llvm-ar` |
|---|---|---|
| a thin archive | its members are added, as paths | the same |
| a regular archive | one member per member of that archive, named `/<name offset>:<origin>`: the long name is the path of the archive, `<origin>` the offset of the member's header in it, the size the member's size | one member: the path of the archive, the size of the whole file |

mortar reads all of them. The first and the last need nothing: they are
members with paths. For GNU's `/<name offset>:<origin>` form `content()`
is `Content::ExternalMember { archive_path, header_offset }` and `name()`
is `archive_path`: the caller reads that file, parses it as an archive and
takes `member_at(header_offset)`, which gives the name `ar t` shows and
the bytes.

What the reference linkers do with the two forms of a regular archive in
a thin one is recorded here for the crates that decide what mortar does
(`spec/input.md`, `spec/resolve.md`): GNU ld links GNU's form; lld
refuses it (the error `llvm-ar` gives) and mold takes no member from it,
without a diagnostic. No linker takes members from `llvm-ar`'s form: the
member is an archive, not an object.

The form is refused in a regular archive (`NestedMemberInRegularArchive`).
A thin archive stored in a regular archive is an ordinary member whose
bytes are a thin archive; what a link does with it is not this crate's
matter.

## 8. Errors

One enum, `Error`, `Copy`, with `Display` and `std::error::Error`. Every
variant except `NotAnArchive` carries `offset`: the offset in the archive
of the member header it is about (for `NoMemberAt` and `OddMemberOffset`,
the offset asked for). A message names what is wrong and where, and quotes
the offending field with non-printing bytes escaped:

```
member header at offset 1234: size field "12x4" is not a decimal number
symbol index at offset 8 (GNU, "/"): the name of entry 2 is not terminated
no member header at offset 9: member headers are at even offsets
```

| variant | section |
|---|---|
| `NotAnArchive` | §2 |
| `TruncatedHeader`, `BadHeaderTerminator`, `BadSize`, `TruncatedMember` | §3 |
| `NulInName`, `AmbiguousName` | §4.1 |
| `BadLongNameReference`, `NoLongNameTable`, `LongNameOutOfRange`, `LongNameUnterminated` | §4.2 |
| `BadBsdNameLength`, `BsdNameTooLong`, `BsdNameInThinArchive` | §4.3 |
| `MisplacedSpecialMember`, `CoffArchive` | §5 |
| `BadIndex` with an `IndexProblem` | §6.5 |
| `NoMemberAt`, `OddMemberOffset` | §7.1 |
| `NestedMemberInRegularArchive` | §7.3 |

For any input bytes whatever, `parse`, the walk, the index iteration and
`member_at` at any offset must return a value: no panic, no allocation,
and work bounded by the size of the input (the walk advances by at least a
header per step; the index iteration by at least a word per entry). All
arithmetic on sizes and offsets read from the input is checked.

## 9. Agreement with the tools

The reader must agree with GNU `ar` and `nm`, and with `llvm-ar` and
`llvm-nm`, wherever those read an archive. For an archive and a pair of
tools the L1 tests compare:

- the member names, their order and count with `ar t`;
- each member's size with the third column of `ar tv`;
- in a regular archive, each member's content offset with `ar tO`;
- all members' contents, concatenated, with `ar p` (for a thin archive
  the reader's side reads the files the paths name, and the nested
  archive of §7.3);
- the symbol index with the listing `nm --print-armap` (`nm -s`) prints
  first, entry by entry: the symbol, and the name of the member that
  `member_at` finds at the entry's offset. An archive without an index
  must have no listing.

The corpus: every archive under `/usr/lib` (the C library's, the
compilers' runtime libraries for the host and the cross targets), with GNU
`ar` and `nm`; archives built at test time by GNU `ar` (`rcs`, `rcS`,
`rcsT`, an empty archive, an archive of no objects, members sharing a
name, a thin archive in another directory with relative and absolute
paths, a regular and a thin archive nested in a thin archive, a thin
archive in a regular one) and by `llvm-ar` (`--format=gnu`, `bsd`,
`darwin`, thin, nested, and the 64-bit index, which `llvm-ar` writes below
4 GiB when `SYM64_THRESHOLD` is set in its environment); and the
hand-built fixtures the other tests use. An archive that both pairs of
tools read is compared with both.

## 10. Where GNU `ar` and `llvm-ar` differ

Observed with binutils 2.44 and LLVM 19.1 and 22.1; pinned by the `s10_`
tests.

What they write:

| | GNU `ar` | `llvm-ar` | mortar reads |
|---|---|---|---|
| a regular archive added to a thin archive | refers to each of its members (`/<n>:<origin>`) | stores the archive whole as one member | both (§7.3) |
| a thin member with a 15-byte name | `/<n>` with a stray `/` in the field's last byte | `/<n>` | both (§4) |
| BSD names and index | never | `--format=bsd`, `--format=darwin`: every name `#1/`, index `__.SYMDEF` named by a `#1/` name | yes (§4.3, §6.3) |
| the 64-bit index | `/SYM64/` | `/SYM64/`, also with `--format=bsd` | yes (§6.2) |
| content padding | to an even offset | `--format=darwin`: to 8 bytes with `\n`, counted in the member's size | as the size says; `llvm-ar` reports the padded size too |

What they read:

| input | GNU `ar` | `llvm-ar` | mortar |
|---|---|---|---|
| GNU's `/<n>:<origin>` thin member | reads | error | reads (§7.3) |
| `__.SYMDEF` named by a `#1/` name (what `llvm-ar` writes) | lists it as a member; `nm` prints no index | index | index (§5) |
| `__.SYMDEF` in the name field | index | lists it as a member | index (§5) |
| the last member's padding byte missing | reads | lists the member, then error | reads (§3): nothing is missing |
| fewer than 60 bytes after the last member | ignores them | error | `TruncatedHeader` (§3) |
| a size with leading spaces or a sign | reads | error | `BadSize` (§3) |
| a name with a space and no terminating `/` (`y z`) | `y` | `y z` | `AmbiguousName` (§4.1) |
| a NUL in the name field (`ab\0cd/`) | `ab` | `ab\0cd` | `NulInName` (§4.1) |
| `/0` without a long-name table | an empty name | error | `NoLongNameTable` (§4.2) |
| a long name without `/\n` | reads to the end of the entry or table | error | `LongNameUnterminated` (§4.2) |
| a symbol index after a member | a member with an empty name | a member named `/` | `MisplacedSpecialMember` (§5) |
| a second `/` after the index (COFF) | skips both | skips both, reads the rest as COFF | `CoffArchive` (§5) |

The rule behind the last column: mortar reads what either tool writes;
for input no tool writes it takes the stricter reading, and refuses where
the two tools would give different members or names. The one leniency
(the missing final padding byte) loses nothing.

## 11. API

```rust
pub const MAGIC: &[u8; 8];        // "!<arch>\n"
pub const THIN_MAGIC: &[u8; 8];   // "!<thin>\n"
pub const HEADER_LEN: u64;        // 60

pub enum Kind { Regular, Thin }

impl<'a> Archive<'a> {            // Copy; borrows the file's bytes
    pub fn sniff(bytes: &[u8]) -> Option<Kind>;
    pub fn parse(bytes: &'a [u8]) -> Result<Archive<'a>, Error>;
    pub fn kind(&self) -> Kind;
    pub fn symbol_index(&self) -> Option<Result<SymbolIndex<'a>, Error>>;   // Err: §6.5
    pub fn members(&self) -> Members<'a>;      // Iterator<Item = Result<Member<'a>, Error>>
    pub fn member_at(&self, header_offset: u64) -> Result<Member<'a>, Error>;
}

impl<'a> Member<'a> {             // Copy, Eq
    pub fn header_offset(&self) -> u64;
    pub fn name(&self) -> &'a [u8];
    pub fn size(&self) -> u64;
    pub fn content(&self) -> Content<'a>;
    pub fn data(&self) -> Option<&'a [u8]>;    // Some for Content::Inline
}

pub enum Content<'a> {
    Inline { offset: u64, data: &'a [u8] },
    External { path: &'a [u8] },
    ExternalMember { archive_path: &'a [u8], header_offset: u64 },
}

pub enum IndexKind { Gnu, Gnu64, Bsd, Bsd64 }

impl<'a> SymbolIndex<'a> {        // Copy; IntoIterator by value and by reference
    pub fn kind(&self) -> IndexKind;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn member_offset(&self, entry: usize) -> Option<u64>;
    pub fn iter(&self) -> Symbols<'a>;         // Iterator<Item = Result<Symbol<'a>, Error>>
}

pub struct Symbol<'a> { pub name: &'a [u8], pub member_offset: u64 }

pub enum Special { Index(IndexKind), LongNames }
pub enum IndexProblem { /* §6.5 */ }
pub enum Error { /* §8 */ }
```

Everything a view returns borrows the bytes given to `parse`, not the
`Archive`. Both iterators are fused. The module uses no allocator.

## Tests

`crates/mortar_elf/tests/archive.rs`, in `archive/`:

| section | tests |
|---|---|
| §2 | `spec::s2_*` |
| §3 | `spec::s3_*`; `decimal_fields`, `space_padding` (unit tests of the module) |
| §4 (a field ends at its first space) | `spec::s4_a_field_ends_at_its_first_space`, `tools::s10_what_the_tools_write_differently` |
| §4.1 | `spec::s4_1_*` |
| §4.2 | `spec::s4_2_*` |
| §4.3 | `spec::s4_3_*` |
| §5 | `spec::s5_*` |
| §6.1 to §6.6 | `spec::s6_1_*` to `spec::s6_6_*` |
| §7.1 to §7.3 | `spec::s7_1_*` to `spec::s7_3_*` |
| §8 | `spec::s8_every_error_says_what_and_where`; `hostile::s8_every_truncation_of_the_seeds` (every prefix of seven fixtures, `member_at` at every offset); `hostile::s8_a_mutation_sweep_over_the_seeds` (30 000 mutants of each) |
| §9 | `tools::s9_system_archives_0` to `_3`, `tools::s9_archives_built_by_gnu_ar`, `tools::s9_archives_built_by_llvm_ar`, `tools::s9_hand_built_fixtures_are_read_alike_by_the_tools`, `tools::s9_extracted_members_hold_the_readers_bytes` |
| §10 | `tools::s10_where_the_tools_differ`, `tools::s10_what_the_tools_write_differently` |

`archive/build.rs` builds the fixtures by hand from this spec; the §9 test
of the hand-built fixtures checks the builder against the tools. The tool
tests skip with a note when a tool is missing, and fail instead under
`MORTAR_REQUIRE_TOOLS=1`.
