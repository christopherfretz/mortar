# spec: incremental linking (`mortar_incr`)

Status: **draft (WP-060; awaiting review)**. Design only: no shipped code
follows from this document until it is reviewed and its amendments (§9)
are applied. Milestone M7 (`docs/ROADMAP.md`); the work-package cut is §10.

Every number here was measured on this host (aarch64, 12 cores shared
with about fourteen other agents; the load average is given with each
measurement) by a script under `.build/review-incr/` of the workspace
(the appendix, §12, lists them and how to re-run them), or is derived
from mortar's own specs (cited by section), or is marked **not verified
here**. A figure marked *estimate* is a sum of measured parts, not a
measurement of the whole.

Rules of this spec are numbered **IN-n** (`spec/image.md` owns the `I-n`
numbers, and cites to its I-95–I-102 would be ambiguous otherwise). A
rule tagged **[v1]** is in the first implementation (§10, stages S1–S6);
**[later]** is a named optimisation with the condition that would make it
safe; **[fallback]** is a case the delta rules do not cover, where mortar
links from scratch and says why (§7).

## 0. Summary of the design

- **The unit of change is the input file.** An edit changes the bytes,
  the symbol-table order and the section indices of the whole object it
  touches, in every compiler measured (§1.2): nothing positional inside a
  changed object is reusable, and nothing outside it changes. The
  incremental link therefore re-reads changed files whole and never
  re-reads unchanged ones, except to refill a unit of theirs that refers
  to something that moved.
- **No indirection.** The output of an incremental link is a valid
  output that a full link *given the same layout* would write byte for
  byte (`spec/model.md` §7.3); there is no thunk table, no padding inside
  functions and no "incremental flavour" of code (`spec/layout.md`
  L-140, kept). Moving a definition costs refilling the units that name
  it: measured on protoc, the units in *other* files naming any global of
  one file are 3 at the median and 533 at the 99th percentile (8 KB and
  1.35 MB of bytes to refill, §1.4). That is cheap enough that the
  indirection prior art pays for (MSVC, Zig; §2) buys nothing here.
- **Slack is per (file, output section) region**, plus a reserve at the
  end of each output section and of each segment; a changed file is laid
  out afresh inside its own regions, so its intra-file references never
  need the index (§5.6).
- **The link state is a separate file beside the output**, bound to the
  output by its identity and its build-id root, never by a hash of its
  bytes (§5.3). Estimated 7–11 MB for protoc `-g` (a 50 MB output).
- **The dependency index is per unit, not per field**: one row per
  (referring unit, global symbol or section it names) for allocated
  sections; 118,145 rows on protoc (about 0.95 MB at 8 bytes), against
  1,776,710 relocations (§1.4). It also carries the liveness graph, so
  `--gc-sections` is recomputed without reading unchanged files.
- **Debug information is replaced per compile unit**: the new
  contribution is appended, and the old compile unit is **neutralized**
  (rewritten as an empty unit of the same length, its abbreviation table
  zeroed), not merely tombstoned. Merely tombstoning it is *not*
  equivalent: lldb then picks the dead unit's stale type as the "best
  match" and lists the dead variable (E6, §1.6). Neutralized, gdb 16.3's
  transcript equals a from-scratch link's and lldb 19's differs only in
  a placement offset, for GCC and Clang, DWARF 4 and 5 (E6b).
- **Changed inputs are found by stat, then content**: an input whose
  `(device, inode, size, mtime, ctime)` equals the state's, and whose
  ctime is older than the previous link's start ("racily clean"
  excluded, as git does), is unchanged; any other is read and hashed
  (§5.4). Hashing every input on every link would cost 120–220 ms on
  protoc (reading 217 MB, E2) of a 560 ms full link.
- **A plain link pays nothing** for any of this (§8.1): no slack, no
  state, no hashes, no index; ids stay per link.
- **The floor of an incremental link** on protoc `-g` (one function body
  edited in one object): *estimate* 40–60 ms with the object given
  directly or in a thin archive, 120–200 ms when the object is a member
  of a 117 MB regular archive (reading the archive to find the changed
  member dominates), against 560–780 ms for the full mortar link (§1.5).

## 1. The workload: what an edit changes

### 1.1 The census (E1)

`census.py` writes a two-unit C++ program (`a.cc`: 24 functions, string
helpers using `std::vector<std::string>`, a class with a virtual
function, a `thread_local`, globals; `b.cc`: 8 functions; both include
`lib.h`: an inline function, a class template, a struct, an abstract
class), applies one edit class at a time, recompiles both units with every
configuration and diffs the objects with `objdiff.py` (sections matched
by name and ordinal, relocations as multisets of `(offset, type, symbol
name, addend)`, symbols by name). Configurations: GCC 14.2 and Clang
19.1.7 × `-O0`/`-O2` × `-g`/none × `-ffunction-sections
-fdata-sections`/none, and kiln 1.2.1 + hod (kiln's driver with
`KILN_BACKEND=hod KILN_HOD_STRICT=1`, hod at 7e30d7c8) at `-O0` only (hod
takes `-O0` objects only, `kiln/spec/driver.md`). Base and edit compile
in the same directory, so `DW_AT_comp_dir` does not differ. 600 rows
(15 classes × 2 units × 20 configurations); load 20–30.

Table 1 (unit `a`, which the edit touches, unless the class edits the
header; "relocations differing" counts records of the old or the new
multiset that the other lacks, so a shifted record counts twice):

