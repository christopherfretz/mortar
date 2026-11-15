# spec: incremental linking (`mortar_incr`)

Status: **draft, revision 1 (WP-060), after an adversarial review**. Design only: no shipped code
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

- **The unit of change detection is the input file, in v1.** An edit
  changes the bytes, symbol order and section indices of the objects it
  touches, and nothing else (§1.1, a 40-unit corpus under GCC, Clang and
  kiln+hod). A changed file is re-read and refilled whole; unchanged
  files are never re-read, except to refill a unit of theirs that names
  something that moved. Whole-file refill is coarse with
  `-ffunction-sections` and no `-g` (up to 4,483× the differing bytes,
  §1.2), so **section-level refill** with mortar's own per-section hash is
  the designed second step (IN-31, WP-068), ordered by §1.5's rule.
- **No indirection.** The output of an update is a valid output that a
  full link *given the same layout* would write byte for byte outside
  its recorded dead ranges (`spec/model.md` §7.3; IN-1); no thunk table,
  no padding inside functions. Moving a definition costs refilling the
  units that name it: 3 units at the median and 533 at the 99th
  percentile of protoc's files (§1.4).
- **Slack only where holes are harmless**: per (file, output section)
  region in layout.md's hole-tolerant (L-143) sections, a tail per
  section and a reserve per segment. Packed (L-142) sections have no
  holes and keep full-link order, rewritten whole within their capacity
  (IN-30); `.eh_frame`'s only holes are padding CIEs (IN-34a). Any change
  of a TLS section's TP offset is a full link (`tls-layout-changed`).
  Cost: +10.0% output on protoc `-g`, +20.1% without (IN-33).
- **Dead ranges** (old debug units, holes, dead merged fragments,
  padding CIEs) are recorded in the decision tables and checked by the
  verifier: nothing live refers into them, each is well formed for its
  section, allocated ones hold the fill (IN-3a).
- **The link state is a separate file beside the output** (8.05 MB for
  protoc `-g`, written in 7 ms, read in 3 ms; R12), bound to the output by
  its stat tuple and build-id leaves (IN-21).
- **Changed inputs are found by stat, stamps and content**: a file whose
  stat tuple equals the state's and whose ctime is older than a stamp
  taken on the same file system at the previous link is clean; a racy
  file is hashed, never trusted, never a full link (IN-22, IN-22a; R6:
  1,942 of 2,000 same-tick rewrites were missed by the previous rule on
  overlay, 0 by this one). A regular archive is read and its members
  hashed; a thin archive is a `stat` per member (IN-24).
- **The dependency index is per unit** (`model.md` §7.2), 118,145 rows on
  protoc against 1.78 M relocations, and carries the liveness graph.
- **Debug contributions are allotted size classes** and replaced in
  place when they fit (the common case: their sizes barely move, §1.1);
  old units are **neutralized**, not merely tombstoned, because lldb
  picks a dead unit's stale type otherwise (§1.6, §6).
- **Writing is by copy and rename by default** (IN-45, IN-47: 18–27 ms at
  50 MB); in place on request, with four `fdatasync`s ordering a zeroed
  magic before the windows (IN-46), because `ETXTBSY` does not see a
  program started through `ld.so` (R7). The default is the operator's
  decision (record (a)).
- **A plain link pays nothing** (§8.1): no slack, no state, no hashes, no
  index, no stamps.
- **The floor** of an update on protoc, emulated with real I/O (§1.5,
  R1): 55–63 ms with a thin archive in place, 90–112 ms by copy,
  107–178 ms with a regular archive, against a 0.19 s full link
  (1.1–3.5×). Dirty 1 MiB windows, not the edit, set the refill. WP-061
  measures it with mortar's code on protoc and a 5× larger workload, and
  its **go/no-go rule** decides whether M7 continues (§1.5).
- **Verification**: the two-full-link oracle runs on every incremental
  test (IN-63a), the equivalence check enforces values and packed-section
  order (IN-2, IN-62), and every delta rule has a mutation test
  (IN-63b).

## 1. The workload: what an edit changes

### 1.1 The corpus (R2)

The previous draft's census (E1, `census.py`: two units, no `assert` or
`__LINE__`) is superseded: three of its claims are artefacts of the toy
(the reviewer's M11), and its "`.text` size change" column printed the
changed span, not a size delta (M9). This section is measured on the
reviewer's 40-unit corpus (`corpus/`: `gen.py`, `lib.py`, `drive.py`,
`summarize.py`, `link.py`, `unity.py`, `pch.py`) and its extension
(`corpus2/`: `drive2.py`, `drift.py`, `hdredit.py`, `link2.py`).

**Corpus.** 38 small units and `big.cc` (4,957 lines, 540 functions) and
`main.cc`, all including `common.h`: a `Widget` class, a container
template, key-function and inline-only vtables, exceptions thrown across
units, a `thread_local`, static constructors, strings, and `assert`,
`__LINE__` and `__builtin_LINE()` in some functions. Twenty
configurations: GCC 14.2 and Clang 19.1.7 × `-O0`/`-O2` × `-g`/none ×
`-ffunction-sections -fdata-sections` ("fs")/none, and kiln 1.2.1 + hod
(`KILN_BACKEND=hod KILN_HOD_STRICT=1`) at `-O0` × `-g`/none × fs/none.
Base and edit compile in the same directory. 3,440 compiles (the
reviewer's helper, load ~25) and 1,369 compiles and 54 linked programs
(corpus2, load 1–2), none failing; every mortar-linked program prints the
same checksum as the system linker's. Rebuilding unchanged sources gives
byte-identical objects in all 20 configurations.

Table 1 (objects that changed among those recompiled; "kiln `-g`" where
kiln with `-g` differs from the other compilers):

| edit | what | objects changed | what changed in them |
|---|---|---|---|
| e1 | a constant in `big.cc`, same size | 1 | with fs one section of 104–120 B of a 176–234 KB `.text`; no debug byte at GCC/Clang `-O0`; at `-O2` the caller that inlines it too |
| e2 | two statements added to a function in the middle of `big.cc` | 1 | without fs the bytes differ from offset 776 (GCC `-O0`), 1,128 (Clang), 1,364 (kiln) of a 129 KB `.text`: GCC's assembler resolves a `bl` to an internal-linkage function in the same section without a relocation, so code growing in between changes an earlier, unedited function (220 of 820 functions differ); with fs 47 of ~1,090 allocated sections differ (`.eh_frame` and functions whose `__LINE__` moved) |
| e3 | a comment line at the top of `big.cc` | 1 | 90 of ~800 functions (those with `assert`/`__LINE__`) in every compiler; GCC's `.text` shrinks 4 bytes |
| e4 | a comment line in `common.h` | 40 of 40 | allocated bytes in 39 of 40 (an inline function's `__LINE__`, `Bag::at`'s `assert`) |
| e5 | a default argument in `common.h` | 13 of 40; kiln `-g`: 40 | one function per caller; kiln `-g`: 27 more objects differ **only** in `common.h`'s DWARF 5 MD5 in `.debug_line` (R2-B) |
| e6 | a member added to `Widget` | 34 of 40; kiln `-g`: 40 | Widget code, `.eh_frame`, `.symtab`, debug; kiln `-g`: 6 more MD5-only |
| e7 | a function renamed in 4 units | 4 | |
| e8 | a line above `checked_07` in `t07.cc` | 1, same size | `checked_07` and `lineb_07` (assert, `__LINE__`), not `plain_07` |
| e9 | a function added to `big.cc` | 1 | as e2, plus the symbol table |
| e10 | a string literal made longer | 1 | GCC `-O2` fs: one per-function `.rodata.str` section and addends in 138–139 sections |
| add a unit | `tx.cc` (a function, a global with a dynamic initializer, a literal) added to the link | 1 new file | 548–652 B allocated, 46–60 KB of debug (`-g`) |
| remove a unit | the reverse | 1 file gone | |
| archive member | e8, and a same-size constant edit, with `t00`–`t37` in `libt.a` | 1 member | regular `ar rcsD`: the archive keeps its size and every member offset; the member's header is identical before and after (`date=0 uid=0 gid=0 mode=644`, same size): **only the member's bytes differ**. Thin `ar rcsDT`: the archive file is byte-identical; the member file's mtime changes |
| extraction | `t07.cc` now calls `tx_fn`, defined by `tx.o` in `libt.a` | 1 member + 1 newly extracted | the archive grows by 96–208 B, 31 of 39 member headers move; live files 488 → 489 |

Findings:

- **No edit changed an object whose source and headers were unchanged**,
  at `-O0` and `-O2` (the one claim of the old census that holds).
- **A comment line changes allocated bytes** wherever a function uses
  `assert`, `__LINE__` or `__builtin_LINE()` below it (e3, e8), in every
  compiler, kiln included.
- **An edit outside a function can change the function's bytes**: by
  `__LINE__` (e3, e8), by inlining at `-O2` (e1), and without fs by
  assembler-resolved branches (e2).
- **A header edit changes the includers that use what changed** (13 of 40
  for e5, 34 of 40 for e6), not all of them; except **kiln `-g`, where
  every includer changes**, measured per object (R2-B,
  `hdredit.py`): the 27 (e5) and 6 (e6) extra objects differ only in
  `.debug_line`, with no relocation differences, and their
  `llvm-dwarfdump --debug-line` dumps differ in exactly one line: the
  `md5_checksum` of `common.h`. This is **not a kiln-versus-Clang
  difference in kind** (kiln's agent measured Clang 19 and 22 behaving
  as kiln does). The mechanism is the DWARF 5 MD5 and which units carry
  it. A re-measurement (R2-B', two units including a local `h.h`, one
  also `<vector>`, an edit that keeps every line number, `-std=c++17 -O0
  -g`) shows it:

  | compiler | unit with local headers only | unit also including `<vector>` |
  |---|---|---|
  | Clang 19 (DWARF 5) | **differs** (2 MD5 entries) | same (0 MD5 entries) |
  | kiln + hod | **differs** (2) | **differs** (20) |
  | GCC 14 | same (0) | same (0) |
  | Clang 19 `-gdwarf-4` | same | same |

  Clang drops every MD5 from a unit when one of its files (here a
  libstdc++ header) has none. Every corpus unit includes libstdc++, so
  Clang's corpus objects carry no MD5 and do not change. kiln writes an
  MD5 for every file (56 of 56 in the corpus), so all its includers
  change. With only local headers, Clang behaves like kiln. The
  consequence for mortar is a `.debug_line`-only change, not an
  allocated-byte change. v1 still sees a changed file and refills it
  whole, so a header edit makes every includer a changed input under
  kiln (40 instead of 13 or 34 here). IN-31's section hashes (v2)
  would refill only that file's `.debug_line` contribution. §8.3's kiln
  request follows.
- **Debug sizes barely move on these edits**: over 745 changed `-g`
  objects, the new debug size over the old is 1.0000 at the median,
  1.0005 / 1.0021 / 1.0002 at p90 (GCC / Clang / kiln), 1.0149 at most
  (`drift.py`). Contents change almost whole (DIE offsets shift), sizes
  do not; larger edits (functions or types added) grow them more (§6).
- **Archives**: deterministic `ar` leaves no trace of a same-size member
  edit in the archive's headers (IN-24); a thin archive turns it into a
  `stat` of the member file.
- **Unity builds and PCH** (the reviewer's helper, `unity.py`, `pch.py`):
  a unity object (`t00`–`t09` in one unit, a constant changed in `t05`)
  is 3.7× (GCC `-O0`) to 7.7× (GCC `-O2`) larger in allocated bytes than
  `t05.o` alone, so a file-level refill costs that much more; with fs the
  differing sections are small either way (12 KB vs 4.6 KB). PCH does not
  widen the change set (40, 13, 34 objects for e4, e5, e6), but no PCH
  object is byte-identical to its non-PCH build, so switching PCH on or
  off refills everything.
- **hod's DWARF needs almost no relocations** (the old census, still
  true): 5 relocation records in `.debug_info` for a kiln unit against
  2,220 for GCC's (hod uses `.debug_str_offsets` and `.debug_addr`;
  `hod/spec/dwarf.md`).

### 1.2 What the corpus decides

1. **Granularity of change detection: the file, in v1.** Every edit
   changes whole objects (symbol order, section indices, `.eh_frame`,
   debug offsets); nothing outside the edited objects changes. v1
   re-reads a changed file whole and refills every unit of it.
