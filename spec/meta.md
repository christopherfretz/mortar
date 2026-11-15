# spec: the Mason metadata container (`mortar_meta`)

**Status: final draft (WP-070, revision 2 after the adversarial review
and re-review, `briefs/reviews/review-070-meta-odr.md` and
`review-070-meta-odr-re.md`; the container is accepted for
implementation; the operator's open decisions are `spec/odr.md` §15).** The container is shared with
kiln (writer), hod (carrier) and tuck (future reader); the parts that bind
them are marked **[kiln]**, **[hod]**, **[tuck]** and go to their agents
through `spec/odr.md` §11. Rules are `MT-n`. Each rule about another tool
names the experiment behind it (`odr.md` Appendix A: `container`,
`dynamic`, `size`, and the revision's `container-adv`, `note-final`,
`model`); `[unverified]` marks the rest and `[estimate]` every number
no script printed.

**Numbering.** Rules are numbered by section and the gaps are reserved
for later rules of the same section, never reused: MT-7..9 (§3),
MT-17..19 (§5), MT-22..24 (§6), MT-29 (§7), MT-34 (§11), MT-39..40
(§9-§10), MT-43..49 (§10). The output rules MT-30..33 (§11) keep their
numbers although §10's MT-41/42 precede them in the text. Every rule has
a test (§12).

## 1. Scope

What the Mason toolchain adds to an object file beside the code, how a
producer writes it, and what mortar and other tools do with it. It is
**additive** (`docs/ARCHITECTURE.md` principle 9): an object without the
container links exactly as before, with fewer guarantees; an object with
it links with GNU ld, lld and mold, which drop it (MT-3, measured).

One container serves several record kinds (§7): the one-definition-rule
records of `spec/odr.md` (the first user), a summary of the translation
unit's ABI-affecting options, per-section content hashes for
`spec/incr.md` (reserved; WP-060 does not need them for correctness), and
debugger context for tuck (reserved). A kind is added by amendment of §7
alone; nothing else in this spec changes for it.

Not here: what the ODR records mean and how they are compared
(`spec/odr.md`); the loadable table of a dynamic output (`spec/odr.md`
§10; MT-31 places it); incremental link state (`spec/incr.md`: a separate
file, not this container).

`mortar_meta` depends on `mortar_elf` only (`docs/ARCHITECTURE.md`): it
parses a section's bytes into units and blocks and writes them; it
interprets no record. `mortar_odr` interprets kinds 1-5.

## 2. Measured behavior of the tools

Six encodings were written with GNU as, one object each, for AArch64,
x86-64 and i386, and passed through each reference linker (final link
and `-r`), `strip -g`, `strip --strip-unneeded`, `objcopy`, the
`llvm-strip`/`llvm-objcopy` 19 equivalents, `eu-strip`, `objcopy
--redefine-sym` and an `ar` round trip (`container`; identical results on
the three targets unless stated). The "strip/objcopy" column is the
first experiment's selection of options; MT-5 has the full measured
matrix (`container-adv`), in which the llvm strippers **drop** P:

| encoding | final link (bfd, lld, mold) | `-r` (bfd, lld, mold) | binutils and llvm strip/objcopy | `eu-strip` | `ar` | `eu-elflint` |
|---|---|---|---|---|---|---|
| **P** `.mason.meta`, `SHT_PROGBITS`, `SHF_EXCLUDE` | dropped; program runs | kept; two inputs' sections concatenated into one, each aligned, flags `E`, alignment 8, input order, no padding (`container-adv`: bfd, lld, mold) | kept by `strip -g`, `--strip-unneeded`, `objcopy`; **dropped** by `llvm-strip` (default and `--strip-all`) and `llvm-objcopy --strip-non-alloc` (MT-5) | **dropped** | byte-identical | clean |
| N `.note.mason`, `SHT_NOTE`, `SHF_EXCLUDE` | dropped | kept | kept | kept | identical | error: unknown note type with owner `Mason` |
| U `SHT_LOUSER+…`, `SHF_EXCLUDE` | dropped | kept | kept | dropped | identical | error: unsupported section type |
| K `SHT_PROGBITS`, no `SHF_EXCLUDE` | **copied into the output** | kept | kept | dropped | identical | clean |
| G P as a member of COMDAT group `g`, two objects with different bytes | dropped | **one copy survives**: the other object's bytes are lost | kept | dropped | identical | clean |
| R P with an absolute relocation against global `f` | dropped | kept, relocation kept | kept; after `--redefine-sym f=f2` the relocation names `f2` | dropped | identical | clean |

Two producer facts (`container`, `odr.md` Appendix A):

- llc 19 and llc 22 emit exactly encoding P (`.mason.meta`, `PROGBITS`,
  flags `E`, alignment 8) from the IR global
  `@m = private constant [N x i8] c"…", section ".mason.meta", align 8, !exclude !0`
  listed in `@llvm.compiler.used`.
- hod today refuses the same IR: `unsupported construct
  'global-metadata:exclude'`. Its object API can already write the
  section (`SectionKind::Elf{sh_type, sh_flags, sh_entsize}` with
  `SHF_EXCLUDE`); the request is to lower `!exclude` (`odr.md` §11, H-1).
- GNU as writes P from `.section .mason.meta,"e",%progbits`, so GCC and
  Clang can produce test units through inline or separate assembly
  without kiln (the test producer of §12).

## 3. The section

**MT-1 (name and type).** The container is the section named
`.mason.meta`, `sh_type` `SHT_PROGBITS`, `sh_flags` exactly
`SHF_EXCLUDE`, `sh_addralign` 8, `sh_entsize` 0, `sh_link` and `sh_info`
0. Chosen over a note (eu-elflint rejects an unknown note owner; a note's
4-byte framing adds 20 bytes and fixes alignment at 4) and over a
reserved section type (eu-elflint rejects it, and `spec/resolve.md` §10
refuses unknown section types in mortar itself, so a foreign Mason
section type would be a refusal in an old mortar); `SHF_EXCLUDE` because
every reference linker then drops it from final outputs (K is copied).
A reader finds the container **by name and type only** (and flags,
MT-6), never by its position in the section table: producers do not
agree on it (clang puts it at index 2 or 3 by target; hod does not
promise llc's position, `spec/odr.md` H-1).

**MT-2 (not in a group, no relocations).** The section is not a member of
any group and has no relocation section. Records refer to symbols and
types by name (MT-12), never by symbol or section index. Reason (the
crux, §6): a record must survive when its definition's COMDAT copy loses
the election, and `-r` by any reference linker keeps one copy of a group
(G); and a relocation would be renamed by `--redefine-sym` and renumbered
by `-r` (R), so a record that relied on one would mean something else
afterwards, silently.

What mortar does today (WP-071; measured on an aarch64 object): no
component checks MT-2 on its own terms. A section of the container's name
with `SHF_GROUP` set (a group member, as GNU as writes `"eG"`) fails
MT-6's flags test ("exactly `SHF_EXCLUDE`"): it is not a container, takes
`spec/resolve.md` §10's `SHF_EXCLUDE` row (dropped) and gives one
`meta-malformed` warning per file; its bytes are never read as units. A
container with a relocation section is classified as a container and
read as one; its relocation section is consumed with it (no relocation
is applied: the container has no output, MT-31/MT-33) and nothing is
reported. On Mason objects MT-2 is checked by the verifier's container
rules (WP-078).

**MT-3 (other linkers).** Final links by GNU ld 2.44, lld 19 and mold
2.37 drop the section and produce the same program as without it (P,
three targets). This is the principle-9 guarantee for Mason objects
linked by others; the L11 corpus re-runs it (§12, T-MT-3).

**MT-4 (how many).** An object has zero or more `.mason.meta` sections
(a producer writes one; a `-r` of several Mason objects by mortar writes
one, MT-30; an object assembled by hand may have several). A reader
reads all of them in section-index order (their relative order, never an
absolute index, MT-1); their units are one sequence.

**MT-5 (tools that keep it, tools that lose it).** Measured on objects
(`container`, `container-adv`; binutils 2.44, llvm 19 and 22, elfutils
0.192; re-run by T-MT-5, WP-071):

| tool and option | the container | the symbols its rows anchor to |
|---|---|---|
| `ar` (any), thin archives | kept, byte-identical | kept |
| `ld -r` (GNU ld, lld, mold) | kept, concatenated (§4) | kept |
| `strip -g`, `strip -x`, `strip --strip-unneeded` | kept | kept (globals) |
| `strip` (default), `strip --strip-all` | kept | **removed**: every row unanchored (`odr.md` O-4) |
| `objcopy --only-keep-debug`, `--localize-hidden`, `--weaken`, `--strip-dwo` | kept | kept (`--weaken`: weak) |
| `objcopy --prefix-symbols=…`, `--redefine-sym` | kept | renamed: rows unanchored (the safe direction) |
| `objcopy -N <sym>` | kept | that symbol removed |
| `objcopy --extract-dwo` | **removed** | |
| `llvm-strip` (default), `llvm-strip --strip-all` (objects and archive members) | **removed** | |
| `llvm-objcopy --strip-non-alloc` | **removed** | |
| `eu-strip` | **removed** | |
| `strip --strip-unneeded` on an archive | kept in every member | kept |

Only the "kept" rows with symbols kept preserve what `odr.md` A2 assumes.
An object that lost its container, or its symbols, links as a foreign
object (principle 9); **nothing reports the loss** (it cannot be told from
an object that never had one). T-MT-5 re-runs this table on the tool
versions of the harness.

**MT-6 (mortar's classification).** One predicate, used here and by
amendment A-1 to `spec/resolve.md` §10 (`odr.md` §14): a section **is a
container** when its name is `.mason.meta`, its `sh_type` is
`SHT_PROGBITS` and its `sh_flags` are exactly `SHF_EXCLUDE` (MT-1; the
alignment, `sh_entsize`, `sh_link` and `sh_info` are checked by the
reader, MT-14, not by the classification). A container is kind `Meta`,
state `Dropped(Consumed)`: read by `mortar_meta` only when a check that
needs it is on (MT-42), never copied by the `SHF_EXCLUDE` path. A section
named `.mason.meta` that is not a container (another type, `SHF_ALLOC`,
no `SHF_EXCLUDE`, `SHF_ALLOC|SHF_EXCLUDE`, …) falls through to the
existing rows of resolve.md §10 as if it had any other name (a
non-`SHF_EXCLUDE` `PROGBITS` is copied, as all three references do with
K; an `SHF_ALLOC|SHF_EXCLUDE` section is dropped by the `SHF_EXCLUDE`
row), and mortar warns `meta-malformed` (`odr.md` §9) once per file,
because a producer meant something and got it wrong. That warning is on
by default: it was run on every corpus of `odr.md` §12.3 (no object there
has such a section) with zero findings (`odr.md` O-1).

## 4. Record framing

All integers are little-endian, in every ELF class: the bytes of a unit
are the same in an ELF32 (i386) and an ELF64 object. Offsets are from the
start of the unit. Every structure below is 8-aligned and a multiple of
8 bytes, so `u64` fields are naturally aligned when the section is.

**MT-10 (units).** The section's contents are a sequence of **units**,
one per translation unit. At an 8-aligned offset, a reader finds either a
unit (its first four bytes are the magic `MASN`, `4d 41 53 4e`) or eight
zero bytes of padding, which it skips; anything else is malformed. A
unit is

| offset | field | meaning |
|---|---|---|
| 0 | `u8[4] magic` | `MASN` |
| 4 | `u16 major` | 1 |
| 6 | `u16 minor` | 0 |
| 8 | `u32 unit_size` | bytes of the unit including this header; a multiple of 8 |
| 12 | `u32 header_size` | 32 in 1.0; the first block is at this offset (a later minor may grow the header) |
| 16 | `u32 block_count` | |
| 20 | `u32 strtab_offset` | the unit's string table; 0 if none |
| 24 | `u32 strtab_size` | |
| 28 | `u16 producer` | 1 kiln, 2 hod, 3 mortar, 4 the test producer of §12, 0xffff another |
| 30 | `u16 flags` | 0 in 1.0; a reader ignores bits it does not know |

`-r` by every reference linker concatenates units unchanged, each
section's start aligned to 8 (P), which is why units are self-delimiting
and padding is zero words.

**MT-11 (blocks).** From `header_size`, `block_count` blocks follow one
another; each is

| offset | field | meaning |
|---|---|---|
| 0 | `u16 kind` | §7 |
| 2 | `u16 version` | of the kind's row layout, from 1 |
| 4 | `u32 block_size` | bytes including this header; a multiple of 8 |
| 8 | `u32 row_size` | a multiple of 8; 0 for a kind whose body is not rows |
| 12 | `u32 row_count` | |
| 16 | rows | `row_count × row_size` bytes, then kind-specific bytes up to `block_size` |

Within a unit a kind appears in at most one block, and blocks are sorted
by kind. Rows are sorted as the kind says (by key for every kind of §7),
so that a reader can merge and binary-search without sorting.

**MT-12 (strings).** The string table is NUL-terminated byte strings;
offset 0 is the empty string; a reference is a `u32` offset into it.
Strings are not required to be UTF-8 (names are bytes, as in `.strtab`).
A row names a symbol either by a string or, where the kind says so, by
**anchor**: the row's `key` is FNV-1a 64 of the name and the name is the
name of a symbol the same object defines (`odr.md` O-4). Anchored rows
cost no string bytes; MT-41 measures the difference.

**MT-13 (keys).** `key` fields are FNV-1a 64 (offset basis
`0xcbf29ce484222325`, prime `0x100000001b3`) over the name's bytes
without the NUL. A key is an index, never an identity: every reader that
groups rows by key confirms equality by comparing the names (`odr.md`
O-4). So a reader need not check that a named row's key is FNV-1a 64 of
its name (mortar's does not): a wrong key can only make the row miss its
group, never put it in a wrong one. **Versions** (one rule, also
`odr.md` O-4): the name is the symbol's name as the object's symbol
table spells it, so a version that `.symver` wrote into the object (`foo@V1`, `foo@@V2`) is part of the
name and of the key; a version that a version script assigns at link
time is not in any object and is not part of the key. It is part of the
identity wherever it exists, in a shared object's `.dynsym` (through
`.gnu.version`): a `.note.mason` row (`odr.md` O-51) is identified by
(name, version node), and two rows of one name with different version
nodes are different entities (`odr.md` O-24, O-54).

**MT-14 (validity).** A unit is malformed if its magic is wrong,
`unit_size` runs past the section or is not a multiple of 8, a block runs
past the unit, its blocks are not strictly increasing by kind (MT-11: a
kind twice, or out of order), `row_count × row_size` runs past its block,
the string table runs past the unit, overlaps the header or (when not
empty) does not start with the empty string (MT-12), a string offset is
past the table or a string has no NUL, or rows that must be sorted are
not. A malformed unit is skipped whole with the warning `meta-malformed`
naming the file and the unit's offset; the units after it are read when
its `unit_size` still says where the next one starts. After a wrong
magic, a header cut short by the section's end, or a `unit_size` that is
not usable (not a multiple of 8, smaller than a header, or past the
section), nothing says where a next unit would start: the reader gives
the one `meta-malformed` (naming the unit's offset and MT-10 or MT-14)
and reads no more of that section; the file's other containers are read
(MT-4). **Malformed metadata never
fails a link** unless the user asked for strictness (`odr.md` O-40): it
is additive.

## 5. References to symbols, sections and types

**MT-15 (by name only).** A record refers to a symbol by its name (string
or anchor), to a type by its mangled `<type>` encoding (Itanium C++ ABI
§5.1.5, as it appears after `_ZTS`), to a source file by a path string
(MT-38). No record holds a symbol index, a section index, an address or
a relocation (MT-2).

What that buys, measured (R): after `objcopy --redefine-sym f=f2` the
symbol is `f2` and an anchored or named record still says `f`; after
`-r` symbol indices change and names do not. A renamed symbol therefore
loses its records' anchor and its records are not compared by default
(`odr.md` O-4), which is the safe direction: renaming is how vendored
copies of a library are made distinct on purpose.

**MT-16 (sections).** A kind that must name a section of its own object
(the content hashes reserved for `incr.md`, kind 0x10) names it by
`(section name, group signature or empty, ordinal among the sections of
the unit with that name and signature)`, never by index, so that it
survives `-r`. [unverified: whether `-r` by the references preserves the
relative order of same-named sections from one input; mortar's `-r`
(M4) will.]

## 6. COMDAT membership: the crux

A record about an inline function or a template instantiation describes
a definition that lives in a COMDAT group. The question is whether the
record lives and dies with the group. It must **not**:

1. The check needs the definitions that **lose** the election. If the
   record were dropped with the losing copy, mortar would see one record
   per entity and could compare nothing: `comdat[G]` keeps one copy
   (`spec/resolve.md` §6.1) and the violation is exactly that the other
   copy differs.
2. Mortar could read records inside dropped members before dropping them,
   but no other linker's `-r` lets them reach mortar: GNU ld, lld and
   mold all keep one copy of a group under `-r` (G), so the evidence of a
   pre-linked object is gone before mortar sees it.
3. Grouping records per entity costs a section header (64 bytes in
   ELF64), a group entry and a section name per inline entity: 594
   entities in the heavy TU (`size`) is 38 KB of headers alone, paid by
   every linker on every link, before any record byte.

**MT-20 (one unit per TU, outside every group).** A producer writes the
records of a translation unit into one unit of one `.mason.meta`
section that is in no group (MT-2). Records describe every definition
the TU emits **whether or not its copy is kept**; which copy is kept is
the linker's business and the record says nothing about it.

**MT-21 (liveness).** A unit is read if its file is live
(`file_live`, `spec/resolve.md` §4). Its rows are not filtered by the
fate of the sections they describe: a row about a dropped COMDAT copy
or a section removed by `--gc-sections` is still compared (`odr.md`
O-20), so that the diagnostics do not depend on `--gc-sections` or on
which copy won.

## 7. Record kinds

| kind | name | rows sorted by | row size (v1) | owner |
|---|---|---|---|---|
| 0x0001 | TU summary | key string bytes | 16 | this spec, `odr.md` O-30 |
| 0x0002 | ODR definitions | `key`, then name bytes | 48 | `odr.md` §5 |
| 0x0003 | type definitions | `key`, then name bytes | 56 | `odr.md` §6 |
| 0x0004 | layout details | (not rows; addressed from kind 3) | 0 | `odr.md` §6 |
| 0x0005 | declarations | `key`, then flags | 40 | `odr.md` §8.2 |
| 0x0010 | section content hashes | (reserved) | — | `spec/incr.md` (WP-060) |
| 0x0011-0x001f | reserved for incremental linking | | | `spec/incr.md` |
| 0x0020 | build context | (reserved) | — | tuck |
| 0x0021 | entity index | (reserved) | — | tuck |
| 0x0022 | source context | (reserved) | — | tuck |
| 0x0023-0x002f | reserved for tuck | | | |
| 0x7f00-0x7fff | experimental: never in a released producer's output | | | |
| others | unassigned: added by amendment of this table | | | |

**MT-25 (unknown kinds and versions).** A reader skips a block whose
kind it does not know, silently. A block whose `version` is higher than
the reader knows is read if the kind says later versions only append
fields to the row (the rule for every kind here: a later version's row
starts with the earlier row, and `row_size` grows); otherwise skipped.
A reader never reads past `row_size` of a row. Kind 0x7f00-0x7fff is
skipped by every release reader.

**MT-26 (kind 1, TU summary).** Rows `{u32 name; u32 value; u32 flags;
u32 reserved}`, both strings, sorted by name bytes, names unique. Flag
bit 0 `ENFORCE`: two units whose rows for the same name both carry
`ENFORCE` and have different values are an ABI mismatch (`odr.md` O-30);
a row without it is information. Names defined now [kiln]:

| name | value | enforce | why |
|---|---|---|---|
| `producer` | `kiln 0.1.0 (hod 0.1.0)` | no | diagnostics |
| `source` | the main file's path after `-ffile-prefix-map` | no | diagnostics |
| `odr.scheme` | the definition-hash scheme, e.g. `kiln-odr-1` | no | rows of kinds 2, 3 are compared only between equal schemes (`odr.md` O-11) |
| `std` | the language standard, e.g. `c++20` | no | `odr.md` O-32 |
| `lang` | `c` or `c++` | no | `odr.md` O-33: C units get no kind 3 rows and C-linkage kind 5 rows are compared under C's rules |
| `abi.rtti` | `on`/`off` | **no** (a note: mixing `-fno-rtti` libraries with RTTI code is common and legal, `odr.md` N24) | K16c: `odr.md` O-25 reads it |
| `abi.exceptions` | `on`/`off` | no (mixing is supported; recorded for diagnostics) | |
| `abi.short-enums` | `on`/`off` | yes | K16a |
| `abi.stdlib` | `libc++`/`libstdc++` | yes | different `std::` layouts under the same names when inline namespaces are not used |
| `abi.stdlib-abi` | e.g. libc++ `_LIBCPP_ABI_VERSION`, libstdc++ `_GLIBCXX_USE_CXX11_ABI` | yes | |
| `abi.stdlib-debug` | `_GLIBCXX_DEBUG`/`_GLIBCXX_ASSERTIONS`/libc++ hardening mode, `off` | yes for the layout-changing modes (`_GLIBCXX_DEBUG`, libc++ debug ABI), no for the others | K16b |
| `abi.struct-return` | `reg`/`mem` (i386 only) | yes | K16e |
| `abi.char` | `signed`/`unsigned` | yes | |
| `abi.wchar` | size in bytes | yes | |
| `abi.long-double` | size in bits | yes | |
| `isa` | the target feature string the TU was compiled for | no | K16d: `odr.md` O-31 |

**MT-27 (kinds 2-5).** Their rows are specified in `spec/odr.md` §5
(kind 2), §6 (kinds 3, 4) and §8.2 (kind 5); the row sizes above are
binding on both specs.

**MT-28 (reserved kinds).** 0x0010-0x001f belong to `spec/incr.md`. WP-060's
draft does not need content hashes for correctness (its IN-22): mortar
hashes input sections itself when it needs to. A producer may still
emit 0x0010 later as an optimisation; its rows name sections as MT-16
says and must be verifiable (a wrong hash must cost a full relink, never
a wrong output). 0x0020-0x002f belong to tuck, which does not exist yet;
they are reserved now so that no other kind takes the numbers, for three
needs a debugger of an incrementally linked C++ program will have and a
linker can carry cheaply: **build context** (the TU's command line and
working directory, so that an expression can be compiled in the TU's
context), **entity index** (a stable id per entity across relinks, so
that breakpoints survive an incremental link), **source context**
(macro definitions and the `#include` graph, which DWARF holds only
partially). Nothing is designed for them here.

## 8. Versioning and compatibility

**MT-35 (major).** A reader reads units of the majors it knows (1).
A unit with another major is skipped with the warning `meta-version`,
once per file; a link never fails for it. A major changes only when a
1.x reader would misread a 2.0 unit; adding kinds, header fields, or
row fields never does. Every major keeps the magic at offset 0, `major`
at offset 4 and `unit_size` at offset 8 (MT-10), so that a reader of any
major can recognise a unit and skip one it does not read.

**MT-36 (minor).** A producer writes the lowest minor that has every
feature it uses. A reader of minor `m` reads a unit of minor `n > m`:
fields after the ones it knows are skipped by `header_size` and
`row_size`.

**MT-37 (schemes).** What a hash means is not versioned by the container:
kind 1's `odr.scheme` names it, and rows of different schemes are never
compared (`odr.md` O-11). A kiln release that changes what its hash
covers changes the scheme name; old and new objects then link without a
check between them, and with a note in `--odr-stats` (`odr.md` O-44).

## 9. Determinism

**MT-38.** A unit is a function of the TU's preprocessed source, the
options that change its meaning, and the producer's version: not of
time, the host, absolute paths, the order of hash-map iteration or the
number of threads. Paths in records (`source`, a row's file) are written
after `-ffile-prefix-map`/`-fdebug-prefix-map` (kiln has
`-ffile-prefix-map`, `kiln_driver/src/args.rs`); a producer without a
map writes the path as given on its command line, never made absolute.
Rows are sorted (MT-11); strings are laid out in the order the sorted
rows first reference them; padding is zero. Test T-MT-38: compiling one
TU twice, in two directories with the same map, gives identical
sections.

## 10. Size budget

Measured on a libc++-heavy TU (`size`: `<algorithm>`, `<functional>`,
`<map>`, `<memory>`, `<sstream>`, `<string>`, `<unordered_map>`,
`<vector>`, `<variant>`, `<optional>` and some uses):

| producer | object | global definitions | undefined | vague-linkage symbols | their names | classes in DWARF (`-g`) |
|---|---|---|---|---|---|---|
| kiln+hod, libc++, `-O0` | 475,720 B | 595 | 66 | 594 | 78,221 B | 334 |
| kiln+hod, libstdc++, `-O0` | 415,448 B | 581 | 43 | 580 | | |
| clang++ 22, libc++, `-O0` | 392,928 B | 606 | 67 | 605 | | 292 |
| clang++ 22, libc++, `-O2` | 50,184 B | 46 | 56 | 45 | | |
| g++ 14, libstdc++, `-O0` | 420,144 B | 705 | 43 | 704 | | 302 |

The container for the kiln `-O0` object, with the rows of `odr.md`:

| part | rows | bytes | basis |
|---|---|---|---|
| header, kind 1 | ~16 | ~0.6 KB | |
| kind 2, anchored (MT-12) | 595 | 28,560 | 48 B a row; names in `.strtab` already |
| kind 2, named instead | 595 | 106,781 | + the 78 KB of names: why anchoring is the default |
| kind 3 | 334 | 18,704 + ~13 KB of type names | 56 B a row; 40 B a mangled type name assumed [estimate] |
| kind 5, anchored | 661 | 26,440 + ~5 KB of type strings | 40 B a row, definitions and references; type strings deduplicated [estimate] |
| kind 4 (optional) | 334 | ~40 KB [estimate: 120 B a class] | only with `-fmason-metadata=detail` |

Measured in that table: the object sizes, the entity counts, the 78,221
bytes of names (`size`), the `-O0 -g` object of 1,039,856 B (kiln+hod
with `KILN_BACKEND=hod`, reproduced by the review), and the type count
under the widened reach of `odr.md` O-17: clang 22's DWARF of the same TU
has 301 complete class/struct/union/enum definitions at `-g` and 334 at
`-g -fstandalone-debug` (pointees included; `k3count`), so the 334 rows
above stand as the upper proxy [estimate: a proxy until kiln writes kind
3]. Everything in the "bytes" column after kind 2's is arithmetic over the
row sizes and assumed name lengths: `[estimate]`.

**MT-41 (budget).** Measured by WP-071's acceptance test (`odr.md` §13,
experiment 7: the test producer at the row counts of the objects;
`spec/landed/meta/WP-071.md` §5.2): without kind 4, the heavy TU's unit
is **112,392 B, 23.6 % of the `-O0` object and 10.7 % of the `-O0 -g`
one** (kind 1 112 B; kind 2 28,528 B, 594 rows; kind 3 16,928 B, 302
rows; kind 5 26,456 B, 661 rows; strings 40,335 B), and protoc's 168
units are 14.57 MB, 31.4 % of its objects. The table's estimate above
(~92 KB, 19 % and 9 %) assumed 40 B a type name; kind 3's qualified
names measured 132 B a name (g++'s DWARF of the same TU, a proxy until
kiln writes kind 3), which is the difference. Kind 5's type strings are
not in these numbers (nothing gave the test producer a declared type).
(`odr.md` Q4's "3-6 %" is kinds 1 and 2 alone, anchored.) **The gate**
(the L11 lane, on kiln's units of the heavy TU): a row is 16 B (kind 1),
48 B (kind 2), 56 B (kind 3), 40 B (kind 5), the v1 row sizes; kind 3's
names at most 160 B a row on average; kind 5's type strings at most
8 KB a unit; the unit (kinds 1, 2, 3 and 5) at most **125,000 B** and at
most **12 %** of the `-O0 -g` object. It is measured again when kiln
writes kind 3. What a link pays with the checks off is none of it:
MT-42.

**MT-42 (a link that does not use the feature does not pay for it).**
A hard rule (the operator's, 2026-11-15). Mortar reads a container's
bytes only when a check that needs them is on (`odr.md` O-42): with
`--odr=off`, and in the default mode as long as every record-based check
is `--odr=strict` only (`odr.md` O-47), **a link pays nothing for the
containers**: mortar parses no unit and keeps no container byte. This
needs amendment **A-8** to `spec/input.md` §11.1 (`odr.md` §14): the
staged store's row "every other section of at most 64 KiB" excludes
containers (MT-6's predicate); a container is never resident; when a
check needs it, `mortar_meta` reads it with positioned reads
(`copy_to`-style, `read_exact_at`) into memory it owns for the check's
duration. The reads (the supervisor's ruling, 2026-11-15, WP-071):

With containers unwanted, mortar makes exactly the reads it would make
if no section were a container (a small file whole; a large object's
head and tail windows, then its body ranges). Container bytes that fall
inside those reads are read and dropped: never resident, never parsed.
No other container byte is read: a large object's body ranges are
clipped around its containers.

So a link whose inputs have no container pays nothing that grows with
the input: the same reads, range for range, as with the containers
wanted. (A first version read each top-level object's headers first and
the rest around its containers, so that no container byte was read at
all; it cost every container-free object three reads more, measured at
about 5.5 ms, 7 %, on 5,000 small objects at one thread: WP-071's
record §5.5.1.) When a check that needs the container is on, a small file
stays resident whole, container included (it is then needed). T-MT-42
checks both, pinned by `crates/mortar_input/tests/t_mt_42.rs`
`container_free_files_cost_what_they_cost_without_the_rule` and
`no_container_byte_is_resident`. The cost the rule leaves is the file's
size on disk and in the page cache of whoever wrote it.

Two named exceptions (accepted 2026-11-15, WP-071); T-MT-42's byte
counter counts each, and a test pins each:

- **E2 (archive members).** A regular archive's members are read before
  their section headers are known (`spec/input.md` §11.1): a member of
  at most 64 KiB is read whole and stays resident, its container with
  it; the chain's windows and a larger member's head and tail reads may
  cover container bytes, which are not kept; a container of a member
  larger than 64 KiB is never resident. With the containers wanted,
  those bytes may be read a second time. Reading every member's section
  headers first would make every archive link pay, Mason or not.
  Measured on protoc's archives: 11.4 % of the container bytes read,
  2.7 % resident (WP-071's record §5.3). Pinned by `t_mt_42.rs`
  `small_member_container_is_the_archive_exception` and
  `member_container_is_not_resident`.
- **E3 (whole-file reads).** A file made fully resident because
  descriptors ran out (`spec/input.md` §11.4) and every file under the
  resident policy are read whole, containers included. Pinned by
  `t_mt_42.rs` `whole_file_paths_are_the_named_exception`.

(E1, the first 64 bytes of a file read before its class was known, went
with the first version: the head is now one of the reads the rule
names.)

## 11. The container in mortar's outputs

One rule per output kind; `odr.md` §10 owns the loadable table.

**MT-30 (relocatable output, `-r`, M4).** The units of every live input
are concatenated, in `FileId` order, into one `.mason.meta` section that
satisfies MT-1, unchanged (mortar does not re-encode a unit it did not
write; a malformed unit is dropped with its warning, MT-14). This is
what the three references do (P) and keeps the losers' records (§6).

**MT-31 (executables and shared objects).** The container is consumed
(MT-6) and not copied. If the loadable table is on (`odr.md` O-50), the
output gets the allocated note `.note.mason` in a `PT_NOTE` with
`p_align` 8 (`odr.md` O-52; it may share that segment with other
8-aligned notes, as `spec/layout.md` places notes); shared-object
outputs are M4.

**MT-32 (kept for tools).** With `--keep-mason-meta` (proposed,
`spec/cli.md`, amendment A-5 of `odr.md` §14; default off), an executable or shared
object also gets a non-allocated `.mason.meta` (`SHT_PROGBITS`, no
flags) holding MT-30's concatenation, for tuck. Not needed for
incremental links (`spec/incr.md` keeps state in its own file).
[unverified: what `strip` does to such a section in an executable.]

**MT-33 (static executables).** As MT-31 without a table: a static
executable has no other module to disagree with, and the static check
has seen every definition (`odr.md` O-20).

## 12. Tests

- **T-MT-3** (L11, per target): a Mason object (test producer) linked by
  GNU ld, lld and mold: same program, section absent.
- **T-MT-10..14** (`mortar_meta` unit tests): framing round trips; every
  malformation of MT-14 produces exactly one `meta-malformed` and the
  neighbouring units are read; zero padding between units; several
  `.mason.meta` sections; ELF32 and ELF64 give identical units.
- **T-MT-25, 35, 36**: unknown kind skipped silently; `version` 2 row
  with a longer `row_size` read as version 1; major 2 skipped with
  `meta-version`; minor 1 with a longer header read.
- **T-MT-30** (M4): `mortar -r` of two Mason objects, then a final link
  by mortar: the same diagnostics as linking the two objects directly.
- **T-MT-38**: determinism, above.
- **T-MT-42** (bytes read, not only outputs): with `--odr=off`, and in
  the default mode while O-47 lists no record-based check as on, a link
  of objects whose `.mason.meta` bytes are garbage (valid section
  header, random contents) succeeds without a warning and produces the
  same output as with the sections removed; and the store's read
  counter (`spec/input.md` §11, a test-only counter per file range)
  shows the same reads as with the containers wanted, no read wholly
  inside a container, no container byte resident, and of a large
  object's containers only the bytes inside its head and tail windows
  read (none for a container in its body). With a check on that needs
  the container, the counter shows each container read once (twice
  where a large object's head or tail window covers it: the window's
  bytes are dropped, `mortar_meta` reads the container again).
- **T-MT-5**: MT-5's table, re-run on the harness's tool versions
  (binutils, llvm, elfutils): a changed row fails the test and is a spec
  change.
- **T-MT-6**: one object per non-container variant (`SHF_ALLOC`, no
  `SHF_EXCLUDE`, `SHF_ALLOC|SHF_EXCLUDE`, `SHT_NOTE`): classified by the
  fall-through rows, one `meta-malformed` per file.
- **T-MT-1, 2, 4, 11-16, 20, 21, 26-28, 31-33, 37, 41**: MT-1/2 by the
  test producer's output read back; MT-4 two sections in one object;
  MT-15/16 after `objcopy --redefine-sym` and `-r`; MT-20/21 a unit whose
  COMDAT copy lost and a unit of a file whose sections `--gc-sections`
  removed are both compared; MT-26..28 kind 1 keys round trip; MT-31..33
  per output kind; MT-37 two schemes, no comparison; MT-41 the gate of
  WP-071's record.

**The test producer.** `mortar_meta` has a writer (it needs one for
MT-30); the test harness uses it to emit `.s` files (`.section
.mason.meta,"e",%progbits` and `.byte` lines) that GCC, Clang and GNU as
assemble, so that every rule here and in `odr.md` is testable before
kiln and hod emit anything (`odr.md` §13, WP-071).
