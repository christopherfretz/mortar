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
   needed, to keep a changed file's region and to let unchanged sections
   of a changed file stay in place (IN-30).
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

### 1.5 The time of a full link and the floor of an incremental one (E2, E4, E5)

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
of later optimisation (IN-23's per-member hint; S-112's per-file
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