2. **That is coarse exactly where the bench's defaults are.** The
   coarseness factor (bytes a whole-file refill rewrites ÷ bytes of only
   the sections whose bytes or relocations differ; allocated and debug,
   allocated only in brackets; without `-g` the bracket is the figure;
   the reviewer's helper, `summarize.py`):

   | config | e1 | e2 | e3 | e4 | e5 | e6 | e7 | e8 | e9 | e10 |
   |---|---|---|---|---|---|---|---|---|---|---|
   | gcc-O0-g-fs | 4483 (1692) | 1.8 (5.8) | 1.8 (5.1) | 2.3 (159) | 285 (35) | 1.1 (2.6) | 1.2 (11) | 2.4 (316) | 1.5 (5.8) | 2.7 (4.6) |
   | gcc-O0-g-nofs | 3.3 (1.4) | 1.2 (1.1) | 1.2 (1.1) | 2.3 (159) | 63 (7.9) | 1.1 (1.8) | 1.2 (10) | 2.4 (8.2) | 1.1 (1.1) | 1.6 (1.3) |
   | gcc-O2-g-fs | 105 (9.4) | 1.1 (3.5) | 1.1 (3.3) | 1.6 (4.0) | 2.3 (2.8) | 1.0 (1.9) | 1.2 (3.2) | 1.5 (2.6) | 1.1 (3.5) | 43626 (3055) |
   | gcc-O2-g-nofs | 18.6 (1.4) | 1.0 (1.1) | 1.0 (1.1) | 1.6 (2.6) | 2.3 (1.9) | 1.0 (1.3) | 1.3 (2.5) | 1.5 (1.8) | 1.0 (1.1) | 1.2 (1.3) |
   | clang-O0-g-fs | 4157 (1876) | 1.8 (5.7) | 2.0 (16.7) | 2.9 (138) | 81 (25) | 1.1 (2.6) | 6.6 (9.9) | 1.3 (217) | 1.6 (5.7) | 3.1 (4.9) |
   | clang-O0-g-nofs | 2.8 (1.3) | 1.1 (1.1) | 1.2 (1.3) | 3.0 (138) | 35 (7.2) | 1.1 (1.7) | 6.7 (9.4) | 1.3 (7.7) | 1.1 (1.1) | 1.6 (1.3) |
   | clang-O2-g-fs | 1.6 (18.9) | 1.1 (5.3) | 1.1 (5.0) | 2.2 (4.0) | 2.8 (3.7) | 1.1 (2.4) | 2.2 (4.3) | 1.1 (2.4) | 1.1 (5.3) | 1.1 (1.4) |
   | clang-O2-g-nofs | 1.4 (1.3) | 1.0 (1.1) | 1.0 (1.1) | 2.1 (1.8) | 2.8 (2.5) | 1.0 (1.3) | 2.2 (3.4) | 1.1 (1.9) | 1.0 (1.1) | 1.0 (1.0) |
   | kiln-O0-g-fs | 6.8 (1946) | 2.4 (5.4) | 2.9 (15.8) | 3.1 (138) | 12.1 (112) | 1.3 (2.9) | 4.8 (10.3) | 3.1 (216) | 1.7 (5.4) | 4.0 (5.0) |
   | kiln-O0-g-nofs | 1.9 (1.3) | 1.2 (1.1) | 1.4 (1.3) | 3.1 (138) | 11.2 (28.6) | 1.3 (2.0) | 5.0 (11.0) | 2.9 (8.9) | 1.1 (1.1) | 1.8 (1.3) |

   With `-g` the factor is 1.0–3.3 for most edits, because the debug
   contribution changes almost whole and dominates; without fs, `.text`
   is one section per object and the factor is 1.1–1.4 for edits in big
   units. With fs and no `-g` (the bench's GCC/Clang defaults) it is
   2.6–316 for most edits and 1,692–4,483 for a same-size constant. So
   the claim "section-level reuse would save nothing in the common case"
   is wrong as a general statement, and **section-level refill is the
   designed second step** (IN-31, its own package, §10), entered by
   §1.5's go/no-go rule. Its benefit is bounded by the window effect of
   §1.5 (a dirty 1 MiB window is refilled whole), which is why it is
   second.
3. **Size changes are the common case** for allocated sections (e2, e6,
   e9, e10 resize; e1, e3, e8 may not), and **debug contributions change
   in content almost whole but in size hardly at all** (§1.1): replacing
   a debug contribution in place within a rounded size class (IN-50)
   covers nearly every edit measured.
4. **Cross-file effects come only through symbols, COMDAT groups, the
   TLS layout, liveness and synthetic tables**, and through archive
   extraction: IN-27's rows.
5. **Nothing a compiler emits is trusted to stay put**: `__LINE__`,
   inlining and assembler-resolved branches change functions the edit
   did not touch, so any finer reuse (IN-31) compares bytes and
   relocations by mortar's own hash.

### 1.3 Archives in the development loop

The protoc workload (`spec/harness.md` "bench", `cxx-medium-g`; static
twin `cxx-medium-g-static`) links `main.cc.o`, `libprotoc.a` (116.8 MB,
84 members) and `libprotobuf.a` (84.7 MB, 83 members) with
`libstdc++.a`, `libc.a` (5.6 MB, 2022 members), `libm.a`, `libgcc.a`,
`libgcc_eh.a`: 217 MB of inputs of which 164 MB are in the 784 live
files (E3). An edit of one protobuf source rebuilds one object and then
the **archive** (CMake's default), so the common protoc edit reaches the
linker as "a regular archive changed". GNU `ar` rewrites the whole file;
with Debian's deterministic default (`D`) every member header carries
date 0, uid 0, gid 0 and mode 644, so **a same-size member edit changes
nothing in the archive but the member's bytes** (R2-C: the archive keeps
its size and every member offset; the header is identical before and
after). Finding the changed member therefore means reading and hashing
every member: 53–94 ms for `libprotobuf.a` single-threaded, 50–70 ms in
the floor emulation (§1.5, IN-24). A thin archive (`ar T`) names members
as files: the archive file stays byte-identical and the member's own
`stat` (4.3 µs, E2) finds it. §8.3 asks kiln's driver to use thin
archives in its development profile.

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

### 1.5 The time of a full link and the floor of an incremental one (R1)

**Baseline.** Full links of the bench's static protoc by mortar at main
`2a31ada` (merged into this branch at `ae4544f`; the branch adds only
spec text), release build, output on the container's overlay `/tmp`,
eight runs each, load 5–6 (`baseline.sh`, `--time-phases`, `/usr/bin/time
-v`):

| link | first run | warm runs (7) | peak RSS |
|---|---|---|---|
| protoc `-O0 -g` static (50.3 MB out, 164 MB live input) | 0.43 s | 0.18–0.22 s | 252–258 MB |
| protoc `-O0` static (11.8 MB out, 44 MB live input) | 0.20 s | 0.12–0.14 s | 144–151 MB |

Phases of a typical `-g` run (191.2 ms): input 87.1, resolve 7.9,
assignment 3.8, split 12.9, liveness 1.1, scan 4.5, synthetic 3.2,
layout 21.6, image 44.6 ms. Without `-g` (120.7 ms): input 57.7, resolve
8.4, assignment 4.1, split 3.5, scan 4.4, synthetic 3.3, layout 21.8,
image 13.5 ms. The previous draft's baseline (565–781 ms and 316–471 ms,
load 21–23) was mortar before the M2 train's input and image work, at
four times the load; the reviewer measured 274–295 ms and 210–355 ms at
load ~20. Every speedup below is against 0.19 s and 0.12 s.

**The floor, as a timed emulation** (`floor.py`, R1): every I/O the
design does, performed for real on the real output and inputs, and the
CPU parts that need mortar code taken from the full link's
`--time-phases` in proportion to the bytes they touch ("derived"). It is
not a prototype: no relocation is applied. For each changed file (the
largest, `descriptor.cc.o`, and the median live member), the refill is
its own units plus the units of other files whose relocations name a
global it defines (IN-27); the **dirty windows** are the 1 MiB windows
(image.md I-75, IMAGE_BLOCK) those units overlap, plus the ELF header's,
`.symtab`/`.strtab`'s (rewritten whole, S-112) and `.eh_frame_hdr`'s; a
dirty window is filled whole (I-95), so the refill reads every unit that
overlaps it, coalesced into one positioned read per run of units with
gaps under 64 KiB, on 8 threads. Timed parts: `stat` of every input,
stamps (IN-22a), reading the 9 MB state with a CRC, change detection
(thin archive: read and hash the object; regular archive: read the
archive and hash every member, IN-24), the refill reads, and the writes
of IN-46 with their four `fdatasync`s (in place) or IN-47's copy, sync and
rename (copy), plus the state's write. Derived: the resolve phase whole
(IN-26 re-resolves everything), the input phase × changed bytes / input
bytes, the image phase × refilled bytes / output bytes. Medians of 5,
load 9–11, CRC-32 standing in for L128 (slower, so the hash parts are
upper bounds):

| protoc `-g` (full link 191 ms) | thin, in place | thin, copy | regular, in place | regular, copy |
|---|---|---|---|---|
| `descriptor.cc.o` (5,309 own units, 456 referrer units, 18 dirty windows, 13.1 MB refilled), overlay | 63 ms (3.0×) | 112 ms (1.7×) | 132 ms (1.45×) | 178 ms (1.07×) |
| the same, virtiofs | 88 ms (2.2×) | 105 ms (1.8×) | 169 ms (1.13×) | 144 ms (1.33×) |
| median member `importer.cc.o` (285 own, 9 referrer units, 15 windows, 10.0 MB), overlay | 55 ms (3.5×) | 90 ms (2.1×) | 107 ms (1.8×) | 150 ms (1.27×) |
| the same, virtiofs | 73 ms (2.6×) | 78 ms (2.5×) | 124 ms (1.5×) | 130 ms (1.5×) |

| protoc without `-g` (full link 122 ms) | thin, in place | thin, copy | regular, in place | regular, copy |
|---|---|---|---|---|
| `descriptor.cc.o` (12 windows, 6.3 MB), overlay / virtiofs | 47 / 53 ms (2.6× / 2.3×) | 54 / 68 ms | 63 / 72 ms | 76 / 76 ms (1.6×) |
| median `primitive_field.cc.o` (10 windows, 4.2 MB), overlay / virtiofs | 38 / 48 ms (3.2× / 2.5×) | 49 / 54 ms | 56 / 66 ms | 66 / 67 ms (1.8×) |

Where the time goes (`descriptor.cc.o`, `-g`, thin, in place, overlay):
stat 0.5, stamps 0.0, state 3.4, detect 3.0, refill reads 12.9, writes
19.1, derived 24.2 ms. Three findings:

1. **A regular archive costs 50–70 ms of detection** (`libprotobuf.a`,
   85 MB, read and hashed; 15 ms for the 25 MB no-`-g` archive), more
   than the rest of the update. Thin archives remove it (§8.3).
2. **The copy costs 30–40 ms more than in place** at 50 MB (IN-47), and
   grows with the output, not with the edit.
3. **The window, not the edit, sets the refill.** The median member's
   own units and referrers are 287 KB, yet 15 windows (10 MB) are
   refilled, because a 1 MiB window is filled whole and `.symtab` and
   `.strtab` (5.4 MB, six windows) are rewritten on every update. Neither
   section-level change detection (IN-31) nor a smaller fan-out shrinks
   this; smaller windows for the update (image.md I-75 is a build-id leaf
   size, not a write size) or per-file `.symtab` ranges (synth.md Q8)
   would.

So on protoc an update is **1.1–3.5× faster** than today's full link,
not the 3–10× the previous draft estimated against a stale baseline:
the full link got 3× faster in the meantime, and protoc is small (a
0.19 s link). The case for incremental linking rests on large programs,
where the full link grows with all inputs (input and image are 69% of
it) and the update with the edit, the windows and (on the copy path)
the output. A 10× protoc (1.6 GB of inputs, a 500 MB output) is not in
the bench; for it the derivation gives a full link of ~2 s and an
update dominated by the copy (0.2–0.75 s at R7's and the reviewer's
rates) unless it is in place or reflinked (*estimate*, linear scaling,
not measured).

**Go/no-go rule.** WP-061's first deliverable is the floor **measured
with mortar's own code** (state read and validated, stat and stamps,
change detection, re-resolution, refill through windows, IN-46/IN-47
writes; relocation of refilled units through the image phase), on
protoc `-g` and on a workload at least 5× larger in live input bytes
(the bench has none: WP-061 adds one, e.g. a generated 200-unit program
scaled up, or LLVM's `opt` if it builds here), thin archives, both write
paths. Then:
- **go** (S2–S6 as cut, §10) if on the large workload the median-file
  update on the default write path is at most **1/3** of a plain full
  link, and on protoc at most **1/2**;
- **change order** if it misses only because of a named part: regular
  archive detection → parallel member hashing and the kiln request
  first; the copy → the operator's M4 decision is revisited with the
  number; window refill → smaller update windows in S4; changed-file
  refill without `-g` with `-ffunction-sections` above **25%** of the
  update → IN-31 (section-level refill) moves before S5;
- **no-go** if no named change brings it under 1/2 on the large
  workload: M7 stops after S1 (the state is kept for tuck), and the
  operator is told that a fast full link is the better investment, with
  the numbers.

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
zero byte is an empty table) fixed it. The revision's R9 repeated this
at depth (20 dead versions of one file): the pad form above makes readelf
print "Bogus end-of-siblings marker" warnings on every output, and a
**block form** (one DIE whose `DW_FORM_block` attribute covers the unit)
is clean in readelf, `--verify`, gdb and lldb. §6 turns this into rules
(IN-51, IN-51b).

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
`docs/ARCHITECTURE.md`) is kept, and corrected in one sentence: its
"index from each symbol to the fields that refer to it" becomes the
per-unit index that `model.md` §7.2 already defines ("the units whose
bytes depend on it"; IN-25 implements it and names its non-relocation
targets), at 15 times fewer rows than a field-level one (§1.4). Two other
positions the previous draft presented as overturned were already the
model's: ids do not survive a link, identities do (model §7.1); and the
index is per unit (§7.2). Per-section content hashes are not needed in
v1 (the file is the unit of change detection) and are mortar's own work
in v2 (IN-31), never a compiler's. Debug contributions are neutralized,
not only tombstoned (§1.6).

## 3. Correctness

An incremental link is a link. Everything mortar promises of a link
(`docs/ARCHITECTURE.md`: correct or refused by name, checked by the
verifier, deterministic) holds of it, and it adds one promise: the
program is the program a from-scratch link would have made.