| edit class | `.text` size change: gcc -O0 / gcc -O2 fs / clang -O0 / kiln -O0 | relocations differing (gcc -O0; with fs) | `.eh_frame` bytes changed (gcc -O0) | symbols added/removed (values changed: gcc -O0; with fs) | `.debug_info` changed (gcc / clang / kiln, -O0 -g) | `.debug_line` changed (gcc / clang / kiln) | unit `b` |
|---|---|---|---|---|---|---|---|
| comment-line (a line inserted near the top) | 0 / 0 / 0 / 0 | 0/627; 0/627 | 0/3784 | 0/0 (0; 0) | 153 B / 156 B / 130 B | 53 B / 98 B / 50 B | unchanged |
| body-same (a constant changed) | 0 / 0 / 0 / 0 | 0; 0 | 0 | 0/0 | 0 | 0 / 0 / 16 B | unchanged |
| body-grow (two statements added) | +56 / +24 / +64 / +56 | 344/627; 12/627 | 8 | 0/0 (15; 0) | 2 B | 27% / 21% / 32% | unchanged |
| body-shrink (a statement removed) | −16 / +24 / −16 / −16 | 340; 12 | 2 | 0/0 (15; 0) | 1 B | 27% / 21% / 32% | unchanged |
| fn-add (a function in the middle) | +56 / +24 / +40 / +64 | 525/630; 209/630 | 2933/3808 | 1/0 (16; 92) | 99% / 99% / 99% | 26% / 20% / 32% | unchanged |
| fn-remove | −180 / −116 / −192 / −204 | 471; 211 | 2884 | 0/1 (14; 90) | 100% / 99% / 100% | 89% / 53% / 54% | unchanged |
| global-add (`int g_new = 5;`) | 0 | 96; 0 | 0 | 1/0 (1; 12) | 99% / 99% / 99% | 0 | unchanged |
| static-add (a static and its user) | +16 / +8 / +12 / +44 | 861; 241 | 3500 | 2/0 (30; 117) | 99% | 9% / 3% / 10% | unchanged |
| string-change (a literal) | 0 | 16; 16 | 0 | 0/0 | 24 B | 16 B / 0 / 16 B | unchanged |
| tls-add (a `thread_local` and its user) | +20 / +20 / +36 / +92 | 869; 241 | 3500 | 2/0 (30; 116) | 99% | 9% / 3% / 10% | unchanged |
| ctor-add (a static object with a constructor) | +88 / +12 / +88 / +116 | 190; 191 | 2329/3880 | 10/0 (85; 82) | 99% | 46% / 46% / 47% | unchanged |
| virtual-add (the vtable grows) | +28 / +12 / +32 / +52 | 170; 170 | 2309 | 2/0 (73; 75) | 99% | 45% / 53% / 54% | unchanged |
| inline-hdr (an inline function's body in the header) | +4 / 0 / +4 / +4 | 0; 0 | 2 | 0/0 | 1 B | 8% / 38% / 11% | changed the same way |
| template-hdr (a template member in the header) | +4 / +28 / +8 / +20 | 0; 0 | 2 | 0/0 | 1 B | 49% / 39% / 12% | changed the same way |
| layout-hdr (a field added to a struct) | 0 (unit `b`: +12) | 0; 0 | 0 | 0/0 | 99% | 0 | `.text` +12, `.debug_info` 99% |

The per-category view (`census.py --detail`, gcc and kiln at `-O0 -g
-ffunction-sections`) adds what the table compresses:

- **Every class that adds or removes a symbol or a section** changes the
  object's symbol-table order (`symtab order changed` in every such row),
  every `SHT_GROUP` section's member list (82 of 90 groups for gcc, 101
  of 107 for kiln, because section indices shift), `.eh_frame` from the
  insertion point on, and nearly all of `.debug_info` (DIE offsets
  shift), `.debug_str` (gcc: 9,974 to 52,815 bytes of 58,344, because the
  object's string table is reordered) and `.symtab`/`.strtab`.
- **Without `-ffunction-sections`, `.text` is one section per object**,
  so a body that grows shifts every later function: 344 of 627
  relocation records move (gcc), 368 of 676 (kiln). With it, 12.
- **A body edit with no size change** changes one byte of one section and
  nothing else; at `-g`, kiln also changes 16 bytes of `.debug_line`
  (the source file's MD5 in the DWARF 5 file table), GCC and Clang
  nothing.
- **A comment line** changes no allocated byte at all; only `.debug_info`
  (`DW_AT_decl_line`) and `.debug_line`.
- **hod's DWARF needs almost no relocations**: 5 relocation records in
  `.debug_info` for the kiln unit against 2,220 for GCC's (hod uses
  `.debug_str_offsets` and `.debug_addr`; `hod/spec/dwarf.md`), and one
  line-table sequence per function by design (`hod/spec/dwarf.md` §5.6).
- **A header edit** changes every including unit the same way (the
  inline function's or template's COMDAT copy in each); the keeper of the
  group stays the lowest live file (`spec/resolve.md` §6.1), so it is "the
  keeper's file changed", plus every other includer changed.
- **No edit class changed unit `b`** unless it edited the header. No
  edit class changed anything outside the objects of the units compiled.

### 1.2 What the census decides

1. **Granularity: the file.** In every class that is not a pure
   same-size body edit, the changed object differs in symbol order,
   section indices, group contents, `.eh_frame` and debug offsets. A
   design that tried to reuse rows *inside* a changed file (relocation
   indices, symbol indices, per-section `.eh_frame` pieces) would save
   nothing in the common case and must match old to new by anchor in the
   rest. The incremental link re-reads a changed file whole and refills
   every unit of it. Section **identity** (model §7.1's anchor) is still
   needed, to keep a changed file's region (IN-30) and, later, to let
   unchanged sections of a changed file stay in place (IN-31).
2. **Size changes are the common case.** 9 of the 15 classes change
   `.text`'s size; a body-grow of two statements adds 24–64 bytes. A
   design that is fast only for same-size edits (body-same, string-change
   at equal length) covers 2 classes of 15.
3. **The debug contribution of a changed unit changes as a whole**
   (99–100% of `.debug_info` whenever a symbol or section is added or
   removed). Replacement is per compile unit; a finer delta inside a unit
   is not worth designing (§6).
4. **Cross-file effects come only through symbols and COMDAT groups**:
   the global symbols a changed file defines or references, the groups it
   holds, archive extraction (which members are pulled), and synthetic
   tables (GOT, PLT, `.init_array`, `.eh_frame_hdr`, `.symtab`).

### 1.3 Archives in the development loop

The protoc workload (`spec/harness.md` "bench", `cxx-medium-g`; static
twin `cxx-medium-g-static`) links `main.cc.o`, `libprotoc.a` (116.8 MB,
84 members) and `libprotobuf.a` (84.7 MB, 83 members) with
`libstdc++.a`, `libc.a` (5.6 MB, 2022 members), `libm.a`, `libgcc.a`,
`libgcc_eh.a`: 217 MB of inputs of which 164 MB are in the 784 live
files (E3). An edit of one protobuf source rebuilds one object and then
the **archive** (CMake's default), so the common protoc edit reaches the
linker as "a regular archive changed". GNU `ar` rewrites the whole file;
the unchanged members keep their bytes (Debian's `ar` is deterministic,
`D`, so their headers too) but move. Finding the changed member means
reading the archive: 80–160 ms for `libprotoc.a` (`cat` to `/dev/null`,
three runs, load 29). A thin archive (`ar T`) names members as files and
needs one `stat` each (4.3 µs measured per `stat`, E2). §8.3 asks kiln's
driver to recommend thin archives in its development profile.

### 1.4 The shape of a big link (E3)

`linkstats.py` over the live files of the protoc `-g` static link (map
from `--mortar-map`):

| quantity | protoc `-g` |
|---|---|
| live files | 784 |
| live input bytes | 164,059,792 |
| allocated input sections placed | 27,901 (7.07 MB) |
| non-allocated contributions | 1,001 (82.6 MB with string sections) |
| split pieces (`.eh_frame` records, merged pieces) | 35,684 |
| symbol-table entries: global / local | 76,402 / 163,381 |
| relocations: alloc / non-alloc | 238,919 / 1,537,791 |
| distinct (referring section, target) pairs: alloc / non-alloc | 118,145 / 170,477 |
| of which the target is a global symbol: alloc / non-alloc | 67,661 / 104 |
| units in **other** files naming a global of one file: p50 / p90 / p99 / max | 3 / 48 / 533 / 2,107 |
| bytes of those units: p50 / p90 / p99 / max | 8.2 KB / 270 KB / 1.35 MB / 3.0 MB |
| referring units per global: p50 / p90 / p99 / max | 1 / 4 / 21 / 2,100 |
| per-file allocated bytes: p50 / p90 / p99 / max | 502 B / 19.9 KB / 135 KB / 499 KB (`descriptor.cc.o`) |
| per-file debug bytes (`nplace`, strings excluded; 127 files with `-g`): p50 / p90 / p99 / max | 210 KB / 406 KB / 823 KB / 1.50 MB |
| output | 50.3 MB: `.debug_info` 25.9 MB, `.debug_str` 8.1 MB (76,974 strings), `.text` 4.4 MB, `.eh_frame` 0.97 MB, `.symtab` + `.strtab` 5.4 MB (31,478 globals, 97 bytes of name each on average) |

Three consequences:

- **Debug relocations never cross files** (104 of 170,477 pairs name a
  global): a debug contribution refers to its own file's sections through
  section symbols. Moving a section therefore never refills another
  file's debug units, and the index need not cover non-allocated
  sections (IN-25).
- **A unit-level index is 15 times smaller than a field-level one**
  (118,145 pairs against 1,776,710 relocations), and since an incremental
  link refills whole units (model §7.2), it loses nothing.
- **Moving everything one file defines is cheap**: refilling every unit
  in other files that names one of its globals is 1.35 MB at the 99th
  percentile. Patching through the index, not indirection, is the right
  mechanism (§2.7).

### 1.5 The time of a full link and the floor of an incremental one (E2, E5)

Full mortar links of the bench's static protoc (release build of this
branch; five runs each, load 21–23; `--time-phases`):

| link | wall (5 runs) | peak RSS |
|---|---|---|
| protoc `-O0 -g` static (50.3 MB out) | 781, 592, 565, 604, 571 ms | 246–254 MB |
| protoc `-O0` static (11.8 MB out) | 471, 355, 379, 345, 316 ms | 143–147 MB |

Phases of a typical `-g` run (565 ms): input 240, resolve 32, assignment
13, split 60, liveness 2, scan 9, synthetic 5, layout 68, image 128 ms.
Without `-g` (316 ms): input 98, resolve 24, assignment 11, split 10,
scan 10, synthetic 17, layout 96, image 43 ms.

The parts of an incremental link, measured on this host:

| part | measured | source |
|---|---|---|
| `stat` of one input | 4.3 µs (8 files × 1000) | E2 |
| read 217 MB of inputs (all of protoc's) | 120–220 ms (`cat`, 3 runs) | E2 |
| read `libprotoc.a` (117 MB) | 80–160 ms | E2 |
| hash, `L128` | 85 ms per GiB on one thread (12.6 GB/s) | `spec/image.md` I-74 |
| positioned write of 4 KiB / 64 KiB / 1 MiB / 16 MiB into a 1 GiB file | 0.02–0.76 / 0.06–0.10 / 1.4–2.0 / 20–34 ms | E5 |
| the same followed by `fdatasync` (4 KiB / 1 MiB / 8 MiB, 50 MB file) | 0.1–0.9 / 1.3–3.4 / 6.8–9.2 ms | E5 |
| copy of 100 MiB / 1 GiB (`copy_file_range`; read/write) | 54–142 / 1785–2913 ms; 47–65 / 2490–2831 ms | E5 |
| reflink (`FICLONE`) on the container's overlay file system | not supported (`ENOTSUP`) | E5 |
| `rename` of a 1 MiB file over a 1 GiB one | 198 ms (the old blocks are freed) | E5 |
| open a running executable for writing | `ETXTBSY`; `rename` over it is allowed | E5 |

The floor of "one function body grew within its region, `-g`, protoc"
(*estimate*, the sum of the parts; the prototype that would measure it is
stage S1's, §10):

| step | thin archive or plain object | regular archive |
|---|---|---|
| stat the inputs and probes (~100) | < 1 ms | < 1 ms |
| read and hash the changed input | ~1 ms (one 1–2 MB object) | 80–160 ms |
| load and validate the state (7–11 MB) | ~10 ms | ~10 ms |
| parse the changed file, re-resolve over stored summaries | ~5 ms (resolve is 32 ms for 784 files) | ~5 ms |
| refill: the file's units (p99 135 KB alloc + 823 KB debug) and the fan-out (p99 1.35 MB) | ~5–10 ms | ~5–10 ms |
| `.symtab`/`.strtab` rewritten whole (5.4 MB, S-112) | ~10 ms | ~10 ms |
| write dirty windows (2–8 MB) | 3–15 ms | 3–15 ms |
| write the state (7–11 MB) | ~10–15 ms | ~10–15 ms |
| **total** | **~40–60 ms** | **~120–200 ms** |

against 565–780 ms for the full link: about 10× and 3–5× faster.
Without `-g` the full link is 316–471 ms and the floor about the same
minus the debug refill. The two largest items of the regular-archive
column (reading the archive; rewriting `.symtab`) are the first targets
of later optimisation (IN-24's parallel member hashing; S-112's per-file
`.symtab` capacity, synth.md Q8).

### 1.6 Debug information under replacement (E6, E6b)

`debug_dead_cu.sh` builds `a.cc` (main), `b.cc`, `c.cc` and an edited
`b2.cc` (one line more, a body that grew, a struct with one more field,
a global with a new initializer) with GCC and Clang, DWARF 4 and 5,
`-O0 -g -ffunction-sections -fdata-sections`, and links them statically
with mortar:

- **full**: `a.o b2.o c.o`: the from-scratch link.
- **emul**: `a.o bold.o c.o b2.o --gc-sections`, where `bold.o` is the
  old `b.o` with its globals renamed (`objcopy --redefine-sym`; DWARF
  names untouched): its code is collected and its debug contributions are
  kept with mortar's tombstones (`spec/image.md` I-47), and the new unit
  comes last. These are the bytes of the rule "tombstone the old
  contribution, append the new".
- **pad**: `emul` with the dead unit neutralized by `dwarf_pad.py`: its
  `.debug_info` unit keeps its length and becomes one childless
  `DW_TAG_compile_unit` DIE with no attributes followed by null entries;
  its abbreviation table becomes `01 11 00 00 00 00` followed by zero
  bytes to the table's end; every other dead contribution
  (`.debug_line`, `.debug_str_offsets`, `.debug_addr`,
  `.debug_rnglists`, `.debug_aranges`) is left as mortar wrote it.

A gdb 16.3 session (break, run, backtrace, `info line`, `info source`,
`list`, `print`, `ptype`, `next`, `info locals`, `finish`, `info
functions`, `info variables`, `break FILE:LINE`, `info scope`) and an
lldb 19.1.7 session (the same through lldb's commands), addresses and
process ids masked, compared with the full link's:

| | gdb, emul | lldb, emul | gdb, pad | lldb, pad | `llvm-dwarfdump --verify` |
|---|---|---|---|---|---|
| g++ DWARF 4 | differs: `info variables g_b` lists the dead `b.cc:1 int g_b` | differs: `target variable g_b` adds `<invalid file address>`; **`image lookup -t BS` picks the dead 8-byte `BS` of `b.cc`** | equal | equal except one placement offset (`.text + 1152` / `+ 1196`) | ok, all three |
| g++ DWARF 5 | as above | as above | equal | as above | ok |
| clang++ DWARF 4 | as above | as above (`<invalid file address>`) | equal | as above (`+1164` / `+1204`) | ok |
| clang++ DWARF 5 | equal | as above (`read memory … failed`) | equal | as above | ok |

So tombstoning alone leaves a dead unit that debuggers still enumerate:
its types and variables are found by name, and lldb shows the stale type
as the best match, which is wrong information. A neutralized unit is
invisible to both debuggers. One subtlety measured on the way: the first
version of `dwarf_pad.py` rewrote only the first six bytes of the dead
abbreviation table; lldb then warned `unsupported DW_FORM value` and lost
the line information of the *live* `b_f`, because lldb parses
`.debug_abbrev` as a sequence of tables and read the dead table's
remaining bytes as the next table. Zeroing the whole dead table (each
zero byte is an empty table) fixed it. §6 turns this into rules.

## 2. Prior art and mortar's positions

None of the incremental linkers below could be run here: `ld.gold` is
not installed (GNU binutils 2.44 deprecates it), and the others are not
ELF linkers for Linux or not packaged. Everything in this section is
**not verified here**, cited from public documentation and reports as the
author knows them.

### 2.1 MSVC `link.exe /INCREMENTAL`

The state is a separate file, the `.ilk`, beside the output. Every
function is reached through the **incremental linking table** (ILT): a
`jmp` thunk per function, so that a function's address is its thunk's and
a moved function updates one thunk. Functions and data are padded so
they can grow in place (padding filled with `int3`). Incompatible with
`/OPT:REF`, `/OPT:ICF`, `/ORDER` and `/LTCG` (the linker warns `LNK4075`
and links fully), and it falls back to a full link when the `.ilk` is
missing or does not match, when padding is exhausted, or when too much
changed. The ILT stays in the program: every call costs an extra jump,
function pointers compare to thunks, and the image is larger. Edit and
Continue (`/ZI`) builds on the ILT and the padding. Reported problems:
stale or corrupt `.ilk` files (the folklore fix is to delete them),
large `.ilk` files, `PDB` mismatch errors.

### 2.2 GNU gold `--incremental`

Options: `--incremental` (update if possible, else full),
`--incremental-full`, `--incremental-update`, `--incremental-base=FILE`,
`--incremental-patch=PERCENT` (free space reserved per output section,
default 10%), and build-system hints `--incremental-changed`,
`--incremental-unchanged`, `--incremental-unknown` that apply to the
following inputs. The state is stored **inside the output** in
`.gnu_incremental_inputs`, `.gnu_incremental_symtab`,
`.gnu_incremental_relocs` (the relocations that refer to global symbols,
so that they can be re-applied), `.gnu_incremental_got_plt` and
`.gnu_incremental_strtab`. There is no indirection: changed objects are
placed into free space and the references to their globals patched from
the stored relocations. It refused (full link, with a message) linker
scripts, `--gc-sections`, `--icf`, `-r`, and most of what made a link
unusual; it was finished for x86-64 only. It was little used and never
matured, its author moved on, and binutils deprecated gold as a whole in
2.44. What users reported: it did not save much over gold's already fast
full links, and they could not tell when it fell back.

### 2.3 Zig's self-hosted linker

Zig's compiler and linker are one program; in debug builds of Zig code
it patches the output in place. Calls go through an offset table (a
GOT), so moving a function rewrites one slot; each "atom" gets surplus
capacity (about a third) and a free list tracks holes. It works because
Zig controls code generation; it does not apply to C objects from other
compilers. As of Zig 0.14 incremental compilation (`-fincremental`) was
still experimental.

### 2.4 wild

A Rust ELF linker whose author has stated incremental linking (and,
further, hot reloading) as long-term aims, with in-place updates and
spare space. As far as the author knows nothing of it is released; its
plans are cited as plans.

### 2.5 mold and lld

Neither has an incremental mode for ELF. mold's author has argued that a
full link made fast enough removes the need, and that incremental linking
is complicated and fragile for the gain. lld's COFF driver accepts
`/INCREMENTAL` for compatibility and links fully. Their full links are
the bar (`spec/layout.md` §18.1: an incremental link that does not beat a
full mortar link is not used).

### 2.6 Apple and hot patching

Apple's ZeroLink (Xcode 1–2) avoided linking during development by
loading objects at run time; it was removed in Xcode 3. ld64 has no
incremental mode. Hot patching a *running* program (Edit and Continue;
Live++ through hot-patchable function prologues;
`-fpatchable-function-entry`) is out of scope for mortar. The hook it
would need is the link state itself: it maps every symbol to its address
and every unit to the units that name it (§5.3), which is what a patcher
needs to place new code and redirect old entry points. tuck can ask for
it after M7; nothing here forecloses it.

### 2.7 mortar's positions

| question | mortar | why |
|---|---|---|
| indirection or patching | **patch through the index**; no thunk table, no padding inside code | a full link given the layout must produce the same bytes (model §7.3); GCC and Clang objects have no indirection to use; measured fan-out (§1.4) makes patching cheap |
| where the state lives | **a separate file beside the output** (`<output>.mortar-state`) | gold's in-output state is stripped by `strip`/`objcopy` and copied by `cp`, and would make an incremental-mode output differ from a plain one in its sections; the cost is binding the state to the output (IN-21) |
| when unsure | **full link with a named reason**, whose output is ready for the next incremental link | gold's silent fall-backs were its users' main complaint |
| what a plain link pays | nothing (§8.1) | operator, 2026-11-15 |
| hints from the build system | accepted only to force more checking, never less | a wrong "unchanged" hint would give a wrong output silently (IN-23) |

The architecture's starting position ("Incremental linking",
`docs/ARCHITECTURE.md`) is kept in shape and corrected in three places,
each with its measurement: the index is per unit and mostly per global
(§1.4), not "from each symbol to the fields that refer to it"; per-section
content hashes are not needed for correctness (IN-22); and debug
contributions are neutralized, not only tombstoned (§1.6).

## 3. Correctness

An incremental link is a link. Everything mortar promises of a link
(`docs/ARCHITECTURE.md`: correct or refused by name, checked by the
verifier, deterministic) holds of it, and it adds one promise: the
program is the program a from-scratch link would have made.

- **IN-1 [v1] The three invariants.** For every incremental update:
  - **(A) verified**: `mortar_plumb` accepts the output with its map,
    exactly as for a full link (§8.2);
  - **(B) replayable**: a full link *given the update's decision tables*
    (model §7.3; holes and neutralized debug units are entries of those
    tables, IN-3, IN-51) writes the same bytes, compared byte for byte
    (image.md I-98);
  - **(C) equivalent**: the output is equivalent (IN-2) to a from-scratch
    full link of the same inputs and options.

  (A) and (B) are the model's two invariants; (C) is new, and is what the
  user relies on. (B) alone would accept an update that kept a stale
  definition, because the stale tables would replay faithfully.
  *Test:* L10 (§8.2) checks all three after every step.
- **IN-2 [v1] Equivalence.** Two outputs `X` (incremental) and `F` (full)
  of the same inputs and options are equivalent when:
  1. they have the same output sections and segments, with the same
     names, types, flags and order;
  2. the same input sections and merge pieces are live in both, and each
     is placed exactly once;
  3. every global symbol resolves to the same definition (file and
     section identity, offset within the section), with the same type,
     size, binding and visibility; the same symbols are in `.symtab` and
     `.dynsym`, with values that differ only by placement;
  4. every relocated field holds, in `X`, the value it holds in `F` after
     both are mapped through the placement (a field naming input section
     `s` at offset `o` names `s` at `o` in both), and relaxations and
     slots are those of the same demand flags;
  5. the synthetic tables (GOT, PLT, `.eh_frame_hdr`, `.init_array`
     order, dynamic tables, notes) have the same entries in the same
     order where order matters (`.init_array`, `.rela.plt`'s
     `IRELATIVE`s, `DT_NEEDED`), up to the placement mapping;
  6. every byte of `X` not covered by 1–5 is a hole filled per IN-3, or
     a neutralized debug unit (IN-51), or a dead debug contribution left
     whole (IN-53);
  7. the diagnostics are the same (IN-5).

  Equal bytes are not required: placement differs (slack, holes, a
  region moved to a tail), and so do the build id and every address.
  *Test:* `mortar_plumb --equiv X.map F.map` (an extension of the map
  checks, §8.2) checks 1–3 and 5 from the two maps and 4 by re-deriving
  each field through both placements; a program-level check runs both
  (L10).
- **IN-3 [v1] Holes and scrubbing.** Every byte of the output that no
  live unit owns is a **hole** and holds its section's fill:
  - code (`SHF_EXECINSTR`): the target's trap fill, as layout L-143
    already requires of gaps (layout.md L-82: `UDF #0` words on AArch64,
    `int3` on x86), so a stale pointer into removed code traps instead of
    running stale instructions;
  - other allocated `PROGBITS`: zero; `NOBITS`: nothing in the file, zero
    in memory;
  - `.eh_frame`: a padding CIE that no FDE uses (synth.md S-111);
  - `.rela.dyn`'s `RELATIVE` prefix: a `RELATIVE` record aimed at the
    scratch word (dyn.md §14); after the prefix, `R_*_NONE`;
  - debug sections: §6.
  No byte of a removed or moved unit survives outside a dead debug
  contribution (IN-53): a hole is written with its fill when it is made.
  A hole is an entry of the decision tables (its section, offset, size),
  so (B) reproduces it and the map lists it (`hole` lines, §8.2).
  *Test:* remove a function and check that its old range is all trap
  fill (`plumb` checks every `hole` line's bytes).
- **IN-4 [v1] Never refuse what a full link accepts.** An update that
  meets a case the delta rules do not cover links from scratch (§7), in
  the same process, and reports why. The full link's result (output or
  errors) is the result. *Test:* each fall-back reason has a scenario in
  L10 that checks the reason line and that the output equals a plain
  full link's in equivalence (IN-2).
- **IN-5 [v1] The same diagnostics.** An update reports exactly the
  diagnostics a full link of the same inputs would: errors and warnings
  found in the changed files are found again; warnings of unchanged files
  come from the state (image.md I-100); a warning attached to an
  unchanged file whose cause changed (a `gnu-warning` symbol now
  referenced from elsewhere, `--warn-unresolved-symbols`) is recomputed,
  because resolution is (IN-26). On failure the update does what a full
  link does (image.md I-65, I-66: the output is removed) and also removes
  the state. *Test:* L10 steps that introduce and then fix an undefined
  symbol; the diagnostics are compared with a full link's.

## 4. Determinism

- **IN-6 [v1] A function of its history.** The output and the new state
  of an update are a function of (inputs, options, previous state) and
  nothing else: not of `--threads`, the schedule, the time, or what else
  is in the directory. The same sequence of edits replayed from the same
  first link gives the same bytes. *Test:* L10 replays each sequence
  twice, with `--threads=1` and the default, and compares outputs and
  states byte for byte.
- **IN-7 [v1] Not reproducible across histories.** Two developers with
  different edit histories get different (equivalent, IN-2) bytes. So
  incremental mode is for development; reproducible and release builds
  link plainly. `docs/` and `--help` say so; a link with
  `SOURCE_DATE_EPOCH` set and `--incremental` gives a warning
  (`incremental-not-reproducible`) once per link. *Test:* a link test
  with the variable set checks the warning.
- **IN-8 [v1] The first link.** A full link in incremental mode (no
  state, or a fall-back) is deterministic in the ordinary sense: the
  output is that of a plain link with the incremental slack policy
  (§5.6), plus a state. It differs from a plain link of the same inputs
  only in placement and holes. *Test:* equivalence (IN-2) of an
  incremental-mode first link with a plain link, over the link-test
  corpus.

## 5. The incremental link

### 5.1 Options and modes

- **IN-10 [v1] Spellings.** gold's are the prior art (§2.2); mortar
  keeps its main word and folds the variants into a value:

  | option | meaning |
  |---|---|
  | `--incremental` | update the output from its state if possible; otherwise link from scratch, with slack, and write a state |
  | `--incremental=full` | link from scratch in incremental mode (gold's `--incremental-full`) |
  | `--incremental=update` | update or fail: a fall-back reason is an error (`incremental-update-failed`) naming the reason, and nothing is written; for tests and for users who want to know (gold's `--incremental-update`) |
  | `--no-incremental` | the default: a plain link; never reads or writes a state |
  | `--incremental-state=PATH` | where the state is (default `<output>.mortar-state`, IN-17) |
  | `--incremental-report[=FILE]` | one line per link to `FILE` (default stderr): `incremental: update` or `incremental: full (<reason>)`, then `changed=<files> refilled=<units> bytes=<written> time=<ms>` |

  A fall-back is reported without `--incremental-report` too, as one
  note (`incr-full-link`, `spec/diag.md`) naming the reason; an update
  that succeeds prints nothing. gold's `--incremental-changed`,
  `--incremental-unchanged` and `--incremental-unknown` are not taken
  (IN-23); `--incremental-base` neither (the state names its output).
  *Test:* a link test per row of the table.
- **IN-11 [v1] A plain link never looks.** Without `--incremental`,
  mortar does not open, stat or remove a state file. A state left beside
  an output that a plain link replaced no longer matches it (IN-21) and
  the next incremental link says `output-changed`. *Test:* plain link
  over an incremental output, then `--incremental`, checks the reason.
- **IN-12 [v1] Incompatible modes.** With `-r`, `--emit-relocs`, a
  `SECTIONS` script, `--compress-debug-sections` other than `none`,
  `--gdb-index`/`--debug-names` generation or `--icf`, `--incremental`
  gives one warning (`incremental-unsupported`, naming the option) and
  links plainly, writing no state; with `--incremental=update` it is an
  error. These are §7's "unsupported" rows: a state for them would never
  be used.

### 5.2 The update, phase by phase

The update runs the ordinary pipeline (`spec/pipeline.md`, model §8)
with each phase fed from the state for unchanged files:

| phase | in an update |
|---|---|
| command line, plan | as always; then the normalized plan is compared with the state's (`options-changed`) |
| inputs | `stat` every input and probe (IN-22); read and hash the changed ones; parse only changed files |
| resolution | in full over per-file summaries: the state's for unchanged files, parsed ones for changed (IN-26) |
| liveness | in full over the stored graph with changed files' edges replaced (IN-29) |
| split, scan | changed files only, plus the units IN-27 refills (their relocations are re-read from the unchanged inputs by position) |
| synthetic | the decision tables extended or rewritten per §5.7 |
| layout | the previous layout plus §5.6's moves; never a fresh layout |
| image | fill the dirty windows (image.md I-95) into memory, then write them (§5.9) |
| state | written last (IN-46) |

- **IN-13 [v1] Everything is decided before anything is written.** The
  update fills every dirty window into memory before the first write to
  the output or the state; any error or fall-back is found before then.
  A fall-back therefore never leaves a half-updated output, and the full
  link that follows starts from untouched files. *Test:* inject each
  fall-back reason and check that the output's bytes before the full
  link equal the previous output's.
- **IN-14 [v1] Refilled units of unchanged files** are read with
  positioned reads of the section and its relocation section, at the
  file offsets the state records, and the file's symbol table; the
  file's content hash (unchanged, IN-22) is what makes the offsets valid.
  An unchanged archive member is read the same way, at its member
  offset. *Test:* covered by (B) and (C) of IN-1.
- **IN-15 [v1] Threads.** The update uses the same parallel phases as a
  full link; determinism is IN-6.
- **IN-16 [v1] Too much changed.** When the units to refill hold more
  than half of the output's allocated bytes, or more than half of the
  inputs changed, the update falls back (`too-much-changed`): a full
  link costs about the same, and compacts. The threshold is a constant
  of the policy, reviewed at S4 (§10) with L10's statistics.

### 5.3 The link state

- **IN-17 [v1] Where.** A separate file, `<output>.mortar-state` in the
  output's directory unless `--incremental-state` says otherwise; written
  as a temporary file in the same directory and renamed (IN-46), so a
  reader never sees half a state. Not inside the output: gold's
  in-output sections are stripped by `strip`, copied by `cp` and
  `install` with the output, and would make an incremental-mode output
  differ in its sections from a plain one (§2.7).
- **IN-18 [v1] Format and version.** Little-endian, fixed-width rows,
  no pointers; a header of magic `MORTARST`, a format version (u32), the
  mortar version string (any difference: `state-version`), the target,
  and the L128 hash (image.md I-114) of the normalized plan; then
  tables, each a (kind, row count, byte length, L128 of the bytes)
  record followed by its rows; names in one string pool referenced by
  offset. A state is untrusted input (model §7.2): every table's hash,
  every count, every offset and every identity is checked before use;
  any failure is `state-corrupt`, never a panic. The format is specified
  table by table at S1 (§10) in this spec, with a fuzzing target for the
  reader. *Test:* the fuzzer; a truncated state, a flipped byte, a state
  of another output: each gives its reason.
- **IN-19 [v1] Contents** (model §7.2, in identity terms, model §7.1):

  | table | rows | protoc `-g`, estimated (E4) |
  |---|---|---|
  | header, plan, options | 1 | < 1 KB |
  | files: identity, `(dev, ino, size, mtime, ctime)`, L128 content hash, probes, member offset, symbol-table offset and size | one per input file and archive member (~2,500 with unextracted members) | 0.15 MB |
  | sections: file, anchor, name, flags, type, size, alignment, file offset, relocation-section offset and count, live, placement (output section, offset) | one per input section kept or discarded (54,000) | 0.7 MB |
  | symbol summaries per file: key, binding, type, visibility, size, defined section and value or undefined, COMDAT group | 239,783 (76,402 global and 163,381 local mentions; locals only where a section symbol is not enough) | 1.5 MB plus names |
  | resolution: global key → defining (file, section, value), and demand flags with the files contributing each | 31,478 globals | 0.5 MB |
  | dependency index (IN-25) | 118,249 pairs | 0.95 MB |
  | decision tables: chunks, regions, holes, slots, pieces, merged maps (fragment hash → offset), islands, dynamic slices | 35,684 pieces, ~4,000 regions | 0.3 MB plus the merged maps, ~1.5 MB for `.debug_str`'s 76,974 fragments |
  | debug layout: per file and debug section, the contribution's offset and length; dead contributions | ~1,500 | < 0.1 MB |
  | warnings (`Acc` vectors, image.md I-100) | per file with warnings | small |
  | build-id leaves (image.md I-76: 16 B per MiB) | 50 | < 1 KB |
  | string pool: symbol names and paths | | 3.9 MB (7.4 MB if local names are kept) |
  | **total** | | **about 7–11 MB** (14–22% of the 50.3 MB output) |

  Not stored: section bytes and relocations (re-read from the inputs,
  IN-14), anything recomputable from the plan. A field-level index (one
  row per relocation, model §7.2 read literally) would be 1,776,710 rows,
  about 28 MB; the unit-level one is 0.95 MB. *Test:* the state's size is
  in the bench's incremental rows (§8.2); a regression of more than 20% is
  flagged.
- **IN-20 [v1] Written by streaming.** The state's tables are written as
  the phases produce them: summaries after input, the index during the
  scan (each worker emits its units' pairs, sorted and deduplicated per
  unit), decision tables after layout. No phase keeps a copy for the
  state's sake beyond what it already holds. *Test:* peak RSS of an
  incremental-mode full link is within 10% of a plain link's (bench).
- **IN-21 [v1] Bound to its output.** The state records the output's
  `(dev, ino, size, mtime_ns, ctime_ns)` read by `fstat` after the last
  write, and its build-id. An update proceeds only if `stat(P)` gives the
  same tuple and the build-id note read from `P` (one positioned read at
  the offset the state records) is the same; otherwise `output-changed`.
  A hash of the output's bytes is not taken: it would read 50 MB per
  link. A ctime equal to the state file's own write time or later is
  treated as changed (the racy case, IN-22). The state also has the
  "updating" flag of IN-46: set means `output-torn`. *Test:* `touch`,
  `strip`, `cp` of another output, `objcopy` in place, each over the
  output: each gives `output-changed`; a killed update gives
  `output-torn`.

### 5.4 Change detection

- **IN-22 [v1] Stat, then content, per file.** An input (file or
  archive) is unchanged when its path resolves as before (the probes of
  `spec/input.md` §4.6 re-checked by `stat`), its `(dev, ino, size,
  mtime_ns, ctime_ns)` equals the state's, and its ctime is strictly
  older than the start of the link that wrote the state ("racily clean"
  inputs, modified in the same clock tick as the previous link read them,
  are hashed, as git does for its index). Any other input is read and
  its L128 hash compared: equal means unchanged (a `touch`), different
  means changed. Content hashes are per file; per-section hashes are not
  needed for correctness, because a changed file is refilled whole (§1.2)
  and kiln and hod need not supply them (§8.3). `--incremental-verify`
  hashes every input regardless of stat (120–220 ms extra on protoc),
  for file systems whose timestamps cannot be trusted. *Test:* L10 steps
  that rewrite an input with the same size and an `mtime` set back
  (`touch -d`), that `touch` without changing, and that replace a file by
  `rename` (a new inode).
- **IN-23 [v1] Hints only ever add work.** No option or input from a
  build system or from kiln can mark an input unchanged: a wrong
  "unchanged" would give a wrong program silently, which is the one
  failure mortar may not have. `--incremental-changed=PATH` (repeatable)
  marks an input changed even when stat says otherwise (for build
  systems that restore timestamps). *Test:* an input rewritten with its
  old stat tuple (`touch -d` plus same size) and the hint gives the new
  program.
- **IN-24 [v1] Archives.** A regular archive that changed (IN-22) is
  read whole; its members are matched by identity (member name and
  ordinal among same-named members, model §7.1) and each member's L128
  hash compared, so only changed members count as changed files; added
  and removed members change the archive's symbol index, and resolution
  (IN-26) decides what is extracted. A thin archive's members are files
  and are stat'ed as such. [later] Read and hash a changed archive's
  members in parallel (the 80–160 ms of `libprotoc.a` is one thread's
  read, §1.3). *Test:* L10 replaces a member, adds one, removes one, and
  reorders the archive (`ar` with `-b`).

### 5.5 Delta resolution and liveness

- **IN-25 [v1] The dependency index.** One row per distinct (referring
  unit, target), from the relocations of:
  - every allocated unit (input section, synthetic chunk unit), where
    the target is a global symbol key, a section of the same file
    (through a section or local symbol), or a merged fragment's input
    section; and
  - every non-allocated unit whose target is a global symbol (104 pairs
    on protoc).
  Non-allocated units naming their own file's sections are not indexed:
  when any section of a file moves, all of that file's non-allocated
  units are refilled (p99 823 KB). Read backwards (target → units), the
  index is model §7.2's; read forwards, its allocated rows are the
  liveness graph (IN-29). End symbols (`__init_array_end`, `_end`,
  `__bss_start`, `__start_SEC`, `__stop_SEC`, `__rela_iplt_end`, …)
  are global targets like any other, so their users are found through
  the index: layout.md Q4 is answered by "the index records them",
  and growth is not refused. *Test:* (B) and (C) of IN-1 after edits that
  move each kind of target; a unit-test that the index of a full link
  equals the index rebuilt from all relocations.
- **IN-26 [v1] Resolution in full.** The update re-resolves every symbol
  (`spec/resolve.md` §13: every input-list change is a full resolution,
  and it is cheap: 32 ms for protoc) over per-file summaries, the
  state's for unchanged files and parsed ones for changed files,
  including archive extraction (members can be pulled in or dropped).
  Its difference from the stored resolution is the delta: per global
  key, a new definer, a new value or size, a new kind (defined,
  undefined, common, absolute, dynamic), or a changed demand flag; per
  COMDAT group, a new keeper; per file, extracted or no longer extracted.
  *Test:* L10 edit classes that move a definition between files, change
  a COMDAT keeper (edit the header of the lowest file's group), and pull
  a new archive member in.
- **IN-27 [v1] What is refilled.** The units refilled are the union of:

  | cause | units |
  |---|---|
  | a file changed, appeared or (as an archive member) became extracted | all of its units |
  | a file disappeared or is no longer extracted | none: its units become holes (IN-3) |
  | a global's definition moved or changed (value, size, definer, kind) | the index's referrers of that global, and its slots' units (GOT, PLT, `.dynsym` per §5.8) |
  | a COMDAT group's keeper changed | the old keeper's group members become holes; the new keeper's are placed (§5.6) and refilled; the referrers of the group's symbols |
  | a demand flag of a global changed (`GOT_NORELAX`, `TLSIE_NORELAX`, `CANON_PLT`, `COPY`; model §7.2) | the referrers of that global |
  | a section of an unchanged file moved (a growth step, L-144) | that section's unit; the index's referrers of the section and of the globals defined in it; all non-allocated units of its file |
  | a synthetic table changed (§5.7) | its units, and the referrers of its end symbols |
  | a patch or thunk was added, moved or removed (arch.md "Incremental links") | the sites' units and the island's |
  | liveness changed (IN-29) | sections that became live are placed and refilled; sections that died become holes; the referrers of their globals |

  Units are refilled whole (model §7.2): relaxations, sorted tables and
  encodings need no special case. *Test:* IN-1 (B) and (C); a unit test
  per row with a two-file program.
- **IN-28 [v1] Ids are per link.** Ids are assigned afresh by each link
  in the order a full link assigns them (load order), and the state
  speaks identities (model §7.1), translated on load. No stable id is
  needed: every rule that orders by id (COMDAT keeper = lowest live
  file, `.init_array` order, slot order at a first link) is a rule on
  load order, which identities preserve; slots added by an update are
  appended (S-113) and recorded in the decision tables, which (B)
  replays. A plain link pays nothing. *Test:* (B) after an edit that
  adds a file in the middle of the command line.
- **IN-29 [v1] Liveness** under `--gc-sections` is recomputed in full by
  walking the stored graph (IN-25's allocated rows of unchanged files,
  plus the changed files' fresh rows) from the roots, without reading
  unchanged files. This amends image.md I-101 (which walks the
  relocations). Its difference from the stored `live` column is IN-27's
  last row. *Test:* L10 removes the last reference to a function in
  another file and checks that the function's section became a hole.

### 5.6 Delta layout and slack

- **IN-30 [v1] Regions.** In incremental mode, layout (layout.md §18)
  groups each file's contribution to each output section and category
  (the GNU `.text` categories, L-53, are categories) into one **region**:
  the file's chunks in their full-link order, followed by slack. A
  changed file's chunks are laid out again from its region's start in
  their order; if they fit, nothing outside the region moves. Units of
  other files that name the changed file's globals are refilled through
  the index (IN-27); measured, that is 3 units at the median and 533 at
  the 99th percentile (§1.4). Slack is past `size` only (L-141); a
  region's slack and the hole a shrinking file leaves are holes (IN-3).
  *Test:* L10 body-grow and body-shrink steps check that only the
  region's file and its referrers were refilled (`--incremental-report`).
- **IN-31 [later] Stable places inside a changed file.** A section of a
  changed file whose anchor (model §7.1) and size are unchanged keeps
  its place, and the others are fitted around it, so that referrers of
  its globals are not refilled. Worth it only if L10's statistics at S4
  show refill of referrers dominating an update; it needs identity
  matching inside a changed file that v1 does not do.
- **IN-32 [v1] Overflow.** A region that no longer fits its contribution
  becomes a hole, and the contribution is placed at the **tail reserve**
  of its output section (category tail), with a fresh region's slack. If
  the tail reserve is exhausted, the output section grows by a growth
  step (L-144) into the **segment reserve**, moving the output sections
  after it in the segment (their units and referrers are refilled,
  IN-27). If the segment reserve is exhausted: `reserve-exhausted`. A
  hole is reused only by its own file's region (to keep the policy a
  function of history that is simple to state); [later] a free list with
  first-fit for any region. *Test:* L10 grows one function step by step
  past its region, its section's tail and its segment's reserve, and
  checks the reasons.
- **IN-33 [v1] How much slack** (answers layout.md Q8: per region, with
  per-section and per-segment reserves):

  | where | capacity | protoc `-g` |
  |---|---|---|
  | each region (allocated) | `size + max(64 B, size/8)`, rounded up to the alignment | ~0.9 MB over 7.07 MB plus ~0.25 MB of minimums |
  | each output section's tail | `max(4 KiB, size/16)` | ~0.3 MB |
  | each `PT_LOAD`: gap to the next, in file and address space | `max(64 KiB, filesz/8)` rounded up to the maximum page size | ~0.6 MB (the address-space gap of `.bss` costs no file bytes) |
  | each debug section's tail (§6) | `max(64 KiB, size/8)` | ~4.7 MB over ~38 MB |
  | merged sections (`.rodata.str*`, `.comment`, `.debug_str`) | `size/8` at the tail (L-147) | in the rows above |
  | `.eh_frame` per-file range (S-111), `.eh_frame_hdr` | `size/8` each | ~0.13 MB |
  | `.symtab`, `.strtab` | none (rewritten whole, S-112) | 0 |

  On protoc the output grows by about 6.5 MB (13% of 50.3 MB) with `-g`,
  and about 1.2 MB (10% of 11.8 MB) without (*estimate*, from the sizes
  in §1.4; S2 measures it). The reserves are written as fill bytes, not
  left sparse (a synced or copied file would expand them anyway). A plain
  link uses capacity = size everywhere: the same code path with a zero
  slack function. *Test:* the bench's incremental rows report the output
  size of an incremental-mode first link against a plain one.

### 5.7 Special structures

- **IN-34 [v1] The table.** Every special structure, its rule in an
  update, and its fall-back:

  | structure | rule | fall-back |
  |---|---|---|
  | `.text` and other code | regions with trap-fill holes (IN-30, IN-3) | `reserve-exhausted` |
  | `.rodata`, `.data`, `.data.rel.ro`, `.bss` | regions; zero holes; `.bss` holes cost no file bytes | `reserve-exhausted` |
  | `.tdata`, `.tbss` (the TLS template) | a changed file's TLS contribution of unchanged size is refilled in place; any change of the template's size or alignment changes every TP-relative offset in every file | `tls-layout-changed` |
  | `.init_array`, `.fini_array`, `.preinit_array`, `.ctors`, `.dtors` (packed, L-142) | rewritten whole in full-link order (priority, then load order) within capacity; `__init_array_end` and the like move, and their users are refilled (IN-25) | `reserve-exhausted` |
  | `.eh_frame` | per-file ranges with padding-CIE holes (S-111) | `reserve-exhausted` |
  | `.eh_frame_hdr` | one unit, rewritten whole when any FDE address changes (S-111) | `reserve-exhausted` |
  | `.gcc_except_table` | regions; zero holes (reached only through LSDA pointers) | `reserve-exhausted` |
  | merged strings and constants | fragments appended (L-147, S-110); dead fragments keep their bytes (an unreferenced constant is harmless and IN-2 allows it) | `reserve-exhausted` |
  | GOT, PLT, IPLT, `.got.plt`, `.rela.plt` | S-113: slots appended within capacity, dead slots written 0; `.rela.plt` compacted | `reserve-exhausted` |
  | `COMMON` symbols | the allocation is a region of the commons chunk; a size change moves it, and its referrers are refilled | `reserve-exhausted` |
  | COMDAT groups | the keeper is recomputed (IN-26); a keeper change is IN-27's row | none |
  | `.note.gnu.property`, `.note.gnu.build-id`, other notes | S-114: recomputed; the build id over the final image, with the stored leaves (image.md I-99) | `property-changed` if the property note changes size |
  | `.symtab`, `.strtab`, `.symtab_shndx` | rewritten whole (S-112) | none |
  | section headers, `.shstrtab`, program headers | rewritten whole; their count does not change | `new-output-section` if an output section or segment appears or disappears |
  | thunks, islands, erratum 843419 patches (arch.md "Incremental links") | re-evaluated for refilled and moved units only; new ones go into their island within capacity | `island-exhausted` |
  | `__start_SEC`/`__stop_SEC`, `_etext`, `_edata`, `_end` | global targets in the index (IN-25) | none |
  | `--icf`, `-r`, `--emit-relocs`, `SECTIONS` scripts, compressed debug, generated debug indexes | not supported in incremental mode (IN-12) | plain link |

  *Test:* one L10 scenario per row: an edit that exercises the rule and
  one that triggers the fall-back.
- **IN-35 [v1] The entry point and `.interp`**: `e_entry` is in the ELF
  header, rewritten every update; `.interp` does not change unless the
  options do (`options-changed`).

### 5.8 Dynamic outputs

M2's dynamic tables (`spec/dyn.md`, `docs/status/m2.md`) and M4's shared
objects.

- **IN-40 [v1] Symbolic tables are rewritten whole.** When the export
  set changes or an exported symbol's value changes, `.dynsym`,
  `.dynstr`, `.gnu.hash`, `.hash`, `.gnu.version`, `.gnu.version_r`,
  `.gnu.version_d`, the symbolic part of `.rela.dyn` and `.rela.plt` are
  regenerated whole by the full-link rules (D-22, D-26, D-27, D-40),
  within capacities of `size/8`. That renumbers `.dynsym` and so every
  `r_sym`, which the rewrite covers; the GOT, PLT and copy slots
  themselves stay where they are (S-113), so no code unit is refilled
  for the renumbering. Programs export few symbols (thousands, not the
  31,478 of `.symtab`), so the rewrite costs little; D-28's fixed
  `nbuckets` becomes an optimisation [later], not a requirement (§9).
  *Test:* L10 adds and removes an exported symbol in a PIE linked
  against a fixture library; the loader runs it (`LD_BIND_NOW=1` too).
- **IN-41 [v1] `RELATIVE` records per section slice.** `.rela.dyn`'s
  `RELATIVE` prefix is a concatenation of per-section slices (dyn.md
  §14); a unit's slice is part of the unit and refilled with it. Slack in
  the prefix is `RELATIVE` records aimed at a reserved scratch word,
  counted in `DT_RELACOUNT` (dyn.md §14); `R_*_NONE` only after the
  prefix. [M4] `.relr.dyn` is one whole-chunk encoding, rewritten whole
  when any `RELATIVE` address changes. *Test:* L10 grows a data array of
  pointers in a PIE; the loader's relocations are checked by running.
- **IN-42 [v1] `.dynamic`** is rewritten whole; a new `DT_NEEDED` uses
  one of the five spare `DT_NULL`s (D-30), and `DT_NEEDED`'s order
  changing, or the spares running out, is `dynamic-changed`.
- **IN-43 [v1] Shared objects are never written in place** (M4): a
  running process maps a shared library `MAP_PRIVATE` without write
  denial, so its unmodified pages follow the file and an in-place write
  would change code under it. A shared object is always updated by copy
  and rename (IN-47). *Test:* a running program that `dlopen`s the
  library keeps working across an update.

### 5.9 Writing

- **IN-44 [v1] Positioned writes, no mapping.** The update writes dirty
  windows (image.md I-95) with `FileExt::write_all_at`; no `mmap`
  (every crate forbids `unsafe`). Measured, a 1 MiB positioned write
  costs 1.4–2.0 ms and 16 MiB 20–34 ms (§1.5). *Test:* covered by L10.
- **IN-45 [v1] In place or copy.** The output is updated in place only
  if all of these hold, otherwise by copy (IN-47):
  - `P` is a regular file, not a symbolic link (a full link replaces the
    link, image.md I-60; writing through it would change its target);
  - `P` has one link (`st_nlink == 1`): a full link leaves other hard
    links with the old bytes (image.md I-60), and an in-place write would
    change them;
  - `P` is not one of the inputs (image.md I-62);
  - `P` is an executable, not a shared object (IN-43);
  - opening `P` for writing succeeds (a running executable gives
    `ETXTBSY`, measured, §1.5);
  - an exclusive lock on the state file is acquired and the file system
    enforces it (the probe of image.md I-59); otherwise two concurrent
    updates of one output could interleave their writes.
  *Test:* one link test per condition, checking the inode afterwards
  (same for in place, new for copy).
- **IN-46 [v1] The order of writes.** In place:
  1. set the state's "updating" flag (a positioned write of its header)
     and `fdatasync` the state (0.1–0.9 ms measured);
  2. write the output's ELF header with `e_ident` zeroed, so that a torn
     output cannot be executed ("exec format error") instead of running
     a mix;
  3. write the dirty windows; truncate or extend (image.md I-97);
  4. write the true ELF header;
  5. `fdatasync` the output (1.3–9.2 ms measured for 1–8 MiB written);
  6. write the new state to a temporary file, `fdatasync` it, rename it
     over the old state (which clears the flag).
  A crash anywhere from 1 to 6 leaves the flag set or the old state with
  an output whose stat no longer matches; the next link says
  `output-torn` or `output-changed` and links from scratch. The two
  `fdatasync`s make that hold across a power loss too (without step 5,
  the renamed state could reach the disk before the output's data); a
  full link does not sync (image.md I-61) and does not need to, because
  it renames a complete file. *Test:* a crash-injection hook (a test-only
  environment variable that aborts after step k) at each step; the next
  link must report the reason and give an output equivalent to a full
  link's.
- **IN-47 [v1] The copy path.** Copy `P` to `T` (image.md I-59's name
  and lock) with `std::io::copy` (`copy_file_range`; 54–142 ms for 100
  MiB, 1.8–2.9 s for 1 GiB here; reflink where supported, not on this
  host's overlay file system), apply steps 2–4 and 6 to `T`, rename `T`
  over `P` (image.md I-60, I-113). The cost grows with the output; it is
  still below a full link's write for outputs under ~1 GiB.
- **IN-48 [v1] The state is written only by a successful link**
  (image.md I-100). A failed update removes the output (image.md I-66)
  and the state.
- **IN-49 [v1] The map** (`--mortar-map`, plumb.md §2.3) is rewritten
  whole (image.md I-99), with `hole` and `dead` lines (§8.2).
