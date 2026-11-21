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

Stage 1 began with `-static` executables that are not PIE, on AArch64;
the table below says what each stage covers now (dynamic executables and
static-pie are stage 3, §12 and §12.4). §4.5 describes how x86-64 and
i386 fit.

## 1. Scope and the stages

| stage | covers | not-checked reasons it removes |
|---|---|---|
| 1 | `ET_EXEC`, static, non-PIE, AArch64. Placement, relocations (the §4.2 table), the GOT slots of a static link, structure (§6), content, symbol values | — |
| 2 (WP-028) | Static TLS: every AAELF64 TLS type and the relaxations W9–W13 (§4.7); IFUNC/`IRELATIVE`, `.eh_frame` and `.eh_frame_hdr` contents (§6.4), the symbol table (§6.5), non-alloc (DWARF) relocations | `tls` (except the constructs §4.7 names), `ifunc`, `eh-frame` (except labels inside `.eh_frame`, §6.4), `non-alloc` |
| 3 (M2) | Dynamic executables (`Exec`, `Pie`) and static PIE: the dynamic sections, dynamic relocations, PLT, copies, shared inputs (§12, P-D1-P-D13; static-pie P-D14 and P-D15, §12.4). Shared-object output is M4 | — |
| 4 (M6) | Mason metadata and the one-definition-rule table: `.mason.meta` in outputs, `.note.mason` (§13, P-M1-P-M3; WP-078); an output with the load-time runtime (`--odr-runtime`): the runtime's note, its input read from the verifier's own copy, its entry, exports, imports and sections (§13.1, P-M2, P-M3; WP-197) | — |
| x | x86-64 and i386 tables (§4.5) | `machine` |
| G (WP-098) | `.gdb_index`: gdb's format for every producer (P-G1–P-G5) and synth.md §17 for mortar's (P-G6–P-G9), §6.10. `.debug_names` is not judged | — |
| R (M4-r; RL-5, WP-185; P-P1: RL-6, WP-186) | Relocatable outputs (`-r`, `ET_REL`): every input section and relocation once, the symbol table, `.eh_frame`, groups, the header, the container, the notes, every section header (§14, P-P2-P-P11, accepted by P-P9; `mortar_plumb::relocatable::verify`); the final program through `-r` against the direct link, on two final links (P-P1, `mortar_plumb::compose::compose`) | — |
| I (M7, WP-091) | An incremental link's holes and dead ranges, for a mortar map that claims them (`incremental`, `hole`, `dead`, §3.2): IN-60's coverage of every allocated section and IN-3a's three checks on every range (§6.8, P-I1–P-I6), the `.symtab` blocks its decision records state (P-I7, WP-268), and its other decision records against the output (P-I8, WP-210). `--equiv` (incr.md IN-62) is not started | — |

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

One input is never the caller's (WP-197): the load-time ODR runtime,
which mortar adds under the name `<mortar:odr-runtime>` (`spec/cli.md`
C-ODR-RT). When a mortar map names that file and its line asks for the
runtime, the verifier reads it from its own copy of the object (§13.1),
whatever the caller supplies by that name; otherwise the name is a path
like any other.

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
| `commons` | `name`, `addr`, `size`, `align` | one common symbol's allocation (mortar's format only, §3.2's allocation records; a foreign map's COMMON is a `common` chunk) |
| `defined` | `name`, `value` | a symbol the linker defined |
| `decisions` | `region`, `capacity`, `order`, `slot`, `redirect`, `frag`, `symblock` rows | an incremental-mode layout's decision records (mortar's format only, §3.2; `spec/image.md` I-78a) |

A section is named by its index in its file (mortar's format) or by its
name (the foreign formats). §3.3 resolves names.

### 3.2 mortar's format, version 2

The format is line-oriented UTF-8 text. Each record is one line of tokens
separated by single spaces, and the map ends with `end`:

```text
mortar-map 2
file <id> <path> [<member> <member-header-offset>]
place <file> <section> <addr> <size>
piece <file> <section> <input-offset> <addr> <size>
nplace <file> <section> <output-section> <offset> <size>
merged <file> <section> <output-section>
discard <file> <section> gc|comdat|script|unspecified
fold <file> <section> <into-file> <into-section>
chunk thunk|patch|synthetic|common <name> <addr>|- <size> [<target> <addend>]
common <name> <addr> <size> <align> [<file>]
define <name> <value>
incremental
hole <output-section> <offset> <size>
dead <output-section> <offset> <size> neutral|contribution|strings|cie
region <file> <out> <cat> <addr> <capacity>
capacity <out> <ordinal> <part> <addr>|- <size> <capacity>
order <out> <n>...
slot <kind> <key> <addr>
slot <kind> <file> <index> <addr>
slot tls-ld <addr>
redirect <file> <section> <offset> <addr>
frag <out> <class> <hash> <offset>
symblock <file>|- <first> <count> <capacity> <strtab-off> <strtab-cap>
output static|exec|pie|static-pie|shared|relocatable
rplace <file> <section> <out-index> <offset> <size>
rframe <file> <section> <input-offset> <output-offset>|-
rgroup <out-index> <file> <section>
rcommon <name> <out-index> <offset> <size> <align>
interp <path>
runpath|rpath <paths>
bind lazy|now
relro on|off
symbolic none|all|functions
dynamic-list <path>
version-script <path>
arg <word>
needed <name>
copy <name> <addr> <size> <align>
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
  lone `%`. A decoded string must be UTF-8, except an `arg` word: a word
  of the line is bytes (`spec/cli.md` §2.1), so a word with a space, a
  newline or bytes that are not UTF-8 is written and read exactly
  (`%20`, `%0A`, `%FF`).
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

**The dynamic records** (§12 P-D3; WP-040 step 0d) follow
`define`. A static link writes none of them but `arg`, which every map
has (**the line records**, §3.2a; WP-102); a dynamic link (`Exec`,
`Pie`, `StaticPie`) writes them, each package filling its own. Every
string is a string token. The `arg` records keep their place in the
grammar's order in every map: in a static map they follow `define` and
the incremental records and end the map.

| record | count | meaning |
|---|---|---|
| `output <kind>` | at most one | the output kind of `dyn.md` D-1: `static`, `exec`, `pie`, `static-pie`; `shared` (planned, WP-116); `relocatable` (M4-r: read by stage R since RL-5, WP-185; mortar writes it since RL-7, WP-187, on AArch64: its `-r` of x86-64 and i386 is refused until RL-9; the `-r` records `rplace`, `rframe`, `rgroup`, `rcommon` and which records a relocatable map may hold are §14.2) |
| `interp <path>` | at most one | the `-dynamic-linker` path |
| `runpath <paths>`, `rpath <paths>` | at most one of the two | the `-rpath` list joined by `:` as written (`$ORIGIN` kept); `rpath` under `--disable-new-dtags` (`DT_RPATH`) |
| `bind lazy`, `bind now` | at most one | `-z now` in force or not |
| `relro on`, `relro off` | at most one | `-z norelro` in force or not |
| `arg <word>` | one per word, in every map (§3.2a) | the link's command line, one word per record, in order, with response files expanded and the program name left out |
| `needed <name>` | any | **hint**: a `DT_NEEDED` name, in the order written |
| `copy <name> <addr> <size> <align>` | one per `COPY` record | **hint**: a copy allocation, by the name its `COPY` record names; `<align>` is its alignment, stated (the allocation records below, M8) |
| `export <name>` | any | **hint**: an exported name |
| `plt <name>` | any | **hint**: the key of a PLT entry, in PLT order (`name` or `name@version`) |
| `iplt <name>` | any | **hint**: the key of an IPLT entry, in IPLT order |

The first five records are facts of the command line and the sixth,
`arg`, is the line itself: the verifier recomputes each of the five
from the `arg` words (or, for a reference link, from its response file)
and reports a record that disagrees. The **hints** are
never the expected value: every decision they name (the needed set and
order, the copies, the export set, PLT order) is recomputed from the
inputs (§12), and a hint only names what is compared. A
second `output`, `interp`, `runpath`/`rpath`, `bind` or `relro` record
is a `MapError`.

**The shared-object line records** (planned, WP-116; read by WP-121
for §12.5). A `-shared` link (`dyn.md` D-1's `Shared`, `output shared`)
writes three more facts of its line, after `relro` and before `arg`;
every other map writes none of them, so no existing map changes. They
are facts of the command line like the first five dynamic records: the
verifier recomputes each from the `arg` words and reports a record that
disagrees. Today no link writes them (`-shared` is refused, `cli.md`
§4.2): the writer is `mortar_image::map::{DynRecords::shared,
SharedLine}` with its tests (`map::tests::s3_2_shared_line_records`,
`s3_2_no_shared_line_records_outside_shared`); the reader is WP-121's.

| record | count | meaning |
|---|---|---|
| `symbolic none\|all\|functions` | exactly one in a `Shared` map, none in any other | the line's symbolic form as `dyn.md` D-112 resolves it: the last of `-Bsymbolic` (`all`), `-Bsymbolic-functions` (`functions`) and `-Bno-symbolic` (`none`), `none` without any. `all` with a `dynamic-list` record does not write `DF_SYMBOLIC` (D-112); the reader derives that from the two records, nothing more is written. A line whose last form is `-Bsymbolic-non-weak` or `-Bsymbolic-non-weak-functions` is refused (D-112) and writes no map |
| `dynamic-list <path>` | one per `--dynamic-list` file, in line order | the path as the line names it (response files expanded, not searched); a `Shared` map only |
| `version-script <path>` | one per `--version-script`/`-version-script` file, in line order | as `dynamic-list`; a `Shared` map only |

A second `symbolic` record, an unknown form, a wrong number of fields,
or any of the three in a map whose `output` is not `shared` is a
`MapError`.

**The incremental records** (§6.8; `spec/incr.md` IN-3a, IN-60;
WP-091) follow `define` and come before the dynamic records. A link
that is not incremental writes none of them, and its map claims nothing
(the checks of §6.8 do not run).

| record | count | meaning |
|---|---|---|
| `incremental` | at most one | the link was made in incremental mode: the map claims IN-60's coverage even with no `hole` or `dead` line |
| `hole <output-section> <offset> <size>` | any | an allocated range that holds its section's fill (IN-3): room an update left, a dead input section's or a dead constant fragment's bytes; or the zero file slack past the end of `.symtab` or `.strtab` (IN-49a, §6.8) |
| `dead <output-section> <offset> <size> <kind>` | any | a dead range whose bytes are a record of its section's grammar (IN-3a check 2), by `<kind>`: `neutral` (IN-51's block unit or IN-51b's neutral form of a debug section), `contribution` (a whole dead debug contribution as its compiler wrote it, addresses tombstoned, IN-53), `strings` (dead fragments of a merged string section, kept bytes, L-147), `cie` (unused CIEs in `.eh_frame`) |

All slack is listed as holes, and so is the start padding of
`layout.md` L-118 (the 843419 padding before a synthetic code chunk on
AArch64: trap fill that no item owns and no alignment explains, from the
end of the chunk before it in the same section): an incremental-mode
map lists it as a `hole`, which §6.8 then judges like any other (P-I5's
fill; P-I1 counts it covered). A plain map is unchanged: it claims no
coverage, and §6.8 does not run on it (WP-062's finding 8; the
`incr-e843` program of `crates/mortar/tests/incremental.rs`).

`<output-section>` is a string token naming the output section;
`<offset>` is from the section's start (`sh_addr`, or 0 for a non-alloc
section) and `<size>` its length, both hex. A map with `incremental` or
any `hole`/`dead` line **claims coverage**. A second `incremental`, an
unknown kind, a wrong number of fields or a bad number is a `MapError`.
What only the output can judge (no such section, a range outside it,
empty, overlapping another line) is read and judged by §6.8 (P-I2).

**The allocation records** (WP-096) state three facts of the layout
that neither the output nor the other records give, and that §6.8's
coverage would otherwise have to infer: where a `NOBITS` section the
linker makes lies, the alignment of each common symbol's allocation and
the alignment of each copy. Every mortar map writes them, a plain link's
as an incremental-mode one's, so the verifier has one path, and each is
checked against the output by a map rule (§3.3, M6–M8) whether or not
the map claims coverage.

| record | count | meaning |
|---|---|---|
| `chunk synthetic <name> <addr> <size>` | one per linker-made `NOBITS` section | the memory of a `NOBITS` synthetic chunk that holds no allocation of its own: today `.pad.relro` (`spec/layout.md` L-64, L-156). The copy chunks (`.dynbss`, `.bss.rel.ro`'s, `spec/dyn.md` D-42) are not listed: their allocations are `copy` records. The record is the existing `chunk` form, so it has no new grammar |
| `common <name> <addr> <size> <align> [<file>]` | one per common symbol allocated | a common symbol (gABI `SHN_COMMON`, x86-64 `SHN_X86_64_LCOMMON`) the link allocated (`spec/layout.md` L-88): `<name>` its symbol, `<addr>` and `<size>` its allocation, `<align>` the alignment it was placed at; `<file>`, the id of an earlier `file` record, is the common's winning file (`spec/incr.md` IN-72 item 11), written in a map that says `incremental` and only there. After the `chunk` records |
| `copy <name> <addr> <size> <align>` | one per `COPY` record | the copy allocation (above), with `<align>` the alignment it was placed at (`spec/dyn.md` D-42) |

`<align>` is hex as every other number, a power of two (1 included). A
`common` or `copy` record with an `<align>` that is not a power of two,
or a wrong number of fields, is a `MapError`. A `common` record has five
fields exactly in a map that says `incremental` and four elsewhere: a
fifth field in a plain map, its absence in an incremental-mode map, or
a `<file>` that names no earlier `file` record is a `MapError`.
**Transitional (WP-222, ruled; reworded by WP-210):** `<file>` is
optional until IN-72 item 11 (an update that places commons) is built:
in v1 an update places no common (`spec/incr.md` IN-72 item 11, WP-216
R-2: `update-not-implemented (common)`), so no update's map has a
`common` record to write it in, and a `common` without `<file>` is a
`MapError` only in a map that has a decision record. The full links of
the incremental mode write it (measured, WP-210: the kept outputs of
the incr-update lane, 72 `common` records in maps that say
`incremental`, every one with `<file>`; 32 in plain maps, none). When
item 11 is built, a `common` without `<file>` in an incremental map is a
`MapError` and this paragraph goes. A plain
link's map is unchanged by the field (not one byte). A foreign map never has
them (§3.4 to §3.6 do not change): GNU ld's and lld's `COMMON` and
mold's `.common` stay `common` chunks, one per file, and their maps
state no alignment.

**Versions.** The incremental records were added within version 1, as
the dynamic records were (WP-040): a reader of version 1 that predates
them refuses a map that has them with `MapError` "unknown record 'hole'"
(or `'incremental'`, `'dead'`), since a record it does not know is an
error, never skipped, and their absence claims nothing (a map without
them claims no coverage), so no reader misreads either kind of map.

The allocation records make **version 2** (WP-096). They do not only add
lines: they change what a missing line means. In version 2 a map with
no `common` record says the link allocated no common, and a `copy`
record without `<align>` is malformed. A version 1 map, written before
the records existed, says neither. A reader of version 2 that took a
version 1 map would judge a correct link by a fact the map never
stated, as a missing record; a version 2 reader therefore refuses
version 1 by its number ("Versions" above), and a version 1 reader
refuses version 2 the same way. Version 2 is version 1 with the
allocation records; nothing else changes.

The decision records (WP-212, `spec/image.md` I-78a) are added within
version 2 as the incremental records were within version 1: a version 2
reader that predates them refuses a map that has them with `MapError`
"unknown record 'region'" (or another of their words), never skips them,
and only an incremental-mode map has them, so no plain map changes.

**The decision records** (`spec/image.md` I-78a; `spec/incr.md`
IN-78, IN-79 (d); WP-211's §R.12, applied by WP-212) follow the
`incremental`, `hole` and `dead` records and come before the dynamic
records, only in a map that says `incremental`; a decision record in a
map without `incremental` is a `MapError`. A plain map has none and is
unchanged. They state `P_U`, the update's `Placement`, for H-1's
judgement (d); the verifier reads them and judges only what follows.

| record | count | meaning |
|---|---|---|
| `region <file> <out> <cat> <addr> <capacity>` | one per file region | a file's region in a hole-tolerant output section (IN-30): `<cat>` decimal (L-53's ordinal, 0 elsewhere), `<addr>` and `<capacity>` hex |
| `capacity <out> <ordinal> <part> <addr>\|- <size> <capacity>` | one per chunk of an output section | `<ordinal>` decimal; `<part>` `b<k>`, `p<k>`, `m` or `s` (`k` decimal); `<addr>` hex or `-` (non-allocated) |
| `order <out> <n>...` | one per output section | the chunks' ordinals (decimal) in file order; at least one |
| `slot <kind> <key> <addr>`, `slot <kind> <file> <index> <addr>`, `slot tls-ld <addr>` | one per slot | `<kind>` `got`, `got-tprel`, `tls-gd`, `tls-desc`, `tls-ld`, `plt`, `iplt`; a global key by name, a local by file id and symbol-table index (decimal), the module slot without a key |
| `redirect <file> <section> <offset> <addr>` | one per redirect | a site and the address of the patch it names |
| `frag <out> <class> <hash> <offset>` | one per live fragment of a merged section | `<class>` three hex numbers joined by `,`; `<hash>` exactly 32 lower-case hex digits; `<offset>` hex |
| `symblock <file>\|- <first> <count> <capacity> <strtab-off> <strtab-cap>` | one per `.symtab` local block | entry numbers decimal, `.strtab` offset and size hex; `-` the internal file |

`<out>` and `<key>` are string tokens; `<file>` an earlier `file`
record's id. A wrong number of fields (`slot`: 2, 3 or 4 after the
word), a bad number, a `<part>` or `<kind>` outside its list, a `<hash>`
of another length, or a `<file>` that names no earlier record is a
`MapError`. **What the verifier judges**
(§6.8, with the other incremental records): a `region` whose `<out>`
names no output section or several, or whose `[<addr>, <addr> +
<capacity>)` is not inside `[sh_addr, sh_addr + sh_size]` of its section,
is P-I2; so are two `region`s of one output section whose ranges share
a byte (WP-222: regions are disjoint by IN-30, so no correct writer
states two that meet), and a `symblock` out of place (WP-268; IN-49a,
S-74, S-86): one whose `<first> + <capacity>` exceeds `.symtab`'s
`sh_info` (WP-222: a block and its reserve lie in the local area), one
at entry 0 (the blocks follow it), one whose `<count>` exceeds its
`<capacity>`, one whose `.strtab` region `[<strtab-off>, <strtab-off> +
<strtab-cap>)` is not inside `[0, sh_size)` of the `.strtab` that
`.symtab` links, one in an output with no `.symtab`, a second record of
one `<file>` (or of `-`), and two blocks that share an entry of
`[<first>, <first> + <capacity>)` or a byte of their regions. A
`symblock` in place is then judged against the output's `.symtab` and
`.strtab`, its entries and their names (§6.8, P-I7). The `capacity`,
`order`, `slot`, `redirect` and `frag` records are judged against the
output's bytes and the map (§6.8, P-I8; WP-210, V-3). **Not judged** by
the verifier, since only the linker's state holds the answer: the
ordinals' creation order, the capacities' amounts (the bound and the
reserve's room are judged), a slot no live reference uses beyond its
place and key (and, in an output without `PT_DYNAMIC`, its word), the
identity order of `frag` records, whether a block's `<capacity>` and
`<strtab-cap>` are IN-49a's sizes, a block's `<file>` against the file
its entries came from (an entry carries no file, an `STT_FILE` name is
not a path), and the records' order (H-1's (d) compares them with the
state, IN-78); the coverage line counts and names them (`decision
records: n read; region judged (place only), symblock judged (place,
names), capacity, order, slot, redirect, frag judged (P-I8); not judged:
the ordinals' creation order, the capacities' amounts, a slot no live
reference uses beyond its place, frag identity order; P-I8: …`, §7.2;
until WP-210 it read `capacity, order, slot, redirect, frag not
judged`). The
`slot` form with two fields after the word is `tls-ld`'s only (the
module slot has no key): another `<kind>` there is a `MapError`. No record name collides with another record of this
format (§3.2's grammar) or with the formats of §3.4-§3.6 (each read
only under its own header; their text is a table, not record words).

The writer emits records in a fixed order: files by id, then places and
pieces, the non-alloc records, discards, folds, chunks, commons and
defined symbols, each group in the order of the map's vectors, then
`incremental`, the `hole` and `dead` lines in the order of the map's
vector, the decision records (in `region`, `capacity`, `order`, `slot`,
`redirect`, `frag`, `symblock` order, each group in `spec/image.md`
I-78a's order), then the dynamic records in the order of the grammar
above.

Every syntax error is a `MapError { line, message }`. Line 0 means the map
was truncated, with no `end`. Tests:

- `map::mortar::tests::s3_2_round_trip`
- `s3_2_tokens_encode_every_byte`
- `s3_2_malformed_maps_are_errors`
- `s3_2_dynamic_records_round_trip`, `s3_2_malformed_dynamic_records_are_errors`
- `s3_2_dead_records_round_trip`, `s3_2_malformed_dead_records_are_errors`
- `crates/mortar/tests/image.rs map_dynamic_records_read_by_the_verifier`
  (the writer side, `mortar_image::map::DynRecords`)
- `s3_7_truncations_are_answers`
- `tests/corpus.rs s3_2_reference_maps_round_trip_through_mortar_format`

### 3.2a The line's options (WP-102)

A mortar map carries its link's command line as `arg` records (§3.2),
in **every** map: static, static-pie, dynamic, plain or incremental. The
verifier's rules that hold only when an option was given read the
option through one reader (`src/opts.rs`) from **the caller's line**
when the caller gives one (`Ctx.line`: `xtask plumb --line`, the
`--inputs-from` response file, or the link test's recorded words), for
every format, and from a mortar map's `arg` records only when the
caller gives none (§12.1a's rule, WP-193; WP-201: before it, a mortar
map's records were the options even when a line was given, so a map
whose records dropped, added or changed an option moved the verdict).
Never from the output, nor from a chunk that only exists because of
the option. A reference linker's map has no line; without the caller's,
a rule that reads one is judged as the table says. A mortar map has at
least one `arg` record (a link of no word is `no-input-files` and
writes no map): a map without one, as a static link's map before
WP-102, is malformed (§3.7), not judged without its options. The map's
records are judged whatever the caller gives (M9 below). The caller's
`Map.fix_843419` statement is a reference map's only (the table).

**Reading the words** is `spec/cli.md` §2's grammar, not restated: one
or two dashes by §2.2's forms (a long name starting with `l`, `m` or `o`
with two only, so `-odr-table=full` is `-o dr-table=full`), §2.2's
lookup order (an exact name, a short option, an abbreviation of a GNU
long name, §2.4, then a short option with its value joined), §2.3's
values (a required value joined by `=` or the next word whatever it is,
an optional one by `=` only), §2.7's `-z keyword` and `-zkeyword`, and
the last of an option and its negation winning (§2.10; image.md I-69 for
`--build-id`). Which spellings exist, which take a value and which are
GNU's (and so abbreviate) are the rows of `docs/status/options.md` that
mortar does not refuse: an accepted line has no other word (a refused
option fails the link, and a failed link writes no map, image.md I-77).
The reader holds a copy of every row, accepted and refused, and a test
holds it equal to the table (`opts::tests::s3_2a_accepted_is_the_options_table`).
One lookup, `opts::Table::word`, reads a word and the value it takes for
every reader of the line (`opts.rs` and stage 3's `line.rs`, WP-131 V1,
V6): whether an option takes the next word is its row's, so a refused
option's value (`--sort-section alignment`) is consumed with it, and
`line.rs` names the rows it interprets by their first spelling
(`line::INTERPRETED`), and `xtask plumb --inputs-from` skips an option's
value by the same lookup. A test over the whole table
(`line::tests::s12_1_every_spelling_of_a_row_reads_alike`) reads every
spelling of every row, abbreviations and `-z` keywords included, in
both value forms: an interpreted row changes the line, every other row
leaves it as it was. An
abbreviation is read as the one accepted GNU spelling it is a prefix
of: mortar accepted the line, so it named exactly one GNU spelling
(§2.4), and the accepted one with that prefix is it.

- **M9 (the line, mortar's map).** A word of the map's `arg` records
  that starts with `-`, is not
  `-` alone, and that no accepted option spells by the rules above (an
  unknown name, a value given to an option that takes none, a required
  value missing at the end of the line, an unknown `-z` keyword, a word
  that is not UTF-8 where an option's name stands) is a finding, one per
  word: no link of this mortar wrote that line. It judges the map's
  records whether or not the caller gives a line. When the records are
  the options (no caller's line), they are read from the other words.

**The rules and where they read their option:**

Below, "the line" for mortar's map is the caller's line, else its `arg`
records (WP-201).

| rule | option | mortar's map | a reference's map |
|---|---|---|---|
| C1, C5, C6 (§5.10; a64.md §11.3) | `--fix-cortex-a53-843419` (any value) given or not | the line | the caller (`Map.fix_843419`: `xtask plumb --fix-cortex-a53-843419` or `--no-fix-cortex-a53-843419`, or the `--inputs-from` response file), else inferred from the fixes found |
| P-G6 (§6.10) | the last of `--gdb-index` and `--no-gdb-index`; `-s`, `--strip-all`, `-S`, `--strip-debug` | the line | not judged: P-G6 to P-G9 are mortar's rules |
| B1 (§6.7) | the last of `--build-id[=STYLE]` and `--no-build-id` (I-69): presence, length and tree | the line; the build-id chunk's style, when it states one, must be the line's | not judged (no tool recomputes their ids) |
| P-M1's kept section, P-M2's presence, P-M3's second half (§13) | `--keep-mason-meta`; the last `--odr-table`; the last of `--odr=M` and `--no-odr-check` | the line | none of them (a reference has none of mortar's options) |
| P-M2's runtime note, P-M3's runtime rules, the runtime input (§13.1, WP-197); P-D3's and P-D6's `now` | the last `--odr-runtime` (default `off`); whether `--odr-table` is spelled; the last of `-z now` and `-z lazy` | the line | none (a reference has none of mortar's options: its `now` is the line's `-z` words alone) |
| P-D4's `PF_X` (dynamic), S10's `PF_X` (static) | the last of `-z execstack` and `-z noexecstack` (L-84) | the line | P-D4: the caller's line; S10: not judged (no line) |
| stage 3 (§12, P-D1 to P-D15) | the whole line: D-1's kind, the files, `-dynamic-linker`, `-z now`, `-z relro`, `-E`, ... (`line.rs`) | the line | the caller's line (`Ctx.line`); without one, P-D2 |
| P-D7, P-D19, M10's export roots, the export set's one derivation (`shared::ExportRule`) | `--exclude-libs` (every kind) and `--dynamic-list`, `--version-script` (their files' contents) | the line's words; each file read from the path the line names, relative to the link's directory (`Ctx.files`): the map's `dynamic-list`/`version-script` records (a `Shared` map only) give the path, never the content, so a file changed after the link is judged as it now reads (WP-161) | the caller's line, the files as above |

**Static outputs, S10's clause** (§9.5 hole 1, §11 item 11): in an
output of mortar's map, `PT_GNU_STACK`'s `PF_X` is P-D4's rule
(L-84): set under the line's last `-z execstack`, clear under `-z
noexecstack`, and with neither set exactly when an input object's
`.note.GNU-stack` is `SHF_EXECINSTR`; a finding is S10.

Still read without the line, and why: P-M3's second half ("every
relocatable file the map names is on the command line") needs the
line's files found as the linker found them (`-l` searched in the `-L`
directories, scripts read), which only stage 3's entry has (`Ctx.files`):
on a static output it is not judged. The runtime's position on the line
(after every input, WP-197) is the words' too, so the input set and
stage 3 read it on a dynamic output only; a static output with the
runtime is a line mortar refuses (P-M2's presence). S7's entry symbol (`-e`) is judged
in stage 3 only (P-D4); a static output's entry is held to "some
symbol's value" (S7, mortar's clause).

Tests: `opts::tests` (the forms, values, last wins, abbreviations, M9's
words, the table), `tests/optline.rs` (WP-201: mortar's map with its
`arg` records dropping, adding or changing an option `opts` reads gives
the caller's line's verdict; M9 on the records with and without a line),
`map::mortar::tests::s3_2_arg_records_in_every_map`
(a static map's words, with a space, a newline, `%`, the empty word and
bytes that are not UTF-8, round-trip and make no dynamic record).

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
     output at the record's place (an allocated section's at its address;
     a non-alloc one's in the file bytes of the output section of its
     name, at the offset §5.8 gives the row: WP-250, before which the
     address was read in the image and GCC's `-g3` `.debug_macro`
     sections of one file fell to M3 and to `section-not-locatable`);
  4. otherwise, of those (of all of step 2's, when the output's bytes are
     not readable), the one not in a COMDAT copy §5.7a's keeper drops,
     in `FileId` order (WP-250: GCC's `-g3` `.debug_macro` groups of one
     file can be equal outside their relocated words, and only one of
     them is kept);
  5. otherwise, the first one, if all the candidates are identical (bytes,
     type and relocation records).

  If none of these settles it, the candidates are *ambiguous*: their
  records are not checked (`section-not-locatable`).

Each input section ends up with one *place*:

| place | from | its relocation records |
|---|---|---|
| whole at `a` | a `place` record | checked, with `P = a + r_offset` |
| pieces | `piece` records (mortar's format), or lld's `.eh_frame+0xOFF` rows | checked where a piece covers `r_offset`. A record outside every piece is in a dropped part and is counted as not in the output |
| merged | an `SHF_MERGE` `SHF_ALLOC` section in a foreign map, placed, absent or ambiguous, except an empty one with a row (WP-143: GNU ld, lld and mold each write one, at the address its labels have; it is whole) | not checked (`in-mergeable`). References *to* it are checked by content (§5.6); to an empty one without a row, as to an empty section below |
| rewritten | `.eh_frame` placed whole with a size other than its input's (GNU ld) | checked by CIE and FDE (§6.4) |
| discarded | a `discard` record, or GNU's "Discarded input sections" and `/DISCARD/`; or an allocated section with no bytes that no record names, in a COMDAT copy not kept or collected by `--gc-sections` (WP-156b, §3.3) | not in the output |
| absent | no record | not in the output. An allocated `.eh_frame` is checked by CIE and FDE whatever its place (§6.4): mold lists none. An allocated section **with no bytes** (`sh_size` 0) and no record is not "not in the output" (WP-143): it has no bytes for a record to place (mortar's map lists none for an empty merged section: it has no piece, `spec/image.md` I-78), and its labels are where its bytes would be, which only the output's symbols say (§4.4 step 1 and 3, Y3); a reference whose type names an address (address-sized `S + A`, 4- or 8-byte `S + A - P`) is judged by the address it names: in or at the end of an allocated output section (`[sh_addr, sh_addr + sh_size]`), else R5; other types are `section-not-locatable` |
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
  `sh_addralign`. Under mortar's map (WP-143), an allocated section
  without bytes placed whole is at an address in or at the end of an
  allocated output section (`[sh_addr, sh_addr + sh_size]`): its labels
  are there, never at 0 or between sections. A foreign map's row of one is
  trusted as its labels' address (GNU ld writes the place its dropped
  empty output section would have had, §10.2 A10).
- **M5.** Two input sections placed whole share bytes. A `.tbss` section
  occupies no memory and is exempt.

- **M10 (WP-143; mortar's map).** Every section of a live object that is
  neither consumed (the list under "By index" above) nor without bytes,
  and has no place (discarded or absent), has a reason the inputs and the
  line give, derived by the verifier, not taken from the map:
  - a row of `spec/resolve.md` §10 that drops or consumes it, whatever the
    record says: `SHF_EXCLUDE`; a `.mason.meta` container; `.note.GNU-stack`;
    `.note.gnu.build-id`; `.note.gnu.property` of type `SHT_NOTE`
    (consumed into the output's note, `spec/synth.md`); `.gnu.lto_*`;
    `.gnu.warning`/`.gnu.warning.*`; `SHT_LLVM_ADDRSIG`,
    `SHT_LLVM_CALL_GRAPH_PROFILE`, `SHT_LLVM_DEPENDENT_LIBRARIES`;
    `SHT_AARCH64_ATTRIBUTES` on AArch64 (`spec/layout.md` L-35); a
    non-alloc debug section (GNU ld's debug names, `spec/resolve.md` §6.3)
    under `-S`/`-s`; `.debug_gnu_pubnames`/`.debug_gnu_pubtypes` under
    `--gdb-index` without `-S`/`-s` (`spec/synth.md` S-G6);
  - **the GNU name index in incremental mode (WP-251; `spec/incr.md`
    IN-55 option (A), built by WP-229).** In an *incremental-mode output*
    (below), a non-allocated input section named `.debug_gnu_pubnames` or
    `.debug_gnu_pubtypes`, not a member of a COMDAT copy that is not
    kept, has a `discard ... unspecified` record (I-78's line, as under
    `-S`) and is not in the output: that is its reason. Without the
    record (no record at all) it is M10 as any section. Both directions
    are judged: in an incremental-mode output, such a set *placed*
    (`nplace`, `merged` or `place`) is M10 at the record, unless `-S`/`-s`
    is on the line (the linker forgot the rule: an update would find a
    dead unit by name, IN-55); and a non-allocated output section of one
    of the two names with bytes, when no record places a set, is M10 once
    per output section (a `discard` record beside the bytes). In a
    plain output (no `--incremental`, or one IN-12 makes plain) a set
    left out has no row but `-S`/`-s` and `--gdb-index`'s, as before. An
    allocated section of either name is not debug information
    (`spec/resolve.md` §6.3): placed is right, left out is M10. Any other
    name (`.debug_pubnames`, `.debug_pubtypes`, `.debug_names`, which
    IN-55 keeps) left out is M10.
    *Incremental mode* is judged from the map AND the line, both: the map
    says `incremental` (§3.2), and the line (§3.2a: the caller's, else the
    `arg` records) asks for it: the last of `--incremental[=MODE]` and
    `--no-incremental` is `--incremental` (IN-10, IN-11), and none of the
    options of IN-12 (and IN-12a) that make an `--incremental` link plain
    and that its words alone show is on it: `-r`, `--emit-relocs`, a
    `--compress-debug-sections` other than `none`, `--gdb-index`,
    `--debug-names`, an `--icf` other than `none`, `--sort-common`,
    `--sort-section`, `--start-lib`, `-shared` (each the last of it and
    its negation). The output itself cannot show the mode: IN-17 keeps the
    state in a file beside the output precisely so that an
    incremental-mode output has the sections a plain one has (measured on
    WP-229's `inc5`, AArch64, against mortar's plain link of the same
    inputs: the same section names, types and segments but for the two
    sets; only sizes and addresses differ, by IN-72's slack, which no rule
    can tell from a plain layout). The map's word is therefore not taken
    alone: when the map and the line disagree, either way, the output is
    judged as a plain one, where a left-out set is M10. The disagreement
    is not itself a finding: mortar's own replay tests
    (`crates/mortar/tests/incremental.rs`) write maps that say
    `incremental` from a line without `--incremental` (WP-251 §0), and a
    line that asks for the mode beside a map that does not say it is
    right when a `SECTIONS` script, the output kind (IN-12, IN-12a) or a
    full link that writes no state (IN-77b) made the link plain, none of
    which the words show.
    Tests: `tests/gnuindex.rs` (mortar's `--incremental=full`, plain,
    `--incremental --sort-common`, COMDAT-group and allocated links, AArch64
    and x86-64: clean; mutations: a set placed in an incremental-mode
    output, the sets' bytes beside `discard` records, the sets left out of
    a plain output, the map's word against the line (no `--incremental`,
    `--no-incremental`, `--sort-common`, `--gdb-index`,
    `--compress-debug-sections=zlib`, `-r`: judged plain)
    and the negations that keep the mode, `.debug_pubnames` left out, the allocated section left out, a
    set with no record, the group's set given `comdat`) and the link test
    `split/incr-gnu-index.test`;
  - a member of a COMDAT group that is not its signature's keeper
    (`spec/resolve.md` §6.1: the group of the least `FileId` among the
    live files, the first by section index there; the map lists the files
    in `FileId` order, I-78);
  - a `discard ... gc` record, when the line has `--gc-sections` (the last
    of it and `--no-gc-sections`) and the section is not one of
    `spec/image.md` I-6's roots the inputs show: `SHF_GNU_RETAIN`; a
    start-up table (type `SHT_INIT_ARRAY`/`SHT_FINI_ARRAY`/
    `SHT_PREINIT_ARRAY`, or a name `.init_array`, `.fini_array`,
    `.preinit_array`, `.init`, `.fini`, `.ctors`, `.dtors`, alone or
    followed by `.`); a note or non-alloc section not in a group with an
    allocated member; `.eh_frame`; a C-identifier name whose `__start_` or
    `__stop_` some input references. A `SHF_LINK_ORDER` section is never a
    root; it is a finding when its link target is placed (I-109). A
    reference from a placed allocated section to a symbol of a collected
    one is R5 (§5.7), which is I-7's edge seen from the other side;
  - a `discard ... script` record, when the line has `-T`;
  - for an `SHT_NOBITS` section with no record (I-78 lists `discard` only
    for a section with file bytes): `--gc-sections` on the line and not a
    root, as for `discard ... gc`.
  - **An empty section not in the output (WP-156b).** An allocated
    section with no bytes (`sh_size` 0) is listed when it is live by
    every map: mortar writes its `place` (I-78; layout L-159 places it),
    GNU ld, lld and mold an entry of size 0 in the memory map (measured,
    the three targets). I-78's `discard` names only a section with bytes,
    and lld and mold list no section that is not in the output, so such
    an empty section has no record. One that no record names is
    *discarded* when it is a member of a COMDAT copy that is not kept
    (`spec/resolve.md` §6.1; the inputs alone say it), or, under
    `--gc-sections` (mortar's `arg` records; a reference's line from the
    caller), when it is not one of I-6's roots above and holds no
    definition of a key the line roots. A reference to it from a
    non-allocated section is then judged as one to any section not in the
    output (§5.8: I-47's tombstone of the referring section; GNU ld also
    the kept copy's address when the sizes are equal; another value is
    R1), and its labels are not in the output. Any other empty section
    with no record stays *absent* (an address is required, the row
    above). In a dynamic output that exports (`-shared`, `-E`), every
    global of default or protected visibility counts as an export root
    for the `--gc-sections` test here (the export set is derived later in
    stage 3), so such a section is left absent.
    Tests: `tests/link/resolve/gc-empty-debug.test` (the three targets,
    the four linkers; `.debug_addr`, `.debug_rnglists`, `.debug_ranges`,
    `.debug_loc`, `.debug_info`; a collected and a live empty section,
    and an empty COMDAT copy not kept) and `tests/gc_empty.rs` (mortar's,
    GNU ld's and lld's links of both shapes, AArch64 and x86-64, clean;
    the mutations: a collected or dropped empty section's word given an
    address or the wrong tombstone, a live one's given the tombstone,
    each R1).
  A `discard ... comdat` record must be the COMDAT row, `... unspecified`
  one of §10's rows, `... gc` and `... script` as above; anything else, and
  a section with bytes no record names and no row explains, is M10.

  **The roots the line names** (WP-147; `spec/resolve.md` §4.2, §4.7). A
  `discard ... gc` record, or a collected `SHT_NOBITS` section, is also
  M10 when the section holds the definition (the first strong one in
  object order, else the first) of a key the line roots: the entry (`-e`,
  else `_start` outside `-shared`); a name of `-init`, `-fini` or
  `--export-dynamic-symbol`; and, when the output exports (the map's
  `output shared`, or `output exec`/`output pie` with `-E`), every global
  or weak of default or protected visibility (merged by base name) that
  no live object makes hidden or internal and that the export set's one
  derivation exports (WP-149: `shared::ExportRule`, which P-D19's set and
  P-1's placeless keys ask too): a name a version script's `local:` (or
  `local: *`) takes out of the set is not a root, so mortar may rightly
  collect its section; a `.symver` key (`foo@V1`, `foo@@V2`) is a root
  when its node exports it (resolve.md §9.7, D-114). Stage 3 asks the
  derivation with the line's script (read from the files the map's
  `version-script` records name). Where stage 3 does not judge the output
  (no `PT_DYNAMIC`: a map that only says `output shared`), the line's
  files are not read: without a version script every such global is a
  root, as above; with one, no export is taken as a root there. The
  derivation also reads the line's `--exclude-libs` (a key whose every
  relocatable definition is in a member of an archive it names is not
  exported, D-110 item 1) and, for an executable, its `--dynamic-list`
  files (without `-E`, only a listed definition is exported, resolve.md
  §9.6; WP-161): an excluded key is no root, a listed one is.
  mortar refuses `-u` and `--require-defined`
  (docs/status/options.md), so its line has none. Not re-derived: a glob
  of `--export-dynamic-symbol`, and a script's `KEEP`. Tests:
  `tests/placeless.rs wp147_gc_of_a_line_root_is_m10` (the entry, `-init`,
  an export of a library: each M10; `-E` in a static link and no root:
  accepted); `tests/shared.rs m10_export_roots_follow_the_version_script`
  (WP-149, `tests/shared/gcver`: mortar's `-shared --gc-sections`
  libraries whose script makes `gone` and `data_gone` local, their
  sections collected: clean; the same records under a script that exports
  them, or with no script: M10 for each) and the link test
  `dyn/version/gc-local-unreferenced` (measured: GNU ld, lld and mold
  collect the local names' sections too, and keep them when the script
  exports them).

Each rule has a test: `tests/mutation.rs s9_2e_each_map_rule_is_enforced`;
the WP-143 rows (M4 for an empty section, M10): `tests/placeless.rs`.

The allocation records (§3.2; WP-096) are mortar's: these rules judge a
map of mortar's format only, every such map, and each is a finding at
the record's address naming its map line. What the output must hold for
each record:

- **M6 (a `NOBITS` chunk).** A `synthetic` chunk whose `<addr>` lies in
  an allocated `SHT_NOBITS` output section that is not thread-local
  (`[sh_addr, sh_addr + sh_size)`, size above 0) is that section:
  `<name>` its name, `<addr>` its `sh_addr`, `<size>` its `sh_size`.
- **M7 (a common).** `<name>` is a name the inputs give only common
  definitions (a global or weak symbol of `SHN_COMMON`, or x86-64
  `SHN_X86_64_LCOMMON`, and no definition in a section the map places),
  named by one record only. The output's `.symtab` defines it (a global or
  weak symbol in a section; or, exactly when the line makes the key
  local, a `STB_LOCAL` one: stage 3's local keys, §6's "A key the line
  makes local, in `.symtab`" (a version script's `local:`,
  `--exclude-libs`), or an input's `HIDDEN`/`INTERNAL` global or weak
  entry of the name; of two such locals the one at `<addr>`; synth S-77,
  WP-169) with `st_value` = `<addr>` and `st_size` = `<size>`, in an
  allocated `SHT_NOBITS` output section that holds
  `[<addr>, <addr> + <size>)`. `<align>` is the largest alignment the
  inputs' common definitions of the name ask (their `st_value`, gABI
  "Symbol Values"), and `<addr>` is a multiple of it. An output without
  `.symtab` (stripped) has its records' names and alignments judged and
  their addresses and sizes not.
- **M8 (a copy).** `<addr>` is the `r_offset` of one `COPY` record of
  the output's dynamic relocations, named by one record only; that
  record's symbol is `<name>`, its `.dynsym` `st_size` is `<size>`, and
  `<align>` is P-D11's alignment of the copied definition, `min(sh_addralign`
  of its defining section`, 2^ctz(st_value))` (`spec/dyn.md` D-42), with
  `<addr>` a multiple of it. A `copy` record in an output that has no
  dynamic section is M8. A dynamic output stage 3 does not model (P-D1)
  has its `copy` records not judged.

Tests: `tests/maprec.rs` (§9.2 (e)), `tests/dyn.rs
s3_3_m8_copy_records_are_the_copies`.

### 3.4 GNU ld (`-Map`)

GNU ld's map is a sequence of blocks, each opened by a line at column 0:

1. **Preamble.** The archive members included, then "Allocating common
   symbols". The member list is read for the input set (§3.8, WP-156):
   a member padded to a 30-column field and its reason after it, or a
   longer member alone on its line and the reason on the next, indented
   30 columns. A member is `archive(member)`, or a thin archive's
   member by its own path alone (`lib/one.o`). A reason is
   `(--whole-archive)`, `(sym)` (a root of the line) or `file (sym)`,
   split at the last ` (`. A long member followed by no reason, or a
   reason with no member, is an error. The list ends at a line that is no member and reason (GNU ld's notes
   follow it without a blank line on i386: ``discarding zero address
   range FDE in libc.a(clone3.o)(.eh_frame).``, ``Local IFUNC function
   `strcpy' in libc.a(strcpy.o)``). GNU ld prints C++ names demangled
   there. Nothing else in the preamble is read.
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

Tests: `map::gnu::tests::s3_4_gnu_map_excerpt`,
`s3_4_the_preamble_s_members_and_reasons` and
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
- **Rows without text** (WP-170 part 2). lld 19 writes each `--defsym`
  as a row of four numbers, size 0 and no text (the assignment, whose
  text it leaves empty). It places nothing and is skipped; a row without
  text whose size is not 0 is still an error ("row has no name"). Test:
  `map::lld::tests::s4_4b_lld_defsym_row_is_an_assignment`.

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

Each reader returns `Ok(Map)` or `Err(MapError)`. It never panics.
Mortar's reader also refuses a map with no `arg` record (§3.2a: every
mortar map states its line; one written before WP-102 by a static link
has none). No
input, however truncated or corrupted, leads to any other outcome (tests:
`s3_7_truncations_are_answers`, `tests/hostile.rs s9_4_mutated_maps`).

### 3.8 The input set (WP-156)

Which files are in the link is judged against the line, the input files
and the archives' members, not taken from the map: a member the map
leaves out has no section for any other rule to judge. The rule runs
after the rules of the output's kind, for every kind (static, dynamic,
static-pie), once per map (`--equiv`: per side). Two ids:

- **M11 (missing)**: a file the link must contain is not in it.
- **M12 (extra)**: a file is in the link that no root or live reference
  reaches; for GNU ld's map also a reason its preamble prints (§3.4) that
  the inputs do not bear out.

**The line.** mortar's map: its `arg` records (§3.2a). A reference's
map: the caller's line (`--inputs-from`, the link-test lane's ld words);
with none, the coverage says `input set: not judged (no command line …)`.
The line is read by stage 3's reader (§12, `line.rs`): the items in
order with their `--whole-archive`, `--start-lib`, `--as-needed` and
group state, implicit scripts expanded (their `EXTERN` and `ENTRY` are
read in place), `-u`, `--require-defined`, `--defsym`, `-e`, `--wrap`,
`-shared`, `-r`, `-T`. A line it cannot read (a file not found, a script
word it does not model), `--undefined-glob` and `-c` leave the link not
judged, by name. A `-T` script with `INPUT`, `GROUP`, `STARTUP` or
`INCLUDE` is not judged; its `ENTRY` and `EXTERN` are roots and every
name it assigns (`PROVIDE` too) is an *open* key, below.

**The files** are numbered as resolve §4.1 numbers them: file 0 (the
line's own definitions and roots), then each item in order, an archive
as its members in archive order. Each file's mentions are read from its
own symbol table (never an archive index; thin members at their stored
path, a member of a nested thin archive not judged) and kept as `(key,
kind)`: a definition (weak or not, function or data, resolve §4.5's
classes), a common (`SHN_COMMON`, `SHN_X86_64_LCOMMON`), a strong or
weak undefined mention after `--wrap` redirection (resolve §7.1). Each
object's `GRP_COMDAT` groups are read too, in section-index order, with
their signatures as resolve §6.1 computes them (the signature symbol's
name; for an `STT_SECTION` symbol the name of its section), and each
definition in a section of one of them is marked with its group (the
first group that lists the section); this is what the iterations below
read (WP-231). Keys
follow resolve §9.4: `foo@@V` is `foo` and also binds `foo@V`; `foo@V`
is its own key. A shared object's visible dynamic definitions are its
definitions (a default `foo@@V` is `foo` and `foo@V`, a hidden `foo@V`
only `foo@V`), and its strong references are edges (an object `--as-needed` at every
mention's only as D-12's stages below say; resolve §4.4; dyn.md
"Archive extraction by the references of shared objects"). Only the
command line's shared objects (and those of its scripts) are read: a
`DT_NEEDED` closure file has no edges.

**D-12's stages (WP-171).** An `--as-needed` command-line shared
object's strong undefined references are edges exactly when it is a
*source* of the last stage: a member of N, as the verifier replays D-11
and D-12 from the line's inputs (below), and named by the output's
`DT_NEEDED`. Both halves are needed: the output's entry alone is no
reason (an entry nothing justifies is P-D6 and licenses no member,
`d12_an_unjustified_needed_entry_licenses_nothing`), and the replay
alone does not judge the output (a needed library the output lacks is
P-D6, and its references are then no edges of that output's closure).
The replay: stage 1 is resolve §4.1's closure with no as-needed object's
reference as an edge; D-11 (§12, `dynmodel.rs`) runs over the output's
objects that the stage's closure reaches, with D-12's item-3 rule (a
shared object's reference whose `first(k)` is a lazy file the stage has
not made live gives no reason, cell 7a2), and N is the union of every
stage's (N only grows: row 2a); the next stage's sources are the
as-needed members of N the output's `DT_NEEDED` names, and stages repeat
until the sources do not change (at most |C| + 1). One closure over the
union of the edges is the least fixed point. An output with no
`DT_NEEDED` at all (no `PT_DYNAMIC`) has no source. Where the replay
does not run (stage 3 outside its model, §12.1) an as-needed object's
references are followed by the upper bound and not by the lower (open),
and the coverage says `D-12's stages not replayed`. An M12 that only a
non-source's references explain says so ("... only the references of an
--as-needed shared object the output does not need reach it (dyn.md
D-12: not an edge)"): lld extracts for an as-needed object not in N
(1c, 3c; a used one, D-13's cell), A70 on lld's output only; and the
P-D6 or P-D7 of what such a member needs ends "(only an object no stage
of D-12 extracts makes it needed)", A70's too. Rule 2's M11 demands
include the sources' references. Tests: `tests/inset_stages.rs`, the
lane's `dyn/shlib/as-needed-extract*.test`.

A reference's key is resolve §9.2's: `name@V` when its
`.gnu.version` entry names a version need `V`, so only a `name@@V` or
`name@V` definition is its target, never an object's plain `name`
(libstdc++.so's `__udivti3@GCC_3.0` does not reach
`libgcc.a(_udivdi3.o)`); `name` when it is unversioned (`VER_NDX_LOCAL`,
`VER_NDX_GLOBAL`, no `.gnu.version`). The rule reads the key the same
way for both bounds (rules 1 and 2). mold alone extracts by the plain
name: an M12 that only such a reference explains says so, and is A49 on
mold's output only (§10.2). Tests: `tests/inset_ver.rs`.

**The roots** (resolve §4.2): the files live from the start (the named
objects outside `--start-lib`, the `--whole-archive` members, the shared
objects, file 0) and the keys of `-e`/`ENTRY` (else `_start`, except
under `-shared`; under `-r` none of the three is a root, resolve.md
RV-R2, §14.1), `-u`, `--require-defined`, `EXTERN` and the
names a `--defsym` expression refers to. A `--defsym` whose expression is
not only names, numbers, `+ - * / ( )` (a builtin like `ADDR(…)`, a
section name, `.`) makes its target an open key and its names open roots.
`-init`/`-fini` are not roots (measured: none of the four linkers
extracts for them). `--export-dynamic-symbol` is a root in resolve §4.2
but no linker extracts for it (measured, WP-156: GNU ld, lld, mold and
mortar, static, PIE and shared, a name and a glob); it is not expanded,
and with it on the line M12 is not judged (the coverage says so).

**mortar's map (rule 1, exact).** The closure of resolve §4.1 is
computed twice: the lower bound with the open keys defined by file 0 and
the open roots not followed, the upper bound the other way.

**§4.1's iterations (WP-200, WP-231).** Each closure (each bound, each
of D-12's stages, and the comparison of rule 2) is resolve §4.1's
procedure "A dropped copy's definitions stop counting when it is
dropped", computed from the inputs as the text states it and from
nothing the output or the map shows:
1. `T₁` is `first` and `cfirst` over every file's mentions (a
   `--defsym` key's target is file 0, which is live: no edge, §4.1's
   "with §7.2's `--defsym` keys removed"); `L₁` the closure from the
   roots; `C₁` the keepers of resolve §6.1 over the relocatable objects
   (named objects, members, `--start-lib` objects) of `L₁`: per
   signature the live file of least `FileId` with a group of it, its
   first group by section index; every other copy dropped.
2. The trigger ("a mention feeds back when it is a defined (strong,
   weak or common) non-local mention of a relocatable object live in
   `L_i`, in a section `C_i` drops, and that object is `first(K)` or
   `cfirst(K)` in `T_i` for its key `K` or, for a versioned `foo@V`
   mention, for the unversioned `foo` too"): a definition marked with a
   dropped group, of a live object that is the key's `first` or `cfirst`
   in `T_i` (for a key with `@`, also the key before the `@`). A common
   is in no section and never feeds back. None: the closure is `L_i`.
3. "`T_{i+1}`: `first` and `cfirst` computed over all files as above,
   except that a mention of a file live in `L_i` defined in a section
   `C_i` drops is not counted; then, per key and per column, the later
   of that and `T_i`" (no target is later than every file); "`L_{i+1}`:
   the closure under `T_{i+1}` from the roots of §4.2 and every file
   live in `L_i`" (a pulling reference already recorded stays: only the
   M11 message reads it); "`C_{i+1}`: the keepers over the relocatable
   objects of `L_{i+1}`". The edges are the ones above: a demoted mention
   is a defined mention and none.
4. The end, as written: after computing `T_{i+1}`, stop when `T_{i+1} =
   T_i`, `L_i = L_{i−1}` and `C_i = C_{i−1}`, with `L_i` and `C_i`;
   else go to 2 with `i+1`. (The text's indices mix `i+1` and `i−1`;
   the reading "stop when `(T, L, C)` repeats" gives the same set:
   `T_{i+1} = T_i` makes `L_{i+1} = L_i` and `C_{i+1} = C_i`, so the
   literal condition holds one iteration later with the same `L`.)

The bounds each run their own iterations. D-12's stages take the last
iteration's `first(k)` for item 3's rule (a reference whose `first(k)`
is a lazy file the stage has not made live). A link with no mismatched
copy runs one iteration; one where a dropped copy's definition is a
target but nothing refers to it strongly runs two and extracts nothing
(the trigger does not ask whether anything refers to the key).
The coverage line ends "; resolve §4.1's closure iterated n times" when
the lower bound's ran more than once. A file the
lower bound reaches and the map does not name is M11, with the
reference that pulls it (the least edge found, by name); a file the map
names that the upper bound does not reach, or that is not an input of
the line, is M12. An M12 member that defines the key of a demoted mention
of a file the upper bound reaches (a definition in a copy its last
iteration drops) says so, naming the mention, its file and the group:
"... and no root or live reference reaches it, only the demoted mention
`hx` of wb.o (its dropped copy of COMDAT group `g`) (resolve §6.2: a
demoted mention pulls nothing)" (resolve §6.2: "a demoted mention, weak,
`STB_GLOBAL` or `STB_GNU_UNIQUE`, extracts no archive member, no lazy
object (`--start-lib`) and no `--start-group` member"; WP-200's ruling). A file between the bounds is not judged. Files are
compared by identity (resolve §4.6; `spec/model.md` §7.1): the path
normalized lexically (`.`, `..` and repeated `/` removed: GNU ld and
lld print the path as found, mold normalizes it, §3.6), the member name,
and the header offset when the map gives it. An archive named twice is
one file: a member is required when either mention's is. A thin
archive's member is also known by its own path (GNU ld's map spells it
so, `lib/one.o`, not `libthin.a(lib/one.o)`).

**A reference's map (rule 2).** Each reference follows its own
extraction rule (resolve §16); the rule holds it to what all of them
share. Rule 2 computes no keepers: a definition in a COMDAT copy defines
its key whichever copy the reference kept, as GNU ld, lld and mold read
it when they scan an archive (WP-200's cells: GNU ld and mold refuse a
strong reference whose only earlier definition is in a dropped copy, lld
writes it as an undefined weak value; none of them extracts for it).
So rule 2 has no M11 for a member only §4.1's iterations reach; a
reference that leaves such a reference unsatisfied is judged by R5 at
the reference (§5.7a; lld's outputs, an `xfail(lld)` of the lane's
`resolve/comdat-mismatch-weak-archive-extract.test`,
`-extract-orders.test`, `-member-copy.test`). Only the closure compared
as coverage (below) is iterated. The files in the link are the members the map names (GNU ld's:
with the preamble's), the files live from the start, and the shared
objects. lld's and mold's maps name only files with a section they
list: under `--gc-sections` neither names a file all of whose sections
were removed, and mold never lists a file whose allocated sections are
all mergeable or `.eh_frame` (§3.5, §3.6). Such a file's absence from the
map says nothing: under `--gc-sections` (lld, mold), or for such a file
(mold), a file live from the start that the map does not name, and a
demand whose candidate member is such a file, are **not judged** and
counted (`not judged n (files the map may leave out: …)`).

- **M12**: a member in the link that no root and no reference of a file
  in the link reaches, where a strong or weak undefined mention or a
  common reaches every member in the link that defines or holds the key
  in common (the union of every reference's edges: lld's weak and
  common rules, mold's R1/R3/R5, GNU ld's walk all extract along a
  subset of them).
- **M11**: a file live from the start that the map does not name; and
  a key with a strong demand (a strong undefined mention of a file in
  the link, a root) or a common demand (GNU ld and mold; lld only with
  `--fortran-common`) that no file in the link, no shared object and
  no file 0 definition satisfies, while a member not in the link defines
  it (a common: with a non-weak data definition). Under GNU ld the
  demand is judged only when such a member is one GNU ld's walk reaches
  after the demand: a later item, the demanding member's own archive, or
  the same group; a root is read before every item, except `EXTERN` and
  `--defsym`, read where the line names them (resolve §4.2, finding 17).
  A back-reference is **not judged** and is counted (`demands not judged
  n (GNU ld's back-references)`). A `-T` script's roots have no position
  for GNU ld and are not judged there.
- **GNU ld's preamble**: `(--whole-archive)` is true when the member's
  archive is under `--whole-archive` on the line; `(sym)` when `sym` is
  a root and the member defines it; `file (sym)` when `file` is in the
  link with an undefined (either binding) or common `sym` and the member
  defines it; a reason naming a `--wrap`ped symbol, a demangled name
  (one with a space, `(`, `:`, `<`, `~` or `,`: the map demangles and the
  verifier has no demangler), or a file not on the line, is not judged
  and counted (`preamble reasons not judged n`). A member the map places and
  the preamble does not list is M12.
- The closure of rule 1 is computed for the reference's line too (lld:
  without common edges unless `--fortran-common`) and compared, as
  coverage, not a finding: resolve §16 records where each differs.

**Coverage**: one line, `input set: files judged n of m (in the link k,
required r)` for mortar's map (with "; resolve §4.1's closure iterated
n times" when it ran more than once), `input set: files judged n of m (in the
link k); mortar's closure: the same` (or `differs (+a -b, c open)`) for a
reference's, followed by what was not judged; or `input set: not judged
(why)`. `m` counts the relocatable files of the line, members one by
one.

**Cost** (§8.1): each object's symbol table is read once (an archive's
members by their headers; nothing else of a member); a file of at most
64 MiB is read whole once and dropped when its item is done, a larger one
by position. The mentions are kept as 5 bytes per symbol with the names
interned once; the closure is linear in the mentions. A COMDAT group
costs its signature (interned) and 8 bytes per definition in it; each
iteration of §4.1 past the first recomputes the targets over every
mention and the closure from every live file (only for a dropped copy's
definition that is a target). Measured on
`cxx-large-g` (7,603 files on the line, 2,845 in the link): the rule
adds 2.9-3.2 s to a 13-14 s pass (5.9 s at a load of 9) and nothing to
the peak (1,709 MiB by GNU time, the linker's 1,977 MiB); the landed
record `spec/landed/plumb/WP-156.md` has the runs.

Tests: `inset::tests` (the closure on synthetic archives: a cycle, one
symbol in two archives, an archive named twice, weak references and
commons, the roots, GNU ld's position, a thin member by its path, maps
that leave files out), `tests/inset.rs` (the EXTERN
defect of 2026-11-17 by name, GNU ld's link of the same line clean, the
mutations of §9.2 (s)), `map::gnu::tests::s3_4_the_preamble_s_members_and_reasons`;
§4.1's iterations: `inset::tests::s3_8_a_dropped_copy_s_definition_stops_counting`,
`s3_8_a_demoted_mention_pulls_nothing`, `s3_8_no_mismatched_copy_runs_once`
(synthetic objects, every order and kind of lazy file of WP-200's
table), `tests/comdat_iter.rs` (mortar's links of WP-200's cells on
AArch64 and x86-64, `tests/comdat_iter/gen.sh`; §9.2 (s) (6) to (8)),
the lane's `resolve/comdat-mismatch-weak-archive-extract.test`,
`-extract-orders.test`, `-member-copy.test`, `-keeper-later.test`.

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
  `.got`; in a stripped output (no `.symtab`, so no symbol of that name
  to read; WP-164) on x86-64 the start of `.got.plt` when the output has
  one (where all four linkers put the symbol, the table of linker-defined
  names below), else `.got` (measured: mold's stripped large-model
  `GOTOFF64` was R1 against `.got` before).
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
  - **offset 0 of an input `.eh_frame`, under mortar's map** (WP-088;
    `synth.md` S-50 rule 1): a section symbol with `st_value + A = 0`, or
    any other symbol except a mapping symbol with `st_value = 0` (A is
    then added after), in an input `.eh_frame` the map does not discard
    is the start of the output's allocated `.eh_frame`, whatever the
    input section holds: nothing (x86's crtbeginT.o), a kept record that
    is not first in the output (crtbeginT.o's own CIE under IN-34a's
    per-file CIEs, or any CIE no other equals), or a removed one. A
    non-zero offset follows its record by the rows above (rule 2). This
    holds for relocations, `.symtab` labels (Y4: `__EH_FRAME_BEGIN__` is
    the section's start) and every other consumer of a resolved target.
    It applies to `Format::Mortar` only, and only when the output has an
    allocated `.eh_frame` (otherwise the row below). Measured on the
    shape (an object whose own CIE is not first, with words referring to
    offset 0 and to its FDE; an object with an empty `.eh_frame`;
    AArch64, x86-64 and i386 alike): offset 0 is the record's place for
    GNU ld and lld 19/22 and the empty section's place after the
    previous file's records for GNU ld; lld gives the empty one the
    section's start; mold gives the section's start for both, and for
    the FDE's offset 0x18 the section's start plus 0x18, not the record
    (§10.4). Under their own maps the references are judged by these
    places (pieces or whole, the rows above) or not at all (mold's
    absent `.eh_frame`). Test: `tests/eh.rs
    s4_4_eh_frame_offset_zero_is_the_output_section_start` (GNU ld's
    bytes under mortar's map: two R1 and two Y4; the section's start:
    accepted; a word or label 4 bytes past the start, or the FDE's word
    at the start: one finding each);
  - **in `.eh_frame`** (a label such as crtbeginT.o's
    `__EH_FRAME_BEGIN__`) when the section is rewritten, absent, or the
    label is in a dropped part, and the row above does not apply:
    `eh-frame`; except, under mortar's map, when the output has no
    allocated `.eh_frame` (`synth.md` S-48a: no live record, no
    terminator, no empty input): the absent section is then not in the
    output, and a reference to it is R5 (WP-143b);
  - **ambiguous:** `section-not-locatable`;
  - **empty without a record** (the "absent" row of §3.3 for a section
    without bytes, or a foreign map's empty merged section without a
    row): the address the field names, §3.3;
  - **discarded or absent:** R5 (§5.7).

  An `SHN_ABS` symbol has S = `st_value`. Anything else is
  `symbol-unresolved`.
- **A global, weak or `STB_GNU_UNIQUE` symbol**, by name, takes the first
  of these that applies:
  0. **No definition with a place** (WP-143). When the name has input
     definitions, none `SHN_ABS` or common, and every one is in a section
     that is discarded or absent (not one without bytes, above; nor an
     input `.eh_frame` while the output has an allocated `.eh_frame`,
     whose labels have `synth.md` S-50's values, WP-143b), and the
     loader does not bind the name (§12.5; in an executable, WP-178: the
     output imports the name and the loader model binds it to a
     definition, a needed library's: `spec/resolve.md` §6.2, a shared
     object's definition binds a demoted mention's key). This predicate, "the section
     has no place" (`spec/resolve.md` §10.1 P-1 as the map shows it), is
     the one derivation every rule that lists the expected symbols asks:
     this step, Y3, Y4 and P-D19's export set (WP-147; P-D19 asks it of
     every definition of the key, WP-147b): the definition is the first
     strong one in object order, else the first, and S follows from its
     section as for a local symbol: R5 (§5.7), or §5.8's tombstone from a
     non-alloc place, whatever the output's `.symtab` says. Before WP-143
     the steps below looked at placed definitions only and such a name
     ended in `symbol-unresolved`, a count: mortar's word 0 against a
     global of a consumed `.note.gnu.property` passed (WP-136).
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

  If none applies and every definition of the name is in a section
  without bytes and without a record, the field names the address (§3.3,
  "absent"); otherwise the record is `symbol-unresolved`.

**An undefined weak** has S = 0, except for a PC-relative type that is
not a branch (`PREL64/32/16`, `LD_PREL_LO19`, `ADR_PREL_LO21`, the
`MOVW_PREL` groups), for which S = P, so X = A (AAELF64: an unresolved
weak reference is the place for a PC-relative relocation). GNU ld and lld
write X = A; mold refuses such a record as out of range (review finding
10; test: `tests/tls.rs s4_4_undefined_weak_pc_relative_is_the_place`).
Branches keep S = 0 and have W4 and W5.

Lookups by name go through hash maps. Nothing depends on their iteration
order (§7.1).

### 4.4a `--wrap=SYM` (WP-170)

`spec/resolve.md` §7.1, derived once (`wrap.rs`) from the line's
`--wrap` words (mortar's `arg` records, a reference's line): an
**undefined**, unversioned mention (raw `st_shndx` `SHN_UNDEF`) of `SYM`
has the key `__wrap_SYM`, one of `__real_SYM` the key `SYM`; one step per
mention (`--wrap=f --wrap=__wrap_f` maps `f` to `__wrap_f` and
`__wrap_f` to `__wrap___wrap_f`, never further); a defined mention keeps
its name (E13), and `f@V` is no mention of `f`. A line without `--wrap`
has the identity, and no rule pays for it. Every rule that asks which
definition a reference binds to reads it:

- §4.4's lookup by name of a global (R1, R2, R4, R5, the slot and PLT
  checks, stage 3's views and S12 at the redirected sites);
- the input set (§3.8): an undefined `__real_f` is an edge to `f`'s
  member, an undefined `f` an edge to `__wrap_f`'s and not to `f`'s (M11,
  M12);
- stage 3's model (§12): the program's keys (P-D6's `DT_NEEDED` under
  `--as-needed`: a library only the wrapped mentions would need is not
  needed; P-D7's imports) and a shared object's export and import sets;
- `.symtab` (Y3): an entry `__real_SYM` (any binding but an input's own
  local) is a finding unless an input defines `__real_SYM`.

Roots are not redirected: `-e`, `-u`, `--require-defined`,
`--export-dynamic-symbol` and §3.3's gc roots name `SYM` itself. An entry
point at `__wrap_ENTRY` is P-D4 (lld's form, A82). `--gc-sections`'
edges are not re-derived: a section `SYM`'s references alone kept is not
judged (§3.3 judges why a section is dropped, never that a kept one is
live); a dropped one a redirected reference reaches is R5.

The shapes resolve.md §7.1 refuses are not judged, by name, on a
reference's output (mortar refuses the line, so its output never has
them, and on mortar's map every record is judged): `--wrap=f@V1`
("the redirection of a versioned symbol is not derived"),
`--wrap=__real_X` beside `--wrap=X` ("the redirection of this shape is
not derived"), and a word with two or more input definitions one of which
is in a group member (`SHF_GROUP`) the output does not place (a dropped
COMDAT copy): every
record against a global of that name is `wrap-shape`, and the coverage
says `--wrap, not judged: --wrap=SYM: a definition in a dropped COMDAT
copy (file); ...`.

lld's forms are named (§10.2): A80 (a reference in `SYM`'s defining
object bound to `__wrap_SYM`, directly or through a PLT entry whose word
holds it: the finding's text ends "(a reference in SYM's defining object
bound to __wrap_SYM; resolve.md §7.1 E13: not wrapped)"), A81, A82, A83,
A84. mold's `.symtab` (`__wrap_SYM` and `SYM` `STB_LOCAL`) is A9. The
same findings on mortar's output are never named. Tests:
`tests/wrap.rs` (`tests/wrap/gen.sh`), `tests/link/resolve/wrap*.test`,
`tests/link/dyn/shared/wrap-shared.test`.

### 4.4b `--defsym=SYM=EXPR` (WP-170 part 2)

`spec/resolve.md` §7.2, derived once (`defsym.rs`) from the line's
`--defsym` words (mortar's `arg` records, a reference's line). Of
several words of one `SYM` the last is the one. The forms derived:

| `EXPR` | the value | section | type | size |
|---|---|---|---|---|
| a number (`0x...`, or decimal without a leading `0`; truncated to 32 bits on ELFCLASS32) | the number, never moved by the load bias | `SHN_ABS` | `STT_NOTYPE` | 0 |
| `B` | `B`'s address, by §4.4's lookup of `B` | `B`'s | `B`'s output type | 0 |
| `B + N`, `B - N` | `B`'s address plus or minus `N` | the section holding the value | `STT_NOTYPE` | 0 |

`def[SYM]` is the option's: no redirection, every mention of `SYM` (and
only of `SYM`) binds to it, whatever an input defines. A line without
`--defsym` has the empty set, and no rule pays for it (each reader
starts with an emptiness test). Every rule that asks which definition a
reference binds to, or what `SYM` is, reads it:

- §4.4's lookup by name of a global (R1, R2, R4, R5, the slot, GOT and
  PLT checks, stage 3's views, S12): a number is an absolute value, and
  §5.1a's "absolute" (a GOT load relaxed to an immediate, a word given
  no `RELATIVE` record) holds for it; an alias or `B±N` is as absolute
  as `B`'s definition is. A `B±N` whose `B` is an IFUNC is not judged
  (`defsym-shape`);
- `.symtab` (Y1, Y3): `SYM` is in `.symtab`, defined, with the table's
  value, section, type and size, binding `STB_GLOBAL` (`STB_LOCAL` when the
  line makes it local, as a version script's `local:` does); the
  ordinary Y1/Y3 checks of a global skip `SYM` (an input's replaced
  definition is not what `.symtab` holds);
- the input set (§3.8): `B` is a root, so `B`'s member is reached; a
  reference to `SYM` is no edge, so `SYM`'s defining member is not reached
  (M12 on mortar's map; a reference's map is the union of edges, rule 2,
  which accepts lld's extraction of it);
- stage 3's model (§12): `SYM` is a definition of the program's file 0
  (no import, P-D25, P-D7's keys); in `-shared` it is exported and
  preemptible by its own name (D-110, D-112), so P-D17's interposer
  views take a call of `SYM` that does not go through the PLT;
- the PIE's dynamic records (P-D9's view 2): a word holding a number
  does not move with the executable; one holding an alias's or `B±N`'s
  address does.

Not derived, and named in the coverage (`--defsym, not judged:
--defsym=SYM=EXPR: the value is not derived; ...`): a chain (`B` is
another word's `SYM`), and every form resolve.md §7.2 refuses on the
command line (octal, `k`/`M` suffixes, arithmetic between numbers,
`ALIGN`, `.`). mortar refuses them, so its output never has them; on a
reference's output every record against such a `SYM` is
`defsym-shape`.

lld 19's `-Map` writes each `--defsym` as a row of its four numbers with
no text (size 0): an assignment, not a placed section (§3.5).

GNU ld's forms are named (§10.2): A85 (`B±N` as `SHN_ABS` with `B`'s
type, and in a PIE a word holding it with no record: Y3 ending "(a
--defsym B+N or B-N written as GNU ld writes it: SHN_ABS, B's type;
resolve.md §7.2: B's section, STT_NOTYPE)", P-D9 ending "(a --defsym
B+N or B-N: in B's section, moved with the executable; resolve.md
§7.2)"), A86 (the replaced definition's size), A87 (a number moved by
the load base, P-D9 or a relaxed GOT load's R1, ending "(a --defsym
number: SHN_ABS, never relocated; resolve.md §7.2)"), A88 (the type of
an earlier word of a repeated `SYM`). mold aborts writing `-Map` when a
`--defsym` names a symbol and refuses `B+N`: such a link has no mold
fixture. The same findings on mortar's output are never named. Tests:
`tests/defsym.rs` (`tests/defsym/gen.sh`),
`tests/link/resolve/defsym*.test`,
`tests/link/dyn/shared/defsym-shared.test`.

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

**A thread-local undefined weak** (AArch64, WP-092b; glibc's
`_nl_current_LC_*`, referenced by `setlocale.o` and `uselocale.o`
through IE): in a static output, `S + A` is judged by the TP and DTP
offsets its linker writes for it, which no ABI text fixes (the gABI,
the TLS ABI and AAELF64 do not say what an undefined weak TLS symbol's
offset is; GNU ld warns "Weak TLS is implementation defined"), so per
map format, from the references measured (a64.md §7's experiment, and
WP-092b's runs of a static glibc `setlocale` program):

- **T1w** mortar and lld: `TPREL(S + A) = DTPREL(S + A) = A` (a64.md
  §7 "thread-local `weak0`": the TP offset 0, no `PT_TLS` needed). The
  IE GOT entry holds `A`, or IE is relaxed to LE with `T = A` (W9);
  TLSDESC and GD are judged with the same `T`.
- **T2w** GNU ld and mold: the formula with `S = 0` against `PT_TLS`
  (`TPREL(A) = A − tls.start + TCB`) for IE's GOT entry and TLSDESC;
  mold's LE too. GNU ld's LE (`0`, the addend dropped) and both linkers'
  GD (a module/offset pair) stay `tls`, as does any reference without a
  `PT_TLS`.
- In an output with `PT_DYNAMIC` the symbol stays `tls` (a64.md D-25:
  mortar refuses it there; lld adds a `TLS_TPREL64`).

On x86-64 and i386 the same records were already judged (x64.md X-18,
x86.md X86-23: 0 for mortar and lld, `TpRel(0)` for GNU ld and mold;
§10.4's tables). The value a program computes is `TP + T`, never read
by glibc (each access is guarded by the non-TLS weak
`_nl_current_LC_<cat>_used`).

**Not judged** (`tls`): an AArch64 thread-local undefined weak in the
cases T2w and the dynamic bullet leave; a non-TLS
type against a TLS symbol; a TLSDESC sequence kept for a dynamic loader;
the TLS records of an output whose `PT_TLS` is unusable (S9).

Static PIE (`ET_DYN` with no `PT_INTERP`) is stage 3 (§12.4, WP-089): on
the reference linkers its TLS takes the same forms and has no TLS
dynamic relocation (P-D14 names one).

Tests: `tests/tls.rs` (§9.1; T1w/T2w: `s4_7_undefined_weak_tls_initial_exec_is_judged`,
per reference linker, with a code and a GOT-entry mutant), `tests/mutation.rs
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

### 5.1a A PC-relative field to an absolute symbol (x86-64, WP-088)

In an `ET_DYN` output (a `Pie` or a static-pie) every address moves with
the load bias and an absolute symbol's value does not, so S + A − P with
S `SHN_ABS` (§4.4's resolution; an undefined input reference counts by the
output's symbol, `x64_abs`) is right at bias 0 only. On x86-64, a record of
an allocated section whose operation is PC-relative (`PC8`..`PC64`,
`PLT32`) against such a symbol, and a GOT load rewritten to `lea S(%rip)`
(X1) for one, is R1 "a PC-relative field to an absolute symbol in a
position-independent output", whatever the field holds. A GOT load kept,
or rewritten to `mov $S` (X4), is right. `ET_EXEC` is not judged by this
rule: nothing moves. i386 is not judged by it (no case measured).

Measured (WP-088; `absv` = 0x1234 defined `SHN_ABS` in another object,
x86-64, `-pie` and `-static-pie`): GNU ld 2.44, lld 22 and mortar refuse
`lea absv(%rip)` (`PC32`), `call absv@PLT` (`PLT32`) and `.long`/`.quad
absv - .` in `.data` (`PC32`, `PC64`); mold 2.37.1 refuses the `.data`
pair but links the `PLT32` call as `call 0x1234`, wrong at any nonzero
bias (anomaly A35). `movq absv@GOTPCREL(%rip)` (`REX_GOTPCRELX`): GNU ld
writes `mov $0x1234`, lld, mold and mortar keep the GOT load; all eight
outputs run (qemu, glibc 2.41). Under `-no-pie` all four link
`lea absv(%rip)` and the program runs. Test: `tests/dyn.rs`
`x64_pc_relative_to_an_absolute_symbol_in_a_pie` (case `abspc`: GNU ld
and lld refuse, mold's output is this rule's).

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
- **`tlsgd-to-ie` (AArch64, stage 3, WP-082).** The GD sequence of W12
  made `ADRP x0, G; LDR x0, [x0, :lo12:G]; MRS x1, TPIDR_EL0; ADD x0, x1,
  x0`, with G a GOT entry (§5.5 valid-slot conditions) holding X; in a
  dynamic output G carries a `TLS_TPREL64` record and X is what the
  loader writes there (§12.1's loader model; a64.md §8.4 R4). GNU ld
  writes it for a GD reference to a library's variable; mold keeps the
  sequence; lld refuses the traditional dialect's relocations.

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

   A group whose section's output section is `SHF_COMPRESSED` is not
   decided: its members are counted not judged, `compressed`, at R6's
   entry (WP-131 V4; there is no inflate). GNU ld's and lld's
   `--compress-debug-sections=zlib` outputs were R6 before, read raw. In
   mortar's output a compressed section is S1 (mortar writes `=none`
   only).

When a group is decided does not change its verdict, only how long its
members are held (WP-109). A group whose section no other object can
name (the section defines no symbol that is not `STB_LOCAL`) has all its
members once the objects are read past its own, so it is decided then;
the others, and every group when the objects are not read in order, are
decided after the last section. Before WP-109 every group waited for the
end: tens of millions of members on a `-g` link (`.debug_info`'s
references to `.debug_str`), the verifier's largest structure.

### 5.7 Discarded targets (R5)

A relocation of a placed section whose symbol's section is discarded or
absent is R5, for a local symbol and, by §4.4 step 0, for a global one
whose every definition is so (WP-143); and a reference to a label of an
empty section without a record that names an address neither in nor at
the end of an allocated output section (§3.3). Whether the section's
absence has a reason is M10's (§3.3); R5 holds either way. This does not apply to `.eh_frame`'s own records, which
§6.4 checks (an FDE whose function is discarded is dropped, not R5), nor
to non-alloc records (§5.8). A field of `.eh_frame` or `.gcc_except_table`
is judged by the record that owns it (§5.7b): R5 when the owner is
written, nothing (or 0 in mortar's output) when it is not.

### 5.7a A demoted mention whose key nothing live defines (WP-178)

A non-local mention defined in a section of a COMDAT copy that is not
kept (§3.3's keeper, the verifier's own, `spec/resolve.md` §6.1: the
first copy the link reads keeps the signature; here the order is the
command line's, the line the verifier reads itself, each object at the
first word naming its path, an archive member at the first word naming
its archive (its path, `-l<name>` for `lib<name>.a`, `-l:<file>`: the
archive is read there, WP-201), and an object the line does not name (a
driver's file) at the rank of the one before it in `FileId` order;
without a line, `FileId` order. A map's order is not
the line's: lld's lists a file where its first section is placed, so an
object without `.eh_frame` comes after a later one with it) is a
*demoted mention* (`spec/resolve.md` §6.2): an undefined mention of its
key with its binding. When no definition of the key has a place and the
loader does not bind it (a needed library's definition, or its `COPY`
into the output, binds it), §4.4 step 0 gives a relocation against the mention the
dropped section, and the record is R5 ("refers to section '…', which is
not in the output"). That is the rule, not an accident of the order of
checks: the model follows `spec/resolve.md` §6.3 case (b), **no value
exists for such a reference in any output kind, weak or strong** (not
0, not an import, not the next instruction), and a linker that writes
one has a defect. The mention's binding decides resolution (precedence,
extraction: a weak one extracts no archive member and makes no
`--as-needed` library needed, D-11) but never gives the reference a
value.

To name the defect, a record in an allocated section against a
**weak** demoted mention that step 0 makes R5 is judged a second time as
an undefined weak reference (§4.4's undefined weak, stage 3's import
bound to nothing in view 1), whose outcome is not kept: when that
judgement verifies or accepts the field (W4, W5, the GOT slot 0 or its
dynamic record), the R5's text ends "(a weak mention defined in a
dropped COMDAT copy, whose key nothing live defines, written as an
undefined weak reference: resolve.md §6.3 case (b))" (A103 for GNU ld
and lld; a FAIL for mortar, which refuses such a link and never writes
it). Anything else in the field (another value, a record not judged)
leaves R5's plain text; a strong or `STB_GNU_UNIQUE` demoted mention's
R5 is always plain. In the trial, §4.4's step 3 (the one output symbol
of the name, of any binding) is not taken: no definition of the key has
a place, so an output symbol of the name is a local the mention never
binds (an input's local; `objcopy --localize-hidden`'s kept copy of the
group, WP-201). A personality field of a written CIE (§6.4) has the
same trial on its output fields: the undefined weak form is the field
holding `A` (the place, §4.4) or `S = 0` in every output CIE that
carries it. Stage 3's model (§12.1) takes a demoted mention as
an undefined mention of its key too: it defines nothing, and a weak one
is a weak reference (so an import of such a key is no P-D7 "no input
references it"). A non-weak one is a strong reference for the binding
it asks of the output, never for D-11's neededness (resolve §6.2, dyn.md
D-11 item 2, WP-200; WP-231): a library in `DT_NEEDED` that only such
mentions refer to a definition of is P-D6 "... which D-11 does not make
needed (only a mention defined in a dropped COMDAT copy refers to a
definition of it: resolve §6.2, D-11 item 2)"; GNU ld makes it needed
(A128). Tests: `tests/demoted.rs`; the lane's
`resolve/comdat-mismatch-weak*.test`.

### 5.7b A field of `.eh_frame` or `.gcc_except_table`: its owner (WP-201)

`spec/resolve.md` §6.3 (WP-177): a field of `.eh_frame` or of a section
named exactly `.gcc_except_table` that refers to a section not in the
output is judged by the **record that owns it**, never by the field
alone. The verifier names the owner itself: its own parse of the
inputs' `.eh_frame` (§6.4, 32- and 64-bit lengths), its own liveness of
each FDE (§6.4's dropped, live, unknown), its own keeper (§5.7a). Nothing
comes from a map beyond the placements every rule reads.

- **`.eh_frame`.** A field's owner is its record; a CIE's personality
  field is owned through the FDEs that use it: written when one live
  FDE uses it (§6.4). A dropped FDE's fields and an unused CIE's
  personality are counted with the records of sections not in the
  output. A written CIE's personality field that refers to a section
  not in the output is R5 at the CIE's field, at the first output CIE
  that carries it (it was `not checked: eh-frame` before WP-201, and
  E4 went unreported).
- **`.gcc_except_table`** (alloc, the exact name; `.gcc_except_table.<fn>`
  is any other alloc section, §5.7). The LSDA starts are the targets of
  the LSDA pointers of the FDEs of every input `.eh_frame` the link
  reads (not `SHF_EXCLUDE`, not a member of a COMDAT copy the keeper
  drops), **over every input file**: an FDE's LSDA may be in another
  file's table (a global label). A start's life is live if an FDE at
  it is live, else unknown if one is unknown, else dead. The owner of
  the field at offset `r` of table `T` is the start of `T` that is the
  greatest one not above `r`.
  - Owner **dead**: the field is free on another linker's output (the
    record is not read: no finding, counted with the records of
    sections not in the output, whatever it holds); in mortar's output
    it must be 0 (`spec/resolve.md` §6.3: mortar writes 0), verified
    `tombstone`, else a Field finding "a field of an LSDA no written FDE
    reads, …, holds another value than 0".
  - Owner **live**: R5, with §5.7a's trial for a weak demoted mention;
    otherwise its text names the field's form: "(a field of a written
    .eh_frame or .gcc_except_table record, written 0: resolve.md §6.3)"
    when it holds 0 (A105, GNU ld), "(…, written as if its target were
    at address 0: …)" when it holds `A` or `A - P` (A106, mold);
    another value is plain R5. The same words for a written CIE's
    personality field.
  - **No owner can be named** (no start at or before the field; an
    unreadable input `.eh_frame` or its relocations, an LSDA pointer of
    another type or whose target the verifier cannot place, which may
    name any table; an FDE at the start whose liveness is unknown): R5,
    its text saying why the owner is not known. Never taken as dead:
    the unsafe direction is a live record's field passed unread.

An LSDA pointer without a relocation is no start (it names no input
section). Tests: `tests/ehowner.rs` (`tests/ehowner/`, AArch64: GNU
ld's, mold's and mortar's outputs: a dead owner, a live one in the same
file, at a start shared with a dead FDE, in another file after a dead
start, in the 64-bit form, a written CIE's personality, no start before
the field, the only FDE in an `.eh_frame` of a dropped copy; each live
or unknown case with the field mutated); `ehowner::tests`; the lane's
`resolve/comdat-eh-dead.test`, `comdat-eh-wrong.test`,
`comdat-mismatch-weak-prelink.test`, `comdat-eh-unused-cie.test`.

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

| referring section | GNU ld | lld | mold | mortar (`spec/image.md` I-47, I-47a) |
|---|---|---|---|---|
| `.debug_ranges` | 1, or the redirect | 1 | 1 | 1, or the kept copy |
| `.debug_loc` | 0, or the redirect | 1 | 1 | 1, or the kept copy |
| `.debug_rnglists`, `.debug_loclists`, `.debug_addr` | 0, or the redirect | 0 | 0, or A | 0, or the kept copy |
| `.debug_aranges` | 0, or the redirect | 0 | 0, or A | all ones, and the tuple's length 0 (I-112), or the kept copy |
| `.debug_frame` | 0, or the redirect | 0 | 0, or A | all ones, or the kept copy |
| other debug names | 0, or the redirect | 0 | 0, or A | all ones (ELFCLASS64), 0 (ELFCLASS32), or the kept copy |
| `.gcc_except_table` | 0, or the redirect | 0 | 0, or A | 0 |
| any other name | 0, or the redirect | 0 | 0, or A | R5: mortar refuses the link (I-46) |

- **mortar's kept copy** (`spec/image.md` I-47a, WP-250). The record's
  section has a debug name (I-45), its symbol is a section symbol or an
  `STB_LOCAL` symbol, and the symbol's section D has no `SHF_ALLOC` and
  is dead only because it is in a COMDAT copy the verifier's keeper
  drops: the keeper is §5.7a's (`spec/resolve.md` §6.1, the first copy
  in the line's order, `FileId` order without a line; the same keeper
  M10 holds the map to), derived from the inputs, never read from the
  map. In the keeper's copy of the group (its `SHT_GROUP` section's
  members) the verifier takes the members with D's name, then of those
  the ones with D's `sh_size`, and requires exactly one, K, with no
  `SHF_ALLOC`, in the output; and K and D are placed whole, neither
  `SHF_MERGE` nor `.eh_frame` (the supervisor's amendment to I-47a,
  WP-250). K's place is its offset in the output section of its name
  (§3.3). Then X is K's place + the symbol's `st_value` +
  A for an absolute operation, that − P for a PC-relative one, decided
  by §5.2 in the field's width (overflow is R1 as for any target). A
  field that differs is R1 "a reference into a dropped COMDAT copy of a
  non-alloc group member does not hold the kept copy's place + st_value
  + A (spec/image.md I-47a)", with "; it holds a tombstone" when it
  holds all ones, 0 or 1. Each branch of the condition that fails (no
  member of D's name; one of another size; more than one of D's name and
  size; it is `SHF_ALLOC`; K or D is `SHF_MERGE` or `.eh_frame`; it is
  not in the output, absent or discarded by the map) leaves the column's
  tombstone, and a field that differs from it is R1 with ";
  spec/image.md I-47a keeps the tombstone here: <the branch>". K placed
  in pieces or ambiguous (§3.3) is not judged: `section-not-locatable`.
  The referring section is binding: only a record of a live non-alloc
  section with a debug name is redirected. A record of an `SHF_ALLOC`
  section (`.gcc_except_table`: 0, §5.7b) or of a non-alloc section
  without a debug name (R5, I-46) into D keeps its column, whatever the
  field holds. The verifier does not parse DWARF and
  does not compare D's bytes with K's (I-47a does not either). A
  verified field counts under `kept-copy` (§7.2).
- **GNU ld's redirect** (`_bfd_elf_check_kept_section`, measured on
  WP-249's probe and on GCC 14 `-g3` links, WP-250): for a record whose
  symbol is a section symbol or `STB_LOCAL` and whose dead target D is
  in a dropped COMDAT copy (`SHF_GROUP`), the keeper's copy (as above)
  is searched in group order for its first member whose defined symbols
  match D's: the same list of (name, `st_info`, `st_other`), sorted by
  name, not empty (section symbols and the mapping symbols `$d`/`$x`
  count; the name of the member is not compared). When that member is
  placed whole with D's `sh_size`, X is its place + `st_value` + A;
  otherwise the column's 0 or 1. So GNU ld writes a `.debug_types`
  offset into a `.debug_macro` field when the keeper's copy has no
  `.debug_macro` (WP-249 §2.1, its case C4): that is GNU's rule and is
  accepted from GNU's map only. A collected section (no group) has no
  redirect.
- **mold's A** (measured, WP-250): mold writes 0 + A for a dead target
  without `SHF_ALLOC` and 0 for an `SHF_ALLOC` one (1 in `.debug_ranges`
  and `.debug_loc` either way). It keeps a second copy of GCC's `wm4.*`
  groups, so its `-g3` imports have live targets and are judged as any
  record.
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

Verified tombstones count under `tombstone`, verified kept copies (mortar's
and GNU ld's redirect) under `kept-copy` (§7.2).

**A target only a shared library defines** (stage 3, WP-104; the
ruling of the RB-1 defect, `spec/landed/harness/WP-100.md` §5). Nothing
relocates a non-alloc section at run time (`spec/arch.md` §4.3
"Non-alloc places", `spec/a64.md` §13.2), so such a field cannot hold
the address the loader gives a library's symbol; it holds the
symbol's *link-time* value. The rule, for a record of a placed section
without `SHF_ALLOC` in a dynamic output (`PT_DYNAMIC`, §12) whose
symbol is a name of binding `STB_GLOBAL`, `STB_WEAK` or
`STB_GNU_UNIQUE` (any visibility of the reference: a `.hidden` or
`.protected` undefined reference is the same record) such that:

1. the output's `.symtab` has no defined global of the name (a name
   the output defines, a copy allocation included, is §4.4 step 1's:
   S is that definition; the copy itself is judged by P-D11, which
   holds every alias of the library's definition at the `COPY`
   record's place);
2. no input object has a placed definition of it (§4.4 step 2); and
3. a shared object of the loader's order (§12.1: the executable's
   `DT_NEEDED` entries breadth-first) has a visible definition of it
   (`.dynsym`, `st_shndx` not `SHN_UNDEF`, not `STV_HIDDEN` or
   `STV_INTERNAL`; the first such object is the *library*, its entry's
   `st_value` is `v` and its type `t`).

Then S is:

- **the copy** when the output's `.dynsym` defines the name
  (`st_shndx` not `SHN_UNDEF`): no input defines it (item 2), so the
  definition is a `COPY` record's allocation, and P-D11 holds every
  alias of the library's definition at that record's place; S is that
  `st_value`. (Item 1 sends a name `.symtab` defines to §4.4, with the
  same value; `.dynsym` decides when `.symtab` does not name it, as in
  mortar's outputs);
- **the canonical PLT entry** when the output is `ET_EXEC` and its
  `.dynsym` has an entry of the name with `st_shndx = SHN_UNDEF` and a
  non-zero `st_value` (`spec/dyn.md` D-44: that value is the symbol's
  address in the whole program; P-D9 and P-D13 judge the entry): S is
  that `st_value`. The verifier reads this from the output's `.dynsym`,
  not from its own D-43/D-44 decision, because P-D7 and P-D9 already
  hold the output's choice to that decision;
- **0** otherwise. An ordinary PLT entry (a PIE's, or one no address is
  taken of), an IPLT entry for a library's IFUNC, a GOT slot and the
  loader's address of the definition are *not* the symbol's address.

The field is then decided by §5.2 with, by the record's operation:

| operation (types) | X |
|---|---|
| absolute (`R_AARCH64_ABS64`/`ABS32`/`ABS16`; `R_X86_64_64`/`32`/`32S`; `R_386_32`, REL's implicit addend read from the input) | S + A |
| PC-relative (`R_AARCH64_PREL64`/`PREL32`/`PREL16`; `R_X86_64_PC64`/`PC32`; `R_386_PC32`) | S + A − P, P the place (`sh_addr` of the output section, 0, plus the offset: §5.8) |
| DTP offset (`R_X86_64_DTPOFF64`/`DTPOFF32`; `R_AARCH64_TLS_DTPREL64`, `R_386_TLS_LDO_32`) when `t` is `STT_TLS` | 0 + A, whether or not the output has a `PT_TLS` |

and every other operation falls to the general procedure (§5.1). A
record whose operation is absolute or PC-relative while `t` is
`STT_TLS` is P-D12 "a non-TLS data word in a non-alloc section against
'...', whose shared definition is STT_TLS (refused by name: spec/a64.md
§13.2, spec/arch.md §4.3)", whatever the field holds: GNU ld refuses
it ("bad value"), and so does mortar. An undefined weak no shared
object defines is not this rule's (item 3 fails): it is §4.4's
undefined weak, X = A for a PC-relative type on AArch64 (S = P) and
S = 0 on x86. i386 dynamic outputs are outside stage 3 (§12.1), so
the i386 rows are the references' measurement, not judged.

A field that differs is R1, and its message names what the field holds
when it is one of the references' conventions, after "the relocated
field holds the wrong value (a word against 'n', which only a shared
library defines, holds its link-time value (copy, canonical PLT entry or 0) + A: spec/plumb.md §5.8)":
"; it holds the library's PLT entry" (A37, A40), "; it holds 0, the
addend dropped" (A38), "; it holds the library's st_value" (A39),
"; it holds A − the PT_TLS address" (A42). The anomaly a suffix names
is accepted only from its linker (§10.2), never from mortar's map. The
rule is in `crates/mortar_plumb/src/dsoword.rs`.

Measured (WP-104, `spec/landed/plumb/WP-104.md`, the table in §10.6):
GNU ld on AArch64 writes exactly this rule's values on every variant;
lld writes them for every PC-relative word and 0 for every absolute
one (A38); GNU ld on x86-64 writes the PLT entry for a function or an
IFUNC that has one in a PIE, and on AArch64 too in a non-PIE for a
function the program only calls (A37); mold writes the library's `st_value`
(A39) or the PLT entry (A40). Test:
`tests/link/dyn/reloc/debug-word-shared.test` (the RB-1 defect's file,
moved and extended), `tests/dyn.rs` `s5_8_a_word_against_a_shared_definition`.

Observed (AArch64, GCC 14 `-g -O0`, the corpus's `debug-comdat`, and
`-gdwarf-4 -O2 --gc-sections`, `debug-gc`): GNU ld redirects the dropped
COMDAT `big`'s `.debug_info`, `.debug_aranges`, `.debug_rnglists` and
`.debug_line` references to the kept copy (0x40073c), writes 1 in
`.debug_ranges` and 0 in `.debug_loc` for a collected function; lld and
mold write 0, and 1 in `.debug_ranges`/`.debug_loc`. No linker changes
the aranges length.

Measured (WP-250, GCC 14 `-g3`, C and C++, DWARF 4 and 5, AArch64 and
x86-64, `spec/landed/plumb/WP-250.md`): every `DW_MACRO_import` of a
dropped `wm4.*` copy meets I-47a's condition; GNU ld's and lld's links
verify with no finding (GNU on AArch64: A10 only), mold's with its known
anomalies only, and one word moved by 1 in each is R1.

Tests: `tests/tls.rs s5_8_debug_references_verify_clean`,
`tests/mutation.rs s9_2j_debug_mutations_are_found`,
`tests/macro_import.rs` (I-47a: mortar's output patched to the kept
copy, the tombstone, 0 and off by 4; each branch of the condition by a
hand-written pair of objects, `SHF_MERGE` members and a `.debug_info`
member referred to from `.debug_gnu_pubnames` included; referrers
without a debug name or with `SHF_ALLOC`; the three reference linkers'
columns on GCC `-g3` links and on WP-249's probe, each with a word
moved).

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
- `PT_GNU_STACK`'s `PF_X` in a static output of a reference (no line;
  mortar's is S10's, §3.2a, and a dynamic output's P-D4's, WP-088);
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
4. **The option.** Mortar's map gives it: its line (§3.2a), the option
   given or not; the caller's statement is not read for it. A
   reference's map has the caller state it (`Map.fix_843419`:
   `xtask plumb --fix-cortex-a53-843419`/`--no-fix-cortex-a53-843419`,
   or read from the `--inputs-from` response file); otherwise any fix
   found shows it was given, and a link with no fix leaves its sites not
   judged (coverage `erratum_not_judged`, and the coverage line says
   which: the link without the option, or, for a reference, the option
   unknown).

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
- **C6** (mortar). A fix in a link without the option (its line, §3.2a,
  or the caller's statement).

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
"Static" is a static `ET_EXEC`; static-pie is judged by stage 3
(§12.4).

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
  class. [M2] Stage 3 widens it by P-D1 (§12): `ET_DYN` with
  `DF_1_PIE` (a `Pie`, and a static-pie when it has no `PT_INTERP`,
  §12.4), and `ET_EXEC` with `PT_INTERP`, are verified. `EI_OSABI` is `ELFOSABI_NONE` (0) or `ELFOSABI_GNU` (3); on
  `EM_386` and `EM_X86_64`, `e_flags` is 0 (the psABIs define none,
  x86.md X86-1). **(mortar)** `EI_OSABI` is 3 exactly when `.symtab` or
  `.dynsym` holds an `STT_GNU_IFUNC` or `STB_GNU_UNIQUE` symbol (dyn.md
  D-23; WP-082), or the output has an `IRELATIVE` record
  in an allocated relocation section (image.md §5.4, a64.md Q11, x64.md
  X-49, x86.md X86-64; WP-028e). The references on i386: GNU ld writes 3
  for every static glibc output and for `ibt` (IFUNC), 0 for the
  `-nostdlib` ones without; lld and mold write 0 (x86.md §15.1).
- **S2.** There is at least one `PT_LOAD`. For each one:
  - `p_filesz <= p_memsz`;
  - `p_align` is 0, 1 or a power of two;
  - `p_vaddr ≡ p_offset (mod p_align)`;
  - its file image lies inside the file. A file image of size 0
    (`p_filesz` 0: a segment of `.bss` only) is the empty range
    `[p_offset, p_offset)`, which lies inside any file whatever
    `p_offset` (WP-088, WP-059's F1). Measured on a `-nostdlib` static
    link whose only writable data is one `.bss` word: GNU ld writes
    `p_filesz` 0 with `p_offset` 0xffe8 in a 1,736-byte file on AArch64
    and `p_offset` 0 on x86-64, lld `p_filesz` 0 on both, mortar
    `p_filesz` 0 on both (on x86-64 with `p_offset` inside `.comment`'s
    bytes); every output runs (natively, kernel 6.8, and under qemu),
    the loader mapping such a segment as anonymous memory. On i386 GNU
    ld, lld and mortar give that segment 12 or 13 file bytes (`.got.plt`
    and the like), and mold, on all three machines, writes file-backed
    writable segments: no empty image there. Tests: `tests/mutation.rs
    s6_2_s2_s17_an_empty_file_image_lies_anywhere` (an empty image past
    the end of the file and one inside `.symtab`: accepted; the same with
    8 file bytes: S2, S17);
  - its memory does not wrap (in an `ELFCLASS32` output, it ends at or
    below 2^32);
  - in mortar's output (by map format), `p_align` is at least 4 KiB
    (`spec/layout.md` L-1; layout.md §20.3, WP-028b), and it is the
    line's maxpage or a section's larger alignment, with the base
    rounded to it (S2 (mortar) below, WP-131 V3; it replaces WP-028e's
    fixed i386 0x1000 and 0x08048000).
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
  literal pools fault. A `PT_LOAD` that holds sections is `PF_W` only
  if it holds a writable section and `PF_X` only if it holds an
  executable one (L-6 made a rule for mortar, WP-028d; every format
  since WP-088: no reference output of the test corpora, static or
  dynamic, GNU ld, lld and mold on the three machines, asks for a
  permission none of its sections needs, and a writable text or an
  executable data segment is the permission an attacker needs).
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
  `p_align` is 16 (x86.md X86-66). **(mortar's map, static)** Its `PF_X`
  is the line's (§3.2a, L-84): set under the last `-z execstack`, clear
  under `-z noexecstack`, and with neither set exactly when an input's
  `.note.GNU-stack` is `SHF_EXECINSTR`.
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
  `PT_LOAD`'s file image (L-18). An empty file image (`p_filesz` 0)
  overlaps nothing, wherever its `p_offset` (S2; WP-059's F1).
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

**mortar's pages and its always-present headers (mortar's map; WP-131 V3,
V5).** These clauses read the line (§3.2a):

- **S2 (mortar).** maxpage is the line's last `-z max-page-size` (0 or
  none: 64 KiB on AArch64, 4 KiB on x86-64 and i386; L-74). Every
  `PT_LOAD` has `p_align` = max(maxpage, the largest `sh_addralign` of
  a section in it) (L-67), and an i386 `ET_EXEC` output's lowest
  `PT_LOAD` is at align_down(0x08048000, that `p_align`) (L-68, X86-65).
  The 64-bit base (0x400000) is not judged: the corpus tests read lld's
  outputs, whose base is 0x200000, under mortar's map. `-z common-page-size`
  changes nothing (L-75). Before WP-131 the clause held i386 at 0x1000
  and 0x08048000 whatever the line said.
- **S10 (mortar).** There is a `PT_GNU_STACK` (L-84: always).
- **S11 (mortar).** Every allocated `SHT_NOTE` section with a size lies
  in a `PT_NOTE` (L-67).
- **S20 (mortar).** With a `.pad.relro` (rank 7 non-empty, L-64) there
  is a `PT_GNU_RELRO`, and its memory ends where `.pad.relro` ends.
- **Y2 (mortar).** There is a `.symtab` when the line has no
  `-s`/`--strip-all` (synth.md S-84). (A `.symtab` under `-s` is not
  judged here: the options audit reads a line with `-s` added against an
  unstripped output, and expects P-G6 alone.)
- **S1 (mortar).** No section is `SHF_COMPRESSED` (V4); `.interp`,
  `.eh_frame_hdr`, `.eh_frame` (on x86-64 also `SHT_X86_64_UNWIND`),
  `.dynamic`, `.dynsym`, `.symtab`, `.dynstr`, `.strtab`, `.shstrtab`,
  `.gnu.hash`, `.hash`, the init, fini and preinit arrays, the three
  `.gnu.version*` sections and `.note*` have their gABI `sh_type`: tools
  (gdb, readelf, strip) find them by type.

Each was found by a mutation that left a correct mortar output's header
or table out and drew no finding (WP-131's walk of P-D1 to P-D15 and
§6.2; `spec/landed/plumb/WP-131.md`).

Not taken from layout.md §20.3: its S19 (`ET_DYN` alignment) is stage
3's (S1 accepts `ET_DYN` only through P-D1, §12); its
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
  in the output, and the FDE in `fdes_dropped`. An input `.eh_frame`
  that is a member of a COMDAT copy the keeper drops (§5.7a) is not
  read by the link: none of its records is judged, all are counted
  with the records of sections not in the output (WP-201);
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
  `fdes_neutralised`) or, under a reference's map, a PLT FDE (E6).
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
- **E6.** An output FDE no live input gives whose initial location is in
  a PLT section (`.plt`, `.plt.got`, `.plt.sec`, `.iplt`) is the
  linker's own unwind information for its PLT (GNU ld's
  `--ld-generated-unwind-info`; lld and mold write none), accepted
  instead of E4 under a reference's map and counted in
  `fdes_linker_plt`. On x86-64 it is judged against the PLT's stack
  (WP-088):
  - it lies inside that one section;
  - at every instruction of the section that a PLT entry or its lazy
    path executes (P-D10's walk, §12.2, from each entry the PLT
    decoder finds, with the lazy word as the file holds it), its CIE's
    initial instructions and its own, run as DWARF 4 §6.4.2 says, give
    the CFA as `%rsp` + *d*, where *d* is 8 (the return address) plus 8
    for each `push` the path has run (`push $n`, `push *GOT+8(%rip)`,
    `push %r11`); a CFA expression is evaluated with `%rsp` and `%rip`
    (the instruction's address) and no other register, at two stack
    pointers;
  - the return address register's rule is "saved at CFA - 8", and no
    other register has a rule other than "same value" (the PLT saves
    none).

  One finding per FDE, at its first wrong instruction, with the count.
  Padding no path executes is not judged (the GNU ld expression's
  `lit11` and `lit10` differ only at offset 10 of an entry, inside the
  `push`). A shorter range, a PLT partly without unwind information, is
  accepted: lld, mold and mortar give none for all of it. AArch64 and
  i386 PLT FDEs are counted and not judged (no reference writes one on
  AArch64; i386 is not a stage 3 target). The instructions judged are
  reported (`plt_fde_insns`). Measured on GNU ld 2.44's x86-64 hello PIE
  lazy and `-z now`, no-PIE, and `-z ibtplt` lazy and now (the `.plt`
  FDE with `lit11`, `lit9` under IBT; the `.plt.got` and `.plt.sec`
  FDEs with the CIE's rule only): clean. Test: `tests/dyn.rs`
  `s6_4_e6_plt_fde_against_the_plt` (case `plt-fde`: three outputs,
  three mutants each: the range 16 bytes past `.plt`, the expression's
  `lit` raised by one, PLT0's `def_cfa_offset 24` lowered to 16; each
  is E6, and none is found with E6 dropped); `src/cfa.rs` unit tests.

**Coverage.** The relocation records of an input `.eh_frame` are counted as
follows:

- the initial-location and LSDA records of a matched FDE are verified
  (`eh-frame`);
- a personality record is verified when one live FDE of its CIE is;
  one that refers to a section not in the output fails it (R5, §5.7b);
- E2 and E3 fail the FDE's records;
- a record at no pointer field of a CIE or FDE is I2;
- a CIE whose FDEs are all dropped has its personality record counted as
  not in the output.

`fdes_checked`, `fdes_dropped`, `fdes_neutralised` and
`eh_frame_hdr_entries` are reported.

**Not judged.** These are reported by name:

- relocations from other sections to a label inside an input `.eh_frame`
  (crtbeginT.o's `__EH_FRAME_BEGIN__`, 4 records per glibc link) under a
  foreign map that rewrites or does not list `.eh_frame` (GNU ld, mold):
  `eh-frame`. Their value is where that input's frames begin in the
  output, which a rewritten or unlisted `.eh_frame` does not say. Under
  mortar's map offset 0 is judged (§4.4, S-50 rule 1), and so is any
  offset of a kept record (rule 2); an offset in or at a removed record
  (rules 3 and 4) is still `eh-frame`;
- the instructions' meaning (they are compared byte for byte, not
  interpreted);
- the LSDA's contents (`.gcc_except_table` is checked by S12 and its
  relocations by §5, a field into a dropped copy by its owner, §5.7b);
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
  - Every entry's `st_name` is the offset of a NUL-terminated string
    inside `.strtab`'s `sh_size`: the output's reader refuses any other,
    so it is O1 (§6.1). An `st_name` in `.strtab`'s file slack (§6.8,
    past `sh_size`) is therefore O1, and one inside a `dead` range of an
    incremental map is P-I3 (§6.8). An `st_name` that names the middle of
    a live string reads as another name: Y5's closed set, Y6 and Y3
    judge that name (WP-246).
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
  - A name `--exclude-libs` excludes from the exports of a dynamic
    executable (P-D7, D-110 item 1; WP-161) is accepted with every
    `.symtab` entry of it `STB_LOCAL` or `STV_HIDDEN`, one of them
    defined: D-110 says the linker makes it `STB_LOCAL`; measured
    (`dyn/dynsym/exclude-libs-exe`, `-pie` without `-E`), GNU ld writes
    the function `STB_LOCAL STV_DEFAULT` and the data object
    `STB_GLOBAL STV_HIDDEN`, lld and mold `STB_LOCAL`. No loader reads
    `.symtab`, and neither form is in `.dynsym`.
  - Its type and size are D's.
  - Its `st_shndx` names an allocated section whose range holds D's input
    section's place. The range includes its end address: a label at the
    end of a section, or of an empty input section placed at a section's
    end, is held by that section (`synth.md` S-122, S-123, R-P4). For an
    `SHN_ABS` definition it is `SHN_ABS`.

  A name with an IFUNC definition, with no placed definition, or a TLS
  name without a usable `PT_TLS`, is counted in `globals_unchecked`,
  except (WP-143):
  - **a missing global.** When the output has a `.symtab`, no symbol of
    any binding has the name, and an input defines it (global or weak) in an allocated
    section placed whole, or as pieces with a piece holding its label (a
    dead piece of a merged section is collected under `--gc-sections`, and
    its labels with it: glibc's `__nptl_version`), or (mortar's map) without bytes and
    without a record: a finding. Whatever its visibility V (WP-246: a
    hidden or internal definition is a local of its definer's block,
    `synth.md` S-77, GNU ld's `GLOBAL HIDDEN`, lld's and mold's `LOCAL`;
    under GNU ld's map a hidden definition in an empty input
    `.eh_frame` is not judged (GNU ld drops it: x86 `crtbeginT.o`'s shape,
    measured in WP-246 on `layout/eh-frame-terminator-only{,-pie}.test`);
    before WP-246 a hidden name was not judged, and an output whose
    `.strtab` named the middle of glibc's `__libc_single_threaded`,
    `readed`, passed). A foreign map's merged section is not
    known to hold anything (lld collects the pieces of one under
    `--gc-sections`), so its names are not judged so. A versioned input
    name (`foo@@V2`) is present when its base name is. Before WP-143 a
    missing name was skipped: mortar's output without `gms` passed;
  - **a label of an empty section without a record** (§3.3, "absent";
    not thread-local): its output value is in or at the end of an
    allocated output section, else a finding;
  - **a label of an input `.eh_frame`** (WP-143b). When the name has no
    placed definition, every input definition is in an input `.eh_frame`
    and the output has an allocated `.eh_frame`, the output value of the
    first strong definition (else the first) at input offset r is judged:
    - under mortar's map, by `synth.md` S-50 (S-48a for a terminator
      alone), re-derived from the map's `piece` records: rule 1 (r = 0:
      the section's start; §4.4 decides this one before this bullet),
      rule 2 (a piece covering r), and rule 3 (rule 4 for a symbol in a
      removed record, whose next kept record is the same): the first
      piece of the same section at or after r, else the first piece of
      the later input `.eh_frame` sections in `SectionId` order (S-46),
      else the final terminator, the output's last 4 bytes. No record
      is parsed: a symbol in a removed record and one at its end both
      take the next kept record, and a symbol inside a kept record is in
      its piece. A value other than this: a finding;
    - under a reference's map, which does not say where a removed record
      went: in or at the end of the output `.eh_frame`
      (`[sh_addr, sh_addr + sh_size]`), else a finding. Measured with a
      global at the end of a terminator-only input (`get.o end.o`; and
      with a live CIE and FDE before or after it: `get.o getc.o end.o`,
      `get.o end.o getc.o`), on AArch64, x86-64 and i386: GNU ld places
      the input (edited to size 0) and writes the label at its end (the
      output's end for the first two, offset 4 for the third, whose
      terminator it drops), which its map's row gives, so Y1 judges it;
      lld writes the output's start + 4 (the input offset, from the
      start), the end only for the first; mold leaves the name out of
      `.symtab` (the missing-global bullet above does not apply: the
      label is in no live piece and no section placed whole). All are in
      or at the end of the output `.eh_frame`;
    - Before WP-143b, such a name was counted in `globals_unchecked`
      (unless at r = 0, or in a kept record), and WP-143's bullet below
      called mortar's S-48a terminator "not in the output".

    When the output has no allocated `.eh_frame`, the bullet below
    applies under mortar's map, and the name stays unchecked under a
    reference's. Test: `tests/ehsym.rs`.
  - **a name with no definition with a place** (§4.4 step 0; mortar's
    map) that the output defines: a finding, whatever its value.
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
    without this, no output `$x` or `$d` was judged); likewise an
    output local in a non-alloc section (GNU ld keeps Clang's `$d` of
    `.comment`, an `SHF_MERGE` string section, where the merged empty
    string lands: `nonpic-analysis-before-start`, WP-121);
  - the symbol is a mapping symbol (`$x…`, `$d…`, AAELF64 "Mapping
    symbols") at an address of an allocated section that no input section
    occupies: the linker's own, marking a veneer or a patch.

  A name the inputs also define globally, or a TLS local without a usable
  `PT_TLS`, is also counted in `locals_unchecked`.

  **A local of a section with no place** (WP-147; mortar's map;
  `spec/resolve.md` §10.1 P-1: omitted from `.symtab`). When every input
  local of the name is in a section with no place (§4.4 step 0's
  predicate), the name is not a mapping symbol and no input defines it
  globally, an output local of the name is a finding, whatever its value
  or section. Before WP-136 mortar wrote a `.note.gnu.property` local as
  its offset in the note; mold still does (`SHN_ABS`, the offset),
  measured with an unreferenced local in the note on AArch64 and x86-64;
  GNU ld and lld omit it. Under a reference's map such a local stays in
  `locals_unchecked`. Test: `tests/pexport.rs
  wp147_local_of_a_placeless_section_in_symtab_is_found` (the local as
  its offset, in a section or absolute: Y4).
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
  | `__bss_start`, `__bss_start__` | in [E, the first writable, non-TLS `NOBITS` section at or after E outside `PT_GNU_RELRO`]; with none, in [E, the next allocated section's start] (layout.md §13.1: the start of the first rank-8 `NOBITS` chunk, else `edata`; the RELRO padding, mortar's `.pad.relro` (L-64) and lld's `.relro_padding`, is rank 7 inside `PT_GNU_RELRO`, so an output with no `.data` does not judge `__bss_start` against it; WP-169) |
  | `_end`, `end`, `__end__`, `_bss_end__`, `__bss_end__` | in [M, M rounded up to 8] |
  | `_etext`, `etext`, `__etext` | in [the end of `.text`, the next non-executable section's start] |
  | `__rela_iplt_start`/`_end` (AArch64, x86-64), `__rel_iplt_start`/`_end` (i386) | the bounds of the only allocated `SHT_RELA` (i386: `SHT_REL`) section; with none, start = end (synth.md R-P6; under `-static-pie` they are not defined, and an `ET_DYN` output's range is P-D15's, §12.4) |
  | `_DYNAMIC` | the start of `.dynamic`, else 0 |
  | `_TLS_MODULE_BASE_` | 0, and `STT_TLS` (§4.7); GNU ld's x86-64 executable (`exec`, `pie`, static) TP, align_up(`p_memsz`, `p_align`), its shared object 0 (measured, WP-144: lld 0 in every kind, mold an address, A3) |
  | `__start_X`, `__stop_X` (X a C identifier) | the bounds of section X |
  | `__CortexA53843419_H` (lld; H the hex address of a site's patched final word) | the patch W8 accepted for the 843419 site whose final word is at H, size 8; with no patched site at H the name labels dead bytes and is not judged (the site's C1 or its not-judged count reports it; WP-082) |

  The data-end symbols allow a range, because the reference linkers place
  them differently within it (§10.5).

  **The closed set (WP-246).** A defined output symbol whose name no input
  defines and the table above does not value is one of these, listed by
  name in `linker_defined_not_judged`, or a **finding** naming the entry
  (`.symtab entry i ('n') is defined, but no input symbol has the name
  and no rule names it the linker's`):
  - `spec/resolve.md` §8.1's rows (the one list of the linker's own
    symbols; on AArch64 `__data_start`, `__end__`, `__bss_start__`,
    `__bss_end__`, `_bss_end__`), `spec/layout.md` L-105's
    `__tdata_start`, and `__start_X`/`__stop_X` for a C identifier X;
  - on AArch64, a thunk's or a patch's name of `synth.md` S-80
    (`__<target>…_veneer`, `__erratum_843419_veneer`; GNU ld's stubs
    have the same form), and the ABI's mapping symbols `$x`, `$d`,
    `$x.…`, `$d.…` (S-80; lld writes them for its thunks);
  - under GNU ld's map on AArch64: its erratum 843419 stubs
    `e843419@…` (measured in WP-246, `tests/spie.rs`'s `cxx.bfd`);
  - a `--defsym` key (§4.4b judges it);
  - under lld's map: `__AArch64ADRPThunk_*`, `__AArch64AbsLongThunk_*`,
    `__AArch64BTIThunk_*`, `__CortexA53843419_*` and `__dso_handle`
    (layout.md §13.1, E-24);
  - under lld's and mold's maps, listed as `<name> (at no input section
    the map lists)`, a name at an address no listed input section
    occupies: neither map lists a member whose sections are all merged
    (glibc's `C_name.o`, `SYS_libc.o`, `itoa-digits.o`, only
    `.rodata.str*`; measured in WP-246 on the corpus's `debug-gc` (lld)
    and the lane's glibc links (mold)); and a name an input defines
    versioned (`foo@@V`), which both write as `foo` (Y3's reading;
    measured on `dyn/dynsym/versioned-export-refused.test`);
    under mold's: a name with `$`, `__start_EHDR`, `__start_PHDR`,
    `_PROCEDURE_LINKAGE_TABLE_` and `__dso_handle` (the not-judged list
    below; `_PROCEDURE_LINKAGE_TABLE_` measured in WP-246: mold 2.x
    writes it, undefined or at `.plt`, in every output of the lane);
  - a name `.dynsym` has, or whose base before `@` it has (a copy
    relocation's or a shared library's symbol, a versioned reference
    `puts@GLIBC_2.2.5`): stage 3 judges the dynamic symbols (P-D5);
  - any name, when the line has a linker script (`-T`): the script may
    assign it; listed as `<name> (the line has a script)`;
  - any name, when there is no line (a foreign map read alone): a
    `--defsym` may define it and GNU ld's map does not record one;
    listed as `<name> (no line: a --defsym may define it)`.

  Also a finding: an entry with no name (other than an `STT_SECTION` or
  `STT_FILE` one: §6.9's fillers and GNU ld's and mold's section symbols
  are unnamed `STT_SECTION`; Y7 judges file symbols), and an undefined
  entry whose name (or its base before `@`) no input symbol, defined or
  not, has, the line does not name (`-e`, `-u`, `--require-defined`,
  `-init`, `-fini`, `--export-dynamic-symbol`, `--wrap`'s `__wrap_X`
  and `__real_X`, and, when the caller reads the line's files, an
  implicit script's `ENTRY` and `EXTERN`) and that is none of the
  above; with no line such a
  name is listed (`<name> (undefined; no line: a -u may name it)`).
  Before WP-246
  every name outside the table was "not judged": mortar's incremental
  output with `readed` (the middle of `__libc_single_threaded`) at
  `.symtab` entry 2489 passed with zero findings.
- **Y6. The locals of each input file (WP-246).** Under mortar's map
  only: a foreign map does not say which locals the linker kept (GNU ld
  keeps crtbegin's `__TMC_LIST__` or not by its own rules, mold makes
  `__gmon_start__` and `_ITM_*` locals, GNU ld names a file symbol after
  the object), so there Y6 judges nothing and says so (`every local:
  Y6's rules are mortar's …`). For each input file in
  the link (§3.8: a file of the map's), each local the rules keep is in
  `.symtab` with its name, `st_info`, `st_other`, `st_size`, section and
  value, and each output local is one of them, a demoted global or the
  linker's own. The kept locals (`synth.md` S-76, S-78, S-83):
  - every `STB_LOCAL` entry other than entry 0 and `STT_SECTION` ones
    (S-79), `STT_FILE` ones included (S-83), `SHN_ABS` ones at their
    input value (S-81);
  - in a section placed whole (a `place`, an `nplace`, a `fold`'s
    target's place): its place plus the input value; a thread-local one
    as its offset from `PT_TLS`'s `p_vaddr` (S-81);
  - in a section placed as pieces: the piece that holds the value (or
    ends at it, a label at the section's end), at the piece's address
    plus the offset; with no such piece the local is dropped (S-76:
    its fragment is not live);
  - dropped: in a discarded section, in one with no place (§4.4 step 0,
    mortar's map), and by S-78: `-x` every local (`STT_FILE` too);
    `-X` every `.L` local; neither of the three, the `.L` locals of a
    section placed as pieces; `--discard-none` none. The last of `-x`,
    `-X`, `--discard-none` on the line (mortar's `arg` records, else the
    caller's line) decides; with no line Y6 judges nothing, named.

  They are compared as a multiset, keyed by name, `st_info`, `st_other`,
  `st_size` and `st_value`; order and index are free (§6.9's fillers,
  `STT_SECTION`, are passed over). A matched entry's `st_shndx` holds the
  input section's range (allocated), names a non-allocated section
  (non-allocated) or is `SHN_ABS`. A kept local with no entry is a finding
  naming the file and the input entry (`the local 'n' (file entry k,
  …) is not in .symtab`); an output local that is none of them is a
  finding naming the entry (`.symtab entry i ('n', …) is no kept input
  local`), unless it is:
  - a **demoted** global: an input defines the name globally (or as a
    common, or the line makes it local): Y3 judges it (S-77);
  - the **linker's own**: a name of Y5's closed set or a `--defsym` key,
    or a mapping symbol (`$x…`, `$d…`) in an allocated section at an
    address no input section occupies (S-80; Y4's second exception);
  - named as a local not judged (below), in the section such a local
    would be in (asked first: a `$d` of a merged `.debug_str` is counted
    there, not as the linker's).

  **Not judged** (counted by reason in `input locals not judged`): the
  locals of an input `.eh_frame` when the output has one (S-50 drops a
  mapping symbol in a removed record and an interior local no relocation
  names; the relocations are not read for this); labels of an empty
  allocated section without a record (S-123: the map does not say where);
  locals of a section merged without pieces (a foreign map's, a mortar
  `merged` record); locals of a section whose place the map does not
  give (a foreign map's `rewritten`, ambiguous or absent row);
  thread-local ones without a usable `PT_TLS`; undefined ones. A
  demoted global absent from the output is Y3's missing global.
- **Y7. File symbols (WP-246).** Every `STT_FILE` entry has a name, and
  it is the name of an input's `STT_FILE` entry (S-83: copied in place;
  under GNU ld's map also the base name of an input's path or member,
  which GNU ld inserts for a file with none, S-83, X-32). An unnamed one
  is a finding (but under GNU ld's map, where it closes the inputs'
  file symbols before the linker's own locals: measured in WP-246 in
  every GNU ld output of the lane, `pie-x64.bfd` entry 3). Under lld's
  and mold's maps a name no listed input's `STT_FILE` has is counted
  not judged (`STT_FILE names of files the map does not list`): both
  leave out of the map a file whose sections are all collected and
  still copy its `STT_FILE` (measured in WP-246, `tests/wrap.rs`'s
  `gc` cell: `gcf-a64.o`'s `fo.c`) (`.symtab entry i is an STT_FILE with no name`), and so is
  one of another name. Under mortar's map Y6 also holds them to their
  files as a multiset.

**Coverage.** `globals_checked`, `globals_unchecked`, `locals_checked`,
`locals_unchecked`, `linker_defined_checked` and
`linker_defined_not_judged` (a list of names) are reported. The summary
line is "symbol attributes: …". Y6 and Y7 add "input locals (Y6): kept
k (missing m), dropped by rule d; output locals: not a kept input's x,
demoted y, the linker's z; STT_FILE judged f" and "input locals not
judged: <reason>: <n>; …".

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
  with the rest of the dynamic sections (P-D5, P-D14; §12.4).

Tests: `tests/corpus.rs s9_1_reference_links_verify_clean` (on the glibc
cases: more than 1000 globals, more than 300 locals and at least 9
linker-defined symbols judged; not-judged names only from mold, §10.5) and
`tests/mutation.rs s9_2h_symbol_table_mutations_are_found`. WP-246's
Y3 (hidden), Y5's closed set, Y6, Y7 and P-E3 on a renamed local:
`tests/symnames.rs` (mortar's incremental and plain static links on
AArch64 and x86-64, mutated by `tests/symnames/mut.py`: a name moved
into its string, an `STT_FILE` emptied, a local dropped, two names
swapped, a global renamed, a value moved; the evidence `inc.bad` when
present).

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

**The style from the line (WP-102).** A map with `arg` records gives the
style as the line asks it (§3.2a; image.md I-69, the last of
`--build-id[=STYLE]` and `--no-build-id`): bare `--build-id` is
`default`, `=fast`, `=sha1`, `=sha256` their trees, `=0xHEX` `given`,
any other value not judged. The line decides, and two more clauses of
B1 hold: the output has an `NT_GNU_BUILD_ID` note exactly when the line
asks for one (absent, `--build-id=none` or `--no-build-id` last: none),
and a style the build-id chunk states is the line's. The tree is the
line's style's, never the chunk's alone.

SHA-1 and SHA-256 are written from FIPS 180-4 and `L128` from I-114's
text, in `src/buildid.rs`; none is shared with `mortar_digest`. The
definition is the one of this mortar version (I-75): a later tag is a
new definition, and B1 must change with it.

Tests: `src/buildid.rs` (FIPS 180-4's vectors, image.md §8.4's `L128`
and tree vectors), `tests/mutation.rs s9_2l_build_id_mutations_are_found`,
`s9_2l_build_id_style_from_the_map`.

### 6.8 Holes and dead ranges (M7, WP-091)

`spec/incr.md` IN-3, IN-3a, IN-60. An incremental update leaves bytes a
full link of the current inputs would not write; mortar's map lists them
(`hole` and `dead`, §3.2). For a map that **claims coverage** (mortar's
format with `incremental` or any `hole`/`dead` line) `src/dead.rs` and
`src/deaddwarf.rs` check, from the output and the map alone, after
stage 2. A map that claims nothing does not run them and pays nothing.

**Resolving (P-I2).** A line names one output section by name; none or
several is P-I2, and so are an empty range, one past the section's end
(but the symbol tables' file slack, below) and two lines that overlap.
Then a range must be one its section admits:

- a `hole` only in an allocated section (IN-3), or as the **file slack**
  of `.symtab` or `.strtab` in a map that says `incremental` (IN-49a;
  WP-212 applied WP-202's §R.3): `<off>` equal to the section's
  `sh_size` and `<off> + <size>` at most the file offset of the next
  section in file order (or of the section header table, when none
  follows), relative to the section's `sh_offset` (WP-222: "the next
  section" is the nearest file offset above this section's of a section
  that occupies file bytes, not `NOBITS` and of size above 0; the end
  of the file bounds it too, whichever comes first). That hole holds zero
  bytes (P-I5) and counts toward no section's coverage (P-I1 covers the
  allocated sections only; P-E6 sees zero file padding). A `hole` in a
  non-allocated section in any other form is P-I2. Not in a packed section
  (layout.md L-142, incr.md IN-30): `SHT_INIT_ARRAY`, `SHT_FINI_ARRAY`,
  `SHT_PREINIT_ARRAY`, `SHT_NOTE`, `.init_array`, `.fini_array`,
  `.preinit_array`, `.ctors`, `.dtors` (and their `.`-suffixed names),
  `.init`, `.fini`, `.tm_clone_table`, `.eh_frame_hdr`, `.note*`, and a
  section whose name is a C identifier (reachable by `__start_`/`__stop_`);
  in a thread-local section only at its end (L-143); `.eh_frame` holes
  are padding CIEs (below);
- `neutral` only in the sections IN-51b gives a form: `.debug_info`,
  `.debug_types`, `.debug_abbrev`, `.debug_line`, `.debug_rnglists`,
  `.debug_loclists`, `.debug_str_offsets`, `.debug_addr`,
  `.debug_ranges`, `.debug_loc`, `.debug_frame`, `.debug_macro`,
  `.debug_macinfo` (WP-253; not `.debug_aranges`, `.debug_str`,
  `.debug_line_str`, which have none);
- `contribution` only in IN-53's list: `.debug_line`,
  `.debug_str_offsets`, `.debug_addr`, `.debug_rnglists`,
  `.debug_loclists`, `.debug_ranges`, `.debug_loc`, `.debug_macro`,
  `.debug_frame`;
- `strings` in a non-executable section holding merged strings (a live
  piece of an `SHF_MERGE|SHF_STRINGS` input; non-alloc: `.debug_str`,
  `.debug_line_str` or a `merged` record's section), and in `.strtab` of
  a map that says `incremental` (IN-49a: names no entry uses, old names
  and the zeros of a region's slack); P-I6 judges them as other
  `strings` ranges (each ends with a NUL);
- `cie` in `.eh_frame` only;
- nothing at all in `.debug_aranges` (IN-54: rewritten whole).

**Coverage (P-I1, IN-60).** Every byte of every allocated output section
is covered by exactly one item: a live `place` (an input section placed
whole, assigned to the output section that contains it, a `.tbss`
input only to a thread-local `NOBITS` section), a `piece`, an addressed
`chunk` (one of the section's name first), a `copy` record's room, a
`common` record's (§3.2; M7 holds each to the output's symbol and to
the alignment its definitions ask, M8 each copy to its `COPY` record),
a `hole` or a `dead` line.
A chunk with pieces or lines inside it is their container (mortar lists
`.eh_frame` as one chunk and its records as pieces) and does not count.
Pieces of merged sections may share bytes (deduplication), and so may
`.eh_frame` pieces (a shared CIE). The rest must be one of:

- **alignment padding** before an item: the gap ends at the item's
  start, which is the first multiple of its alignment at or after the
  gap's start. The alignment is the input section's `sh_addralign` for
  places and pieces; for a merged piece any power of two from its own up
  to the largest of the merged pieces of that output section (mortar
  lays them out as one block); for a common or a copy, the `<align>` its
  record states; after a copy or a common symbol, also up to the output
  section's `sh_addralign` (mortar aligns what follows the block of
  copies or commons to its chunk's alignment, which the map does not
  record). Padding of a `PROGBITS` section
  holds the section's fill (trap fill in code, zero elsewhere);
- `.eh_frame`'s terminator: its last 4 bytes, zero.

A `NOBITS` section no line names is P-I1 like any other: the one the
linker makes (`.pad.relro`) is a `chunk` (§3.2, M6 requires it). Until
WP-096 these three facts were inferred here: a `NOBITS` section with no
line was accepted whole, a common's alignment was taken from the inputs
and its place from the output symbol, and a copy's alignment from its
address (the largest power of two dividing it, at most 64). An
inference in the checker can agree with a wrong output by guessing as
the linker erred; the map now states each, and M6-M8 check the
statement.

Anything else, a gap or a byte covered twice, is P-I1 naming the range
and what covers it. Measured with `cargo xtask plumb --coverage` (which
prints the walk's counts for any map, as if it claimed coverage) over
the 546 mortar outputs of a `cargo xtask linktest` run (AArch64, x86-64,
i386; static, PIE, dynamic, static-pie): **0 findings on all 546**.
What today's maps leave to the rules above, summed over the run:

| what | bytes | where |
|---|---|---|
| alignment padding, holding the fill | 1,130,391 / 454,831 / 30,199 / 35,604 / 14,726 / 660 / 470 / 143 / 56 / 8 | `.text`, `.rodata`, `.bss`, `.data.rel.ro`, `.data`, `.gcc_except_table`, `.tbss`, `.ltext`, `.bss.rel.ro`, `.tdata` |
| padding not holding the fill | 0 | — |
| shared by merge pieces | 782,502 + 288 | `.rodata`, `rodata.cst32` |
| `.eh_frame` terminator | 4 per output | `.eh_frame` |
| a `NOBITS` section no line names | 13,557,476 (506 outputs) | `.relro_padding` |
| covered by commons (symbols) and copies | in `.bss`, `.lbss`, `.bss.rel.ro` | — |
| other gaps, overlaps, uncovered tails | 0 | — |

Without the common and copy items the run had 1,844 + 6,897,168 bytes
uncovered in `.bss`/`.lbss`; with pieces aligned only to their entry
size, 321,063 bytes of `.rodata`. A "claims full coverage" marker is
therefore needed only to make a map with no line claim (an incremental
link that has no dead range yet): that is `incremental`.

**Nothing live names a range (IN-3a check 1; P-I3, P-I4).** Judged by
what a reference names (the re-review's N3):

- P-I3, `.strtab`: no `.symtab` entry other than a filler (§6.9's form,
  IN-49a) has an `st_name` inside a `.strtab` dead range (WP-202's §R.3);
- P-I3, symbols: a defined symbol of `.symtab` or `.dynsym` (section and
  file symbols aside; thread-local ones at `PT_TLS`'s `p_vaddr` plus the
  value) whose value lies in a range of its section. An input symbol
  defined at its section's end (`x_end`) is accepted when the output's
  value is the end of a live item;
- P-I3, relocations: a record of a live input section whose symbol's
  input section (its base, whatever the addend; an undefined symbol's
  chosen definition) is placed in a range. So `arr+40` for a 40-byte
  `arr` and gfortran's `blk_-8` are accepted wherever they point. An
  input section (or piece) of size 0 covers no byte: it is in a range
  only when its address is strictly inside it; at a range's start or
  end it names the boundary, as `arr+40` does (N3), whatever the
  range's kind (WP-088, WP-062's finding 9). GNU ld, lld and mold all
  place an empty input section at the address of the next item after
  its alignment (libgfortran's `.bss.empty_string.0`-`.4`, NOBITS of
  size 0, 8-aligned: at `unit.o`'s `.bss._gfortrani_c_locale` in all
  three static links of `lang/fortran-c.test`'s program), so in an
  incremental-mode layout the last ones of a region sit at its slack
  hole's start. Only allocated sections are judged here, so the
  debug-only kinds (`contribution`, `neutral`) never meet the case
  (P-I4 judges those); a size-0 section at a `contribution` range's start
  would name the boundary the same way. Tests: `tests/dead.rs
  in3a_size_zero_section_at_a_range_boundary_is_accepted` (a PROGBITS
  `.data.empty` and a NOBITS `.bss.empty`, libgfortran's shape, each at
  its hole's start and end: accepted; 8 bytes inside: P-I3);
- P-I3, tables: a word of `.got`, `.got.plt`, `.igot`, `.igot.plt`
  (outside the ranges) holding an address in a range that is not the
  end of a live item; an allocated relocation record (outside the ranges)
  that writes in a range, or a `RELATIVE`/`IRELATIVE` record whose target
  (the addend, REL: the word) is in one, except a `RELATIVE` record whose
  `r_offset` is the place of a live input section's relocation record:
  that is the record's dynamic form, judged by what it names (the
  relocation rule above; gfortran's `blk_-8` in a PIE, WP-092); an `.eh_frame_hdr` entry whose
  location or FDE is in one; `e_entry` in one; a live `.eh_frame` FDE
  whose CIE pointer lands in one;
- P-I4, DWARF: every live unit of `.debug_info` (one not in a range) is
  walked, DIE by DIE: its abbreviation offset, `DW_FORM_ref_addr`,
  `DW_FORM_strp`, `DW_FORM_line_strp` and `DW_FORM_sec_offset` by
  attribute (`DW_AT_stmt_list` → `.debug_line`; `DW_AT_ranges`,
  `DW_AT_start_scope` → `.debug_rnglists` (v5) or `.debug_ranges`; the
  location attributes → `.debug_loclists` or `.debug_loc`;
  `DW_AT_str_offsets_base`, `DW_AT_addr_base`, `DW_AT_rnglists_base`,
  `DW_AT_loclists_base`; `DW_AT_macros`, `DW_AT_GNU_macros`,
  `DW_AT_macro_info`; before DWARF 4, `data4`/`data8` of the line, range,
  location and macro attributes) may not point into a range; nor may the
  `strp`/`line_strp` entries of the v5 `.debug_line` headers live units
  name, the entries of their `.debug_str_offsets` tables,
  `.debug_aranges`' unit offsets, or a live `.debug_frame` FDE's CIE
  pointer. A live unit the walk cannot read (an unknown form, a missing
  abbreviation) is a P-I4 finding naming it: refused, not passed.

**Symbol blocks (P-I7; WP-268).** What a correct writer guarantees of
a `symblock` (`spec/image.md` I-78a; `spec/incr.md` IN-49a; synth.md
S-74, S-79, S-86), judged against the output's `.symtab` (its local
area, entries 1 to `sh_info`) and the `.strtab` it links, when every
`symblock` is in place (§3.2's P-I2 finds none: records out of place
state no blocks to judge the tables against, and are judged no
further):

1. its `<count>` entries from `<first>` are `STB_LOCAL` and none is a
   filler (IN-49a's form, §6.9): a block holds its file's `STT_FILE`,
   kept and demoted locals (S-74), and no other `STT_SECTION` entry
   exists (S-79);
2. its entries from `<first> + <count>` up to `<first> + <capacity>`
   are fillers (IN-49a: what is left of the capacity becomes fillers);
3. each of its `<count>` entries names a string inside its region:
   `st_name` and the string's NUL in `[<strtab-off>, <strtab-off> +
   <strtab-cap>)`; or one of S-86's shared names (the empty string,
   `$x`, `$d`, `__erratum_843419_veneer`) at an `st_name` in no block's
   region (the fixed table);
4. no other entry of `.symtab` (local or global; entry 0 and the
   fillers, whose `st_name` is 0, aside) has an `st_name` inside its
   region (S-86: a name is written once per entry that uses
   it; only the fixed table is shared). A record that states a block the
   output no longer has, its region now holding a global's name, is
   this (M2b-5's map, WP-268);
5. in a map that says `incremental`: every entry of the local area but
   the fillers is one of some block's `<capacity>` entries (item 1 or
   2 judges it there) (IN-49a: the local area is the blocks,
   each followed by its fillers, then the tail reserve of fillers).

A finding names the record's map line, the entry's index and its name.
Not judged: the sizes of `<capacity>` and `<strtab-cap>`, a block's
`<file>`, the records' order (§3.2). Tests: `tests/symfill.rs`
`p_i2_p_i7_symbol_blocks` (WP-268).

**The other decision records against the output (P-I8; WP-210, V-3).**
The update builds on `capacity`, `order`, `slot`, `redirect` and
`frag` (`spec/image.md` I-78a; `spec/incr.md` IN-78): a record the
output contradicts is a stale datum the next update would inherit.
What the output and the map show of each (`src/decide.rs`), in a map
that says `incremental`, each sentence a finding of P-I8 naming the
record's map line:

1. `capacity`: its `<out>` names one output section; `<size>` ≤
   `<capacity>`; an allocated section's chunk has an address and lies
   in `[sh_addr, sh_addr + sh_size)`, a non-allocated one has `-`. Two
   chunks of a section with one `<ordinal>` are a finding.
2. `order`: one per output section that has chunks (none for a section
   without), a permutation of its chunks' ordinals. In that order an
   allocated section's chunks ascend without sharing a byte, the first
   at `sh_addr`, the last ending at `sh_addr + sh_size`; the reserve
   `[addr + size, addr + capacity)` of each holds no other chunk and no
   byte of another allocated section (IN-33: the reserve is the chunk's
   own room to grow); a non-allocated section's chunks hold at most
   `sh_size` bytes in all, exactly `sh_size` when there is one. A TLS
   `NOBITS` section's addresses overlay what follows it: its chunks are
   judged by sentence 1 and its order, not its reserves.
3. Every live item the map places (`place`, `piece`, `chunk` records
   with a size) in an allocated section that has `capacity` records lies
   inside one chunk's `[addr, addr + size)` (I-78a: `size` is what the
   chunk holds; an item past it is outside every chunk the update
   knows). The first eight are named, then a count.
4. `slot`: a GOT kind's address is `room` bytes (the word; two words
   for `tls-gd`, `tls-desc`, `tls-ld`) of a `.got*` section with bytes;
   a `plt` or `iplt` address is in executable code. One record per kind
   and key (a version suffix `@v` aside: the references' keys are input
   symbols), and one key per GOT address.
5. Every GOT slot a live reference is verified through (§5.5, R4: the
   engine's own derivation from the relocated field, not the record)
   is its key's record of that kind: the reference's key (a global by
   name, a local by the map's file id and its symbol-table index, the
   module for `tls-ld`) has a record, and its address is the slot the
   reference uses. A reference with an addend is not keyed (the slot is
   of the symbol plus the addend). A used slot no record states is a
   finding ("no slot record states it", eight named, then a count).
6. A `got` or `got-tprel` slot of a global no live reference is
   verified through (its references relaxed, §5.3, or none), in an
   output without `PT_DYNAMIC` (no loader writes its word): its word is
   its key's value: 0 for an undefined name, an IPLT entry of the
   resolver for an IFUNC (§4.8), `st_value` for other data and code,
   the `TPREL` of the TLS symbol for `got-tprel`; a `got` slot of a TLS
   symbol is not judged by its word.
7. An `iplt` record's address is an IPLT entry of its key's resolver
   (§4.8; the output's `.symtab` names the IFUNC, global or demoted,
   `STT_GNU_IFUNC`); a `plt` record's address is a PLT entry whose slot
   has a `JUMP_SLOT` record naming the key.
8. `redirect`: its `<addr>` is a `chunk thunk` or `chunk patch`
   record's address; the site's section is placed (`place`) and
   `<offset>` is inside it; the branch at the site reaches `<addr>`
   (AArch64: `B`/`BL` at the site; x86-64: the `rel32` field at the
   site, the relocation's place, plus 4); one record per site.
9. `frag`: its `<out>` names one output section; its bytes at
   `<offset>` (a string to its NUL character, `entsize` bytes, of its
   class's `entsize`; or `entsize` bytes of a constant) have the L128
   hash `<hash>` (image.md I-114, `src/buildid.rs`); its address is a
   multiple of its class's alignment; it lies inside the section's
   merged chunk (`capacity … m`; a non-allocated section's chunk at the
   sum of the sizes before it in `order`); it lies in no dead range
   (§6.8 above). The first eight are named, then a count.

Not judged, by name, since only the linker's state holds the answer
(the coverage line lists them): the ordinals' creation order (`ChunkId`
order: the output shows file order, which `order` gives); the
capacities' amounts (IN-33's policy, the bound `size ≤ capacity` and the
reserve's room judged); a slot no live reference is verified through,
beyond its place and key, in an output with `PT_DYNAMIC` or of a local
key or a TLS pair kind (its word is the loader's or the TLS runtime's);
the identity order of several `frag` records of one fragment (`FragId`
order). A `frag` of a live fragment is judged; that every live fragment
has one is not (the merged section's strings are P-E6's, §6.9).

Coverage: `P-I8: capacity N, order N, slot N (J by a live reference,
their word or their entry, U by place and key only), redirect N, frag
N` after the `decision records` line's names. The real corpus (WP-210,
the kept outputs of `cargo xtask incr-update --keep`, 1,410 links of
49 scenarios, kiln and GCC, AArch64 and x86-64): 0 findings; capacity
37,155, order 32,976, slot 114,070 (114,052 judged by a reference, a
word or an entry, 18 by place and key only), redirect 0 (no output of
the lane has a thunk or a patch), frag 1,209,393. Tests:
`tests/decide.rs` (mortar's own link of a GOT, IFUNC, TLS and
merged-string program, AArch64 and x86-64: accepted with every slot
judged; each sentence's mutation of the map or the output a P-I8
finding alone), `tests/symfill.rs`
`p_i8_decision_records_against_the_output` (capacity and order).

**Fill (IN-3a check 3; P-I5).** The file slack `hole` of `.symtab` or
`.strtab` (above) holds zero bytes. An allocated `hole` holds its section's
fill: `UDF #0` (zero words) in AArch64 code, `0xcc` in x86 code, zero
elsewhere; nothing in `NOBITS`. In `.eh_frame` a hole is **one** padding
CIE of IN-34a, exactly: at least 16 bytes and a multiple of 4,
`length` = size − 4, `CIE_id` 0, version 1, augmentation "", code
alignment 1, data alignment −4, the machine's return-address register
(30, 16, 8), `DW_CFA_nop` to the end. In an allocated relocation section
a hole is whole records: inside `DT_RELACOUNT`'s (`DT_RELCOUNT`'s)
prefix of the table `DT_RELA` (`DT_REL`) points at, `RELATIVE` records
with symbol 0, all aimed at one word (the scratch word, dyn.md §14);
after it, `R_*_NONE` with symbol 0 (or such a `RELATIVE`).

**Well formed (IN-3a check 2; P-I6).** A `strings` range ends with a
NUL. **Whole and distinct strings (WP-210).** IN-3a check 3 keeps a dead
string piece's bytes; what the output alone can show of a piece zeroed
instead is judged here. Let `E` be the section's character size
(`sh_entsize`; 1 in a non-allocated section whose `sh_entsize` is 0:
DWARF's strings are bytes). In a section aligned at most at `E`
(`sh_addralign` ≤ `E`), other than `.strtab`, no fragment is preceded by
padding (`spec/synth.md` S-27: padding exists only before a fragment
aligned above `E`), so a `strings` range is whole strings: its offset
and size multiples of `E`, each string ending with a NUL character (`E`
zero bytes), and **no two equal**, an empty string (a NUL character that
ends no string) included, since a merged section holds each string
once (S-24, `spec/layout.md` L-147: fragments are unique by content in
their class). A dead piece of more than one character zeroed, or partly
zeroed, reads as the empty string twice: P-I6. Not judged further, the
NUL end only: `.strtab` (IN-49a: the zeros of its region slack), a
section aligned above `E` (S-27's padding is zero characters, which a
zeroed string cannot be told from) and an allocated section with
`sh_entsize` 0 (several classes); and anywhere a piece rewritten with
other bytes that still end with a NUL (the bytes it keeps are the
previous output's, which the verifier does not read; the frag records
of §3.2 name live fragments only). On the lane every `strings` range is
in `.debug_str` (aligned at 1). `tests/dead.rs`
`in3a_zeroed_dead_string_pieces_are_rejected`. A `cie` range is whole CIEs (`CIE_id` 0, version 1 or 3), nothing
else. In the debug sections:

- `neutral`, a sequence of the section's forms tiling the range, each
  **exactly IN-51b's bytes** (WP-253; `src/neutral.rs`, written from
  the spec's table, never from the linker's writer), in its notation
  (`L` the form's size, `A` the output's address size, `T`, `B`):
  - **The one table** (IN-51). In a map that says `incremental`, or
    that has a `neutral` line in `.debug_info` or `.debug_types`, an
    output with a `.debug_abbrev` holds IN-51's 38-byte table as the
    section's **last 38 bytes**, inside one `neutral` line that ends at
    the section's end (mortar writes the `ABBREV_TABLE` hole as its own
    line, `dead .debug_abbrev <size - 0x26> 26 neutral`); `T` is that
    offset. No such line, a table byte that differs, or a table
    elsewhere (any byte of a `neutral` `.debug_abbrev` range before
    `T` that is not zero) is P-I6. Every other neutral
    `.debug_abbrev` byte is zero: the per-file tables accepted until
    WP-253 (`01 11 00 80 40 09 00 00`, `01 11 00 73 17 …` in a dead
    range) are findings now.
  - `.debug_info`: units of either version in any range, `L` at least
    14: `u32(L−4) 05 00 01 A u32(T)` (DWARF 5, `DW_UT_compile`, a type
    unit's range included) or `u32(L−4) V 00 u32(T) A` (`V` 2 to 4,
    IN-51's "same version"), then abbreviation code 1, or code 3 and
    `u32(D+8)` in a version-5 unit only, or code 4 and `u32(D)` in a
    version 2–4 unit only, then `B`: the ULEB length in the smallest
    width `k` that ends the block at the unit's end with at least one
    byte, padded with `0x80` continuation bytes, and the block's zeros.
    A code-3 unit's `D` is the start of a v5-header form of a `neutral`
    `.debug_addr` range; a code-4 unit's `D` the start of a headerless
    one (below); a base naming anything else is P-I6.
  - `.debug_types`: `u32(L−4) 04 00 u32(T) A u64(S) u32(23) 02 B`, `S`
    = `0xdead << 48` | the unit's offset in `.debug_types`, `L` at
    least 26.
  - `.debug_line`: `u32(L−4) 05 00 A 00 u32(20) 01 01 01 fb 0e 07 00 01
    01 01 01 00 01 01 08 00 01 01 08 00` and `0x06` to the end (`L` at
    least 32), or `u32(L−4) 04 00 u32(14) 01 01 01 fb 0e 07 00 01 01 01
    01 00 00 00` and `0x06` (`L` at least 24), either in any range.
  - `.debug_rnglists`, `.debug_loclists`: `u32(L−4) 05 00 A 00 u32(0)`
    and zeros (`L` at least 12); `.debug_str_offsets`: `u32(L−4) 05 00
    00 00` and zeros, `L` a multiple of 4.
  - `.debug_addr`: at a point a code-4 unit's base names, a
    **headerless** table: zero bytes up to the next point a base names
    (code 4's `D`, code 3's `D`) or the range's end, a multiple of `A`
    and at least `A`; elsewhere `u32(L−4) 05 00 A 00` and zero entries,
    `L − 8` a multiple of `A`. So a zero range no code-4 unit names
    reads as a v5 header of length 0: P-I6 (IN-51: a dead table stays
    named). A v5 table no code-3 unit names is accepted (IN-51b does
    not say that a v5 form must be named; readelf's "Corrupt
    .debug_addr" is the cost, WP-228 §A.3).
  - `.debug_ranges`, `.debug_loc`: zero bytes, at least `2·A`;
    `.debug_macinfo`: zero bytes.
  - `.debug_frame`: `u32(L−4) ff ff ff ff 01 00 01 7c 1e` and zeros
    (`DW_CFA_nop`), `L` at least 13: column 30 on every machine.
  - `.debug_macro`: `u16(V) 00 02 00`, `x` bytes, `00 00`, `V` 4 or 5
    in any range.

  Before WP-253 these were accepted and are findings now: a
  `.debug_line` header with other values in its fixed fields (the
  minimum instruction length, `maximum_operations_per_instruction`,
  `default_is_stmt`, `line_base`, `line_range`, the standard opcode
  lengths and the v5 address size were not read), a v2 or v3 line
  table (IN-51b's v2–v4 row is the version-4 header), a `.debug_frame`
  CIE with other alignment factors or another return-address column, a
  `.debug_info` unit whose address size is not the output's, list
  headers with another address size, a `DW_FORM_block` length in a
  width other than the smallest, and a unit naming any table but the
  one at `T`. Before WP-253 these were findings and are accepted now:
  a v5 `.debug_addr` table of 4-byte addresses in ELFCLASS32 (8 was
  required in every output) and a `.debug_ranges` range of 8 to 15
  zero bytes in ELFCLASS32 (16 was required; IN-51b says `2·A`).
  No finding is a P-I6 offset alone: each names the map line's range
  and the section offset of the first byte that differs;
- `contribution`, a chain of whole units (records) of the section, with
  every address operand image.md I-47's tombstone: `DW_LNE_set_address`
  (all ones in ELFCLASS64, 0 in ELFCLASS32); `.debug_addr` entries,
  `DW_RLE_`/`DW_LLE_` `base_address`, `start_end` and `start_length`
  operands (0); a `.debug_frame` FDE's initial location (all ones); a
  `.debug_ranges` base selection's base (1). In a position-independent
  output (`ET_DYN`) a `contribution` in `.debug_rnglists`,
  `.debug_loclists`, `.debug_addr` or (ELFCLASS32) `.debug_line` is
  P-I6 (IN-53: neutral there). `.debug_loc` and `.debug_macro`
  contributions are accepted as they are (below).

**Not judged, and open questions.**

1. A `.debug_ranges` (and `.debug_loc`) pair other than a base
   selection is an address or an offset from the dead unit's base, and
   the output no longer says which: only base selections are judged.
   `.debug_loc` and `.debug_macro` contributions are not parsed. Proposed
   text for IN-53: "a dead `.debug_ranges` or `.debug_loc` contribution
   has every pair `(1, 1)` or a base selection of 1 followed by its
   offsets", which the verifier could then check.
2. The `strings` kind is new here: IN-3a says dead fragments are dead
   ranges but its map grammar has no kind words. Dead **constant**
   fragments are `hole` lines (they take the fill).
3. A live reference through `DW_FORM_strx`, `addrx`, `rnglistx`,
   `loclistx` goes through a live table: the tables' own entries are
   judged (`.debug_str_offsets`), the others' are not (they point inside
   their own contribution).
4. `.gdb_index` (judged in a full link's output, §6.10) and
   `.debug_names` are not judged here (not generated with
   `--incremental`, incr.md §7).
5. `R_*_RELATIVE` records' scratch word is not in the map (dyn.md §14):
   the verifier only requires one word per hole.
6. Allocated relocation sections other than the `DT_RELA` table (such
   as `.rela.plt`) take `NONE` records only.
7. The decision records (§3.2, `spec/image.md` I-78a): only each
   `region`'s place inside its output section, two regions of a
   section that meet, and a `symblock`'s place (P-I2, WP-222, WP-268)
   and its entries and names (P-I7, WP-268), and since WP-210 the
   `capacity`, `order`, `slot`, `redirect` and `frag` records against
   the output (P-I8, above) are judged here; what only the state holds
   (P-I8's "Not judged") is H-1's (`spec/incr.md` IN-79 (d)), named in
   the coverage line as not judged. `spec/image.md` I-78a still says "region judged" only
   and "each `symblock` within `.symtab`'s local area": a sentence
   there is the supervisor's (WP-222's record, F-2; WP-268's).

Tests: `tests/dead.rs` (§9.2 (p)), `map::mortar::tests::s3_2_dead_*`;
the neutral forms both ways, table-driven (WP-253, `src/neutral.rs`):
`neutral::tests::in51b_every_form_is_accepted` (20 forms, one per row
of IN-51b's table with a form, at the minimum, minimum + 1 or + the
granule, an odd and a long length, `A` 4 and 8: 160 outputs),
`in51b_every_mutation_is_a_finding` (1,916 rows: every header byte,
the length ± 1, a byte in the body's run, the block one byte short with
a null entry after it, codes 3 and 4 in the wrong version, bases naming
no table of their form, a signature off the scheme, ranges below their
minimum or off their granule; it fails when a form has fewer than 8
rows or a section the judgement knows has no form in the table),
`in51b_versions_mix_in_one_range`, `in51_the_one_table` (each of its
38 bytes, an abbreviation missing, not at the end, no line, a second
table, a unit naming another), `in51b_block_widths`.
Cost (WP-091, measured; release `xtask plumb`, 3 runs each, host load
about 12; mortar's static AArch64 protoc, 11.7 MB, and protoc `-g`,
50.3 MB): a map that claims nothing runs none of this (protoc 0.27–0.33 s
and 175 MiB, protoc `-g` 2.15–2.42 s and 1.30 GiB, against 0.27–0.44 s
and 1.87–2.12 s at the base commit: noise, same peak RSS). With
`incremental` added to the map (coverage, the fills, the symbol,
relocation and table walks): protoc 0.32–0.42 s, `-g` 2.01–2.18 s, RSS
unchanged, 0 findings: today's protoc maps are complete. With also one
`dead .debug_str` line (the walk of every live unit): `-g` 2.02–2.76 s,
RSS unchanged, 0 findings.

### 6.9 Equivalence of an update and a full link (M7, WP-092)

`spec/incr.md` IN-2, IN-62. `equiv(X, F)` judges two links of the same
inputs and options: `X` an incremental update, `F` a full link
(`--incremental=full`; any mortar map is accepted for `F`, so a plain
link is an `F` too, which is IN-8's pair). Each side is an output, its
mortar map, and the inputs its map names (one set, read once; §8). It
never panics on any of them (§9.4).

**The two sides first.** Each side is verified as any link
(`verify`/`verify_with`, every stage; §1). `X`'s map must claim coverage
(§3.2: `incremental` or a `hole`/`dead` line), so §6.8 runs on `X`;
`F`'s need not. Every finding of either side is part of the answer,
named by its side (`X:`, `F:`): `X` not verified is IN-1 (A) failing,
and an `F` that is not verified cannot be compared with. P-E findings
come on top. A side whose output or map cannot be read, a map of
another format, or an `X` map that claims nothing, is a usage error
(§8), not a finding.

**Identities.** An item's identity is what it is, not where it went:

- a **file**: its map record's `(path, member, member offset)`, the same
  string tokens in both maps (file ids are per map and never compared);
- an **input unit**: `(file, section index)` for a `place` or `nplace`;
  `(file, section index, input offset)` for a `piece`; a `merged`
  record: `(file, section index, output section name)`;
- a **chunk**: `(kind, name, k)`, its kind and name and the number of
  earlier chunks of that kind and name in its map;
- a **common** or a **copy** (§3.2's allocation records): its `common`
  record's name, or the name its `copy` record names; its range is the
  record's `<addr>` and `<size>`, stated (WP-096: before version 2 a
  common's place was known only through its output symbol);
- an **address** `v` of side `O`: `ids_O(v)`, the set of
  `(unit, offset)` for every unit, chunk, common or copy of `O`'s map
  placed at `a` with size `z` and `a ≤ v ≤ a + z` (an input unit's offset is the input offset:
  `piece`s of one section share it; a `.tbss` unit by its TLS address),
  plus `(section name, start)` when `v` is an allocated output section's
  `sh_addr` and `(section name, end)` when it is its `sh_addr + sh_size`
  (a `NOBITS` section's too). Two addresses **correspond** when their
  sets meet. Where `F`'s set has two members and `X`'s only one of
  them, the meaning `F`'s reference had is not known from the files;
  for a global symbol it is (P-E3 below), elsewhere (table words, the
  linker's symbols) the check accepts either (not judged further).

**P-E1, item 1: sections and segments.** From the two section header
tables: the same count; at each index the same name, `sh_type`,
`sh_flags`, `sh_entsize` and, where `sh_link`/`sh_info` name a section,
that section's name. From the program headers: the same count; at each
index the same `p_type`, `p_flags` and `p_align`, and the same list of
allocated section names of size above 0 whose `sh_addr` is inside
`[p_vaddr, p_vaddr + p_memsz)`, counting the `SHF_TLS` sections in
`PT_TLS` only and only those there (`.tbss` overlaps what follows it),
and leaving `.pad.relro` out (WP-222, L10's L-2: L-64's pad is sized
from the addresses, so it is empty in one layout and not in the other;
its header still compares above, and each side's S20 and M6 judge it). From the file
header: the same `e_ident` (class, data, OS ABI), `e_type`, `e_machine`,
`e_flags`; and `e_entry`s that correspond. Sizes, addresses and offsets
are not compared.

**P-E2, item 2: the same live units, each placed once.** From the two
maps: the set of input-unit identities placed (`place`, `nplace`,
`piece`, `merged`) is the same in both, a `place` or `nplace` has the
same size in both, and no identity is placed twice in one map (M2
catches the whole case; a piece repeated is P-E2). A unit discarded or
absent in one map and placed in the other is P-E2, whatever the reason
word. `.eh_frame` is apart (IN-34a: per-file CIEs in incremental mode,
records that absorbed a 4-, 8- or 12-byte hole): a piece whose input
record is a CIE (`CIE_id` 0) is not a unit of this rule; an FDE piece is,
its size equal or (in `X` only) larger by 4, 8 or 12. Chunks: the
`common` chunks by name and the `synthetic` chunks by `(name, k)` (`.pad.relro` aside, P-E1;
its chunk record exists only when its size is above 0) are
the same sets, and so are the `common` records by name (an update that
allocates a common the full link does not, or the reverse, is P-E2),
and a common's `<file>` names, on both sides, the file of one identity
(a file by its identity, as the units' files are; another winning
file is P-E2). The comparison is made only where both maps state the
`<file>`; where one does not (a plain full link's map, §3.2), the
common's winning file is **read, not judged** (WP-222: the verifier
does not derive resolve.md §3.3's `≺`), and the equivalence summary
names it: `equiv not judged: common <file> of n common(s): a map
states none (resolve.md §3.3's winner is not derived)`; `thunk` and `patch` chunks may differ (item 4). `fold`
records are the same set. Each identity of one side missing from the
other is one finding.

**P-E3, item 3: symbols.** `.symtab` of each side, symbol 0 aside:

- **fillers** (`spec/incr.md` IN-49a; WP-212 applied WP-207's §R.2): in
  a side whose map says `incremental`, an entry before `sh_info` with
  `st_name` 0, `st_info` 0x03 (`STB_LOCAL`, `STT_SECTION`), `st_other` 0,
  `st_shndx` 1, `st_value` equal to section 1's `sh_addr`, `st_size` 0
  (and 0 in `.symtab_shndx` when there is one) is a **filler**: it is
  passed over by every rule below and counted in the side's coverage
  line (`fillers=<n>`, §7.2). A filler in a side whose map does not say
  `incremental`, or an entry at or after `sh_info` of that form, is
  P-E3. An entry before `sh_info` that differs from the form in exactly
  one of these fields (a **near-filler**: `STT_SECTION` with a name,
  another value, a size, another `st_shndx`, `st_other` set; or
  `STT_NOTYPE`/`STB_LOCAL` with every other field of the form) is
  reported as P-E3 naming the field: mortar writes no other
  `STT_SECTION` entry and no other unnamed entry (`spec/synth.md`
  S-79), so no correct writer makes one. A side that holds an unnamed
  `STT_SECTION` local of a section `k` other than 1 at `k`'s `sh_addr`
  (GNU ld's section symbols; section 1's is then of the form) follows
  another linker's convention: its fillers are not judged (WP-222), and
  the equivalence summary names it (`equiv not judged`);
- every symbol with a global, weak or unique binding is matched by name
  (a name defined twice in one table is P-E3) and has the same
  `st_info`, `st_other`, `st_size` and kind of `st_shndx` (undefined,
  absolute, common, or a section, compared by name). Its value: for a
  section-defined symbol whose name has defining symbols in the inputs
  (placed, not common), some one definition `(file, section, value)`
  is in both `ids_X(v_X)` and `ids_F(v_F)` (for `STT_TLS`, `v` is
  `PT_TLS`'s `p_vaddr` plus `st_value`): the **same definition**,
  checked by identity, not only "one of the placed ones" (Y1). Without
  an input definition (the linker's symbols), the values correspond;
  an absolute symbol's values are equal;
- the local symbols, fillers excluded, are the same multiset of
  `(name, st_info, st_other, st_size, section name)` with corresponding
  values; their
  order and indexes may differ (a moved section's locals are
  renumbered); a local of a non-allocated section corresponds by the
  identities of that section's `nplace` units: `(unit, offset)` for
  each unit at `o` of size `z` with `o ≤ v − sh_addr ≤ o + z`, plus the
  section's start and end (WP-222, L10's L-1: IN-51's slack moves the
  debug units of an incremental-mode link, and the mapping symbols
  clang puts on them, `$d`, moved with them; an address rule saw no
  counterpart); `STT_SECTION` and `STT_FILE` symbols are compared without
  their values; the locals of `.eh_frame` are passed over (an input's
  mapping symbol at its CIE: per-file CIEs are in incremental mode only,
  IN-34a);
- `.dynsym`, when there is one: the same entries **in the same order**
  (its indexes are named by relocations and hash tables), each as a
  global above.

**P-E4, item 4: relocated fields by value.** For every relocation record
of every input unit placed in both (a piece: the record its piece
covers), each side's engine (§5) gives the record's outcome: verified,
accepted by a named rewriting (W1-W13, X1-X4), not checked, or a
finding. The value check is each side's own: the engine recomputes `S`
from the definition through that side's placement and checks the
field, so a record that is verified or accepted on both sides holds
the same value up to placement, provided `S` names the same item in
both, which P-E3 (globals), the input symbol table (locals and section
symbols, the same bytes on both sides) and P-E5 (GOT slots) make so.
P-E4 is then:

1. a record verified or accepted on one side and not on the other, or
   not checked on either side (the reason is named: a value the check
   cannot see is not passed);
2. a record whose forms differ, unless both forms are in IN-2's set:
   none (the unrelaxed form), W1 (`ADRP`+`ADD` ↔ `NOP`+`ADR`), W2 and
   W3 (the GOT load and its relaxed forms), W6 (a range thunk ↔ a direct
   branch), W7 and W8 (erratum 843419), and, in a position-independent
   output (`ET_DYN`) only, X1-X4 (x86-64 `GOTPCRELX`). Every other
   rewriting must be the same on both sides: the TLS ones (W9-W13, GD to
   IE), the weak-branch ones (W4, W5), and X1-X4 in an `ET_EXEC` output
   (IN-2: "`GOTPCRELX` for a non-preemptible symbol in a non-PIC link";
   ruled for v1, 2026-11-16: a static or other `ET_EXEC` output whose
   distances would make them differ is refused, `incr.md` IN-2 item 4);
3. a byte of a unit placed in both that differs between `X` and `F`
   outside the relocated ranges of its records (§6.3; on x86, also the
   bytes of an accepted rewriting's window) and outside the words a
   side's S12 accepted as rewritten (an erratum site's branch, W7's
   `ADR`, W12's word). `piece` units are compared too (S12 does not
   read them); an `.eh_frame` FDE piece is compared over its input
   record's length, outside its relocated ranges and its CIE pointer,
   with its length word and any absorbed bytes (all `DW_CFA_nop`) apart.

DWARF is reached only through relocated fields (IN-2 item 4): a debug
unit's non-relocated bytes are compared byte for byte (3), its
relocated fields by value (§5.8 tombstones included).

**P-E5, item 5: synthetic tables and packed order.**

- the GOT (`.got`, `.got.plt`, `.igot`, `.igot.plt`): the multisets of
  words outside `X`'s holes, each word that lies in an allocated section
  by its address identity (corresponding), every other word by value
  (a TP offset, a module index, 0), are the same; the GOT header's
  reserved words (`.got.plt`'s first three) by position;
- `.rela.plt`'s and `.rela.iplt`'s records: the same, **in the same
  order** (IN-2 item 5: the `IRELATIVE`s), each by type, symbol name,
  `r_offset` in the same output section, and corresponding addends;
- `.rela.dyn` (`.rel.dyn`): the same multiset of records outside `X`'s
  holes, each by type, symbol name, corresponding `r_offset` and
  addend (an addend that lies in no allocated section by value);
- `.eh_frame_hdr`: the same set of entries, each the pair (initial
  location, FDE) by identity (the location corresponding, the FDE the
  same input record);
- notes: in each allocated `SHT_NOTE` section (a non-allocated one,
  `.note.stapsdt` with addresses in its descriptors, is made of units
  that (4) and P-E6 compare), the same notes in the same order,
  each by name, type and descriptor, except the descriptor of
  `NT_GNU_BUILD_ID` ("GNU", 3), whose length only is compared (IN-2: the
  build id differs);
- the dynamic tables: `.dynamic`'s entries, the same tags in the same
  order, an address tag's value corresponding (`DT_INIT`, `DT_FINI`,
  the arrays, hash and string and symbol tables, `DT_JMPREL`,
  `DT_PLTGOT`, the relocation tables, the version tables), every other
  value equal except `DT_RELASZ`/`DT_RELSZ` and `DT_RELACOUNT`/
  `DT_RELCOUNT`, which in `X` may exceed `F`'s by the bytes and
  records of `X`'s holes in that table; `.dynstr`, `.hash`, `.gnu.hash`
  (its bloom filter included), `.gnu.version`, `.gnu.version_r` and
  `.gnu.version_d` byte for byte;
- **packed order**: in every packed output section (§6.8's list:
  `SHT_INIT_ARRAY`, `SHT_FINI_ARRAY`, `SHT_PREINIT_ARRAY`, `SHT_NOTE`,
  `.init_array`, `.fini_array`, `.preinit_array`, `.ctors`, `.dtors`
  and their `.`-suffixed names, `.init`, `.fini`, `.tm_clone_table`,
  `.note*`, a C-identifier name), the units of size above 0 in address
  order are the same sequence of identities (a `.tbss` unit, whose TLS
  address overlaps what follows it, is none of them); in `.eh_frame`, the FDE
  pieces in address order are the same sequence (CIEs and padding CIEs
  passed over).

**P-E6, item 6: nothing else.** `X`'s §6.8 findings are item 6's
(every allocated byte covered once; every range passing IN-3a's three
checks). Beyond the allocated sections, every byte of `X`'s file is: in
the file header, the program or section header table, a section's file
image, or zero (file padding; on x86, inside an executable `PT_LOAD`'s
file image, also `0xcc` and `0x90`, the code fill and the nop padding); a non-allocated output section with
`nplace` units and no `merged` record is tiled by them, `dead` lines of
`X`'s map and zero alignment padding; **a non-allocated section with
`merged` records is not tiled** (WP-268, ruled on G-5b's
`.debug_str`): its `dead` lines are §6.8's (P-I2's admission, P-I6's
NUL end), its other bytes are the strings its references reach (§5.6
judges each reference by content); no map lists its pieces one by one,
so no map could tile it, with or without `nplace` units beside the
records. **Its strings are `F`'s (WP-210, WP-252's gap (ii)).** Split
at its NUL characters (`sh_entsize` bytes, 1 when 0), the non-empty
strings of `X`'s section outside `X`'s `dead` lines of it are, as a
set, those of `F`'s section of that name outside `F`'s, and none is
there twice (a merged section holds each string once): a stray byte
inside a string, or one that joins or splits two, is a string `F`
lacks; a string `X` keeps outside its dead lines that `F` does not hold
is one an edit left without its dead line; a dead line over a string
`F` holds is a live string `X` dropped. Empty strings (zero bytes) are
not judged (S-27's padding, `.comment`'s leading NUL). Measured on the
incr-update lane (49 scenarios, 636 hops, WP-210): 684 sections
(`.comment` 516, `.debug_str` 84, `.debug_line_str` 84), equal sets in
every one, no string twice. A non-allocated section with no unit and no
`merged` record, other than `.symtab`, `.strtab` and `.shstrtab` (P-E1,
P-E3), holds the same bytes as `F`'s, **one with `SHF_MERGE|SHF_STRINGS`
and no line at all too** (WP-210, WP-252's gap (i); WP-268 passed it
over): 112 such `.comment` sections on the lane, each `F`'s bytes.
`tests/equiv.rs` `p_e6_merged_nonalloc_is_not_tiled`,
`p_e6_merged_strings_with_no_line_hold_f_bytes`.

**Item 7**, the diagnostics, is not in the files: the harness compares
the two links' diagnostics (IN-5, L10; `spec/incr.md` IN-63).

**What it does not judge.** The program's behaviour beyond these items
(a program run is L10's); the meaning of a table word or a linker
symbol whose address has two identities (above); merged strings of a
non-allocated `merged` section (their references are checked by content
on each side, §5.6) beyond the set of their strings (above); thunks' and patches' placement (only the values
they carry); `.symtab`'s order of local symbols; a common's `<file>`
where a map does not state it, and the fillers of a side that writes
another linker's section symbols (both named in the summary, WP-222);
`.pad.relro`'s size and segment (each side's S20 and M6 judge it,
WP-222).

**Rulings** (the coordinator, 2026-11-16, on WP-092's questions; each
rule above is as ruled):

1. `.symtab`'s locals are compared as a multiset (P-E3): yes.
2. `GOTPCRELX`: X1-X4 may differ only in an `ET_DYN` output (P-E4 2):
   the conservative reading, accepted for v1. A static executable whose
   distances would make them differ (over 2 GiB, where X1 is not taken
   by distance) is refused; `incr.md` IN-2 item 4 says so.
3. `.eh_frame_hdr`'s table is compared as a set (P-E5): yes (each side
   sorts it by location, §6.4).
4. The build id is compared by length only (P-E1): yes; a style that
   hashes the output differs by design.
5. `.gnu.hash`'s bloom filter, `.hash`, `.dynstr` byte for byte (P-E5):
   accepted for v1 (static executables first); an update that appends
   an export is refused.
6. `.eh_frame`'s CIEs are not compared by order; §6.4 judges the CIE an
   FDE names against the input's on each side: as written.
7. File padding outside every section is zero in `X` (x86 code fill
   inside an executable `PT_LOAD`, P-E6): as written.
8. A record neither side checks is P-E4 1, and stays so. The 20 AArch64
   records of static glibc's `setlocale.o` (initial-exec loads of
   undefined weak TLS symbols) are now judged (§4.7 T1w, WP-092b): the
   R13 pair is equivalent on all three targets.
9. `.eh_frame`'s records are a packed section (IN-2 item 5): a re-link
   with the files in another command-line order is refused by
   `.eh_frame`'s order: as written. R13's case 1 is the pair at other
   addresses in the same order.
10. Two values whose identity sets meet only at a unit's end and the
    next unit's start correspond: as written.
11. IN-62's command is `cargo xtask plumb --equiv X.map F.map X F` (§8);
    `incr.md` says so (WP-092b).

Tests: `tests/equiv.rs` (§9.2 (q)); `tests/symfill.rs` (WP-222: fillers,
the file slack, `.strtab`'s dead names, the decision records, a
common's `<file>`, L10's L-1 and L-2).

### 6.10 `.gdb_index` (WP-098)

`spec/synth.md` §17 (S-G1 to S-G9) says what mortar writes under
`--gdb-index`; gdb's manual, appendix "`.gdb_index` section format", says
what any producer's index is. gdb trusts the index: a name it leaves out
is a name gdb does not find, a range it gets wrong attributes an address
to the wrong unit. The rules below run in the plain pass (§5, after the
relocation engine, `src/gdbindex.rs` and `src/gdbnames.rs`) on every
output with a section named `.gdb_index`; an output without one pays
only for the presence check (P-G6). They read the output's own
`.debug_info`, `.debug_abbrev`, `.debug_addr`, `.debug_rnglists` and
`.debug_ranges` (relocated: what gdb reads), and for mortar's map the
inputs. Every count, offset and length is untrusted: each read is
bounded by its section, a probe by the table's size, a walk by its
section's end, and what cannot be read is a finding of the rule reading
it, never a panic.

Two kinds of rule:

- **format rules** (P-G1 to P-G5) hold for every producer: lld's,
  mold's and `gdb-add-index`'s indexes are judged by them too, and what
  they find there is recorded in §10.2;
- **mortar's rules** (P-G6 to P-G9, `Format::Mortar` only) hold mortar's
  index to S-G1 to S-G7 exactly: complete against the inputs' name
  sets, and byte for byte.

**Reading the index.** All integers little-endian (gdb's format). The
header is the version and then the offsets of the areas, each a `u32`
from the section's start: the CU list, the TU list, the address area,
the symbol table, (version 9 only) the shortcut table, the constant
pool: 24 bytes, 28 for version 9. An area runs from its offset to the
next one's; the constant pool to the section's end.

**The units of the output.** The output's `.debug_info` is walked from
offset 0 to its end, unit by unit: a 32-bit `unit_length` (not
`0xffffffff`, and not past the section), the version (2 to 5), for
version 5 the unit type, the address size (the output's word size) and
the abbreviation offset; the next unit starts `4 + unit_length` after
this one. A *type unit* is a version-5 unit of type `DW_UT_type` (2) or
`DW_UT_split_type` (6). The root DIE is read with its abbreviation from
the output's `.debug_abbrev` at the unit's offset: its tag, whether it
has children, and its attributes, each skipped by its form's size
(synth.md S-G5's list: every DWARF 5 form, `DW_FORM_indirect`, GNU's
`0x1f01`, `0x1f02`, `0x1f20`, `0x1f21`).

**A unit's claimed ranges.** The ranges the root DIE states in the
output, read as S-G5 reads them in the inputs but from the output's
relocated bytes (no relocation: every value is what gdb reads):
`DW_AT_low_pc` with `DW_AT_high_pc` (an address, or a constant added to
`low_pc`); `DW_AT_ranges` (DWARF 5: a `DW_FORM_sec_offset` into
`.debug_rnglists` or `DW_FORM_rnglistx` through the offsets table at
`DW_AT_rnglists_base`; DWARF 2-4: an offset into `.debug_ranges`, with
`DW_AT_low_pc` as the base); addresses through `.debug_addr` at
`DW_AT_addr_base` (`DW_AT_GNU_addr_base`). A claimed range is *live*
when it is not empty and lies inside one allocated section of the
output (`SHF_ALLOC`): a tombstone (0, 1 or all ones, §5.8) or an address
offset from one lies in none, since no allocated section of an
executable starts at 0. A unit's live claimed ranges are merged where
they touch or overlap before an entry is held to them: `gdb-add-index`
writes adjacent ranges of a unit as one entry.

- **P-G1 (the header).** The section is not allocated and has at least
  a header's bytes. The version is 7, 8 or 9 (gdb reads them; 1 to 6
  are obsolete). The offsets are in order, CU list ≤ TU list ≤ address
  area ≤ symbol table (≤ shortcut table) ≤ constant pool ≤ the
  section's size, and the CU list starts at the header's end. Each
  area's size is a whole number of its entries: 16 bytes in the CU
  list, 24 in the TU list, 20 in the address area, 8 in the symbol
  table, 8 in the shortcut table.
- **P-G2 (the CU list).** The CU list equals the units of the output's
  `.debug_info` that are not type units, in section order: entry `i` is
  `(offset, 4 + unit_length)` of the `i`-th such unit, both `u64`. A
  unit the walk cannot read, a missing or extra entry, or another offset
  or length is P-G2, naming the entry. (A non-empty TU list is not
  judged entry by entry; mortar's must be empty, P-G7.)
- **P-G3 (the address area).** Each entry `(low, high, cu)`: `cu` is
  below the CU count; `low < high`; `[low, high)` lies inside one
  allocated section of the output. So no entry is at a tombstone or at
  address 0 of a discarded function. In mortar's output (S-G5: the area
  is the units' ranges), each entry also lies inside one merged live
  claimed range of unit `cu`: no entry names a unit that does not cover
  it, and a unit all of whose ranges are dead (S-G5a) has no entry; a
  unit whose claimed ranges cannot be read (a form, a code or an offset
  the reading above does not know, a read past a section) makes each of
  its entries P-G3. That bound is not a format rule: gdb 16's
  `gdb-add-index`, on clang's split-DWARF units, writes an entry
  `[a, a + 1)` for a unit's global variable at `a` in `.data`, outside
  every range the unit's root DIE states (WP-098). The format sets no
  order and no completeness: gdb builds its own map, and a unit without
  entries is found by name.
- **P-G4 (the symbol table).** The slot count `T` is a power of two
  (gdb's reader masks with `T − 1`) and at least one slot is empty: a
  slot is empty when both its words are 0, and gdb's lookup of a name
  the table does not hold ends only at an empty slot. For each slot
  that is not empty: its name offset and its vector offset lie in the
  constant pool; the name is NUL-terminated inside the pool; the
  vector's count and values lie inside the pool; gdb's probe for the
  name **finds this slot**: with `h` the hash of synth.md S-G7 (gdb's
  `mapped_index_string_hash`, versions 5 and later: `h = h × 67 +
  lower(c) − 113` modulo 2^32 over the name's bytes, `lower` ASCII-only)
  and `m = T − 1`, the sequence `h & m`, then `+ (((h × 17) & m) | 1)`
  modulo `T`, reaches this slot before any empty slot and before any
  other slot holding the same name; and no name is held by two slots.
  The probe is bounded by `T` steps (the step is odd, so `T` steps visit
  every slot).
- **P-G5 (the CU vectors).** Each vector has a count of at least 1, and
  each value `v`: bits 0-23 (the unit) below the number of CU and TU
  entries; bits 24-27 zero (reserved, the manual); bits 28-30 (the
  kind) not 5, 6 or 7 (reserved). Kind 0 is accepted: gdb's manual
  calls it reserved, but GCC writes it (`GDB_INDEX_SYMBOL_KIND_NONE`)
  and gdb reads it.

**Mortar's rules.** The option is read from the map's line (§3.2a: the
last of `--gdb-index` and `--no-gdb-index` wins; `-S`, `--strip-debug`,
`-s`, `--strip-all` strip the debug information, S-G8); the chunk
`chunk synthetic .gdb_index - <size>` is the section's size, not the
option. Which inputs are units, and their name
sets, are read again from the inputs, by synth.md S-G2, S-G3a, S-G4a
and S-G5, through the inputs' relocations (§4.4: a symbol's section and
`st_value + addend`, REL's implicit addend) and the map's places.

- **P-G6 (presence, S-G1, S-G6, S-G8).** With the option in force, no
  strip option and at least one unit in the output's `.debug_info`, the
  output has exactly one `.gdb_index`, and the map one chunk of that
  name whose size is the section's; otherwise none. The section is
  `SHT_PROGBITS`, `sh_flags` 0, `sh_addralign` 4, `sh_entsize` 0, and
  its header comes after every `.debug_*` section's and before
  `.symtab`'s. With an index, no output section is named
  `.debug_gnu_pubnames` or `.debug_gnu_pubtypes` and the map places no
  input section of those names (they are consumed, S-G6).
- **P-G7 (the units of the inputs, S-G2, S-G3, S-G3a).** Each input
  `.debug_info` section the map places (`nplace`, or a `place` of a
  non-alloc section) is walked as S-G2 walks it: its units, at the
  section's output offset plus their own, must be the CU list's entries
  in that order and none other; no unit is a type unit, a partial unit,
  DWARF64 or of another address size, and no live input `.debug_types`
  is in the output. Every unit that is debug-bearing (S-G3a: a
  skeleton, a root DIE with `DW_AT_GNU_dwo_name` or `DW_AT_dwo_name`, or
  a root DIE with children) is named by exactly one pubnames set and
  exactly one pubtypes set of S-G4a (sets of input sections not
  dropped as COMDAT copies, their `debug_info_offset` read through its
  relocation). A debug-bearing unit without its sets is the silent form
  of `gdb-index-incomplete`: the index mortar should have refused. A set
  that does not parse, names no unit start, names a unit twice, or names
  a unit whose section is not in the output (S-G4a's refusals) is P-G7
  too.
- **P-G8 (complete, S-G4, S-G5).** The index's names and values are the
  inputs': for each name, the set of values of its CU vector equals the
  set `{cu | attributes << 24}` over every entry of that name in every
  live unit's sets (`cu` that unit's CU-list index); no name is in the
  index without such an entry and no entry's name is missing. The
  address area equals S-G5's list for the inputs: for each unit in
  CU-list order, its ranges in the order its root DIE and range list
  state them, read through the inputs' relocations, with the empty ones
  and those whose section the map does not place left out, each `(low,
  high, cu)` with `low` the output address of the range's start
  (the section's place plus the offset; a piece's address for a
  section placed by pieces) and `high = low + length`.
- **P-G9 (the bytes, S-G7, S-G7a).** The section's bytes are exactly
  S-G7's for that content: version 7 (8 and 9 are valid indexes, P-G1,
  but not mortar's); the offsets `24`, `24 + 16C`,
  the same, `24 + 16C + 20R`, then `+ 8T`; `T = max(1024, P)` with `P`
  the smallest power of two above `⌊4N/3⌋`; each CU vector sorted
  ascending with each value once; the names inserted in name order (the
  order of first occurrence, units in CU-list order, pubnames before
  pubtypes, set order) at the first empty slot of their probe; the
  constant pool the vectors in name order, then the names in name
  order, each with its NUL; empty slots `(0, 0)`; the size S-G7a's. The
  first area that differs is named, with the offset of its first
  differing byte. This rule judges what the content rules do not see
  (the order of the vectors and names, the empty slots, the padding a
  writer could leave): with it every byte of mortar's index is decided.

**Not judged.** `.debug_names` (lld's `--debug-names` is refused by
mortar, synth.md S-G9; input tables are copied, I-49a). The TU list's
entries (mortar's is empty; a reference's is counted, not read). The
shortcut table of a version-9 index (`gdb-add-index`'s). Whether a name
is spelled as gdb canonicalizes it (the manual: "the names of C++
symbols in the hash table are canonicalized"; S-G4 copies the
compiler's spelling, as lld and mold do; §6.10's gdb witness, below,
found no lookup that fails on it).

Tests: `tests/gdb_index.rs` (§9.2 (r), §9.4); `src/gdbindex.rs`'s unit
tests (the hash, the probe, the header). The byte sweep's `gdbindex` case
(§12.3) sweeps every byte of mortar's index.

## 7. The report

### 7.1 Findings

A finding has:

- the rule id: `O1`, `S1`–`S21`, `I1`, `I2`, `M1`–`M12` (M11, M12: §3.8), `R1`–`R9`, `T1`,
  `Y1`–`Y5`, `E1`–`E5`, `F1`–`F5` (§4.8), `P1`–`P2` (§6.6), `B1` (§6.7),
  `C1`–`C6` (§5.10), `P-D1`–`P-D13` (§12), `P-I1`–`P-I8` (§6.8; P-I7 WP-268, P-I8 WP-210), `P-M1`–`P-M3` (§13), `P-P1`–`P-P11` (§14; P-P2–P-P11 RL-5, WP-185; P-P1 RL-6, WP-186), `P-E1`–`P-E6`
  (§6.9; a side's finding there carries its side, `X:` or `F:`) or
  `P-G1`–`P-G9` (§6.10);
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
  (an IFUNC reference, §4.8), `tombstone` (§5.8), `kept-copy` (a
  reference into a dropped COMDAT copy that holds the kept copy's
  place: mortar's I-47a and GNU ld's redirect, §5.8), and for a `-r` output
  `place` and `string` (§14.6);
- **accepted**, by rewriting (W1 to W13; on x86-64 §5.9's `got-to-lea`,
  `got-to-direct`, `got-to-imm`, `got-to-mov-imm` and the TLS ones), and
  for a `-r` output `per-linker` (§14.5 P-P3's list);
- **not checked**, by reason: `tls`, `tls-sequence`, `ifunc`, `non-alloc`, `eh-frame`, `compressed`,
  `type`, `machine`, `section-not-locatable`, `piece-not-locatable`,
  `in-mergeable`, `symbol-unresolved`, `got-unpaired`,
  `input-unreadable`, `dynamic` (§12) or `wrap-shape` (§4.4a, a
  reference's output only);
- **none**, for type 0 and type 256;
- **failed**, for records with a finding.

`checked_types` counts the verified and accepted records by type. The
records of sections not in the output are counted separately
(`absent_records`). So are the sections by place, the symbols Y1 checked
and left unchecked, the bytes S12 compared, and the FDEs and header
entries of §6.4, the symbols of §6.5, the IRELATIVE records and IPLT
entries of §4.8, and the property types §6.6 judged and did not judge, what B1 made of
the build id (§6.7), the x86-64 large sections whose placement no
rule judges (`placement_not_judged`, §6.5), stage 4's `meta` (§13.2;
with the ODR runtime also `ODR runtime: outputs, entries, imports`,
WP-197), and for an output with a
`.gdb_index` its version, units, address entries and names and the rules
that judged it (`gdb index: ...`, §6.10), for each side of `--equiv`
whose map says `incremental` the fillers P-E3 passed over (`symbol
table: fillers=<n>` in the side's summary, `, fillers=<n>` at the end
of `cargo xtask plumb --equiv`'s `X:`/`F:` line; §6.9), for `--equiv`
what it read and did not judge (`equiv not judged: ...`, §6.9: a
common's `<file>` a map does not state, a side's fillers under another
linker's section symbols), and for a map with decision records (§3.2)
`decision records: n read; region judged (place only), symblock judged
(place, names), capacity, order, slot, redirect, frag judged (P-I8); not
judged: the ordinals' creation order, the capacities' amounts, a slot no
live reference uses beyond its place, frag identity order; P-I8:
capacity N, order N, slot N (J by a live reference, their word or their
entry, U by place and key only), redirect N, frag N` (WP-210, §6.8; before
it `…, symblock judged (place, names); capacity, order, slot, redirect,
frag not judged`, WP-268; before that `region, symblock judged (place
only)`). A plain map's lines
gain no word (WP-222). A stripped dynamic output
(no `.symtab`; WP-164) adds one line, a `-r` output its `relocatable`
and `composition` lines (§14.6), `stripped output (no .symtab),
not judged: a; b`, naming each clause a rule could not judge without the
symbol table (P-D6 and P-D22's `DT_INIT`/`DT_FINI` judged by segment
only, §12.5; P-D9's GOT-slot addends, §12.3); the relocations against a
name only the linker defines are `symbol-unresolved` as before (§4.4).

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
Relative paths are taken from the current directory (no map format this
verifier reads records the link's directory; a mortar map has no `cwd`
record, §3.2). When the map names files and not one of them can be read,
the command is a usage error naming the first file and the directory it
looked in (WP-109): before, it judged every input I1.

With `--inputs-from <file>`, only the map's files whose canonical path
equals that of an entry in the response file are read; the others are I1.
The response file is parsed as GNU ld parses `@file`: words separated by
white space, with single and double quotes and backslash escapes. Words
starting with `-` are dropped, and so is the value of an option that takes
the next word (`-o`, `-m`, `-plugin`, `-dynamic-linker`, `-z`, ...). Each
`-l name`, `-l:file` and `--library=name` is resolved as the linker
resolves it (`spec/input.md` §4.2-4.4): through the `-L` and
`--library-path` directories of the whole line in order (`=D` and
`$SYSROOT D` under the last `--sysroot`), `libname.so` then `libname.a`
in each directory, only `libname.a` where `-static` or `-Bstatic` is in
effect (`-Bdynamic` and `--push-state`/`--pop-state` as the linker reads
them). A library found nowhere adds nothing (WP-036: before it, every
member of `libc.a` was I1 on a static hello).

A file of the response file, or a library found, that is neither an ELF
file nor an archive is read as a linker script (glibc's `libm.a` is
`GROUP ( libm-2.41.a libmvec.a )`; WP-109: before, its two archives were
I1 and every reference into them E4). It stands for the files of its
`INPUT` and `GROUP` commands, an `AS_NEEDED` list inside them included,
each a plain file name found as GNU ld finds it: an absolute name as it
is, any other in the current directory and then the library directories
in order; a name found nowhere adds nothing. `OUTPUT_FORMAT` names no
file and is passed over. Any other command, an `-l` name inside the
script, unbalanced parentheses or scripts nested more than 8 deep are a
usage error naming the script, not a guess.

The command prints the rendered findings and the coverage summary. It
exits 0 when the report is clean, 1 on any finding, and 2 on a usage error
or an unreadable output or map.

Tests: `xtask/src/plumb.rs s8_arguments`, `s8_response_files`,
`s8_libraries_are_resolved`, `s8_linker_scripts_are_followed`,
`s8_no_input_resolved_is_a_usage_error`.

```text
cargo xtask plumb --equiv <X.map> <F.map> <X> <F>
```

The equivalence check (§6.9; `spec/incr.md` IN-62; the verifier has no
binary of its own). Both maps are mortar maps; their files are read once,
from the paths they give (relative to the current directory), and a
file named by both maps is one input. It prints `X`'s findings, `F`'s
findings (each prefixed with its side), the P-E findings and one summary
line per side and for the pair. It exits 0 when both sides are clean
and no P-E finding was made, 1 on any finding, and 2 on a usage error:
a missing or extra argument, an option it does not take, an output or
map it cannot read, a map that is not a mortar map, or an `X` map that
claims no coverage (§3.2: a plain map given as `X`), or maps none of
whose files can be read from the current directory. Swapped arguments
(a map where an output is expected) are one of these. It never panics:
a truncated output is the side's findings (O1, S1) or exit 2.

Tests: `xtask/src/plumb.rs s8_equiv_arguments`; `tests/equiv.rs`.

### 8.1 Resources (WP-109)

The verifier judges every link mortar is asked to make, so it must fit
where the link fits. **The budget**: the verifier's peak resident set is
at most twice the linker's peak on the same link, and at most 4 GB on the
reference `-g` link (`cxx-large-g`, LLVM's clang with `-g`, linker peak
1,958 MiB); `--equiv` holds two outputs and two engines, and is held to
the same 4 GB. Time: a plain pass is linear in the records and bytes it
judges; `--equiv` costs about two plain passes and a comparison linear in
what it compares. Memory is saved by not holding data, never by skipping
a rule: a rule that cannot be run within the budget is named in the
coverage (§7.2), not dropped.

What is held, and for how long:

- the output, whole, for the run (every rule reads it);
- of every object the map names, its skeleton (WP-109b): the file
  header, the section header table, the section names, the symbol table
  with its strings and extended indexes, the groups and `.mason.meta`;
  an archive's member headers. A section's body is read through the
  caller's `Source` (the command's open files, by position) when a rule
  asks for it and kept only in a bounded cache (64 MiB of sections, the
  last two objects a phase read whole). An object whose skeleton
  `mortar_elf` finds not enough to read (a malformed file) is read
  whole, so the verdict is the whole bytes' in every case; a read that
  fails after the skeletons were read (the file changed under the
  verifier) is an I2 finding naming the file;
- per record, nothing after its verdict, except the members of a GOT
  group (§5.5) and of a piece group (§5.6) until the group is decided
  (§5.6: as early as its members are known);
- `--equiv`: both outputs whole, one skeleton store for both sides, and
  each side's traced form per record, keyed by input section (§6.9),
  its offsets in 32 bits when they fit, and its identity spans in a
  segment tree (each lookup logarithmic in the spans, linear in those
  that hold the address); the update's relocated ranges (the full
  link's are not read).

`MORTAR_PLUMB_PROFILE=1` (or `=<file>`) prints (or appends to the file,
line by line, so a run killed at a memory cap keeps them) the resident
set and its peak after each phase and the sizes of the large structures.
It is the measurement of this section, not a rule; it changes no verdict.

Measured (WP-109b, the dev-profile verifier as the bench and CI run it;
the linker's peak in brackets):

| link | plain: peak, time | `--equiv`: peak, time |
|---|---|---|
| protoc `-g` static (control) | 206 MiB, 1.0 s (251 MiB) | |
| clang split (`cxx-large-split`) | 1,285 MiB, 4.0 s (777 MiB) | 2,700 MiB, 27 s |
| opt `-g` (`cxx-large-g-opt`) | 823 MiB, 5.5 s (951 MiB) | 2,025 MiB, 32 s |
| clang `-g` (`cxx-large-g`) | 1,795 MiB, 14 s (1,960 MiB) | 4,307 MiB, 57 s |

The plain pass is within the budget on all four. `--equiv` is not on
clang `-g`: it holds both outputs whole (2 x 1,163 MiB) and, while the
full link's side runs, the update's kept forms, units and spans
(about 0.6 GiB) and the second engine. Its peak is about the two
outputs plus 0.9 to 2.0 GiB for the rest (931 MiB on opt `-g`, 1,478 on
split, 1,981 on clang `-g`): it passes 4 GiB at about 1 GiB of output
per side with an input set the size of clang `-g`'s. The remedy (the
outputs read by position too, each held whole only while its own side's
engine runs) is designed in `spec/landed/plumb/WP-109b.md` §5 and not
done. Times vary by some 30 % with the host's load.

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

The relocatable corpus (stage R; RL-5, WP-185) is §14.9's: GNU ld's
and lld's `-r` outputs verify clean but for the per-linker forms of
§14.5 (and lld's A91, A93, A100, A120, A122, A125, A126), mold's show
only their anomalies (A90-A102, A121-A124 as they apply), and P-P1
(RL-6, WP-186; `tests/compose.rs`) judges the D.3 components' final
links through GNU ld's and lld's `-r` by GNU ld, lld and mortar against
the direct links by the same linker, static (PIE: the lane's smoke
tests, `spec/harness.md`, "Relocatable outputs").

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
  Each must produce its own finding. The allocation records' rules
  (M6–M8, WP-096; `tests/maprec.rs`, and `tests/dyn.rs` for M8): on a
  link with the records stated as the output holds them (ACCEPTED, no M6,
  M7 or M8 finding), each field of each record changed (`<name>`,
  `<addr>`, `<size>`, `<align>`, and a record added twice) is REJECTED
  by its rule. So is each record left out (the padding chunk, each
  common, a copy) and each output symbol a `common` record is judged by
  changed (made local, made `SHN_ABS`, moved into `.data`, sized past
  `.bss`).
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
- **(p)** Holes and dead ranges (§6.8, `incr.md` IN-60; WP-091,
  `tests/dead.rs`). A GNU ld link is read as an incremental output: one
  input (`dead.o`) leaves the map, and the ranges it held become `hole`
  lines holding their fill (an `.eh_frame` one a padding CIE among
  `.eh_frame` chunks), or, in a debug program, neutral `.debug_info`,
  `.debug_abbrev`, `.debug_line` ranges and a tombstoned `.debug_frame`
  contribution. On AArch64, x86-64 and i386 (REL: the addends are in the
  words), DWARF 4 and 5:
  - ACCEPTED: the baseline, with `arr+40` (one past `arr`'s end, the
    first byte of the `.data` hole), the label `arr_end` there and
    gfortran's `blk_-8` inside the hole, a `.tdata` hole at the
    section's end and the dead string GNU ld merged after a live one as
    a `strings` range; the debug baseline;
  - REJECTED: a live relocation aimed into a hole (its section declared a
    hole: P-I3, and P-I1 covered twice); the same with the section out of
    the map (its symbol: P-I3); `e_entry` in the code hole (P-I3); a
    stale byte in a code and in a data hole (P-I5); a live unit's
    `DW_AT_stmt_list` aimed at a dead `.debug_line` range (P-I4); the
    dead `.debug_info` unit not neutralized (P-I6); a stray byte in the
    neutral `.debug_abbrev` (P-I6), a line program byte other than
    `0x06` (P-I6); the dead FDE's address left (P-I6); a 12-byte CIE and
    a zero word in the `.eh_frame` hole (P-I5); a 12-byte `.eh_frame`
    hole (P-I5); a `cie` range over an FDE (P-I6); a live FDE whose CIE
    pointer names the hole's padding CIE (P-I3); a live `.debug_frame`
    FDE naming the dead contribution's CIE (P-I4); in an i386 output made
    `ET_DYN`, a dead `.debug_line` left as written (P-I6, IN-53's PIE
    rule); a `.tdata` hole before a live word (P-I2); the dead string not
    ending with a NUL (P-I6); a hole in `.init` and
    a zero slot in the `__start_my_array` array (P-I2); the `.data` hole
    unlisted (P-I1); overlapping lines, a line past its section, a line
    naming no section, `cie`, `strings` or `neutral` in a section that
    has no such range, a hole in `.shstrtab`, a line in `.debug_aranges`
    (P-I2); a `strings` range of `.debug_str` not ending with a NUL
    (P-I6);
  - and every dead line of 8 sections × 5 kinds × 9 offsets × 9 sizes
    (hostile, 3,240 lines) is judged without a panic.
  - **PIE** (WP-092, `in3a_pie_tables_and_relocation_fill`; AArch64 and
    x86-64, GNU ld `-pie --no-dynamic-linker --eh-frame-hdr`): `dead.o`'s
    two `RELATIVE` records become a `.rela.dyn` hole of `RELATIVE`
    records aimed at one scratch word. ACCEPTED: that baseline, with
    `ptrs`' records for `arr+40` and `blk_-8` (judged by name) and
    `arr_end`'s GOT word at the data hole's first byte (a live end).
    REJECTED (P-I3): a `.got` word, a live `RELATIVE` target, a live
    record's `r_offset` and an `.eh_frame_hdr` location aimed into a
    hole; (P-I5) a hole record not `RELATIVE`, a `NONE` record inside
    `DT_RELACOUNT`'s prefix, the hole's records aimed at two words.
  - **Clang DWARF 5** (WP-092, `in3a_dwarf5_line_strings_and_str_offsets`;
    AArch64, x86-64, i386; clang-22 `-gdwarf-5`): ACCEPTED, the link with
    the claim and no range; REJECTED (P-I4), a `strings` range of
    `.debug_line_str` under a file name only the v5 line header names,
    and one of `.debug_str` under a name only a `.debug_str_offsets`
    entry names (`DW_FORM_strx1`).

- **(q)** Equivalence (§6.9, `incr.md` IN-2, IN-62; WP-092,
  `tests/equiv.rs`). `X` is a GNU ld link with `pad.o` (code, data) after
  `main.o`, `pad.o` out of the map and its ranges holes holding the fill
  (every unit of `a.o` and `b.o` at another address); `F` the link
  without it. AArch64, x86-64 and i386 unless named:
  - ACCEPTED: R13 case 1, `X` against `F`, and `X` against itself; a
    `--build-id` pair (the descriptors differ); a non-allocated
    section of units; a `merged` `.debug_str` with a `dead ... strings`
    line, and with the same string and no line (WP-268,
    `p_e6_merged_nonalloc_is_not_tiled`: not tiled; that dead line one
    byte short of its NUL is `X`'s P-I6); a PIE pair (AArch64, x86-64: `-pie
    --no-dynamic-linker --export-dynamic`); AArch64 `ADRP`+`ADD` in `F`,
    `NOP`+`ADR` in `X` (W1); x86-64 a GOT kept by `addq S@GOTPCREL`;
  - REJECTED: R13 case 2, `F` of `main.o b.o a.o` (P-E5, `myset` in
    another order); the one-byte control, a code byte no relocation
    covers flipped in `X` (P-E4); `e_flags`, a section's flags, a
    segment's flags, `e_entry` (P-E1); a unit in one map only, a size, a
    synthetic chunk in one map only (P-E2); a global naming another
    placed definition, a global renamed, a global made local (P-E3); the
    `.init` byte (P-E4); a note's type (P-E5); a stray byte between
    sections (P-E6); a non-allocated unit made shorter (P-E6, P-E2);
    AArch64 a GOT page no `LDR` pairs (`got-unpaired`, a record neither
    side checks: P-E4 1, also for a GOT group's member judged after its
    record); AArch64 an undefined weak TLS initial-exec load whose GOT
    entry, mortar's `A` on both sides (T1w), is made 8 in `X` (`X`'s R4;
    ACCEPTED before the mutation, `p_e4_undefined_weak_tls_initial_exec`); x86-64 `GOTPCRELX` as `mov $S` (X4, `F`, GNU ld's map) and
    as `lea` (X1, `X`) in an `ET_EXEC` output (P-E4 2); a `.got` word
    (P-E5); in the PIE pair a `.rela.dyn` addend, a `.dynamic` value, a
    `.dynstr` byte (P-E5) and a `.dynsym` entry's size (P-E3);
  - the forms that may differ, per output type
    (`equiv::tests::p_e4_forms_that_may_differ`: W1-W3, W6-W8 always,
    X1-X4 in `ET_DYN` only; W4, W5, W9-W13, GD to IE never; a record not
    checked never);
  - hostile (`hostile_pairs_never_panic`): `X` cut at 300 lengths and
    flipped at 300 bytes, either map's text cut at 100 lengths, the
    outputs swapped, a foreign `F`, an empty `F`: no panic, and a cut,
    swapped, foreign or empty side is never clean.
- **(r)** `.gdb_index` (§6.10; WP-098, `tests/gdb_index.rs`). Three
  hand-written objects, each a function, a DWARF 4 unit with a child DIE
  and GNU name sets whose names, attributes and order the test states
  (`main_a` and `Main_a` share a hash; `shared` and `T_a` occur in more
  than one unit; `fn_c` is static), linked by lld `--gdb-index` on
  AArch64, x86-64 and i386. lld's output judged as lld's has no P-G
  finding. The model of mortar's output is lld's with its index replaced
  (`objcopy --update-section`, alignment 4) by the bytes S-G7 gives for
  the sets, written by the test from synth.md, and the map read as
  mortar's with the chunk:
  - ACCEPTED: the model (no P-G finding);
  - REJECTED, each by the rule named, and the format rules' mutants also
    when judged as lld's: version 6 (P-G1); version 8 (P-G9); the CU
    list not at the header's end, the symbol table after the pool, the
    pool past the end, a non-empty TU list (P-G1); a CU entry's offset
    or length (P-G2); an address entry's `cu` out of range, `low ≥
    high` (P-G3, both), its `cu` another unit's, its range past or
    before the unit's (P-G3, mortar's bound); a slot emptied, a value of
    another kind, a static bit (P-G8); a name copied into an empty slot,
    a name or vector offset past the pool, the last name without its
    NUL (P-G4); a vector of count 0, a value's unit past the units, its
    reserved bits, kind 5 (P-G5); a name's case changed (same hash:
    P-G8); a vector's values swapped (P-G9 only: the sets are equal);
    the section's alignment 1, the chunk's size, no chunk (P-G6);
  - the silent `gdb-index-incomplete` (`p_g7_incomplete_index`): `c`
    assembled without its sets and the index built from `a`'s and `b`'s,
    everything else consistent: P-G7 only, naming unit 2; judged as
    lld's, no finding (the index is a valid one);
  - hostile (`p_g_hostile_index`, §9.4): each header word set to 0, 1,
    3, 23, 24, 25, the size − 1, the size, the size + 1, `2^31` and
    `2^32 − 1`; a 1000-slot table; a full table (P-G4); every byte of the
    header, CU list and area and 400 random bytes set to 0, `0xff` and
    flipped by bit 7: findings, no panic.
- **(s)** The input set (§3.8; WP-156, `tests/inset.rs`), on the checked-in
  links of AArch64 and x86-64 (`tests/inset/gen.sh`), mortar's map and
  GNU ld's: (1) a required member removed (`r.o libk.a` without
  `libk.a(k.o)`): M11 naming it and `k_fn`, for mortar's map in both
  orders of the line, for GNU ld's with `r.o` first; with the archive
  first GNU ld's demand is a back-reference, not judged and counted;
  (2) an unreferenced member added (`libk.a(j.o)`): M12, and for GNU ld
  also M12 for a member the preamble does not list; (3) a
  `--whole-archive` member dropped: M11 "live from the start" (mortar's
  map read with the option added to its `arg` records, since mortar at
  e1e60cf refuses it; GNU ld's map without `j.o`); (4) a member added
  that only a weak reference names: M12; (5) GNU ld's printed reason
  made false: M12 naming the map line. The two orders of an archive and
  the object referring to it give mortar the same set, both clean.
  §4.1's iterations (WP-231, `tests/comdat_iter.rs`, mortar's links of
  WP-200's cells `ext`, `after`, `member`, `copy`, `grp` and `dem`):
  (6) the member only the second iteration reaches removed: M11 naming
  it and `hx`, by wu.o's reference (the model before WP-231, which
  counted the dropped copy's `hx` as `first(hx)`, passed that map and
  called the member in the unmutated one M12); (7) a member added that
  only a demoted `STB_GLOBAL` mention names (`dem`): M12 naming the
  mention, its file and the group; (8) the unmutated links: clean, the
  coverage naming two iterations.

- **(t)** Relocatable outputs (§14; V-R12 of record
  `spec/landed/layout/WP-154.md`; RL-5, WP-185, `tests/relocatable.rs`; (21) RL-6, WP-186, `tests/compose.rs`),
  on GNU ld's `-r` output of the D.3 C and C++ components on each target
  (i686 for the REL mutations), read with mortar's map form where the
  mutation needs a mortar clause (as (l) reads lld's link as mortar's).
  Each mutation is caught by the rule named, and by nothing that is a
  recorded anomaly:
  (1) a relocation record's `r_offset` moved by 4: P-P3;
  (2) a section-symbol record's addend without its piece offset, in
  `r_addend` (x86-64, AArch64) and in the field (i686): P-P3;
  (3) a piece's bytes zeroed (dropped) and a piece's bytes written a
  second time over its neighbor: P-P2;
  (4) a second copy of a COMDAT group kept (its group, members and
  records appended from the losing input): P-P6 and P-P11 (P-P2 when
  the map places the losing copy) (the supervisor's ruling on RL-5's
  question 13);
  (5) a hidden global demoted to `STB_LOCAL` (moved below `sh_info`):
  P-P4;
  (6) a common's `st_shndx` set to `SHN_ABS` (mold's A90): P-P4;
  (7) a `.symver` name stripped to its base (A95's form): P-P4;
  (8) the FDE of a dropped copy kept (copied in with its records):
  P-P5;
  (9) an `R_*_NONE` record written at a live record's offset, read as
  mortar's: P-P3;
  (10) the container's bytes appended twice: P-P8;
  (11) an opcode bit flipped under `CALL26` and under
  `ADR_PREL_PG_HI21` (AArch64), under a `PC32` `call` (x86-64): P-P2;
  (12) a DWARF 5 `.debug_info` field of a record into a dropped copy
  written 0, read as mortar's (I-47's tombstone is 0xffffffff): P-P2;
  (13) a global's `st_value` without its piece offset: P-P4;
  (14) a local `$d` moved to a removed FDE's offset, read as mortar's:
  P-P4;
  (15) on a `--unique` output, a record moved into the other
  `.rela.text`: P-P3;
  (16) a `SHF_LINK_ORDER` section's `sh_link` set to 0: P-P11;
  (17) `SHF_GNU_RETAIN` cleared on a section holding a retained piece:
  P-P11;
  (18) a type-unit group signature's `st_shndx` pointed at a member:
  P-P4;
  (19) the `.note.GNU-stack` flags cleared over an X input: P-P10;
  (20) `e_type` set to `ET_EXEC`, `e_phnum` to 1, `EI_OSABI` flipped:
  P-P9 and P-P7;
  (21) for P-P1, the final link `V` of the D.3 C component through GNU
  ld's `-r` output, `D` its direct link: a `.data` pointer's word in `V`
  increased by 8: P-P1 item 3; a global's `.symtab` value in `V` moved by
  4: item 4; two pieces of `V`'s `.text` swapped (their bytes and
  records): item 2. (`V` is also wrong for stages 1-3; P-P1 must name
  the composition all the same.)
  (22) an input `R_*_NONE` record dropped from the `-r` output (and its
  relocation section removed when it held only that record): P-P3.
  Each mutation's finding names its rule; (11), (12) and (17) are the
  review's x-cases (record §H).

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
otherwise catch everything), by `buildid::reseal`, the function the byte
sweep uses too (§12.3, WP-101): a map in mortar's format only, so a
reference's id is never rewritten. Places that carry a baseline finding are
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
  alignment (but `PT_GNU_STACK`'s `PF_X`, P-D4 in stage 3 and S10 in a
  static output of mortar's, and
  `PT_NOTE`'s and `PT_GNU_PROPERTY`'s `p_align`, S11; `PT_DYNAMIC`'s
  `PF_W` is read by glibc since 2.35 as "`.dynamic` is read-only", which
  only changes whether ld.so relocates its pointers in place or adds the
  bias on each read: a GNU ld AArch64 PIE with it cleared runs, and gdb
  lists its libraries, WP-088); `PT_LOAD` `p_align` doubled (the
  congruence still holds or S2 finds it; a bias aligned more) or halved
  below the page (kernel 6.8 and glibc 2.41's ld.so, run directly and
  as `ld.so ./prog`, both start a GNU ld AArch64 PIE whose `PT_LOAD`s
  have `p_align` 0x800, WP-088); `PT_GNU_STACK`'s and
  `PT_GNU_RELRO`'s `p_filesz`, `PT_GNU_STACK`'s `p_memsz`.
- **Holes, cheap, closed in WP-028d:** the W1 NOP-ADRP form (§5.3), S5
  (flags), S7 (entry), S9 (TLS), S11 (notes), S13 (`PT_PHDR`), S20
  (RELRO), S21 (`PT_GNU_EH_FRAME`), and Y4's mapping symbols (no `$x`
  or `$d` was judged before).
- **Holes, by cost:**
  1. `PT_GNU_STACK` `PF_X` flipped (the stack made executable): judged
     in stage 3 (P-D4, WP-088) and, since WP-102, in a static output of
     mortar's map from its line (S10, §3.2a).
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

**Holes and dead ranges (WP-091, §6.8).** Each check of `src/dead.rs`
and `src/deaddwarf.rs` was dropped in a test build, one at a time, and
`tests/dead.rs` run (AArch64, x86-64, i386; DWARF 4 and 5). "Caught by"
is the test that failed; 30 of 37 drops are caught.

| check dropped | caught by |
|---|---|
| coverage (P-I1) | `in3a_live_relocation_naming_a_hole_is_rejected`, `in60_unlisted_hole_and_bad_lines_are_rejected` |
| resolution (P-I2: no section, outside, overlap) | `in60_unlisted_hole_and_bad_lines_are_rejected` |
| admission, whole (P-I2) | `in30_…`, `in3a_debug_mutations_are_rejected`, `in60_unlisted_…` |
| `.debug_aranges` takes no line | `in3a_debug_mutations_are_rejected` |
| a hole in a non-allocated section | `in60_unlisted_…` |
| packed sections (L-142) | `in30_holes_in_packed_sections_are_rejected` |
| thread-local holes at the end only (L-143) | `in60_unlisted_…` |
| `cie` outside `.eh_frame` | `in60_unlisted_…` |
| `strings` in a section without merged strings | `in60_unlisted_…` |
| `neutral` outside IN-51b's sections | `in60_unlisted_…` |
| `contribution` outside IN-53's list | `in3a_debug_mutations_are_rejected` |
| fills, whole (P-I5, allocated P-I6) | `in34a_eh_frame_holes_are_padding_cies`, `in3_stale_bytes_in_holes_are_rejected` |
| the fill byte | `in3_stale_bytes_in_holes_are_rejected` |
| the padding CIE (IN-34a) | `in34a_…` |
| an allocated `strings` range ends with a NUL | `in60_unlisted_…` |
| a `cie` range is whole CIEs | `in34a_…` |
| symbols (P-I3) | `in3a_live_relocation_…` |
| relocations of live sections (P-I3) | `in3a_live_relocation_…` |
| tables, whole; `e_entry` (P-I3) | `in3a_live_relocation_…` |
| live `.eh_frame` FDEs' CIE pointers (P-I3) | `in34a_…` |
| N3: input end labels accepted at a live end | `in60_baseline_and_n3_references_are_accepted` (the baseline rejected) |
| neutral debug forms; `.debug_abbrev`'s (P-I6) | `in3a_debug_mutations_are_rejected` |
| contributions; the `.debug_frame` tombstone; the PIE rule (P-I6) | `in3a_debug_…` |
| a non-allocated `strings` range ends with a NUL | `in3a_debug_…` |
| the walk of live units (P-I4) | `in3a_debug_…` |
| live `.debug_frame` FDEs' CIE pointers (P-I4) | `in3a_debug_…` |

**Survivors (7), with the reason** (WP-091; all caught since WP-092,
below):

1. The relocation-section fill (`DT_RELACOUNT` prefix, `NONE` after
   it), the `.got` words, allocated relocation records' `r_offset` and
   `RELATIVE`/`IRELATIVE` targets, and `.eh_frame_hdr` entries: the
   models are static executables without `--eh-frame-hdr`, so none of
   these tables exists. Closing them needs a PIE model (a hole whose
   `.rela.dyn` records are rewritten to the scratch word, a GOT word and
   a header entry aimed into it): moderate, the first thing to add once
   WP-062/WP-064 write lines into PIE outputs, whose maps can then be
   used directly.
2. Table references at a live item's end accepted (the N3 rule for
   unnamed words): no model has a table word at a hole's start, so
   dropping the acceptance makes nothing fail; it only loosens, and
   the corpus (§6.8) has no dead range yet to show it is needed.
3. The `strp`/`line_strp` entries of v5 line headers: no case puts a
   dead range under a string only a line header names. In GCC's output
   the header's directory and file names are also the unit's
   `DW_AT_comp_dir`/`DW_AT_name` strings (merged), so such a case would
   be caught by the DIE walk first (suspected, not separated); a case
   with a header-only file name would close it.
4. Live `.debug_str_offsets` entries: GCC does not use `DW_FORM_strx`
   without split DWARF; needs a Clang DWARF 5 model (cheap).

**Re-run with WP-092's two models** (§9.2 (p), PIE and Clang DWARF 5):
the seven, with the relocation records split into where they write and
what a `RELATIVE`/`IRELATIVE` record targets, eight drops, **all caught**:

| check dropped | caught by |
|---|---|
| the relocation-section fill (`rel_fill`) | `in3a_pie_tables_and_relocation_fill` |
| the `.got` words | `in3a_pie_…` |
| an allocated record that writes in a range | `in3a_pie_…` |
| a `RELATIVE`/`IRELATIVE` target in a range | `in3a_pie_…` |
| `.eh_frame_hdr` entries | `in3a_pie_…` |
| table words at a live item's end (N3) | `in3a_pie_…` (the baseline: `arr_end`'s GOT word) |
| v5 line-header strings | `in3a_dwarf5_line_strings_and_str_offsets` |
| `.debug_str_offsets` entries | `in3a_dwarf5_…` |

The PIE model found one false finding of the table walk, now ruled
(§6.8 P-I3, tables): a `RELATIVE` record at a live relocation's place
carries an address that may lie in a hole by an addend (`blk_-8`); it
is judged by the input record's name.


**The equivalence check** (WP-092, §6.9): 25 mutations of
`src/equiv.rs` (a condition made `false`, a clause or a finding
dropped), each run against `tests/equiv.rs` and the unit test; caught
23 (the `.dynsym` name and `st_info` clauses and the `.eh_frame_hdr`
entries survived). WP-092b re-ran them (four patterns re-cut after
`rustfmt`) with six more, over `src/equiv.rs` and `src/reloc.rs`, against
`tests/equiv.rs`, the unit test and `tests/tls.rs
s4_7_undefined_weak_tls_initial_exec_is_judged`: 31 mutations, caught
29. The two former survivors are caught (`p_e5_dynamic_tables` mutates
a `.dynsym` entry's `st_info` and `st_name`; `p_e5_eh_frame_hdr`, an
AArch64 GNU ld pair with `--eh-frame-hdr`, a table entry). Survivors:

- the locals of a thunk or patch passed over (P-E3): no test pair has a
  thunk on one side only; the real AArch64 pair of WP-092's record §6
  (two 63.5 MiB sections, a veneer in `X` only) fails without it;
- a GOT group's member's outcome traced after its record (P-E4): an
  equivalent mutant today, since `finish_got`'s outcomes other than the
  traced `got-unpaired` are W2/W3 (which may differ) and findings
  (which the side reports); a deferred outcome that must not differ
  would catch it.

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

Stage 3 (WP-050, from the dynamic link tests of WP-051, `tests/link/dyn`;
each declared by the tests that show it). A20-A27 are wrong outputs and
reference behaviours `spec/dyn.md` already names; A28 and A29 are new.

- **A20 (lld, mold).** H1: `IRELATIVE` records in `.rela.dyn`, applied
  before `.rela.plt`'s `JUMP_SLOT`s; a resolver that calls through the
  PLT jumps to an unbound slot (P-D10 "(D-41, H1)").
- **A21 (GNU ld).** W2: an alias of a copied variable (`_environ` of
  `__environ`) is not defined at the copy and not exported (P-D11
  "(D-21, W2)", P-D7 "(W2)").
- **A22 (GNU ld).** W10: with no `-dynamic-linker`, its built-in
  interpreter (right on AArch64, `/lib/ld64.so.1` on x86-64) (P-D4 "a
  linker default (D-31, W10)").
- **A23 (lld, mold).** W10: with no `-dynamic-linker`, no `PT_INTERP`
  (P-D1 and P-D4 ending "(D-31, W10)").
- **A24 (GNU ld, mold).** W6's form on AArch64 and x86-64: a record in
  read-only memory and `DT_TEXTREL` where D-47 refuses (an address-sized
  word in `.rodata`; WP-051's finding 5) (P-D9 "a text relocation (D-47,
  W6)", P-D6 "(D-47, W6)").
- **A25 (lld).** In a `-no-pie` executable lld exports an undefined weak
  key as `WEAK UND` and writes no record for its absolute uses, so a
  preloaded definition never reaches them (D-25: no reference writes one
  in `-no-pie`; W12's no-record form, without a split): P-D9 view 3
  ending "no dynamic record names it (D-25, W12)", on an `ET_EXEC`.
- **A26 (lld, mold).** D-11 item 3 (E-shlib 2.3b, review N5): an
  under-linked library's provider is left out of `DT_NEEDED`; the load
  fails (P-D6 "which D-11 item 3 makes needed").
- **A27 (GNU ld, lld, mold).** W1: a reference bound to a `STV_HIDDEN`
  `.dynsym` entry; the output fails to load (P-D6, P-D7 or P-D9 ending
  "(only a STV_HIDDEN entry: W1)").
- **A28 (mold, new).** A `-fno-pic` CIE's absolute personality
  (`R_X86_64_32` against `__gxx_personality_v0`) holds a `.plt` entry
  while the `.dynsym` entry has `st_value` 0: the routine has two
  addresses (E3, "differs in its personality routine", the value found
  in `.plt`). Harmless to unwinding, which only calls it. Reproducer:
  `tests/link/dyn/reloc/copy-typeinfo-relro.test`, x86-64.
- **A29 (lld, new).** `.eh_frame_hdr`'s table sorted by unsigned offset
  from the table: an FDE whose function lies below it (an x86-64
  `.ltext` placed first, Clang `-mcmodel=large`) comes last, where the
  unwinder's binary search cannot find it (E5 "not sorted", at an
  address below `.eh_frame_hdr`). Reproducer:
  `tests/link/dyn/reloc/weak-class-table.test`, x86-64 `exec` (lld 22).

Static-pie (WP-089, §12.4; each shown by running a program, the cases of
`crates/mortar_plumb/tests/spie.rs`, declared by
`tests/link/dyn/kinds/output-kind-table-static-pie.test`):

- **A30 (GNU ld, AArch64, new).** The GOT slot of an absolute symbol
  (`SHN_ABS`; glibc's `_nl_current_*_used`, value 2, in every static-pie
  with the C library) gets a `RELATIVE` record: the program reads the
  value plus the load bias (P-D9 view 2 ending "(a record moves a value
  that is not an address of the executable)"). The same slot in a `-pie`
  output does too. glibc only tests those words for non-zero, so its own
  are harmless; a program's absolute symbol read through the GOT is wrong
  (`tests/spie/absgot.c` exits 1). lld and mold write 2 and no record.
- **A31 (GNU ld, x86-64, new).** The `.plt.got` entry of an undefined
  weak function (crtbeginS's `__cxa_finalize`, g++'s `_ZTH` init guard of
  a `thread_local` with a constant initializer) jumps through its GOT slot
  plus 1, a misaligned word no record writes (P-D10 "jumps through" an
  address not a multiple of 8, and R1 on each `R_X86_64_PLT32` call whose
  rel32 reaches such an entry). Dead code: each caller tests the slot
  itself for 0 first (`cmpq $0, sym@GOTPCREL(%rip)`; `je`).
- **A32 (GNU ld, mold, x86-64).** W3 in a static-pie: an address word of
  an IFUNC is its own `IRELATIVE` slot (the resolved function) while a
  PC-relative use takes the IPLT entry, so `fp == f` is false (F3 "takes
  the function its IRELATIVE record resolves to"; `tests/spie/ifunc.c`
  prints 0). lld makes every use the IPLT entry; on AArch64 every use of
  all three is the resolved function (a GOT load).
- **A33 (mold).** A shared object named on a `-static-pie` line (after
  the driver's `-Bstatic`) is linked: the output has `DT_NEEDED` and no
  `PT_INTERP`, nothing loads the library, and the call to it faults
  (WP-052's `dyn/spie/shlib-refused`, both architectures: SIGSEGV). GNU
  ld and lld refuse it, as D-19 and D-2 do; stage 3 stops at P-D2 "a
  shared object after -Bstatic (dyn.md D-19)".
- **A34 (GNU ld).** A static-pie linked with `--export-dynamic` (WP-088,
  `tests/spie.rs` case `export`): the absolute symbols it exports (glibc's
  `_nl_current_*_used`, value 2) get `RELATIVE` records on the words that
  hold them, input words (`setlocale.o`'s `.data.rel.ro`) as well as GOT
  slots (P-D9, view 2 and §12.4's non-address rule; both machines), and
  on x86-64 the GOT slots of exported TLS variables get `TPOFF64`
  records that glibc's self-relocation applies before the thread pointer
  exists (P-D14): the program faults at start-up (SIGSEGV, WP-056 F4).
  lld and mold write neither.
  A30 and A34 are matched on a static-pie or a `-pie` output (`DF_1_PIE`
  in `DT_FLAGS_1`) alone since WP-166: a GNU ld shared object's GOT slot
  of `_end` was matched as A34 before, the one-past-the-end value P-D9
  now accepts.
- **A36 (lld, mold).** dyn.md W16: a used shared object's strong
  reference that no member of N ∪ U satisfies is linked, the library that
  defines it is left out of `DT_NEEDED` (it is not needed by D-11), and
  the program fails at load (`symbol lookup error`, exit 127): P-D6
  ending "(D-13: the link is refused)" (WP-088). Reproducer:
  `tests/link/dyn/shlib/used-underlinks-refused.test`; `tests/dyn.rs`
  case `W16`. mold's form of the same refusal for `--exclude-libs`
  (dyn.md D-110 item 1; WP-161): a needed object's strong reference whose
  only definition is the program's, in an archive member the line
  excludes, is linked with no export, and the program fails at load
  (`undefined symbol`); P-D6 names the exclusion and ends the same way.
  Reproducer: `tests/link/dyn/dynsym/exclude-libs-exe.test` (`bad-mold`).
- **A35 (mold, x86-64, new).** A `call sym@PLT` (`R_X86_64_PLT32`) to
  an absolute symbol (`SHN_ABS`) in a `-pie` or `-static-pie` output is
  written as a PC-relative `call` to the symbol's value, right at load
  bias 0 only (R1 "a PC-relative field to an absolute symbol in a
  position-independent output", §5.1a). GNU ld, lld and mortar refuse the
  link; mold refuses the `PC32`/`PC64` data forms. Reproducer:
  `tests/dyn.rs` case `abspc`.

A37–A43 are the references' conventions for a non-alloc word against a
name only a shared library defines (§5.8 "A target only a shared
library defines", WP-104; measured in §10.6). Nothing relocates such a
word at run time, so none of them is a mislink of the program; each
gives a debugger a value that is not the symbol's address (§10.6). Each
is accepted from its linker only, never from mortar's map.

- **A37 (GNU ld; x86-64, and AArch64 `-no-pie`).** A word against a
  function or an IFUNC the library defines, for which the output has an
  ordinary PLT entry (a PIE's, or a non-PIE's for a function the program
  only calls), holds that entry's address + A (PC-relative: − P): R1
  ending "; it holds the library's PLT entry". In an AArch64 or i386 PIE
  GNU ld writes 0 + A. Reproducer: `tests/link/dyn/reloc/debug-word-shared.test`.
- **A38 (lld).** An absolute word against a name the library defines
  holds 0: the addend dropped (S + A with S = 0 would be A). R1 ending
  "; it holds 0, the addend dropped"; with A = 0 lld's word is the
  rule's. The same for a DTP-offset word against a library's TLS
  symbol (`R_X86_64_DTPOFF64`/`DTPOFF32`, with or without a `PT_TLS`).
  lld's PC-relative words are the rule's (A − P).
- **A39 (mold).** An absolute word (and, on i386, a PC-relative one,
  where mold also ignores P) against a data object the library defines
  holds the library's `st_value` + A: an address in the library's own
  numbering, of nothing in the program. R1 ending "; it holds the
  library's st_value" (the RB-1 defect's finding on mold's outputs,
  `spec/landed/harness/WP-100.md`). mold refuses a PC-relative
  non-alloc word against an import on AArch64 and x86-64 ("relocation
  ... cannot be used against symbol"), so those links have no output.
- **A40 (mold).** A word against a function or an IFUNC the library
  defines, for which the output has an ordinary PLT entry, holds that
  entry's address + A: R1 ending "; it holds the library's PLT entry".
- **A41 (lld, mold).** An absolute word against a library's `STT_TLS`
  symbol is linked (lld: 0; mold: `st_value` + A), where GNU ld ("bad
  value") and mortar refuse it by name: P-D12 "a non-TLS data word in a
  non-alloc section against '...', whose shared definition is STT_TLS
  (refused by name: ...)".
- **A42 (GNU ld, mold).** A DTP-offset word against a library's TLS
  symbol, in an output with a `PT_TLS` of its own, holds A − the
  `PT_TLS` segment's `p_vaddr` (the reference computes the offset as if
  the symbol were the executable's, at address 0); without a `PT_TLS`
  both write 0 + A, the rule's. R1 ending "; it holds A − the PT_TLS
  address".
- **A43 (lld, AArch64).** A PC-relative non-alloc word against an
  undefined weak no object defines holds A − P (S = 0), where §4.4 makes
  S the place on AArch64 (X = A, as GNU ld and mortar write): R1 ending
  "; it holds A − P, S = 0 for an undefined weak (spec/plumb.md §4.4,
  §5.8)". On x86 all four write A − P (S = 0 there).
- **A44 (lld; WP-131 V2).** Under `-Bsymbolic` lld sets `DF_SYMBOLIC`
  in an executable's `DT_FLAGS`; GNU ld and mold write nothing, and
  D-30 gives mortar no such bit. The loader ignores the flag in an
  executable (its references bind locally already), so the program is
  the same. P-D6 with the message "DT_FLAGS has DF_SYMBOLIC under
  -Bsymbolic in an executable, where D-30 writes none", which stage 3
  splits from D-30's "DT_FLAGS … has bits …" for a reference whose line
  says `-Bsymbolic` last of its family (other bits keep D-30's
  message). Accepted from lld only; mortar's rule stays D-30's. A
  shared object's `DF_SYMBOLIC` (WP-117, WP-121) is not this anomaly.
- **A45 (GNU ld, lld, mold; WP-122).** An executable takes a `COPY` of
  writable data that its library binds to itself (`-Bsymbolic`, or a
  `NONPREEMPTIBLE` row): the program and the library use two objects
  (`spec/dyn.md` D-43a, W30), which mortar refuses. P-D24 ending "(D-43a,
  W30)". Reproducer: `tests/link/dyn/shlib/copy-of-symbolic.test`.
- **A46 (GNU ld, AArch64; WP-144).** In a shared object GNU ld writes
  one spare `R_AARCH64_NONE` record, every field 0, per local
  general-dynamic pair (a hidden or static thread-local, traditional
  dialect: its `DTPMOD64` has `r_sym` 0 and its DTP offset word is a
  constant): it reserved a `DTPREL64` record and did not need it. They
  sit in `.rela.dyn` right after the `RELATIVE` records, at index
  `DT_RELACOUNT` onwards and counted in `DT_RELASZ` (`.rela.got` under
  `-z nocombreloc`, where no `DT_RELACOUNT` is written); glibc skips
  type 0. Measured: 1, 2, 3 and 5 locals give 1, 2, 3 and 5 records,
  the same with `-z now` or two functions using one local; none for
  the TLSDESC or initial-exec dialects, none from mold or lld (lld
  takes no traditional dialect), none from any reference on x86-64 or
  i386. P-D25 with the message "a spare R_AARCH64_NONE record ...
  (A46)" (P-D25's shape); a type-0 record with a non-zero field, past
  that count, inside `DT_RELACOUNT`'s prefix or in mortar's output is
  P-D25's plain finding. Reproducer:
  `tests/link/dyn/shared/tls-shared-gd.test`. (GNU ld's `-nostdlib`
  AArch64 PIE that leaves `__tls_get_addr` undefined writes one in
  `.rela.plt` for the PLT entry it dropped; not this anomaly.)

- **A47 (lld, mold; WP-163).** A root (`-u K`, an implicit script's
  `EXTERN(K)`) that no input references and only a needed shared library
  defines gets no `.dynsym` entry: dyn.md D-21 item 1 imports it, as GNU
  ld and mortar do. Nothing at load depends on the entry (no reference
  binds through it); a `dlsym` of the program for K finds the library's
  definition either way. P-D7 with the message "... (D-21 item 1: a
  root is imported)" (P-D7's shape); in mortar's output it is P-D7's
  plain finding. Reproducers: `tests/link/dyn/shared/root-as-needed-extern.test`
  (lld's `extnoas-*`), `tests/shared.rs` case `roots`.
- **A48 (mold; WP-163).** mold makes an `--as-needed` library needed when
  only a root (`-u`, `-e`) names a definition of it, where D-11 item 2
  (GNU ld, lld, mortar) does not: an extra `DT_NEEDED` entry, the
  library loaded for nothing. P-D6 with the message "... (only a root
  names a definition of it: D-11 item 2)"; in mortar's output it is
  P-D6's plain finding. Reproducers:
  `tests/link/dyn/shared/root-as-needed-unneeded.test` (`umold-*`),
  `tests/shared.rs` case `roots`.
- **A49 (mold; batch 9).** mold extracts an archive member for a
  command-line shared object's strong versioned undefined reference
  (`name@V`) by the member's plain `name`, `--as-needed` or not
  (D-11's rule aside): libstdc++.so's
  `__udivti3@GCC_3.0` extracts `libgcc.a(_udivdi3.o)` under
  `-static-libgcc`. Resolve §9.2 gives the reference the key `name@V`,
  which a plain `name` does not define: GNU ld, lld and mortar do not
  extract. M12 (§3.8) with the message "... is in the link and only a
  shared object's versioned reference reaches it, by its plain name
  (resolve §9.2: not an edge)"; in mortar's, GNU ld's or lld's output it
  is the same M12, not accepted, and M11 is never excused by it (mold
  that does not extract is judged as GNU ld). Reproducers:
  `tests/link/dyn/cxx/threads-tls.test` (`pie-sgcc`),
  `tests/inset_ver.rs`; an `--as-needed` library's versioned reference
  (dyn.md D-12 cell 4b): `dyn/shlib/as-needed-extract-versioned.test`,
  `tests/inset_stages.rs` (`vr`).
- **A70 (lld; WP-171).** lld extracts an archive member for the strong
  references of an `--as-needed` command-line shared object that is not
  in N (dyn.md D-12: an unneeded one, cells 1c and 3c; a used one, D-13's
  cell), where GNU ld, mold and mortar extract nothing: M12 (§3.8) "...
  only the references of an --as-needed shared object the output does
  not need reach it (dyn.md D-12: not an edge)", and what the member then
  needs (3c: libu.so, and its `u2`) P-D6 or P-D7 ending "(only an object
  no stage of D-12 extracts makes it needed)". On mortar's, GNU ld's or
  mold's output the same findings are not accepted. Reproducers:
  `dyn/shlib/as-needed-extract.test` (`unneeded`),
  `-stages.test` (`self`), `-refused.test` (`used`);
  `tests/inset_stages.rs` (`r1c`).
- **A71 (lld; WP-171).** lld drops an `--as-needed` library that D-11
  makes needed at stage 1 when a later stage's member defines the keys it
  gave (dyn.md D-12 row 2a: N only grows; GNU ld and mold keep it, the
  program runs alike from the member's definition): P-D6 "DT_NEEDED
  lacks '...', which D-11 makes needed and D-12 keeps needed (row 2a: N
  only grows; a later stage's member defines what it gave)". Reproducers:
  `dyn/shlib/as-needed-extract-kept.test`, `tests/inset_stages.rs`
  (`k2a`).

- **A55 (mold; WP-165).** An undefined weak key reached by branches
  (alone, or beside an unfillable use in `-no-pie`) gets no `.dynsym`
  entry: the call goes to 0 (x86-64) or is skipped (AArch64 `bl .+4`)
  whatever a later library or `LD_PRELOAD` defines, where GNU ld and lld
  import it (dyn.md D-25's branch cells, §13.1). P-D7 with the message
  "... (D-25: WeakDyn)" or "... (D-25 ruling (A): WeakDyn for its
  branches)" (P-D7's shape); in mortar's output it is P-D7's plain
  finding. Reproducers: `tests/link/dyn/reloc/weak-branch-*.test`,
  `weak-class-table.test` (`k_branch`), the `-static-libstdc++` links of
  `dyn/census/gxx-static-libstdcxx.test` and `dyn/cxx/threads-tls.test`
  (seven `_ITM_*`/`_ZGTt*` keys).
- **A56 (GNU ld, x86-64; WP-165).** In `-no-pie`, the split key (a
  branch and an unfillable use, no GOT use, no writable absolute word)
  gets no `.dynsym` entry (GNU ld's `-z nodynamic-undefined-weak` form):
  the call goes to 0 under `LD_PRELOAD` too. dyn.md §13.1's named
  difference: a program that works under it never executes the call
  while the key is undefined. P-D7 with the message "... (D-25 ruling
  (A): WeakDyn for its branches)", on an `EM_X86_64` output of GNU ld
  only. Reproducers: `tests/link/dyn/reloc/weak-branch-split.test`,
  `tests/weak.rs` (`split-x64.bfd`).
- **A57 (mold; WP-166).** mold exports none of D-21 item 2's seven
  linker-defined names (dyn.md D-110 item 2, D-21 item 2): its library
  binds a referenced `_end` to its own (the interposer's view of P-D17
  fails: a program's `_end` does not reach it), and its executable
  exports none that a needed library references or defines, so the
  library's `_end` is its own (a `char _end[1]` library's `api_end()`
  is not the program's `_end`). P-D19 "... is in the export set and not
  defined in .dynsym (D-110)", P-D7 "... is defined by the program and
  referenced by a needed or used object, and is not exported" or "...
  an object references it and the line's -E or --dynamic-list exports
  it, and it is not exported", or P-D17 "view B (an interposer defines
  'K'", K one of the seven, on mold's output only. Reproducers:
  `tests/link/dyn/shared/linker-defined-export.test` (xfail: mortar then
  hosts no mold library), `dyn/dynsym/linker-defined-export-exec`,
  `dyn/dynsym/linker-defined-def-exec`; `tests/ldef.rs`.
- **A58 (GNU ld; WP-166).** GNU ld's shared object exports `__etext`
  and, on AArch64, `__end__`, `__bss_start__`, `__bss_end__`,
  `_bss_end__` that an object references, preemptible, where D-110 item
  2 exports the seven alone (mortar and lld keep them unexported): P-D19
  ") is in .dynsym and not in the export set", P-D25 "... which the line
  binds (D-112)" or P-D7 "... which does not hold it", on one of those
  names, in a GNU ld output without `DF_1_PIE`. Reproducer:
  `tests/ldef.rs` (`shx-*.bfd`).
- **A59 (GNU ld; WP-166).** An executable against a library that
  defines the seven (`char _end[1]`): GNU ld defines the program's own
  `_end`, `_edata`, `__bss_start` with the library's type and size
  (`STT_OBJECT`, size 1, so one byte past the end of its section): the
  value is the program's (the runs agree), the size is wrong. The
  structure finding "... is outside its section" on one of the seven
  that `.dynsym` defines as `STT_OBJECT`. Reproducers:
  `dyn/dynsym/linker-defined-def-exec`; `tests/ldef.rs` (`exdef*`).
- **A60 (GNU ld; WP-166).** Under `-E` GNU ld's executable exports
  `_edata`, `__bss_start` and `_end` though no object and no library
  references them (D-21 item 2: `-E` with no reference exports none;
  §13.1's named difference, harmless: nothing looks them up). P-D7
  "'K' is exported under -E, and no needed library ...", K one of the
  seven, on GNU ld's output. Reproducer:
  `dyn/dynsym/linker-defined-export-exec`.
- **A61 (lld; WP-169).** Under a version script's `local: *` lld keeps
  `__start_SEC` and `__stop_SEC` exported `GLOBAL PROTECTED` (in
  `.dynsym` and `.symtab`), where dyn.md D-114 makes them `STB_LOCAL`
  and unexported (GNU ld, mold and mortar). Harmless to the library
  (`PROTECTED`: its own uses bind to its own section), but a program or
  library that defines or looks up the name sees lld's. P-D19
  "'__start_SEC' (...) is in .dynsym and not in the export set (a
  version script or --exclude-libs makes it local)" on lld's output.
  Reproducers: `dyn/version/symtab-linker-defined-local`;
  `tests/symloc.rs` (`vl-*.lld`).

- **A80 (lld; WP-170).** Under `--wrap=SYM` lld binds a reference in
  `SYM`'s own defining object to `__wrap_SYM` (resolve.md §7.1 E13: a
  defined mention is not wrapped; GNU ld, mold and mortar call `SYM`).
  The program differs: `g=1010` for `g=10` in `resolve/wrap.test`; in a
  shared object the call is to `__wrap_SYM`'s PLT entry, not `SYM`'s
  (preemptible). R1 (or an overflow, and stage 3's view B P-D17) whose
  text ends "(a reference in SYM's defining object bound to __wrap_SYM;
  resolve.md §7.1 E13: not wrapped)": the field is what binding to
  `__wrap_SYM` gives, directly or through a PLT entry whose word holds
  it. Reproducers: `resolve/wrap.test`, `resolve/wrap-kinds.test`,
  `resolve/wrap-refused.test`, `dyn/shared/wrap-shared.test`;
  `tests/wrap.rs` (`st-*.lld`).
- **A81 (lld; WP-170).** Under `--wrap=SYM` lld writes a weak `SYM`
  `STB_GLOBAL` in `.symtab` when a strong undefined `__real_SYM` binds
  to it (glibc's weak `open` under `--wrap=open`). Harmless in an
  executable. Y3 "symbol 'SYM' has another binding than its definitions
  (a --wrap word whose strong __real_ reference binds to its weak
  definition)". Reproducer: `resolve/wrap-cxx.test`.
- **A82 (lld; WP-170).** Under `--wrap=ENTRY` lld's entry point is
  `__wrap_ENTRY` (resolve.md §7.1: a root is not redirected;
  `spec/layout.md` L-102). P-D4 "the entry point is not 'ENTRY' (it is
  __wrap_ENTRY: a root is not redirected, resolve.md §7.1)".
  Reproducers: `resolve/wrap-kinds.test` (`entry`); `tests/wrap.rs`
  (`dm-*.lld`).
- **A83 (lld; WP-170).** lld extracts an archive member that defines
  `SYM` though only references `--wrap=SYM` binds to `__wrap_SYM` name
  it (it resolves before it wraps; resolve.md §7.1, §16; the programs run
  alike). M12 "... is in the link and no root or reference of the link
  reaches it (§3.8) (it defines a --wrap word SYM, and only references
  bound to __wrap_SYM name SYM)". Reproducers: `resolve/wrap.test`
  (`wrap-member`); `tests/wrap.rs` (`mem-*.lld`).
- **A84 (lld; WP-170).** Under `--wrap=SYM` lld leaves an input's
  definition of `__real_SYM` out of `.symtab` (GNU ld and mold keep
  it). Y3 "symbol '__real_SYM', defined in ..., is not in the output's
  .symtab (a definition of __real_SYM under --wrap=SYM)". Reproducer:
  `resolve/wrap.test` (`chain`).

- **A85 (GNU ld; WP-170).** GNU ld writes `--defsym=SYM=B+N` (or
  `B-N`) as an `SHN_ABS` symbol with `B`'s type, not in `B`'s section as
  `STT_NOTYPE` (resolve.md §7.2, §16); in a PIE it gives a word holding
  `SYM`'s address no `RELATIVE` record, so the program reads `B+N`
  without the load bias (the bytes are wrong at any bias but 0). Y3
  "... (a --defsym B+N or B-N written as GNU ld writes it: SHN_ABS, B's
  type; resolve.md §7.2: B's section, STT_NOTYPE)"; P-D9 view 2 "... (a
  --defsym B+N or B-N: in B's section, moved with the executable;
  resolve.md §7.2)". Reproducers: `resolve/defsym.test`,
  `resolve/defsym-pie.test`; `tests/defsym.rs` (`st-*.bfd`,
  `pie-*.bfd`).
- **A86 (GNU ld; WP-170).** GNU ld gives a `--defsym` alias that
  replaces an input's definition that definition's size, not 0. Y3
  "symbol 'SYM' of --defsym has a size, not 0 (resolve.md §7.2) (the
  size of the definition --defsym replaced; resolve.md §7.2: size 0)".
  Reproducers: `resolve/defsym-override.test`; `tests/defsym.rs`
  (`ov-*.bfd`, `mem-*.bfd`).
- **A87 (GNU ld; WP-170).** GNU ld adds the load base to a `--defsym`
  number in a PIE or static-pie (AArch64: a `RELATIVE` record on its
  word, P-D9; x86-64: a `GOTPCRELX` load relaxed to a `lea`, R1), so the
  program reads the number plus the bias. "... (a --defsym number:
  SHN_ABS, never relocated; resolve.md §7.2)". Reproducers:
  `resolve/defsym-pie.test`; `tests/defsym.rs` (`pie-*.bfd`).
- **A88 (GNU ld; WP-170).** Of several `--defsym` of one `SYM`, GNU ld
  writes the type of an earlier one, not the last one's. Y3 "symbol
  'SYM' of --defsym has another type than resolve.md §7.2 gives it (SYM
  has several --defsym: resolve.md §7.2, the last one's type)".
  Reproducer: `resolve/defsym.test` (`last`).
- **A90 (mold; WP-180).** At `-r` mold turns a common (no definition,
  RV-R5) into a definition `SHN_ABS` whose value is its alignment
  (`comm2`, 32; record `spec/landed/layout/WP-154.md` §B, `exp/moldbug/`):
  the final link then binds every reference to that absolute address,
  and every mold `-r` WRONG or SEGV cell of record §D.3 is this. P-P4
  "common {name} is SHN_ABS, not SHN_COMMON". Reproducer: the D.3 C
  component (`tests/relocatable.rs`, case `d3-c-gcc`; RL-5, WP-185).
- **A91 (lld; WP-180).** At `-r` lld keeps the FDE of a dropped COMDAT
  copy, its initial location then naming a section that is not in the
  output (record §D, `rl0` case 8). P-P5 "output FDE at {offset} for
  {target} comes from a removed FDE of {file}". Reproducer: `rl0` case 8.
- **A92 (mold; WP-180).** At `-r` mold drops non-COMDAT groups: no
  `SHT_GROUP`, the signature symbols `SHN_ABS`, the members still
  `SHF_GROUP` and their relocation sections not (`rl0` case 3, gas's
  `plaingrp`). P-P6 "group {signature} of {file} is not in the output",
  with P-P4's and P-P11's findings on the signature and the flags.
- **A93 (lld; WP-180).** At `-r` lld writes `.note.GNU-stack` without
  `SHF_EXECINSTR` over an input that has it (every X cell of
  `work/stack.log`): the final link then gives a non-executable stack to
  code that asked for one. P-P10 "an input asks for an executable stack
  ({file}) and the output's .note.GNU-stack is N".
- **A94 (mold; WP-180).** At `-r` mold drops an `STB_GNU_UNIQUE` member
  symbol (`_ZZ1fiE1k`; record B, `3-comdat`). P-P4 "{name} is not in
  .symtab".
- **A95 (mold; WP-180).** At `-r` mold strips `.symver` names to their
  base (`vfoo@V1`, `vfoo@@V2` become two `vfoo`): a mislink in the final
  link. P-P4 "{name} is not in .symtab" for each versioned name.
- **A96 (mold; WP-180).** At `-r` mold accepts two strong definitions of
  one name silently and writes both (RV-R4 refuses; GNU ld and lld
  refuse). P-P4 "{name} is in .symtab 2 times".
- **A97 (mold; WP-180).** At `-r` mold writes a debug record whose
  target is a dropped copy against symbol 0, addend 0 (record §D, `rl0`
  case 8). P-P3 "... has an output record against symbol 0, not
  {target}" (no target is equivalent: the record should be dropped with
  the field tombstoned, or kept as GNU ld's or lld's per-linker form).
- **A98 (mold; WP-180).** At `-r` mold writes no `.note.gnu.build-id`
  under `--build-id` (GNU ld and lld write one; `work/rl0-opts.log`).
  P-P10 "the line asks for a build id (sha1) and the output has none".
- **A99 (mold; WP-180).** At `-r` mold merges a `SHF_GNU_RETAIN` piece
  into a section of its name without `SHF_GNU_RETAIN` (`rl0` case 7), so
  a final `--gc-sections` may collect it. P-P11 "section {name} holds
  the SHF_GNU_RETAIN piece {file}:({section}) without SHF_GNU_RETAIN".
- **A100 (lld; WP-180).** At `-r` lld merges `SHF_MERGE` pieces of
  different `sh_entsize` into one section `SHF_MERGE` with `sh_entsize` 0
  (`rl0` case 10; GNU ld drops `SHF_MERGE`, mold writes two sections).
  P-P11 "section {name}: SHF_MERGE with sh_entsize 0".
- **A101 (mold; WP-180).** At `-r` mold writes `sh_link` 0 on a
  `SHF_LINK_ORDER` section (`__patchable_function_entries`; the review's
  `w/lo`, `rl0` case 9): the final link cannot order it. P-P11 "section
  {name}: sh_link is 0".
- **A102 (mold; WP-180).** At `-r` mold copies clang's
  `.llvm.call-graph-profile` and drops its `SHT_REL` section (`rl0`
  case 13), so the profile's weights name no function and their
  `R_*_NONE` records (references) are lost. P-P3 "... R_AARCH64_NONE
  against cga has no output record ...". mortar drops both sections
  (`spec/layout.md` L-R1); GNU ld and lld keep both.
- **A103 (GNU ld, lld; WP-178).** A reference through a weak mention
  defined only in a dropped COMDAT copy, whose key nothing live defines
  (`spec/resolve.md` §6.3 case (b): `comdat-mismatch`, the supervisor's
  ruling of 2026-11-17), is written as an undefined weak reference: a
  call to the next instruction (AArch64) or to 0 (x86-64, i386), the GOT
  word 0, or a `WEAK UND` import in a dynamic executable or a `-shared`
  library (with an archive member or an `--as-needed` library that
  defines the key left out). One id for every form: what is written is
  in each output kind exactly the undefined-weak reading of the
  reference, and the verifier judges each form by the same trial (§5.7a).
  R5 ending "(a weak mention defined in a dropped COMDAT copy, whose key
  nothing live defines, written as an undefined weak reference:
  resolve.md §6.3 case (b))". mold refuses (`hx refers to a discarded
  COMDAT section probably due to an ODR violation`), as mortar does.
  The same trial names the form in a written CIE's personality field
  and in `.gcc_except_table.<fn>` (§5.7a, §5.7b): GNU ld and lld write
  the personality and the type entry of `objcopy --localize-hidden`'s
  pre-link idiom as the undefined weak value, 0 on AArch64 (the place:
  the first throw aborts, signal 6) and `S = 0` on x86-64 (`-P`: it
  faults, signal 11) (`resolve/comdat-mismatch-weak-prelink.test`,
  WP-201).
- **A104 (mold, AArch64; WP-201).** mold writes the mapping symbols of
  an input section it drops as `SHF_EXCLUDE` (Clang's `$d` of
  `.llvm.call-graph-profile`) as `SHN_ABS` locals of value 0; GNU ld,
  lld and mortar drop them with the section. Y4's text ends "(a mapping
  symbol of an input section dropped as SHF_EXCLUDE, written SHN_ABS
  with value 0)" exactly when the output symbol is `SHN_ABS`, value 0,
  and an input local of the name is in an `SHF_EXCLUDE` section without
  a place (`resolve/call-graph-profile.test`; it skipped mold on AArch64
  before).
- **A105 (GNU ld; WP-201).** A field of a written `.eh_frame` or
  `.gcc_except_table` record (a live CIE's personality, a live LSDA's
  type entry) that refers to a dropped COMDAT copy (`spec/resolve.md`
  §6.3 case (a): a local symbol of the dropped section) is written 0:
  the first throw through the CIE's frames finds no personality and
  aborts, and a type entry 0 makes the `catch` a catch-all. R5 ending
  "(a field of a written .eh_frame or .gcc_except_table record, written
  0: resolve.md §6.3)" (§5.7b: the field's owner is written and every
  output field holds 0); another value is plain R5
  (`resolve/comdat-eh-wrong.test`, every target; `tests/ehowner.rs`).
- **A106 (mold; WP-201).** The same field written as if its target
  were at address 0 (`A`, or `A - P` PC-relative): the programs die
  (signal 11; 7 on AArch64's dynamic form). R5 ending "(a field of a
  written .eh_frame or .gcc_except_table record, written as if its
  target were at address 0: resolve.md §6.3)" (`comdat-eh-wrong.test`,
  every target; `tests/ehowner.rs`). lld and mortar refuse these links.
  Reproducers: `resolve/comdat-mismatch-weak.test`,
  `-dynamic`, `-shared`, `-archive`, `-needed`; `tests/demoted.rs`.
- **A120 (lld; WP-185).** At `-r` lld writes a `NOBITS` output section
  named `COMMON` that holds no symbol (the D.3 C component's commons
  stay `SHN_COMMON`, as RV-R5 says): not empty, its `sh_size` is the
  commons' (8 for the C component, `SHF_WRITE|SHF_ALLOC`, alignment 8;
  measured by RL-6 on AArch64). P-P11 "section {index} COMMON comes from
  no input section". Harmless for GNU ld's and lld's final links, which
  put an input section named `COMMON` in `.bss`; mortar's final link
  makes it an orphan output section `COMMON` (P-P1 item 1; the
  supervisor's ruling: WP-203, a gap of mortar's final link, record
  `spec/landed/harness/WP-186.md`).
  Reproducer: the D.3 C component (`tests/relocatable.rs`).
- **A121 (mold; WP-185).** At `-r` mold lists a group member's relocation
  section in the output group without `SHF_GROUP` (the D.3 C++
  component, `debug-cxx`, `none-overlap`). P-P6 "section {index}
  .rela{name} is a group member without SHF_GROUP".
- **A122 (lld, mold; WP-185).** At `-r` lld and mold write an output
  with an `STB_GNU_UNIQUE` or `STT_GNU_IFUNC` symbol with `EI_OSABI`
  `ELFOSABI_NONE` (I-R2; GNU ld writes `ELFOSABI_GNU`). P-P7 "EI_OSABI is
  0, not 3". Reproducer: the D.3 `cifunc` component; GCC's C++ (a
  `UNIQUE` static of an inline function).
- **A123 (mold; WP-185).** At `-r` mold moves `.ctors` and `.ctors.N`
  pieces into `.init_array`, dropping their priority suffix (the D.3
  `cctors` component): a `-r` keeps input section names (L-R1). P-P2
  "{file}:(.ctors...) is in output section {o} .init_array, of another
  name"; P-P11 "{o} .init_array: its name is not its pieces' name".
- **A124 (mold; WP-185).** At `-r` mold makes a group's local signature
  symbol `SHN_ABS` instead of defining it in the output `SHT_GROUP`
  (L-R3; GCC's `wi.` type-unit signatures, gas's `plaingrp` with A92).
  P-P4 "local {name} at 65521+0x0 is no input's local there".
- **A125 (lld; WP-185).** At `-r` lld leaves an `R_*_NONE` record in
  `.rela.eh_frame`/`.rel.eh_frame` where it removed (or, with A91, kept)
  the FDE of a dropped COMDAT copy. GNU ld's identical form is its
  per-linker rule (P-P3); lld's is not one, so it is named. P-P3 "output
  record R_*_NONE at {o} .eh_frame+{offset} ... comes from no input
  record". Reproducer: `rl0` case 8, `debug-cxx`, `none-overlap`.
- **A126 (lld; WP-185).** At `-r` lld keeps the local group signature of
  a dropped COMDAT copy (GCC's `wi.` type units of the second input) as
  an `SHN_ABS` local of value 0, where its group is gone. P-P4 "local
  {name} at 65521+0x0 is no input's local there". Reproducer: the
  `-fdebug-types-section` corpus (`tests/relocatable.rs`, `debug-types`).

- **A127 (lld; WP-186).** At `-r -s` lld keeps the inputs' debug
  sections (GNU ld drops them, §14.3, I-R9). P-P2 "{file}:({section})
  is in the output; §14.3 drops it: a debug section under -S or -s"
  and "... not the tombstone ..." at their records' fields, and P-P3
  "(a record into a dropped copy has no equivalent target ...)" for
  the records between debug sections, and P-P11 "section {i}
  .debug_str comes from no input section" (likewise `.debug_line_str`,
  on x86-64). Reproducer: `tests/relocatable.rs`,
  `opt-s` (the D.3 C component, `-r -s`), every target. A QUESTION to
  the supervisor: an anomaly, or lld's form accepted at `-r -s`.

- **A128 (GNU ld; WP-231).** GNU ld makes an `--as-needed` library
  needed for a mention defined in a dropped COMDAT copy (a demoted
  `STB_GLOBAL` mention) when nothing else refers to the library: an
  extra `DT_NEEDED` entry, the program linked and run (8), where
  resolve §6.2 and dyn.md D-11 item 2 (WP-200's ruling) refuse it
  (`comdat-mismatch`), as lld and mold do. P-D6 with the message "...
  which D-11 does not make needed (only a mention defined in a dropped
  COMDAT copy refers to a definition of it: resolve §6.2, D-11 item
  2)"; on mortar's output the same P-D6, a FAIL. Reproducer:
  `resolve/comdat-mismatch-strong-demoted-needed.test` (`asn`, AArch64
  and x86-64).

lld also links a `.hidden` or `.protected` undefined reference that only
a library defines (writing 0) and drops the library from `DT_NEEDED`:
that is dyn.md W9, P-D6 (A36's family), declared where a test has it,
not a §5.8 convention.

On i386 (WP-028e, `tests/x86.rs`; x86.md §15.2): W1 (two readings of a
`GOT32X` operand) is R8 under every map; W3 (mold's no-base GOT value
G − `.got`) is R4, and its no-base `mov` → `lea` R1; W4 (mold's LE form
from P − 3) is R1; W5 (GNU ld's `GOTOFF` of an IFUNC, the entry − 12)
is F4; W10 is A15 (GNU ld, mold); W11 (GNU ld's `16`/`8` truncation) is
R2; W12 (`SHT_RELA` in `EM_386`) is R8 under every map; W14 (GNU ld's
dynamic types) is R8. W2 reads as based (X86-13) and is accepted (an
accepted risk of the spec). W6–W9 and W16 are static-pie forms, judged
by stage 3 on AArch64 and x86-64 (§12.4); i386 dynamic outputs,
static-pie included, are outside stage 3 (§12.1), so on i386 they are
not judged; W13 and W15 are false refusals, so there is no output.

W1 (GNU ld truncates `16`/`8` fields) and W2 (GNU ld writes `PC16` one
past either end) are violations, R2; W3 (GNU ld writes S + A for a
dynamic-only type) is R8; W5 (lld writes
a link-time value in a static-pie) is an i386 static-pie, outside stage
3 (§12.1: one P-D1 line). GNU ld and mold refuse W5's input; lld and mold refuse
W1's, W2's and W3's.

**Notes, not anomalies** (`spec/layout.md` §20.3; never findings, so no
id):

- lld aligns the RELRO end to 4 KiB with a 64 KiB `p_align`, so under
  64 KiB pages the last part of RELRO stays writable: a loss of
  protection, not of correctness (S16 holds).
- With no `.note.GNU-stack` in any input, GNU ld writes no
  `PT_GNU_STACK` (L-84).
- At `-r` GNU ld gives a non-allocated output section the address its
  location counter had (`.note.GNU-stack` at 0x5c; WP-180, `rl0` case 1).
  Nothing reads it (§14.1); P-P7 does not judge it for GNU ld.
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
- **Offset 0 of an input `.eh_frame`** (WP-088, §4.4; all three
  machines, GNU ld 2.44, lld 19 and 22, mold 2.37.1). For an input whose
  own CIE is kept but not first: GNU ld and lld give the CIE's place;
  mold the output section's start. For an empty input `.eh_frame`: GNU
  ld the place after the previous input's records; lld and mold the
  start. For offset 0x18 (the input's FDE): GNU ld and lld the FDE's
  place; mold the section's start plus 0x18, which is not the FDE (on
  AArch64 inside the CIE, on x86 the CIE's start: mold places every input `.eh_frame`'s symbols as if at the
  section's start; its map lists none, so the verifier does not judge
  them). mortar gives the start for offset 0 and the FDE's place for
  0x18 (`synth.md` S-50).

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
  redirected to the kept copy (`.debug_info`, `.debug_aranges`,
  `.debug_line`, `.debug_rnglists`; GCC `-g3`'s `DW_MACRO_import`). Its
  condition, measured on WP-249's probe (WP-250), is not "the same name
  and size": it is the keeper's first group member whose defined
  symbols match the dead section's (names, `st_info`, `st_other`), then
  an equal size; the name is not compared (§5.8). A reference to a section collected by
  `--gc-sections` holds 1 in `.debug_ranges` and 0 in `.debug_loc` and
  elsewhere. `spec/image.md` §15 says GNU ld writes "the redirect or 0"
  in `.debug_ranges`/`.debug_loc`; the experiment (`-gdwarf-4 -O2
  -ffunction-sections`, `--gc-sections`) shows 1 in `.debug_ranges`, so
  the table of §5.8 follows the observation.
- **lld** and **mold**: 1 in `.debug_ranges` and `.debug_loc`, 0
  elsewhere, except that mold writes 0 + A when the dead target has no
  `SHF_ALLOC` (measured on WP-249's probe, WP-250: `.debug_macro`'s
  `.Ld + 4` holds 4). Neither redirects; mold keeps both copies of
  GCC's `wm4.*` groups. Neither changes the `.debug_aranges` tuple's
  length.
- **Compressed output** (`--compress-debug-sections=zlib`): all three
  compress the output's debug sections (`SHF_COMPRESSED`, `ELFCOMPRESS_ZLIB`).
  GNU ld's and lld's maps keep the input rows; mold's lists only the
  output sections, at their compressed sizes.

These are per-linker rules (§5.8), not anomalies: none is a defect by
DWARF's text, and each is accepted only from its own linker.

**A word against a name only a shared library defines** (WP-104, the
RB-1 defect; §5.8's rule; GNU ld 2.44, lld 19, mold 2.37, Debian 13's
cross toolchains; AArch64, x86-64, i386; `-pie` with the inputs
`-fPIE`, and non-PIE `-no-pie`; one test per type and mode, every word
in a `.debug_` section). A is the addend, P the place, `st` the
library's `st_value`, `plt` the output's ordinary PLT entry, `copy` and
`canon` the output's copy and canonical PLT entry:

| reference against | GNU ld | lld | mold | rule (mortar) |
|---|---|---|---|---|
| data object, PIE, absolute | 0 + A | 0 (A38) | st + A (A39) | 0 + A |
| function or IFUNC, PIE, absolute | 0 + A (AArch64, i386); plt + A (x86-64, A37) | 0 (A38 if A ≠ 0) | plt + A (A40) | 0 + A |
| any import, PIE, PC-relative | A − P (x86-64 function: plt + A − P, A37) | A − P | refused (AArch64, x86-64); i386 st + A, P ignored (A39) | A − P |
| protected in the library; weak reference the library defines; a name only debug words reference | 0 + A | 0 | st + A (A39) | 0 + A |
| data object, non-PIE (copied) | copy + A | copy + A | copy + A | copy + A |
| function, non-PIE (address taken) | canon + A | canon + A | canon + A | canon + A |
| function, non-PIE (only called: an ordinary PLT entry) | plt + A (A37) | 0 (A38) | plt + A (A40) | 0 + A |
| undefined weak, no definition, PC-relative | AArch64 A (S = P), x86 A − P | A − P (AArch64: A43) | A − P | §4.4 |
| library `STT_TLS`, absolute | refused ("bad value") | 0 (A41) | st + A (A41) | refused by name (P-D12) |
| library `STT_TLS`, `DTPOFF64`/`DTPOFF32`, no `PT_TLS` | 0 + A | 0 (A38) | 0 + A | 0 + A |
| the same, the output has a `PT_TLS` | A − `PT_TLS` p_vaddr (A42) | 0 (A38) | A − p_vaddr (A42) | 0 + A |
| `.hidden` undefined reference | refused | 0, library dropped (P-D6) | refused | refused |
| `.protected` undefined reference | refused | 0, library dropped (P-D6) | st + A (A39) | 0 + A |

AArch64's `R_AARCH64_TLS_DTPREL64` in a non-alloc section against a
library's TLS symbol is refused by all three references and by mortar.
i386 rows are measured only: mortar refuses every i386 dynamic output
and stage 3 does not judge one (§12.1). GNU ld and lld differ on the
DTP offset with a `PT_TLS` (A − p_vaddr against 0); the rule's 0 + A is
GNU ld's value without one and the value the offset has in no module,
and is the supervisor's ruling (WP-104).

**What a debugger does with the word** (WP-104; gdb 16.3, lldb 19.1.7;
Clang 22 `-g -O2`, whose `DW_TAG_variable` `_vtable$` of a class with a
library-defined vtable, `std::bad_alloc`'s `_ZTVSt9bad_alloc`, is a
`DW_OP_addrx` into `.debug_addr`): neither debugger reads `_vtable$`.
gdb prints `static _vtable$ = <optimized out>` and `info vtbl` reads the
object's vtable pointer; lldb finds no `_vtable$` variable. Both behave
the same with 0 (GNU ld, lld, mortar), the library's `st_value` (mold)
and a PLT entry (a patched copy of GNU ld's output). A consumer that
did read it would see 0 as "no address" but would take `st_value` for
an address of the executable (of nothing, or of the wrong object) and a
PLT entry for code: `image lookup` names it "symbol stub for:
std::bad_alloc::what() const". So the tombstone 0 + A is the only one of
the three a consumer can tell from an address.

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
7. Static PIE: closed by WP-089. Stage 3 judges it (§12.4: P-D1 accepts
   `ET_DYN` without `PT_INTERP`, P-D9's views judge the `RELATIVE`
   records, P-D14 its `.dynsym` and `.dynamic`, P-D15 glibc's
   application); with `-E` too since WP-088. i386 static-pie stays
   outside stage 3 (§12.1).
8. A9 to A11 (§10.2): mortar's `.symtab` must keep the inputs' bindings,
   name the section that holds each label (or `SHN_ABS`), and define
   `__bss_start` within §6.5's range even when there is no `.bss`.
9. i386 (§4.5): the table and its rewritings need a spec of their own;
   until one exists every i386 record is `machine`.
10. X-48: resolved (WP-028d, §5.9): in mortar's static `ET_EXEC`
    output, a branch or GOT reference to `__tls_get_addr` is R8.
11. (Resolved by WP-102, §3.2a.) The map states the link's options that
    decide a rule: its line, in every map; the erratum option (§5.10)
    and `-z execstack`/`-z noexecstack` (S10's `PF_X` clause) are read
    from it.

## 12. Stage 3: dynamic executables (M2)

The rules of stage 3 (M2; moved from `spec/dyn.md` §12 by WP-040r,
request R-P1; their ids stay `P-D`*n*, and the `D-` rules they cite are
`spec/dyn.md`'s). The verifier reads the output, the inputs (now
including shared objects) and the machine map, and must not share code
with what it checks (§1). Until stage 3 lands it rejects anything but a
static `ET_EXEC` (S1). Each rule checks a `spec/dyn.md` D rule from the
output bytes and the inputs. **The verifier never takes from the linker
what it can recompute from the inputs**: from the map it reads the
command line's record (files in order with their `--as-needed` and
`-Bstatic` state, options), as stage 2 does; every decision of
`spec/dyn.md` (needed set and order, export set, import set, versions
bound, copy and canonical-PLT choice) is recomputed by the verifier's
own implementation of D-11-D-27 and compared with the output.

- **P-D1 (accept).** `ET_DYN` with `DF_1_PIE` and `ET_EXEC` with
  `PT_INTERP` are verified, and so is a static-pie (`ET_DYN` with
  `PT_DYNAMIC`, no `PT_INTERP`; §12.4); the kind is recomputed from the
  headers and the command line and must agree (D-1 table, every row).
- **P-D2 (shared inputs).** Read every shared object the command line or
  a script names, and the `DT_NEEDED` closure (input §8.6, searched by
  the verifier itself), with `mortar_elf`: `.dynsym`, versym, verdefs,
  `DT_NEEDED`, `PT_LOAD`/`PT_GNU_RELRO` extents. An input it cannot open
  is an error.
- **P-D3 (map records).** The map gains the output kind, the interpreter,
  rpath, `-z now`/`-z norelro`, and, as **hints for order only**, the
  needed list, the copy allocations, the export set and the PLT/IPLT key
  order: the verifier uses them to name what it compares, never as the
  expected value.
- **P-D4 (segments).** `PT_PHDR` covers the program headers and lies in
  the first `PT_LOAD`; `PT_INTERP` exists in `Exec`/`Pie`, covers
  `.interp`, and its string is the command line's `-dynamic-linker`
  (W10; when the line names none, D-31 refuses the link, so any
  `PT_INTERP` is P-D4 "a linker default (D-31, W10)" and none is P-D1
  and P-D4 "(no -dynamic-linker, no PT_INTERP: D-31, W10)": a reference's
  output is anomaly A22, GNU ld's built-in string, right on AArch64 and
  `/lib/ld64.so.1` on x86-64, or A23, lld's and mold's missing
  `PT_INTERP`; WP-050 proposal 3); `PT_DYNAMIC` covers exactly `.dynamic` and is writable; unless
  `-z norelro`, `PT_GNU_RELRO` covers `.dynamic`, `.got`, `.bss.rel.ro`
  and (`-z now`) `.got.plt`, and ends on maxpage (D-33, D-42, D-49,
  D-61): for mortar's map, maxpage is S2's, the line's last `-z
  max-page-size` (0 or none: 64 KiB on AArch64, 4 KiB on x86-64 and
  i386; layout.md L-74), and `-z common-page-size` changes nothing
  (L-75; WP-121); under `-z norelro` there is no `PT_GNU_RELRO`.
  `PT_GNU_STACK`'s `PF_X` (WP-088; the kernel and glibc read only that
  bit of it): under the line's last `-z execstack` it is set, under
  `-z noexecstack` clear; with neither, it is set only when an input
  object's `.note.GNU-stack` is `SHF_EXECINSTR`, and under mortar's map
  exactly then (L-84). Measured (GNU ld 2.44, lld 19, mold 2.37.1,
  mortar; x86-64 and AArch64; an object with no stack note, one with an
  executable note, one with a plain note, each with no option, `-z
  execstack` and `-z noexecstack`): every linker obeys the options; with
  neither, an executable note gives `PF_X` from GNU ld and mortar (with
  a warning), none from lld 19 or mold, and lld 22 refuses the link
  ("requires an executable stack, but -z execstack is not specified");
  a missing note gives no `PF_X` from any (GNU ld 2.44's default). An executable stack the link did not ask
  for is a permission the program does not need; one it asked for and
  did not get faults in the code that needed it (a nested function's
  trampoline). No `PT_GNU_STACK`: not judged here (S10; for mortar's
  map S10 makes it a finding, WP-131). Test:
  `tests/dyn.rs` `s12_p_d4_gnu_stack_execute` (case `stack`: the three
  linkers' `-z execstack` and `-z noexecstack` PIEs and GNU ld's and
  mold's executable-note PIEs clean, each `-z` output and the lazy hello PIE with `PF_X` flipped
  P-D4).
  The entry point (WP-088, P-D14's rule in an `Exec` or a `Pie`): it is
  in an executable `PT_LOAD` and is the entry symbol's value (`-e`, else
  `_start`) where `.symtab` defines it; S7 already judges an `ET_EXEC`'s
  segment, and a `Pie` had no entry rule before. The references' outputs
  all hold it (the corpus and §12.1's cases clean). Test: `tests/dyn.rs`
  `s12_pie_entry_and_non_address_relative` (case `pie-rules`: each
  linker's `-pie` output of `relx.c` with `e_entry` + 4 is P-D4 on both
  machines, missed with the rule dropped).
  An entry symbol the output does not define (WP-163; resolve.md §5.5:
  `-e K` where no live object defines K, as when only an `--as-needed`
  library that the root does not make needed defines it, D-11 item 2):
  the link warned `entry-not-found` and mortar writes `e_entry` 0
  (layout.md L-103), so 0 is accepted from every linker; in mortar's
  output any other value is P-D4 ("... is defined by no live object and
  the entry point is X: layout L-103 writes 0"); a reference's own
  fallback (GNU ld: the start of `.text`) is held to an executable
  `PT_LOAD`. "Defined" is the verifier's model: a definition in a live
  relocatable object or one of file 0's (the linker's own). When the symbol is
  defined, a 0 is P-D4 as any other wrong value. Tests: `tests/shared.rs`
  `p_d_roots_mutations_are_caught` (GNU ld's `-e k_fn` outputs, whose
  `e_entry` is the start of `.text`, are P-D4 as mortar's; each
  reference's output with `_start` defined and `e_entry` 0 is P-D4).
  **No `PT_DYNAMIC` (WP-131 V5).** The output kind comes from the line
  and the inputs (D-1), never from what the output holds: an output
  without `PT_DYNAMIC` is judged as static only when nothing says it is
  dynamic. When the output has `PT_INTERP`, an `SHT_DYNSYM` section, a
  `.dynamic` section, or an allocated relocation section with a record
  other than `IRELATIVE`, or (mortar's outputs) the line's kind (§12.1,
  `line.rs`) is `Exec`, `Pie` or `StaticPie`, or (a reference's) the
  line loads a shared object (not under `--as-needed`), then a missing
  `PT_DYNAMIC` is P-D4 ("no PT_DYNAMIC, and ...", naming each reason):
  the loader finds no `.dynamic`, so no `DT_NEEDED` and no relocation
  table. The line's kind alone binds mortar only (D-1's `Exec` row always
  has `.dynamic`): GNU ld, lld and mold all write a `-no-pie` line with
  no shared input as a static-shaped executable, judged as the static
  executable it is when none of the other reasons holds (GNU ld's;
  lld's and mold's carry `PT_INTERP` and crash at start, measured on
  AArch64: P-D4, `reloc/x64-gotpcrelx-slot`; WP-121, test `tests/dyn.rs`
  `s12_p_d4_reference_without_shared_inputs`). Each shared
  object the line loads (not under `--as-needed`) is also P-D6 ("no
  PT_DYNAMIC, so no DT_NEEDED names ..."). Such an output gets the
  structure rules (§6.2) and these findings, not the static rules,
  whose model of it would be wrong. mold 2.37 writes such a file after
  `--push-state -Bstatic --pop-state -l<lib>` (PT_INTERP, `.dynsym`, a
  PLT and a JUMP_SLOT, no `.dynamic`); it crashes at start-up
  (`tests/link/dyn/shlib/push-state.test`, `xfail(mold)`). Before, the
  verifier took "dynamic" from `PT_DYNAMIC` itself and passed it. The
  same decision chooses the path of `equiv`'s sides (§6.9) and of the
  mutation runner. Tests: `tests/dyn.rs` `s12_p_d4_no_pt_dynamic`,
  `s12_p_d4_no_pt_dynamic_in_equiv`.
- **P-D5 (`.gnu.hash`).** Recompute the table from `.dynsym` with D-26's
  formulas and compare word for word; then look up every exported and
  every canonical-PLT name with the loader's algorithm (it is found at
  its index) and a fixed set of absent names (not found). The lookup of
  an entry is by its name and its `.gnu.version` index, as glibc's
  `check_match` takes a chain entry only of the version asked: a
  library's `.symver` `foo@V1` (hidden) and `foo@@V2` are each found at
  their own index (D-115; WP-121, WP-123's V3).
- **P-D6 (`.dynamic`).** Each D-30 tag is present iff its condition holds
  (recomputed from the sections and the command line), in D-30's order,
  with the value equal to the address or size of its section; exactly
  six `DT_NULL` entries at the end. The `DT_NEEDED` entries equal the
  needed set the verifier computes by D-11/D-12 from the P-D2 inputs, in
  D-19a's order (W9). Under D-12's stages (WP-171, §3.8's replay) N is
  the union of every stage's, and D-19a's reasons are every stage's (a
  member a later stage extracts is a relocatable object at its archive's
  position): a library that joins N through it is kept at the first
  mention after that archive, and one kept needed (row 2a) at the
  mention its stage-1 reason gave. A needed library the output lacks
  whose every key it gave at its stage is defined by the output's own
  objects (a later stage's member: row 2a) is P-D6 "DT_NEEDED lacks
  '...', which D-11 makes needed and D-12 keeps needed (row 2a: N only
  grows; a later stage's member defines what it gave)"; lld drops it
  (A71). GNU ld's own neededness (`gnu_unbound`) reads the keys a
  library gave at the stage that made it needed.
  A root (WP-163; resolve.md §4.2's table: `-e`, else the script's
  `ENTRY`; `-u`; `--require-defined`; an implicit script's `EXTERN`,
  quoted or not) is no reference for D-11: it never makes an
  `--as-needed` library needed (D-11 item 2); the needed set comes from
  the inputs' references alone. A library in `DT_NEEDED` that only a
  root names a definition of is P-D6 "... which D-11 does not make
  needed (only a root names a definition of it: D-11 item 2)"; mold
  makes it needed (A48). The verifier reads the roots once
  (`Line::root_names`), from the fields the input set's rule (§3.8)
  reads.
  D-13 (WP-088, W16): unless the line has `--allow-shlib-undefined`,
  every strong undefined reference of a member of N ∪ U is satisfied by
  a program definition that is not hidden, or by a visible definition, of
  the version it requires, in a member of N ∪ U; one that is not is P-D6
  "... (D-13: the link is refused)", once per name. lld and mold make
  that link and leave the defining library out of `DT_NEEDED` (it is not
  needed by D-11), so the equality above holds and only this rule sees
  the program fail at load (`symbol lookup error`, exit 127). Test:
  `tests/dyn.rs` `w16_used_object_reference_unsatisfied`.
- **P-D7 (`.dynsym`).** The set equals D-21 recomputed from the inputs:
  imports are the program's references the verifier resolves to needed
  shared objects (hidden `.dynsym` entries ignored, W1), exports the
  program definitions referenced by needed or used objects (W8), every
  copy alias, the `WeakDyn` keys; order per D-22; fields per D-23;
  `sh_info` 1; `.dynstr` holds each name once. The `WeakDyn` keys come
  from one derivation (WP-165, `weakclass.rs`): each undefined weak key
  that no member of N ∪ U defines gets D-25's class of every live
  relocation naming it (in a placed, allocated section; the writable
  place from the OUTPUT section; the branch-opcode test of x86 `PC32`;
  a type the table does not list unfillable), and D-25's verdict in the
  output's kind. A key D-25 makes `WeakDyn`, or in `Exec` `WeakDyn` for
  its branches (the split key, ruling (A)), that `.dynsym` lacks is P-D7
  "'K' is an undefined weak key reached by ... and is not in .dynsym
  (D-25: WeakDyn)" (the split key: "... (D-25 ruling (A): WeakDyn for
  its branches)"), and one with a branch whose entry no `JUMP_SLOT`
  names is P-D9 "... has no JUMP_SLOT record (D-25: every branch through
  a PLT entry)". mortar's output is held to this for every such key; a
  reference's for the keys whose uses besides their branches are
  unfillable or none (a branch alone; the split key: WP-155's cells,
  where GNU ld and lld import): mold writes no entry (A55), GNU ld on
  x86-64 none for the split key (A56). A key with a GOT use or a
  writable absolute word is not held to the import in a reference's
  output (D-25 records mold's 0 everywhere for it, and mold writes no
  `__gmon_start__` entry in any AArch64 driver link: measured, WP-165).
  Whether a key D-25 makes `Weak0` is in `.dynsym` is not judged. A root (P-D6 above) the
  program does not define is imported iff a library of the final needed
  set has a visible definition of it, unversioned or of its default
  version (what a reference without a version binds; D-21 item 1,
  WP-163); otherwise it is written nowhere. A root a needed library
  defines and `.dynsym` lacks is P-D7 "... is a root defined by L, a
  needed library, and is not in .dynsym (D-21 item 1: a root is
  imported)": lld and mold write none (A47); a root imported though no
  library of the needed set defines it is P-D7 "... is imported and no
  library of the needed set defines it (a root: ...)". Tests:
  `tests/shared.rs` `p_d_roots_of_as_needed_libraries` (case `roots`:
  each reference's `-u k_fn` link with `libks.so` as-needed and not,
  and a quoted `EXTERN("k_fn")`) and `p_d_roots_mutations_are_caught`;
  the link tests `dyn/shared/root-as-needed*.test`. The line's export options
  (WP-161; D-110 item 1, resolve.md §9.6), read in every format: a key
  whose every relocatable definition is in a member of an archive
  `--exclude-libs` names (`ALL`, or the member's archive by file name)
  is exported by nothing, neither `-E`, nor a list, nor a shared
  object's reference ("... its definition is in an archive member
  --exclude-libs names, which no export gives"); a shared object's
  strong reference whose only definition is such a key is unsatisfied
  at load, the P-D6 finding above ("... which no export gives, D-110
  item 1 (D-13: the link is refused)": mold links it and the program
  exits 127, A36). An executable's `--dynamic-list` files (read from the
  paths the line names, as §12.5 reads a version script) list
  definitions that are exported, with or without `-E` ("... listed by
  the line's --dynamic-list, and is not exported"); an `extern "C++"`
  block in one is P-D1's (D-112). Without `-E` and without a list only
  W8's exports are wanted; whether an executable exports more than
  these is WP-157's, not judged. A static-pie exports nothing whatever
  the line says (P-D14). Tests: `tests/shared.rs`
  `s12_5_executable_option_tail_verifies_clean` (the references),
  `p_d7_option_tail_mutations_are_caught` (each reference's output
  under the line with an option added or dropped),
  `p_d_mortar_option_tail` (mortar's, `tests/shared/optail`); the link
  test `dyn/dynsym/exclude-libs-exe`.
  D-21 item 2's seven linker-defined names (`_etext etext _edata edata
  __bss_start _end end`, WP-150 §3a; WP-166) in `Exec` and `Pie`: the
  link defines each, whatever a needed library defines (a library's
  `char _end[1]` never serves the program's reference: the seven are
  file 0's even where a `DT_NEEDED` object defines them, and a needed
  library's definition does not make the name an import, so P-D11 does
  not ask a `COPY` of it), and exports it iff a needed or used object
  references or defines it (`ref_dynamic`, `def_dynamic`), or an object
  references it (strong or weak, not hidden) and the line has `-E` or a
  `--dynamic-list` that names it. `-E` with no reference exports none.
  One missing is P-D7 "... is defined by the program and referenced by
  a needed or used object, and is not exported" or "... an object
  references it and the line's -E or --dynamic-list exports it, and it
  is not exported (D-21 item 2)"; one exported with neither is P-D7
  "'K' is exported, and no needed library references or defines it and
  no object's reference under -E or --dynamic-list exports it (D-21
  item 2)" ("'K' is exported under -E, ..." when the line has `-E`; not judged under `--export-dynamic-symbol`, whose words the
  line model does not read). A name of the seven that `.dynsym` defines
  and no input defines is the link's definition however `.symtab`
  reads (absent, or stripped). Strict for mortar; mold exports none
  (A57); GNU ld under `-E` exports `_edata`, `__bss_start` and `_end`
  though no object references them (A60, the `-E` form only). Tests: `crates/mortar_plumb/tests/ldef.rs` (`clean`,
  `mutations`) on `tests/ldef/` (`gen.sh`), the link tests
  `dyn/dynsym/linker-defined-export-exec` and
  `dyn/dynsym/linker-defined-def-exec` (a library that defines the
  seven, `-pie` and `-no-pie`, the run `api_end() == _end`).
- **P-D8 (versions).** Each `.gnu.version` index is 0, 1, or names a
  `Vernaux` whose version is **the version the input reference requested**
  (the default `@@` definition for an unversioned reference, the
  `.symver` name for a versioned one, resolve §9.2), defined by the
  needed library that defines the symbol; `vna_hash` is the ELF hash of
  the name, `vn_file` a `DT_NEEDED` name, `vna_other` unique; a
  `memcpy@GLIBC_2.2.5` reference bound to `memcpy@@GLIBC_2.14` is
  reported. A weak import whose definition is only in a library the
  output does not need (`--as-needed` dropped it; the loader reaches it
  through another library's dependencies) has no `Verneed` to name
  (`vn_file` is a `DT_NEEDED` name): it is unversioned, index 1, as GNU
  ld writes crtbeginS's `__cxa_finalize` in a `g++ -shared` library
  (WP-121, WP-123's V5). In a `Shared` output, `VERSYM_HIDDEN` on a
  definition whose index names a `Verdef` is D-115's `foo@V`, not D-27's
  "bit 15 never set" (an executable's), and P-D22 reports a name with
  more than one definition without it. Every other index of a `Shared`
  output's `.gnu.version` is P-D27's (WP-149).
- **P-D9 (dynamic relocations).** A defined `.dynsym` entry with
  `VERSYM_HIDDEN` (`foo@V1`) is bound by the loader model as the key
  `foo@V1`, never as the plain `foo`: an unversioned lookup does not
  find it (glibc's `check_match`; WP-149: GNU ld writes `foo@V1` before
  `foo@@V2`, and R1 judged the call to `foo@plt` against the hidden
  one). The extension of stage 2's
  reconstruction: for each input relocation whose place is in the output,
  the run-time value of the place is computed from the output (its
  bytes, then its dynamic records applied by a loader model of the
  verifier's own, symbols looked up in the P-D2 objects **in ld.so's
  order**: the executable, then the `DT_NEEDED` entries breadth-first,
  the order D-19a writes and `bind` uses, D-11-D-13; an image whose copy
  size or provider came from a library later in that order, the review's
  `dupdef`, fails here) and from the
  input (`S + A`, `S` the definition's run-time address), in **three
  views**, which must all agree: (1) the load bias 0 and (2) a second
  bias, for a `Pie` (base independence: every absolute word has its
  record, W7, W11); (3) every `WeakDyn` key defined at a fresh address,
  as a preloaded library would (the GOT and the call must both reach it:
  the image of the review's F2 fails here). This proves at once the
  `RELATIVE`/symbolic choice, addends, the copy redirection,
  canonical-PLT equality and IFUNC equality (D-44, D-45; W3). Whether
  the output defines a reference's name (else the loader binds it) is
  read from its `.symtab`; a stripped output has none (WP-164), and the
  name is then defined exactly when resolution chooses an input
  definition or a common for it (§4.4), so a stripped library's
  exported, bound key stays its own (measured: mold's `-s -Bsymbolic`
  library, view B, a false finding before). Plus:
  `RELATIVE` first and sorted, `DT_RELACOUNT` right (D-40); no record in
  a non-writable place outside RELRO (D-47; W6). One past the end
  (WP-166): a `RELATIVE` record (or an absolute word, view 2's
  non-address rule) whose value is in `[E, E rounded up to 8]`, E the
  end of a `PT_LOAD`, is an address of the executable when the input
  symbol is a global or weak segment-end key no input defines (`_end`,
  `end`, `__end__`, `_bss_end__`, `__bss_end__`, `_edata`, `edata`,
  `__bss_start`, `__bss_start__`, `_etext`, `etext`, `__etext`) and an
  output symbol of that name has exactly that value (Y5's window: GNU
  ld's `_end` is the end rounded up to 8). Before WP-166 such a record
  was P-D9 view 2 "(a record moves a value that is not an address of
  the executable)", undeclared as A30/A34 on GNU ld's libraries. The
  same value moved 2 bytes further is P-D9 (a slot no reference asks
  for) or R4 (`tests/ldef.rs`, `mutations`).
- **P-D10 (PLT and GOT).** Each PLT entry decodes (the architecture's
  plumb rules, already stage 2 for IPLT) to its `.got.plt` word; that
  word has one `JUMP_SLOT` against the entry's key; its initial content
  is the lazy target (D-49); IPLT words have one `IRELATIVE`, every one
  after every `JUMP_SLOT` (D-41; H1). The lazy target leads, through
  the PLT header, to the loader's resolver with the inputs glibc's
  trampoline reads, and `.got.plt`'s reserved words are the loader's
  (§12.2, WP-082).
- **P-D11 (copies).** Each `COPY` record's place is an allocation of the
  shared definition's `st_size` (never 0: W4) and alignment in a writable
  `PT_LOAD`, the allocations do not overlap each other or any other
  definition, and every alias at that `(file, value)` is defined in
  `.dynsym` at the copy (W2). Where the output has a `PT_GNU_RELRO`, the
  allocation is inside it exactly when the definition lies in a
  `PT_GNU_RELRO` or non-writable `PT_LOAD` of its library (D-42). Without
  one (`-z norelro`; a missing segment the line asks for is P-D4's)
  nothing is read-only after relocation and RELRO membership is not
  judged: every reference then copies a read-only definition into
  ordinary writable memory (measured, WP-088, x86-64 C++ PIE and both
  machines' `copy-relro` case: GNU ld `.data.rel.ro`, lld `.bss.rel.ro`,
  mold `.copyrel`). `Format::Mortar`, with or without RELRO: the
  allocation is in the output section `.bss.rel.ro` for a read-only
  definition and in `.bss` (its `.dynbss` chunk) otherwise (D-42).
- **P-D12 (TLS).** Every TLS slot holds the record of its model (IE:
  `TPREL`; unrelaxed GD: `DTPMOD` + `DTPOFF`; LE: none, value checked by
  stage 2) and no TLS relocation against an executable-defined symbol
  leaves a dynamic record (D-48). A TLS record with symbol 0 (against the
  executable's own block) is accepted from a reference and is P-D12 in
  mortar's output, which D-48 relaxes to local-exec (WP-050 proposal
  12). The one kept descriptor (D-48): an
  AArch64 large-model TLSDESC sequence (`movz`/`movk` of
  `:tlsdesc_off_g1:`/`:tlsdesc_off_g0_nc:`, relocations 565 and 566 with
  567 and 569 against the same symbol, no small-model TLSDESC reference
  to it) keeps one `TLSDESC` record, exactly at the pair the two
  instructions address (`_GLOBAL_OFFSET_TABLE_` + their 32-bit offset);
  any other `TLSDESC` record in mortar's output is P-D12 (WP-082).
  D-16's other direction (WP-088, W14): an input's reference of a
  non-TLS type of the machine's table, from an allocated section, to an
  undefined name the loader binds to a library's `STT_TLS` definition is
  P-D12 "a non-TLS reference to '...', whose shared definition is
  STT_TLS (D-16)", once per name. lld writes the GOT word the code reads
  as an address as a `TPREL64` slot, mold a `GLOB_DAT` that glibc
  resolves to the library's base plus the TLS offset (the program reads
  the library's ELF header); GNU ld and mortar refuse the link. The TLS
  direction (a TLS reference to a non-TLS definition) was already P-D12's
  record rule, and T1 where the output has no `PT_TLS`. Test:
  `tests/dyn.rs` `w14_non_tls_reference_to_shared_tls`.
- **P-D13 (protected).** No `COPY` record and no canonical PLT
  (`SHN_UNDEF` with a non-zero `st_value`) names a key whose shared
  definition is `STV_PROTECTED` (D-18; W5): P-D9's equality cannot see
  it, because the executable's own `&pf` is its PLT entry everywhere.
- **P-D14 (static-pie: what it owes glibc).** The headers, `.dynamic`,
  `.dynsym`, the record types and the GOT's `_DYNAMIC` word of a
  static-pie (§12.4).
- **P-D15 (static-pie: how glibc applies it).** Every `RELATIVE` and
  `IRELATIVE` record is applied exactly once by glibc's self-relocation,
  no `RELATIVE` after an `IRELATIVE`, and the C runtime's
  `__rela_iplt_*` range is empty (§12.4).

What reports each wrong output and hazard of `spec/dyn.md` §13.2:

| finding | reported by |
|---|---|
| W1 hidden `.dynsym` entry bound | P-D7 (the verifier's resolution sees the reference undefined) |
| W2 copy alias not exported | P-D11, P-D7 |
| W3 IFUNC address inequality | F3 (stage 2, in view 1), P-D9 |
| W4 zero-size copy | P-D11 |
| W5 protected canonical PLT or copy | P-D13 |
| W6 text relocation | P-D9 (record in a non-writable place) |
| W7 read-only word, copy, no record | P-D9 (views 1 and 2) |
| W8 closure-referenced symbol not exported | P-D7 |
| W9 needed library dropped | P-D6 |
| W10 wrong or missing interpreter | P-D4 |
| W11 canonical PLT word without a record in a PIE | P-D9 (views 1 and 2) |
| W12 weak symbol split between GOT and absolute use | P-D9 (view 3) |
| W13 executable IFUNC exported as `STT_GNU_IFUNC` / at the implementation | P-D7 (D-23 fields), P-D9 |
| W14 TLS/non-TLS mismatch against a shared definition (lld, mold) | P-D12 (a non-TLS reference to a shared `STT_TLS` definition; a TLS reference to a non-TLS one: the record rule, and T1 without `PT_TLS`); mold's copy of an `STT_TLS` definition: S8 (WP-088) |
| W15 GNU ld's AArch64 GD relaxed to IE without a64 R4's `nop` | R1 "the instruction is rewritten into a form no specified rewriting allows" (`tls-gd-unrelaxed`, L3) |
| W16 a used object's reference left unsatisfied, its library dropped (lld, mold) | P-D6 (D-13, WP-088) |
| W17 lld's x86-64 large-model general-dynamic TLS (crashes) | R1, anomaly A12 (x64.md W4's 16-byte local-exec form over the 22-byte sequence; `tests/dyn.rs` case `w17`, PIE and `-no-pie`) |
| H1 `IRELATIVE` before `JUMP_SLOT`s | P-D10 |
| F2 image (GOT `WeakDyn`, call `Weak0`) | P-D9 (view 3) |

Mutation tests (the WP-028d harness) gain one mutation per rule: drop a
record, swap two `.dynsym` entries, flip a Bloom bit, change an addend,
shrink a copy to 0, overlap two copies, misorder `IRELATIVE`, drop a
`DT_NEEDED`, bind a different version, patch a `WeakDyn` call to its
`Weak0` form. D-25's branch cells (WP-165, `tests/weak.rs`, mortar's,
lld's and GNU ld's outputs on both machines): the import removed (a
branch alone; the split key), the `JUMP_SLOT` removed, the branch
patched to 0, the split key's unfillable use given a value or a record,
and `Weak0` written for a branch-only key (mortar's form before WP-155).

### 12.1 How the verifier reaches them (WP-050)

This subsection records how WP-050's verifier reaches the rules above.
A dynamic output (`PT_DYNAMIC`) is judged with the map, the inputs and
the **line**: the caller's (the linker's words: `--line FILE`, one word
per line, or `--inputs-from`), for a mortar map its `arg` records
(§3.2a) only when the caller gives none (§12.1a, WP-193), and the
options §3.2a's reader gives (P-D4's `-z execstack`, B1's `--build-id`,
...) from the same line (WP-201). The line gives D-1's kind, `-dynamic-linker`, `-z
now`, `-z relro`, `-E` and the files of D-11/D-12; the shared objects are read
from the file system (`line::Files`).

- **The model** (`dynmodel.rs`): D-11's needed set, D-12's used set and
  closure, D-19a's order (GNU ld's and mortar's keeping mention; lld's and
  mold's first mention, `spec/dyn.md` §13.1), every key's view from the
  placed objects (a mention defined in a COMDAT copy the verifier's
  keeper drops is an undefined mention of its binding, §5.7a, WP-178).
  Map records are order hints only; nothing is taken from
  the output that the model can compute.
- **Static checks** (`dyncheck.rs`): P-D1, P-D4..P-D8, P-D10..P-D13 and
  P-D9's record order. `Format::Mortar` is held to the D rules exactly
  (D-22/D-24/D-26/D-30, six `DT_NULL`, RELRO ending on the maximum page);
  a reference output to its own form where §13.1 says the references
  differ. RELRO coverage uses the end rounded down to 4 KiB, as
  `_dl_protect_relro` does. ld.so applies the first `DT_RELACOUNT` records
  as `RELATIVE` whatever their type: a count above the leading `RELATIVE`
  run is P-D6 for every linker. P-D5 also checks the chains' shape (each
  bucket's group contiguous, the bucket word its first entry, the end bit
  exactly on a group's last entry, each chain word the name's hash with
  bit 0 cleared). P-D8 also checks exported definitions with a version
  index of 2 or more (a copy or an interposition): glibc's versioned
  lookup skips the executable's definition unless a library defines the
  name under that version; and an import nothing defines carries no
  version. H1 (P-D10) is every `IRELATIVE` after every symbolic record
  (`JUMP_SLOT`, `GLOB_DAT`, `COPY`, absolute), D-41's reason.
- **P-D2 stops stage 3**: a line word or a `DT_NEEDED` object the
  verifier cannot read is reported and nothing computed without it is
  judged.
- **Outside stage 3** (P-D1, a finding starting "outside stage 3: ",
  `mortar_plumb::OUTSIDE`): a link stage 3 does not model is named and
  nothing of stage 3 is judged: an option `spec/dyn.md` refuses
  (`--hash-style` other than `gnu`, `--copy-dt-needed-entries`,
  `-shared`), any i386 dynamic output (`Pie`, `Exec` with `PT_DYNAMIC`, static-pie:
  REL records and `DT_REL`, while every stage 3 rule is written for the
  RELA targets; one line "outside stage 3: i386 ..." replaces every
  other, WP-089), an output with version definitions
  (`DT_VERDEF`, M4), and a `DT_NEEDED` object of the closure no search
  finds (a link D-13 refuses, which lld and mold make). It is a finding,
  so mortar's output fails with it; the link-test harness counts a
  reference's output that has it as not judged, "outside stage 3"
  (`spec/harness.md` "L3").
- **A reference to its own form** where `spec/dyn.md` records its rule:
  besides §13.1's order rules, GNU ld may leave out an as-needed object
  needed by D-11 item 2 alone whose keys all bind to an object before it
  in D-19a's order (D-11: GNU ld decides neededness by its binding), and
  mold records the giver of a key referenced only weakly (D-11, E-shlib
  2.2a).
- **The loader model** (`loader.rs`): the output's records applied as
  ld.so applies them (symbols looked up in the executable, then the
  needed objects breadth-first; `COPY` lookups skip the executable;
  `JUMP_SLOT` lookups skip a canonical PLT), giving the bytes the program
  sees. Library `i` is at `0x7f00_0000_0000 + (i << 32)`. TLS: a
  library's block sits at `start + ((i + 1) << 32)` in the program's
  TLS address space, module `2 + i`; `DTPOFF`/`TPOFF` follow from that;
  a `TLSDESC` pair is `[0x7e5c_0000_0000_0001, TPOFF]` (the static-TLS
  resolver). An executable's TLS `st_value` is a segment offset (gABI).
- **The views** (`dynverify.rs`): stage 2's engine judges every input
  relocation against the loaded bytes of view 1 (bias 0), view 2 (bias
  `0x5555_5a5a_0000`: its low 32 bits are not 0, so a 32-bit word cannot
  hide a missing record) and view 3 (every `WeakDyn` key defined at
  `0x6000_0000_0000 + 0x1000·n`). In view 3 every use must reach the
  definition, except a use whose field cannot hold any library address
  (R2: an input's 32-bit absolute use in a no-PIE executable), which
  fails only when another use does reach it (a split: W12, F2). A
  branch is never such a use: it reaches any definition through a PLT
  entry, so a branch of a `WeakDyn` key to 0, or past its PLT entry, is
  P-D9 view 3 whatever the field's range (WP-165; before, the fresh
  address's overflow of the branch's range hid it). Each unfillable use
  of the split key (D-25 ruling (A), `Exec`) is not a use here: it holds
  `Weak0`'s value whatever is loaded, which view 1 judges (§4.4: 0, an
  AArch64 PC-relative instruction `S = P`), and a dynamic record on its
  field is P-D9 "a dynamic record ... on an unfillable use ... of 'K'
  (D-25 ruling (A): an unfillable use of the split key keeps Weak0's
  value and has no record)" (WP-165). A use
  in a section without `SHF_ALLOC` is not a use here: nothing relocates
  it at run time, so view 3 does not judge it (view 1 does, by §5.8:
  WP-104, lld's exported weak key in a PIE). An import
  is `Target::Tls` at its fake TLS address; the kept GD, LD and
  descriptor sequences are judged against the GOT pair the loader wrote;
  GNU ld's x86-64 GD to IE form (`mov %fs:0,%rax; add G(%rip),%rax`) is
  the rewriting `tlsgd-to-ie`.
- **Exported IFUNCs** (D-23, W13): on x86 a library reference to an
  executable's IFUNC exported as `STT_GNU_IFUNC` makes the loader refuse
  (P-D7); on AArch64 such an export is P-D9 when the executable's own uses
  take an IPLT entry (two addresses), and accepted when every use is the
  `IRELATIVE` slot. An exported IFUNC nothing in the program uses
  (WP-088) is accepted when `.symtab`'s global of the name is
  `STT_GNU_IFUNC` at the same value (Y1 holds that value to the
  resolver): before, every reference's `-E` output of one was a false
  P-D7 "... which is no resolver of the program (W13)" (measured, `tests/dyn.rs`
  case `W13E`: GNU ld, lld and mold, PIE and `-no-pie`, both machines,
  all at the resolver), except mold's `-no-pie` outputs, which export it
  (and write `.symtab`'s entry) at 0xfffffff0 (AArch64) or 0xfffffff8
  (x86-64), in no section: S8, Y1 and P-D7, a defect of mold's. mortar
  refuses the link by name (`exported IFUNC ifx that no relocation names`,
  D-23), so W13 cannot fire on mortar's output there.
- **Mutations** (`dynmutate.rs`, §9.5's table): `dyn-needed`, `dyn-tag`,
  `dyn-rel-offset`, `dyn-rel-drop`, `dyn-rel-addend`, `dyn-rel-symbol`,
  `dyn-rel-order` (only swaps that move a `RELATIVE` or `IRELATIVE`),
  `dynsym-value`, `dynsym-visibility`, `dynsym-unique` (§12.3), `gnu-hash`, `versym`,
  `version-need`, `interp`, `relro`, `lazy-slot` (under `-z now` too,
  WP-082), `copy-size`, `copy-relro` (`PT_GNU_RELRO`'s end moved so a
  copy's membership flips) and `copy-section` (the copy's output section
  renamed: only `Format::Mortar`'s chunk rule reads the name, equivalent
  for a reference; both WP-088), and §12.2's `plt0-disp1`, `plt0-disp2`,
  `plt-index`, `plt-back-jump`, and §12.4's static-pie classes. `xtask plumb --mutate --line` judges each mutant with
  stage 3. The kill table and its survivors are WP-050's record.
- **Stage 2 inside a dynamic output**: `__rela_iplt_start` and
  `__rela_iplt_end` are equal (the loader applies `IRELATIVE`); a
  canonical `STT_FUNC` at an IPLT entry is Y1-accepted; GNU ld's AArch64
  `-z now` `_GLOBAL_OFFSET_TABLE_` is the `.got` word holding `_DYNAMIC`
  (GOT[0], after the PLT's slots); a GNU map's repeated `.eh_frame`
  placement of the first input is skipped; an output FDE covering a PLT
  section (GNU ld x86-64) is the linker's, judged by E6 (§6.4). The
  executable's own TLS block is the engine's, so a local-exec sequence
  against the executable's variable is R1's as in a static output
  (AArch64's large-model `movz`/`movk`/`movk`, `MOVW_TPREL_G2`/`G1_NC`/
  `G0_NC`, against a variable of another file: clean from the three
  linkers, a moved `G0_NC` immediate is R1; `tests/dyn.rs`
  `r1_a64_large_model_local_exec_in_a_dynamic_executable`, WP-088).

- **Static-pie** (WP-089): §12.4.

### 12.1a D-19a's order in the model (WP-193)

P-D6's expected `DT_NEEDED` order is computed from what the verifier
reads itself: the caller's line, the scripts it names (read from the
file system) and the inputs' symbol tables. Never from what the judged
linker writes of its mentions or groups.

- **The line.** It is the caller's words (`--line`, the lane's recorded
  words) for every format. A mortar map's `arg` records are the line
  only when the caller gives none; they are not compared with it here.
  A mention's state, group and position are never taken from a map.
- **The mentions** are the line's shared-object items, in order. Scripts
  are expanded inline at their own position, depth first: `INPUT`,
  `GROUP`, `AS_NEEDED`, a nested script, and `-l` inside a script
  searched as on the command line. A relative name is looked up in the
  current directory, then in the script's directory, then in the `-L`
  path.
- **A mention's as-needed state** is the one in force where the mention
  is named. For a mention inside a script, that is the state of the
  script's entry on the line (the script's own entry when it is
  nested). `AS_NEEDED(…)` forces it on. `--push-state` and
  `--pop-state` restore the saved state, and the state of an entry
  between them is its own.
- **A mention's group**:
  - a `--start-group` region, or a script's `GROUP`;
  - every entry of a nested script is in the group of the entry that
    named the script;
  - an `INPUT` entry of a script is in the group of the script's own
    entry. So beside a `GROUP` in a script named outside every group,
    it is in no group (`input_then_group`).
- **A mention again** (a mention, inside a script, of an object already
  met) has no state or group of its own. It takes those of its script
  entry, as above.
- **Each relocatable input's position and group** come from the line
  item that is that input. The map tells which file is which, by path.
  The map's path is taken as the same file as a line item when the two
  are equal once `.` components and repeated `/` are removed and each
  `dir/..` is collapsed (lexically). GNU ld names an object that a
  script found in the script's directory `./use_x.o`, and that is the
  line's `use_x.o`. mold's map names the driver's
  `/usr/lib/gcc/T/14/../../../T/crti.o` as `/usr/lib/T/crti.o`, and that
  is the line's word. An input that no line item is (any other
  spelling) is P-D2 "an input of the map that no line item is (D-19a
  needs its position)", and stage 3 stops.
- **The keeping mention, and the order**, are then those of `spec/dyn.md`
  D-19a. A strong reference from an object counts as a reason for an
  as-needed mention when either:
  - the object is at a position before the mention, or
  - the object is in the mention's group.

  A mention again inside a script `GROUP` therefore keeps a library that
  an object of the same `GROUP` references (`g_anx_obj`, `nested_group`,
  `nested_group_asn`). The fallback is the first mention. GNU ld's and
  mortar's outputs are held to this order. lld's and mold's outputs are
  held to the first-mention order (§13.1; mold's W9 where it applies).

Before WP-193, the matching was by the exact path, and an input it could
not place counted as being at position 0 with no group (before every
mention). GNU ld's `./use_x.o` was not placed, so it became a reason for
libnx.so's first, as-needed mention. That made P-D6 expect
`na nx nb nc c` on GNU ld's own correct list. mortar's map spells the
file `use_x.o`, so mortar's identical list passed. The verifier also took
a mortar map's `arg` records as the line even when the caller gave one.
In the lane those records are the caller's words verbatim, so the
verdict did not depend on them, but they were believed.

Tests: `tests/scriptorder.rs` (fixtures `tests/scriptorder/`), with
GNU ld's and mortar's AArch64 outputs of `g-anx-obj`, `nested-group` and
`s-twice-off`, and mortar's `input-then-group`:
- each output is clean;
- each list permuted to lld's order is P-D6;
- `input-then-group` with the list mortar's base wrote (`na nb nx nc
  c`) is P-D6;
- a mortar map whose `arg` records change libnx.so's state, put
  `use_x.o` in a command-line group, or are removed, or whose `use_x.o`
  is spelled `./use_x.o`, gives the same verdict on the same image. The
  first of these edits, given with no caller's line, is P-D6, which
  shows the edit is one the model would follow if it believed the map;
- GNU ld's map spelled as the line spells it gives the same verdict;
- an input spelled by no line item is P-D2.

`line.rs` `one_file_by_its_spellings`; the lane's
`dyn/shlib/needed-order-script*.test`.

### 12.2 The PLT header and the way back to it (WP-082)

P-D10 executes every PLT entry whose word carries a `JUMP_SLOT`
symbolically (`plt.rs`): from the entry to the indirect jump through its
own word, then from that word's initial content (the lazy target) to
the next indirect jump, the one into the resolver. What the resolver
receives is what glibc's trampolines read (`dl-trampoline.h`,
`dl-trampoline.S`). GOTPLT is `DT_PLTGOT`'s value; *k* is the index of
the entry's `JUMP_SLOT` record in `DT_JMPREL`.

- **x86-64.** The walk knows `endbr64`, `push imm32`, `push
  *rel32(%rip)`, `mov $imm32, %r11d` with `push %r11`, `jmp rel32`/`rel8`
  (with or without `bnd`) and `jmp *rel32(%rip)`. A `push` before the
  entry's own jump is a finding. At the resolver's jump the stack must
  hold, from the top, `GOTPLT[1]`'s word and *k* (GNU ld's and lld's
  `push $k; jmp PLT0`, mold's `%r11d` pushed by its header), and the
  jump must go through `GOTPLT + 16`. When the output's x86 feature
  property has IBT, the lazy target starts with `endbr64`. Padding after
  the jump is not judged.
- **AArch64.** The walk knows `nop`, `bti`, `stp x16, x30, [sp,
  #-16]!`, `adrp x16`, `ldr x17, [x16, #lo12]`, `add x16, x16, #lo12`
  and `br x17`. At the resolver's jump: `x17` was loaded from `GOTPLT +
  16` and `x16` is `GOTPLT + 16`, the link map being read at `x16 - 8`;
  the `x16` the header pushed equals the entry's `x16`, which is `GOTPLT
  + 24 + 8k`: the trampoline takes the index from it. When the output
  asks for BTI (`DT_AARCH64_BTI_PLT` or the feature property), the lazy
  target is a landing pad (`bti c`, `bti jc`, `paciasp`, `pacibsp`).
- **The reserved words.** `GOTPLT[1]` and `GOTPLT[2]` are 0 in the file
  and no dynamic record writes them (the loader fills them);
  `GOTPLT[0]` is `_DYNAMIC`'s address on x86-64 (all three references)
  and on AArch64 for mold; 0 for GNU ld and lld on AArch64 and for
  mortar (`spec/a64.md`). No `DT_PLTGOT` with a `JUMP_SLOT` record is a
  finding.
- **Shapes.** Every shape the references write is judged, lazily and
  under `-z now` (they all keep the lazy words then): GNU ld's and lld's
  `.plt` with `push $k`, mold's `%r11d` form, GNU ld's `-z ibtplt` and
  lld's `-z force-ibt` `.plt.sec` entries with their lazy stubs in
  `.plt`, mold's `-z ibt` form, GNU ld's and lld's BTI headers and
  entries. `.plt.got` entries (GNU ld, mold) jump through a `GLOB_DAT`
  word and have no lazy path: their word is judged as before (the word's
  record), nothing else. mortar refuses the IBT PLT and writes no
  `.plt.got`.
- **Mutations** (`dynmutate.rs`, sites found by the walk itself, a site
  shared by several entries mutated once): `plt0-disp1` (x86-64: the
  header's `push *rel32` ±8; AArch64: the header's `ldr` offset +8),
  `plt0-disp2` (x86-64: the resolver jump's rel32 ±8; AArch64: the
  header's `add` +8), `plt-index` (x86-64: `push $k` or `%r11d` +1;
  AArch64: the entry's `add` +8), `plt-back-jump` (x86-64: the lazy
  path's `jmp PLT0` ±16; AArch64 has none: the lazy word is its way
  back), `lazy-slot` (the lazy word + 4).

### 12.3 The byte sweep and the field rules (WP-082, WP-101)

**The sweep** (`sweep.rs`, `xtask plumb --sweep`) flips bit 0 and bit 7
of every byte of the synthetic sections (`.dynamic`, `.dynsym`,
`.dynstr`, `.gnu.hash`, `.gnu.version`, `.gnu.version_d` (WP-161), `.gnu.version_r`, `.rela.dyn`,
`.rela.plt`, `.plt`, `.plt.got`, `.plt.sec`, `.iplt`, `.got`, `.got.plt`,
`.interp`, `.eh_frame_hdr`, `.eh_frame`, `.gdb_index` (WP-098), stage
4's `.note.mason` and the kept `.mason.meta` (§13), and the descriptor
of `.note.gnu.build-id`) one mutant at a time and judges each. A mutant
is caught when a finding key (rule, address, message) appears that the
unmutated output does not have (a panic counts as caught).

**What it measures** (WP-101). In an output with mortar's build id (B1,
§6.7: a hash of the whole file with the id zeroed) every unsealed flip
is a B1 finding, so before WP-101 the sweep measured B1 and nothing
else: "0 survivors" on mortar's outputs (WP-082, WP-088) was B1's. Now,
after each flip of a byte outside the id, the sweep reseals the id with
the mutation tool's function (`buildid::reseal`, §9.5: mortar's map
only; a reference's id is left as its linker wrote it, so a reference's
sweep measures the same as before), and records for each flip the rules
whose findings are new. Two cases keep B1 judged: the id's own bytes
are flipped and never resealed (each must be B1's), and one flip of
`e_ident`'s last padding byte is judged unsealed (B1 must be among its
rules). `--no-reseal` turns the reseal off (to measure B1 alone). It
prints per section and field the bytes and survivors, each surviving
range (marked `equivalent` when its field is in the table below), then
`flips N, caught M, survived K (E of fields named equivalent)`, and per
rule the flips it caught and those it alone caught.

**The equivalent fields.** A survivor is closed either by a rule (below,
and "The field rules of WP-101") or by naming its field here, with who
reads the bytes and why no correct consumer can act otherwise. By field
(the sweep's `kind`), never by offset; `sweep::EQUIVALENT` holds the same
rows and the lane (harness.md "ci") fails on a survivor of any other
field.

| section | field | reason |
|---|---|---|
| `.plt` | code no path executes | Bytes of the PLT that no walk of §12.2 reaches (from every entry `loader::plt_entries` finds, to the jump through its word and on from the word's initial content to the next indirect jump; `plt::executed`): the AArch64 header's three `nop`s after `br x17`, x86-64's padding after the header's `jmp *GOTPLT+16`. Every path through the PLT ends in an unconditional indirect jump before them, and every input branch into the PLT is judged to reach an entry (R1-R4), so no instruction there is ever fetched. |

| `.dynamic` | d_val DT_SYMBOLIC | The gABI's table of dynamic array tags gives `DT_SYMBOLIC`'s `d_un` as "ignored": a loader reads the tag's presence only (glibc sets `l_info[DT_SYMBOLIC]` and tests it for non-null). mortar writes 0 (measured; D-102 does not state the value); no consumer can act on it (WP-161). |

**The lane's cases** (WP-101, resealed). The `gdbindex` case (WP-098)
sweeps `.gdb_index` only, of a PIE linked `--gdb-index --build-id=none`.
Measured on both targets after WP-101's rules: every case but the
equivalent `.plt` bytes is caught (`spec/landed/plumb/WP-101.md` has the
table before and after).
The `Shared` cases (WP-161; in `--quick` too): `shared`, a `-shared
-fPIC` library of two version nodes (`--version-script`, `-soname`)
with an exported and a local TLS variable reached through TLS
descriptors (AArch64 `-mtls-dialect=desc`, x86-64 `gnu2`) and a PLT
call; `shared.tail`, the same source unversioned (every default global
exported) under the option tail: the five D-103 `-z` words,
`-Bsymbolic`, `-init`/`-fini`, `--exclude-libs ALL`. The survivors
WP-128 measured, and how each is closed: `DT_SYMBOLIC`'s `d_val`
(equivalent, the row above); `DT_FLAGS`' `DF_ORIGIN` (P-D22, §12.5);
bit 7 of an export's `st_other` (P-D7: an export's
`STO_AARCH64_VARIANT_PCS` is its input definition's, AAELF64; on x86-64
the bit is P-D7's already); the exports' `.gnu.version` entries (P-D8
and P-D27 catch every flip in both cases); a TLS descriptor's two `.got`
words (P-D9 **(mortar)**: both hold 0 in the file, D-119's "place 0
under RELA"; the loader writes both, so for a reference they are
equivalent bytes and not judged); `.plt` padding (equivalent, the first
row). The sweep found one more: `.dynsym` entry 0's `st_name` flipped
to an offset at a NUL of `.dynstr` read as the empty name, so P-D7's
"entry 0 is all zero" now reads the field, not the string. Measured
(WP-161, load about 20, 4 threads): about 20 s per case per target
(AArch64 1815 and 1696 bytes, x86-64 1712 and 1593), every flip caught
but the equivalent fields'.
`shared.table` (WP-164; in `--quick` too): the versioned library of
`shared` with test-producer units (`sw_old`, `sw_new` `SYSTEM`,
`sw_data`, the script-local `sw_hidden`, a type) under
`-Bsymbolic-functions`, `--odr-table=full`, `--keep-mason-meta` and
`--build-id=none`, over `.note.mason` and the kept `.mason.meta` only:
the table's rows carry both values of `NONPREEMPTIBLE` (the functions'
set, the variable's clear, O-60), `=full`'s note the local definition.
Measured (load about 11, 4 threads): 576 bytes, 1152 flips per target,
every one caught (P-M1 720, P-M2 432, S11 30; no survivor, no field
named equivalent), 3.1 s on AArch64 and 3.6 s on x86-64.

**The field rules** (`dyncheck.rs` `fields`, every format unless marked):

- **P-D5.** No non-zero bucket is below `symoffset`.
- **P-D6.** No tag repeats but `DT_NEEDED` and `DT_NULL`; no unknown tag;
  the first `DT_NULL` has value 0; `DT_DEBUG` is 0. A tag is present
  exactly when its section is: `DT_INIT`/`DT_FINI` (values the section
  starts; under `-init`/`-fini` the named function's value instead, and
  presence not judged), the three array tags with their sizes, `DT_GNU_HASH`,
  `DT_VERSYM`, `DT_VERNEED`/`DT_VERNEEDNUM`, `DT_JMPREL`/`DT_PLTRELSZ`/
  `DT_PLTREL`; `DT_RELA`/`DT_RELASZ`/`DT_RELAENT` with `.rela.dyn`;
  `DT_STRTAB`, `DT_STRSZ`, `DT_SYMTAB`, `DT_SYMENT` always. `DT_FLAGS`
  holds only `DF_ORIGIN`, `DF_TEXTREL` (references; D-47), `DF_BIND_NOW`, `DF_STATIC_TLS`, and
  `DT_FLAGS_1` only `DF_1_NOW`, `DF_1_PIE`. **(mortar)** `DT_FLAGS` is
  present exactly under `-z now` and is then exactly `DF_BIND_NOW`;
  `DT_RELACOUNT` is present exactly when a `RELATIVE` record is.
  Throughout stage 3, `-z now` is the effective one (§13.1, WP-197): the
  line's last of `-z now`/`-z lazy`, or, for a mortar map whose line asks
  for the ODR runtime and says neither, `-z now` (cli.md C-ODR-RT:
  `DF_BIND_NOW` and `DF_1_NOW`, and P-D3's `bind now`); with `-z lazy`
  the output is as without the option.
- **P-D7.** `.dynsym` entry 0 is all zero (its `st_name` field, not the
  string it names: WP-161); `st_other` has no bits beyond
  the visibility (AArch64: also `STO_AARCH64_VARIANT_PCS`, 0x80, which a
  defined export has exactly when an input definition of its name has
  it, WP-161; a
  `JUMP_SLOT` of such a symbol needs `DT_AARCH64_VARIANT_PCS`, AAELF64). An
  import is `STV_DEFAULT`, `STB_GLOBAL` or `STB_WEAK`, and named by a
  reference of the program (or an alias, at the same library value, of a
  referenced name). A defined export is `STB_GLOBAL`, `STB_WEAK` or
  `STB_GNU_UNIQUE`; the section it names holds its value (an
  `STT_SECTION` entry: the section's start; not judged for reserved
  indices, `STT_TLS` and a value at a PLT entry); its size, section and
  `st_info` equal its `.symtab` entry's (an IFUNC's are P-D7's own). Its
  `.symtab` entry is the first non-local defined one whose name is the
  export's and whose value is the export's; with none, the first global
  of the name (WP-149: lld's and mold's `foo@V1` and `foo@@V2` are two
  entries named `foo`, the first of which need not be the export's). For
  a `.symver` key (an input defines `foo@V1` or `foo@@V2`) mortar's and
  GNU ld's entry is the one named `foo@V1`/`foo@@V2` by the export's
  `.gnu.version`, at its value, and none is P-D7 (synth S-75; §6's
  "A key the line makes local, in `.symtab`", WP-169); lld's and mold's
  are matched by either name. An exported key whose only `.symtab`
  definition at its value is `STB_LOCAL` is P-D7. A name stands twice in `.dynsym` (every
  format; STB_LOCAL entries aside) exactly when the entries' version
  indices differ and at most one of them is not `VERSYM_HIDDEN` (WP-149,
  D-115: `foo@V1` and `foo@@V2`); any other repeat is P-D7. A
  name every input defines `STB_GNU_UNIQUE` is exported `STB_GNU_UNIQUE`
  (dyn.md D-23's export row: glibc's unique table then holds the
  program's instance, which an `RTLD_DEEPBIND` library binds to); mortar's
  `EI_OSABI` is then S1's (lld and mold write 0). The `STT_SECTION`
  entries are GNU ld's (its AArch64 PIE writes `.init` and `.data`;
  mortar writes none, D-21): no loader looks one up, so only their
  section and value are judged; their name, `st_info`'s binding,
  `st_other` and size are equivalent mutants (WP-088: the sweep of GNU
  ld's AArch64 C and C++ PIEs leaves no `st_value` or `st_shndx`
  survivor). Test: `tests/dyn.rs` `s12_p_d7_dynsym_section_symbols`
  (case `secsym`: the last section symbol's value + 8, its `st_shndx`
  + 1; each P-D7, neither found with the check dropped).
- **P-D8.** `.gnu.version` entry 0 is 0; every Verneed has `vn_version`
  1 and `vn_cnt` its chain's length; the last Verneed's `vn_next` and
  each chain's last `vna_next` are 0; `vna_flags` is 0 or `VER_FLG_WEAK`.
- **P-D9.** `RELATIVE` and `IRELATIVE` records name symbol 0; on x86-64
  `GLOB_DAT`, `JUMP_SLOT` and `COPY` records have addend 0 (the psABI
  computes S; glibc's lazy path adds a `JUMP_SLOT`'s addend).
- **P-D10.** An import's non-zero `st_value` is a PLT entry; `DT_JMPREL`
  holds only `JUMP_SLOT`, `IRELATIVE` and `TLSDESC` records. An AArch64
  PLT entry is `adrp x16; ldr x17, [x16, #lo12]; [add x16, x16,
  #lo12;] br x17`.
- **R1, x86-64 (X-58).** A TLSDESC sequence relaxed to initial-exec in a
  dynamic output is `mov G(%rip), %reg` into any of the sixteen
  registers (REX.W and ModRM kept, `8d` made `8b`), G a GOT entry with
  the symbol's `TPOFF64` record.

**The field rules of WP-101** (`dynfields.rs`, and where named
`dynverify.rs`, `eh.rs`): what the resealed sweep showed no rule judged.
Each was written here before its code; the ones marked **(mortar)** hold
`Format::Mortar` only, where `spec/dyn.md`, `a64.md` or `x64.md` gives
mortar a single answer and the references differ or are not ruled.

- **P-D5 (mortar), D-26.** The Bloom words are exactly the two bits of
  each hashed entry (a cleared bit is every format's finding already: the
  name's lookup fails; a bit no entry sets only sends a lookup on to the
  chains, which no correct loader can tell from a true hit). `symoffset`
  is the index of the first hashed entry (the `.dynsym` count when none
  is hashed).
- **P-D6 (mortar), D-30.** Every `DT_NULL` from the first to the end of
  `PT_DYNAMIC` has value 0 (the first's is every format's; the spare
  ones' `d_un` the gABI says is ignored).
- **P-D7 (mortar), D-23.** An import (not under `-static-pie`) has
  `st_size` 0; `STB_WEAK` exactly when no input references it strongly
  and the line does not `-u` it; type `STT_FUNC` for a function or IFUNC
  definition or a canonical PLT, else the definition's, `STT_NOTYPE` when
  nothing defines it (`WeakDyn`; in `Shared`, when no member of N
  defines it, D-10 and P-D19, WP-140); on AArch64 `STO_AARCH64_VARIANT_PCS`
  exactly when the definition has it or, with none, an input's undefined
  reference has it. A defined entry at a `COPY` record's place (the copy
  and each alias) has the shared definition's type and `st_size`, and
  `STV_DEFAULT` (D-23's copy row).
- **P-D8.** Every format: a defined global or weak entry (a local one,
  GNU ld's `STT_SECTION` entries, is index 0 by right) with version index 0 (gABI
  `VER_NDX_LOCAL`: musl skips it, glibc does not, so the export is one a
  loader may refuse). An import's index 0 is read as 1 (unversioned:
  glibc reads 0 and 1 alike for a reference, musl reads no reference's
  index), and
  **(mortar)** is a finding (D-27 writes 1).
- **P-D9.** Every format: a `RELATIVE` record on a word that no input's
  absolute use makes an address is the record of a GOT slot an input
  demands (D-40, D-46): its place is an aligned word of `.got` and its
  addend the value some GOT-indirect reference of the inputs asks a slot
  to hold (relaxed or not, a TLS reference's TPREL word or module pair
  included, a kept x86-64 GD or LD sequence's pair too, a bound key's
  static DTP offset with it, WP-144; `reloc.rs`'s `got_demands`), or an IPLT entry (D-45: an
  entry whose word carries an `IRELATIVE`, in `.plt` or `.iplt`). For a
  reference the value of `_GLOBAL_OFFSET_TABLE_` is accepted too: mold
  gives an x86-64 `GOTPC64` reference to it a GOT slot of that symbol
  which nothing loads (measured, WP-101: `weak-class-table`, the
  large-model tests; harmless, so not an anomaly). Before WP-101 any
  addend inside a `PT_LOAD` passed. In a stripped output (no `.symtab`;
  WP-164) a GOT-indirect reference to a name only the linker defines
  (`_GLOBAL_OFFSET_TABLE_`, `__ehdr_start`, `__start_X`/`__stop_X`, an
  undefined weak) is `symbol-unresolved` (§4.4: no symbol to read), so
  the demands are not all known: a slot's addend that no known demand
  asks for is then not judged and counted in the coverage line ("P-D9: N
  .got slot addends (a GOT-indirect reference to a name only the linker
  defines is unresolved)"); with every reference resolved the clause is
  judged as above (measured: mold's stripped libraries and PIEs, both
  targets, before this sentence a false finding on every one).
  **(mortar)**, D-46: a record's place holds in the file `S + A` for
  `RELATIVE` (the addend) and 0 for `GLOB_DAT`, the symbolic word type
  and `TPOFF`, and a `TLSDESC` record's descriptor 0 in both words (D-119;
  WP-161); an `IRELATIVE` record's place (the IPLT word) holds the
  resolver's address on AArch64 (a64.md §10.2) and 0 on x86-64 (x64.md
  X-39); a `.got` word no record writes (AArch64's `.got[0]` aside)
  holds a value an input's GOT reference demands. For a reference these
  bytes are equivalent (every RELA type of the two machines but a lazy
  `JUMP_SLOT` computes the word from S, A and the base and ignores the
  place), so the rule is mortar's alone.
- **P-D10 (mortar).** AArch64's `.got[0]` holds `_DYNAMIC`'s link-time
  address and no record writes it (D-32); `DT_PLTGOT` is `.got.plt`'s
  address and `.got.plt[0..3]` are (x86-64 `_DYNAMIC`, else 0), 0, 0
  with no record on them, in every kind, a static-pie's included (D-32,
  D-49; §12.2 judges them only with a `JUMP_SLOT`); no `JUMP_SLOT` record
  is outside `DT_JMPREL` (D-41); every AArch64 PLT and IPLT entry's third
  instruction is `add x16, x16, #lo12(G)` (a64.md §10.3, §10.4: the walk
  judges it where the lazy path reads `x16`; an IPLT entry's `x16`
  reaches only the chosen function, where IP0 is scratch, so only
  mortar's single answer holds it).
- **E5, every format.** `.eh_frame_hdr`'s encodings with
  `DW_EH_PE_indirect` (0x80) are read as libgcc's
  `read_encoded_value_with_base` and LLVM libunwind's `getEncodedP` read
  them: the value is the word at the decoded address. The flag was
  ignored, and a header no unwinder reads as written passed.

**Mutations.** `dynsym-unique`: an exported `STB_GNU_UNIQUE` definition
made `STB_GLOBAL` in `.dynsym` and in its `.symtab` entry (WP-054's case
(a)), caught by P-D7.

### 12.4 Static-pie (WP-089)

A static-pie (D-1's `StaticPie`: `ET_DYN`, `PT_DYNAMIC`, no `PT_INTERP`)
has a `.dynamic` for one reader, glibc's `_dl_relocate_static_pie`
(`elf/dl-reloc-static-pie.c`, 2.41), and nothing a loader reads (D-2).
That function takes the load bias from `&__ehdr_start`, finds `.dynamic`
through `_DYNAMIC`, and runs `ELF_DYNAMIC_RELOCATE` with lazy 0 (every
`DT_JMPREL` record is applied at start-up, whatever `-z now` says), its
symbol lookup answering with the executable itself; TLS is set up after
it. `csu/libc-start.c` then applies `[__rela_iplt_start,
__rela_iplt_end)` with `apply_irel`, which adds no load bias. The
verifier judges a static-pie with the whole of stage 3 (the views, the
model, P-D4..P-D13) plus the rules below, with `-E` or
`--export-dynamic-symbol` on its line as well (WP-088; WP-056 measured
the exports useless: glibc's static `dlopen` binds no library to them and
`dlsym(RTLD_DEFAULT)` does not find them); an i386 static-pie is outside
stage 3 (REL records; §12.1, one P-D1 line), and `.dynsym`'s set is
P-D14's, not P-D7's (nor W13's exported-IFUNC rule: nothing binds an
export).

- **P-D14, the headers.** The first `PT_LOAD` maps the ELF header at
  address 0 (`p_vaddr` and `p_offset` 0) and `__ehdr_start`, where
  `.symtab` defines it, is 0: glibc's bias is `&__ehdr_start`. The entry
  point is in an executable `PT_LOAD` and is the entry symbol's value
  (`-e`, else `_start`) where `.symtab` defines it (S7 judges `ET_EXEC`
  only). `Format::Mortar`: no `PT_PHDR` (D-1, layout L-67; lld and mold
  write one, GNU ld none).
- **P-D14, `.dynamic`.** No `DT_NEEDED` (a static-pie loads nothing),
  no `DT_VERSYM` (glibc's self-relocation would take the versioned path),
  no `DT_VERNEED`, `DT_VERNEEDNUM`, `DT_VERDEF`, and no `DT_RELR` (stage 3
  does not model it). `DT_TEXTREL`, the address/size pairs, the flags
  (`DF_1_PIE`, `-z now`'s two) and `DT_RELACOUNT` are P-D6's as for a
  `Pie`. The references' tag sets (measured, both architectures): GNU ld
  `INIT FINI INIT_ARRAY(SZ) FINI_ARRAY(SZ) GNU_HASH STRTAB SYMTAB STRSZ
  SYMENT DEBUG PLTGOT PLTRELSZ PLTREL JMPREL RELA RELASZ RELAENT [FLAGS]
  FLAGS_1 RELACOUNT`; lld `FLAGS_1 DEBUG RELA* RELACOUNT SYMTAB SYMENT
  STRTAB STRSZ GNU_HASH`, the arrays, `INIT FINI [FLAGS]`; mold `RELA*
  PLTGOT SYMTAB SYMENT STRTAB STRSZ`, the arrays, `INIT FINI GNU_HASH
  FLAGS FLAGS_1 DEBUG` (no `DT_RELACOUNT`; on AArch64 `DF_STATIC_TLS`).
  `Format::Mortar` is D-30's set and order (P-D6).
- **P-D14, `.dynsym`.** No defined entry (nothing can look one up),
  except GNU ld's AArch64 `STT_SECTION` entries (`.init`, `.data`) in a
  reference's output; `Format::Mortar`: the null entry only (D-1),
  whatever the line says (`-E` and `--export-dynamic-symbol` have no
  effect, D-1). An undefined entry in a reference's output is inert (no
  record names one: lld 19's x86-64 `__tls_get_addr`). `.gnu.hash` is
  P-D5's. With `-E` or `--export-dynamic-symbol` on the line a
  reference writes the exports (measured, WP-088, hello: GNU ld 439
  AArch64 / 457 x86-64 global entries, lld and mold 432 / 454; with
  `--export-dynamic-symbol=main` the one name), and each defined entry
  that is not a section symbol is held to the program's definition: the
  value of `.symtab`'s global of that name, a name `.symtab` defines
  (when it has one), and not `STV_HIDDEN` or `STV_INTERNAL`. The records
  stay this rule's: GNU ld's x86-64 `-E` output carries `TPOFF64`
  records against exported TLS variables and faults at start-up (A34).
- **P-D14, the records.** Every record of an allocated `SHT_RELA`
  section is `RELATIVE` or `IRELATIVE` with symbol 0, or `R_*_NONE`
  (GNU ld writes two on AArch64, mold two on x86-64; `Format::Mortar`:
  none, D-40). A TLS record is named: glibc sets the thread pointer up
  after self-relocation. Any other type is refused even though glibc's
  lookup would answer with the executable: no reference writes one.
- **P-D14, the GOT's `_DYNAMIC` word (D-32).** Where `.symtab` defines
  `_GLOBAL_OFFSET_TABLE_`, the word there holds `.dynamic`'s link-time
  address and no record writes it (all three references, both
  architectures, lazy and `-z now`; lld's AArch64 output defines no
  `_GLOBAL_OFFSET_TABLE_`). `Format::Mortar`: x86-64 `.got.plt[0]` and
  AArch64 `.got[0]` hold `_DYNAMIC`.
- **P-D15, application.** glibc's ranges (`elf/dynamic-link.h`,
  `_ELF_DYNAMIC_DO_RELOC`): range 0 is `DT_RELA`/`DT_RELASZ`; when
  `DT_JMPREL` is present it becomes range 0's start if `DT_RELA` is
  absent, is cut off range 0's end when range 0 ends where it does, joins
  range 0 when it starts where range 0 ends, and is range 1 otherwise.
  glibc reads `DT_JMPREL` only under `DT_PLTREL`, and the RELA pass only
  when `DT_PLTREL` is `DT_RELA` (7): a `DT_JMPREL` without it is a table
  glibc does not apply, and its records are "does not apply".
  Every `RELATIVE` and `IRELATIVE` record of an allocated `SHT_RELA`
  section is in exactly one range (0: "does not apply"; 2: "applies n
  times"), and a range covers records only. The first `DT_RELACOUNT`
  records of range 0 are applied as `RELATIVE` whatever their type
  (P-D6 holds the count to the leading run).
- **P-D15, order (measured).** No `RELATIVE` record is applied after an
  `IRELATIVE` one, the order being glibc's: range 0, then range 1; on
  x86-64 each range's `IRELATIVE` records are deferred to its end
  (`ELF_MACHINE_IRELATIVE`), on AArch64 a range is applied in table order.
  Measured with `tests/spie/order.c`, whose resolver picks the right
  function only if its pointer's `RELATIVE` record was applied first:
  lld's and mold's tables rewritten with the `IRELATIVE` records first
  (`DT_RELACOUNT` 0) run right on x86-64 and wrong on AArch64; GNU ld's
  `DT_RELA` and `DT_JMPREL` swapped (the `IRELATIVE` table as range 0)
  runs wrong on x86-64 and faults on AArch64 (glibc's own resolvers).
  The references as written all apply every `IRELATIVE` last.
- **P-D15, the C runtime's range.** `__rela_iplt_start` and
  `__rela_iplt_end`, where `.symtab` defines both, are equal (lld leaves
  them undefined, GNU ld absent, mold absolute 0): a non-empty range is
  applied without the load bias, and twice when it covers records the
  self-relocation applies. Stage 2's `Want::Equal` holds the same in
  every dynamic output (§12.1).
- **IFUNC (§4.8 in a static-pie).** An IPLT word has one `IRELATIVE`
  record whose addend is the resolver (F2), counted per entry by P-D10;
  a PLT entry of an undefined weak function may jump through an aligned
  word holding 0 that no record writes (it reaches 0 at any bias, as the
  call does in a static executable: GNU ld x86-64). An input's address
  word against an IFUNC may be its own `IRELATIVE` slot of that resolver
  (x86.md X86-50's form; GNU ld and mold): it is the resolved function,
  F3 when another use takes an IPLT entry (A32), and an `IRELATIVE` slot
  inside a placed section that no such word claims is F2. P-D10's header
  and lazy walk (§12.2) and the lazy-binding record rule do not apply:
  nothing is bound lazily.
- **No record on a non-address word (P-D9, static-pie and `Pie`).** Every
  `RELATIVE` record's place is a word the verifier knows holds an address
  of the executable: an input's absolute use of an internal target
  (direct, through a mergeable piece, an IFUNC's canonical IPLT entry),
  or a GOT slot such a use reaches; outside the placed sections (a
  linker-made word, such as a GOT slot no relaxed reference still uses)
  the record's addend must be an address inside a `PT_LOAD`. View 2
  checks the other direction (every address word moves) and now also
  holds an absolute symbol's word (`SHN_ABS`, glibc's
  `_nl_current_*_used`) to its value at any bias, in a `Pie` as well:
  A30.
  The direction from a record to its word holds in a `Pie` as well
  (WP-088): the references' corpus and §12.1's PIE cases are clean, and
  each linker's `-pie` output of `tests/dyn.rs`'s `relx.c` with `rx_p`'s
  record moved onto `rx_v`, its addend the number `rx_v` holds, is this
  rule's on both machines (missed with the rule kept to static-pie; lld,
  which leaves a `RELATIVE` word 0 in the file, is R1 as well, since
  `rx_p` then holds 0).
- **What is stricter for `Format::Mortar`.** No `PT_PHDR`; the null
  `.dynsym` entry only; no `R_*_NONE`; GOT[0] holds `_DYNAMIC` at D-32's
  place; `DT_RELACOUNT` exactly the leading run; D-30's tags in order.

Mutation classes (`dynmutate.rs`; kill table in WP-089's record):
`spie-rel-drop` (a `RELATIVE` record removed, the table, `DT_RELASZ`,
`DT_RELACOUNT` and `sh_size` adjusted), `spie-rel-extra` (an `R_*_NONE`
made a `RELATIVE` on a `.data` word no record writes, view 1 unchanged),
`spie-irel-twice` (the `__rela_iplt_*` range set over an `IRELATIVE`),
`spie-interp` (a `PT_NOTE` made `PT_INTERP`), `spie-needed` (`DT_DEBUG`
made `DT_NEEDED`), `spie-relacount` (plus and minus one; minus one is
equivalent for a reference: glibc applies that record as an ordinary
one), `spie-dynamic-word` (the GOT's `_DYNAMIC` word 0 or plus 8).

### 12.5 Shared objects (M4; planned: WP-121)

Requests P-R1-P-R11 of `spec/dyn.md` §18.3, applied by WP-115 as rules
P-D16-P-D26 (P-R*n* is P-D(15+*n*)); P-D27 (version definitions) is
WP-149's. Until WP-121 lands, P-D1 rejects
an `ET_DYN` without `DF_1_PIE` and without `PT_INTERP` that is not a
static-pie, as today. The `D-` rules cited are `spec/dyn.md` §17's; D-117
items 1-9 are its "Bound use through a register" and are cited, not
restated: the verifier implements them from that text.

A shared object cannot be judged "resolves as the program assumed" from
the library alone: what a preemptible reference binds to depends on the
process (D-112). **The interposer stands in for the host.** P-D9's
loader model runs with a synthetic first module, the interposer, in
place of an executable. It defines **every key the verifier computes as
exported** (D-110: the whole export set, preemptible and bound alike)
**and every key the inputs import**, each at a fresh address: a
function at a fresh code address, data at a fresh allocation of its
`st_size`, a TLS key at a fresh offset in a second module's block, as a
preloaded library would. Both sets and each key's binding are
**recomputed** from the inputs and the map's record of the line (the
`-Bsymbolic` forms in order, `--dynamic-list` and version-script files,
`--exclude-libs`, `-soname`, `-init`/`-fini`: D-110, D-112, D-114),
never read from the output's `.dynsym`, `.gnu.version_d` or
`DF_SYMBOLIC`. Two views:

- **view A**: no interposer; a defined key resolves to the library's
  own definition, an import to its definition in the shared inputs
  (P-D2), at P-D9's two load biases. View A has no host: an unversioned
  import no shared input defines is the host's to resolve (D-118, legal
  without `-z defs`/`--no-undefined`), so neither P-D9's "the loader
  finds no definition" nor P-D7's "no needed or used object defines it"
  is reported for it, and its places are judged in view B; under `-z
  defs`, and for a versioned import (W42), both stay findings. Both are
  decided from the verifier's sets, never from the output's `.dynsym`
  or `.gnu.version`: an import is a key no live input defines, versioned
  when an input reference names a version; `__start_X`/`__stop_X` that
  a live section's relocation names, X an allocated section of the
  inputs, is the link's own (layout L-96 keeps every input section X
  live), never an import (W23; WP-121);
- **view B**: the interposer first in lookup order.

The loader model resolves every record of the output by the symbol it
names, as glibc would. Each input relocation's place must hold, in both
views, the value the input computes with `S` the **expected**
definition: the library's own for a key the verifier computes as bound,
in both views and whatever the interposer defines; the interposer's for
a preemptible key or an import in view B. So a record that names a
bound key resolves to the interposer in view B and its place differs
from the expected value, and P-D25 names the cause. This is P-D9's view
3 generalised from `WeakDyn` keys to every exported and imported key,
and what turns P-D9's equality into a check of D-112.

- **P-D16 (accept; request P-R1).** P-D1 accepts `ET_DYN` without
  `DF_1_PIE` and without `PT_INTERP`, and with no static-pie line
  (`-static-pie` absent from the map's line), as `Shared`; the kind is
  recomputed from the headers and the line (`-shared`) and must agree.
  P-D2-P-D15 apply with this section's changes: P-D4 has no `PT_PHDR`
  and no `PT_INTERP` rule for `Shared` (D-100: mortar writes neither; a
  reference's `PT_PHDR` is accepted); P-D9 runs views A and B in place
  of its views 1 and 3 (view 2, a second load bias, stays, within view
  A); P-D12 is replaced by P-D26; P-D7's set is P-D19's.
- **P-D17 (the views; request P-R2).** *Recomputed:* for every relocation
  of an `SHF_ALLOC` place of a live input (`.eh_frame` and the LSDA
  included: checked by value, not by the address clause), the expected
  value in each view. *Read:* the place's bytes after the loader model
  applies the output's records. *Reported:* a place whose value differs
  in either view, naming the input relocation, the view and the key;
  view A's findings keep stage 2's ids (R1 a field's value, R2 its
  range), view B's are P-D17's. An output without a section X that
  L-96 keeps live for a `__start_X`/`__stop_X` an input relocation
  names is reported (W23: the input computes X's address).
  **The address clause** (D-117): a use whose named address lies in a
  preemptible data definition is reported even when both views agree
  on its value. The named address is computed by the verifier's own
  implementation, written from D-117's text, of (a) the relocation-level
  rule with its scope (exact fields only; on x86 no PC-relative code
  field is judged, so there is no x86 instruction decoder in the
  verifier), its boundary rule and its "before the start" paragraph (a
  known refusal of correct code, reported like the linker's), and (b)
  on AArch64 the analysis, D-117 items 1-9 (an AArch64 def-use decoder
  of the verifier's own). Each part is checked, off the gate, against
  the oracle `tests/shared-design/exp3/s1d.py` on the record's corpora
  (same refused objects, same definitions). *Not checked in view B:*
  non-alloc places (debug sections hold link-time addresses by design:
  no interposer moves them). View A judges them as §5.8 does for any
  dynamic output: S is the library's own definition for a key it
  defines (§4.4 step 1), and for an import §5.8's "A target only a
  shared library defines", which in a `Shared` output is 0 (there is no
  copy and no canonical PLT; WP-121). *Not checked:* the residual hole
  D-117 names (asserted not caught, by a test).
- **P-D18 (IFUNC; request P-R3).** Every use of one IFUNC key, in every
  place and both views, yields one address: the IPLT entry for a bound
  key, the interposer's or the resolver's record for a preemptible one
  (W21). For a bound key this is stage 2's F3, reported under its id. An i386 `Shared` output has no `STT_GNU_IFUNC` symbol and no
  `IRELATIVE` record (D-130).
- **P-D19 (`.dynsym`; request P-R4).** *Recomputed:* the export set
  (D-110, `__start_`/`__stop_` `PROTECTED` included) with each entry's
  binding, type, visibility and version (D-115). *Read:* `.dynsym`'s
  defined entries and `.gnu.version`. *Reported:* a defined entry
  missing, extra or with a field or version that differs. A key every
  definition of which has no place (§4.4 step 0's predicate: each in a
  section discarded or absent; a COMDAT copy another object keeps is not
  the key's definition) is not in the export set (`spec/resolve.md`
  §10.1 P-1: an unreferenced placeless symbol is omitted from `.dynsym`,
  D-110; WP-147). Otherwise the key's definition, whose binding, type
  and size the entry carries, is chosen among the definitions WITH a
  place: the first strong one in object order, else the first not
  common. lld and mold write no line for a discarded COMDAT copy, so
  under their maps the copy is absent and may come before the kept one
  in object order (WP-147b: Debian's unoptimised cross `libstdc++.a`
  has a weak copy of an inline function in up to ten members; before
  WP-147b the first copy was taken as the definition and the export of
  the kept one reported as placeless). Under mortar's map such a key defined in `.dynsym`, in any
  form, is a finding ("its definition has no place"). Under a
  reference's, what each writes is accepted as measured (an unreferenced
  global in `.note.gnu.property`, `.note.GNU-stack`, an `SHF_EXCLUDE`
  section and an input build-id note, `-shared` and `-pie
  --export-dynamic`, AArch64 and x86-64): GNU ld an `SHN_ABS` symbol
  whose value is the definition's offset in its section (its
  `.note.GNU-stack` global it places at the end of `.text`, an ordinary
  export); lld an undefined symbol under `-shared` (P-D7's "imported and
  no input references it" is not reported for it) and nothing under
  `-pie`; mold nothing (its `SHF_EXCLUDE` section it places, an ordinary
  export). GNU ld's form at another value is a finding. Before WP-147 the
  set held these keys: mortar's library without them was "in the export
  set and not defined in .dynsym". An executable's exports
  (`-pie --export-dynamic`) are not P-D19's: not judged. Test:
  `tests/pexport.rs`. The set is one derivation (WP-149,
  `shared::ExportRule`): P-D19's keys, P-1's placeless keys (a key every
  definition of which has no place is placeless only when the script
  exports it; one the script makes local is local) and M10's export
  roots (§3.3) all ask it. Its rule 12 (resolve.md §9.7, a `.symver`
  key under the script) is each format's, as measured (dyn.md §17.8):
  mortar's and GNU ld's as resolve §9.7 states it; mold never makes a
  `.symver` key local; lld makes `foo@V` local when a `local:` entry of
  node V matches `foo` and no exact `global:` entry of V names it,
  whatever `-E` says, and `foo@@V` only for an exact `local:` entry.
  A hidden `.symver` key the script makes local is the local key
  `foo@V1` (the plain `foo` may still be another node's export). Imports are
  P-D7's import rule and P-D8, unchanged, except that an import's
  static binding, `bind(k)`, ranges over N alone (`spec/dyn.md` D-10 as
  WP-137 amends it; WP-140): a weak-only key whose every definition lies
  in an object that is not needed is unbound, D-111 item 3's `WEAK
  NOTYPE`, unversioned (GNU ld; lld's `FUNC` and mold's needed object and
  version are reference forms, judged as before). The loader's run-time
  lookup (P-D9, P-D17) still reaches that object through a needed
  object's `DT_NEEDED`.
  D-110 item 2's seven names (WP-150 §3a; WP-166): in a `Shared` output a
  name of the seven that no input defines, that a live object references
  (an undefined reference, not hidden) and that the script does not make
  local is in the export set as `GLOBAL NOTYPE DEFAULT`, size 0, its
  version the script's, preemptible by D-112 as a `NOTYPE` key
  (`line_preempts`: under `-Bsymbolic` bound; under
  `-Bsymbolic-functions` preemptible for mortar and lld, bound for GNU
  ld, which binds `NOTYPE` there, the format's measured rule, no
  anomaly). It is no import. The one derivation is "an object references
  it": a referenced name missing is P-D19 "... is in the export set and
  not defined in .dynsym", one exported though no object references it,
  or though `local:` hides it, P-D19 "... is in .dynsym and not in the
  export set". Strict for mortar. mold exports none (A57). GNU ld exports
  more: `__etext` and, on AArch64, `__end__`, `__bss_start__`,
  `__bss_end__`, `_bss_end__` when an object references them (A58);
  another linker-defined key exported by mortar (`__etext`,
  `__executable_start`) is P-D19's plain finding.
- **P-D20 (forbidden forms; request P-R5).** Reported, each by name: a
  `COPY` record; a canonical PLT (`SHN_UNDEF` with a non-zero
  `st_value`); `DT_TEXTREL`; a dynamic record whose place is in a
  `PT_LOAD` without `PF_W` outside `PT_GNU_RELRO`; an LE TLS form in the
  output's code (D-117, D-119, D-122); an output section `.ctors` or
  `.dtors` (their input goes to `.init_array`/`.fini_array`, D-124).
  i386: every place the output resolves to a PLT entry or GOT slot comes
  from an input relocation whose form sets `%ebx` (`PLT32`, `GOT32X`
  with a base register; D-130).
- **P-D21 (segments; request P-R6).** Each `PT_LOAD` has `p_align` the
  max page size and `p_offset ≡ p_vaddr (mod p_align)` (glibc checks
  only the page, D-123).
- **P-D22 (`.dynamic` of a shared object; request P-R7).** In addition
  to P-D6 with D-102's rows: mortar's tags in D-102's order (D-30's with
  `DT_SONAME` and `DT_SYMBOLIC` after `DT_NEEDED`, then the run path; no
  `DT_PREINIT_ARRAY*`, no `DT_DEBUG`; `DT_VERDEF`/`DT_VERDEFNUM` before
  `DT_VERNEED`), each a tag D-102 writes ("the .dynamic tags are not in
  D-102's order", "a tag D-102 does not write"; D-30's order is an
  executable's and does not judge a `Shared` output); `DT_SYMBOLIC` and `DF_SYMBOLIC` present iff
  the line's last `-Bsymbolic` form is `-Bsymbolic` and the line has no
  `--dynamic-list` (D-112, D-102, D-103); `DT_INIT`/`DT_FINI` point at
  defined, live code (D-124); `DT_SONAME` the `-soname` string;
  `DT_VERDEF`/`DT_VERDEFNUM` and `.gnu.version_d` as P-D27 recomputes
  them (D-115; WP-149);
  `GNU_PROPERTY_1_NEEDED_INDIRECT_EXTERN_ACCESS` present iff an input
  has it (D-112); `DT_FLAGS_1`'s `DF_1_NODELETE`, `DF_1_NOOPEN`,
  `DF_1_INITFIRST`, `DF_1_INTERPOSE` and `DF_1_ORIGIN` each present iff
  the line has its `-z` word (D-103; D-30's executable row does not
  judge them in a `Shared` output; `DF_1_NOW` stays P-D6's), and
  `DT_FLAGS`' `DF_ORIGIN` present iff `-z origin` is (D-103 writes both
  bits; measured, WP-161: GNU ld, lld and mold write both too; P-D6's
  mask lets `DF_ORIGIN` through for the references, this rule holds it
  to the line in every format). `DT_INIT`/`DT_FINI` under `-init`/`-fini`
  are the named function's value, and that value lies in a `PF_X`
  `PT_LOAD` ("... which is not in an executable segment: the loader calls
  it"; WP-161). The same holds for an executable's tags (P-D6, D-30 rows
  3-4). Measured (WP-161, `-init`/`-fini` naming a data object): GNU ld,
  lld and mold all write the data object's address, in a library and in
  a PIE, and GNU ld's PIE dies at start-up (SIGSEGV): the loader calls
  the address, so it is a finding of every format, not an anomaly (no
  reference test links it).
  **A stripped output** (no `.symtab`: `-s`, `--strip-all`; WP-164).
  Before WP-164 the rule read `_init`/`_fini` (and the `-init`/`-fini`
  names) from `.symtab` alone and reported "the output defines no
  '_fini'" on every `-shared -s` library of the four linkers. Without a
  `.symtab` the function is looked up where the output and its map still
  show it: an exported definition in `.dynsym` (its value), else the
  relocatable inputs' non-local definitions of the name in sections the
  map places whole (place + `st_value`: the tag holds one of them); "the
  output defines no ..." is reported only when neither `.dynsym` nor any
  live input defines the name. A name the inputs define in sections the
  map does not place whole leaves the value judged by its segment only
  (a `PF_X` `PT_LOAD`), named in the coverage line ("stripped output (no
  .symtab), not judged: DT_INIT: the symbol is not in the output, value
  judged by segment only"). P-D6's `-init`/`-fini` clause for an
  executable reads a stripped output the same way (`.dynsym`, else the
  one address the placed input definitions agree on, else the segment,
  named the same way: "the function is not in the output").
- **P-D23 (mutations; request P-R8).** Each wrong output of `spec/dyn.md`
  §17.7, written into a correct mortar library (or, where it concerns
  the executable, a host), is reported by the rule the table below
  names, and each row is a test (`tests/shared.rs`, `p_d23_*`: the
  wrong output as the reference that writes it links it, on AArch64 or
  x86-64; the i386 rows as P-D1's refusal); a row marked NOT caught is
  a test that asserts it is not.
- **P-D24 (executables: a copy of locally bound data; request P-R9).**
  P-D11 gains, for `Exec` and `Pie`: a `COPY` of a definition its
  library binds locally (`DT_SYMBOLIC`/`DF_SYMBOLIC`, or a `.note.mason`
  row with `NONPREEMPTIBLE`, odr.md O-60) whose `st_value` lies in a
  writable `PT_LOAD` of that library outside `PT_GNU_RELRO`, or in no
  `PT_LOAD`, is reported (`spec/dyn.md` D-43a; WP-122 owns this rule's
  body in `dyncheck.rs`, WP-121 its call). Decided from the library's
  program headers and `.dynamic`, as the linker's rule: a table that does
  not parse binds every definition, and a library with a `PT_LOAD` whose
  `p_vaddr + p_memsz` overflows is reported for every such copy. A copy
  of read-only or `PT_GNU_RELRO` data is not reported (two equal
  constants). The references link W30: anomaly A45.
- **P-D25 (records name only preemptible keys; request P-R10; amends
  P-D9 for `Shared`).** No dynamic record of a `Shared` output names
  (`r_sym` not 0) a key the verifier computes as bound (D-112: hidden,
  protected, local by a version script, `-Bsymbolic`,
  `-Bsymbolic-functions` for a function, unlisted by `--dynamic-list`).
  A use of a bound key that needs a record has one of these kinds, with
  `r_sym` 0: AArch64 `R_AARCH64_RELATIVE` (1027), `IRELATIVE` (1032),
  `TLS_DTPMOD64` (1028), `TLS_TPREL64` (1030), `TLSDESC` (1031); x86-64
  `R_X86_64_RELATIVE` (8), `IRELATIVE` (37), `DTPMOD64` (16), `TPOFF64`
  (18), `TLSDESC` (36, refused as an input form by x64.md §6.2 until a
  compiler emits it); i386 `R_386_RELATIVE` (8), `IRELATIVE` (42, never
  in i386 `Shared`: D-130), `TLS_DTPMOD32` (35), `TLS_TPOFF` (14),
  `TLS_DESC` (41). Every other kind with `r_sym` 0 is reported (P-D9's
  `RELATIVE`/`IRELATIVE` symbol-0 rule stays). From a reference, a
  record naming a `PROTECTED` key, or naming any bound key of a library
  that the line makes `-Bsymbolic` and that carries `DT_SYMBOLIC` or
  `DF_SYMBOLIC`, is accepted: glibc binds both to the object's own
  definition, and view B judges the value (GNU ld's form, on both
  targets, and lld's x86-64 GD pair under `-Bsymbolic`; WP-121). A
  `DTPOFF` record with `r_sym` 0 is accepted from a reference (P-D26).
  A record whose `r_sym` is past `.dynsym` is reported by name.
  A type-0 record (`R_AARCH64_NONE`) is reported with its own message
  when it has A46's shape, from a reference's AArch64 output only: in
  the `DT_RELA` table (`DT_RELASZ` bytes, not `DT_JMPREL`'s), `r_sym`,
  `r_offset` and `r_addend` all 0, at an index not below `DT_RELACOUNT`
  (the count of leading records glibc applies as `RELATIVE` without
  reading their type: a type-0 record inside it would write the load
  base at address 0 + base), and no more of them than `TLS_DTPMOD64`
  records with `r_sym` 0. glibc's loop over the rest skips type 0. Any
  other type-0 record with `r_sym` 0, and any in mortar's output
  (D-30), keeps the message above; a type-0 record that names a key is
  reported too, in every output (WP-144).
- **P-D26 (TLS in a shared object; request P-R11; replaces P-D12's
  executable forms for `Shared`; D-119).** Per model and binding, the
  records of a live TLS use are:
  - GD and LD against a preemptible key or an import: `DTPMOD` +
    `DTPOFF` (AArch64 `TLS_DTPMOD64` + `TLS_DTPREL64`, x86-64 `DTPMOD64`
    + `DTPOFF64`, i386 `TLS_DTPMOD32` + `TLS_DTPOFF32`), both naming the
    key; against a bound key: one `DTPMOD` with `r_sym` 0 per pair, the
    offset word the static offset with no record (a `DTPOFF` record with
    `r_sym` 0 holding the same offset is accepted from a reference);
  - TLSDESC against a preemptible key or an import: one `TLSDESC`
    naming the key (in `.rela.dyn` for mortar, D-119; in `.rela.plt`
    from a reference); against a bound key: `TLSDESC` with `r_sym` 0 and
    the offset as addend (RELA) or in the descriptor's second word
    (i386 REL). On x86-64 the pair is kept (`spec/x64.md` X-61): the
    `lea`'s three instruction bytes are the input's and its field
    reaches the descriptor, `call *(%rax)` stays (R1's kept branch; the
    field itself is the link's, WP-144); GCC's local-dynamic descriptor
    for `_TLS_MODULE_BASE_` is `r_sym` 0 with addend 0 and holds the
    block's start, not X-47's relaxed `T = 0`. Every reference measured
    (WP-144): GNU ld in `.rela.plt` with `DT_TLSDESC_PLT` and
    `DT_TLSDESC_GOT`, under `-z now` in `.rela.plt` without them; lld
    and mold in `.rela.dyn`, eager, both ways;
  - IE against a preemptible key or an import: `TPREL64`/`TPOFF64`/
    `TLS_TPOFF` naming the key; against a bound key: the same type with
    `r_sym` 0 and the offset as addend (RELA) or in place (REL);
  - LE: none (refused, P-D20).

  No relaxation: the number of `__tls_get_addr` calls equals the
  input's (D-119). `DF_STATIC_TLS` iff some IE slot exists (GNU ld on
  AArch64 never writes it: accepted from that reference, dyn.md's
  reference table). The values:
  in view B a preemptible key's module and offset are the interposer's,
  a bound key's the library's (P-D17). P-D12's D-16 direction (W14)
  applies unchanged.

- **P-D27 (version definitions; WP-149; D-115).** *Recomputed:* the
  script's named nodes in order, each node's parents, the line's
  `-soname` and `-o`, the export set's versions (P-D19). *Read:*
  `DT_VERDEF`, `DT_VERDEFNUM`, `.gnu.version_d` (by `DT_VERDEF`, its
  header when the output has one), `.gnu.version`, the Vernaux records.
  *Reported* (one finding per Verdef, per entry, or per header):
  - `DT_VERDEF` present exactly when the script names a node;
    `DT_VERDEFNUM` without `DT_VERDEF`; `DT_VERDEFNUM` other than the
    nodes + 1, or other than the chain's length;
  - per Verdef *k* (from 0): `vd_version` 1; `vd_ndx` *k* + 1; `vd_flags`
    exactly `VER_FLG_BASE` on entry 0 and 0 on the others; `vd_hash` the
    ELF hash of its first name; its names (`vda_name`, in chain order):
    entry 0 the soname, else the output's name (the basename of `-o`,
    default `a.out`; lld's the `-o` as written, measured: `sub/p.so`
    stays `sub/p.so`); entry *k* the script's *k*-th named node, then,
    for mortar and GNU ld, its parents last-written first (`V3 {...} V1
    V2;` gives `V3`, `V2`, `V1`); `vd_cnt` equal to the names read and to
    the expected count; the last Verdaux's `vda_next` 0; `vd_next` 0
    exactly on the last entry;
  - the section header: `sh_info` the number of Verdefs, `sh_link`
    `.dynstr`, `sh_addr` `DT_VERDEF`; mortar's `sh_addralign` 4;
  - no `.gnu.version` while the script names a node;
  - each `vna_other` above the definitions' indices (`max(DT_VERDEFNUM,
    1)`: the index space is shared, definitions first, needs after;
    every linker measured writes the first need at definitions + 1);
  - `.gnu.version` per `.dynsym` entry: 0 for an `STB_LOCAL` entry; an
    import 0, 1 or a need's index, never `VERSYM_HIDDEN` and never a
    definition's index 2..=`DT_VERDEFNUM` (lld and mold write 0 for an
    unversioned weak import, resolve §9.2); GNU ld's node-name symbols
    (`V1`, `SHN_ABS`) their node's index; each defined export one of the
    indices the export set gives its name, each consumed once: 1 for the
    base (no script, an anonymous node, matched by no node), the node's
    index for `foo` and `foo@@V`, the index with `VERSYM_HIDDEN` exactly
    for the non-default `foo@V`. A name the script makes local is absent
    (P-D19's; `-E` changes nothing for it: rule 12's per-format reading
    above).

  *Judged for each reference* only on common ground or as measured:
  every field above but lld's and mold's parents (they write none:
  `vd_cnt` 1; measured, and they refuse a node with two parents), lld's
  base name (the `-o` as written, measured, not the basename) and the
  references' `sh_addralign` (GNU ld's 8, lld's and mold's 4: not
  judged). *Not judged:* the layout of `.dynstr` (D-116); the i386
  output (P-D1). Tests: `tests/shared.rs`
  `p_d27_verdef_libraries_verify_clean` (case `verdef`: `vd.c` under
  two and three nodes, with and without `-soname`, by each reference on
  both arches), `p_d27_verdef_mutations_are_caught` and
  `p_d27_mortar_libraries` (`tests/shared/verdef`, mortar's): each of
  `verdef_mutants` is a new P-D27 finding: a wrong `vd_hash`,
  `VER_FLG_BASE` cleared on the base and set on V1, V1's and V2's
  `vd_ndx` swapped, `DT_VERDEFNUM` ± 1, the last node missing, the
  base's name and V1's name wrong, a wrong parent, `vd_cnt` ± 1,
  `bar`'s and `baz`'s `.gnu.version` swapped, `VERSYM_HIDDEN` cleared on
  `foo@V1` and set on `bar` and `foo@@V2`, `bar`'s index 0 and 1, the
  import `ext`'s index 2; `p_d27_mortars_alignment_only` (GNU ld's
  library judged as mortar's: `sh_addralign 8`).

  **A key the line makes local, in `.symtab` (WP-150's forms; judged
  since WP-169).** The keys the line makes local are stage 3's: a
  `Shared` output's set's local keys (a version script's `local:`,
  `--exclude-libs`; by the full key and by its base name), an
  executable's excluded keys (D-110 item 1); none for a static output.
  For each such key whose inputs' visibility is below `HIDDEN` (a hidden
  or internal one is a local as before):
  - **mortar** (synth S-77, S-121): the `.symtab` entry is `STB_LOCAL`
    with its own visibility. Else Y3 "symbol 'K', which the line makes
    local, is not STB_LOCAL (synth S-77)" or "... does not keep its own
    st_other visibility (synth S-77)".
  - **a reference** (WP-150 item 1's measured forms, named): `STB_LOCAL`
    with its own, `DEFAULT` (GNU ld; lld's `--exclude-libs`) or `HIDDEN`
    (mold's `--exclude-libs`) visibility; GNU ld's executable `GLOBAL
    HIDDEN` (`-no-pie` and static, WP-161). Else Y3 "... is not
    STB_LOCAL" or "... has neither its own st_other visibility nor the
    references' DEFAULT or HIDDEN (WP-150 item 1)". The older Y3 binding
    and visibility rows of these keys are dropped for a reference only
    (`shared::filter_view1`, `excluded_locals`).
  - **An exported definition is global** in `.symtab`: an exported key
    whose only `.symtab` definition at its value is `STB_LOCAL` is P-D7
    "'K' is exported, and .symtab defines it STB_LOCAL only" (every
    linker).
  - **D-114's linker-defined keys under `local:`** (a `Shared` output
    with a version script): a `__start_SEC`/`__stop_SEC` (SEC a C
    identifier) or one of D-21 item 2's seven that no input defines and
    the script makes local is, in mortar's `.symtab`, `STB_LOCAL` with
    `PROTECTED` (`__start_`/`__stop_`) or `DEFAULT` (the seven); GNU ld's
    and mold's `LOCAL DEFAULT` for `__start_`/`__stop_` is named. Else
    Y5 "the linker-defined 'K', which the version script makes local, is
    binding B visibility V in .symtab; ...". lld keeps `__start_`/
    `__stop_` exported `GLOBAL PROTECTED` (§10.2 A61, by P-D19); its
    `.symtab` rows of them are not judged. That none is exported is
    P-D19's (outside the export set).
  - **A `.symver` definition** (an input defines `foo@V1` or `foo@@V2`):
    mortar's and GNU ld's `.symtab` keep the versioned name (synth
    S-75): the `.dynsym` export `foo` of version `V2` (`.gnu.version`;
    `@` for a hidden index) is matched to the `.symtab` global named
    `foo@@V2` at its value alone, else P-D7 "'foo@@V2' is exported, and
    .symtab has no entry of that name at its value (synth S-75: a
    .symver definition keeps its versioned name)". lld's and mold's two
    plain `foo` are named: their exports are matched in both forms, and
    Y3's "not in .symtab" accepts the plain base name for them alone.

What reports each wrong output of `spec/dyn.md` §17.7 (P-D23; the P-D
rules are this section's, applied through P-D16; the table was redone
by WP-110's third pass against the interposer above, P-D25 and P-D26):

| wrong output | reported by |
|---|---|
| W19 (LE TLS against own TLS, AArch64) | P-D20 (LE form in a shared object's code; P-D26's LE row is P-D20's: an LE use owes no record) |
| W20 (exported IFUNC bound) | P-D17, view B; P-D18 |
| W21 (two addresses of one IFUNC) | P-D18 (`hif` is bound: stage 2's F3) |
| W22 (`DT_INIT = 0`, the function collected) | P-D22 (mold 2.37.1 writes it: a link, measured here) |
| W23 (`__start_` undefined, section dropped) | P-D17 (the input's relocation against `__start_cid_ref` computes the section's address; the output's place holds 0); not P-D19: the version script makes `__start_cid_ref` local, so it is outside the export set; and, for the `GLOB_DAT` naming it, P-D9 (no definition), P-D25 (it is bound) and P-D7 |
| W24 (`.ctors` kept as an output section) | P-D20 |
| W25 (text relocations in `.eh_frame`, i386) | P-D20 (record in a non-writable `PT_LOAD`; `DT_TEXTREL`) |
| W26 (`%ebx`-relative IPLT entry as canonical address, i386) | P-D18 (no IFUNC in i386 `Shared`) |
| W27 (hidden IFUNC called through an `%ebx` stub, i386) | P-D18 |
| W28 (`-shared -static` C++ without `--eh-frame-hdr`, i386) | NOT caught: the image is what the line asks (no `--eh-frame-hdr`, so no `PT_GNU_EH_FRAME`); needs a check of the line, not the image (a `-shared` line with C++ unwind tables and no `--eh-frame-hdr`), proposed for WP-130 (i386), which reduces the root cause the record lists as untraced (D-100's `-shared -static` cell) |
| W29 (`-shared -static` C library crashes, mold i386) | NOT caught: the cause was not isolated (record §B); WP-130 (i386) reduces it to a record or a PLT form before a rule can name it |
| W30 (a `COPY` of writable locally bound data) | P-D24 (executables) |
| W31 (`DF_SYMBOLIC` with `--dynamic-list`) | P-D22; view B (P-D17) |
| W32 (`ADR_PREL_LO21`, `LD_PREL_LO19`, `PREL32` without a record, AArch64) | P-D17, view B |
| W33 (`PREL32` in `.data` bound; weak undefined non-zero) | P-D17, view B; P-D9 (P-D16) for the weak key |
| W34 (`PC32` dynamic record truncated, x86-64) | P-D9 (a dynamic record of a type the loader model does not apply, `R_X86_64_PC32`); view A's place keeps its link-time bytes (R1); P-D17, view B, for the preemptible and the imported key |
| W35 (`PC32` call through an `%ebx` PLT, i386) | P-D20 (i386 clause) |
| W36 (`GOT32X` without a base register, i386) | P-D20 (i386 clause) |
| W37 (`GOTOFF` to a preemptible key, i386) | P-D17, view B |
| W38 (`&f` of an import is a PLT stub, AArch64) | P-D17: view A's value is the import's definition, the place holds the stub (R1, R2); view B the interposer's |
| W39 (`R_386_TLS_IE` in `.text` without `DT_TEXTREL`) | P-D20 (record in a non-writable `PT_LOAD`) |
| W40 (`extern "C++" {*}` exports nothing) | P-D1: the verifier has no demangler, so a script with `extern "C++"` is outside stage 3 by name (D-114) and P-D19 is not reached; P-D19 once it has one |
| W41 (`NEEDED_INDIRECT_EXTERN_ACCESS` dropped) | P-D22 |
| W42 (versioned reference unversioned, `DT_NEEDED` dropped) | P-D6, P-D8 (P-D16: index 1 for `vf@VA`), P-D9 and P-D7 (view A: a versioned import is never the host's) |
| W43 (TLS GD pair half bound, x86-64) | P-D25 (the `DTPMOD64` names `tv`, which `--dynamic-list` leaves bound) and P-D26 (a bound GD pair is `DTPMOD` with `r_sym` 0); P-D17, view B: the interposer defines `tv` (it is exported), the loader model resolves the `DTPMOD64` to the interposer's module while the offset is the library's, and the expected value is the library's own `tv` |
| W44 (anchor+immediate bound use, AArch64) | P-D17's address clause (b): the verifier's analysis derives `.data + 4`, inside `g`, from the `ldr` that follows the anchor (both views agree on the value, the clause reports it). What D-117's residual hole names stays not caught, by the verifier as by the linker |
| a use of a preemptible key bound locally through a named symbol (a `PREL32`, `ABS64` or `ADRP`/`ADD` resolved statically, no record) | P-D17, view B: the interposer defines the key, the place holds the library's own address |
| an export missing from `.dynsym` | P-D19 (the defined set differs from the verifier's export set); P-D17, view B, for every record that should have named it |
| a wrong version index on a defined entry or an import | P-D19 (a defined entry's version, D-115); P-D27 (the index the export set gives the name, `VERSYM_HIDDEN` exactly on `foo@V`; an import's never a definition's; WP-149); P-D8 (an import's `.gnu.version` index against its Verneed and the shared definition's version, through P-D16) |
| a wrong version definition (a node missing or extra, its index, flags, hash, name or parents; `DT_VERDEFNUM` off; a need's index among the definitions') | P-D27 (WP-149) |
| a name twice in `.dynsym` at one version, or twice not hidden | P-D7 (WP-149; D-115) |
| a `JUMP_SLOT` naming the wrong symbol index | P-D10 (the slot's record is against another key than its PLT entry's); P-D17, view B (the interposer defines both keys at distinct addresses, the call place reaches the wrong one); P-D25 too when the named key is bound |

Measured for WP-121: its own implementation of D-117 against the
oracle on every corpus of `spec/landed/dyn/WP-110.md` §G.1 still built
(same verdicts, `spec/landed/plumb/WP-121.md` §6). `[unmeasured]`: each
row above as a mutation of a mortar library.

## 13. Stage 4: Mason metadata and the ODR table (M6)

Implemented by WP-078 (amendment A-7 of `spec/odr.md` §14; the rules
are `spec/odr.md` §12.5's; record `spec/landed/meta/WP-078.md`). The
verifier re-reads the container format of `spec/meta.md` itself
(`meta.rs`, a second reader: `mortar_elf` and `mortar_diag` only) and
judges the output's `.mason.meta` and `.note.mason` (`odrtable.rs`).
Stage 4 runs on every output `verify` and `verify_with` read, of every
format (a reference linker has none of these options, so it is held to
"none of these sections"), before stage 3's early returns: an output
stage 3 puts outside (P-D1) is still judged here.

### 13.1 What the rules read

- **The options**, from the mortar map's line (§3.2a; `spec/cli.md`
  §2's forms: `--odr-table` and `--odr` take two dashes only):
  `--odr-table=V` and `--odr-table V` (the last wins, default `off`),
  `--odr=M` and `--odr M` (default `warn`), `--no-odr-check`
  (`--odr=off`), `--keep-mason-meta`. A reference's map has none of
  them (none of mortar's options).
- **The runtime's options** (WP-197; `spec/cli.md` C-ODR-RT, `spec/odr.md`
  O-53a): `--odr-runtime=MODE` and `--odr-runtime MODE`, the last one
  (default `off`; `report` and `fatal` ask for the runtime, any other
  value is a line mortar refuses); whether the line spells `--odr-table`
  at all; the last of `-z now` and `-z lazy`. With the runtime asked
  for, the **effective** options are: `--odr-table` `exported` unless
  the line spells one (`=full` and `=exported` stand; an explicit last
  `=off` is `conflicting-options`, a line mortar refuses: a P-M2
  finding), the table due under `--odr=off` too (cli.md: "the runtime
  reads the table"; WP-198 §2: the two are independent), and `-z now`
  unless the line's last of the two is `-z lazy` (stage 3's `now`,
  P-D3 and P-D6, reads the effective value). A reference's map has none
  of them.
- **The runtime input** (WP-197). With the runtime asked for, mortar
  adds the object of the output's machine and kind as an input after
  every input of the line, named `<mortar:odr-runtime>` (cli.md
  C-ODR-RT). The verifier holds its own copy of the six objects
  (`include_bytes!` of `runtime/odr/obj/*.o`; a unit test holds each
  equal to `runtime/odr/PINS`' SHA-256 and size) and chooses one by the
  **expected runtime id**, derived from the output alone, never from
  the map: the machine (`EM_AARCH64` 0, `EM_X86_64` 2, `EM_386` 4) plus
  1 for an executable (the output has `PT_INTERP`, PIE or not) and 0 for
  a shared object (`ET_DYN` without `PT_INTERP`); an output of neither
  (a static executable) has no id. When a mortar map names the file
  `<mortar:odr-runtime>` (not an archive member) and its line asks for
  the runtime, that object is the file's bytes for every reader, the
  caller's file of that name hidden: the store (§8.1), so the
  placement, relocation re-application (§5), merged pieces, `.eh_frame`
  (§6.4), symbols (§6.5), content (S12) and the dead-range rules judge
  its sections as any object's; the line's readers (`Ctx.files`), so
  the input set (§3.8) and stage 3's model (§12: D-11's needed
  libraries, P-D7's references, the dynamic relocations) see its
  references; and the line's words, which get the name appended after
  every word (the input's place in resolve §4.1's order). An object
  that is not the one of the output's machine and kind would show as
  bytes the output does not hold (§5, S12), and its note's id is P-M2's
  finding. Without the option, or for a reference's map, the name is
  a path like any other: not among the inputs, I1 (today's rule).
- **The output kind.** A table is due (O-50, as implemented: WP-075)
  exactly when the output has a `PT_INTERP` (a dynamic executable, PIE
  or not), `--odr-table` is not `off` and `--odr` is not `off`, units
  or none (with none, every row set is empty: a count-0 table; measured
  with no `.mason.meta` input at all, with and without shared inputs,
  with and without tables in them). An output with no `PT_INTERP`
  (static, static-pie) never has one, `=full` included. A shared object
  (`-shared` on the line, the output kind `Shared`) is due under the
  same options (O-60, WP-164; measured on mortar's libraries: no unit, or
  C units only, give a count-0 table, as for an executable: "present
  iff asked", not "iff there is a row").
- **The live units.** The map's files in map order (FileId order; §3.2's
  ids count up in the order the linker read the files), each read as an
  `ET_REL`; of each, the sections that are containers (MT-6: name
  `.mason.meta`, `SHT_PROGBITS`, flags exactly `SHF_EXCLUDE`; MT-2a: no
  relocation section applies to it), in section index order; of each,
  its units in offset order (MT-10). A container whose `sh_addralign` is
  not 8, or whose `sh_entsize`, `sh_link` or `sh_info` is not 0 (MT-1's
  attributes, which MT-6 leaves to the reader) has no unit that parses
  (measured: mortar keeps none of its units, and `--odr=strict` names the
  alignment by MT-1). A unit
  **parses** when its major is 1 and none of MT-14's malformations holds
  (magic, `unit_size`, blocks inside the unit, kinds strictly increasing,
  rows inside their block, the string table inside the unit, not over
  the header and starting with the empty string, the string fields of
  kinds 1-3 inside the table and NUL-terminated, kind 1 sorted strictly
  by name bytes, kinds 2 and 3 sorted by key, ties of two named rows by
  name bytes). Zero words between units are skipped; after a malformation
  that leaves no next offset the section's other units are not read
  (MT-14). A unit of another major is skipped (MT-35).
- **A kind 2 row's name**: its string, or for an anchored row (name 0)
  the name of a symbol its object defines (non-local, not `SHN_UNDEF`)
  whose FNV-1a 64 is the row's key; an anchored key no defined symbol of
  the object has names nothing. Rows match a name when their key is the
  name's FNV-1a 64 and their name is the name. A row is **qualifying**
  when its flags have `VAGUE` (bit 0) or `EXPLICIT_SPEC` (bit 3).
- **The export set** of a shared object (WP-164): stage 3's (§12.4,
  `shared.rs`'s `Sets`: D-110's `ExportRule`, the version script's
  `local:` and `--exclude-libs` applied) and its binding, D-112's, the
  derivation P-D17's view B uses (`Sets::preemptible`): protected,
  `-Bsymbolic`, `-Bsymbolic-functions` and a function, `--dynamic-list`
  not listing it bind a name inside the library.
- **The keeper** of a name (odr.md O-51): the input file whose
  definition the output holds. Candidates are the map's files whose
  symbol table defines the name non-locally in a section the map places
  whole; the keeper is the one candidate whose definition's output
  address (place + `st_value`) equals the output's symbol value
  (`.dynsym` for an exported name, `.symtab` otherwise). A single
  candidate is the keeper whatever its address only for a name the output
  does not hold (a definition `--gc-sections` collected). No candidate,
  or two at the output's value (two weak copies at one place are not
  possible in a correct output, but the verifier does not assume it),
  and the name is **not judged** (counted). The keeper's row is the
  first matching kind 2 row of the keeper's live units, in unit then row
  order; a keeper without one has no row (measured: a keeper without a
  unit gives no table row even when a losing copy's unit has one).

### 13.2 The rules

- **P-M1 (containers in outputs).**
  - No input container is placed (`place`, `piece`, `merged`) in an
    executable or shared object: MT-31 consumes them.
  - A `.mason.meta` output section in an executable or shared object
    (one rule for every output kind, WP-164; WP-076's for executables
    before) is either the kept
    section or an ordinary copy. The **kept section** is the one the map
    calls `chunk synthetic .mason.meta`; it must be `SHT_PROGBITS`, flags
    0, `sh_addralign` 8, at address 0 and inside no segment's file bytes
    (D-126 item 3; WP-164: "takes no address and lies in no segment"),
    and its bytes must equal MT-32's: the units that
    parse, in the order above, each followed by zeros to a multiple of 8.
    It is present exactly when `--keep-mason-meta` was given and at least
    one unit parses. An **ordinary copy** is a `.mason.meta` that
    the map fills only from placed input sections that are not containers
    (encoding K, `spec/meta.md` §2: an input `.mason.meta` without
    `SHF_EXCLUDE`): legal without the option, and with it when no live
    unit parses; a finding with the option and a unit (MT-32, as WP-099
    wrote it: refused by name when the output gets the kept section).
    Any other `.mason.meta` in an executable or shared object is a
    finding. MT-32's padding is empty for every unit that parses (MT-14:
    `unit_size` is a multiple of 8), so a unit's own zero bytes (its
    string table's padding up to `unit_size`) are compared as its bytes
    and a padding after a unit is bytes the concatenation does not have.
  - MT-32's loaded-bytes sentence (every `PT_LOAD` byte equals the link
    without the option but `e_shoff`, `e_shnum`, `e_shstrndx` and the
    build ID) is a property of two links and is **not judged** by a rule
    of one output; what one output shows of it is judged above (no
    address, no segment). The lane tests' `cmp` of the two links
    (`tests/link/dyn/odr/mt32-keep-meta-*.test`) is its only check.
  - A relocatable output is judged by stage R: its container by P-P8
    (MT-30), the units' contents in the final link that consumes it
    (§14.5).
- **P-M2 (`.note.mason`).**
  - *Place.* The section is `SHT_NOTE`, `SHF_ALLOC`, `sh_addralign` 8,
    and lies inside a `PT_NOTE` with `p_align` 8 whose file range holds
    its bytes. It holds whole notes (`n_namesz`, `n_descsz`, `n_type`,
    the name padded to 4 and the descriptor to 8 from offset 24, O-52;
    every padding byte zero, since the bytes are a function of the rows,
    O-50: WP-078's spec finding 11), each owned by `MasonMeta` (`n_namesz` 10), with types `0x4d4f4401`
    (exactly one, first), `0x4d4f4402` (at most one, right after the
    first) and `0x4d4f4403` (at most one, last; O-52 as WP-198 wrote
    it), and no other type.
  - *Presence.* The section exists exactly when a table is due (§13.1,
    with the runtime's effective options);
    the second note exactly when `--odr-table=full`; the runtime note
    (WP-197) exactly when the line's last `--odr-runtime` is `report` or
    `fatal` and the output has `PT_INTERP` or is a shared object. A line
    that asks for the runtime is itself a finding when mortar refuses
    it: an output with neither (a static executable or static PIE,
    `odr-runtime-static`), an explicit last `--odr-table=off` beside it
    (`conflicting-options`), a value other than `off`, `report`,
    `fatal`. A `-r` output is stage R's (the option is a no-op there,
    cli.md C-R2b: P-P8 holds it to no `.note.mason`). Options unknown
    (a reference's map is held to none of mortar's options, so a
    runtime note in a reference's output is a presence finding as the
    table's note is): presence is not judged, the content rules run.
  - *Runtime note* (WP-197; O-52, O-59): `n_descsz` exactly 16; the
    descriptor's four little-endian `u32` are version 1, the mode (1
    for `report`, 2 for `fatal`, the line's last `--odr-runtime`;
    options unknown: 1 or 2), flags 0, and the runtime id, which is
    the output's expected id (§13.1: its machine and kind, from the
    header and `PT_INTERP`; an output that has none is a finding).
  - *Header and shape* (O-51): version 1, `row_size` 24, reserved 0,
    `n_descsz` exactly `16 + 24 × count`; each row's reserved `u16` 0;
    rows sorted by key, then `.dynsym` index, no two equal; each
    `dynsym` index inside `.dynsym` and not 0.
  - *Key.* A row's key is FNV-1a 64 of the `.dynsym` entry's name
    (MT-13; without a version, which `.gnu.version` carries).
  - *Version.* The entry's `.gnu.version` index (when the output has
    one) is not 0 (local); two rows of one name have different indices.
  - *Row set* (O-50). The rows' `.dynsym` indices are exactly the
    entries that are defined (`st_shndx` not `SHN_UNDEF`), `STV_DEFAULT`
    or `STV_PROTECTED`, and whose keeper's row qualifies; entries whose
    keeper is not judged are left out of both sides (counted). A row
    naming an undefined, local, hidden or internal entry is a finding
    ("which is not an exported definition", WP-164). In a shared object
    also: every row's name is in the export set ("which the export set
    does not export"), and every name of the export set whose keeper's
    row qualifies has a row, also when its `.dynsym` entry is missing
    ("is in the export set (D-110) ... and no row names it"); a name all
    of whose entries the export set marks optional is not required.
  - *Hash.* A row's hash is the first 8 bytes of its keeper's row's hash.
  - *Flags.* A row's flags are the keeper's row's flags with bit 15
    (`NONPREEMPTIBLE`) set; in an executable every row has it (O-51,
    WP-075 finding 1: the executable binds its own definitions). In a
    shared object (O-60 statement 2; WP-164) the other fifteen bits are
    the keeper's row's (its own bit 15 cleared), and bit 15 is set
    exactly when the export set's binding (§13.1) binds the name inside
    the library; the message names which ("bound inside the library" or
    "preemptible", by D-112). A row whose name the export set does not
    hold has no binding to judge and is the row set's finding.
  - *`=full`'s note* (O-51, O-52): header version 1, `row_size` 32,
    `n_descsz` exactly `16 + 32 × (definitions + types)`, each array
    sorted by its bytes as integers in field order with no two equal.
    *Definitions*: layout, size and alignment 0, bit 15 clear; each row
    is the first 8 hash bytes and the flags of a qualifying kind 2 row of
    a live unit whose key is the row's, and when the name's keeper is
    judged, of the keeper's row; no row's name is one the first note
    carries. Every name a live file defines non-locally, not carried by
    the first note, whose keeper is judged and whose keeper's row
    qualifies has a row. *Types*: exactly one row per kind 3 group (key
    and name) of the live units, the group's first row in (file, unit,
    row) order, carrying its key, the first 8 bytes of its hash, its
    layout, size, alignment and flags.
- **P-M3 (no code, or exactly this code).** In an output with a
  `.note.mason`, with `--odr-table` not `off` or with the runtime asked
  for: every byte of `.init_array`, `.fini_array`
  and `.preinit_array` (by `sh_type`) is covered by input sections of the
  same type the map places there, and no `chunk` record lies in them;
  every relocatable file the map names is on the command line (a
  `Relocatable` item of `Line`, by path, or a member of an archive that
  is), except, exactly when the line asks for the runtime, the one file
  `<mortar:odr-runtime>`, whose bytes are the verifier's (§13.1). Options
  unknown: the second half is not judged. With the runtime asked for
  (WP-197; O-59), also:
  - *input*: the map names `<mortar:odr-runtime>`, and no file that is
    not an archive member follows it in the map's file order (the
    runtime is the line's last input; members extracted after it, as
    for it, may follow). Without the option the file is not on the
    command line: the rule above, unchanged.
  - *kept*: every allocated section of the runtime with bytes has a
    place (§4.4 step 0's `no_place` is false: an input `.eh_frame`
    merged into the output's counts as placed): `--gc-sections` keeps
    it (its entry is a start-up root, `spec/live.md` E14 Q5), and nothing
    else removes it. ICF is not judged: mortar refuses `--icf`
    (cli.md), so no output folds it.
  - *entry*: the runtime's entry section (its `.preinit_array` in an
    executable, its `.init_array.00000` in a shared object; the two
    flavours have one each) is placed whole, once, inside an output
    section of that `sh_type`. Every other input section of that type
    the map places in the same output section lies before the entry
    exactly when the order puts it there, derived from the input
    sections' names and the map's file order (WP-198 §2.2, measured on
    the three references and mortar): in `.preinit_array` input order
    (an earlier file before, a later after); in `.init_array` by
    priority, `.init_array.N` being N and an unnumbered `.init_array`
    or `.ctors` after every numbered one, then input order. So a user's
    own `.preinit_array` or priority-0 entry of a file of the line runs
    before the runtime's, and every other constructor after it. That
    the entry word holds the runtime's function is §5's (its `.rela`
    record re-applied, a RELATIVE record's addend by P-D9).
  - *exports*: no `.dynsym` definition lies inside a placed section of
    the runtime, and neither do `DT_INIT` nor `DT_FINI` (O-53: it
    defines and exports no symbol; its entry is the init array's). No
    version definition can name it (a version definition names a
    `.dynsym` definition). A `DT_NEEDED` the runtime alone needs is
    judged by D-11 (P-D6) with the runtime's references in the model
    (§13.1): libc is needed exactly when some reference, the runtime's
    included, binds to it.
  - *imports*: each non-weak undefined symbol of the runtime but
    `__ehdr_start` and `_GLOBAL_OFFSET_TABLE_` (the linker's, O-53) is
    imported by `.dynsym` or defined by the output or an input. The six
    objects need `clock_gettime`, `dl_iterate_phdr`, `getauxval`,
    `getenv`, `write`, `mmap`, `munmap` and `abort` (O-53's list;
    `-nostdlib` fails the link, cli.md). That each import binds to a
    needed library's definition is stage 3's (P-D7).

Messages start with the sub-rule's name (`kept section`, `place`,
`presence`, `runtime`, `header`, `order`, `key`, `version`, `row set`, `hash`,
`flags`, `full`, `init array`, `input`, `kept`, `entry`, `exports`,
`imports`), so a mutant's catch is named
by rule and field. Coverage: `meta` counts outputs judged, live units,
rows checked, names not judged, and whether the options were known
(§7.2; `Report::summary`'s `ODR metadata` line); with the runtime
(WP-197), `runtime_outputs`, `runtime_entries` (entry sections judged)
and `runtime_imports` (undefined symbols of the runtime judged), the
`ODR runtime` line. The runtime's relocation records, bytes, FDEs and
symbols are counted where any object's are (records, S12's bytes, §6.4,
§6.5), not apart.

What stage 4 does **not** judge: `-r` outputs (stage R's, §14.5 P-P8); MT-32's
loaded-bytes sentence (above: two links); a unit's kinds 4 and 5 beyond their framing; the ODR verdict
itself (`spec/odr.md` Q11, answered in WP-078's record: not
implemented); the load-time hook (O-58): the runtime's BEHAVIOUR at
load (what it checks, reports and aborts on) is `tests/hook/run.sh`'s
matrix, not a rule of one output; the verifier judges that the bytes,
the entry, the note and the binding mode are the ones that behaviour
was measured on.

## 14. Stage R: relocatable outputs (M4-r; RL-5, WP-185; P-P1: RL-6, WP-186)

The rules of a `-r` output (`ET_REL`), applied from record
`spec/landed/layout/WP-154.md` §D.1 by WP-180 (RL-0; record
`spec/landed/plumb/WP-180.md`). Request V-R*n* of the design is rule
P-P*n* here (n = 1 to 11); V-R12, the mutations, is §9.2 (t). RL-5
(WP-185; record `spec/landed/plumb/WP-185.md`) implements P-P2 to
P-P11 and V-R12 in `crates/mortar_plumb/src/relocatable.rs` (with
`relsym.rs`, P-P4; `releh.rs`, P-P5; `relgroup.rs`, P-P6): every entry
point (`verify`, `verify_with`, `verify_from`, the dynamic one) hands an
output to stage R when it is `ET_REL`, its map has `output relocatable`,
or a foreign map's line has `-r` (P-P9), and stage R is also callable
alone, `mortar_plumb::relocatable::verify(output, src: &dyn Source, map:
&Map, ctx: &Ctx) -> Report`. P-P1 is RL-6's (WP-186): `mortar_plumb::compose::compose`, §14.5. The
linker rules cited are those of
`spec/cli.md` §3.16 (C-R*n*), `spec/resolve.md` §11 (RV-R*n*),
`spec/layout.md` §24 (L-R*n*), `spec/image.md` §17 (I-R*n*),
`spec/arch.md` §4.6 (A-R*n*), `spec/model.md` §4.6 (MD-R*n*) and
`spec/meta.md` MT-30.

A `-r` output is judged against the `-r`'s own inputs, as a final link
is (P-P2-P-P8, P-P10, P-P11). That the component then links as its
inputs would have is judged on two final links (P-P1). **The verifier
recomputes from the inputs and the line every decision the rules make**
(which file is live, the COMDAT keepers, the dropped sections and FDEs,
the merged visibility, each piece's output section under mortar's
rules): the map says only where each input section went.

Measured for this section on GNU ld 2.44, lld 22 and mold 2.37.1
(`tests/relocatable-design/exp/rl0/run.sh`, `work/rl0.log`; the
options in `exp/rl0/opts.sh`, `work/rl0-opts.log`), on top of the
design's experiments. "The references" in this section means GNU ld and
lld: their `-r` outputs of the corpus (§14.9) pass, but for lld's
anomalies (A91, A93, A100; RL-5 measured A120, A122, A125 and A126 as
well, with lld 22.1.8). mold's fail by name (§10.2 A90-A102, A121-A124);
a mold `-r` output is judged like any other. RL-5's measurement on the
three targets: GNU ld's outputs of every case have no finding.

### 14.1 What stage R reads

**The output.** `ET_REL` of a supported machine (P-P9). It has no
program headers: §2.1's memory model does not apply, nothing is read by
address, and every place is **(output section index, offset)**: the
section header's index in the output and the offset from the section's
start. An output section is identified by its index, never by its name:
two output sections of one `-r` output may have the same name
(`spec/layout.md` L-R1, L-R2, L-R11).

**The inputs** are the files the map names (§2.2), each `ET_REL` of the
output's machine (I2). They are numbered in map order: the `FileId`
order of the rules. For a foreign map, RL-5 numbers them in the line's
order of their words (an archive's members by member offset), then GNU
ld's `LOAD` lines, then map order; a path the line names twice is two
inputs, its rows read copy after copy (RL-5's reading, a QUESTION to the
supervisor, record WP-185: GNU ld's map names a file first in its
discarded-sections list, so its map order is not the link's order). A file the line marks binary (`-b binary` or
`--format=binary` before it, until `-b default`/`--format=default`;
`spec/layout.md` L-R9) is not read as ELF: it is a **binary input**
with one section, index 1, `.data` (`SHT_PROGBITS`, `SHF_ALLOC |
SHF_WRITE`, alignment 1, the file's bytes), and three globals, L-R9's
`_binary_M_size` (`SHN_ABS`, the size), `_binary_M_end` (`.data`, the
size) and `_binary_M_start` (`.data`, 0), `M` the path as the line
writes it with every byte outside `[A-Za-z0-9]` replaced by `_`. P-P2
and P-P4 judge it as an ELF input of that one section and those three
symbols; I2 does not apply to it. A foreign map's row for it is read as
any `.data` row of that file.

**The line** is §3.2a's, for every format (WP-210 item (a); WP-201's
rule): the caller's line when the caller gives one, as stage 3 reads it
(`--line`, the `--inputs-from` response file, the link-test lane's ld
words), else a mortar map's `arg` records. A mortar output always has
a line: a mortar map has at least one `arg` record (§3.2a; the reader
refuses one without), so "no line" exists only for a reference's map
the caller gives no line with. Whether stage R applies, and P-P9's
"the line has no -r", read the same line (WP-210). Until WP-210 stage R read a mortar map's `arg` records only
and ignored the caller's line, against §3.2a: a map whose records
dropped, added or changed an option moved stage R's verdict (M9 judges
the records themselves whatever the caller gives). Without a line the
clauses that need an option are not judged (`options unknown`) and the
others run as if the line had none of C-R3's options. *Test:*
`tests/relocatable.rs` `s14_1_the_callers_line_first_{aarch64,x86_64}`
(GNU ld's `-r` of the C component read as mortar's map: the records
without `arg -r` and the caller's line right, nothing new; the caller's
line without `-r`, P-P9; with `--build-id=sha1`, P-P10; no caller's
line, the base's findings; before WP-210 the second and third were not
found). The line's options stage R reads: `-d`/`-dc`/`-dp`,
`-s`, `-S`, `-x`, `-u`, `-e`, `--build-id[=STYLE]`/`--no-build-id`,
`-z execstack`/`-z noexecstack` (the last of the two), `--unique` and
`--unique=PATTERN`, `--force-group-allocation`, `--allow-multiple-definition`
(and `-z muldefs`).

**The input set** (§3.8) is judged as for a final link, with RV-R2's
roots: `-e SYM` is a root at `-r` exactly as in a final link (all three
references extract the member that defines `SYM`; the supervisor's
ruling of 2026-11-17 on RL-2's measurement, which amends RV-R2's
sentence "`-e` is not a root at `-r`"); `-u` is one; the default
`_start` is not (`crates/mortar_plumb/src/inset.rs` roots `_start` only
when the line has neither `-shared` nor `-r`). `-e` writes nothing by
itself: an `-e SYM` nothing defines is no key of P-P4 (GNU ld's `UND
SYM`, written with a warning, is GNU ld's alone and no finding on its
output; lld, mold and mortar write none, and its absence is no finding),
and mortar's `e_entry` stays 0 (P-P7).

**The places.** Each input section gets one place from the map (§14.2):

| place | meaning |
|---|---|
| whole at `(o, d)` | the input section is the piece at offset `d` of output section `o`; `off(S) = d`, `out(S) = o` |
| merged into `o` | a foreign map's `SHF_MERGE` input section whose pieces are in output section `o` and are not listed (lld's `<internal>` rows, mold's unlisted mergeable sections) |
| framed | an input `.eh_frame`: its records are placed one by one (mortar's `rframe` records), or the section is placed whole with another size (GNU ld's edited `.eh_frame`), or not listed (mold); judged record by record (P-P5) |
| consumed | an input section the `-r` makes an output section from rather than placing it: its `SHT_GROUP`, its relocation sections, `.symtab`, `.symtab_shndx`, `.strtab`, the section-name table; `.note.gnu.property` and `.note.GNU-stack` (merged, P-P10); `.note.gnu.build-id` (dropped, I-R8); a Mason container (P-P8) |
| dropped | a section a rule drops (§14.3) |
| absent | no record and no rule: a finding when the section has bytes (P-P2) |

**The keepers.** The keeper of each COMDAT signature is recomputed by
`spec/resolve.md` §6.1 among the live inputs of the `-r` alone (RV-R7):
the group of the least `FileId`, the first by section index there.

### 14.2 The map at `-r`

**mortar's map.** A `-r` link writes version 2 (§3.2) with these
records, added within the version as the incremental records were (a
reader that predates them refuses the map: "unknown record"):

| record | count | meaning |
|---|---|---|
| `output relocatable` | exactly one | the output kind (`Plan.kind == Relocatable`); §3.2's `output` record gains the value |
| `rplace <file> <section> <out> <offset> <size>` | one per input section placed whole | the input section `<section>` of `<file>` is the piece at `<offset>` (hex) of the output section whose header index is `<out>` (decimal), with `<size>` (hex) its input size |
| `rframe <file> <section> <input-offset> <output-offset>\|-` | one per record (CIE, FDE, zero terminator) of each input `.eh_frame` | the record's output offset in the output `.eh_frame` (L-R6's record map), or `-` for a removed record |
| `rgroup <out> <file> <section>` | one per output `SHT_GROUP` | **hint**: the input group section the output group at header index `<out>` is made from (the keeper; I-R12) |
| `rcommon <name> <out> <offset> <size> <align>` | one per common allocated under `-d` | as §3.2's `common`, with the place `(out, offset)` in place of an address (RV-R5, L-R4) |
| `discard <file> <section> comdat\|unspecified` | one per input section with bytes that is dropped | §3.2's record: `comdat` for a member of a COMDAT copy that is not kept, `unspecified` for every other drop of §14.3 |

with `file` and `arg` as in every map, in §3.2's writer order (`rplace`
where `place` goes, then `rframe`, `rgroup`, `discard`, `rcommon`,
`output`, `arg`). In a map with `output relocatable`, `place`,
`piece`, `nplace`, `merged`, `fold`, `chunk`, `common`, `define`, the
incremental records and every dynamic record but `arg` are a `MapError`
("<record> in a relocatable map"): nothing at `-r` has an address. An
`rplace` whose `<out>` is not an output section header, or an `rframe`
in a map whose output has no `.eh_frame` output section or more than one,
is M1 against the output. The `.eh_frame` output section is the one
section of that name (L-R11 never splits it). The hints are never the
expected value: P-P6 recomputes the keepers and P-P5 the record map, and
a hint that disagrees is a finding of that rule.

**Foreign maps** (§3.4-§3.6) are read as they are, with three changes
when the output is `ET_REL`:

- **Offsets.** An input row's address is read as an offset: the row's
  address minus its output section block's address. (GNU ld gives a
  non-allocated output section at `-r` the address its location counter
  had, `.note.GNU-stack` at 0x5c after a 0x5c-byte `.eh_frame`, and its
  rows the same base; lld and mold give 0.) mold gives every row of a
  non-allocated output section the address 0 as well: its rows are laid
  end to end in map order, each at the next multiple of its input
  section's alignment (RL-5's reading, a QUESTION to the supervisor,
  record WP-185; measured on the corpus's debug sections, where it
  places every piece as P-P2 then finds it).
- **Which output section.** The *k*-th output section block of a name
  in the map is the *k*-th section header of that name in the output, in
  index order (measured on the three references with two `.text.r`
  sections, one `SHF_GNU_RETAIN`: GNU ld lists two blocks in header
  order; lld and mold write one section and one block). A GNU ld block
  line is an output section only when it has an address and a size,
  on its line or on the wrapped next line; GNU ld's `-r` script lists
  every statement, so most block names have neither and are skipped. A
  map whose blocks of a name outnumber the output's headers of that name
  is M1.
- **Which input section** (RL-5's reading of a cell this section left
  open; a QUESTION to the supervisor, record WP-185): a row names its
  file and a section name, and a file may have several sections of one
  name. The row's section is the
  first section of that name in that file (over the copies of a file the
  line names twice, in order) not yet named by a row, **of the row's
  size**; among several of that size, the one whose bytes outside its
  relocated fields are the output's at the row's place; else the first
  not yet named. Measured: with GCC's `-fdebug-types-section`, GNU ld
  lists the script-placed `.debug_info` (the CU, the input's last
  section of that name) before the type units, and mold lists the type
  units in its own order, several of one size; reading the *k*-th row
  as the *k*-th section misplaces every type unit (4679 findings on GNU
  ld's output, none with this reading).
- **Consumed rows.** A row naming an input relocation section (lld lists
  `m1.o:(.rela.text)` under `.rela.text`) is skipped: §3.3 consumes
  those sections. lld's `<internal>` rows under an output section make
  that section's same-named `SHF_MERGE` inputs *merged* into it; its
  `<internal>` `.symtab`, `.strtab`, `.shstrtab`, `.comment` and
  `.note.GNU-stack` rows are the consumed or merged sections they name.

GNU ld's edited `.eh_frame` (a whole row of another size: GNU ld trims an
FDE's trailing `DW_CFA_nop` padding at `-r` too, 0x38 to 0x34 in the
measurement) is *framed*; so is lld's whole `.eh_frame` row and mold's
missing one.

M2-M5 apply with places for addresses: M4 reads "inside an output
section of its name" and "`off(S)` a multiple of its `sh_addralign`",
and has no `SHF_EXECINSTR`/`SHF_WRITE` clause (P-P11 judges the flags);
M5 reads "two pieces of one output section overlap". M6-M8 do not apply
(`rcommon` is judged by P-P4). **M10 at `-r`** (mortar's map): a section
of a live input with bytes that has no place has a reason of §14.3, or
it is M10.

### 14.3 What a `-r` drops

The verifier derives each from the inputs and the line:

- the members of a COMDAT copy that is not its signature's keeper (RV-R7,
  §14.1), with their relocation sections;
- under `--force-group-allocation`, no member is dropped for that reason
  other than the losers' (the keeper's members become ordinary sections);
- under `-S` or `-s`, the non-allocated debug sections (§3.3's M10 list:
  GNU ld's debug names) with their relocation sections (I-R9);
- **(mortar)** `.llvm_addrsig` (`SHT_LLVM_ADDRSIG`) with its relocation
  section, if any (L-R1); the references copy it (all three, with
  `sh_link` 0), which is accepted from them;
- **(mortar)** `.llvm.call-graph-profile` (`SHT_LLVM_CALL_GRAPH_PROFILE`)
  with its relocation section (an `SHT_REL` of `R_*_NONE` records on
  every target, `exp/rl0/none.sh`; L-R1, A-R4). GNU ld and lld copy both
  sections, which is accepted from them (per-linker: the section is then
  placed, P-P2 judges its bytes and P-P3 its records; P-P11 judges
  neither its `sh_link` nor its `sh_entsize`, which GNU ld and mold
  write 0 and lld 8); mold copies the section and drops its relocation
  section, A102;
- the input relocation records RV-R7 drops: the records of a removed FDE
  (P-P5) and the debug and `.gcc_except_table` records whose target is a
  dropped copy (I-R7);
Input `R_*_NONE` records are **not** dropped: a `NONE` record is a
reference (a `--gc-sections` edge, `spec/image.md` I-7), and every
linker carries it (I-R5; `rl0` case 5, `exp/rl0/none.sh`). Nothing else
is dropped: `--gc-sections` is refused at `-r` (I-R10),
and `SHF_EXCLUDE` sections are copied (L-R1). Every other section of a
live input that is not consumed must have a place.

### 14.4 How a target is compared

Every input relocation record names a **target**, and so does every
output record. P-P2-P-P6 compare targets by this one rule.

The **effective place** of a record against a symbol defined in a
section `T` is `(out(T), off(T) + st_value + A)` in the input (the
symbol's input value, the record's addend `A`), and `(shndx, st_value +
A')` in the output, where `A` and `A'` are the addends: `r_addend` on
RELA, and on REL the field's implicit addend read by §4.5's `Implicit`
description of the type from the input's and the output's bytes
(`spec/arch.md` A-R2). An `STT_SECTION` symbol's value is 0. A record
against a symbol of `SHN_ABS` has the effective place `(ABS, st_value +
A)`; one against an undefined, common or global symbol has the target
`(name, A)` (the name with its version, `foo@V`, `foo@@V`, as written).

An output record's target is **equivalent** to the input's when:

- the input names a global or weak symbol (defined or not), a common or
  an undefined symbol: the output names a global or weak symbol of the
  same name (version included), and `A' = A`;
- the input names a local symbol, an `STT_SECTION` symbol or a
  symbol of `SHN_ABS`: the output's effective place equals the input's,
  through a local, `STT_SECTION` or `SHN_ABS` symbol (a reference may
  write either form; mortar's is fixed, below);
- the input's target lies in a section placed *merged* into `o` (a
  foreign map): the output's effective place lies in `o`, and the
  NUL-terminated string (`SHF_STRINGS`) or the `sh_entsize`-byte entry
  that starts there equals the input's at its effective place. Counted
  as verified `string`. This is the form of lld's and mold's merged
  strings (record B.4, "the same string").

**(mortar)** I-R5's form exactly: a global or weak target by the
output index of its key (I-R4), `A' = A`; a local non-section symbol by
the output index of the same kept local, `A' = A`; an `STT_SECTION`
symbol of input section `T` by the `STT_SECTION` symbol of `out(T)`,
`A' = A + off(T)` (on REL in the field, written by `write_addend`).

A target in a dropped section has no equivalent: a record that needs
one is a finding of the rule that reads it, except where §14.3 drops the
record.

### 14.5 The rules

Each finding is at the output section and offset it names, sited at the
input file, section and offset when there is one. The messages below
are the text of each finding (`{…}` a field).

- **P-P1 (the final program; V-R1).** Judged on two final links, not
  on the `-r` output: the **direct** link `D` of a line `L`, and the
  link `V` of the same line with the component's inputs (contiguous in
  `L`) replaced, at the place of the first of them, by the `-r` output of
  those inputs; the same linker. Each is also verified by stages 1-3;
  P-P1 runs whatever they find, and reports only its own comparisons. The entry point is
  `mortar_plumb::compose::compose(direct, via, rmap) -> Report`
  (`crates/mortar_plumb/src/compose.rs`, RL-6's; §14.8 Q-R2):
  `direct` and `via` are each an output with its inputs and map, `rmap`
  the `-r` output's own inputs and map. `φ` maps a place of `D` to a
  place of `V`: an input piece's bytes by the piece (in `V` through the
  `-r` map: `V`'s place of the `-r` output's section `out(S)`, plus
  `off(S)`); a GOT slot, PLT or IPLT entry, copy, or TLS offset of a key
  to the one of the same key; a linker-made section to the section of
  its name. Compared:
  1. **The sections.** The output sections' (name, `sh_type`,
     `sh_flags`) in header order are equal. "P-P1 the output sections
     differ at {index}: {direct} in the direct link, {via} through -r".
  2. **The pieces.** Every input section of `L` is placed in `D` exactly
     when it is placed in `V`, in an output section of the same name,
     and the pieces of each output section are in the same order. "P-P1
     {file}:({section}) is {placed or not} in the direct link and {...}
     through -r".
  3. **The bytes.** Every placed piece's bytes are equal outside its
     relocated fields; every relocated field resolves (§5.1: stage 1-3's
     `S`, or the slot it names) in `V` to `φ` of what it resolves to in
     `D`. A record of a component input is the `-r` output's record
     P-P3 matched to it. "P-P1 {file}:({section})+{offset} {type}
     resolves to {target} in the direct link and to {target'} through
     -r".
     **A mergeable target is judged by content** (the supervisor's
     ruling, WP-232). When the record's symbol is defined in an
     `SHF_MERGE` input section and the field holds its whole place
     (`full`: an absolute or PC-relative field of the full width), the
     target is the field's place `S + A` (in a merged section `S + A` is
     the piece that holds offset `A`, not `S` moved by `A`: the addend's
     effect is judged with it), and the field matches when `V`'s place
     lies in a piece of an `SHF_MERGE` input section of `V` with the
     same `sh_flags` and `sh_entsize` as one of `D`'s pieces at the
     target, in an output section of the same name and `sh_flags`, and
     holds the same content from the place on: for `SHF_STRINGS`, the
     bytes from the place through the string's terminator (the first
     entry of zeros, inside the piece), so a place inside a string
     compares the suffix and the place's offset inside its entry is
     equal; for fixed-size entries, the entry's bytes and the place's
     offset inside it. "P-P1 {file}:({section})+{offset} {type} resolves
     to {target} ({piece}: {content}) in the direct link and to {target'}
     ({content'} or "no mergeable piece") through -r, not equal content".
     Every other field stays exact (`φ` of `S`, above). *Why this loses
     nothing:* the gABI makes equal entries of `SHF_MERGE` sections of
     equal flags and entry size interchangeable (a linker keeps any one
     copy, and no conforming program can tell which); to pass, a wrong
     `V` would have to point at an equal string (or entry, at the same
     offset in it) of the same kind of section, which is a correct link.
     *What it does not excuse:* a target that is not in a mergeable piece
     on both sides (a non-mergeable `D` symbol is judged by address; a
     `V` place in a non-mergeable section, in the padding of something
     that is not a string, or between pieces is "no mergeable piece");
     unequal content (another string, the string plus one); a different
     addend's effect (it moves the place, so the content from it
     differs); a mergeable piece of other flags, entry size or output
     section. *A piece the `-r` step made* (the byte of padding a `-r`
     section of `sh_addralign` > `sh_entsize` holds between two strings,
     which a final link reads as a string `""` of its own) has no `D`
     counterpart: items 2 and 3 iterate `D`'s input pieces, so it is no
     finding by itself; a field of `V` that points at it is judged by
     the content its `D` field points at (a `""` in `D`: equal, clean;
     anything else: unequal); a field whose `D` target is not mergeable
     and whose `V` place is that padding is judged by address and is a
     finding. Fields judged by content are counted
     (`merged-by-content=<n>`); one whose `V` place is not also `φ` of
     the place by its piece counts under (t5).
  4. **The globals.** For every global or weak name of `D`'s and `V`'s
     `.symtab` and `.dynsym`, `V`'s value is `φ` of `D`'s, and its
     size, type, binding and visibility are equal. "P-P1 symbol {name}:
     {value} in the direct link, {value'} through -r".
  **Tolerances** (V-R1's; nothing else may differ):
  - **(t1)** addresses, offsets and sizes of sections and pieces: `φ`
    absorbs them. A `-r` section is one input aligned to its largest
    piece, so the final link may pad it differently;
  - **(t2)** `.symtab`'s locals (the `-r` output's `STT_SECTION` and
    `STT_FILE` symbols, the order of locals): not compared; `.strtab`'s
    and `.shstrtab`'s bytes: not compared;
  - **(t3)** the build-id descriptor, which hashes (t1) and (t2): not
    compared;
  - **(t4)** the members of a COMDAT group that has a copy inside the
    component and one outside, with different bytes: item 2 and 3 accept
    either copy as the kept one (the `-r` elects first);
  - **(t5)** a field whose target is mergeable (item 3): an equal entry
    other than the one `φ` maps the piece to (WP-232). Measured 0 on
    every composition with mortar's finals (mortar's map places each
    piece, and its fields point at their own piece's place); the count
    makes the tolerance visible where it is used.
  **Exclusions**: P-P1 is not applied, and the report says which (`not
  judged: relocatable composition ({letter})`), when:
  - **(a)** the `-r` line names an archive or a `--start-lib` group, and
    §3.8's closure of the `-r` line alone extracts other members than the
    closure of `L` does from the same archive (the review's x06 case 1:
    the `-r` extracts a strong `foo` that a weak `foo` outside the
    component keeps the direct link from extracting);
  - **(b)** `L`'s last of `--gc-sections`/`--no-gc-sections` is
    `--gc-sections`, or `L` has `--icf` other than `=none`, and some
    output section of the `-r` output holds pieces of two input files
    (L-R1's merge by name; x06 case 2). Under `--unique` (L-R11) each
    piece is its own section and (b) does not apply;
  - **(c)** `L` has `--wrap=SYM` and the component both defines `SYM`
    and references it (a reference the `-r` binds inside the component;
    `work/equiv.log`). A component that only references `SYM`, or only
    defines it, composes;
  - **(d)** `L` has `--exclude-libs` and the `-r` line names an archive
    member that §3.8's closure of the `-r` line extracts.
  Not judged: anything only a run shows (the lane runs both programs,
  `spec/harness.md`); `D` and `V` by different linkers.
  Linker rules judged: the composition every `-r` rule exists for
  (record §D.1's V-R1); directly RV-R2 (exclusion (a)), RV-R7 (t4),
  L-R1 (exclusion (b)), L-R4 (t1).
  **RL-6's readings** (`compose.rs`; each a QUESTION to the supervisor,
  record `spec/landed/harness/WP-186.md`):
  - `φ` of a common is the place of the symbol of the same key in `V`
    (the `-r` output may make it `SHN_COMMON` again, lld, or allocate
    it, GNU ld): the commons are the inputs' `SHN_COMMON` names;
  - a place of `D` that no input piece holds and that lies inside an
    allocated output section (a linker-made section, an IPLT entry, a
    gap) maps to the section of the same name in `V` at the same offset
    from its start; only a place outside every section maps to the
    "ends at" of the piece before it;
  - item 2's order compares **units**: a piece of `D` outside the
    component, or one output section of the `-r` output (L-R1's merge
    by name makes every piece of `.text.wk` of two component inputs one
    unit), each at its first piece and once;
  - a consumed input section (`.note.GNU-stack`, P-P10) is not judged by
    item 2; an empty piece is placed or not, but its output section's
    name is not compared (a linker may put it anywhere);
  - `.eh_frame` pieces are not compared by item 3 (the `-r` output's
    `.eh_frame` is re-framed, P-P5; the final link's is judged by
    stages 1-3): counted as `not compared: .eh_frame`;
  - addends: an x86 field's addend is the record's (`D`'s on both
    sides); a REL field's is read from the input's field;
  - exclusion (a)'s closures are the maps' extracted members (the
    `-r` map's and `D`'s), not a re-run of §3.8. A thin archive's member
    is named as the `-r` map names it, `(archive, member name)`, on
    both final links (WP-232; measured with GNU ld 2.44, lld 19.1.7 and
    mold 2.37.1, an archive `lib/libthin.a` of stored `../sub/m1.o`): GNU ld's
    map names it by its own path alone (`lib/../sub/m1.o`, with and
    without `--whole-archive`, in the preamble and the section lines),
    lld's `lib/libthin.a(../sub/m1.o)` (the stored name), mold's
    `lib/libthin.a(lib/../sub/m1.o)` (the path it opened). Without
    guessing: the thin archives are those of the `-r` map's members
    whose bytes (the `-r` link's input) begin `!<thin>\n`; each member's
    own path is its stored path from the archive's directory (§2.2),
    normalized lexically (§3.8). A bare path of `D` or `V` is that
    member when exactly one member of those archives has it as its own
    path and no word of that link's line is the same path (a file named
    both as an object and through the thin archive is not told apart:
    it keeps its bare name, and (a) applies if the closures then
    differ); `archive(m)` with `m` not the stored name is that member
    when `m` normalizes to its own path. An archive that cannot be read
    renames nothing;
  - (t3) has no "just outside" case of its own: P-P1 compares no
    linker-made byte, so a change outside the descriptor (the note's
    flags) is caught by item 1;
  - (t4) is unreached by every case so far: with contiguous components
    and first-wins election both links keep the same copy. Its logic
    has no test (a QUESTION: a case that needs it).
  *Passes:* the D.3 corpus's GNU ld and lld `-r` outputs, final-linked
  by GNU ld and by mortar, static and PIE (the cells marked `=`).
  Measured by RL-6 (`crates/mortar_plumb/tests/compose.rs`,
  `p_p1_{aarch64,x86_64,i386}`, static, `-e _start`): the components
  `compc` (c1, c2), `cifunc` (c4), `cctors` (c3; not with mortar's
  final, which refuses `.ctors`) and `compx` (two C++ COMDAT objects),
  `-r` by GNU ld and lld, finals by GNU ld, lld and mortar: judged, no
  finding, but one cell: lld's `-r` of `compc` final-linked by mortar
  had the finding "P-P1 ... COMMON (type 0x8, flags 0x3) through -r"
  (mortar's final link made lld's input section `COMMON` an orphan:
  WP-203, a linker gap). Since WP-210 (item 4) the test is written for
  WP-203's linker: that cell has no finding, like every other, and its
  mutant `WP-203: the COMMON orphan kept (.bss named COMMON)` (V's
  `.bss`, where WP-203 places the section, named `COMMON`) is caught by
  item 1 with that text. On a linker without WP-203 the cell fails.
  Measured by WP-232 (`crates/mortar_plumb/tests/compose.rs`): RL-7's
  `r-kernel-style-m` and `r-kernel-style` (static, mortar's final):
  their 36 findings each were the verifier's, not the rule's: item 3
  took `S` as the field less `A` for a section symbol of a merged
  section (`.rodata.str1.8 + 0x30`), which is no place (merged pieces
  are not contiguous). With the target `S + A` and by content they
  compose clean (`merged-by-content=467`, (t5) 0: `φ` by piece already
  maps each `""` of libc to the `-r` output's padding byte where `V`
  merged it). `p_p1_merged_{aarch64,x86_64,i386}` (GNU ld's `-r` of two
  string files, mortar's finals; on AArch64 `kc_tbl[0]` is the `-r`'s
  padding `""` in `V` and `kc.o`'s own in `D`): clean, 5-6 fields by
  content; the mutants of `V` "a field to a different string of the
  section", "to the equal string plus one", "to equal bytes in
  `.rodata` (not mergeable)" and "a `""` field to a zero byte of a
  non-mergeable array" are each named "not equal content" on all three
  targets (the last two with "no mergeable piece"). `p_p1_thin_aarch64`
  (mortar's `-r` of a thin archive's members, GNU ld's and lld's finals,
  with and without `--whole-archive`): judged, no finding (before WP-232
  GNU ld's were "not judged (a)").
  *Catches:* mold's `-r` of the C component (COMMON→`SHN_ABS`: item 4
  and item 3, the field resolves to the absolute 4); the `.ctors.65335`
  order mold's `-r` moves (item 2); a `-r` output whose piece offsets are
  off by 4 (item 3).

- **P-P2 (every live input section once; V-R2).** For every section of a
  live input that is neither consumed nor dropped (§14.1, §14.3):
  - it has exactly one place: whole, merged or framed. None is "P-P2
    {file}:({section}) is not in the output"; a second is M2;
  - placed whole at `(o, d)`: `o` has the input's name, `d` is a multiple
    of the input's `sh_addralign`, and `[d, d + size)` lies in `o`
    (`size` the input's; a `NOBITS` input in a `PROGBITS` output section
    holds zeros, L-49);
  - its bytes equal the input's in every bit no relocation record writes.
    For a relocated field (§4.3's field of the record's type, the
    machine's table in §4.2 and §4.5), the bits outside the field's
    encoded value, an instruction's opcode and register bits, are
    compared too: "P-P2 {file}:({section})+{offset}: {n} bits outside the
    {type} field differ". Inside the field:
    - RELA: the bits equal the input's;
    - REL: they hold the output record's implicit addend, which P-P3
      judges (on i386 GNU ld, lld and mold all rewrite a section-symbol
      record's field to `A + off(T)`: 4 to 0xc for `.data+4` with
      `.data` at 8, 1 to 4 for `.text+1` with `.text` at 3, and the
      same for `R_386_16`; a global's field stays `A`);
    - a record §14.3 drops for its dropped target (I-R7): the field holds
      §5.8's value for a target not in the output, in mortar's column
      for mortar (I-47's tombstone of the referring section and the
      field's width; 0 in `.gcc_except_table`), and in the referring
      linker's column, or its input bits under its record (P-P3's
      per-linker forms), for a reference. "P-P2 {file}:({section})+{offset}
      holds {value}, not the tombstone {tombstone} of a record into a
      dropped copy";
  - a merged section's strings or entries are not compared by bytes
    (`in-mergeable`); records into it are judged by content (§14.4).
  **(mortar)** the place is the rules':
  - `o` is the output section of L-R1's key (the exact name; for
    `SHF_LINK_ORDER` the output section of the linked-to section; whether
    `SHF_GNU_RETAIN` is set; under `--unique` the piece itself, L-R11),
    or for a member of a kept group (and no `--force-group-allocation`)
    an output section of its own (L-R2): "P-P2 {file}:({section}) is in
    output section {o}, not {o'} (L-R1's key {key})";
  - the pieces of `o` are in input order `(FileId, section index)`, each
    at the next offset aligned to its `sh_addralign` after the previous
    one, the gaps zero (L-R4), and `sh_size` is the last piece's end;
  - no piece is merged or deduplicated (L-R5): an `SHF_MERGE` piece is
    placed whole, byte for byte;
  - a `PROGBITS` and a `NOBITS` piece of one key share one `PROGBITS`
    output section (L-R1a, L-49; lld's form; GNU ld and mold write two
    sections, accepted from them).
  Linker rules judged: L-R1, L-R1a, L-R2, L-R4, L-R5, L-R9 (a binary
  input's `.data`), L-R11, I-R7, I-R9 (what `-S`/`-s` drop), A-R2 (the
  field), MT-16 (mortar's L-R4 order keeps same-named sections of one
  input in input order).
  Not judged: a merged piece's bytes (the final link merges again);
  references' piece order and offsets beyond alignment and bounds
  (theirs differ, record B.1).
  *Passes:* GNU ld's and lld's outputs of the corpus, of
  `rl0/run.sh`'s cases 1-14.
  *Catches:* a piece dropped or written twice; an opcode bit flipped
  under `CALL26` or `ADR_PREL_PG_HI21`; a tombstone written as 0 in a
  DWARF 5 `.debug_info` field (§9.2 (t)).

- **P-P3 (every input relocation once; V-R3).** For each record of a
  placed input section (whole or merged; a framed `.eh_frame`'s records
  are P-P5's, which uses this rule's matching), `R_*_NONE` included,
  that §14.3 does not drop, there is exactly one output record, for
  every linker:
  - in the relocation section whose `sh_info` is `out(S)` (on AArch64
    and x86-64 `SHT_RELA`, on i386 `SHT_REL`, A-R4); never matched by
    the target section's name, so two output sections of one name are
    two relocation sections;
  - at `r_offset = off(S) + r_offset_in` there;
  - with the same `r_type` (nothing is relaxed, A-R1);
  - with an equivalent target (§14.4).
  "P-P3 {file}:({section})+{offset} {type} against {target} has no
  output record at {out}+{offset'}" (for an input `NONE` record the text
  adds "(an R_*_NONE record is a reference: a --gc-sections edge the
  final link needs)"); "... has an output record against
  {target'}, not {target}"; "... has an output record of type {type'}".
  **No other records**: an output record that no input record gives is
  "P-P3 output record {type} at {out}+{offset} comes from no input
  record", except:
  - GNU ld: an `R_*_NONE` record in `.rela.eh_frame`/`.rel.eh_frame`
    where it removed an FDE (L-R6; record §D finding 1);
  - GNU ld: a debug-section record retargeted from a dropped copy to
    the kept copy's section, when the two input sections have the same
    name and size (§10.6's per-linker rule, now at `-r`: record B.2,
    "retargeted ... all of them");
  - lld: an `R_*_NONE` record at the place of a debug record into a
    dropped copy, the field holding its input bits (lld writes 12 of
    them for the corpus's 10 records in DWARF 4, 10 in DWARF 5);
  - lld: a plain `foo` reference bound to the definition `foo@@V`, its
    output record naming `foo@@V` (RV-R6's lld form; both final links
    run, the review's x09).
  These are per-linker rules (§10.6), accepted only from their linker.
  GNU ld's form for the same input (`foo@V1` and `foo@@V2` defined, an
  `UND foo`, the record naming `foo`) is P-P3's general rule, verified
  for GNU ld; mold's (two plain `foo` globals) is A95.
  **(mortar)** RV-R6 with a versioned definition `foo@@V` and a plain
  reference: lld's form, required (the supervisor's ruling of
  2026-11-17): the record names `foo@@V`; a record naming `foo` is "P-P3
  ... has an output record against foo; mortar's names foo@@V (RV-R6,
  lld's form)". I-R5 exactly: no `R_*_NONE` record but the inputs'
  (none of its own: P-P5's removed FDE leaves nothing), and records in
  the order of `S`'s pieces, then the input order; the target in §14.4's
  mortar form; a relocation section exists exactly for an output section
  with at least one output record, a `NONE` record included, and an
  input relocation section whose records RV-R7 all drops gives none
  (L-R10, §14.8 Q-R1); it is
  named `.rela<name>`/`.rel<name>` of its target, `SHF_INFO_LINK`, plus
  `SHF_GROUP` iff its target is a group member (P-P11).
  Linker rules judged: I-R5, I-R7 (no record), A-R1, A-R2 (the addend
  through the field), A-R4.
  Not judged: what the final link computes (P-P1).
  *Passes:* rcheck's measurement (record B.4): GNU ld 2264 of 2264
  records equal on AArch64, 2230 on x86-64, 2044 on i686; lld and mold
  the same with "the same string" for merged strings.
  *Catches:* a record moved by 4 bytes; a section-symbol addend without
  the piece offset, on RELA and on REL in the field; a `NONE` record left
  at a live record's offset (mortar); an input `NONE` record dropped
  (every linker); mold's dropped `.rel.llvm.call-graph-profile` (A102); a record moved into the other
  `.rela.text` of a `--unique` output; a global's record matched by name
  but at the wrong place; mold's debug records against symbol 0 (A97).

- **P-P4 (the symbol table; V-R4).** For every producer:
  - the locals precede the globals and `.symtab`'s `sh_info` is the
    index of the first non-local: "P-P4 .symtab's sh_info is {n}, the
    first non-local is {m}";
  - every key RV-R6 writes appears exactly once among the globals: every
    global or weak name a live input mentions, other than the losers
    RV-R4 drops (a weak definition against a strong one). Absent: "P-P4
    {name} is not in .symtab"; twice: "P-P4 {name} is in .symtab {n}
    times";
  - its binding, type and visibility: the merged visibility
    (`spec/resolve.md` §3.5), and a hidden or internal key stays
    `STB_GLOBAL` or `STB_WEAK` ("P-P4 {name} is local; a hidden key
    stays global at -r (RV-R6)"); `STB_GNU_UNIQUE` kept when the
    definition has it; the definition's type; versioned names under their
    full names (RV-R6);
  - a defined key: `st_shndx = out(S)` of the definition's input section
    `S` and `st_value = off(S) + v` (`v` its input value): "P-P4 {name}
    is at {out}+{value}, not {out'}+{value'}"; in a section placed
    *merged*, the value is judged by content as §14.4 says;
  - a common key (no definition, RV-R5) without `-d`: `SHN_COMMON`,
    `st_size` the largest of its mentions' sizes and `st_value` the
    largest alignment: "P-P4 common {name} is {shndx}, not SHN_COMMON";
    with `-d`/`-dc`/`-dp`: a definition `STT_OBJECT` of its size in a
    `NOBITS` `.bss` output section at an offset that is a multiple of its
    alignment, which mortar's `rcommon` record states (L-R4);
  - an undefined key: `SHN_UNDEF`, `STB_WEAK` exactly when every live
    mention is a weak reference (RV-R3);
  - a local that is not `STT_SECTION` or `STT_FILE`, of the output: an
    input local of the same name, type and size, at its mapped place
    (`out(S)`, `off(S) + v`; in `.eh_frame` the record map's, P-P5);
    "P-P4 local {name} at {out}+{value} is no input's local there". A
    mapping symbol (`$x`, `$d`, `$t`, with or without a `.` suffix) in a
    non-executable output section is not judged for a reference
    (I-R4: optional there; GNU ld moves an `.eh_frame` `$d`, mold makes
    them `SHN_ABS`);
  - an `STT_SECTION` symbol has value 0 and names an output section that
    exists;
  - a group's local signature defined in its input `SHT_GROUP` (GCC's
    type-unit `wt.`/`wi.`, gas's non-COMDAT group signature) has
    `st_shndx` = the output `SHT_GROUP` of that group and value 0
    (L-R3): "P-P4 group signature {name} is in section {shndx}, not its
    group {g}";
  - no linker-defined symbol (RV-R8): a name of `spec/resolve.md` §8.1
    that no live input defines is undefined or absent, never defined;
  - `.symtab_shndx`, when present, has one word per `.symtab` entry, and
    an entry with `SHN_XINDEX` has its index there.
  Per-linker forms: a `-u` name no input defines is absent from lld's
  and mold's output (RV-R3: GNU ld and mortar write it); lld accepts
  `-d` and leaves commons `SHN_COMMON` (record B: "accepted, no
  effect"), and mold accepts `-dc`/`-dp` likewise; lld's plain
  `foo` bound to `foo@@V` (P-P3) leaves no `UND foo`; GNU ld's `UND
  SYM` for an `-e SYM` nothing defines is no key and no finding (§14.1).
  **(mortar)** RV-R6's lld form, required (the supervisor's ruling of
  2026-11-17): a plain `foo` bound to `foo@@V` leaves no `UND foo`
  (judged, not accepted); GNU ld's `UND foo` beside `foo@@V` on mortar's
  output is "P-P4 foo is undefined in .symtab beside foo@@V: mortar
  binds the plain reference to foo@@V and writes no UND foo (RV-R6,
  lld's form)".
  **(mortar)** I-R4 exactly: the null entry, then one `STT_SECTION` per
  output section other than `SHT_GROUP`, the relocation sections,
  `.symtab`, `.symtab_shndx`, `.strtab` and `.shstrtab`, in index order
  (GNU ld writes this set; lld has none for `.note.GNU-stack`, mold one
  for every section: `rl0` case 4, accepted from them); then per live
  input in `FileId` order its `STT_FILE` entries and kept locals in its
  table order, the kept set being I-R4's "kept local symbols at `-r`"
  (no more, no fewer: "P-P4 local {name} of {file} is {kept or not}, by
  I-R4"); then the globals in `SymbolId` order (the first mention in
  `FileId` and symbol order); a `-u` name written `STB_GLOBAL`
  `SHN_UNDEF` (RV-R3); an `.eh_frame` local through the record map and
  one in a removed record dropped; `.symtab_shndx` exactly when some
  index is `SHN_LORESERVE` or more.
  Linker rules judged: I-R4, RV-R1, RV-R3, RV-R4, RV-R5, RV-R6, RV-R8,
  MD-R2, L-R3 (the signature), L-R9 (a binary input's symbols), I-R9
  (`-s`/`-x`: the locals dropped), C-R3 (`-u`, `-d`).
  Not judged: `.strtab`'s layout (S-86 at `-r`) beyond each name
  reading back; the order of a reference's locals.
  *Passes:* GNU ld's and lld's outputs of the corpus (both keep hidden
  keys `GLOBAL HIDDEN`, commons `COM` with the largest size and
  alignment, `UNIQUE` and versioned names).
  *Catches:* mold's COMMON→`SHN_ABS` (A90); mold's dropped `UNIQUE`
  member (A94); mold's stripped `.symver` names (A95); mold's two strong
  definitions (A96); a hidden global demoted to local; a global's value
  off by its piece offset; a local `$d` left at a removed FDE's offset
  (mortar); a type-unit signature pointed at a member.

- **P-P5 (`.eh_frame`; V-R5).** The output `.eh_frame` and each input
  `.eh_frame` are read as §6.4 reads them (framing, CIE pointers,
  augmentations, encodings: E1), with the pointer fields read as
  relocation targets (§14.4) instead of values: an FDE's initial location
  is the target of the record at its field.
  - An input FDE whose initial location's target is a dropped section, or
    whose address range is 0, is **removed**; every other input FDE is
    **live**.
  - Every live input FDE has exactly one output FDE whose initial
    location is equivalent (§14.4), with E3's equality: the same range,
    augmentation data length and instructions (without trailing
    `DW_CFA_nop` padding: GNU ld trims it), an equivalent LSDA target, and
    a CIE with the same version, augmentation, alignments, return address
    register, initial instructions and an equivalent personality target.
    "P-P5 FDE for {target} of {file} is not in the output"; "... differs:
    {field}".
  - No output FDE comes from a removed input FDE or from no input FDE:
    "P-P5 output FDE at {offset} for {target} comes from {a removed FDE of
    file, or no input FDE}".
  - Each output FDE's CIE pointer names a CIE (E1).
  - The relocation records of a live FDE or CIE are matched by P-P3 at
    the record's output offset plus the field's offset in it.
  - A live FDE's or CIE's field whose target is a dropped section (an
    LSDA, a personality into a dropped copy) is P-P3 "no equivalent
    target": the `-r` must have refused (RV-R7, WP-177's rule), so an
    output that has it is wrong.
  Per-linker forms: mold deduplicates CIEs (4 to 2 in the corpus) and
  writes a 4-byte terminator when no input has `.eh_frame`; GNU ld trims
  padding; all accepted by the comparison above.
  **(mortar)** L-R6 and I-R6 exactly: the output `.eh_frame` is the
  input records of the live inputs in `(FileId, record)` order, every
  CIE kept (none deduplicated), every live FDE kept, each byte for byte
  except its CIE-pointer word, which is its distance back to its own
  CIE's output place; a zero terminator exactly where an input has one;
  the map's `rframe` records equal this record map ("P-P5 rframe
  {file} {offset} says {x}, the record map gives {y}").
  Linker rules judged: L-R6, I-R6, RV-R7 (`.eh_frame`).
  Not judged: the instructions' meaning (as §6.4); an `.eh_frame` with an
  `SHT_X86_64_UNWIND` type mix (P-P11 judges the type).
  *Passes:* GNU ld's outputs (removed FDE, `NONE` record left: P-P3's
  per-linker form).
  *Catches:* lld's kept FDE of a dropped copy (A91); an FDE of a dropped
  copy kept (§9.2 (t)).

- **P-P6 (groups; V-R6).** Without `--force-group-allocation`:
  - **one `SHT_GROUP` per kept group**: the keeper of every COMDAT
    signature (§14.1) and every non-COMDAT group (flag word 0) of every
    live input, each copy its own (RV-R7). Missing: "P-P6 group {signature}
    of {file} is not in the output"; a COMDAT group written twice: "P-P6
    COMDAT group {signature} is in the output {n} times";
  - each output group: `sh_flags` 0, `sh_link` `.symtab`, `sh_entsize` 4,
    `sh_info` the index of a symbol whose name is the signature (for an
    assembler group named by its section, an `STT_SECTION` symbol of the
    member's output section), the flag word the input's
    (`GRP_COMDAT` = 1, or 0), and the members exactly the output sections
    that hold the pieces of the input group's members (each placed whole
    and alone in its section: "P-P6 member {section} of group {signature}
    shares output section {o} with {other}") and the relocation sections
    of those, each with `SHF_GROUP`. Measured: GNU ld and lld write
    `sh_flags` 0, the input's flag word (1 for the COMDAT
    `_Z13shared_inlinei`, 0 for gas's `plaingrp`), and the members
    `[member, its .rela]` with `SHF_GROUP` on both;
  - no section is a member of two groups, and an output section has
    `SHF_GROUP` exactly when it is a member;
  - the map's `rgroup` hints (mortar) name the keeper's group section.
  With `--force-group-allocation`: no `SHT_GROUP`, no `SHF_GROUP`, and the
  keepers' members placed by L-R1's key (P-P2).
  **(mortar)** L-R3's order: the members in the input member order, each
  member's relocation section right after it; the group section named
  `.group`, `sh_addralign` 4.
  Linker rules judged: RV-R7, L-R2, L-R3, C-R3's
  `--force-group-allocation`.
  Not judged: whether a reference's non-COMDAT group copies have
  distinct signature symbols beyond P-P4's.
  *Passes:* `rl0` case 3 by GNU ld and lld; the corpus's COMDAT C++.
  *Catches:* mold's dropped non-COMDAT groups (A92: no `SHT_GROUP`, the
  signatures `SHN_ABS`, the members still `SHF_GROUP`, their relocation
  sections not); two copies of a COMDAT group kept (§9.2 (t)).

- **P-P7 (the header; V-R7).** `ET_REL`; `e_machine` the inputs';
  `e_flags` A-R3's merge of the live inputs' (0 on the three targets);
  `e_phoff` 0, `e_phnum` 0, `e_phentsize` 0; `e_entry` 0;
  `EI_OSABI` `ELFOSABI_GNU` when an output symbol is `STT_GNU_IFUNC` or
  `STB_GNU_UNIQUE`, else `ELFOSABI_NONE` (I-R2); `e_shstrndx` names a
  `SHT_STRTAB`, with the `SHN_XINDEX` escape when there are 0xff00
  sections or more (I-R1); every section with file bytes lies in the
  file and no two overlap, nor the file header or the section header
  table (S6's file clause). "P-P7 {field} is {x}, not {y}". Every
  allocated section's `sh_addr` is 0 (all three).
  Per-linker forms: with `-e SYM`, GNU ld and lld write `SYM`'s offset in
  its section as `e_entry` (8 in the corpus; L-R7); a non-allocated
  section's `sh_addr` is not judged for GNU ld, which writes its location
  counter there (0x28, 0x5c: a note, §10.2); GNU ld also writes
  `ELFOSABI_GNU` when an output section has `SHF_GNU_RETAIN` and no
  symbol is `IFUNC` or `UNIQUE` (`rl0` cases 7 and 14), accepted from GNU
  ld only (RL-5's reading of the "when ... else": a QUESTION to the
  supervisor, record WP-185).
  **(mortar)** L-R7 and L-R8: every `sh_addr` 0; `e_entry` 0 with `-e`
  too; the headers in L-R8's order (index 0; the build-id note with
  `--build-id`; the output sections by their first piece's
  `(FileId, section index)`, the merged notes and the container at their
  first input's place; each `SHT_GROUP` right before its first member;
  each relocation section right after its target; `.symtab`,
  `.symtab_shndx`, `.strtab`, `.shstrtab`); file offsets in the same
  order, each aligned to its `sh_addralign`, `NOBITS` without bytes;
  the section header table last, aligned to 8 (ELF64) or 4 (ELF32).
  "P-P7 section {index} {name} is at position {i}, L-R8 puts it at
  {j}".
  Linker rules judged: I-R1, I-R2, L-R7, L-R8, A-R3.
  Not judged: the file's mode (I-R3; the bytes do not carry it: the
  link-test lane checks it, `spec/harness.md`); determinism (I-R11; two
  links, the lane's).
  *Catches:* the mutations `e_type`, `e_phnum`, `EI_OSABI` (§9.2 (t)).

- **P-P8 (the Mason container; V-R8).** At most one output section that
  is a container (MT-6). Its bytes equal `kept_units` over the live
  inputs (MT-30: their units in `FileId`, section, offset order, each
  padded with zeros to 8, malformed units dropped), `SHT_PROGBITS`,
  `SHF_EXCLUDE`, alignment 8; no input container is placed as a piece
  anywhere else. "P-P8 the output has {n} containers"; "P-P8 container
  bytes differ from kept_units at {offset}". A `.mason.meta` that is not a
  container is copied by name (P-P2). The references concatenate the
  containers by name, which gives the same bytes when every unit parses
  and is a multiple of 8 (measured, MT-30's check), but not when an
  input has bytes between its units: `tests/meta`'s `b.o` has 8 zero
  bytes after its first unit, which all three copy and `kept_units`
  drops. A reference's output that is the concatenation (each input
  container at the next multiple of 8, in `FileId` order) is accepted
  as its form (RL-5's reading: the sentence above is wrong for that
  input, a QUESTION to the supervisor, record WP-185).
  **(mortar)** the container at its first input container's place
  (L-R8).
  Linker rules judged: MT-30.
  Not judged: the units' contents (stage 4, §13, judges them in the final
  link); MT-30a's checks (diagnostics).
  *Catches:* a container duplicated (§9.2 (t)).

- **P-P9 (accept; V-R9).** S1 accepts `ET_REL` as a `-r` output, and the
  kind must agree with the map (`output relocatable`) and with §14.1's
  line (`-r`, `-i`, `--relocatable`, `-Ur`): "P-P9 the output is ET_REL
  and the map says {kind}", "P-P9 the output is ET_REL and the line has
  no -r, -i, --relocatable or -Ur". The line check is every format's
  (WP-210): before WP-210 it was a reference's only, since a mortar
  map's line was its `arg` records. Stage R then applies:
  §6.1's O1, I1, I2; §3.3's map rules as §14.2 reads them; §3.8; P-P2
  to P-P8, P-P10, P-P11; B1 (§6.7) on mortar's build-id note, over the
  whole file; P1 (§6.6) on the property note. Stage 1-3's address rules
  (S2-S5, S7-S9, S13-S21, the R rules, Y1-Y5, E1-E6, F1-F5, P-D*) do
  not run: an `ET_REL` has no memory.
  Linker rules judged: C-R1 (the kind), I-R1.

- **P-P10 (the notes; V-R10).**
  - **`.note.GNU-stack`.** Over the live inputs' notes (N: an input note
    without `SHF_EXECINSTR`, X: with it): when some input is X, the
    output has one `.note.GNU-stack` with `SHF_EXECINSTR` ("P-P10 an
    input asks for an executable stack ({file}) and the output's
    .note.GNU-stack {is N or absent}"). **(mortar)** L-R6a's table
    exactly, per target, for the line's last `-z execstack`/`-z
    noexecstack`: at most one, empty, `PROGBITS`, alignment 1, flags 0
    (N) or `SHF_EXECINSTR` (X), absent in the "−" cells. Per-linker
    forms, accepted by the general clause (`work/stack.log`): GNU ld
    writes X over N inputs under `-z execstack` on AArch64 and ignores
    it on x86-64 and i686; mold ignores `-z` and writes no note when no
    input has one; lld writes N in every cell, so over an X input it is
    A93.
  - **`.note.gnu.property`.** P1's merge (§6.6) over the live inputs, by
    the writing linker's column; P2 without its segment clause (no
    `PT_GNU_PROPERTY` at `-r`): one allocated `SHT_NOTE` section, aligned
    to 8 (ELFCLASS64) or 4, one property note. Measured at `-r`
    (`rl0` case 6): AArch64 GNU ld and lld AND the feature words (`BTI,
    PAC` with `PAC` gives `PAC`) and write no section when one input has
    no note, mold writes none (P1's "dropped" column); x86-64 all three
    AND `IBT`/`SHSTK`, and an object with only the `USED`/`NEEDED`
    properties merged with one with `FEATURE_1_AND` or with none gives
    no note in all three.
  - **`.note.gnu.build-id`.** Exactly when the line's last of
    `--build-id[=STYLE]`, `--build-id=none`, `--no-build-id` asks for one
    (I-R8): "P-P10 the line asks for a build id ({style}) and the output
    has none", or the reverse. On mortar's output B1 judges the
    descriptor over the whole file (I-R8: I-69/I-72 over the `-r`
    output's bytes, the descriptor zeroed), the style from the map's
    line (§6.7). A reference's descriptor is not judged (§6.7). The
    inputs' build-id notes are not in the output (consumed).
  Linker rules judged: L-R6a, L-R1 (the merged notes), I-R8, A-R5, C-R3
  (`-z execstack`/`noexecstack`, `--build-id`).
  *Passes:* GNU ld's outputs for every L-R6a cell (`work/stack.log`);
  lld's and GNU ld's property notes.
  *Catches:* the stack note written without X over an X input (§9.2 (t);
  lld's A93); mold's missing build-id note (A98).

- **P-P11 (every output section header; V-R11).** For each output
  section, every header field is recomputed and compared ("P-P11
  section {index} {name}: {field} is {x}, not {y}"):
  - `sh_type`: the pieces' type by L-49 (`PROGBITS` with `NOBITS` gives
    `PROGBITS`); a relocation section `SHT_RELA` (AArch64, x86-64) or
    `SHT_REL` (i386); `SHT_GROUP`, `SHT_SYMTAB`, `SHT_STRTAB`,
    `SHT_SYMTAB_SHNDX` for the sections the `-r` makes;
  - `sh_flags`: the OR of the pieces' `SHF_WRITE`, `SHF_ALLOC`,
    `SHF_EXECINSTR`, `SHF_TLS`; `SHF_MERGE`/`SHF_STRINGS` only when every
    piece has the same merge class, and then `sh_entsize` that class's
    (L-50): "SHF_MERGE with sh_entsize 0" is a finding (lld, A100);
    `SHF_LINK_ORDER` and `SHF_EXCLUDE` when every piece has them;
    `SHF_GNU_RETAIN` when some piece has it ("P-P11 section {name} holds
    the SHF_GNU_RETAIN piece {file}:({section}) without SHF_GNU_RETAIN";
    mold, A99); `SHF_GROUP` exactly on group members and their relocation
    sections (P-P6); a relocation section `SHF_INFO_LINK`;
  - `sh_addralign`: the largest piece alignment (at least 1); under
    `-d` a common allocated in the section is a piece of its alignment
    (RL-6, measured on GNU ld's `.bss`);
  - `sh_size`: at least the end of its last piece; a relocation
    section's a multiple of its entry size;
  - `sh_link`: for `SHF_LINK_ORDER`, the output section of the linked-to
    input section of every piece, one section for all of them ("P-P11
    section {name}: SHF_LINK_ORDER pieces link to {a} and {b}" or
    "sh_link is 0"; mold, A101); for a relocation section and
    `SHT_GROUP`, `.symtab`; for `.symtab`, `.strtab`; for
    `.symtab_shndx`, `.symtab`; otherwise 0;
  - `sh_info`: for a relocation section, the output section its records
    belong to (P-P3); for `SHT_GROUP`, the signature's index (P-P6); for
    `.symtab`, the first non-local (P-P4); for a copied section of
    another type, the input's (all three copy it);
  - `sh_entsize`: 24/16 (`.symtab`), 24/12 (`SHT_RELA`), 16/8
    (`SHT_REL`), 4 (`SHT_GROUP`, `.symtab_shndx`), the merge class's
    entsize, otherwise the pieces' common value, else 0. **(mortar)**
    exactly that: 0 where the pieces agree on a value is "P-P11 section
    {index} {name}: sh_entsize is 0, not {e} (mortar: the pieces' common
    value, exactly)". A reference may also write 0 whatever the pieces'
    value, and on `SHT_INIT_ARRAY`, `SHT_FINI_ARRAY` or
    `SHT_PREINIT_ARRAY` the pointer size (GNU ld writes 8 on clang's
    `.init_array`, whose pieces have 0) (the supervisor's ruling on
    RL-5's question 5);
  - a reference's copy of `.llvm_addrsig` or `.llvm.call-graph-profile`
    (§14.3) has neither its `sh_link` nor its `sh_entsize` judged; its
    relocation section is judged as any other;
  - `sh_name` reads back, in `.shstrtab`, as the pieces' name;
  - every output section holds a piece of a live input or is one the
    `-r` makes (§14.1's consumed sections: a group, a relocation
    section, `.symtab`, `.symtab_shndx`, `.strtab`, `.shstrtab`, the
    merged notes, the build-id note, the container): "P-P11 section
    {index} {name} comes from no input section" (A-R5: no GOT, PLT,
    IPLT or `.eh_frame_hdr` at `-r`). mold's 4-byte `.eh_frame` when no
    input has one is its per-linker form.
  **(mortar)** L-R1a exactly: `SHF_GNU_RETAIN` is part of the key, so a
  section with it holds only retained pieces (GNU ld's form; lld merges
  and keeps `R`, accepted from it); `sh_size` exactly the last piece's
  end; `sh_entsize` 0 when the merge classes differ, with `SHF_MERGE`
  and `SHF_STRINGS` dropped (L-50; GNU ld's `A` with entsize 0 in `rl0`
  case 10); a `SHF_COMPRESSED` input decompressed and no output
  compressed (L-R1a).
  Linker rules judged: L-R1, L-R1a, L-R2, L-R3, L-R10 (one relocation
  section per output section with records, none for a group or a
  relocation section), I-R5's section form, A-R4, A-R5.
  Not judged: `sh_offset` beyond P-P7; a reference's non-allocated
  `sh_addr`.
  *Catches:* mold's `sh_link` 0 on `__patchable_function_entries`
  (A101; the review's `w/lo`, `rl0` case 9); mold's dropped
  `SHF_GNU_RETAIN` (A99); lld's `AM` with entsize 0 (A100); `sh_link` of a
  `SHF_LINK_ORDER` section set to 0 and `SHF_GNU_RETAIN` dropped
  (§9.2 (t)).

### 14.6 Coverage

§7.2's sum holds at `-r` with the records of every placed input
section: **verified** by `place` (P-P3's match by place and target),
`string` (§14.4's content match into a merged section), `eh-frame` (a
pointer field of a matched CIE or FDE) or `tombstone` (a field of a
dropped record, P-P2); **accepted** by `per-linker` (P-P3's list, by
linker); **not checked** by `in-mergeable` (a record inside a merged
foreign section: its place is not listed) or `options-unknown`. An input
`R_*_NONE` record is verified by `place` like any other (§7.2's **none**
does not apply at `-r`: the record must be carried). The summary adds `relocatable:
pieces {n}, merged {m}, framed {f}; groups {g}; FDEs {k} live, {r}
removed; symbols {s}; sections {h}` and, under P-P1, `composition: {x}
judged, {y} not judged ({letters})`, then `composition compared: pieces
{p}, records {r} ({f} by form), bytes {b}, globals {g},
merged-by-content={m}` (item 3's fields judged by content, WP-232) and
`composition tolerances: ({t}) {n}, ...` ((t1)-(t5) as used).

Measured by RL-5 (`s14_references_*`, the corpus of §14.9 summed per
target and linker; records = verified + accepted + not checked +
failed, asserted on every link):

| target | linker | links | records | `place` | `string` | `eh-frame` | `per-linker` | failed |
|---|---|---|---|---|---|---|---|---|
| AArch64 | GNU ld | 27 | 10039 | 9834 | 0 | 185 | 20 | 0 |
| AArch64 | lld | 27 | 10039 | 1656 | 8177 | 185 | 21 | 0 |
| AArch64 | mold | 25 | 9733 | 1480 | 8076 | 155 | 0 | 22 |
| x86-64 | GNU ld | 26 | 9919 | 9715 | 0 | 184 | 20 | 0 |
| x86-64 | lld | 26 | 9919 | 1583 | 8131 | 184 | 21 | 0 |
| x86-64 | mold | 24 | 9623 | 1413 | 8034 | 154 | 0 | 22 |
| i386 | GNU ld | 22 | 8184 | 8024 | 0 | 140 | 20 | 0 |
| i386 | lld | 22 | 8184 | 1236 | 6787 | 140 | 21 | 0 |
| i386 | mold | 21 | 8006 | 1119 | 6745 | 122 | 0 | 20 |

`tombstone` is 0 (a reference's map is not mortar's), `in-mergeable` is
0 (every reference's merged section is listed or recomputed in the
corpus). `options-unknown` IS counted at `-r` (the supervisor's ruling
on RL-5's question 15): a record whose verdict an option of the line
decides, when there is no line, is not checked `options-unknown` (its
output record at the place is consumed, matched or not). Since RL-6 that
is a record against a common (`-d`/`-dc`/`-dp` decide the target's form:
a name, or the allocated place). Not yet counted: a line with an option
whose class at `-r` is "acts" (WP-181 §B, `docs/status/options.md`'s
"At `-r`" column) that §14.1 does not read; this needs that column in
the verifier's option mirror (`crates/mortar_plumb/src/opts.rs`, which
the supervisor regenerates: the `at_r` class of each row). The corpus
links all have a line, so the term is 0 in the table above. mold's failed
records are those of its findings, every one a §10.2 anomaly of §14.9's
table. GNU ld's `NONE` records
where it removed an FDE are output records that no input record gives:
accepted (P-P3's list), they are in none of the sum's terms, which count
input records. A per-linker symbol form (P-P4: lld's `foo@@V`, a `-u`
name omitted) counts as a judged symbol, not a record.

### 14.7 Not judged at `-r`

- The program's behavior: P-P1's comparison is static; the lane runs
  both programs (`spec/harness.md`, "Relocatable outputs").
- I-R3's file mode and I-R11's determinism (two links; the lane's).
- C-R2's no-ops (`r-noop-options`: the lane's `check cmp`), and every
  refusal (no output).
- The references' build-id descriptors (§6.7), section order, piece
  order and offsets, kept-local sets and mapping symbols in
  non-executable sections.
- A reference's `.llvm_addrsig` contents, and its and
  `.llvm.call-graph-profile`'s `sh_link` and `sh_entsize` (§14.3); the
  call-graph profile's bytes and records are judged.
- P-P1 on two final links by mold: RL-7 saw "{file}:(.gnu.warning.*):
  its bytes cannot be read on one side" (item 3) on such a composition;
  the cause is not measured. Not asked (the lane's LT-R2 does not
  compose mold's `D` and `V`): not judged.
- `-T` at `-r`: not judged. RL-11 keeps `-T` refused at `-r` unless a
  verifier rule for placement by a script at `-r` is written first, in a
  verifier package before it (the supervisor's ruling on WP-180).

### 14.8 Rulings (the supervisor's, on WP-180's questions)

- **Q-R1 (a relocation section with no output record): ruled (a), and
  input `R_*_NONE` records are carried.** A relocation section exists
  exactly when it holds at least one output record, a `NONE` record
  included; an input relocation section whose records RV-R7 all drops
  gives none (`spec/layout.md` L-R10, `spec/image.md` I-R5). The
  question showed that I-R5 as designed dropped the inputs' `NONE`
  records, which are `--gc-sections` edges: `exp/rl0/none.sh` (every
  target) keeps `keepme` through GNU ld's, lld's and mold's `-r` into a
  `--gc-sections` final link by GNU ld and lld, and collects it without
  the record. P-P3 and P-P11 are written for the ruling.
- **Q-R2 (who writes P-P1 and P-P9): ruled (a).** RL-6 (WP-186) writes
  P-P1 in `crates/mortar_plumb/src/compose.rs`
  (`mortar_plumb::compose::compose`; RL-1 adds the `mod` line), as a
  verifier-side package that reads no linker crate; RL-5 (WP-185)
  writes P-P9 (S1's widening and the dispatch to stage R, the entry of
  `relocatable.rs`). RL-6 calls both from `linktest`.

- **RL-5's questions (record `spec/landed/plumb/WP-185.md`,
  "Questions"), ruled at RL-6.** 1-4, 6-12 and 14 are ACCEPTED as
  implemented. (5) `sh_entsize`: for mortar's output exact, the pieces'
  common value, else 0 (0 where the pieces agree is a finding, P-P11);
  for a reference 0 is accepted, and GNU ld's pointer size on the array
  sections (`relocatable.rs`; mutants `(q5-mortar)`, `(q5-reference)`).
  (13) §9.2 (t) (4) reads "P-P6 and P-P11 (P-P2 when the map places the
  losing copy)". (15) `options-unknown` is counted at `-r` (§14.6; the
  test `(q15)`); the column it still needs is named there. (16) the
  "planned: RL-5" places are corrected, the "planned: RL-6" ones made
  true.
- **RL-2's measurements (rulings at RL-6).** (1) `-e SYM` at `-r` is an
  archive-extraction root and writes nothing by itself: no `UND SYM`
  for mortar, lld or mold (GNU ld's `UND SYM` is no finding on GNU
  ld's output, its absence none on mortar's), and `e_entry` stays 0
  (§14.1, P-P4). (2) RV-R6 with `.symver`: mortar writes lld's form
  (the `@@V2` entry, the caller's record names it, no `UND vfoo`); GNU
  ld's form stays accepted for GNU ld; mold's is the anomaly (P-P3,
  P-P4; the mutant `(rv-r6-mortar)`). (3) `-rpath`/`--rpath` at `-r`
  is a no-op; the option-mirror constants are unchanged.
- **WP-203 (RL-6's finding, ruled a linker gap).** lld's `-r` writes an
  input section `COMMON` (A120: `NOBITS`, `WA`, align 8, the commons'
  size); GNU ld and lld put it in `.bss` at the final link, mortar's
  final link makes it an orphan output section `COMMON`, which P-P1
  item 1 reports. P-P1 is unchanged. The composition test expected the
  finding by name until WP-210 wrote the cell for WP-203's linker (no
  finding, and a mutant that keeps the orphan: §14.5's cell list); the
  two packages merge in one batch.

### 14.9 The corpus

The D.3 corpus of record `spec/landed/layout/WP-154.md` (the C component
with a common, a merged string, hidden, weak and undefined weak symbols
and constructors at three priorities; `cifunc`; `cctors`; the C++
component with exceptions; gcc and clang-22, `-O1 -g -fcommon
-ffunction-sections -fdata-sections`, three targets), the `-O0 -g`
debug corpus (`exp/debug/`), `exp/none-overlap/`, and
`exp/rl0/run.sh`'s fourteen cases, each partially linked by GNU ld, lld
and mold with `-Map`, and `exp/rl0/none.sh`'s `NONE` case per target:
`_start` with `.reloc ., R_*_NONE, keepme`, `keepme` in its own section
of a second object, partially linked by GNU ld, lld and mortar (since
RL-7, WP-187: AArch64 only, x86-64 and i386 `-r` refused until RL-9),
then final-linked with `--gc-sections` by GNU ld and by mortar:
`keepme` is in the final output (`nm`), as in the direct link, and P-P3
finds the record in every `-r` output. The lane's LT-R6 test
(`spec/harness.md`, `relocatable/none-record-kept`) is not written yet
(WP-187 §F; WP-210 checked `tests/link/relocatable/`): the case runs in
`exp/rl0/none.sh` and in RL-5's fixtures only. RL-5's fixtures are built from them at test time
(§9.1: nothing binary is checked in). For each GNU ld and lld output:
no finding but the per-linker forms (and lld's A91, A93, A100, A120,
A122, A125, A126); every mold output: its anomalies by name (A90-A102,
A121-A124 as they apply), and nothing else.

RL-5's fixtures (`crates/mortar_plumb/tests/relocatable.rs`,
`s14_references_{aarch64,x86_64,i386}`, the sources in
`tests/relocatable/`) are, per target: the D.3 C, `cifunc`, `cctors`
and C++ components by gcc and (not on i386) clang; `debug-cxx` (`-g
-O0 -ffunction-sections`); `none-overlap`; `none` (`none.sh`'s first
part); `rl0` cases 1 (with 12), 3, 4, 6 (the target's property flags),
7, 8, 9, 10, 11, 13 (clang), 14, case 5 on AArch64 and case 2 on i386;
and four cases RL-5 adds: `symver` (`foo@@V1` and a plain `foo`
reference), `debug-types` (the debug corpus with
`-fdebug-types-section`: local `wi.` signatures), `execstack` (an X
`.note.GNU-stack` input) and `meta` (`tests/meta`'s two objects with
Mason containers; not i386). RL-6 (WP-186) adds the D.3 C component
with `--build-id=sha1`, `-u nosuch`, `-e nosuch`, `-s`, `-x` and `-d`
(`opt-*`; mold refuses `-d`); with them lld shows A127 under `-s`.
Each case's anomalies per linker are the
test's table, which must equal what the run shows; a finding no
anomaly names fails the test. mold is not run on `cifunc`: mold 2.37.1
crashes (`SIGSEGV`) at `-r` on an input that defines an IFUNC (a
QUESTION to the supervisor: a crash is not an output to judge, record
WP-185).