- **IN-1 [v1] The three invariants.** For every incremental update:
  - **(A) verified**: `mortar_plumb` accepts the output with its map,
    exactly as for a full link (§8.2);
  - **(B) replayable modulo dead ranges**: a full link *given the
    update's decision tables* (model §7.3) writes the same bytes as the
    update everywhere outside the **dead ranges** those tables record
    (IN-3a), compared byte for byte (image.md I-98). A dead range holds
    bytes of an input version that is no longer an input, so no replay
    from the current inputs can produce them; their bytes are checked
    independently instead (IN-3a's three conditions, by the verifier);
  - **(C) equivalent**: the output is equivalent (IN-2) to a from-scratch
    full link of the same inputs and options.

  (A) and (B) are the model's two invariants; (C) is new, and is what the
  user relies on. (B) alone would accept an update that kept a stale
  definition, because the stale tables would replay faithfully.
  *Test:* L10 (§8.2) checks all three after every step, and runs the
  refill oracle (IN-63a) on every step.
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
  4. every relocated field holds, in `X`, the **value** it holds in `F`
     after both are mapped through the placement (a field naming input
     section `s` at offset `o` names `s` at `o` in both), checked field by
     field by its value, not by its encoding; slots are those of the same
     demand flags. Only these relaxations may differ between `X` and `F`,
     because their outcome depends on a distance that placement changes,
     and each is checked by the value the relaxed sequence computes:
     AArch64 `ADRP`+`ADD` ↔ `ADR`+`NOP` and `ADRP`+`LDR` (GOT) ↔
     `ADR`/`ADRP`+`ADD` where arch.md relaxes them, the choice of a range
     thunk or a direct branch (arch.md "Incremental links"), erratum
     843419 patches, and x86-64 `R_X86_64_GOTPCRELX` relaxations whose
     choice depends on whether the target fits in 32 bits. Relaxations
     whose outcome depends only on symbol properties (TLS model
     relaxation, `GOTPCRELX` for a non-preemptible symbol in a non-PIC
     link) must be the same;
  5. the synthetic tables (GOT, PLT, `.eh_frame_hdr`, notes, dynamic
     tables) have the same entries, and **every packed section of
     layout.md L-142** (`.init_array`, `.fini_array`, `.preinit_array`,
     `.ctors`, `.dtors`, `.init`, `.fini`, every section reached through
     `__start_`/`__stop_` symbols, `.tm_clone_table`, notes, and
     `.eh_frame`'s records with padding CIEs (dead ranges) passed over)
     holds the same contributions **in the same order** as in `F`, as do
     `.rela.plt`'s `IRELATIVE`s and `DT_NEEDED`; all up to the placement
     mapping. (The order of other code and data, and so the relative
     order of function addresses, may differ; a program that sorts or
     hashes function pointers can observe it, as it can observe any
     relink. `docs/` says so.);
  6. every byte of `X` not covered by 1–5 is in a **dead range**
     recorded in the decision tables (IN-3a) and passes IN-3a's three
     checks;
  7. the diagnostics are the same (IN-5).

  Equal bytes are not required: placement differs (slack, holes, a
  region moved to a tail), and so do the build id and every address.
  *Test:* `mortar_plumb --equiv X.map F.map` (an extension of the map
  checks, §8.2) checks 1–3 and 5 from the two maps and 4 by re-deriving
  each field's value through both placements; a program-level check runs
  both (L10). R13 (`equiv.py`, §12; 6,654 units, 539 KB compared, with
  relocated fields and `.eh_frame` CIE pointers masked): a re-link with
  six units at other addresses and the same order is accepted; R4's "b a"
  output is rejected by the order clause (`myset` holds b, a) and
  **accepted without it**, though it prints a different result; a
  control flipping one non-relocated code byte is rejected.
- **IN-3 [v1] Holes.** Every byte of an allocated output section that no
  live unit owns is a **hole** and holds its section's fill, written when
  the hole is made:
  - code (`SHF_EXECINSTR`): the target's trap fill, as layout L-143
    already requires of gaps (layout.md L-82: `UDF #0` words on AArch64,
    `int3` on x86), so a stale pointer into removed code traps instead of
    running stale instructions;
  - other allocated `PROGBITS`: zero; `NOBITS`: nothing in the file, zero
    in memory;
  - `.eh_frame`: IN-34a's padding CIE;
  - `.rela.dyn`'s `RELATIVE` prefix: a `RELATIVE` record aimed at the
    scratch word (dyn.md §14); after the prefix, `R_*_NONE`.
  Holes exist only in hole-tolerant sections (layout.md L-143, IN-30);
  a packed section (L-142) never has one (IN-34), except `.eh_frame`,
  whose holes are padding CIEs: records its walkers parse (IN-34a). A hole is an entry of
  the decision tables (its section, offset, size), so (B) reproduces its
  fill and the map lists it (`hole` lines, §8.2). Holes are the
  allocated case of a dead range (IN-3a). *Test:* remove a function and
  check that its old range is all trap fill (`plumb` checks every `hole`
  line's bytes); R4 (`packed_holes.sh`) is the negative case: a hole in
  `.init` traps (SIGILL) and a zero slot in a `__start_` array crashes
  (SIGSEGV).
- **IN-3a [v1] Dead ranges.** A **dead range** is a byte range of the
  output that a full link of the current inputs would not write, left by
  an update: an allocated hole (IN-3), a dead debug contribution (§6),
  a dead fragment of a merged section (L-147, S-110), an unused CIE in
  `.eh_frame`. The decision tables record every dead range as
  (output section, offset, length, kind), and the map lists them
  (`dead` lines, §8.2). Invariant (B) holds outside them (IN-1). Inside
  them the verifier checks, independently of the linker and of the
  replay:
  1. **nothing live refers into the range**: no relocated field's target,
     no DWARF form offset (`DW_FORM_ref_addr`, `DW_FORM_sec_offset`,
     `DW_FORM_strp`, `DW_FORM_line_strp`, `DW_AT_str_offsets_base`,
     `DW_AT_addr_base`, `DW_AT_rnglists_base`, `DW_AT_loclists_base`,
     `DW_AT_stmt_list`, an abbreviation offset) of a live unit, no
     `.eh_frame_hdr` entry, no symbol's value;
  2. **the range is well formed for its section's grammar**, so the
     sequential readers (`readelf`, `llvm-dwarfdump`, gdb, lldb, libgcc's
     and libunwind's `.eh_frame` walkers) pass over it: a neutral unit of
     IN-51's form in `.debug_info`, `.debug_abbrev`, `.debug_line`,
     `.debug_rnglists`, `.debug_loclists`, `.debug_str_offsets`,
     `.debug_addr` (IN-51, IN-51b; `.debug_aranges` has none, IN-54), a padding CIE in
     `.eh_frame` (IN-34a), a whole dead contribution as its compiler wrote
     it elsewhere in a debug section (IN-53), any bytes in a merged
     string section (reached only by offset);
  3. **in allocated sections a dead range of code or data holds IN-3's
     fill**, except dead fragments of merged string sections, which keep
     their bytes because a live reference may share a suffix (L-147).
  These three are the one statement IN-2 item 6, IN-3, IN-34, IN-53,
  layout.md L-147 and synth.md S-110/S-111 refer to. *Test:* plumb's
  dead-range checker over every L10 step; its mutation tests put a live
  reference into a dead range, a malformed byte into a dead debug range,
  and a stale (non-fill) byte into a code hole, each of which it must
  reject (§8.2).
- **IN-3b [later] Scrubbing non-allocated dead ranges.** Rewrite every
  dead debug range to its section's neutral form (IN-51b) when it is made,
  so that no byte of an old input version survives in the output at all,
  and (B) holds byte for byte without the dead-range exception for
  debug sections. Cost: the writes of the whole dead contribution
  (p50 210 KB, p99 823 KB, max 1.50 MB on protoc, §1.4) on each update of
  that file, against today's rewrite of the `.debug_info` unit and
  abbreviation table only; the range's file bytes are in a dirty window
  either way when the contribution is small. Worth it if a user asks for
  "no stale bytes" (a privacy concern: a removed comment's string is
  still in `.debug_str`'s dead fragments either way, which no scrub can
  remove without moving live strings). *Test (when built):* plumb checks
  that every dead debug range is in neutral form over its whole length,
  not only its first unit.
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
  be used. *Test:* one link test per option: the warning, no state file
  written, and the output byte-identical to a plain link's; with
  `--incremental=update`, the error.

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
  full link; determinism is IN-6. *Test:* L10 runs each step's update
  with `--threads=1` and with the default, and the outputs must be
  byte-identical (IN-6).
- **IN-16 [v1] Too much changed.** When the units to refill hold more
  than half of the output's allocated bytes, or more than half of the
  inputs changed, the update falls back (`too-much-changed`): a full
  link costs about the same, and compacts. The threshold is a constant
  of the policy, reviewed at S4 (§10) with L10's statistics. *Test:* an
  L10 step that changes more than half the inputs (a header every unit
  includes) must report `too-much-changed`, and one just under the
  threshold must update.

### 5.3 The link state

- **IN-17 [v1] Where.** A separate file, `<output>.mortar-state` in the
  output's directory unless `--incremental-state` says otherwise; written
  as a temporary file in the same directory and renamed (IN-46), so a
  reader never sees half a state. Not inside the output: gold's
  in-output sections are stripped by `strip`, copied by `cp` and
  `install` with the output, and would make an incremental-mode output
  differ in its sections from a plain one (§2.7). *Test:* a link test
  checks the state's path with and without `--incremental-state`, that
  no temporary file is left after success or a crash at IN-46 step 5,
  and that a plain link creates no state file.
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

  | table | rows | protoc `-g`, measured (R12) |
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
  | archive symbol tables (armaps): per archive, symbol → member | 32,338 (`libprotobuf.a`) + 4,578 (`libc.a`) + 7,414 (`libstdc++.a`) + the others | 0.53 MB for those three, measured |
  | section content hashes (IN-31, [v2]) | one per live input section | 0.44 MB, measured |
  | string pool: symbol names and paths | | 3.9–4.1 MB (7.4 MB if local names were kept; they are not, IN-49a) |
  | **total** | | **8.05 MB measured** as written by R12 (16% of the 50.3 MB output) |

  R12 (`state_emul.py`) writes the state's records as bytes from the
  protoc `-g` map and inputs (files 56 KB, units 658 KB, pieces 856 KB,
  global definitions 649 KB, the index's 67,627 (unit, global) rows 812
  KB, three armaps 532 KB, section hashes 439 KB, strings 4.05 MB) and
  times it: written (temporary file, `fdatasync`, rename) in 7.0 ms and
  read back with a CRC in 3.1 ms on overlay, load ~14. The emulation
  leaves out the per-section rows for discarded sections and the own-file
  section targets of the index (118,145 − 67,661 rows, ~0.4 MB), so 8–9
  MB is the expected size. **Armaps are stored** rather than re-read:
  re-reading and parsing them costs 7.5 ms for `libprotobuf.a`'s 3.3 MB
  armap, 0.47 ms for `libc.a`'s and 0.86 ms for `libstdc++.a`'s (R12),
  ~10 ms per update for protoc's archives against ~1 ms to read them from
  the state; a stored armap is used only while its archive is clean
  (IN-22), and a changed archive is read whole anyway (IN-24). Not
  stored: section bytes and relocations (re-read from the inputs,
  IN-14), anything recomputable from the plan. A field-level index (one
  row per relocation, model §7.2 read literally) would be 1,776,710 rows,
  about 28 MB; the unit-level one is under 1 MB. *Test:* the state's size
  is in the bench's incremental rows (§8.2); a regression of more than
  20% is flagged.
- **IN-20 [v1] Written by streaming.** The state's tables are written as
  the phases produce them: summaries after input, the index during the
  scan (each worker emits its units' pairs, sorted and deduplicated per
  unit), decision tables after layout. No phase keeps a copy for the
  state's sake beyond what it already holds. *Test:* peak RSS of an
  incremental-mode full link is within 10% of a plain link's (bench).
- **IN-21 [v1] Bound to its output.** After its last write to the
  output (IN-46 step 5), the link `fstat`s it, then takes **stamps**
  (IN-22a) on the output's file system until a stamp's ctime is later
  than the output's (at most one timestamp tick: measured 0.9–1.0 ms on
  this host's overlay, whose ctime tick is 1 ms, and 0.08–0.12 ms on its
  virtiofs, tick 82 µs; R6), `fstat`s the output again, and records the
  second tuple `(dev, ino, size, mtime_ns, ctime_ns)`, that stamp's
  ctime as the output's **reference stamp**, and the build-id. If the two
  `fstat`s differ, something wrote the output during the link: no state
  is written (the next link says `output-changed`). An update proceeds
  only if `stat(P)` gives the recorded tuple, `P`'s ctime is strictly
  older than its reference stamp, and the build-id note read from `P` (one
  positioned read at the offset the state records) is the same. A tuple
  that differs is `output-changed`. A tuple that is equal but whose ctime
  is not older than the reference stamp (a write in the stamp's own tick,
  after the link) is **racy**: the output's bytes are re-hashed against
  the stored build-id leaves (image.md I-76; one read of the output, ~10–
  20 ms for protoc's 50 MB from the page cache, *estimate* from R1s read
  rates) and the update proceeds if every leaf matches, else
  `output-changed`; racy is never by itself a full link. Without a
  build id (`--build-id=none`) the leaves are still stored in the state
  (they are computed for the windows anyway, I-95), so binding does not
  depend on the note. Every comparison is between timestamps of one file
  system; none is with a clock. What this cannot see: a write to `P` by
  another process *during* the link (between the last write and the
  second `fstat`, with the size kept), which the link's lock (IN-45)
  excludes for cooperating writers and nothing excludes for others, as
  for any file two programs write at once. The state also has the
  "updating" flag of IN-46: set means `output-torn`. *Test:* `touch`,
  `strip`, `cp` of another output, `objcopy` in place, each over the
  output: each gives `output-changed`; a killed update gives
  `output-torn`; `stamp_rule.py` case B (an update followed at once by
  the next link's check, 200–400 times on overlay and virtiofs: own
  output seen as changed 0 times) and the reviewer's `state_tick.py`
  (39/200 and 60/200 own-output fall-backs under the old rule) become L10
  cases on a coarse-timestamp file system.

### 5.4 Change detection

- **IN-22a [v1] Stamps.** A **stamp** on a file system is the ctime of a
  file the link creates there (`<dir>/.mortar-stamp-<pid>`, created,
  `fstat`ed and unlinked; measured 8–10 µs on overlay, 0.3–0.5 ms on
  virtiofs, R6). Before it `stat`s any input, a link in incremental
  mode takes one stamp on each file system (`st_dev`) that holds an
  input, in a writable directory of that file system among those it
  knows (the output's, the state's, each input's, `TMPDIR`), and records
  it in the state per device. A file system on which no stamp can be
  created is **unstampable**; its inputs are hashed on every update
  (IN-22). Timestamps are compared only with stamps of their own file
  system, never with a clock: file systems have their own timestamp
  granularity (1 ms on this host's overlay, 82 µs on its virtiofs), and a
  wall clock is finer than either. *Test:* `stamp_rule.py` (R6) with the
  state and the inputs on the same and on different file systems.
- **IN-22 [v1] Stat, then content, per file.** An input (file or
  archive) is **clean** when its path resolves as before (the probes of
  `spec/input.md` §4.6 re-checked by `stat`), its `(dev, ino, size,
  mtime_ns, ctime_ns)` equals the state's, and its ctime is strictly
  older than the previous link's stamp of its file system (IN-22a). An
  input with an equal tuple and a ctime not older than that stamp is
  **racy** (it may have been rewritten in the same tick the previous link
  read it, with its size kept), and is hashed like a changed one: racy
  is never "unchanged" and never by itself a full link. An input on an
  unstampable file system is hashed. Every input that is not clean is
  read and its L128 hash compared with the state's: equal means
  unchanged (a `touch`, a racy file that was not rewritten), different
  means changed. Measured (R6, 2,000 trials on overlay): a same-size
  rewrite right after the link read the file kept its tuple 1,942 times;
  the old rule (ctime older than the link's start by the wall clock) took
  every one of them as unchanged, the stamp rule none (all 1,942 racy and
  hashed); on virtiofs neither rule missed one. The reviewer's
  `clock_race.py` (2,000/2,000 late, 1,983/2,000 tuple-identical on
  overlay) is the same case. Per-file hashes decide change; per-section
  hashes are IN-31's, [v2]. `--incremental-verify` hashes every input
  regardless of stat (all 217 MB of protoc's inputs, 120–220 ms), for
  file systems whose timestamps cannot be trusted. *Test:* L10 steps that
  rewrite an input with the same size and an `mtime` set back
  (`touch -d`), that `touch` without changing, that replace a file by
  `rename` (a new inode), and `stamp_rule.py` case A and `clock_race.py`
  on an overlay file system.
- **IN-23 [v1] Hints only ever add work.** No option or input from a
  build system or from kiln can mark an input unchanged: a wrong
  "unchanged" would give a wrong program silently, which is the one
  failure mortar may not have. `--incremental-changed=PATH` (repeatable)
  marks an input changed even when stat says otherwise (for build
  systems that restore timestamps). *Test:* an input rewritten with its
  old stat tuple (`touch -d` plus same size) and the hint gives the new
  program.
- **IN-24 [v1] Archives.** A regular archive that is not clean (IN-22) is
  read whole, its members' bytes hashed in parallel (one task per
  member); members are matched by identity (member name and ordinal among
  same-named members, model §7.1) and each member's L128 hash compared,
  so only changed members count as changed files; added and removed
  members change the archive's symbol index, and resolution (IN-26)
  decides what is extracted. A thin archive's members are files and are
  stat'ed as such (IN-22). Measured (R1, the "detect" part): reading and
  hashing `libprotobuf.a` (84.7 MB) costs 53–94 ms here on one thread
  (one `read` of the file from the page cache, then CRC-32 per member),
  40–70% of a regular-archive update (§1.5); parallel hashing divides
  only the hash part. The reviewer's cheaper rule (hash only
  members whose offset, size or header changed) is **not sound** for the
  archives developers have: Debian's and most distributions' `ar` is
  deterministic (`D`) by default and writes every member's date, uid and
  gid as 0 (checked: `ar tv libprotobuf.a` shows 1970-01-01 for every
  member), so an edit that keeps an object's size (a changed constant, a
  same-length string) changes no header field and no offset, and the rule
  would keep the old member. Without `D` the date has one-second
  resolution, which a compile and an `ar` inside one second defeat the
  same way. R2-C (§1.1) confirms it on the corpus: after a same-size
  member edit the member's header and every offset are identical and
  only the member's bytes differ. *Test:* L10 replaces
  a member (with the same size too), adds one, removes one, and reorders
  the archive (`ar` with `-b`).

### 5.5 Delta resolution and liveness

- **IN-25 [v1] The dependency index** implements `model.md` §7.2 ("for
  every target, the units whose bytes depend on it") at unit granularity,
  as §7.2 already defines it; it narrows nothing. Its targets are of two
  kinds. **Non-relocation targets**, each with the units that depend on
  it, recorded by the phase that makes the decision: a GOT, PLT, IPLT,
  TLS-descriptor or copy slot (the slot's own unit, and every unit with
  a relocation that uses the slot); a demand flag of a global
  (`GOT_NORELAX`, `TLSIE_NORELAX`, `CANON_PLT`, `COPY`; the units whose
  relocations raised it); a range-extension thunk, an erratum patch and
  their redirected sites (arch.md "Incremental links": the island's unit
  and each site's unit, because a redirected site names the patch, not
  the symbol); an end symbol of a packed section or segment
  (`__init_array_end`, `__start_SEC`, `_end`, …); a synthetic table's
  base (`_GLOBAL_OFFSET_TABLE_`, `.eh_frame_hdr`'s table). **Relocation
  targets**: one row per distinct (referring unit, target), from the
  relocations of:
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
  | a COMDAT group's keeper changed | the old keeper's group members become holes; the new keeper's are placed (§5.6) and refilled; the referrers of the group's symbols; and **every file whose copy of the group became dead or live** (the old and the new keeper's files, changed or not): that file's `.eh_frame` contribution and all its non-allocated units |
  | a demand flag of a global changed (`GOT_NORELAX`, `TLSIE_NORELAX`, `CANON_PLT`, `COPY`; model §7.2) | the units the index records for that flag and that global |
  | a section of an unchanged file moved (a growth step, L-144) | that section's unit; the index's referrers of the section and of the globals defined in it; all non-allocated units of its file |
  | a synthetic table changed (§5.7) | its units, and the referrers of its end symbols |
  | a patch or thunk was added, moved or removed (arch.md "Incremental links") | the sites' units and the island's |
  | liveness changed (IN-29): a section of any file became dead or live | sections that became live are placed and refilled; sections that died become holes; the referrers of their globals; and that file's `.eh_frame` contribution and all its non-allocated units |
  | any TLS input section's TP offset would change, in any file | none: `tls-layout-changed` (IN-34) |

  The COMDAT and liveness rows cover the reviewer's B4 (`comdat_keeper.sh`):
  the old keeper's file `a.o` is unchanged, yet its `.debug_info` names
  the dropped copy through a section symbol (its `DW_AT_low_pc` is an
  address in one full link and a tombstone in the other) and its
  `.eh_frame` range holds the copy's FDE, which a full link drops (S-46);
  without the row an exception through the revived copy calls
  `std::terminate`. Units are refilled whole (model §7.2): relaxations,
  sorted tables and encodings need no special case. *Test:* IN-1 (B) and
  (C); a unit test per row with a two-file program; the refill oracle
  (IN-63a) on every L10 step. R3 (§12) ran it over a body edit (same
  size, +16 bytes with and without slack), both directions of a keeper
  change (the reviewer's script and a size-neutral swap), a gc kill and
  revive (with and without unwind tables), TLS growth on AArch64 and
  x86-64, and a value change seen only by debug information, each at
  GCC `-O0 -g` and `-O2 -g`: in all 32 pairs every differing unit of an
  unchanged file is predicted (and the mutation table, IN-63b, shows
  which row each catches). The oracle compares two *full* links, so its
  predictor has two causes an update does not have, because an update
  keeps them in place: a slot that moved in a re-laid synthetic table
  (`crti.o`'s `.text` names `__gmon_start__`'s GOT slot, which moved
  with `.got`; the synthetic-table row) and a merged fragment that moved
  (`.debug_line_str` reordered by a changed file placed first, so an
  unchanged file's `DW_FORM_line_strp` value changed; an update appends
  fragments, S-110, so it does not happen there). The moved-section
  row's "all non-allocated units of its file" applies when an
  **allocated** section moved; the oracle applying it to every offset
  shift in `.eh_frame` or a debug section masked the COMDAT and
  liveness rows.
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
  liveness row. *Test:* L10 removes the last reference to a function in
  another file and checks that the function's section became a hole and
  that file's FDE and debug entries were refilled (tombstoned); then
  restores the reference and checks the revived section has its FDE
  (an exception through it is caught) and live debug entries.

### 5.6 Delta layout and slack

- **IN-30 [v1] Regions, in hole-tolerant sections only.** In incremental
  mode, layout (layout.md §18) groups each file's contribution to each
  **hole-tolerant** output section (layout.md L-143: `.text` and other
  code, `.rodata`, `.data`, `.data.rel.ro`, `.bss`, `.gcc_except_table`;
  the TLS sections are L-143's "holes only at the end" case and follow
  IN-34's TLS row) and category (the GNU `.text` categories, L-53, are
  categories) into one **region**: the file's chunks in their full-link
  order, followed by slack. A changed file's chunks are laid out again
  from its region's start in their order; if they fit, nothing outside
  the region moves. Units of other files that name the changed file's
  globals are refilled through the index (IN-27); measured, that is 3
  units at the median and 533 at the 99th percentile (§1.4). Slack is
  past `size` only (L-141); a region's slack and the hole a shrinking
  file leaves are holes (IN-3). **A packed section** (layout.md L-142:
  `.init_array`, `.fini_array`, `.preinit_array`, `.ctors`, `.dtors`,
  `.init`, `.fini`, `.eh_frame`, `.eh_frame_hdr`, notes,
  `.tm_clone_table`, every section reached through `__start_`/`__stop_`
  symbols) **has no regions and no holes**: it is one chunk, rewritten
  whole in full-link order (IN-34) within its capacity (IN-33). R4
  (`packed_holes.sh`) shows why: 64 bytes of trap fill between `crti.o`'s
  and `crtn.o`'s `.init` pieces kill the program at start-up (SIGILL), an
  8-byte zero slot in a `__start_`/`__stop_` pointer array crashes it
  (SIGSEGV), and moving one file's contribution to the section's tail
  makes it print `b a` where a full link prints `a b`. *Test:* L10
  body-grow and body-shrink steps check that only the region's file and
  its referrers were refilled (`--incremental-report`); R4's three cases
  in L10 with the fixed rule (the update's packed sections equal the full
  link's order, IN-2 item 5).
- **IN-31 [v2] Section-level refill inside a changed file** (the M12
  package, §10). v1's unit of change detection stays the file; this is
  the designed second step, specified now so that v1's state and index
  leave room for it:
  - under `--incremental` only, every link computes per live input
    section a **content hash**: L128 over the section's bytes and its
    relocation multiset (each record as (offset, type, target identity,
    addend), sorted), and stores it in the state's sections table (16
    bytes a row). A link without `--incremental` computes none (the
    bench gate of §8.1 covers it);
  - in an update, the sections of a changed file are matched to its
    previous version by anchor (model §7.1). A matched section whose
    anchor, size and content hash are all unchanged, **and none of whose
    relocation targets moved**, keeps its place and is not refilled; every
    other section of the file is laid out in its region around the kept
    ones and refilled; the file's debug contributions are replaced as in
    §6 (under `-g` they change almost whole, §1.2);
  - nothing is trusted from a compiler: the hash is mortar's, over the
    bytes it reads (the corpus shows `__LINE__`, `assert` and assembler
    pre-resolved branches changing functions far from the edit, §1.1).
  Measured costs on protoc `-g` (R12): 27,416 live input sections, 35.6
  MB of section bytes, 1,717,502 relocations. The hashes add 0.44 MB to a
  state of 8.05 MB (5.4%). Their computation is 35.6 MB of L128 (~3 ms of
  one thread at 12.6 GB/s, derived from image.md I-74) plus a sort and
  hash of 1.7 M relocation records (0.38 s in Python's `hashlib` here
  for the whole job, an upper bound; *estimate* 10–30 ms of one thread
  natively, spread over the input workers), against a 0.18–0.22 s full
  link: 13–33 ms of CPU, 2–5 ms of wall time if spread over 8 workers,
  *estimate*; zero for a plain link.
  The gain (the coarseness table, §1.2) is large exactly without `-g`
  and with `-ffunction-sections` (the bench's GCC/Clang defaults): a
  same-size constant edit refills one 104–120-byte section instead of
  176–234 KB. When it goes first rather than second is §1.5's go/no-go
  rule. *Test (WP-068):* L10 with `-ffunction-sections` and no `-g`: the
  refilled bytes of e1 equal the differing section's size, and the
  oracle (IN-63a) still holds; a mutation that keeps a section whose
  relocation target moved must be caught (IN-63b).
- **IN-32 [v1] Overflow.** A region that no longer fits its contribution
  becomes a hole, and the contribution is placed at the **tail reserve**
  of its output section (category tail), with a fresh region's slack. If
  the tail reserve is exhausted, the output section grows by a growth
  step (L-144) into the **segment reserve**, moving the output sections
  after it in the segment (their units and referrers are refilled,
  IN-27). If the segment reserve is exhausted: `reserve-exhausted`. A
  packed section never overflows into a tail: it is rewritten whole
  within its capacity, and grows by a growth step (moving what follows
  it) when its capacity is exhausted. A hole is reused only by its own
  file's region (to keep the policy a function of history that is simple
  to state); [later] a free list with first-fit for any region (debug
  sections have one in v1, IN-50). *Test:* L10 grows one function step by
  step past its region, its section's tail and its segment's reserve,
  and checks the reasons.
- **IN-33 [v1] How much slack** (answers layout.md Q8: per region in
  L-143 sections, with per-section and per-segment reserves):

  | where | capacity | protoc `-g` (measured model, `regions2.py`) |
  |---|---|---|
  | each region (allocated, L-143 sections only) | `size + max(64 B, size/8)`, rounded up to the alignment | 797 KB over 5.28 MB in 2,936 regions (1,820 of them under 64 B, so the minimum dominates); `.bss` regions included, which cost no file bytes, so an upper bound |
  | each allocated output section's tail | `max(4 KiB, size/16)` | 463 KB |
  | each `PT_LOAD`: gap to the next, in file and address space | `max(64 KiB, filesz/8)` rounded up to the maximum page size (64 KiB here) | 983 KB (4 `PT_LOAD`s) |
  | each packed (L-142) section | `size/8` past the chunk (IN-30) | 122 KB |
  | each debug section's tail (§6) | `size/32` | 1.20 MB |
  | each debug contribution (§6, IN-50) | its size class: `size` rounded up to `m·2^k`, `m` in 8..15, at least 32 B | 1.45 MB of rounding (+4.8% of the debug bytes) |
  | merged sections (`.rodata.str*`, `.comment`, `.debug_str`) | their tail (L-147) | in the rows above |
  | `.symtab`, `.strtab` | none (rewritten whole, S-112) | 0 |

  On protoc the output grows by **5.02 MB (+10.0% of 50.3 MB) with `-g`**
  and **2.37 MB (+20.1% of 11.8 MB) without** (`regions2.py` over the real
  map and output; the previous draft's 13% / 10% were not a model of
  these rules, and the reviewer's model of them gave +14.2% with `-g`).
  Without `-g` the fixed minimums (4 `PT_LOAD` reserves of at least 64
  KiB, 4 KiB section tails, 64-byte region minimums) are most of it; on a
  larger program they shrink relative to the output. The debug tail was
  sized from the M2 model (IN-57): with `size/32` plus size classes, the
  p50 and p90 files take more than 500 consecutive corpus-drift updates
  before compaction, the p99 file 297 and the largest 227; with no debug
  tail, 18–121; tails of `size/8` (+17.2% in all) reach 500 for all four
  but only on the corpus's drift, which is nearly 1.0 (IN-57 gives the
  sensitivity to growing edits). The operator decides the overhead
  (record (a)).
  The reserves are written as fill bytes, not left sparse (a synced or
  copied file would expand them anyway). A plain link uses capacity =
  size everywhere: the same code path with a zero slack function.
  *Test:* the bench's incremental rows report the output size of an
  incremental-mode first link against a plain one, and S2 compares it
  with this table.

### 5.7 Special structures

- **IN-34 [v1] The table.** Every special structure, its rule in an
  update, and its fall-back:

  | structure | rule | fall-back |
  |---|---|---|
  | `.text` and other code | regions with trap-fill holes (IN-30, IN-3) | `reserve-exhausted` |
  | `.rodata`, `.data`, `.data.rel.ro`, `.bss` | regions; zero holes; `.bss` holes cost no file bytes | `reserve-exhausted` |
  | `.tdata`, `.tbss` (the TLS template) | a changed file's TLS contribution is refilled in place **only if no TLS input section's TP offset changes, in any file**; the update computes every TLS input section's TP offset under the new layout and compares it with the stored one. Measured (the reviewer's `tls_shift.sh`, and R3's oracle over it): making one `thread_local` initialized (`.tbss` → `.tdata`) keeps `PT_TLS`'s size and alignment yet moves an unchanged file's `c_var` from TP+0x30 to TP+0x38 (its `.tbss` section from TP+0x40 to TP+0x48, confirmed in the disassembly: `add x0, x0, #0x40` → `#0x48`); two files changing by −8 and +8 move an unchanged `e.o`'s `.tbss` from TP+0x68 to TP+0x60; on x86-64 (TLS variant II, negative offsets) any growth of the template moves every unchanged TLS section (`c.o` −0x78 → −0x80, `e.o` −0x50 → −0x58) with no address moving | `tls-layout-changed`; [later] refill every TP-relative referrer of every moved TLS section through the index |
  | `.init_array`, `.fini_array`, `.preinit_array`, `.ctors`, `.dtors`, `.init`, `.fini`, `.tm_clone_table`, `__start_`/`__stop_` sections (packed, L-142) | one chunk, no holes, rewritten whole in full-link order (priority, then load order; for `.init`/`.fini`, `crti.o`'s prologue first and `crtn.o`'s epilogue last) within capacity (IN-30); `__init_array_end`, `__stop_SEC` and the like move, and their users are refilled (IN-25) | growth step; `reserve-exhausted` |
  | `.eh_frame` | per-file ranges in full-link order, padding-CIE holes of 0 or ≥ 16 bytes, the terminator last; a range that overflows rewrites the section whole (IN-34a) | growth step; `reserve-exhausted` |
  | `.eh_frame_hdr` | one unit, rewritten whole when any FDE address changes (S-111); its table lists only live FDEs | growth step; `reserve-exhausted` |
  | `.gcc_except_table` | regions; zero holes (reached only through LSDA pointers) | `reserve-exhausted` |
  | merged strings and constants | fragments appended (L-147, S-110); dead fragments keep their bytes and are dead ranges (IN-3a: a live reference may share a suffix) | `reserve-exhausted` |
  | GOT, PLT, IPLT, `.got.plt`, `.rela.plt` | S-113: slots appended within capacity, dead slots written 0; `.rela.plt` compacted | `reserve-exhausted` |
  | copy relocations (`R_*_COPY`, M2/M4) | the copy slot keeps its place and size; a shared library's object whose size changes needs a bigger slot | `copy-size-changed` |
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
- **IN-34a [v1] `.eh_frame`.** The one packed section (L-142) that
  takes holes, because a hole there can be a record the walkers parse.
  - **Ranges.** In incremental mode the records are grouped per file into
    ranges in S-46's order, and CIEs are not shared across files (S-44's
    deduplication is per file), so that no FDE points into another
    file's range; on protoc that keeps at most one extra CIE per file
    (784 files, ~20 KB, *estimate* from S-44's record sizes). A changed
    file's range is rewritten from its start; it may shrink or grow
    within the range's capacity (IN-33).
  - **Hole sizes.** Layout keeps every hole either 0 or at least 16
    bytes. A difference of 4, 8 or 12 bytes between a range's capacity
    and its records is absorbed by the range's last record: its length
    grows by that many bytes of `DW_CFA_nop` (S-47 already pads records
    this way). A hole of 16 bytes or more is **one padding CIE**:
    `length` = hole − 4 (u32), `CIE_id` 0 (u32), version 1, augmentation
    `""` (one 0 byte), code alignment 1 (ULEB `01`), data alignment −4
    (SLEB `7c`, on every target: no FDE uses the CIE, so its factors
    are never applied), return-address register (one byte: 30 on AArch64, 16 on x86-64, 8 on
    i386), then `DW_CFA_nop` (0) to the end. No FDE uses it. 16, not the
    12 the ruling proposed: a whole CIE is 13 bytes, so the smallest
    4-aligned one is 16; a 12-byte CIE must drop the return-address
    register.
  - **Overflow.** A range that does not fit its capacity does not go to
    a tail: the whole section is rewritten in full-link order with fresh
    range capacities (IN-30's rule for packed sections), so IN-2 item 5
    holds for `.eh_frame` too (B2 and B5 together; moving the range to a
    tail would put its FDEs out of full-link order). On protoc that is
    0.97 MB, one or two dirty windows, and `.eh_frame_hdr` is rewritten
    whenever an FDE address changes anyway (S-111). **The terminator is
    always last**: S-48's zero word is the last 4 bytes of the section's
    content, and when the content's end moves the old terminator's word
    becomes part of a padding CIE (or of the last record's padding); R5
    shows a terminator left before live FDEs aborts a throw.
  - Measured (R5, `eh_holes.sh` with `ehpatch.py`, a g++ `-O0 -g` static
    program whose `f_dead` FDE is turned into a hole, linked by mortar
    with `.eh_frame_hdr` and by GNU ld without; throw/catch under
    libgcc, gdb and lldb backtraces through `thrower`, `readelf -wf` and
    `llvm-dwarfdump --eh-frame` errors):

    | hole holds | throw/catch | gdb, lldb `bt` | readelf | llvm-dwarfdump |
    |---|---|---|---|---|
    | one padding CIE over the whole FDE | caught | ok | clean | clean |
    | previous record +4 nop, padding CIE over the rest | caught | ok | clean | clean |
    | previous record absorbs all but 16 bytes, one 16-byte CIE | caught | ok | clean | clean |
    | previous record absorbs all but 12 bytes, one 12-byte CIE (no RA register) | caught | ok | clean | **error** ("parsing entry instructions … failed") |
    | a CIE, then an 8-byte CIE with no body | caught | ok | clean | **error** |
    | a CIE, then a 4-byte zero word | **aborted** | ok | clean | clean |
    | all zero | **aborted** | ok | clean | clean |
    | a terminator before live FDEs (range moved, terminator left) | **aborted** | ok | clean | clean |

    The same with and without `.eh_frame_hdr`: static start-up registers
    `.eh_frame` through `crtbeginT.o`, whose walk stops at the first zero
    word. LLVM libunwind is not installed on this host and not in
    `scripts/provision.sh`'s package list, so it was not tested (the
    installed `libunwind8` is the nongnu libunwind runtime, without
    headers); WP-064's gate adds it where available.
  *Test:* R5 in L10 (each row a scenario; the first three must pass, the
  others are the verifier's negative cases: plumb rejects a hole that is
  not one padding CIE of ≥ 16 bytes, and a zero word before the end).
- **IN-35 [v1] The entry point and `.interp`**: `e_entry` is in the ELF
  header, rewritten every update; `.interp` does not change unless the
  options do (`options-changed`). *Test:* an L10 step that changes
  `-e` reports `options-changed`; the oracle (IN-63a) checks `e_entry`
  after an edit that moves the entry function.

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
  for the renumbering. A copy slot whose object's size changes in the
  shared library is `copy-size-changed` (IN-34). Programs export few
  symbols (thousands, not the 31,478 of `.symtab`), so the rewrite costs
  little; D-28's fixed `nbuckets` stays the `[M7 hook]` dyn.md already
  calls it, an optimisation [later] that v1 does not use.
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
  changing, or the spares running out, is `dynamic-changed`. *Test:* L10
  (WP-066) adds a `-l` library (a spare used), adds six (the spares run
  out: `dynamic-changed`) and swaps two (`dynamic-changed`).
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
- **IN-45 [v1] Copy by default; in place on request.** *(The default is
  an operator decision, recorded with its options and measured costs in
  the WP-060 record, "Operator decisions"; this text is the
  recommendation, option (c).)* An update writes a copy of the output and
  renames it over `P` (IN-47) unless `--incremental-write=in-place` is
  given. Why not in place by default, measured (R7, `write_safety.py`):
  - `ETXTBSY` is not a test of "no process runs `P`": a program started
    as `/lib/ld-linux-aarch64.so.1 ./prog`, or under `qemu-aarch64`, maps
    `P` with no write denial; `open(P, O_WRONLY)` succeeds while it runs,
    and a `pwrite` of new code into `P` made the running process print
    the **new** function's output (it ran a mix of two programs); an
    `O_TRUNC` rewrite killed it (SIGSEGV);
  - a write lease (`F_SETLEASE`, `F_WRLCK`) is no better: it is granted
    while such a process runs (it detects open descriptors, not
    mappings), and refused only where `ETXTBSY` already is;
  - lldb with `P` loaded maps `P` without write denial (open succeeds,
    lease granted), and crashed (a stack dump in liblldb on `bt`) after
    `P` was rewritten in place under it; gdb with `P` loaded holds a
    descriptor and reads debug sections lazily, so an in-place write can
    give it a mix of old and new sections;
  - a scan of `/proc/*/maps` for `P`'s (device, inode) finds the
    `ld.so`/qemu mappers, but costs 1.0 ms with 57 processes, 5.6 ms with
    575 and 55 ms with 5,083 (R7), sees only the link's own pid
    namespace (not other containers or the host) and, for an
    unprivileged user, only that user's processes; and `/proc/PID/
    map_files` cannot be `stat`ed here (EPERM).
  The copy costs 18–27 ms for protoc's 50 MB output at low load (first
  run 40–58 ms; R7) and 52–76 ms at load ~20 (the reviewer's
  `write_paths.py`), i.e. ~0.4–1.5 s per GiB, and nothing where the file
  system clones (`FICLONE`; not this host's overlay, E5). With
  `--incremental-write=in-place`, the output is updated in place only
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
  - no process visible in `/proc` maps `P` or holds it open (a scan of
    `/proc/*/maps`, measured above, and of `/proc/*/fd`, whose cost is not
    measured here), and the user is
    told, once per link, that processes outside the scan's view are not
    protected (`incremental-in-place-unchecked`, a note);
  - an exclusive lock on the state file is acquired and the file system
    enforces it (the probe of image.md I-59); otherwise two concurrent
    updates of one output could interleave their writes. (The
    workspace's virtiofs now enforces `flock`, the reviewer's m2.)
  What a debugger holding the old file sees (R7): after copy and rename,
  gdb and lldb keep the old inode and show the old program consistently
  (old symbols, old line table, the running process's old code) until
  the next `run`, where gdb prints "`prog' has changed; re-reading
  symbols" and lldb re-reads silently; a process started before keeps
  running the old code (`/proc/PID/exe` shows `prog (deleted)`). In
  place, a debuggee running under gdb or lldb makes the open fail with
  `ETXTBSY`, so the update takes the copy path anyway; a debugger that
  has loaded `P` but not run it is the unprotected case above. *Test:*
  one link test per condition, checking the inode afterwards (same for
  in place, new for copy); R7's `ld.so` and qemu cases with the scan.
- **IN-46 [v1] The order of writes, in place.** Correctness first: a
  torn output must not look valid to the next link **or to a user who
  runs it**, across a crash and a power loss.
  1. set the state's "updating" flag (a positioned write of its header)
     and `fdatasync` the state;
  2. write the output's ELF header with `e_ident` zeroed, and
     `fdatasync` the output: from here a torn output cannot be executed
     ("exec format error") instead of running a mix;
  3. write the dirty windows; truncate or extend (image.md I-97);
     `fdatasync` the output;
  4. write the true ELF header; `fdatasync` the output;
  5. write the new state to a temporary file, `fdatasync` it, rename it
     over the old state (which clears the flag), `fsync` the directory.
  Without the syncs of steps 2 and 3, the page cache may write the
  step-4 header back before the step-3 windows, and after a power loss
  the file has a valid magic over half-written windows (the reviewer's
  M3). Measured (R7, killing the process after each step, protoc's 50 MB
  output, four 1 MiB windows): after step 1 the old output runs and the
  flag is set; after 2 and 3 `exec` fails with `ENOEXEC`; after 4 the new
  output runs and the flag is still set (the next link says
  `output-torn`); after 5 the new output runs and the flag is clear.
  Power loss is covered by argument (each sync orders the write before
  it ahead of every write after it), not by a test here: this host has
  no `dm-flakey`. Cost of the syncs (R7, median of 5, write + sync):
  step 1 0.05 + 3.70 ms on overlay, 0.24 + 0.79 ms on virtiofs; step 2
  0.03 + 0.25 / 0.20 + 0.15; step 3 (4 MiB) 2.38 + 2.51 / 6.03 + 0.88;
  step 4 0.03 + 0.21 / 0.15 + 0.10; step 5 0.14 + 0.92 + 0.08 / 0.62 +
  0.11 + 0.42, plus the directory sync 0.62 / 0.23: about 8.2 ms of syncs
  on overlay and 2.3 ms on virtiofs, included in the floor (§1.5). A full
  link does not sync (image.md I-61) and does not need to, because it
  renames a complete file. **Operator option, not the default:**
  `--incremental-sync=next-link` drops the syncs after steps 2 and 3 and
  keeps those of steps 1, 4 and 5, so the *next link* is protected across
  a power loss (the state never reaches the disk before the output's
  data) but a user who runs the output after one may run a mix. It saves
  two syncs' fixed cost, ~0.3–0.5 ms (the window data is synced once
  either way; R7's step 2 and 4 syncs). *Test:* a crash-injection hook (a test-only environment
  variable that aborts after step k) at each step; the next link must
  report the reason and give an output equivalent to a full link's, and
  `exec` of the torn output must fail with `ENOEXEC` after steps 2 and 3.
- **IN-47 [v1] The copy path** (the default, IN-45). Copy `P` to `T`
  (image.md I-59's name and lock) with `std::io::copy`
  (`copy_file_range`; reflink where the file system clones), write the
  dirty windows and the true header into `T`, `fdatasync` `T`, rename
  `T` over `P` (image.md I-60, I-113), `fsync` the directory, then step
  5's state write. `P` is never torn: it is the old or the new file, so
  no flag and no zeroed header are needed (the flag is still set, step
  1, so that a crash between the rename and the state write is
  `output-torn`, not a stale state against a new output; the stat tuple
  would catch it too). Without the sync of `T` before the rename, the
  rename can reach the disk before `T`'s data and leave a torn `P` after
  a power loss. Measured (R7): 27.0 ms on overlay (copy 13.6,
  `fdatasync` 11.7, rename 2.0) and 17.8 ms on virtiofs at load ~2, first
  runs 57.6 and 39.8 ms; 52–76 ms at load ~20 (`write_paths.py`); 1 GiB
  copies took 1.8–2.9 s (E5). *Test:* the crash hook on the copy path:
  `P` is the old or the new file after a kill at every step.
- **IN-48 [v1] The state is written only by a successful link**
  (image.md I-100). A failed update removes the output (image.md I-66)
  and the state. *Test:* an update that introduces an undefined symbol
  leaves neither file; the next `--incremental` link reports `no-state`.
- **IN-49 [v1] The map** (`--mortar-map`, plumb.md §2.3) is rewritten
  whole (image.md I-99), with `hole` and `dead` lines (§8.2). *Test:*
  plumb reads every L10 step's map.
- **IN-49a [v1] `.symtab` and `.strtab`.** S-112 rewrites them whole. In
  an update, the entries of unchanged files (163,381 local and 76,402
  global mentions on protoc, §1.4) are taken from the **previous
  output's** `.symtab`/`.strtab` (bound to the state by IN-21), with
  values remapped through the placement, and the changed files' entries
  from their parsed symbol tables; neither the state (which would need
  7.4 MB of local names, IN-19) nor the unchanged inputs are read for
  them. Cost: one read of 5.4 MB from the output (*estimate* 2–5 ms from
  R1s read rates) and the windows they occupy written whole (in the
  floor's dirty windows, §1.5). *Test:* (B) of IN-1 covers it (a replay
  builds `.symtab` from the inputs); `nm` of the update equals `nm` of a
  full link modulo values (Q9).

## 6. Debug information

What a debug contribution is: for each input file and each debug output
section, the file's bytes in that section (`nplace` lines of the map).
The file is the unit (§1.2): a changed file's debug contributions change
in content almost whole (DIE offsets shift) but in size hardly at all
(745 corpus samples: new/old 1.000 at the median, at most 1.015, §1.1).
Debug contributions refer to their own file only (104 of 170,477 debug
pairs name a global, §1.4), so a debug contribution is refilled when its
file changes or when one of its file's sections moves (IN-25).

- **IN-50 [v1] Where a new contribution goes.** Every debug contribution
  (strings excepted, IN-56) is **allotted its size class**: its size
  rounded up to `m·2^k` with `m` in 8..15 (at most 12.5% over, at least 32
  bytes), and to the next class when the slack would be smaller than the
  section's minimum neutral form (IN-51b); the bytes between its end and
  the allotment's end are a dead range in that form. A changed file's new contribution
  goes, first fit:
  1. **in place**, if it fits the file's current allotment (the common
     case: sizes barely move, §1.1);
  2. else **best fit into a dead range** of the same section, where the
     remainder is 0 or at least the section's minimum neutral length
     (IN-53);
  3. else **at the section's tail** reserve (IN-33);
  and the old allotment, if left, becomes a dead range (IN-3a). A full
  link's first-time placement is the full-link order with every
  contribution in its allotment. The order of units in `.debug_info` is
  then no longer the load order; nothing in DWARF depends on unit order
  (units are found by offset, through `.debug_aranges`, or by walking),
  and E6 links the new unit last with equal gdb transcripts. The rounding
  costs 1.45 MB on protoc `-g` (+4.8% of 30.4 MB of debug bytes,
  `debug_tail_reuse.py`). *Test:* E6b in L10 (IN-59); an L10 sequence
  that grows a file's debug contribution past its class, so that each of
  the three placements happens, with `--incremental-report` naming which.
- **IN-51 [v1] Dead `.debug_info` units are neutralized, in block
  form.** Each unit of a dead contribution keeps its offset and its
  `unit_length`, and becomes a compile unit of the same version and
  address size (DWARF 5: `DW_UT_compile`) holding **one DIE whose single
  attribute is a block covering the rest of the unit**: abbreviation
  code 1, `DW_TAG_compile_unit`, `DW_CHILDREN_no`, attribute 0x2000
  (`DW_AT_lo_user`, ULEB `80 40`) with form `DW_FORM_block` (0x09), whose
  ULEB length (padded to a fixed width when needed) makes the DIE end
  exactly at the unit's end, with no null entries after it. Its
  abbreviation table is `01 11 00 80 40 09 00 00`. For a file that has a
  `.debug_addr` contribution (Clang DWARF 5), the neutral unit also keeps
  `DW_AT_addr_base` (0x73, `DW_FORM_sec_offset` 0x17) aimed at the file's
  dead `.debug_addr` table, with the abbreviation table `01 11 00 73 17 80
  40 09 00 00`. readelf reads `.debug_addr` tables only through the units
  that name them, so an unnamed dead table between live ones gives
  "Corrupt .debug_addr section". The rest of the file's dead
  `.debug_abbrev` contribution is zero bytes, and each zero byte is an
  empty table. Minimum unit length: 14 bytes (DWARF 4), 15 (DWARF 5) or
  19 (with `DW_AT_addr_base`), so that the block holds at least one byte.
  A 0-byte block makes readelf warn "Block ends prematurely".
  Measured by R9 (`debug_chain.sh` with `dwarf_pad.py --form=block`): 20
  dead versions of one file, g++ and clang++, DWARF 4 and 5.

  | form | `--verify` errors | readelf `--debug-dump=info` warnings | gdb, lldb vs full link |
  |---|---|---|---|
  | the previous draft's (one DIE `01`, then null entries) | 0 | 4 per output ("Bogus end-of-siblings marker") | same |
  | **block** | 0 | 0 | same |
  | block with `DW_AT_addr_base` (Clang DWARF 5) | 0 | 0, and none in a full `readelf --debug-dump` | same |
  | tombstoned only (E6's "emul") | 0 on DWARF 4; 116 (g++) and 250 (clang) on DWARF 5 | 0 | differ |

  Type and skeleton units are detected (IN-55a) and fall back
  (`debug-unit-kind`) until S5 tests their conversion. *Test:* R9 in L10
  per compiler and DWARF version. plumb checks that each dead
  `.debug_info` range is a sequence of block-form units.
- **IN-51b [v1] Neutral forms of the other debug sections.** A dead
  range in a debug section is either a whole dead contribution left as
  its compiler wrote it (IN-53), or a remainder or allotment slack in the
  section's neutral form:

  | section | neutral form | minimum, granule (bytes) |
  |---|---|---|
  | `.debug_info` | IN-51's block unit | 14 / 15 / 19, 1 |
  | `.debug_abbrev` | zero bytes | 1, 1 |
  | `.debug_line` v5 | a header with `opcode_base` 7, one format entry `[DW_LNCT_path, DW_FORM_string]` for directories and for files, each with count 0, `header_length` exact, and the rest `DW_LNS_negate_stmt` (0x06) bytes | 32, 1 |
  | `.debug_line` v2–v4 | the same with empty directory and file lists | 24, 1 |
  | `.debug_rnglists`, `.debug_loclists` | a v5 header with `offset_entry_count` 0, the body zero (`DW_RLE_end_of_list`/`DW_LLE_end_of_list`) | 12, 1 |
  | `.debug_str_offsets` | a v5 header, then zero entries | 8, 4 |
  | `.debug_addr` | a v5 header (address size 8), then zeros | 8, 8; never split (see below) |
  | `.debug_ranges` (v4) | zero bytes | 16, 1 |
  | `.debug_frame` | a CIE (version 1, empty augmentation), then `DW_CFA_nop` | 13, 1 |
  | `.debug_aranges` | none: rewritten whole (IN-54) | — |

  Measured by R9 (`neutral_chain.sh`, `dwarf_neutral.py`). Neutral
  pieces of 4 to 36 lengths per section, many of them odd, were spliced
  into the 19 real dead contributions, and pieces of 6 lengths were
  appended to live sections. With g++ and clang++, DWARF 4 and 5, every
  form above passes readelf on its section, a full readelf dump,
  `llvm-dwarfdump --verify` and `--debug-<section>`, and the gdb and lldb
  transcripts. There are two exceptions.
  - An unreferenced `.debug_addr` remainder between referenced tables
    gives readelf's "Corrupt .debug_addr" warning. gdb, lldb and verify
    stay clean. Hence: a dead `.debug_addr` table is never split or
    reused, and it stays named by its neutral unit (IN-51).
  - readelf warns of "unused bytes at the end" of `.debug_loclists` for
    trailing unreferenced pieces. A dead `.debug_loclists` or
    `.debug_frame` range in the middle of a section was not tested (no
    build had one). S5 tests both before reuse is enabled in those two
    sections. Until then, they take only appends.

  The forms first proposed fail. A `.debug_line` header whose
  `header_length` covers the rest gives 156–350 readelf and 35–104 llvm
  warnings, and `.debug_aranges` padded with zeros after its terminator
  is a "premature terminator". **Remainders.** Size classes are 4 bytes
  apart at 32–60, so a remainder can be smaller than every minimum but
  `.debug_abbrev`'s. IN-50 therefore allots each contribution the
  smallest class `c` such that `c − size` is 0, or at least the
  section's minimum and a multiple of its granule. Reuse of a dead range
  likewise leaves a remainder of 0 or at least the minimum. The extra
  rounding is at most one minimum per contribution: ≤ 32 B × ~1,500
  contributions, under 50 KB on protoc (*estimate*). *Test:* R9's
  splices in L10, and plumb's check that every debug dead range parses
  as one or more of these forms or as a whole compiler-written
  contribution.
- **IN-52 [v1] Why not tombstones alone.** A dead unit left with
  tombstoned addresses (image.md I-47, what a full link with
  `--gc-sections` writes for a collected file's debug information) is
  still enumerated. gdb lists its variables, and lldb lists them and
  picks its stale type as the best match (E6, §1.6). On DWARF 5,
  `llvm-dwarfdump --verify` also reports 116–250 errors at 20 dead
  versions (R9). That is acceptable for a file a full link collected,
  because its types and variables were never part of the program. It is
  not acceptable for the old version of a file whose new version is
  live. Hence IN-51 for replaced files. A file that leaves the link (no
  longer extracted, removed from the command line) is neutralized too,
  so an update's debug information never holds more than a full link's.
  *Test:* E6's "emul" transcripts are kept in L10 as a negative case:
  the transcript comparison must flag them.
- **IN-53 [v1] Whole dead contributions reached only by offset.** A
  whole dead contribution in `.debug_line`, `.debug_str_offsets`,
  `.debug_addr`, `.debug_rnglists`, `.debug_loclists`, `.debug_ranges`,
  `.debug_loc`, `.debug_macro` or `.debug_frame` may keep the bytes its
  compiler wrote, with every address in it tombstoned (image.md I-47).
  Nothing live names it (IN-3a check 1). E6b and R9 left `.debug_line`,
  `.debug_str_offsets`, `.debug_addr` and `.debug_rnglists` this way and
  measured equal transcripts. When part of it is reused, the remainder
  takes IN-51b's form. **In a PIE or shared object**, the tombstone of
  `.debug_rnglists`, `.debug_loclists` and `.debug_addr` is 0
  (`mortar_arch`'s `dead.rs`, read, not run). A dead `[0, len)` entry
  there names a valid unrelocated address range, and R11 could not test
  it because mortar refuses `-pie` on this branch. So in those outputs,
  dead contributions to these three sections are rewritten in IN-51b's
  neutral form, never left as written, until WP-066 runs R11. *Test:*
  `llvm-dwarfdump --verify` of every L10 step with `-g`; R11 in WP-066's
  gate.
- **IN-54 [v1] `.debug_aranges` is rewritten whole.** gdb drops the
  whole section ("duplicate debug_info_offset, ignoring .debug_aranges")
  when two sets name one unit. It drops it too ("does not exist") when a
  set names an offset that is not a unit start (R9). So a dead set can
  be neither split nor left naming a reused range. Nothing refers into
  `.debug_aranges` by offset, so an update rewrites it whole from the
  live units' sets, in full-link order, as it rewrites `.eh_frame_hdr`.
  It has no dead ranges. The section is small, one set per unit with -g
  (measured size not taken here; *estimate* under 100 KB on protoc).
  *Test:* L10 checks `.debug_aranges` equals the full link's up to the
  placement mapping (IN-2 item 5), and gdb's transcript has no "ignoring
  .debug_aranges" line.
- **IN-55 [v1] Name indexes fall back.** `.debug_pubnames`,
  `.debug_pubtypes`, `.debug_gnu_pubnames`, `.debug_gnu_pubtypes` and
  `.debug_names` are enumerated by name, so a dead contribution's entries
  would find the dead unit. A changed or removed file with any of these
  falls back (`debug-name-index`) in v1. g++ `-gpubnames` emits
  `.debug_pubnames`/`.debug_pubtypes`, clang `-gpubnames` emits
  `.debug_names`, and `-ggnu-pubnames` and **every `-gsplit-dwarf`
  object** emit `.debug_gnu_pubnames`/`.debug_gnu_pubtypes` (R10, both
  compilers). [later] Neutralize the sets (a header with no entries,
  padded), once transcripts show it equivalent. Neither hod nor the
  bench's GCC and Clang defaults emit them (§8.3). *Test:* R10's objects
  in L10: each edit of one must report `debug-name-index`.
- **IN-55a [v1] Detecting unit kinds** (`debug-unit-kind`, IN-51), from
  section names and unit headers alone (R10, g++ and clang++). mortar
  links all of these today: verify clean, gdb and lldb correct.
  - type units, DWARF 4: a `.debug_types` section;
  - type units, DWARF 5: a `.debug_info` unit of type `DW_UT_type`
    (0x02) or `DW_UT_split_type` (0x06), in COMDAT groups;
  - split DWARF, DWARF 5: a unit of type `DW_UT_skeleton` (0x04);
  - split DWARF, DWARF 4: the header cannot tell (a plain compile unit
    with `DW_AT_GNU_dwo_name`). Such objects always carry
    `.debug_gnu_pubnames` (above), so IN-55 catches them first. A
    DWARF 4 object with `.debug_addr` is also treated as split.
  *Test:* R10's 18 configurations in L10, each reporting its reason.
- **IN-56 [v1] Strings.** `.debug_str` and `.debug_line_str` are merged
  sections: new strings are appended (S-110), dead ones stay. A neutralized
  unit names no string. *Test:* an L10 step that adds and removes a
  string literal checks with plumb that the new string's offset is past
  the old content (appended) and that no live `DW_FORM_strp` names a dead
  fragment (IN-3a check 1).
- **IN-57 [v1] Compaction.** When the dead bytes **summed over all debug
  sections** exceed a quarter of their summed size, or a contribution
  fits nowhere (IN-50), the update falls back (`debug-compaction`): the
  full link writes the debug sections without dead ranges. Measured by
  the M2 model (`corpus2/debug_tail_reuse.py`: the real protoc `-g` map,
  every contribution in its size class, IN-50's three placements, new
  sizes drawn from the corpus's 745 drift samples, 500 consecutive
  updates of one file), the updates of one file before compaction:

  | debug tail | largest file (1.50 MB of debug) | p99 (748 KB) | p90 (403 KB) | p50 (214 KB) |
  |---|---|---|---|---|
  | `size/8` (3.98 MB) | ≥ 500 | ≥ 500 | ≥ 500 | ≥ 500 |
  | `size/16` (1.99 MB) | 227 | ≥ 500 | ≥ 500 | ≥ 500 |
  | **`size/32` (0.99 MB), IN-33** | 227 | 297 | ≥ 500 | ≥ 500 |
  | none | 88 | 18 | 19 | 121 |

  Reuse of dead ranges (IN-50 step 2) changed none of these numbers: in
  a sequence of edits to one file the old allotment is the size class
  below the new one and never fits it. It is kept because it serves
  edits that alternate between files or that shrink and grow again (not
  measured here). The result is sensitive to drift: with synthetic drift
  uniform in [1.00, 1.02] per update and tails of `size/8`, the four
  files reach compaction after 10, 19, 45 and 88 updates; in [1.00,
  1.05], after 3, 7, 20 and 34; in [0.95, 1.05] (no net growth), ≥ 500
  for all (`results/drift-*.tsv`). So a long run of growing edits
  compacts every few dozen updates whatever the tail, which IN-57's fall
  back makes a cost, not a failure. [later] A debug-only rewrite: the
  allocated part of the output is untouched (no address depends on a
  non-allocated section), and only the non-allocated sections are
  rewritten compactly (the debug part of the image phase, ~31 of 45 ms of
  image on protoc `-g`: the phase with `-g` less without, §1.5, *estimate*). *Test:* L10 runs
  a growing sequence to compaction and checks the reason; the model is
  re-run on S5's real sizes.
- **IN-58 [v1] Debug options.** `--compress-debug-sections` other than
  `none`, `--gdb-index` and `--debug-names` (generated indexes) are not
  supported in incremental mode (IN-12): a compressed section cannot be
  updated by window, and a generated index is a function of every unit.
  `--strip-debug`/`-S` is supported (no debug sections at all). *Test:*
  as IN-12, per option; an L10 project linked with `-S` updates.
- **IN-59 [v1] What "the same debug information" means.** For IN-2,
  debug information is compared by the debuggers' view: the transcripts
  of fixed gdb and lldb sessions (R6s commands: break by name and line,
  run, backtrace, `info line`/`source list`, `print`/`frame variable`,
  `ptype`/`image lookup -t`, step, `info functions`/`image lookup -n`,
  `info variables`/`target variable`, `info scope`), with addresses and
  in-section offsets masked, equal for the update and a full link.
  *Test:* L10 runs them on its `-g` projects at every step on hosts with
  gdb and lldb.

Split DWARF (`-gsplit-dwarf`): the linker then sees only skeleton units
and `.debug_addr`, `.debug_line`, `.debug_rnglists`, and
`.debug_gnu_pubnames`/`pubtypes` (R10); the `.dwo` files are not its
business. That shrinks the debug bytes an update refills and the debug
reserve by most of `.debug_info` (not measured here: the bench has no
split-DWARF build). hod supports it for DWARF 5 (`hod/spec/dwarf.md`).
v1 falls back on it twice over (IN-55's name index, IN-55a's skeleton
units); S5's follow-up tests skeleton neutralization and the GNU name
index sets first.

## 7. Fall-backs

Each is a full link in incremental mode (its output ready for the next
update), reported as `incremental: full (<reason>)` (IN-10). With
`--incremental=update` each is an error instead.

| reason | when | v1 or later |
|---|---|---|
| `no-state` | no state at the state path (the first link); reported only by `--incremental-report` | permanent |
| `state-corrupt` | the state fails validation (IN-18) | permanent |
| `state-version` | the state's format or mortar version differs | permanent |
| `options-changed` | the normalized plan differs: any option, the target, the emulation, the library search path, the input list's order (adding or removing an input, or an archive's members changing, is not a change of plan) | permanent; [later] options that touch no layout (`-Map`, `--time-phases`, `--threads`, `--incremental-report` are excluded already) |
| `output-changed` | `P` is missing, or its stat tuple or build id differs from the state's, or it is racy and a build-id leaf differs (IN-21) | permanent |
| `output-torn` | the state's "updating" flag is set (IN-46) | permanent |
| `inputs-reordered` | two unchanged inputs' relative order changed | v1; [later] when no COMDAT keeper or placement order depends on it |
| `too-much-changed` | IN-16's threshold | permanent (it is the compaction rule) |
| `reserve-exhausted` | a segment reserve, a packed array's, `.eh_frame`'s, a slot region's or a dynamic table's capacity is exhausted (IN-32, IN-34) | permanent |
| `island-exhausted` | a new thunk or patch does not fit its island | permanent |
| `tls-layout-changed` | any TLS input section's TP offset would change, in any file (IN-34) | v1; [later] refill every TP-relative referrer of every moved TLS section through the index |
| `copy-size-changed` | a copy relocation's object changed size in its shared library (IN-34, IN-40) | v1 |
| `new-output-section` | an output section or segment appears or disappears | v1; [later] when it can go at the end of its segment's reserve |
| `property-changed` | `.note.gnu.property` changes size (S-114) | v1 |
| `dynamic-changed` | `DT_NEEDED`'s order changes or `.dynamic`'s spares run out (IN-42) | v1 |
| `debug-compaction` | IN-57 | v1; [later] the debug-only rewrite |
| `debug-name-index` | IN-55 | v1; [later] neutralized sets |
| `debug-unit-kind` | a changed file has type units or split-DWARF skeletons (IN-55a) | v1; [later] after S5's tests |

Not fall-backs but plain links (IN-12; a warning, no state written):
`-r`, `--emit-relocs`, a `SECTIONS` script, `--compress-debug-sections`
other than `none`, `--gdb-index`, `--debug-names`, `--icf`. The copy
path is the default (IN-45); with `--incremental-write=in-place` these
take it instead of a fall-back: `ETXTBSY`, a process that maps or holds
the output, a hard-linked or symbolic-link output, a shared object, an
output that is an input, a file system without enforced locks. Racy
inputs and a racy output are hashed, never a fall-back (IN-21, IN-22). *Test:* each row has an L10
scenario (IN-4).

## 8. Costs, the verifier, and the other projects

### 8.1 What a plain link pays

The operator's rule (2026-11-15): a link that is not in incremental
mode pays nothing for incremental mode. Per mechanism:

| mechanism | plain link | incremental-mode full link (first link, fall-back) | update |
|---|---|---|---|
| slack and reserves (IN-33) | none: capacity = size, the same code with a zero slack function | +5.02 MB (+10.0%) output on protoc `-g`, +2.37 MB (+20.1%) without (`regions2.py`); written as fill | consumed; dead ranges |
| content hashes of inputs (IN-22) | none (hash 0, `spec/input.md` §11.6, lazy) | every input hashed: 217 MB at 12.6 GB/s is ~17 ms of one thread, spread over the input workers (*estimate*) | changed and racy inputs only |
| stamps (IN-22a) | none | one per file system: 8–10 µs on overlay, 0.3–0.5 ms on virtiofs (R6) | the same, plus the tick wait for the output (IN-21): 0.9–1.0 ms overlay, 0.08–0.12 ms virtiofs |
| the state (IN-17–IN-20) | none: nothing opened, stat'ed or written (IN-11) | 8.05 MB written in 7.0 ms (R12); rows built from data the phases already hold (IN-20) | read and validated in 3.1 ms, rewritten in ~7 ms |
| section content hashes (IN-31, [v2]) | none | 0.44 MB of state; 13–33 ms of CPU, 2–5 ms of wall time on 8 workers (*estimate*, R12) | changed files' sections only |
| the dependency index (IN-25) | none | emitted by the scan's workers, sorted per unit: 238,919 alloc relocations become 118,145 rows (a few ms, *estimate*) | changed files' rows replaced |
| armaps (IN-19) | none | 0.53 MB stored for protoc's three large archives | read from the state (~1 ms) instead of re-parsed (~10 ms, R12) |
| ids (IN-28) | none: ids are per link in every mode | none | identity translation on load |
| regions (IN-30) | none: layout is unchanged | a region per (file, hole-tolerant output section, category): 2,936 on protoc | as the delta needs |
| debug size classes (IN-50) | none | +1.45 MB of rounding on protoc `-g` (inside the +10.0% above) | in place within the class |
| debug neutralization (IN-51) | none | none | the dead units' bytes |
| `fdatasync` (IN-46, IN-47) | none (image.md I-61) | none (a full link renames a complete file) | in place: about 8.2 ms on overlay, 2.3 ms on virtiofs; copy path: 27.0 / 17.8 ms for copy, sync and rename at 50 MB (R7) |

"Pays nothing" is a gate, not a hope: the bench (`spec/harness.md`
"bench") runs plain protoc links built with and without `mortar_incr`'s
code paths compiled in (S1 adds a cargo feature for the comparison only)
and flags a difference above its noise band. An incremental-mode full
link is allowed to cost more than a plain one, by at most 10% of wall
time and of peak RSS on protoc (*target*, S1 measures it).

### 8.2 The verifier and L10

- **IN-60 [v1] The verifier needs no state.** `mortar_plumb` checks an
  update's output from the output and the map alone, as for any link
  (`docs/VALIDATION.md` "independent verifier"). The map gains two line
  kinds (plumb.md §2.3): `hole <output section> <offset> <size>` and
  `dead <output section> <offset> <size> <kind>`. plumb checks that every
  byte of an allocated output section is covered exactly once by a
  `place`, `piece`, synthetic chunk, `hole` or `dead` line, and runs
  IN-3a's three checks on every `hole` and `dead` range. *Test:* plumb's
  mutation tests (plumb.md §9.2) gain: a live relocation aimed into a
  hole; a live unit's `DW_AT_stmt_list` aimed at a dead `.debug_line`
  range; a stale code byte in a code hole; a dead `.debug_info` unit not
  neutralized; a 12-byte CIE and a zero word in an `.eh_frame` hole
  (R5's negative rows); a hole in `.init` and a zero slot in a
  `__start_` array (R4). Each must be rejected.
- **IN-61 [v1] The replay hook.** A test-only entry (an environment
  variable read by the test build, not an option; model §7.3) runs a
  full link whose decision tables come from a state file; the harness
  compares its output with the update's byte for byte outside the dead
  ranges, which it compares by IN-3a's checks (IN-1 B). *Test:* every L10
  step runs it; a mutation that changes one decision-table entry in the
  state must make the comparison fail.
- **IN-62 [v1] The equivalence check.** `mortar_plumb --equiv X.map
  F.map X F` checks IN-2's items 1–6 between an update `X` and a full
  link `F` of the same inputs (§3). It lives in `mortar_plumb` because it
  is a verifier of the incremental linker's claim and must not share its
  code. *Test:* R13's two cases (accept a re-link with other addresses
  in the same order; reject "b a") are its first tests; the relaxation
  list of item 4 gets one positive and one negative case per target.
- **IN-63 [v1] L10** (`docs/VALIDATION.md`): random edit sequences.
  - *Projects*: R2's 40-unit corpus (thin and regular archives, add and
    remove a unit, extraction changes); a generated program of 200 units
    over 3 archives; protoc's object set with edits applied by
    recompiling one source of the bench's tree; the go/no-go workload of
    §1.5; each with GCC, Clang and kiln+hod, `-O0` and `-O2` (kiln: `-O0`),
    `-g` and none, `-ffunction-sections` and none, static, PIE and (M4)
    shared.
  - *Edits*: R2's edits e1–e10 and the rows of Table 1, plus: move a
    definition between files, change a COMDAT keeper both ways, kill and
    revive a section under `--gc-sections`, grow and shrink TLS, change a
    shared library's copy-relocated object, reorder an archive, `touch`
    without change, a same-size rewrite within one timestamp tick
    (`clock_race.py`, R6), the output rewritten within its own tick
    (`state_tick.py`, R6), add and remove an input, flip `--gc-sections`
    (an option change). Sequences of 20–50 steps from a recorded seed; a
    failing sequence is minimized (delta debugging over steps) and its
    seed kept as a regression.
  - *After each step*: IN-1's three checks; **IN-63a**; the program run
    with its expected output; at `-g`, IN-59's transcripts; the
    `--incremental-report` line recorded, so the mix of updates and
    fall-backs is a statistic (and a sequence that only ever falls back
    is flagged).
  - *Faults*: the crash hook of IN-46 and IN-47 at each step; a process
    running the output through `ld.so` during an in-place update (R7); a
    corrupted state; a state on another file system (IN-22a).
  - *Cost* (R3's oracle run time, and §1.5's full-link times): one step
    costs two full links (the base is the previous step's), the update,
    the oracle and the equivalence check. On small programs (R3's, about
    390 input files with libc members) a full link is tens of ms, one
    oracle run 0.07–0.13 s and `equiv.py` 0.04–0.05 s and 18 MB (Python
    prototypes; R3's 32 pairs and 8 mutations, 288 oracle runs, took
    28 s). On protoc a full link is 0.2 s and the oracle reads two 50 MB
    outputs. *Estimate*: a pre-merge smoke of one 20-step sequence per
    small project and compiler at `-O0` (3 compilers × 2 projects) is
    about 120 updates, 240 full links and 120 oracle runs, ~1–2 minutes
    of one core plus compiles (a corpus edit recompiles one unit, ~0.2–1
    s); the nightly lane at full size (every configuration, 50-step
    sequences, protoc included) is 30–60 core-minutes. WP-067 measures
    the smoke's real cost; the pre-merge budget is 5 minutes of wall
    time on the jobserver pool, else the smoke shrinks. *Test:* the lane's own
    check: a seeded wrong refill (IN-63b) is found and minimized to at
    most 3 steps.
- **IN-63a [v1] The two-full-link oracle.** For every incremental test
  step: link the step's inputs fully twice (the previous inputs and the
  new ones, both with the update's options); for every unit of an
  **unchanged** file whose placement is the same in both, compare its
  bytes; each unit whose bytes differ must be in the refill set that
  `--incremental-report` (or WP-063's dry run) predicted, and each unit
  of an unchanged file that moved must be named as moved; bytes that
  differ outside every unit are named by output section and must belong
  to a synthetic chunk the update rewrote. It needs no state and no
  incremental code: it is the reviewer's experiment 3 made a test, and
  it is what keeps IN-27's table honest as rows are added. *Test:* R3's
  scenarios (body edit, COMDAT keeper both ways, gc kill and revive, TLS
  growth, a debug-only referrer) on GCC `-O0 -g` and `-O2 -g`.
- **IN-63b [v1] Mutation tests of the delta rules.** Each IN-27 row, and
  each of IN-3a's checks, is dropped in turn in a test build; the lane
  records which scenario's oracle (IN-63a) or verifier run fails. A row
  whose removal no scenario catches has no test, and a scenario is added
  before the row is relied on. R3 measured the table below with the
  oracle's prototype as the predictor. *Test:* the lane itself (WP-069).

  | row dropped from the predictor | scenarios whose oracle run fails (R3) |
  |---|---|
  | a file changed | **none**: the oracle checks only unchanged files; this row is tested by IN-62's equivalence and IN-61's replay instead |
  | a global's definition moved or changed | gc kill and revive (`-O0`), the debug-only referrer (`-O0`, `-O2`) |
  | a COMDAT keeper changed | the size-neutral keeper swap, both directions, `-O0` and `-O2` |
  | liveness changed | gc kill and revive, with and without unwind tables, both directions, `-O0` and `-O2` |
  | a section of an unchanged file moved | +16 B body, both keeper scenarios, gc with unwind tables |
  | a synthetic table changed (slots) | +16 B body (`-O0`), the reviewer's keeper script (`-O0`), gc with unwind tables |
  | a merged fragment moved (oracle only) | the size-neutral keeper swap |
  | TLS (`tls-layout-changed`) | **only x86-64 TLS growth at `-O0`**: on AArch64 every TP-offset change R3 produced also moved an address, so the global and moved-section rows caught it first |

  Two gaps follow, and WP-069 closes them: the TLS row needs an AArch64
  scenario where a TP offset changes at unchanged addresses (a `PT_TLS`
  alignment change; R3 could not produce one through layout padding,
  not verified), and the R3 x86-64 programs were linked but not run (no
  x86-64 execution here).

- **IN-64 [v1] The bench** gains incremental rows: for protoc `-g` and
  without, and for the go/no-go workload, the full link plain and in
  incremental mode, and an update after (a) one body edit in
  `descriptor.cc.o` given directly, (b) the same through `libprotobuf.a`
  regular, (c) thin; each on the copy path and in place; wall time, peak
  RSS, output size and state size (§1.5's floor is the target to
  compare). *Test:* the bench's own regression flags (`spec/harness.md`
  "bench"): an update slower than §1.5's go threshold is flagged.

### 8.3 What kiln and hod are asked for

Ranked by value to mortar; none blocks v1, which works on GCC and Clang
objects as they are. The standalone texts, ready to send, are in the
record (`spec/landed/incr/WP-060.md`, (c)).

1. **kiln's driver: thin archives (or no archives) in its development
   profile**, so that an edit to one source reaches the linker as one
   changed file, not a changed 85–117 MB archive: a deterministic regular
   archive hides a same-size member edit from everything but a read of
   every member (§1.3), 50–70 ms of a 107–178 ms update on protoc `-g`
   (§1.5). Cost to kiln: `ar rcsT` instead of `ar rcs` where its build
   examples make archives.
2. **kiln: a switch to omit `DW_LNCT_MD5`, on in the development
   profile.** Measured (§1.1, R2-B, R2-B'): under kiln `-g` a header edit
   changes every includer's object (40 of 40). The objects whose code
   did not change (27 for e5, 6 for e6) differ only in the edited
   header's DWARF 5 MD5 in `.debug_line`. Clang does the same for units
   whose files all have MD5s. GCC 14 emits none. Today kiln's only way
   to drop the MD5 is `-gdwarf-4`. What mortar saves: those objects stop
   being changed inputs. That is 27 of 40 (e5) and 6 of 40 (e6) whole-file
   refills in v1, each the file's allocated units plus its debug
   contributions (p50 0.5 KB + 210 KB on protoc, §1.4). In v2 (IN-31)
   only their `.debug_line` contributions would be refilled, so the
   request is worth most before WP-068. Alternative: emit the MD5 only
   for the primary source file.
3. **hod: tell mortar before emitting name indexes or type units**
   (`.debug_names`, `.debug_pubnames`, `DW_UT_type`): today it emits
   none, and v1 falls back on them (IN-51, IN-55).
4. **kiln's driver: `-gsplit-dwarf` in the development profile**, once
   S5's skeleton tests pass: it moves most debug bytes out of the link.
   Ask kiln to measure its effect on its own test links first.
5. **kiln's driver: pass `--incremental` to mortar in the development
   profile** when mortar is the linker, after §1.5's go/no-go result and
   S6. Not before: the speedup and the fall-back rate must be known
   (record (a)).

**Withdrawn**: the previous draft's request 2 ("hod: keep 'an edit
outside a function does not change the function's bytes' as a stated
guarantee"). No compiler gives it: `__LINE__`, `assert` and
`__builtin_LINE()` change functions below a comment line in every
compiler, kiln included, and so do inlining and assembler-resolved
branches (§1.1). Mortar does not need it: IN-31 compares bytes and
relocations by its own hash and trusts no compiler guarantee.

Not requested, against the architecture's starting position
(`docs/ROADMAP.md` "Cross-project"): per-section content hashes from
hod. File-level change is decided by stat and an L128 hash (IN-22), and
IN-31's section hashes are mortar's own (a compiler's could not be
trusted, and computing them costs 2–5 ms of wall time). No "unchanged"
hint either (IN-23).

## 9. Amendments to other specs

Ranked by the conflict each removes: the first five contradict a rule
this spec needs (the reviewer's M7), the rest add to a spec without
contradicting it. On main, `model.md` §4 already allows `capacity ≥
size` per chunk and plumb's checks already accept gaps between
placements, so no shipped code assumes "no slack"; none of these is
urgent for M2 code.

| rank | spec | amendment | conflict it removes |
|---|---|---|---|
| 1 | `layout.md` L-142 | text A below | L-142 lists `.debug_*` and `.eh_frame` as hole-free; this spec gives them dead ranges that are well-formed records (IN-3a, IN-34a, §6), and requires full-link order (IN-2 item 5) |
| 2 | `layout.md` L-141 | text B | "a percentage per output section, as in gold" vs IN-33's per-region capacities in L-143 sections and size classes in debug sections |
| 3 | `layout.md` L-144 | text C | L-144's growth order (in place → slack of the chunk → relayout the chunk → full relayout) vs IN-32's (region → section tail → segment reserve → `reserve-exhausted`) |
| 4 | `synth.md` S-111 | text D | "`.eh_frame` grows by layout L-144" and "a CIE that becomes unused stays" vs IN-34a's per-file ranges, padding CIEs and rewrite in full-link order |
| 5 | `layout.md` L-147, `synth.md` S-110 | text E | L-147's "every hole is a valid piece" says nothing of the verifier; IN-3a makes dead fragments dead ranges with its three checks |
| 6 | `model.md` §7.3 | invariant (C) and equivalence (IN-1, IN-2); (B) holds modulo the dead ranges of IN-3a, which the decision tables record | — |
| 7 | `plumb.md` §2.3, §9 | the map's `hole` and `dead` lines; plumb's dead-range checker (IN-3a's three checks) and its mutation cases (IN-60); `--equiv` (IN-62) | — |
| 8 | `model.md` §7.2 | IN-25 implements §7.2's per-unit index; it adds the liveness graph and names the non-relocation targets (slots, patches' redirected sites, demand flags) | — |
| 9 | `image.md` I-96 | IN-45's default (copy) and conditions for in place; IN-46's order with four `fdatasync`s; IN-47's sync before rename | — |
| 10 | `model.md` §7.1, `docs/ROADMAP.md` "Cross-project" | drop the request to kiln and hod for per-section content hashes (§8.3); IN-31 computes its own | — |
| 11 | `image.md` I-101 | liveness walks the stored graph, not the relocations (IN-29) | — |
| 12 | `layout.md` Q4, Q8 | answered: the index records end-symbol users (IN-25); per-region slack plus section and segment reserves (IN-33) | — |
| 13 | `dyn.md` D-28 | stays an [M7 hook]: v1 rewrites the symbolic tables whole (IN-40); fixed `nbuckets` and `.dynsym` capacity are a [later] optimisation | — |
| 14 | `input.md` §11.6 | in incremental mode every input's content hash is computed (IN-22) | — |
| 15 | `cli.md`, `docs/status/options.md` | IN-10's options, `--incremental-write`, `--incremental-sync`; IN-12's warning | — |
| 16 | `diag.md` | keys `incr-full-link` (note), `incremental-unsupported`, `incremental-update-failed`, `incremental-not-reproducible` (warnings and error) | — |
| 17 | `docs/ARCHITECTURE.md` "Incremental linking" | the one correction of §2.7 | — |
| 18 | `docs/VALIDATION.md` L10 | IN-63's content, the two-full-link oracle (IN-63a) | — |
| 19 | `arch.md` "Incremental links" | `island-exhausted` as the fall-back name | — |

**Text A (L-142, replaces the last paragraph).** "Each is one chunk
whose contributions keep the full link's order. Its slack is only its
capacity beyond `size`, which is beyond `sh_size` (L-86, L-141); an edit
inside it rewrites the whole chunk in the full link's order, within its
capacity. Two packed sections admit dead ranges that are records of
their own grammar rather than holes: `.eh_frame`, whose per-file ranges
may end in a padding CIE (`spec/incr.md` IN-34a), and the `.debug_*`
sections, whose dead contributions are neutral units (IN-50–IN-53). Both
are dead ranges in the sense of IN-3a, recorded in the decision tables
and checked by the verifier. No other packed section has a dead range.
Growing such a section changes its end symbol's value, and the
dependency index records the symbol's users (`spec/incr.md` IN-25)."

**Text B (L-141, replaces "Its reserve is a percentage … open (Q8).").**
"Its capacities are `spec/incr.md` IN-33's: per (file, output section,
category) region in the hole-tolerant sections of L-143, a tail per
output section, a reserve per `PT_LOAD`, `size/8` past each packed
chunk, and a size class per debug contribution. Q8 is answered there."

**Text C (L-144, replaces the list).** "When a contribution changes
size, the incremental link tries, in order: (1) in place, within its
region's capacity (a shrink leaves a hole, L-143 sections only); (2) the
output section's tail reserve, the old region becoming a hole; (3) a
growth step of the output section into its `PT_LOAD`'s reserve, moving
the sections after it in the segment and refilling their units and
referrers; (4) a full link with the reason `reserve-exhausted`. A packed
chunk (L-142) skips (1) and (2): it is rewritten whole within its
capacity, then (3), then (4). Each step yields a valid layout or reports
why it gave up (`spec/incr.md` IN-32)."

**Text D (S-111, replaces "When it grows … no FDE uses.").** "In
incremental mode `.eh_frame` is grouped in per-file ranges in S-46's
order, CIEs are deduplicated within a file only, and a range's unused
tail is 0 bytes, absorbed `DW_CFA_nop` padding of the range's last
record (4, 8 or 12 bytes), or one padding CIE of at least 16 bytes
(`spec/incr.md` IN-34a, with its bytes). A range that outgrows its
capacity makes the whole section be rewritten in full-link order. The
terminator is always the section's last word. `.eh_frame_hdr` is one
unit, refilled whenever an FDE's address changes (a sort of 10^6
entries, about 10 ms)."

**Text E (L-147 and S-110, appended).** "An unused piece is a dead
range (`spec/incr.md` IN-3a): recorded in the decision tables, never
the target of a live reference, and exempt from fill, because a live
reference may share its suffix."

## 10. The work-package cut

M7's stages; each is one package with its own tests, and L10 grows with
them. WP-061 comes first and ends in §1.5's go/no-go decision; WP-062
and WP-067 may start beside it (they are useful whatever the decision:
plumb's dead-range checker and L10's oracle check plain links too).
WP-063 after WP-061; WP-064 after WP-061–WP-063 and a "go"; WP-065 and
WP-066 in parallel after WP-064; WP-068 when §1.5's rule triggers it,
else after WP-065. Estimates are *estimates*: lines of Rust including
tests, and implementer sessions, one session being one package of the
size M2's took: six merged M2 packages (WP-043, WP-044, WP-046, WP-047,
WP-049, WP-050) added 1,318–6,055 lines of Rust each, median ~2,800
(`git diff --shortstat` of their merges). The sizes here are judged
against those, not measured.

| WP | stage | content | depends on | parallel with | gate | estimate |
|---|---|---|---|---|---|---|
| WP-061 | S1 state + measured floor | `mortar_incr`: the state's format (IN-17–IN-20, armaps, IN-19), writer in an incremental-mode full link, reader with validation and a fuzz target, stamps and the clean / racy / hashed rule (IN-21, IN-22, IN-22a), the identity translation, the replay hook (IN-61); then the **floor measurement**: a test-only update path for a same-size body edit (the layout is the previous one, so no delta layout is needed) that does every step of §1.5's emulation with mortar's code, through the image phase and IN-46/IN-47's writes, on protoc `-g` and a workload ≥ 5× larger (added to the bench) | amendments 6, 8 | WP-062, WP-067 | state round-trips on the link-test corpus; replay (B) byte-identical for every first link; plain-link bench unchanged (§8.1); **the go/no-go numbers of §1.5, reported to the operator** | 5,000–7,000 lines, 2–3 sessions |
| WP-062 | S2 slack and dead ranges | regions in L-143 sections, packed chunks with capacity, tails and segment reserves (IN-30, IN-32, IN-33), `.eh_frame` ranges and padding CIEs (IN-34a), debug size classes (IN-50), holes and their fill; map `hole`/`dead` lines; plumb's dead-range checker (IN-3a) | amendments 1–5, 7 | WP-061, WP-067 | incremental-mode first links pass plumb and are equivalent (IN-2) to plain links; output sizes against IN-33's table; R4 and R5 as plumb negative cases | 3,000–4,500 lines, 1–2 sessions |
| WP-063 | S3 delta | change detection (IN-22–IN-24, parallel member hashing), delta resolution and the refill set (IN-25–IN-29), TLS and copy-size detectors, a dry-run `--incremental-report`; the **two-full-link oracle** (IN-63a) as a test lane | WP-061 | WP-062 | the dry run's refill set covers every unit the oracle finds differing, on every L10 step and R3s scenarios | 3,500–5,000 lines, 2 sessions |
| WP-064 | S4 update | delta layout, refill and writing (IN-13, IN-14, IN-44–IN-49a) for static executables, `-g` falling back; copy path by default, in place on request; crash hooks; `plumb --equiv` (IN-62) | WP-061–WP-063, "go" | WP-067 | L10 static, no `-g`: IN-1 on every step; crash injection at each IN-46/IN-47 step; floor against WP-061's | 5,000–7,000 lines, 2–3 sessions |
| WP-065 | S5 debug | IN-50–IN-59: size classes in use, reuse, neutralization of every debug section (IN-51, IN-51b), compaction fall-back; type units and skeletons tested before enabling | WP-064 | WP-066 | L10 with `-g`: transcripts equal (IN-59), `--verify` clean, readelf warning-free | 3,000–4,000 lines, 1–2 sessions |
| WP-066 | S6 dynamic | PIE and dynamic executables (IN-40–IN-42), shared objects by copy (IN-43, M4), `copy-size-changed` | WP-064, M2 (M4 for shared objects) | WP-065 | L10 PIE and shared | 2,500–4,000 lines, 1–2 sessions |
| WP-067 | L10 | the generator over the corpus (R2), the projects, the minimizer, the fault hooks, nightly and pre-merge lanes, the bench rows (IN-63, IN-64) | — (every step a fall-back until WP-064) | all | lanes run; a seeded failure is minimized; the lane's cost within IN-63's budget | 2,500–3,500 lines (Rust and scripts), 1–2 sessions |
| WP-068 | v2 section-level refill (M12) | IN-31: per-section content hashes under `--incremental`, matching by anchor, kept sections in place, layout around them; plain-link gate | WP-064 (WP-065 if after it) | WP-066 | L10 with `-ffunction-sections`, no `-g`: refilled bytes against the coarseness table (§1.2); plain-link bench unchanged | 2,000–3,000 lines, 1 session |
| WP-069 | mutation tests | the delta rules' mutation lane (IN-63b): each IN-27 row and each IN-3a check dropped in a test build, and the L10 scenario that must catch it; plumb's mutation cases (IN-60); the two gaps R3 left (an AArch64 TLS scenario with a TP offset change at unchanged addresses; x86-64 scenarios run, not only linked) | WP-063, WP-067 | WP-064 | every row and check caught by at least one scenario, else a scenario is added | 1,000–1,500 lines, 1 session |

Total *estimate*: 27,500–38,500 lines, 12–17 sessions, with WP-061's
go/no-go after the first 2–3. Not cut yet, after M7: the hole free list
for allocated sections, debug-only compaction, name-index neutralization,
D-28's bucket capacity, scrubbing (IN-3b), smaller update windows (§1.5),
per-file `.symtab` ranges. Each has its condition in its rule.

## 11. Open questions

Each with the default this draft takes. The operator's decisions (the
write path, section-level refill's place, the slack overhead, kiln's
default) are in the record, not here.

1. **Is the fall-back note printed by default?** Default: yes, one line
   (`incr-full-link`), because silent fall-backs were gold's users'
   complaint (§2.2); successful updates print nothing. Build systems
   that treat stderr as failure would need `--no-warn`-style silencing:
   use the diagnostic key's level control (`spec/diag.md`).
2. **`ETXTBSY` on newer kernels.** Measured on 6.8 here: writing an
   executable started by `exec` fails, but one started through `ld.so`
   or qemu-user, or loaded by lldb, does not (R7, IN-45). Reports (not
   verified here) say newer kernels dropped the write denial altogether.
   Default: the copy path (IN-45), which does not depend on it; the
   in-place option adds the `/proc` scan and says what it cannot see.
3. **File locks.** The workspace's virtiofs now enforces `flock` (the
   reviewer's m2, `write_paths.py`; the previous draft measured the
   opposite on an earlier mount), so the in-place option is available
   there; IN-45 still probes each time (image.md I-59). Default: probe.
4. **L128 is not a cryptographic hash.** It is consulted only when stat
   says "maybe changed" or a file is racy, and an accidental collision of
   128-bit hashes is not a practical concern; a deliberate one requires
   an attacker who writes the build's inputs, who could write the program
   anyway. Default: L128; SHA-256 would cost 750 ms on protoc's inputs
   (287 MB/s, image.md I-74).
5. **The slack fractions** (IN-33): region `size/8` (64 B minimum),
   section tail `size/16`, segment `filesz/8`, packed `size/8`, debug
   tail `size/32` plus size classes. The debug numbers come from the M2
   model (IN-57); the allocated ones have no measured fall-back rate yet.
   Default: these; WP-064 measures the rate of `reserve-exhausted` on
   L10 and tunes them.
6. **Regular archives.** Default: read the changed archive and hash its
   members in parallel (IN-24); the cheaper header rule is unsound with
   deterministic `ar`. Ask kiln for thin archives (§8.3). Whether
   CMake-style regular archives are common enough in kiln's users' builds
   to make more work here worthwhile is WP-061's measured floor to answer.
7. **Where the state lives.** Default: beside the output. A build
   directory with outputs on a slow share might prefer the state on fast
   local disk; `--incremental-state` allows it, with stamps on both file
   systems (IN-22a).
8. **Debug compaction in v1** is a full link (`debug-compaction`): a run
   of growing edits compacts every few dozen updates (IN-57). Default:
   accept for v1; the debug-only rewrite is the first [later] item if
   L10 shows it frequent.
9. **Equivalence of `.symtab` order.** A full link orders `.symtab` by
   file; the update rewrites it whole in the same order (S-112, IN-49a),
   so `.symtab` is identical modulo values. No question, recorded because
   `nm` users compare it.
10. **Power loss.** IN-46/IN-47's sync order is argued, not tested (no
    `dm-flakey` here). Default: argue it in the spec and test it where a
    host provides `dm-flakey` or a fault-injecting file system (WP-064's
    gate, if available; not a blocker).

## 12. Appendix: experiments

Scripts are in the workspace's `.build/review-incr/` (not committed, as
the brief asks; they need the workspace's bench cache and toolchains).
`run-all.sh WORK MORTAR` runs them all into `WORK`; each is also
runnable alone. Tool versions: GCC 14.2.0, Clang/LLD/lldb/llvm-dwarfdump
19.1.7, GNU ld 2.44, readelf 2.44, mold 2.37.1, gdb 16.3, kiln 1.2.1 at
b6c546bd6, hod at 7e30d7c8, Python 3; mortar a release build of this
branch (main `2a31ada` plus spec text). Host: aarch64, 12 cores, Linux
6.8, container on an overlay file system (the workspace on virtiofs).
The first draft's experiments ran at load 19–30. The revision's ran at
the loads given per row; where a row has no timing, its load does not
matter. The reviewer's scripts were copied from `.build/review-incr-adv/`
unchanged; edited copies have new names.

The first draft's experiments (still cited where they hold):

| id | what | script | output | load |
|---|---|---|---|---|
| E1 | the first census: 15 edit classes × 2 units × 20 configurations (superseded by R2) | `census.py WORK` (`run-census.sh` sets up kiln+hod) using `objdiff.py` | `census.tsv` | 20–30 |
| E2 | full-link phase times, input read and `stat` costs | `run-all.sh` part 2 | §1.3 (`stat` 4.3 µs), §1.5 (superseded by R1) | 21–29 |
| E3 | the shape of the protoc link | `linkstats.py MAP` | §1.4 | 19–30 |
| E4 | the state's size, derived (superseded by R12) | — | — | — |
| E5 | writing: copy, reflink, positioned writes, `fdatasync`, rename, `ETXTBSY` | `write_cost.py WORK EXE` | IN-45, IN-47 (1 GiB copies) | 19–30 |
| E6, E6b | dead compile units: tombstoned vs neutralized, gdb and lldb transcripts | `debug_dead_cu.sh WORK MORTAR` with `dwarf_pad.py` | §1.6 | 19–30 |

The revision's experiments, numbered as the reviewer's list (its §6):

| id | what | script | output | load |
|---|---|---|---|---|
| R1 | baseline at main `2a31ada` (8 runs each), and the floor as a timed emulation: real I/O of every update step, CPU parts derived from `--time-phases` | `baseline.sh WORK MORTAR`; `floor.py WORK MAP OUTPUT RESPONSE CWD TP_FILE --fs DIR --runs 5` | §1.5 | 5–6 (baseline), 9–11 (floor) |
| R2 | the 40-unit corpus: e1–e10, 20 configurations, 3,440 compiles (the reviewer's helper), plus 1,369 compiles and 54 mortar-linked programs | reviewer's `corpus/` (`gen.py`, `drive.py`, `summarize.py`, `link.py`, `unity.py`, `pch.py`); `corpus2/drive2.py B`, `corpus2/link2.py B` | §1.1 Table 1, §1.2 coarseness table | ~25 (reviewer), 1–2 (corpus2) |
| R2-A | debug size drift of 745 changed `-g` objects | `corpus2/drift.py B drift.tsv` | §1.1, IN-57 | 1–2 |
| R2-B | kiln `-g` header edits per object, line-table dumps | `corpus2/hdredit.py B` | §1.1 | 1–2 |
| R2-B' | MD5 mechanism: Clang, kiln, GCC, Clang `-gdwarf-4` on a same-line header edit | `corpus2/md5_check.sh WORK` | §1.1 | ~1 |
| R2-C | add/remove a unit, archive member (regular and thin), extraction | `corpus2/link2.py B` | §1.1, §1.3, IN-24 | 1–2 |
| R3 | the two-full-link oracle over IN-27's rows, and its mutation table | `oracle_scen.sh WORK` (builds the scenarios from copies of the reviewer's `comdat_keeper.sh` and `tls_shift.sh`), `oracle_run.sh WORK` (all pairs and the mutation table), `oracle.py BASE VARIANT [--drop ROW]` | IN-27, IN-34, IN-63a, IN-63b | not timed beyond the oracle's own run time |
| R4 | holes in packed sections run under the program | `packed_holes.sh WORK MORTAR` (reviewer's) | IN-30, `results/e4.out` | not timed |
| R5 | `.eh_frame` hole shapes under libgcc, gdb, lldb, readelf, llvm-dwarfdump | `eh_holes.sh WORK MORTAR` with `ehpatch.py` (modes `cie`, `absorb4`, `min16`, `min12`, …) | IN-34a, `results/e5.out` | not timed |
| R6 | racy timestamps: the stamp rule against the old rule, 2,000 trials, overlay and virtiofs; the output rewritten within its own tick | `stamp_rule.py INPUT_DIR STATE_DIR 2000`; reviewer's `clock_race.py`, `state_tick.py` | IN-21, IN-22, IN-22a | not timed (stamp costs are medians) |
| R7 | write safety: `ETXTBSY` and `F_SETLEASE` under `exec`, `ld.so`, qemu-user, gdb, lldb; `/proc` scan cost; debugger views after copy; kill at each IN-46/IN-47 step; sync costs | `write_safety.py WORK --alt DIR`; reviewer's `write_paths.py` | IN-45, IN-46, IN-47, `results/out14.txt`, `out345.txt`, `out45.txt` | 5–11; reviewer ~20 |
| R8 | debug tail model: size classes, reuse, sum limit, 500 updates, corpus and synthetic drift | `corpus2/debug_tail_reuse.py MAP drift.tsv 500` | IN-33, IN-57, `results/drift-*.tsv` | not timed |
| — | slack model of IN-33 on the real protoc map and output | `regions2.py MAP ELF [DEBUG_TAIL_FRAC]` | IN-33, §8.1 | not timed |
| R9 | neutralization at depth (20 dead versions): pad and block forms; neutral forms of every debug section, spliced and appended | `debug_chain.sh WORK 20 MORTAR` with `dwarf_pad.py --form=block`; `neutral_chain.sh WORK dead`, `neutral_chain.sh WORK append` with `dwarf_neutral.py` | IN-51, IN-51b, IN-54 | not timed |
| R10 | detection of type units, split DWARF and name indexes (18 configurations) | `dwarf_detect.sh WORK` | IN-55, IN-55a | not timed |
| R11 | PIE `[0, len)` tombstones | not run: mortar refuses `-pie` on this branch | IN-53 | — |
| R12 | the state written as bytes, its write and read times; armap re-read cost; IN-31's hash inputs | `state_emul.py WORK MAP CWD libprotobuf.a libc.a libstdc++.a` | IN-19, IN-31, `results/e12.out` | ~14 |
| R13 | IN-2's equivalence check with and without the order clause | `equiv_run.sh WORK` with `equiv.py OUT_A MAP_A OUT_B MAP_B [--no-order]` | IN-2, IN-62 | not timed |

Incidents during the measurements, recorded for the reviewer: an early
run of E2 overwrote the bench cache's GNU-ld-built
`cxx-medium-g-static/build/protoc-3.21.12.0` (the response file's `-o`
had not been redirected); it was rebuilt by the cache's own GNU ld
command line and runs (`libprotoc 3.21.12`, 50,062,880 bytes), with a
new timestamp. An early build of mortar for the first draft wrote some
release artifacts into the main checkout's cargo target directory before
it was stopped and redirected. None in the revision.
