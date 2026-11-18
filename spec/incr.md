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
  R1), plus the phases the emulation leaves out (≤ 55 ms, upper bound):
  for a one-function edit, **1.3–2.5×** faster than the 0.19 s full link
  by copy with thin archives, 1.5–3.5× in place, 0.9–1.5× with a
  regular archive (*estimate*). Dirty 1 MiB windows, not the edit, set
  the refill. The **scaling model** (§1.5) projects 4.0–4.5× in place
  and 2.3–2.5× by copy from 500 MB to 10 GB of output (*estimate*): the
  O(symbols) and, on the copy path, O(output) terms bound the ratio, and
  §1.5 lists what would make each O(change). WP-061 measures it with
  mortar's code on protoc `-g` and a ≥ 10× static `-g` workload, and its
  **go/no-go rule** (thresholds an operator decision) decides whether M7
  continues.
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
| symbol attributes | not in the corpus: a global's type, binding or visibility changes (e.g. FUNC → IFUNC) | — | IN-26: `symbol-kind-changed` (a full link) when an unchanged file references it; frequency not measured |
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

**The omitted phases (the re-review's N6).** "Derived" above charges the
resolve phase whole, the input phase by changed bytes and the image
phase by refilled bytes. It leaves out CPU work an update also does:
layout 21.6 ms (delta layout, region fitting, TP offsets, `.eh_frame`
ranges), split 12.9 ms (the changed file's merged fragments against the
stored maps), assignment 3.8, synthetic 3.2, the scan of the refill set
(≤ 4.5) and the liveness walk (≤ 1.1), all measured as whole phases of
the full link; IN-49a's `.symtab` merge (2–5 ms, *estimate*); decoding
the state and translating identities (IN-28; only the read and its CRC
are timed: ≤ 5 ms, *estimate*); the `.eh_frame_hdr` and
`.debug_aranges` rewrites (their windows are in the dirty set, so their
writes are timed; building them is not, < 1 ms, *estimate*). Charged as
whole phases, that is **≤ 55 ms** (47.1 measured + ≤ 8 estimated), an
upper bound: a delta layout and a split of one file should cost less,
but that is not measured. With it, a one-function edit on protoc `-g`
(full link 191 ms), as an interval *floor (measured) – floor + 55 ms
(upper bound)*:

| protoc `-g`, overlay / virtiofs | thin, in place | thin, copy | regular, in place | regular, copy |
|---|---|---|---|---|
| median `importer.cc.o` | 55–110 / 73–128 ms: 3.5–1.7× / 2.6–1.5× | 90–145 / 78–133 ms: 2.1–1.3× / 2.5–1.4× | 107–162 / 124–179 ms: 1.8–1.2× / 1.5–1.1× | 150–205 / 130–185 ms: 1.3–0.93× / 1.5–1.03× |
| largest `descriptor.cc.o` | 63–118 / 88–143 ms: 3.0–1.6× / 2.2–1.3× | 112–167 / 105–160 ms: 1.7–1.1× / 1.8–1.2× | 132–187 / 169–224 ms: 1.45–1.02× / 1.13–0.85× | 178–233 / 144–199 ms: 1.07–0.82× / 1.33–0.96× |

**What the design promises** for a one-function edit on protoc `-g`
(the median file; the lower ends of the intervals are measured floors,
the upper ends add the 55 ms bound, so the interval as a whole is an
*estimate*):

| write path, archives | promise | the largest file | basis |
|---|---|---|---|
| copy (the default, IN-47), thin archives | **1.3–2.5×** | 1.1–1.8× | floor measured, omitted phases estimated |
| copy, regular archive | **0.9–1.5×**: can be slower than a full link | 0.8–1.3× | the same; detection measured (50–70 ms) |
| in place (IN-46), thin archives | **1.5–3.5×** (up to ~3× typical) | 1.3–3.0× | the same |
| in place, regular archive | 1.1–1.8× | 0.85–1.45× | the same |

The reviewer's figures (copy thin ~1.5–2.5×, regular 1.1–1.5×, in place
up to 3×) are inside these intervals; the lower ends here are lower
because the omitted CPU is charged as whole phases (55 ms, not "some of"
45 ms). On protoc the edit itself (≈ 0.3 MB of units) does not decide
the time: the 1 MiB windows (10–13 MB refilled, 5.4 MB of it
`.symtab`/`.strtab`), the copy (30–40 ms) and regular-archive detection
(50–70 ms) do.

**Scaling model** (the re-review's N6 and the operator's question on the
cost of the delta for very large static executables). Each update cost
is put in one of four classes, with its constant measured on protoc `-g`
(50.3 MB output, 164 MB live input, 31,478 globals, 8.05 MB state,
~170 input files and thin members stat'ed, 2,500 file rows with system
archive members); a full link's phases are classed the same way:

| class | update steps (constant on protoc `-g`) | full-link steps |
|---|---|---|
| O(change): the changed file and the windows its units and referrers touch | read and hash the changed file (3.0 ms); refill reads and relocation of ~9 dirty code/data windows (≈ 9 MB × 1.9 ms/MB ≈ 17 ms: reads 1.0 ms/MB measured, image 0.89 ms/MB derived); delta layout and split (≤ 34.5 ms, upper bound, *estimate*); in-place `fdatasync`s (8.2 ms on overlay, measured); build-id: only dirty leaves rehashed (I-76; 16 B per MiB of root input, negligible) | — |
| O(inputs): per input file and member | `stat` (4.3 µs each; 0.5–0.7 ms); stored armaps instead of re-read (≈ 1 ms) | input phase 87.1 ms (0.53 ms/MB of live input) |
| O(symbols): per global, section and edge, and the sections rewritten whole | state read whole and validated (3.1 ms per 8.05 MB) and rewritten whole (7.0 ms); re-resolution of every key (IN-26: 7.9 ms); assignment, synthetic, liveness walk (≤ 8.1 ms); `.symtab`/`.strtab` merged (2–5 ms, *estimate*) and rewritten whole (5.4 MB); `.debug_aranges` rewritten whole (IN-54: 0.68 MB on protoc); `.eh_frame_hdr` rewritten whole when present (≈ 8 B per FDE, ≤ 0.13 MB, *estimate*); the three sections at 1.9 ms/MB ≈ 11.8 ms | resolve 7.9, assignment 3.8, split 12.9, liveness 1.1, scan 4.5, synthetic 3.2, layout 21.6 ms |
| O(output): every output byte | copy path only: copy, `fdatasync` of the whole new file, rename (30–40 ms per 50 MB, measured: 0.6–0.8 ms/MB) | image phase 44.6 ms (0.89 ms/MB of output) |

`.gdb_index` and `.debug_names` are not generated with `--incremental`
in v1 (§7 `options-changed`), so they add nothing. Summed: an update is
≈ 63 ms of O(change) (with the 34.5 ms bound) plus ≈ 42 ms per protoc
of O(inputs) + O(symbols), plus ≈ 35 ms per protoc of O(output) on the
copy path; a full link is ≈ 191 ms per protoc. **Projection** (*estimate*:
every class linear in the scale S, a C++ code base of protoc's shape,
thin archives, a one-function edit to a median file; S = 10 is the
go/no-go workload's size):

| output with `-g` | S | live input | user objects (+ ~2,300 system members) | globals | state, read and written whole | full link (time, peak RSS) | update in place | update by copy | update peak RSS |
|---|---|---|---|---|---|---|---|---|---|
| 50 MB (protoc, measured) | 1 | 164 MB | 168 | 31 K | 8 MB | 0.19 s, 255 MB | 0.06–0.11 s (1.7–3.5×) | 0.09–0.14 s (1.4–2.1×) | ~55 MB |
| 500 MB | 10 | 1.6 GB | 1,700 | 315 K | 80 MB | 1.9 s, 2.5 GB | 0.48 s (4.0×) | 0.83 s (2.3×) | ~0.33 GB |
| 2 GB | 40 | 6.5 GB | 6,700 | 1.3 M | 320 MB | 7.6 s, 10 GB | 1.7 s (4.4×) | 3.1 s (2.4×) | ~1.3 GB |
| 10 GB | 200 | 33 GB | 34,000 | 6.3 M | 1.6 GB | 38 s, 51 GB | 8.5 s (4.5×) | 15.5 s (2.5×) | ~6.2 GB |

Update peak RSS is *estimated* as 3 × the state's size (decoded tables;
not measured) plus the sections rewritten whole (6.2 MB × S) plus the
refilled windows and the changed file (~25 MB); a full link's as 255 MB
× S (linear in live input, *estimate*). WP-061 measures both (time and
peak RSS, `/usr/bin/time -v`) on protoc and the go/no-go workload. The
full link's first run (0.43 s cold on protoc) is not counted.

**The reference configuration** (the operator's target): a static
executable with debug information always on, **0.5 GB** of output
(typical) and **2 GB** (rare), linked from **1,000 and 3,000 static
archives**, on a file system with clones (IN-47). Assumed, as
*estimates* scaled from protoc: live input 3.26 × the output (1.6 and
6.5 GB); an object of 0.4–1 MB with `-g` (protoc's members are
1.0–1.4 MB); 90% of members extracted; globals 31,478 per 50 MB of
output (315 K and 1.26 M); so 1,800–4,400 members at 0.5 GB and
7,200–17,800 at 2 GB in all, a typical regular archive of 1.8 MB
(0.5 GB, 1,000 archives), 0.6 MB (0.5 GB, 3,000), 7.2 MB (2 GB, 1,000)
or 2.4 MB (2 GB, 3,000). Split DWARF changes the output's size and the
debug terms: the figures here are plain `-g`; §6 "Split DWARF" (R14)
measures protoc both ways. There the full link is 37–44% faster while
the floor falls only 12–37%, so the ratio is lower (thin, copy, median
file: 1.65× GCC, 2.0× Clang, against 2.2× plain, measured floors).

Every per-archive and per-member step, per update (*estimate* from the
measured constants named):

| step | 0.5 GB, 1,000 archives | 0.5 GB, 3,000 | 2 GB, 1,000 | 2 GB, 3,000 | class |
|---|---|---|---|---|---|
| `stat`, regular archives (one per archive, 4.3 µs) | 4.3 ms | 12.9 ms | 4.3 ms | 12.9 ms | O(inputs) |
| `stat`, thin archives (each archive's index and every member) | 12–24 ms | 21–32 ms | 35–81 ms | 44–90 ms | O(inputs) |
| armaps stored in the state (≈ 1.4 rows per global, 12 B a row as R12 measured, names in the pool), read with it | 5.3 MB, 2 ms | 5.3 MB, 2 ms | 21 MB, 8 ms | 21 MB, 8 ms | O(symbols) |
| (armaps re-read from the archives instead, 2.3 ms per MB of armap, R12) | ~100 ms | ~100 ms | ~410 ms | ~410 ms | — |
| one regular archive changed: read and hash all its members (0.7 ms/MB, §1.5 finding 1) | 1.8 MB, 1.3 ms | 0.6 MB, 0.4 ms | 7.2 MB, 5.0 ms | 2.4 MB, 1.7 ms | O(change) |
| one thin member changed: read and hash it | 0.3–0.7 ms | 0.3–0.7 ms | 0.3–0.7 ms | 0.3–0.7 ms | O(change) |
| the state: size, read whole and validated (IN-18) and rewritten whole | 80 MB: 31 + 70 ms | 80 MB | 320 MB: 125 + 280 ms | 320 MB | O(symbols) |

So at this scale a **regular archive costs 0.4–5 ms** of change
detection and a thin one saves at most that, while costing 8–77 ms more
`stat`s on every update: the number of archives is not the problem, the
size of the one that changed is, and here archives are small. Thin
archives pay off only for a few large archives (protoc's 85–117 MB:
50–70 ms saved); §8.3 request 1 is ranked accordingly. No per-archive
step is O(all inputs) with a constant that matters except `stat`
(4–13 ms), which a trusted change list from the build system would make
O(change) (not v1, IN-23).

**The four terms** for the reference configuration, a one-function edit
to a median member of a regular archive, clones available
(*estimate*):

| term | steps | 0.5 GB (S = 10) | 2 GB (S = 40) |
|---|---|---|---|
| per changed file | detect (the changed archive: 0.4–5 ms), refill ~9 MB of windows (17 ms), delta layout and split (≤ 34.5 ms, bound), syncs (8.2 ms) | ~60–65 ms | ~63–68 ms |
| per input file | `stat` of each archive | 4–13 ms | 4–13 ms |
| per output byte | the clone (O(extents), ≤ 10 ms, *estimate*, not measured); build-id: dirty leaves only (I-76); the sections rewritten whole: `.symtab`/`.strtab` (54 / 216 MB), `.debug_aranges` (IN-54: 6.8 / 27 MB), `.eh_frame_hdr` (≤ 1.3 / 5.2 MB) at 1.9 ms/MB; no `.gdb_index` or `.debug_names` (not generated with `--incremental`, §7) | ~130 ms | ~480 ms |
| state, read and written whole, armaps included | 80 / 320 MB | ~100 ms | ~405 ms |
| (CPU over all symbols: re-resolution, assignment, synthetic, liveness walk, `.symtab` merge) | | ~195 ms | ~780 ms |
| **update** | | **~0.5 s** (full link ~1.9 s: **3.8×**) | **~1.75 s** (full link ~7.6 s: **4.3×**) |
| update peak RSS (3 × state + rewritten sections + windows) | | ~0.33 GB (full link ~2.5 GB) | ~1.3 GB (full link ~10 GB) |

On the reference configuration the copy is not a cost (clones); the
update is bounded by the O(symbols) terms, and the order of the table
"What bounds the ratio" below (by cost at 2 GB: `.symtab`/`.strtab`
≈ 610 ms, state ≈ 405 ms, CPU passes ≈ 324 ms, re-resolution ≈ 316 ms,
`.debug_aranges`/`.eh_frame_hdr` ≈ 60 ms, `stat` 4–13 ms) is the order in
which they would be made O(change) after v1 (record (b)).

**What bounds the ratio.** The edit's cost stays constant while every
other term grows with S, so the ratio tends to (full link per S) /
(update per S): **≈ 4.5× in place and ≈ 2.5× by copy** (*estimate*).
On the copy path the O(output) term (0.6–0.8 ms/MB) is the largest
single bound: at 500 MB it is 0.35 s of the 0.83 s. In place removes it;
a reflink copy (`copy_file_range`, which `std::fs::copy` uses, shares
extents on btrfs and XFS: *not verified here*, neither is on this host)
would make it O(change). In place, the bound is the O(symbols) terms
that v1 does whole. Each could be made O(change), in this order of
cost per S (what it would take; none is in v1):

| step (cost per S) | to make it O(change) |
|---|---|
| copy + whole-file sync, 30–40 ms | in place (IN-46, exists: the write-path decision), or a reflink copy where the file system shares extents |
| `.symtab`/`.strtab` merged and rewritten, ≈ 13.5 ms | per-file `.symtab`/`.strtab` ranges with slack, filled like regions (synth.md Q8) |
| state read and rewritten whole, 10.1 ms | a ranged state read (positioned reads of the tables an update needs, no mmap) and a state patched in place or journaled, with per-table hashes kept |
| re-resolution of every key, 7.9 ms | re-resolve only the keys the changed files' summaries name, plus archive lookups through the stored armaps (the stored resolution already has per-key demand) |
| assignment, synthetic, liveness walk, ≤ 8.1 ms | delta versions over the changed files' sections and symbols; a liveness walk from the changed edges only (needs reverse edges in the state) |
| `.debug_aranges` and `.eh_frame_hdr` rewritten whole, ≈ 1.5 ms | patched in place where no address moved; `.debug_aranges` must stay whole while gdb rejects duplicate unit offsets (IN-54) |
| `stat` of every input, 0.6 ms | a trusted change list from the build system (not in v1: hints only add work, IN-23) |

**What the design buys for a very large, old C++ code base.** At 2–10 GB
of output, a v1 update is a constant ~4.5× (in place) or ~2.5× (copy)
faster than a full link, by the estimate; that is seconds instead of
tens of seconds, and peak memory falls from tens of GB to the state's
decoded size plus the windows (~12% of the full link's, *estimate*).
Beyond that, every further gain comes from the steps in the table above
in that order; the format of v1 does not rule any of them out (the state
is tables with per-table hashes, the output's sections are already
placed in regions). If an update at the scale of the operator's code
base must be O(change) end to end, those steps are M7 work after v1 and
the cut (§10) must make room for them; WP-061's measurement on the
go/no-go workload is what shows which of them pays first.

**Measuring the large workload.** No static `-g` link in the container
is ≥ 10× protoc. Two candidates:
- **`synthetic-objects-g10x`**: the bench's `synthetic-objects` shape
  (`xtask/src/bench/synth.rs`; 80.7 MB of objects at 1,000 objects of
  200 functions) compiled with `-g`, its object count N fixed at WP-061's
  start as the smallest giving ≥ 1.64 GB of live input (N ≈ 10,000–20,000,
  *estimate*), linked statically. Cheap, reproducible, generated; but C,
  uniform, with no templates, COMDATs or `.debug_str` sharing, so it
  under-represents the O(symbols) terms of real C++. It must be deleted
  after measuring (the workspace disk is synced to the host).
- **clang, linked statically with `-g`** (LLVM, public, Apache-2.0 with
  LLVM exception; GCC builds it here): a real, old, very large C++ code
  base, the shape the operator targets; its `-g` output is several GB
  (*estimate*). It fits the tests' needs (public sources, a fixed
  release tag, a deterministic build, a reference link by GNU ld and
  lld for the oracle) on two conditions: its archives are built thin
  (CMake's archive rule with `ar qcT`) so the gate measures the
  thin-archive path, and the operator accepts its build cost (*estimate*:
  hours of CPU on the shared pool and 10–30 GB of objects on the synced
  disk; not measured). Recommendation: the synthetic workload is the
  go/no-go gate (it can be built inside WP-061); clang static `-g`
  confirms the decision at the scale that matters, as a bench workload
  added when the operator approves its disk and CPU.

**Go/no-go rule** (the re-review's N7). WP-061's first deliverable is
the floor **measured with mortar's own code** (state read and validated,
stat and stamps, change detection, re-resolution, refill through
windows with relocation through the image phase, IN-46/IN-47 writes;
the edit is same-size, so the previous layout stands and the delta
layout's cost stays the §1.5 bound until S2), for a one-function edit to the median file, thin archives,
both write paths, time and peak RSS, on **protoc `-g`** and on
**`synthetic-objects-g10x`**, both fixed before anything is measured.
The thresholds are an **operator decision** (record (a) 6), recommended:
the update on the default write path at most **1/2** of a plain full
link on protoc `-g` and at most **1/3** on the large workload. Then:
- **go** (S2–S6 as cut, §10) if both are met;
- **change order** if one is missed and the miss is attributed to one of
  the named parts below. Each named part carries its own measurement,
  done inside WP-061 or in **one time-boxed follow-up of at most one
  session**, and the thresholds are checked again on its numbers:
  - the copy → WP-061 already measures in place; if in place meets the
    threshold, the operator's write-path decision (record (a) 1) is
    revisited with that number;
  - window refill → smaller update windows prototyped in the follow-up
    (a write-size change only; image.md I-75's leaf size stays) and
    measured;
  - changed-file refill without `-g` with `-ffunction-sections` above
    25% of the update → IN-31 (section-level refill) prototyped for the
    changed file in the follow-up and measured;
  - an O(symbols) term of the table above (state, `.symtab`,
    re-resolution) above 25% of the update → that row's O(change)
    version prototyped in the follow-up and measured;
  - regular-archive detection does not gate (the gate uses thin
    archives); its number is reported;
- **no-go** if, after the named parts are measured, the update is still
  above the threshold on the large workload: M7 stops after S1 (the
  state is kept for tuck), and the operator is told that a fast full
  link is the better investment, with the numbers. A miss attributed to
  a part not named here is no-go too.

By the scaling model (*estimate*), in place is predicted to pass both
thresholds (3.5–1.7× on protoc, 4.0× at 500 MB) and the copy to miss
1/3 on the large workload (2.3×), which triggers the first escape: the
write-path decision would be revisited with measured numbers.

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
     choice depends on whether the target fits in 32 bits, **in an
     `ET_DYN` output only** (v1: in a static or other `ET_EXEC` output the
     `GOTPCRELX` forms must be the same, so an update whose distances
     would make them differ is refused; `plumb.md` §6.9 P-E4). Relaxations
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
     relink. `docs/` says so.) For an `SHF_LINK_ORDER` section the
     full-link order is the gABI's (that of the linked-to sections), so
     the two agree only while no linked-to region moves; a move is the
     full link `link-order-moved` (§7; the re-review's n3);
  6. every byte of `X` not covered by 1–5 is in a **dead range**
     recorded in the decision tables (IN-3a) and passes IN-3a's three
     checks;
  7. the diagnostics are the same (IN-5).

  Equal bytes are not required: placement differs (slack, holes, a
  region moved to a tail), and so do the build id and every address.
  *Test:* `cargo xtask plumb --equiv X.map F.map X F` (`plumb.md` §6.9,
  §8; `mortar_plumb::equiv::check` in a test; an extension of the map
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
  allocated case of a dead range (IN-3a). **Symbolization of a hole**
  (the re-review's n4): a hole has no symbol, so a stale pointer into a
  removed function is attributed by `addr2line -f` to the preceding
  function (measured by the re-reviewer: `_Z7throweri` for an address in
  `f_dead`'s old range), while gdb ("No symbol matches"), lldb and
  llvm-symbolizer (`??`) report none. Decided: this is a **known
  difference**, stated in `docs/` and in Tuck's notes, not fixed with a
  symbol: a local `STT_NOTYPE` symbol per hole would cost a `.symtab`
  entry (24 B plus a name) per hole, change `.symtab` from a full link's
  (IN-2 item 4 would need an exception), and still leave `addr2line`
  naming a symbol that is not a function; nothing in the output is
  wrong, only one tool's attribution. *Test:* remove a function and
  check that its old range is all trap fill (`plumb` checks every `hole`
  line's bytes); R4 (`packed_holes.sh`) is the negative case: a hole in
  `.init` traps (SIGILL) and a zero slot in a `__start_` array crashes
  (SIGSEGV).
- **IN-3a [v1] Dead ranges.** A **dead range** is a byte range of the
  output that a full link of the current inputs would not write, left by
  an update: an allocated hole (IN-3), a dead debug contribution (§6),
  a dead fragment of a merged section (L-147, S-110), an unused CIE in
  `.eh_frame`, a retired slot (IN-72 item 7: its words 0,
  `HoleKind::DeadSlot`, a `hole` line in the map; WP-214 seam 6). The decision tables record every dead range as
  (output section, offset, length, kind), and the map lists them
  (`dead` lines, §8.2). Invariant (B) holds outside them (IN-1). Inside
  them the verifier checks, independently of the linker and of the
  replay:
  1. **nothing live names the range**, judged by what a reference
     names, not by the address it computes (the re-review's N3): no
     relocation of a live section names a symbol defined in a dead range
     or a dead input section, or a section symbol of a dead input
     section; no dynamic or synthetic table entry (GOT, PLT, `.dynsym`,
     `.eh_frame_hdr`, `RELATIVE` targets) does; no live symbol is defined
     in it; and no DWARF form offset of a live unit (`DW_FORM_ref_addr`,
     `DW_FORM_sec_offset`, `DW_FORM_strp`, `DW_FORM_line_strp`,
     `DW_AT_str_offsets_base`, `DW_AT_addr_base`, `DW_AT_rnglists_base`,
     `DW_AT_loclists_base`, `DW_AT_stmt_list`, an abbreviation offset)
     points into it. A reference's base is the symbol's input section,
     or the section symbol's, whatever the addend; an offset equal to the
     section's size belongs to the section. So an address that lands in a
     hole through an addend from a live symbol is valid: one past the end
     of an array (`R_X86_64_32 arr+0x28` for a 40-byte `arr`), gfortran's
     1-based bias (`R_X86_64_PC32 blk_-8`, before the symbol), an
     `extern char x_end[]` label at its section's end;
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
     fill**, except dead fragments of merged **string** sections
     (`SHF_MERGE|SHF_STRINGS`), which keep their bytes because a live
     reference may share a suffix (L-147). Dead fragments of merged
     **constant** sections (`SHF_MERGE` without `SHF_STRINGS`,
     `.rodata.cst*`) take the fill like any data: constants share no
     suffixes, so zero-filling them is safe (the re-review's n2; IN-34's
     row says the same).
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
  the state. In v1 an update that would report an error does not report
  it itself: it falls back to the full link, whose failure is this
  rule's (its diagnostics, the output removed, the state removed;
  IN-48's test holds for it). *Test:* L10 steps that introduce and then
  fix an undefined
  symbol; the diagnostics are compared with a full link's.

## 4. Determinism

- **IN-6 [v1] A function of its history.** The output and the new state
  of an update are a function of (inputs, options, previous state) and
  nothing else: not of `--threads`, the schedule, the time, or what else
  is in the directory. The same sequence of edits replayed from the same
  first link gives the same bytes. *Test:* L10 replays each sequence
  twice, with `--threads=1` and the default, and compares the outputs
  byte for byte and the states byte for byte apart from what records the
  file system rather than the link: `STAT`'s stamps (the device rows'
  `stamp_ns`) and the output row's `ino`, `mtime_ns`, `ctime_ns` and
  `stamp_ns` (IN-18a). Every other table is identical (measured on
  WP-061's corpus by the review: two identical links, and `--threads=1`
  against `--threads=7`, differ only there).
- **IN-7 [v1] Not reproducible across histories.** Two developers with
  different edit histories get different (equivalent, IN-2) bytes. So
  incremental mode is for development; reproducible and release builds
  link plainly. `docs/` and `--help` say so; a link with
  `SOURCE_DATE_EPOCH` set and `--incremental` gives a warning
  (`incremental-not-reproducible`) once per link: under
  `SOURCE_DATE_EPOCH` the bytes of an incremental output still depend on
  the edit history (slack consumed, holes, dead ranges, region order),
  so equal inputs give equal bytes only from the same history; a plain
  link's bytes do not depend on history. Whether this warns or refuses
  is an operator decision (record (a)); the default is a warning. No
  byte-identity test (`static-identical`, the reference-output tests)
  links with `--incremental`; L10's tests compare by IN-2. *Test:* a
  link test with the variable set checks the warning; a lint of the test
  suites checks no byte-identity test passes `--incremental`.
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
  | `--incremental-report[=FILE]` | one line per link to `FILE` (default stderr): `incremental: update` or `incremental: full (<reason>)`, then `changed=<files> refilled=<units> bytes=<written> time=<ms>`; after it, when IN-10a says so, the dry run's `delta` lines, bounded (IN-10b) |

  A fall-back is reported without `--incremental-report` too, as one
  report line (`incr-full-link`; a line, not a diagnostic: `spec/diag.md`
  §2.1) naming the reason; an update
  that succeeds prints nothing. gold's `--incremental-changed`,
  `--incremental-unchanged` and `--incremental-unknown` are not taken
  (IN-23); `--incremental-base` neither (the state names its output).
  *Test:* a link test per row of the table.
- **IN-10a [v1] Which links compute a delta** (WP-106). The delta
  (change detection, IN-25..IN-29, the refill set) is computed only for
  a state an update could start from: one that passed every check of §7
  before the delta (read and validated, the plan, the target, the
  updating flag, the output), so that the reason is
  `update-not-implemented` (stage A) or `small-output`. `small-output`
  is a policy decided from the output's size alone, which
  `--incremental=update` does not apply; the dry run says what that
  update would have done, at the cost of the delta of an output under
  256 MiB. Every other reason (`no-state`, `state-corrupt`,
  `state-version`, `options-changed`, a state of another target among
  them, `output-changed`, `output-torn`, `--incremental=full`, a shared
  object input) says before any delta that no update could run: the
  link keeps no old state past the check and the report names the
  reason alone. Until WP-064 the dry run is computed only when
  `--incremental-report` asks for it: a link without the report
  computes no delta and keeps no old state. A fall-back that the delta
  itself finds (`delta fallback`, `delta policy`) does not stop it: its
  refill set is the oracle's evidence that each fall-back is needed
  (IN-63b), and IN-10b bounds what is printed of it. *Test:*
  `a_target_change_at_the_same_path_is_options_changed` (a state of
  another target: no `delta` line), `a_small_output_is_linked_in_full_by_name`
  (the dry run under `small-output`), `fall_back_reasons`.
- **IN-10b [v1] The report is bounded** (WP-106; WP-094 measured
  801,742,419 B, 12,924,615 lines, on one fall-back of g10x). Each kind
  of listing line (`path`, `stat-missed`, `file`, `probe`, `symbol`,
  `fallback`, `unit`, `hole`, `chunk`: `docs/cli.md` §3.13) is written
  for its first `REPORT_BOUND` = 10,000 instances in the report's order
  (`mortar_incr::delta::REPORT_BOUND`); past that the lines are counted,
  not formatted, and the kind's last line is followed by `delta elided
  <kind> listed=10000 total=<n>`. The count lines (`delta inputs`, `delta
  symbols`, `delta refill`) are never bounded, so every total is stated,
  and `delta listed bound=10000 elided=<n>` before `delta time` gives the
  number of lines left out (0 when none): nothing is dropped unsaid. A
  reader that needs every line (the oracle lane, IN-63a) treats a report
  with a `delta elided` line as one it cannot judge. The report is a
  function of the link's inputs, options and state except for its two
  times (`time=` and `delta time`). The bound keeps every oracle-lane
  report whole (its largest kind is 2,137 units) and caps a report at
  90,000 listing lines. *Test:* `the_report_is_bounded`
  (100,000 moved sections), `delta::tests::the_report_bound`.
- **IN-11 [v1] A plain link never looks.** Without `--incremental`,
  mortar does not open, stat or remove a state file. A state left beside
  an output that a plain link replaced no longer matches it (IN-21) and
  the next incremental link says `output-changed`. *Test:* plain link
  over an incremental output, then `--incremental`, checks the reason;
  `a_plain_link_touches_nothing_of_incremental_mode` (WP-106: FIFOs at
  both state paths, an existing state's bytes and times, the report
  options without `--incremental`, the directory's entries).
- **IN-12 [v1] Incompatible modes.** With `-r`, `--emit-relocs`, a
  `SECTIONS` script, `--compress-debug-sections` other than `none`,
  `--gdb-index`/`--debug-names` generation, `--icf`, `--sort-common` or
  `--sort-section` (`spec/layout.md` L-88a, L-53a: an update keeps each
  input's place and does not re-sort; WP-111), `--start-lib` (its
  objects are lazy files but not archive members, IN-71b; WP-212, F-9;
  U-3 implements), and (until WP-066)
  an output kind other than a static executable (`-pie`, `-static-pie`,
  `-shared`, a dynamic `-no-pie` executable; keyed on the plan's output
  kind, so `-no-pie` without `-static` is unsupported even when no
  shared object is linked), `--incremental`
  gives one warning (`incremental-unsupported`, naming the option) and
  links plainly, writing no state; with `--incremental=update` it is an
  error. These are §7's "unsupported" rows: a state for them would never
  be used. *Test:* one link test per option: the warning, no state file
  written, and the output byte-identical to a plain link's; with
  `--incremental=update`, the error.
- **IN-12a [M4] (`-shared` stays unsupported).** Request R-N10 of
  `spec/dyn.md` §18.2. WP-066 lifts IN-12's output-kind clause for
  executables only: `-shared` (output kind `Shared`) stays an
  unsupported incremental kind, IN-12's warning and plain link, until
  `spec/dyn.md` D-127's interface comparison (export set, versions,
  `DT_SONAME`, the `NONPREEMPTIBLE` set; `interface-changed`) is
  designed in its own package (none cut yet). No `Shared` package of
  WP-117..WP-130 writes incremental state. *Test:* IN-12's, with
  `-shared`.

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
  `--incremental-state` and `--incremental-report=FILE` naming the output,
  any file the link reads or another written file (the same path, or the
  same `(dev, ino)` as an existing one) are refused by name before
  anything is written: since WP-087 by the one rule of every written
  path (`spec/cli.md` §3.14, `written-file-clash`; `incremental-file-clash`
  is no longer emitted), to which the session gives its two files
  (`Session::written`), which refuses the output as an input too
  (image.md I-62 since WP-087). *Test:*
  `the_state_or_report_naming_a_file_of_the_link_is_refused`,
  `crates/mortar/tests/written.rs`.
  Since WP-114 (image.md I-58b) the state path is refused by name
  (`cannot-write-output`, the link fails) before anything is read or
  written unless it is absent, or a regular file, in a directory where
  the link may create files (a link through a descriptor table such as
  `/dev/stdout`, a device, a pipe, a name in `/dev`: before WP-114 the
  state's writer renamed over `/dev/stdout`, and read a pipe behind it
  until the link hung); the report is appended in place, and refused the
  same way when I-58b refuses its path. The state's own writer and the
  in-place update's copy (IN-47) check the same rule again before their
  rename (`mortar_image::output::may_replace`).
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
  | decision tables: chunks, regions, holes, slots, pieces, merged maps (fragment hash → offset), islands, dynamic slices | 35,684 pieces, ~4,000 regions | 0.3 MB plus the merged maps (`.debug_str`'s 76,974 fragments: [later, G-1]; format 5 stores no `.debug_*` fragment, WP-239 R-1) |
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
  *Amended at WP-061 stage A:* the version 1 state is built in memory
  after the link and encoded at once (8 MB for protoc `-g`); stage B
  measures the peak of an incremental-mode full link against a plain
  one on its workloads and decides then whether the writer streams.
- **IN-21 [v1] Bound to its output.** After its last write to the
  output (IN-46 step 5), the link `fstat`s it, then takes **stamps**
  (IN-22a) on the output's file system until a stamp's ctime is later
  than the output's (at most one timestamp tick: measured 0.9–1.0 ms on
  this host's overlay, whose ctime tick is 1 ms, and 0.08–0.12 ms on its
  virtiofs, tick 82 µs; R6), `fstat`s the output again, and records the
  second tuple `(dev, ino, size, mtime_ns, ctime_ns)`, that stamp's
  ctime as the output's **reference stamp**, and the build-id. If the two
  `fstat`s differ, something wrote the output during the link: no state
  is written (the next link says `output-changed`). The state is bound to
  the file the link wrote, not to the path: the link `fstat`s its
  temporary output before the rename (IN-46) and keeps its `(dev, ino)`;
  if the path's `(dev, ino)` or size after the rename are not those,
  another link has replaced the output, and no state is written and any
  state at the state's path is removed, so nothing is left that the next
  link could accept for this link's command line and the other's output
  (a plain link does not pay this `fstat`: it is taken only in
  incremental mode). Two such links never write the state's temporary
  at once: it is locked by its writer, and the one that finds it held
  writes no state (IN-77d, WP-219). An update proceeds
  only if `stat(P)` gives the recorded tuple, `P`'s ctime is strictly
  older than its reference stamp, and the build-id note read from `P` (one
  positioned read at the offset the state records) is the same. A tuple
  that differs is `output-changed`. A tuple that is equal but whose ctime
  is not older than the reference stamp (a write in the stamp's own tick,
  after the link) is **racy**: the output's bytes are re-hashed against
  the stored build-id leaves (image.md I-76; one read of the output, ~10–
  20 ms for protoc's 50 MB from the page cache, *estimate* from R1's read
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
  "updating" flag of IN-46: set means `output-torn`. *(Amended by
  WP-207: an update makes these checks through the one descriptor U1
  opened, and checks the path again before its first write to `P`
  (IN-77a); "the link's lock" is IN-77b's lock on `S`; a full link's
  unsynced output is checked once after a reboot (IN-77c).)* *Test:* `touch`,
  `strip`, `cp` of another output, `objcopy` in place, each over the
  output: each gives `output-changed`; a killed update gives
  `output-torn`; `stamp_rule.py` case B (an update followed at once by
  the next link's check, 200–400 times on overlay and virtiofs: own
  output seen as changed 0 times) and the reviewer's `state_tick.py`
  (39/200 and 60/200 own-output fall-backs under the old rule) become L10
  cases on a coarse-timestamp file system; the WP-061 review's
  `cross_race.sh` (two links racing to one output and one state, with
  `--build-id=none`) is `concurrent_links_never_cross_output_and_state`:
  0 crossed pairs.
- **IN-18a [v1] The bytes, format version 1** (S1, written by WP-061
  stage A; the contract a reader is checked against, and the one a
  re-implementation must write). All integers little-endian, rows
  fixed-width and packed (no alignment padding); `u8`…`u64`, `i64` as
  usual; a **str** is `(offset u32, len u32)` into the `STRINGS` table's
  bytes; a **row reference** is a `u32` row number in the named table,
  `NONE = 0xffff_ffff` where the field says it may be absent; `pad`
  fields are zero. A hash is 16 bytes of `L128` (image.md I-114).

  **Header**, 64 bytes at offset 0:

  | offset | field |
  |---|---|
  | 0 | magic `MORTARST` (8 bytes) |
  | 8 | format version `u32` = 1 |
  | 12 | flags `u32`: bit 0 **updating** (IN-46); any other bit set is `state-corrupt`; not covered by the header hash, so IN-46 can set and clear it with one positioned write |
  | 16 | table count `u32` = 23 (any other count is `state-corrupt`) |
  | 20 | pad `u32` |
  | 24 | file length `u64`: the state's size in bytes |
  | 32 | plan hash: `L128` of the normalized plan (IN-18b) |
  | 48 | header hash: `L128` of bytes 0..12 followed by bytes 16..48 |

  **Tables** follow the header back to back to the end of the file, each
  a 40-byte record `(kind u32, pad u32, row count u64, byte length u64,
  L128 of the rows' bytes)` and then its rows. Byte length is row count
  × the kind's row width (`STRINGS`: row count, one byte per row). In
  version 1 every kind below appears exactly once, in ascending kind
  order, `STRINGS` last; the end of the last table is the file length.
  Kinds 32–63 are reserved for the later tables of IN-19 (32 symbol
  summaries, 33 resolution, 34 dependency index, 35 liveness graph, 36
  warnings, 37 armaps, 38 section hashes, 39 regions and holes, 40 debug
  layout); a kind this version does not define is `state-corrupt` (a new
  table is a new format version, never an optional extra).

  | kind | table | row (width) | rows |
  |---|---|---|---|
  | 1 | `META` | version str, `e_machine u16`, class `u8` (1, 2), output kind `u8` (0 static executable; the others are refused by IN-12 in this stage), leaf function `u8` (0 L128, 1 SHA-1, 2 SHA-256), pad 3, output size `u64`, build-id descriptor start `u64`, end `u64` (the file offsets of the build id's bytes in the output; both 0: none), build id str (the id's bytes, at most 64; empty: none) (48) | 1 |
  | 2 | `STAT` | tag `u8` (0 device stamp, 1 input file, 2 output), trust `u8`, pad `u16`, pad `u32`, path str, `dev u64`, `ino u64`, `size u64`, `mtime_ns i64`, `ctime_ns i64`, `stamp_ns i64`, content hash, file-system type str (88) | device rows sorted by `dev`, then one row per path opened (`spec/input.md` §10.4 `opened`, in that order, a path opened twice once, at its first opening), then exactly one output row |
  | 3 | `FILES` | kind `u8` (0 internal, 1 object, 2 shared object), member `u8` (0, 1), pad `u16`, path str, mention `u32`, member name str, member ordinal `u32`, base `u64`, length `u64`, content hash (60) | one per `FileId`, in order (row 0 the internal file) |

  `STAT`'s rows by tag (a field a row does not use is zero, a str
  `(0, 0)`). A **device** row (IN-22a): path the directory the stamp was
  taken in, `dev`, `stamp_ns` the stamp's ctime, trust 1 when the file
  system is not trusted (IN-22a's allow-list and probe), file-system
  type the mountinfo type; `ino`, `size`, the times and the hash zero. An
  **input** row: path as the link opened it, the stat tuple
  `(dev, ino, size, mtime_ns, ctime_ns)` of the file read, its content
  hash and trust 0 when the link read and hashed it, else the tuple of a
  `stat` and trust 1 (hash zero; the whole tuple zero when the file is
  gone). The **output** row: path the output, the second tuple of IN-21
  and `stamp_ns` its reference stamp; its `size` is `META`'s output
  size. Times are nanoseconds since the epoch (`st_*tim` seconds × 10⁹ +
  nanoseconds), as `i64`.

  `FILES`: path and mention are the file's identity (model §7.1:
  for a file named directly, how many earlier files not archive members
  have that path; for a member, how many earlier archive mentions have
  its archive's path), with member name and ordinal for a member (for a
  file that is not a member: `(0, 0)` and 0); base and length are the
  file's byte range in what was read (its archive, for a member; base
  plus length must not pass 2⁶⁴), both 0 for the internal file, which has
  no bytes; the content hash is `L128` of that range (zero without
  bytes).
  | 4 | `PROBES` | path str, present `u8`, pad 3 (12) | `Inputs::probe_checks`, in order |
  | 5 | `OUTSECS` | name str, ordinal `u32` among earlier rows with that name (12) | one per output section, `OutSecId` order |
  | 6 | `SYMBOLS` | kind `u8` (0 global, 1 versioned global, 2 local), pad 3, file `FILES` row (locals; else `NONE`), ordinal `u32` (locals; else 0), name str, version str (kind 1; else `(0, 0)`) (28) | every key the other tables name, in first-use order, no duplicates |
  | 7 | `SECTIONS` | file `FILES` row, anchor kind `u8` (0 none, 1 COMDAT, 2 global, 3 local), pad 3, anchor str (`(0, 0)` when the kind is 0), name str, ordinal `u32`, `sh_type u32`, `sh_flags u64`, `sh_size u64`, alignment `u64`, file offset `u64` (0 for `SHT_NOBITS`), place chunk `CHUNKS` row or `NONE`, place offset `u64` (0 when `NONE`) (76) | one per `SectionId`, in order |
  | 8 | `CHUNKS` | tag `u8` (below), bits `u8` (0 has address, 1 has `shndx`, 2 relro, 3 address-dependent synthetic), alignment `u8` (log2, below 64), pad `u8`, name str, ordinal `u32`, output section `OUTSECS` row or `NONE`, part `u32`, `sh_type u32`, `sh_flags u64`, size, capacity (size ≤ capacity), address (0 without bit 0), file offset `u64`, `shndx u32` (0 without bit 1) (72) | one per `ChunkId`, in order |
  | 9 | `ORDER` | chunk `CHUNKS` row (4) | `Placement::order` |
  | 10 | `SEGMENTS` | `p_type u32`, `p_flags u32`, offset, address, `filesz`, `memsz` `u64`, alignment `u64` holding the **log2** of `p_align` (below 64), as `Segment::align` (48) | `Placement::segments` |
  | 11 | `FRAGS` | output section `OUTSECS` row, ordinal `u32` or `NONE`, content hash, offset `u64` (32) | one per fragment identity (model §7.1: output section and content hash) that has a live fragment, strictly ascending by (output section as a number, hash compared as 16 bytes from the first, ordinal as a number, `NONE` greatest); ordinal `NONE` when all its live fragments share one offset, else one row per live fragment with its ordinal among them in `FragId` order |
  | 12 | `SLOTS` | slot kind `u8` (0 GOT, 1 PLT, 2 IPLT, 3 TLS descriptor, 4 TLS GD, 5 TLS IE), pad 3, key `SYMBOLS` row (8) | the slot lists in that kind order, each in its order |
  | 13 | `EH_RECORDS` | section `SECTIONS` row, piece `u32` (8) | `Slots::eh_records`: the output `.eh_frame` is made from this list (synth.md S-49, which says which lists are admitted) |
  | 14 | `SYMTAB_LOCALS` | start `u32`, length `u32` (8) | `Slots::symtab_locals`, one per `FILES` row (or none: no `.symtab`); held to the symbol table's own spans, the only value S-88 admits in version 1, and read by an update for the previous output's entries (IN-49a) |
  | 15 | `SIZES` | chunk `CHUNKS` row, size `u64` (12) | `Slots::sizes` |
  | 16 | `SYM_PLACE` | kind `u8` (0 common), pad 3, symbol `SYMBOLS` row, chunk `CHUNKS` row, offset, size `u64`, alignment `u8` (log2), pad 3 (32) | `Placement::sym_place` |
  | 17 | `DEFINED` | symbol `SYMBOLS` row, kind `u8` (0 absolute, 1 in a chunk), pad 3, chunk `CHUNKS` row (kind 1; else `NONE`), value or offset `u64` (20) | `Placement::defined` |
  | 18 | `PATCHES` | chunk `CHUNKS` row, kind `u8` (0 thunk to a key, 1 thunk to a key's PLT entry, 2 erratum 843419 patch), pad 3, offset `u64`, key `SYMBOLS` row (kinds 0, 1; else `NONE`), section `SECTIONS` row (kind 2; else `NONE`), addend `i64` (kind 0) or site offset (kind 2) (32) | `Placement::patches` |
  | 19 | `REDIRECTS` | section `SECTIONS` row, patch `PATCHES` row, offset `u64` (16) | `Placement::redirects` |
  | 20 | `SIZED` | chunk `CHUNKS` row, size `u64` (12) | `Placement::sized` |
  | 21 | `LAYOUT` | bits `u8` (0 TLS, 1 entry, 2 TLS LD module slot), TLS alignment `u8` (log2), pad `u16`, pad `u32`, TLS start, TLS size, entry `u64` (32) | 1 |
  | 22 | `LEAVES` | digest (32: the leaf function's bytes, zero-filled) (32) | `leaf_count(output size)` (image.md I-72), window order |
  | 23 | `STRINGS` | bytes (1) | the pool |

  `CHUNKS`' tag is the chunk's kind (`ChunkTag`): 0 file header, 1
  program headers, 2 section headers, 3 body, 4 merged, 5 patches, 6
  synthetic, 7 group, 8 relocations. Its name is the synthetic chunk's
  name for tag 6, the output section's name for tags 3, 4, 5, 7 and 8,
  and `(0, 0)` for the headers; its output section is the chunk's for
  tags 3, 4, 5, 7 and 8 and `NONE` otherwise; part is the body or patch
  part's number (tags 3 and 5), else 0. The **ordinal** (the chunk's
  identity, model §7.1, with tag and name) is the number of earlier
  chunks, in `ChunkId` order, with the same tag and the same name (no
  name counts as one name). `OUTSECS`' ordinal is the same count among
  output sections by name.

  **The validating reader** checks, before any row is used and in this
  order: the length is at least 64; magic (else `state-corrupt`); the
  format version (else `state-version`); the header hash; the file length
  equals the file's size; each table record lies inside the file, has
  the expected kind in order, a byte length equal to row count × width,
  and its hash; then every row: pad and reserved bits zero, enumerations
  and booleans in range, every str inside `STRINGS`, every row reference
  inside its table (or `NONE` where allowed), the counts of one-row
  tables, no bytes after `STRINGS`; then, table by table: `META`'s
  build-id descriptor inside the output and exactly the build id's
  length, at most 64 bytes (so the output check reads that many bytes
  and no more); the `STAT` order (device rows ascending by `dev`, then
  inputs, then one output row), no input path twice, the output row's
  size equal to `META`'s output size; `FILES`' member fields zero for a
  file that is not a member, and base plus length within 2⁶⁴; `OUTSECS`
  ordinals as stated; `SYMBOLS`' fields by kind (a global: file `NONE`,
  ordinal 0, no version; versioned: file `NONE`, ordinal 0; a local: a
  file, no version) and no key twice; `SECTIONS`' anchor empty for kind
  0 and place offset 0 without a chunk; `CHUNKS`' fields as stated in
  its row; `ORDER` a permutation of the chunks (each once); `SEGMENTS`'
  alignment below 64; `FRAGS` strictly ascending as stated; `SLOTS` in
  kind order with no (kind, key) twice; `SYMTAB_LOCALS` empty or as long
  as `FILES`; `SYM_PLACE` kind 0 and alignment below 64; `DEFINED`'s
  chunk by kind and no symbol twice; `PATCHES`' key, section and value
  by kind (a thunk to a key's PLT entry has value 0); `LAYOUT`'s fields
  zero where their bit is clear; `LEAVES` as long as the output size
  says, its bytes past the leaf function's digest zero. Then
  `META`'s version string is compared with this mortar's
  (`state-version`). Any failure is `state-corrupt` with a detail naming
  the table, row and field (`state-corrupt (CHUNKS row 7: output section
  out of range)`), never a panic; arithmetic on stored values is checked.
  What can only be checked against the present link (identities that
  must resolve, model §7.2) is the replay's and the update's (IN-61,
  WP-064), not the reader's. **Dynamic tables** (`Slots::dynsym`,
  `dyn_relocs`, `needed`, `verneed`, `dynstr`, `copies`, `copy_of`) and
  `Folds` have no table: version 1 states exist only for static
  executables (IN-12), where they are empty; a writer that finds them
  non-empty writes no state. *Test:* `mortar_incr`'s round trip and
  mutation tests: every single-byte change and every truncation of a
  state of the link-test corpus is a named error or (a change of the
  header's flags) accepted; never a panic.
- **IN-18a2 [v2] Format version 2: the delta tables and the holes**
  (WP-063, WP-062). Version 2 is version 1 with these changes:
  - The format version is 2.
  - The table count is 27.
  - Four tables, kinds 32, 33, 34 and 39 (IN-19's reserved numbers),
    come after `LEAVES` and before `STRINGS`. Kind numbers do not give
    the order; the file order is this list, each table exactly once:
    `META` (1), `STAT` (2), `FILES` (3), `PROBES` (4), `OUTSECS` (5),
    `SYMBOLS` (6), `SECTIONS` (7), `CHUNKS` (8), `ORDER` (9),
    `SEGMENTS` (10), `FRAGS` (11), `SLOTS` (12), `EH_RECORDS` (13),
    `SYMTAB_LOCALS` (14), `SIZES` (15), `SYM_PLACE` (16), `DEFINED`
    (17), `PATCHES` (18), `REDIRECTS` (19), `SIZED` (20), `LAYOUT` (21),
    `LEAVES` (22), `MENTIONS` (32), `RESOLVED` (33), `INDEX` (34),
    `HOLES` (39), `STRINGS` (23) last.

  There is one version 2. A state of version 1 is refused by name like
  any other version (`state-version`, `format version 1, this mortar
  reads 2`): it has none of these tables, an update has nothing to
  compute a delta from, and no compatibility path reads it (nothing
  shipped a version-1 state). The link is a full link that writes a
  version-2 state. A reader of version 1 likewise answers a version-2
  state `state-version`.

  | kind | table | row (width) | rows |
  |---|---|---|---|
  | 32 | `MENTIONS` | file `FILES` row, key `SYMBOLS` row, kind `u8` (0 undefined, 1 in a section, 2 absolute, 3 common), `st_bind u8`, `st_type u8`, `st_other u8`, section `SECTIONS` row (kind 1; else `NONE`) (16) | one per (live object, global key in its symbol table), strictly ascending by (file, key). An entry the file's symbol table repeats keeps the first. |
  | 33 | `RESOLVED` | kind `u8` (0..5 as `MENTIONS` plus 4 linker-defined and 5 shared object; 6 COMDAT keeper; 7 live file), `st_type u8`, `st_bind u8`, `st_other u8`, key `SYMBOLS` row (kinds 0..5; else `NONE`), file `FILES` row (the definer, the keeper or the live file; `NONE` for kinds 0 and 4), section `SECTIONS` row (kind 1; else `NONE`), value `u64` (`st_value` for kinds 1 and 2; for kind 3, a common, its alignment `max(1, st_value)`: 0 and 1 mean the same, stored as `COMMONS` stores the alignment, so an update whose winning common is an unchanged file's gives the same row (WP-215 R.6 5, ruled; state rows only, no output byte); the definer's symbol index for kind 5, and for a definition outside an object's symbol table, which is recorded as kind 2; else 0), size `u64` (`st_size`; for a common, the resolved size), name str (kind 6: the group signature; else empty) (40). An undefined key has `st_bind` 2 when weak, else 1, and its resolved visibility in `st_other`; a linker-defined key only its visibility. A definition in a section the reader keeps no id for is recorded as kind 2, in both tables. | first one row per key the resolution names (kinds 0..5), ascending by key; then one per COMDAT group with a keeper, ascending by signature bytes; then one per live file, ascending by file |
  | 34 | `INDEX` | unit `SECTIONS` row, kind `u8` (0 global, 1 section, 2 slot, 3 `.eh_frame`, 4 link order, 5 local slot), pad 3, target `u32` (a `SYMBOLS` row for kinds 0 and 2; a `SECTIONS` row for 1, 3, 4 and 5) (12) | one per (unit, kind, target), strictly ascending in that order |
  | 39 | `HOLES` | chunk `CHUNKS` row, kind `u8` (`mortar_core::layout::HoleKind`: 0 fill, 1 `.eh_frame` padding, 2 debug, 3 abbreviation table, 4 dead debug, 5 dead fragment, 6 dead CIE; format 5 adds 7 dead slot, WP-214 seam 6), pad 3, offset `u64`, size `u64` (24) | one per range of a chunk no live unit owns (IN-3, IN-3a, IN-30), ascending by (chunk, offset), never overlapping, inside the chunk's size (a capacity tail is not a hole, L-141) (WP-062) |

  `INDEX` records the references of every kept allocated section and of
  every `.eh_frame` input of a live object file, read from its
  relocations. One relocation gives these rows:
  - A global key gives kind 0. Through a GOT, PLT or TLS slot, or the
    GOT's base (the architecture's class of the type), it also gives
    kind 2. An undefined entry is first redirected to its definition.
  - A local with a section gives kind 1 for that section, or kind 3
    when the section is an `.eh_frame`. A slot class also gives kind 5
    with the section as target.
  - Symbol 0, or a local without a section, gives kind 5 with the unit
    itself as target when the class is a slot.
  - A type the architecture cannot classify counts as a slot use.
  - A unit's references to its own section are not recorded.
  - An `SHF_LINK_ORDER` unit gives kind 4 for its `sh_link` section.

  The reader adds these checks to IN-18a's:
  - every reference is in range;
  - `MENTIONS`' kind is 1 exactly when it has a section;
  - `RESOLVED`'s fields match its kind as stated, and its rows are in
    the stated order with no key, group or file twice;
  - `INDEX`'s kind is in range, its target is in its table, and its rows
    strictly ascend.
  - `HOLES`' chunk and kind are in range, no size is 0, its rows
    ascend by (chunk, offset) without overlap, and each lies inside its
    chunk's size.

  *Test:* `mortar_incr`'s round trip, the incremental tests' replay
  (each table matters), and `a_version_1_state_is_refused_by_name` (the
  version-1 fixtures `crates/mortar/tests/fixtures/incr-state-v1-51aa582/`
  are `state-version` and give a full link).
- **IN-18a3 [v1] Format version 3: positions** (WP-094, for IN-28a).
  Version 3 is version 2 with these changes:
  - The format version is 3. The table count is 28.
  - `SYMBOLS` (6) rows are 36 bytes: after the version str, `at_file`
    and `at_index`, two `u32`. A local's are its own `FILES` row (equal
    to `file`) and its index in that file's symbol table. A global's are
    the file of its **first mention** (inputs rule 6: the first live
    object, in load order, whose symbol table names the key) and the
    entry's position among that file's non-local entries (its index
    minus `sh_info`). A key no object mentions (linker-defined, a
    `--defsym`) has `at_file` `NONE` and `at_index` 0.
  - A new table, kind 40, `FRAG_AT`, comes after `HOLES` and before
    `STRINGS`: section `SECTIONS` row, offset `u32` (the fragment's
    offset in its input section), `FRAGS` row `u64` (16); one per
    fragment of a split section, strictly ascending by (section,
    offset). It is `FRAGS` keyed by where a fragment was read, not by
    what it holds.

  A state of version 2 is refused by name (`state-version`, `format
  version 2, this mortar reads 3`) and gives a full link that writes a
  version-3 state; no compatibility path reads it. The reader adds these
  checks to IN-18a2's: a local row's `at_file` equals its `file`; a
  global's `at_file` is a `FILES` row or `NONE`, and `at_index` is 0 when
  it is `NONE`; `FRAG_AT`'s section is in range, its `FRAGS` row is in
  range, and its rows strictly ascend. Cost (WP-094, measured): the
  state grows 5.6% on protoc `-g` (29.4 to 31.0 MB) and 10.8% on
  `synthetic-objects-g10x` (364.7 to 404.2 MB), `FRAG_AT` being most
  of it; a plain link writes no state and pays nothing (IN-11).
  *Test:* `mortar_incr`'s round trip and validation tests; the
  incremental tests' replay; `the_floor_update_is_the_full_link_of_the_edited_inputs`
  (the update maps by position).
- **IN-18a4 [v1] Format version 4: position-exact rows** (WP-064a,
  for IN-28a as amended). Version 4 is version 3 with these changes:
  - The format version is 4.
  - A `FILES` row has flags: its byte 2 (version 3's first pad byte) is
    `flags u8`, byte 3 stays pad; the row is still 60 bytes. Bit 0 is
    `POSITIONAL` (`mortar_incr::format::file_flags`): set exactly when
    the row's position rows are position-exact (IN-28a). A full link
    sets it on every row; the test path's update (WP-064a) recaptures
    the changed file's rows and keeps it, or clears it when the file's
    sections were permuted. The reader refuses a row with any other bit
    set (`state-corrupt`).
  - `FRAG_AT`'s third field is the `FRAGS` row (`u64`), as IN-18a3
    always said; the version-3 writer stored the fragment's offset in
    its output section instead, and the reader did not check it. The
    reader checks it in range.
  - A file's `SECTIONS` rows are one contiguous range in section-index
    order.

  A state of version 3 is refused by name (`state-version`, `format
  version 3, this mortar reads 4`) and gives a full link that writes a
  version-4 state. The ranged state of WP-105 (block hashes, the
  per-file index) is a later version, 5. *Test:* `crates/mortar/tests/incr_review.rs`:
  `out_of_range_positions_are_refused_by_name` (a flag other than
  `POSITIONAL`, a `FRAG_AT` row naming no `FRAGS` row: `state-corrupt`,
  in both translations), `a_version_3_state_is_refused_by_name`.
- **IN-18c [v1] Format version 5: the state read by range** (WP-105 §4's
  text, which WP-202 adopts; the prototype is `mortar_incr::ranged` on
  `wp/105-incr-state-by-range`, there numbered 4). The state is
  block-hashed: its header (magic `MORTARSR`, format version 5, the
  flags word, table count, block size, length, plan hash) carries the
  L128 of a directory with one entry per table (kind, row width, rows,
  capacity, body range, and the L128 of the table's block-hash list),
  and each table's body is covered by a list of L128 hashes, one per
  block of the body (4 KiB; WP-105 measured 4, 16 and 64 KiB). A reader
  checks the header and the directory in full, then reads only the
  blocks it uses, by positioned reads (no mmap), and hashes each block
  against its list entry, and the list against its directory entry,
  **before** any byte of the block is used; every row index, range and
  string offset it follows is checked against the directory. A block or
  list that fails its hash, a range out of bounds, or a reference out of
  range is `state-corrupt` for the whole state: a full link that writes a
  new state, never a panic, never an update from a partly trusted
  state. The checks of IN-18a2/IN-18a3 that span a table are made over
  the ranges read; what an update does not read it does not trust and
  does not use. The flags word (bit 0 "updating", IN-77) is the only
  field outside every hash. A format-4 state is `state-version`.
  Measured (WP-105 §4, the prototype, warm cache): an update's reads
  are 6.1 MB in 3.1 ms on g10x and 8.0 MB in 4.1 ms on `cxx-large-g`,
  against 404 and 247 MB read whole in 1.6 and 1.0 s. **An update reads
  and verifies only the header, the directory and the blocks it touches**
  (the blocks of the rows it reads, and those W5 patches, IN-46b);
  **reading or validating the whole state in an update is a defect**
  (WP-211, review-wp207 §7: the whole read alone, 201 ms on `cxx-large-g`
  and 562 ms on g10x, exceeds protoc's whole budget, so the go/no-go gate
  of §10 would be missed by design). The only whole reads are the
  compaction (IN-18f), `incr_state` and the tests' `canon`, and two
  named reads of `FILES` whole (WP-214 seam 6): the property pass of
  IN-72 item 10 (only when a file of the change differs in `FILES.stack`
  or `FILES.properties` from its stored row, appears or goes) and
  `LayoutRows::eh_ranges` (only for IN-34a's whole rewrite of
  `.eh_frame`, itself O(records)); a
  `StateView` is built lazily, per block. The update's own `Placement`
  (IN-72's `LayoutDelta`) is sparse in the same way (WP-216 R-3): it
  names the files whose sections it places and the chunks whose holes it
  replaces; any other section's place and chunk's holes are the stored
  rows, read by key through one reader (`LayoutDelta::place_of`,
  `holes_of`), never copied whole into it. *Test (U-2):*
  `update_reads_only_touched_blocks` (the reader's block count for a
  one-file edit on the large fixture is O(the delta's rows), and a
  flipped byte in an untouched block does not stop the update).
- **IN-18d [v1] The per-file index.** `FILE_INDEX` (kind 41), one row
  per `FILES` row: for each file-keyed table (IN-18e) the first row, the
  count and the capacity of the file's range, a `gone` flag (a file
  removed by an update: its ranges are dead until the next compaction),
  and `first_global` (`u32`: its `.symtab`'s `sh_info`, 0 for a file
  without one; the index of its `MENTIONS` rows, IN-18e; WP-215 D7), and
  `locals` (`first`, `count`: `u32` each, 8 bytes; WP-214 seam 6): the
  file's local keys, ONE contiguous run of `SYMBOLS` rows, by
  symbol-table index. The capture writes a file's locals together
  (IN-18e's row order); an update that replaces a file's symbol table
  (`Changed`, `Appeared`, `Extracted`, `NoLongerExtracted`:
  `FileRanges::locals` is `Some`) appends its new locals as one block,
  names it here, and marks every row of the old run `GONE`; a gone
  file's old run is marked `GONE`; a drawn-in or only moved file keeps
  its run (`None`). That costs O(the file's locals), never a pass over
  `SYMBOLS`. The compaction recomputes the runs.
  The reader checks that the ranges are in bounds and do not overlap.
- **IN-18e [v1] Format version 5: every table, its class and its patch
  rule** (rewritten by WP-207; review-in77 B-5, M-9). Every table of the
  format is in exactly one of five classes. "Capacity" is rows the body
  holds; unused rows are zero. A patch (W5, IN-46b) that does not fit a
  table's capacity, or its class's bound, is the compaction (IN-18f).

  | class | tables (kind) | how an update changes it |
  |---|---|---|
  | **whole** (small; capacity = rows + max(64, rows/8)) | `META` (1), `PROBES` (4), `OUTSECS` (5), `CHUNKS` (8), `ORDER` (9), `SEGMENTS` (10), `SLOTS` (12), `SIZES` (15), `SYM_PLACE` (16), `DEFINED` (17), `PATCHES` (18), `REDIRECTS` (19), `SIZED` (20), `LAYOUT` (21), `LEAVES` (22) | the table is written again whole into its capacity when any row differs (its blocks only; the rest of the state is untouched); over capacity: compaction. Together under 2 MB on g10x (WP-105 §3's "the rest" and `CHUNKS`). `LEAVES`' row count follows the output's size (W3 truncates or extends), within its capacity |
  | **append-only** | `FILES` (3), `STAT` (2), `FILE_INDEX` (41), `STRINGS` (23) | rows (bytes, for `STRINGS`) are only appended, into a reserved append area of max(4,096 rows, rows/64); a row is never moved, so references to it stay valid. `FILES` and `FILE_INDEX` are parallel (one row each per file, members of archives included, `LAZY` when not extracted); `STAT` has one row per file plus the device rows; a file's load-order position is its `FILES.rank` (`u64`): the first link writes `rank = FileId << 20`, an appeared or newly extracted file takes the integer midpoint of its neighbours' ranks (none left: compaction), and `FileId` order is `rank` order. A gone file's row is marked `GONE`. A string is never overwritten or reused; strings no row names any more are dead until compaction. The build id is in `META` (a length and 64 bytes), not in `STRINGS`, so an update appends no string for it |
  | **file-keyed** (one range per file through `FILE_INDEX`, with row slack: capacity = count + max(4, count/8)) | `SECTIONS` (7), `MENTIONS` (32), `INDEX` (34), `EDGES` (42: one row per stored edge, 17 bytes: kind `u8`, source `u32`, target `u32`, offset `u64`), `FRAG_USE` (56: one row per (file, fragment its live split pieces name), 28 bytes, below; WP-239), `SYMTAB` (43: per file its local block and `.strtab` region; file 0's range holds the global area's row), `REGIONS` (51: per file its regions, one row per (output section, category, ordinal): the chunk, offset and capacity; IN-72 item 2, WP-213) | a changed, drawn-in or appeared file's range is rewritten in place when the new count fits its capacity; a file's `SECTIONS` rows are also rewritten when IN-72 moved, killed or revived its sections (IN-78); a gone file's ranges are dead (`FILE_INDEX.gone`); a new file gets new ranges in the table's append area (max(4,096 rows, rows/64)); no room: compaction |
  | **key-keyed** (sorted body + append area of max(4,096 rows, rows/64), kept sorted on insert + tombstones) | `KEY_REFS` (45: key, file, attributes, `demand`), `GROUPS` (44: by (signature, file): one row per live relocatable object's copy; the keeper is the copy whose file has the least `FILES.rank` (resolve.md §6.1; never the least row number: `FILES` is append-only), not stored; WP-215), `FRAGS` (11: by (out, class, hash), offset, `refs`; below), `HOLES` (39: by (chunk, offset)), `COMMONS` (46: by (key, file): one row per live file with a common mention of the key, its size and alignment (`max(1, st_value)`, as `RESOLVED.value` of a common); the winning common (resolve.md §3.3: the file of least `FILES.rank`, the largest size and alignment) is computed from the rows, not stored; WP-215), `ODR_CLASSES` (47: ODR key, class hash and layout; PROVISIONAL, not written until WP-236), `ODR_REFS` (48: ODR key, file, row kind, class; PROVISIONAL, not written until WP-236), `COPIES` (55: by (key, file, symbol index): one row per global definition in a section of a live COMDAT copy, its `st_size` and `st_type`, 24 bytes, for every input unless `--odr=off`; the copy's group is its `MENTIONS` row's section's `SECTIONS.anchor` (`anchor_kind` 1, the signature), not stored; no rank stored: W1 and W2 order copies by the files' current `FILES.rank`; kinds 53 and 54 are reserved for WP-236; WP-220), `DIAGS` (49: subjects, code, text, order), `NAMES` (52: the by-name index of `SYMBOLS`' globals, 12 bytes, keyed by (hash, row): `hash` is model.md's 64-bit hash (`mortar_core::hash::hash_bytes`) of the bytes of `SYMBOLS.name`, one zero byte and the bytes of `SYMBOLS.version` (empty when unversioned), as `STRINGS` holds them; `row` the `SYMBOLS` row; a lookup compares the name and version bytes of every row of the hash, so a hash never identifies a key alone; a `GONE` key keeps its row, and a key that comes back takes its old `SYMBOLS` row; WP-213) | a lookup binary-searches the body and the append area (the area is kept sorted: a new row is inserted into it at its place, moving at most the area's rows; WP-211, review-wp207 m-3: no linear scan of up to 4,096 rows per lookup); a deleted row is a tombstone (its key kept, a flag set); a changed row is changed in place when its key is unchanged, else deleted and appended. Compaction when an append area is full or tombstones pass 25% of a table's rows |
  | **key-parallel** (one row per `SYMBOLS` row, in its order) | `SYMBOLS` (6), `RESOLVED` (33), `SYMTAB_KEYS` (50: a global's `.symtab` index and `.strtab` offset) | a key's rows are changed in place by its `SYMBOLS` row number; a new key appends a row to each (append area of max(4,096 rows, rows/64)); a key no file mentions any more is a tombstone (`SYMBOLS.kind` `GONE`) |

  Table 14 (`SYMTAB_LOCALS`) is gone (`SYMTAB` and `SYMTAB_KEYS` replace
  it); `LEAVES` no longer keeps its row count ("patched by block, same row
  count" is withdrawn); `RESOLVED`'s kind-6 and kind-7 rows are gone:
  groups are `GROUPS`, and a file's extraction is its `FILES` flags and
  **round** (IN-71b). A
  table not in this list is a format defect: the reader refuses an
  unknown kind (`state-corrupt`).

  **`KEY_REFS`** holds **one row per (key, mentioning file)**: every key,
  including keys one file mentions (review-in77 B-4), file 0's
  linker-defined keys (file 0 is the definer of each; every file that
  mentions one has a row, review-in72 B-2), and the defined and common
  mentions of archive members not extracted (IN-71b). A row is 14 bytes:
  key (`u32`, the `SYMBOLS` row), file (`u32`, the `FILES` row), the
  attributes resolution reads (definedness, binding, type, visibility,
  common, section or absolute, `LAZY`), and **`demand`** (`u16`, WP-239
  S4-4): the OR of the `DemandFlags` the scan raised on the key over the
  file's live units (counted per file; `INDEX` is never the demand). It is `MENTIONS` transposed, so
  its row count equals `MENTIONS`' (plus nothing: the lazy members'
  defined mentions are `MENTIONS` rows of their files too).
  **`at_file`** is maintained by IN-71's rule: the first LIVE
  mentioner, by the files' **current** `FILES.rank` (never by `FILES`
  row number, never a file that is gone, dead or an archive member not
  extracted: such a member's mentions decide nothing stored), and
  `at_index` its entry among that file's non-local entries; `NONE` and 0
  when no live file mentions the key. The full link's capture writes
  the same rule (WP-214 F-12: today's format-4 capture takes the first
  mention among ALL loaded files; the format-5 capture from a link
  changes to this rule). The output does not read `at_file`: `.symtab`'s
  globals are in `SymbolId` order (synth.md S-75; read from the code,
  not compared on bytes); `at_file` orders `SYMBOLS`' rows (IN-18e) and
  places a positional replay.

  **Items 5, 6, 7's data** (WP-239, ruled; the arithmetic and the
  experiments in `spec/landed/incr/WP-239.md`):
  - **`FRAGS`** is keyed **(out, class, hash)**: `out` the `OUTSECS` row,
    `class` the merge class word `strings << 31 | align_log2 << 24 |
    entsize` (`mortar_core::incr::MergeClass::word`), `hash` the L128 of
    the bytes; `refs` is the number of files with a `FRAG_USE` row naming
    it. (P-1) Two different fragments with one key refuse the STATE,
    never the link: the capture, which sorts the rows by key, sees them
    adjacent and writes no state (`state-collision`, §7).
  - **`FRAG_USE`** (kind 56, file-keyed): one row per (file, fragment its
    live split pieces name), `{file, out, class, hash}`, 28 bytes,
    referring to the fragment by its key, never by a `FRAGS` row number.
    (P-2) **No split section named `.debug_*` has a row in `FRAGS` or
    `FRAG_USE`** (R-1: every file whose debug contribution changes is
    `update-not-implemented (debug)`; IN-56's needs are G-1's). `.comment`
    has rows. Format 4's `FRAG_AT` (kind 40) is not in format 5; kind 40
    is never reused.
  - **`FILES`** gains three `u32` columns, the file's `.eh_frame` range:
    `eh_offset` (bytes from the `.eh_frame` chunk's start),
    `eh_capacity` (records and pad), `eh_records` (kept records; 0: no
    range). Record sizes are not stored (IN-34a). A chunk past `u32`
    refuses the state (`state-limit`, §7). A fourth, `eh_fdes` (`u32`,
    WP-243 via seam 6): the FDEs among the records, written wherever
    `eh_records` is; `.eh_frame_hdr`'s new size is the stored chunk's
    FDE count minus the changed files' stored `eh_fdes` plus their new
    ones (IN-72 item 5).
  - **`FILES`** gains what the full link's two note merges read from ONE
    file (WP-214 seam 6, ruling 3): `stack` (`u8`: the file's
    `.note.GNU-stack` as layout.md L-84 reads it, 0 absent, 1 without
    and 2 with `SHF_EXECINSTR`) and `properties` (a `Str`, 8 bytes: the
    file's own `.note.gnu.property` properties after synth.md S-126, each
    `pr_type` (`u32`), length (`u32`) and value, ascending by type,
    `update::rows::encode_properties`; empty for none; the capture
    interns each distinct value once). 0 and empty for a file that is
    not a relocatable object or is `LAZY`. IN-72 item 10 reads them.
  - **`FRAGS`** gains `size` (`u32`, 4 bytes; WP-242 C via seam 6): the
    fragment's bytes, a function of its identity (a dead string
    fragment's hole is this long).
  - **`SLOTS`** gains `refs` (`u32`): the live files whose
    `KEY_REFS.demand` needs the slot (a local's slot, IN-28b: 1).
  - `EH_RECORDS` (kind 13) is not in format 5 (WP-214, third part): the
    `FILES` eh columns replace it; the format-4 replay reads its own.
  - Measured (WP-239 §8, against U-2's format 5): protoc 1.241 → 1.112,
    `cxx-large-g` 1.130 → 0.674 of format 4. As built (WP-214 §10, with
    a stand-in `KEY_REFS` row per LAZY definition): 1.117 and 0.664.

  **`MENTIONS` by position** (WP-215 D7): a file's `MENTIONS` range holds
  **one row per non-local entry of its symbol table, in the table's
  order, none skipped; the entry's index is the file's
  `FILE_INDEX.first_global` plus the row's position in the file's
  range**. (Format 4's capture sorts a file's rows by key and drops
  repeats; format 5's does neither.) **`META`** (WP-215 D1, D8) gains
  `iterations` (`u32`: the iterations of resolve.md §4.1's closure in the
  full link that wrote the state; IN-71b Iterations) and `versioned`
  (`u32`: the non-local symbol-table entries named `foo@V` or `foo@@V`
  over the link's relocatable objects and archive members, extracted or
  not; IN-71 Versions): 8 bytes. What the command line gives (the root
  keys, `--wrap`, `--defsym`) is passed in, never stored (IN-71), with one
  exception (WP-214 seam 6, ruling 2): **`SYMBOLS.option`**, a bit in
  byte 1 of the row (one of format 4's three pad bytes after `kind`: 0
  bytes), set by the capture on the row of every name
  `mortar_core::plan::Plan::option_keys` gives (`--wrap`'s three names,
  `--defsym`'s name and base: the names that have a row whether or not a
  file mentions them, model.md §2.3) and never changed by an update (a
  state is bound to its command line, `options-changed`; a key with the
  bit is never `GONE`). The dead-key rule of the canon, the compaction
  and `incr_state` reads the bit and needs no command line. `-e`, `-u`,
  `EXTERN` and `--require-defined` give no row (a name no file mentions
  has none in a full link either) and so no bit.

  **Shrinking.** `MENTIONS` drops its `file` column (the range gives
  it): 12 bytes a row. `RESOLVED` drops `symbol` (its row is the key's),
  `file` (the section's row gives it; a common's or an absolute's file is
  `KEY_REFS`'s definer row) and `name` (`GROUPS` holds the signatures): 24
  bytes a row. `INDEX` is grouped by referring file and packed to 9 bytes
  a row.

  **`SYMBOLS`' row order** (WP-211, review-wp207 M-2; one order, used
  by the capture of every full link and by the compaction alike):
  first the globals (kinds 0 and 1) by (`FILES.rank` of the first
  mentioning file, `at_index`); then the keys no file mentions (file 0's
  unreferenced linker-defined keys, keys only a slot or a `--defsym`
  names), by name as bytes; then the locals (kind 2) by (their file's
  `FILES.rank`, symbol-table index). New keys of an update are appended
  (key-parallel) and take this order again at the next compaction.
  Today's `capture.rs` numbers rows in first-call order (slots first,
  locals interleaved, then `SYM_PLACE`, `DEFINED`, patches and
  `MENTIONS`): **U-2 changes the capture to this order** (it owns
  `capture.rs`); the order is a function of the inputs, so a full link's
  state stays deterministic (IN-6). *Test (U-2):*
  `symbols_row_order_is_the_stated_one`, and
  `compaction_equals_full_link_capture` with it.

  **Size, restated** (*estimate* over WP-105 §3's measured rows, the
  arithmetic in `spec/landed/incr/WP-207.md` §M.6; review-in77 M-9):

  | | g10x | `cxx-large-g` |
  |---|---|---|
  | format 4 (measured) | 404.2 MB | 246.6 MB |
  | `MENTIONS`, `RESOLVED`, `INDEX` shrunk | −51.7 | −7.4 |
  | `KEY_REFS` (12 B × `MENTIONS` rows) | +51.7 | +3.6 |
  | `SYMTAB_KEYS` (8 B a key) | +13.2 | +0.9 |
  | `EDGES` (`--gc-sections` only; 17.5 B a section, from protoc's 0.48 MB) | +3.2 | +9.9 |
  | row slack (12.5% of the file-keyed tables) | +16.6 | +12.9 |
  | append areas (1/64 of key-keyed and `STRINGS`; 1% used here) | +2.9 | +1.5 |
  | block lists (16 B a 4 KiB block) and `FILE_INDEX` | +2.4 | +1.3 |
  | **format 5** | **442.5 MB (+9.5%)** | **269.3 MB (+9.2%)** |

  `GROUPS` takes `RESOLVED`'s kind-6 rows, plus one row for each further
  live copy (WP-215: by (signature, file); U-2 measures it); `COMMONS`,
  the `FILES` round and `DIAGS` are under 1 MB here; the `ODR_*` record
  tables are not written (WP-236); `COPIES` (W1 and W2, every input
  unless `--odr=off`) is 1.34 MB on protoc and 2.35 MB on `cxx-large-g`
  (measured counts × 24 B, WP-220 §F.1; g10x's generator makes no COMDAT
  group) and is part of U-2's gate. WP-202's
  "at most 150 MB against 167.4" compared three tables and left out the
  slack and the new tables; it is withdrawn. **U-2's gate**: the format-5
  state measured on g10x and `cxx-large-g` is at most 1.15 × format 4's
  measured total (465 and 284 MB), and U-2 reports the per-table sizes
  against this table (`state5_size_report`); a state above it goes to
  the supervisor with the table, it is not trimmed silently.

- **IN-18f [v1] The compaction** (WP-207; review-in77 B-5). An algorithm,
  not "the whole new state":
  1. Read every block of `S` through U1's descriptor and check it against
     its block-list entry and the list against the directory (IN-18c);
     any failure is `state-corrupt` (before W1: the update does not run;
     the full link that follows writes a fresh state). O(state): WP-105
     measured the whole read and validation at 562 ms on g10x and 201 ms
     on `cxx-large-g`.
  2. Apply the update's patch to the rows in memory (what W5 would write).
  3. Drop: `GONE` files' `FILES`, `STAT`, `FILE_INDEX` rows and their
     ranges; tombstones of every key-keyed and key-parallel table; slack
     and append areas; strings no remaining row names.
  4. Renumber, through one old→new map per referenced table, built in
     the order the full link's capture numbers rows: `FILES` by `rank`
     (new `FileId` = position), `SYMBOLS` in **IN-18e's one row order**
     (below; WP-211, review-wp207 M-2), `SECTIONS` by (file, index),
     `FRAGS` by identity, `CHUNKS` by (output section, ordinal, part),
     `STRINGS` offsets by the new pool. Every reference (`FRAG_AT` →
     `FRAGS`; `INDEX`, `MENTIONS`, `SLOTS`, `DEFINED`, `SYM_PLACE`,
     `KEY_REFS` → `SYMBOLS`; everything → `SECTIONS`, `FILES`, `STRINGS`)
     goes through its map; a reference with no image in a map is a
     defect (`state-corrupt` in a test build, a panic never).
  5. Write the tables in kind order with fresh slack (IN-18e's
     capacities), sort the key-keyed bodies, rebuild the block lists, the
     directory and the header (flag clear), to `dir(S)/.<base(S)>.mortar-compact`, then
     IN-77's rename.
  The result is, byte for byte, the state the full link of the update's
  inputs writes when its layout is the update's `Placement` (IN-78's
  `C(U)`), except `META`'s history fields; every full link writes a state
  that is compact by construction. *Test (U-2):*
  `compaction_equals_full_link_capture` (after a series of patches, the
  compacted state's bytes equal `C(U)`'s); `compaction_reads_every_block`
  (a flipped byte in a block no patch touches: `state-corrupt`);
  `rank_midpoint_exhausted_compacts`.

- **IN-46b [v1] The state patched in place.** W5 of IN-77 writes only
  the blocks whose rows changed, their block-list entries, the
  directory entries and the header; rows inserted go into a file's row
  slack or a table's append area (IN-18e); when neither has room the
  whole state is rewritten (the compaction, IN-77), and
  `--incremental-report` says so (`delta state compacted <reason>`),
  with the compaction's time on its own line, apart from the update's
  (WP-211: the bench and U-10's gate count a compaction separately, at
  its amortized rate, review-wp207 §7).
  **Verified before re-hashed** (WP-207, review-in77 M-4): every block
  W5 writes is read and checked against its current block-list entry
  (IN-18c) before it is modified, in U-steps before W1 (the update reads
  the blocks it will patch anyway, to modify their rows); a block that
  fails is `state-corrupt` before W1, and the update does not run. Two
  calls (WP-213): `state5::plan_patch` in U9 makes every read, check and
  decision (in place or compaction, IN-18f step 1's read included) and
  returns an opaque plan; `state5::patch` or `state5::compact` at W5
  writes the plan with W4's binding and reads nothing. A new
  hash therefore only ever vouches for bytes an old hash vouched for,
  plus the update's own rows. The block list and directory entries W5
  rewrites are those of verified blocks only. *Test (U-2):*
  `patched_block_is_verified_first`.
  Measured on the prototype (WP-105 §4, same-length patches of one
  file): 21.6 KB written in 191 ms on g10x and 23.4 KB in 154 ms on
  `cxx-large-g`, the three `fdatasync`s under load being the time; a
  compaction is the full encode and write, 0.38-0.80 s on g10x and
  0.27-0.47 s on `cxx-large-g` (WP-105 §4, format 3).
- **IN-18b [v1] The normalized plan.** The plan hash is `L128` of a
  sequence of items, each a tag byte followed by fields that are each a
  `u32` length and that many bytes: first `C` with the working
  directory; then, by argument index (after `@file` expansion), `O`
  with the option row's canonical name (its first spelling) and its
  value (empty when it takes none) for every option use whose row is not
  excluded, and `I` with the path of every input named by a path (an
  argument that is not an option). Excluded rows touch no output byte:
  `-Map`, `-M`/`--print-map`, `--mortar-map`, `--time-phases`,
  `--threads`, `--thread-count`, `--no-threads`, `--dependency-file`,
  every `--incremental` row (IN-10), and every row the option table
  accepts and ignores (status `noop`, `docs/status/options.md`: its
  audit links it to the same bytes as without it). The last class is
  why a compiler driver's second link finds its state: gcc passes
  `-plugin-opt=-fresolution=/tmp/ccXXXXXX.res`, a new name on every run
  (the WP-061 review's F1). Any other difference of the
  command line, its spelling included, is `options-changed`: conservative
  (`-o x` and `--output=x` hash alike, being one row, but two orders of
  the same options do not). *Test:* a link test: `--time-phases` and
  `--threads` added keep the plan; `-O1` added changes it;
  `a_compiler_driver_link_twice_finds_its_state` links twice through
  gcc, g++, gfortran and clang with `-B` naming mortar as `ld`, and the
  second link finds the first's state.

### 5.4 Change detection

- **IN-22a [v1] Stamps.** A **stamp** on a file system is the ctime of a
  file the link creates there (`<dir>/.mortar-stamp-<pid>`, created,
  `fstat`ed and unlinked; measured 8–10 µs on overlay, 0.3–0.5 ms on
  virtiofs, R6; the name is one per process and directory, so a process
  that runs several links takes its stamps one at a time). Before it `stat`s any input, a link in incremental
  mode takes one stamp on each file system (`st_dev`) that holds an
  input, in a writable directory of that file system among those it
  knows (the output's, the state's, each input's, `TMPDIR`), and records
  it in the state per device. A file system on which no stamp can be
  created is **unstampable**; its inputs are hashed on every update
  (IN-22). Timestamps are compared only with stamps of their own file
  system, never with a clock: file systems have their own timestamp
  granularity (1 ms on this host's overlay, 82 µs on its virtiofs), and a
  wall clock is finer than either.

  **Stamps are trusted only on an allow-list, after a probe** (the
  re-review's N5). A file system's type is read from
  `/proc/self/mountinfo` (the mount whose `major:minor` is the input's
  `st_dev`; field "filesystem type"), not from `statfs`: on this host
  `statfs` names the workspace's virtiofs `fuse` (virtiofs uses
  `FUSE_SUPER_MAGIC`), the same as sshfs, while mountinfo names it
  `virtiofs` (measured, `stamp_probe.py`); reading a text file also needs
  no `unsafe` (std has no `statfs`). **The allow-list is the measured
  types: `overlay`, `virtiofs`, `tmpfs`.** ext4, xfs and btrfs keep
  nanosecond ctimes that change on every data write and are expected to
  pass, but are not measured here; they join the list when R6 and the
  probe run on them (WP-061), not before. On a type off the list the
  file system's inputs are **always hashed**. Named as not trusted, by
  argument, not by reproduction here: **FUSE** in general (`fuse`,
  `fuseblk`, and so sshfs): ctime is whatever the user-space server
  reports, and with attribute caching (sshfs `cache=yes`, the kernel's
  `attr_timeout`) a stale ctime can be served after a rewrite; **sshfs**
  in addition takes timestamps from the remote server's clock and
  file system, at one-second granularity over SFTP v3; **vfat** has no
  ctime at all (the kernel reports the modification time, 2-second
  granularity) and **exFAT** keeps 10 ms timestamps without a separate
  change time, so a same-size rewrite with the time set back (`touch -d`,
  `cp -p`) is not seen. NFS is off the list too (attribute caching;
  not measured).

  **The probe**, once per file system when the state is created and
  again when its type or `st_dev` changes: create the stamp file and
  `fstat` it (c0); set its mtime one day back with `utimensat`, `fstat`
  (c1); `pwrite` one byte of the same size at offset 0, restore the
  mtime, `fstat` (c2). It passes iff c1 ≥ c0 and c2 > c1 (ctime moves
  forward on a metadata change and strictly forward on a data write made
  after one, regardless of mtime). Measured (`stamp_probe.py`, load ~13):
  overlay 0.29–1.99 ms, virtiofs 2.25–3.90 ms, tmpfs 0.18–0.49 ms; it
  passed on all three. A failed probe records the file system as
  always hashed in the state.

  **Cost of always hashing** (n9): a file system off the list, or read
  only (no stamp can be created there: a system image, a Nix store, a
  read-only bind mount), has its inputs read and hashed on every update.
  Measured for the usual case, the system archives of a C++ link
  (`libstdc++.a` 6.6 MB, `libc.a` 5.6 MB, `libgcc.a` 3.2 MB,
  `libgcc_eh.a`, `libm.a`; 15.5 MB): 6.9–10.8 ms single-threaded, load
  ~13. A size + mtime stamp is **not** accepted on read-only mounts in
  v1: an image or a package can be replaced at the same mount point
  (an upgrade, a new container layer) with an equal `(size, mtime)` and,
  across a remount, an equal `st_dev`/inode, and nothing in the tuple
  tells the two apart (argued). The 7–11 ms is the price of not
  trusting that; a content-addressed store whose path names the content
  (Nix) is a [later] exception.

  **Where the stamps are taken** (WP-061): before the input phase, in
  this order of directories, the first writable one of each `st_dev`
  wins: the output's directory, the state's, `TMPDIR` (else `/tmp`), the
  working directory, then the directories of the plan's path inputs and
  `-L` directories. A file opened on a device with no stamp row is
  hashed. In `STAT` a device row's trust is 0 when its type is on the
  list and the probe passed, else 1 (always hashed); an input row's
  trust is 0 when its content hash is recorded, 1 when none could be
  (it is then changed on every update); the output row's `stamp_ns` is
  its reference stamp (IN-21). The device's major and minor numbers for
  mountinfo are Linux's (glibc's `major`, `minor`): major `(dev >> 8)
  & 0xfff | (dev >> 32) & 0xffff_f000`, minor `dev & 0xff | (dev >> 12)
  & 0xffff_ff00`.

  *Test:* `stamp_rule.py` (R6) with the state and the inputs on the same
  and on different file systems; `stamp_probe.py` on each allow-listed
  type; a unit test that maps a mountinfo line to its type and an
  off-list type (`fuse`, `vfat`) to "always hashed".
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
  hashes are IN-31's, [v2]. **The content hash** of a file or of an
  archive member's bytes is `L128(le64(n) ‖ L128(b_0) ‖ … ‖
  L128(b_{k−1}))`, where `n` is the byte count and `b_i` the
  consecutive 4 MiB blocks (the last one shorter; none when `n = 0`): a
  tree of one level, so that blocks hash in parallel as they are read
  (`spec/input.md` §11.6's store computes it). `--incremental-verify` hashes every input
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
  stat'ed as such (IN-22). A member file's `STAT` path is its own while
  its `FILES` row's path is the archive's (model §7.1), so change
  detection ties the two by content: a read path whose bytes are a
  member row's whole range (an object, base 0, the same length and
  content hash) and whose last components are the row's member name is
  that member's file, and its change is judged by the member's row, not
  taken for a script's (§7 `options-changed`; WP-106). A path that
  matches no member row stays a script's (conservative). Measured (R1, the "detect" part): reading and
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
  index is model §7.2's. The liveness graph is stored beside it, by
  IN-29's own rule (the index's rows are not assumed to be that graph). End symbols (`__init_array_end`, `_end`,
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
  **Symbol attributes are part of the delta too** (the re-review's N1):
  for every global key a file defines or references, the state stores
  (IN-19 already has the columns) the definition's type (`STT_*`: FUNC,
  GNU_IFUNC, OBJECT, TLS, NOTYPE, COMMON), binding, visibility and the
  rest of `st_other` (e.g. AArch64's `STO_AARCH64_VARIANT_PCS`), and
  whether it is absolute, common or in a section. **v1 rule: a change in
  any of these, for a key that an unchanged file references, is a
  full link named `symbol-kind-changed`**, not a refill: an unchanged
  referrer's relocation types are not in the state, so the update cannot
  recompute the decisions that depend on them (an IPLT entry for a
  FUNC → IFUNC change, preemptibility and GOT/PLT choices for binding
  and visibility on dynamic outputs). A change for a key that only
  changed files reference needs nothing beyond their refill. How often
  it fires is not measured: the corpus's edits e1–e10 (§1.1) contain no
  type, binding or visibility edit of a surviving symbol, so it would
  fire 0 times on them (derived from the edit definitions, not from the
  objects); a renamed or added function is a key that appears, not a
  change of attributes.
  *Test:* L10 edit classes that move a definition between files, change
  a COMDAT keeper (edit the header of the lowest file's group), and pull
  a new archive member in; and the named case `ifunc-type` (the
  re-review's `re/ifunc_type.sh`: `f` turns from `STT_FUNC` into
  `STT_GNU_IFUNC` in `c.o` with the same definer, value and size while an
  unchanged `u.o` calls it; an emulated update that keeps `u.o`'s bytes
  prints `f(5)=4423704`, the full link `f(5)=10`), which must report
  `symbol-kind-changed` and print `f(5)=10`; one L10 edit per attribute
  (binding weak ↔ global, visibility default ↔ hidden, the variant-PCS
  bit, a definition turned absolute or common).
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
  | the value synth.md S-50 gives a reference into an input `.eh_frame` from another section changed (IN-34a, "References into `.eh_frame`") | the referring unit |
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
  which row each catches). The final round's by-value oracle adds N1's
  FUNC → IFUNC pair, a weak undefined that becomes defined, the LSDA
  pair and the gc edge-kind pair (`gc-edge-kinds`, IN-29), on AArch64 and
  x86-64: **56 of 56 pass** (IN-63b). The oracle compares two *full* links, so its
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
- **IN-28a [v1] Unchanged files translate by position** (WP-094;
  amended after the adversarial review of 2026-11-17,
  `spec/landed/incr/WP-064a.md`: the first text let a STALE state
  through, a silent mislink on the second update). The translation of
  IN-28 reads every input's symbol table and section names to build
  identities (model §7.1); measured, it was the largest term of an
  update (WP-061b: 74 ms on protoc `-g`, 2.3-2.7 s on
  `synthetic-objects-g10x`). An update translates the rows of some
  files by position instead, under the rules below. Position is only
  ever position inside one file: `FILES` rows are matched to the new
  files by file identity, never by row number.

  **Position rows.** A `FILES` row's position rows are its `SECTIONS`
  rows, the `at_index` of its local `SYMBOLS` rows and of the global
  rows whose `at_file` it is, and its `FRAG_AT` rows.

  **Position-exact (the writer's duty).** A `FILES` row carries the
  `POSITIONAL` bit (IN-18a4) exactly when ALL its position rows were
  captured from the bytes whose content hash the row holds, by the link
  that wrote that hash. EVERY writer of a state (a full link, an
  update, a patch of the state by range) that writes a file's hash
  either recaptures all of that file's position rows from the same
  bytes, in the same write, or clears the bit. There is no third case:
  a writer that changes a hash and leaves a position row is the defect
  the review found (the next update saw the file as unchanged and
  mapped its stale rows). A file's `SECTIONS` rows are written as one
  contiguous range in section-index order, one row per section header
  (index 0 included or excluded as the writer does today, but the same
  for every file: the count the reader compares is the writer's count
  for `e_shnum`).

  **The positional set (the reader's decision).** An update decides the
  set from `FILES`, `STAT` and the stamps BEFORE it builds any
  identity. A `FILES` row is in the set iff all of these hold:
  1. its `POSITIONAL` bit is set;
  2. the file it maps to by file identity is unchanged by IN-22 (clean
     by its stamp, or hashed equal), and the update uses no bytes of
     that file other than the bytes so judged (it does not read the
     file again after judging it, or it verifies what it read: the
     descriptor's `fstat` after the read equals the judged tuple);
  3. the new entry's path, base and length equal the row's (a member
     that moved inside a regular archive, or whose archive was
     rewritten around it, is translated by identity);
  4. its `SECTIONS` rows are one contiguous range of exactly the
     count the writer stores for the file's `e_shnum` (no rows only
     when the file has no sections: zero stored and non-zero new, or
     the reverse, is not positional);
  5. where the reader HAS the file's parsed section headers without
     reading the file for this check (the test path, whose front
     parses every input): each row's header fields (type, flags, size,
     alignment, file offset) equal those of section `k`. A reader that
     does not have them does not read them (the last sentence of "A row
     of a file in the set"): a state with two rows of one file swapped
     and its hashes recomputed is then the forged class below, as a
     state with two places swapped is for every reader (ruling on
     WP-064a's question 1, 2026-11-17).

  Identities are built for EVERY file outside the set (model §7.1, from
  those files' symbol tables and section names; a global the partial
  lookup does not hold builds the whole lookup once, and the update
  goes on).

  **A row of a file in the set** translates:
  - `SECTIONS` row `first + k` to section `k` of the file;
  - a local `SYMBOLS` row to the entry `(file, at_index)`; an
    `at_index` at or above the file's `sh_info` (its count of locals)
    is `state-corrupt` (a file in the set has no identities to fall
    back to, and no correct writer stores such a row; ruling on
    WP-064a's question 2);
  - a global row to `sym_map(at_file)[at_index]`, accepted when that
    key's name and version equal the row's;
  - a fragment of a split section, found in `FRAG_AT` by (section,
    offset), to the offset of the `FRAGS` row `FRAG_AT` names, accepted
    when that row's ordinal is `NONE` (a content that is in two merge
    classes of one output section has ordinals, and its copies are
    ranked again by an update: never positional) AND every occurrence
    of the fragment is in a file of the set (fragments are numbered by
    their first occurrence, model §3.5: a fragment that also occurs in
    a file outside the set is found by its content hash, never by
    position; cost: the merge pieces of the files outside the set;
    ruling on WP-064a's question 3).

  A global or a fragment row that is not accepted does not fall back:
  it is translated by identity (key bytes; output section and content
  hash: neither needs the file's identities), which either translates
  it or fails as IN-28 does (the fall-backs of §7); a failure for one
  fragment sends every fragment to identity. Under these rules an accepted position names the
  row identity would find, and IN-2 holds. No check reads an unchanged
  input: each is made against the state and the new link's tables, so
  its cost is that of the rows used.

  **What the state is protected against.** The state's hashes detect
  corruption (a torn, truncated or flipped file: `state-corrupt`, a
  full link), not forgery: whoever can write the state file can choose
  the output, in either translation, as whoever can write the output's
  directory can. The state is therefore written with the output's
  permissions beside it and never taken from another place. A reader
  never panics, loops or indexes out of range on ANY state. Against a
  STALE state (one a correct mortar wrote, for inputs that have since
  changed in any way) the update is exact: that is IN-2, and the
  position-exact rule is what keeps the positional path inside it.

  Measured on the test path before the amendment (WP-094): the
  translation fell from 103.5 to 6.9 ms on protoc `-g` and from 7.4 s
  to 0.15 s on `synthetic-objects-g10x` (replay keys included); the
  figures after it are WP-064a's to record. *Test:* the review's cases
  as permanent tests (`tests/incr-review/`: sections, strings and
  locals of a file permuted and then ANOTHER file updated, on the three
  targets; a string in two merge classes; a state with two `SECTIONS`
  rows swapped and one with `FRAG_AT` rows swapped give the identity
  translation's output or a fall-back by name, never another program);
  every update of those tests is judged against the full link of the
  same inputs by `xtask plumb --equiv`; the floor test translates by
  position and, with `MORTAR_FLOOR_IDENTITY` set, by identity only.
- **IN-28b [v1] A local whose identity is a position** (WP-064a §0.1:
  a wrong program in BOTH translations before it; amended by WP-202 under
  the ruling on WP-064a's question 4). Model §7.1 gave a local the
  identity (file, name, ordinal among the file's same-named locals);
  every `STT_SECTION` symbol has the empty name, and GAS writes `:got:`
  relocations against section symbols, so a `SLOTS` row keyed by one
  translated, after the file's sections were permuted, to another
  section's symbol, and the slot (not refilled) kept the old address.
  Model §7.1 as amended (WP-202's REQUEST): a section symbol's identity
  is its section's identity, with its ordinal among the section symbols
  of that section; a named local's identity is (its section's identity,
  name, ordinal among the same-named locals of that section, by
  `st_value` then index); a mapping symbol (`$x`, `$d`, `$a`, `$t` and
  their `.`-suffixed forms) has no identity, and no state row is keyed
  by one. A `SLOTS`, thunk or patch row keyed by a local of a CHANGED
  file translates by that identity when the new file has exactly one
  local of it; otherwise (none, or several: two same-named locals at one
  value of one section) the row is not translated: its slot becomes a
  dead slot (written 0, S-113) and the new file's demand, if any, gets a
  slot appended. Only units of the same file name a local's slot, and
  that file is refilled whole, so nothing else is refilled and nothing
  falls back. Measured (WP-202 §B.4, `tests/incr-design/elfdiff.py
  census`): no GOT relocation against a section symbol and no
  same-named local (mapping symbols aside) in 7,556 objects (protoc and
  `cxx-large-g`'s GCC objects, libc, libstdc++ and libgcc on AArch64,
  libc and libstdc++ on x86-64); the case is hand-written assembly's.
  *Test:* `crates/mortar/tests/incr_review.rs` s3 as an update
  equivalent to the full link; a hand-written object with two
  same-named locals at one value, each referenced through the GOT,
  edited.
- **IN-29 [v1] Liveness** (*not built: WP-238*; until then an update
  under `--gc-sections` is `update-not-implemented (live)`, WP-215 seam
  3) under `--gc-sections` is recomputed in full
  from the roots, without reading unchanged files, by walking a **stored
  graph that is, by construction, the edge set the full link's liveness
  pass walks** (image.md I-6 roots, I-7 edges; the re-review's N2). This
  amends image.md I-101 (which walks the relocations). The liveness pass
  has one **edge enumerator** (for a section, every edge of every I-7
  kind, with its kind); in an incremental-mode full link it writes the
  enumerator's output as rows, file by file; an update walks the same
  enumerator's output: **stored rows for unchanged files, fresh rows
  (the enumerator run on the new bytes) for changed files**. There is no
  second derivation of the edges. One row kind per I-7 edge kind:

  | I-7 edge (image.md text) | stored as | resolved at walk time | protoc `-g`, rows / bytes at 8 B a row |
  |---|---|---|---|
  | `reloc`: a record of a live, non-debug, non-`.eh_frame` section names a global key → the section of its `def` (through `sym_map` and `redirect`; `Common`, `Internal`, `Shlib`: nothing), `R_*_NONE` included | (source section, key) | the key's definition from the update's resolution (IN-26), so a moved definer is followed | 52,400 / 419 KB |
  | `startstop`: a conditional `__start_NAME`/`__stop_NAME` (`Internal`) → every section named `NAME` | (source section, key) like `reloc` | the sections named `NAME` from the sections table (IN-19), the changed files' included | 0 (counted with `reloc`) |
  | `local`: a local or section symbol → its section; into a split section, the piece by offset (`frag_live`: value, plus addend for a section symbol); under `SHT_REL`, every piece | (source section, target section, offset or "all pieces") | — (both ends in one file) | 2,192 / 17.5 KB |
  | `linkorder`: a section whose `sh_link` is `s` and has `SHF_LINK_ORDER`, alloc or not, is live with `s` (I-109) | a column of the sections table (the dependent's `sh_link` target) | — | 0 on protoc (a column) |
  | `fde`: a code section → its FDEs → the LSDA's and personality's sections through the relocations of the FDE and of its CIE; the FDE's edges are `s`'s | (code section, target section or key) rows, from the file's own `.eh_frame`, which an update does not re-read | keys as `reloc` | 4,210 / 33.7 KB |
  | `group`: every other member of `s`'s group | the sections table's group column (IN-19) | the group's keeper from the update's resolution | 1,164 member pairs / 9.3 KB as rows (a column) |
  | `nonalloc`: records of non-alloc, non-debug sections (notes; `.note.stapsdt` → `.stapsdt.base`) | (source section, key) and (source section, target section) | as `reloc` | 5 / 40 B |
  | roots (I-6): rooted keys (entry, `-u`, exported), `SHF_GNU_RETAIN` and script `KEEP`, start-up tables, notes and non-alloc sections not grouped with an alloc member | flags in the sections table; rooted keys from the plan | the plan's options (a change is `options-changed`) | — |

  Debug relocations are not edges (I-7) and are not stored. A future
  edge kind in I-7 is a change of the enumerator, so it changes the
  state's format version (IN-18): an old state is refused by name, never
  walked with a missing kind. No kind needs a fallback: every row's
  ends are either in one file (re-enumerated when that file changes) or
  a key resolved at walk time. Its difference from the stored `live`
  column is IN-27's liveness row. *Test:* the stored rows of every file
  equal the enumerator's output on replay (IN-61); one L10 mutation per
  edge kind: drop that kind from the stored graph and a gc scenario must
  fail (IN-63b; `livemodel.py` is the prototype: from the inputs alone
  it walks I-7's kinds from I-6's roots and is compared with mortar's
  live set). Measured: it equals mortar's live set, at section and at
  fragment level, on all 20 gc links of R3 (c, cf, l, g at `-O0` and
  `-O2`, both versions; x86-64 lx). **Every edge kind is caught by a
  scenario** when dropped from the walk (`livemodel.py --drop KIND`):

  | kind dropped | gc links that then disagree with mortar's live set | the section the walk loses |
  |---|---|---|
  | `reloc`, `local`, `fde` | all 20 | (many) |
  | `nonalloc` | l, `-O0` and `-O2`, both versions (AArch64) | `.stapsdt.base` |
  | `startstop` | g, all 4 links | `u.o`'s `ureg`, reached only through `m.o`'s `__start_ureg`/`__stop_ureg` |
  | `linkorder` | g, all 4 links | `.meta_k` (and in v1 `.meta_d`): alloc `SHF_LINK_ORDER` sections whose `sh_link` is `.text.keepf` (`.text.dropf`), referenced by nothing |
  | `group` | g, all 4 links | `.rodata.gaux`, the second member of the COMDAT group `gsig`, referenced by nothing; its first member `.text.gfun` is called |

  The g scenario (`oracle_scen.sh` "g", `-ffunction-sections
  --gc-sections`; `m.o` calls `keepf` and `gfun` and sums the `ureg`
  registry in v0, and also calls `dropf` in v1, so that `.meta_d` is
  dead in v0 and live in v1) is the L10 case `gc-edge-kinds`; it prints
  `42 6 7` and `42 6 7 10`, and its oracle pairs pass (IN-63b). Rows on
  the `-O2` l link, for scale: `reloc` 2,135, `local` 326, `fde` 42,
  `group` 4, `nonalloc` 5 (2,512 rows, 20 KB at 8 B a row). On protoc `-g`
  (`livemodel.py --stats` over the bench's inputs, distinct (source,
  target) pairs from sections live from the roots; the bench link has
  no `--gc-sections`, so the model's live set, 24,844 of 30,348
  sections, is the gc one, not the map's): 59,971 rows, **0.48 MB** at 8
  B a row, against the 8.05 MB state; the named L10 case `lsda-edges` (the re-review's
  `re/lsda_edges.sh`: LSDAs reached only through `.eh_frame`, one unit
  per file: the update must keep `keep()`'s LSDA and collect
  `dropme()`'s, and a throw through `keep()` must be caught); L10 removes
  the last reference to a function in another file and checks that the
  function's section became a hole and that file's FDE and debug entries
  were refilled (tombstoned); then restores the reference and checks the
  revived section has its FDE (an exception through it is caught) and
  live debug entries.

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
  link's order, IN-2 item 5). *(Amended by WP-207: "laid out again from
  its region's start" is replaced by IN-72 item 2's keep / place /
  overflow phases; and the list of hole-tolerant sections is closed:
  every other allocated section, except the TLS sections and notes, is
  packed, as `mortar_layout`'s `slack.rs` `class()` does, review-in72
  m-1.)*
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
  to state; amended by WP-207: a region that overflowed is not given up,
  its file keeps it and every later region until compaction, and IN-72
  item 2 fills them in order; the growth step's size and rounding are
  IN-72 item 3's); [later] a free list with first-fit for any region (debug
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
  | `.symtab`, `.strtab` | per file and tail reserves (IN-49a); file slack max(4 KiB, size/16) after each | *estimate* 0.5 MB of fillers on protoc, plus the two slacks |

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
  | merged strings and constants | fragments appended (L-147, S-110); dead fragments are dead ranges: a string fragment keeps its bytes (a live reference may share a suffix), a constant fragment is zero-filled (IN-3a check 3) | `reserve-exhausted` |
  | GOT, PLT, IPLT, `.got.plt`, `.rela.plt` | S-113: slots appended within capacity, dead slots written 0; `.rela.plt` compacted | `reserve-exhausted` |
  | symbol attributes of a global an unchanged file references (type, binding, visibility, `st_other`, absolute/common) | no rule in v1 (IN-26) | `symbol-kind-changed` |
| copy relocations (`R_*_COPY`, M2/M4) | the copy slot keeps its place and size; a shared library's object whose size changes needs a bigger slot | `copy-size-changed` |
  | `COMMON` symbols | the allocation is a region of the commons chunk; a size change moves it, and its referrers are refilled | `reserve-exhausted` |
  | COMDAT groups | the keeper is recomputed (IN-26); a keeper change is IN-27's row | none |
  | `.note.gnu.property`, `.note.gnu.build-id`, other notes | S-114: recomputed; the build id over the final image, with the stored leaves (image.md I-99) | `property-changed` if the property note changes size |
  | `.symtab`, `.strtab`, `.symtab_shndx` | per file blocks with fillers, a dense global area, file slack (IN-49a) | `symtab-exhausted` |
  | section headers, `.shstrtab`, program headers | rewritten whole; their count does not change | `new-output-section` if an output section or segment appears or disappears |
  | thunks, islands, erratum 843419 patches (arch.md "Incremental links") | re-evaluated for refilled and moved units only; new ones go into their island within capacity | `island-exhausted` |
  | `__start_SEC`/`__stop_SEC`, `_etext`, `_edata`, `_end` | global targets in the index (IN-25) | none |
  | `--icf`, `-r`, `--emit-relocs`, `SECTIONS` scripts, compressed debug, generated debug indexes, `--sort-common`, `--sort-section`, `--start-lib` (WP-212 F-9) | not supported in incremental mode (IN-12) | plain link |

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
  - **Pad and move** (WP-239 S4-3, P-4). The incremental-mode full link
    gives each file's records a capacity of size + max(16,
    ceil4(size/8)), stored in its `FILES` row (IN-18e); a plain link pads
    nothing. An unchanged range that moves by Δ is moved from the
    previous output's bytes: its records walked by their length words,
    each FDE's `pc_begin` and its LSDA and personality fields adjusted by
    −Δ; the CIE pointer is relative within the range and unchanged. Only
    the encodings `0x1b` (pcrel sdata4) and `0x9b` (indirect pcrel
    sdata4) are moved so (the only ones in protoc, g10x and `cxx-large-g`,
    GCC, both 64-bit targets); a range with any other is
    `update-not-implemented (eh)`, decided by the plan, which reads every
    range it will move, before W1. Measured on the M.1 series: pad 0
    overflows in 1 to 2 of 5 edits, 1/8 in none of 20. A file with
    records that appears, or a member newly extracted, between two stored
    ranges rewrites the section whole in full-link order (R-3: no in v1;
    [later] by the go/no-go counts).
  - **Hole sizes.** Layout keeps every hole either 0 or at least 16
    bytes. A difference of 4, 8 or 12 bytes between a range's capacity
    and its records is absorbed by the range's last record: its length
    grows by that many bytes of `DW_CFA_nop` (S-47 already pads records
    this way). In the placement DATA that rest is a short
    `HoleKind::EhPad` entry (never at offset 0), as the full link's
    `eh_pads` reads it; in the BYTES and the map it is the last record's
    padding, never a `hole` line or a padding CIE (WP-243, seam 6;
    `mortar_core::incr::EhRange::capacity`). A hole of 16 bytes or more is **one padding CIE**:
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
  - **References into `.eh_frame`** from other sections (synth.md §4.4:
    `crtbeginT.o`'s `frame_dummy`, 4 records per glibc link; on x86-64
    and i386 an `R_X86_64_32`/`R_386_32` to offset 0 of an **empty**
    input `.eh_frame`, on AArch64 an `ADRP`/`ADD` pair to offset 0) take
    S-50's value, which is not a placement of a unit: rule 1 gives the
    start of the output `.eh_frame` whatever the input holds, rule 3 the
    next kept record, possibly another file's, or the terminator. The
    map has no line for an empty `.eh_frame` input (R3: `crtbeginT.o`'s
    section 5 has no `piece`), so IN-27's moved-section row cannot see
    such a target move. The state stores, for each such reference, the
    S-50 value it was given; the update recomputes S-50 for each after
    `.eh_frame`'s layout and refills the referring unit when the value
    changed (IN-27's `.eh_frame`-reference row). It changes when the
    section's start moves (a growth step) or, under rule 3, when the
    records after the target change. Found by R3: the x86-64 lx pair's
    `crtbeginT.o` field (`R_X86_64_32 .eh_frame+0` at `.text+0x91`)
    holds 0x41d4a8 and 0x41d4c0 in the two full links, each the start of
    that link's `.eh_frame`, and the oracle, which did not implement
    S-50, compared it raw.
  - **Which records the delta refills** (WP-063, against a full link's
    layout, S-46/S-47's packing): the records equal from the start (same
    piece of the same section of an unchanged file) keep their offsets;
    when the output `.eh_frame` keeps its address and size, so do the
    records equal from the end, provided each FDE there points to the
    same CIE in both links: its own CIE kept at or after the suffix's
    start, or its own CIE removed by S-44 (an equal CIE earlier) while
    the new link keeps no CIE between the prefix and the suffix; a piece
    the new link's split does not know counts as a CIE. The records
    between are refilled (`eh-frame`). A reference into an input
    `.eh_frame` is refilled unless its section is empty and the chunk
    did not move (rule 1), or every piece of its section is kept and
    none is between (rule 3's next record is then the same).
  *Test:* R5 in L10 (each row a scenario; the first three must pass, the
  others are the verifier's negative cases: plumb rejects a hole that is
  not one padding CIE of ≥ 16 bytes, and a zero word before the end); an
  x86-64 static L10 step whose edit grows the sections before
  `.eh_frame` past their slack (a growth step) checks that
  `crtbeginT.o`'s `.text` was refilled and that its field equals the new
  `.eh_frame` start; the row's mutation (IN-63b) must fail that step.
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
- **IN-45 [v1] Copy by default; in place on request.** *(Amended by
  IN-76 (WP-202): in place is the default at or above `small-output`'s
  bound; the conditions below still decide whether in place is taken.)*
  *(The default is
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
  `write_paths.py`), i.e. ~0.4–1.5 s per GiB, and O(extents) where the
  file system clones (IN-47; not this host's overlay or virtiofs, E5;
  the operator's target does). With
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
    measured here; a descriptor of `P` held by a child of the linking
    process between its fork and its exec is held elsewhere too: the
    result is the copy path, safe and slower; mortar starts no process
    while U1 holds `P` open, WP-219 F-11), and the user is
    told, once per link, that processes outside the scan's view are not
    protected (`incremental-in-place-unchecked`, a note);
  - an exclusive lock on the state file is acquired and the file system
    enforces it (the probe of image.md I-59); otherwise two concurrent
    updates of one output could interleave their writes. (The
    workspace's virtiofs now enforces `flock`, the reviewer's m2.)
    *(Amended by WP-207: the lock is IN-77b's, taken by every
    incremental-mode link on `S`'s descriptor; where it is not enforced
    no update runs at all, `lock-unavailable`, on either path.)*

  **Who in place does not protect** (WP-207, review-in77 M-2; this list
  is the text of the `incremental-in-place-unchecked` note, printed once
  per link that writes in place):
  - a reader that opened `P` before W2 and reads across W2..W4: `cp`,
    `install`, `rsync` of the build directory, a test harness copying the
    binary, a packager; it can copy a runnable mix whose `e_ident` it read
    before W2 (IN-77's aside name makes a reader that opens `P` after W2
    fail with `ENOENT` instead);
  - a process that maps `P` without write denial: a program started
    through `ld.so` or qemu, lldb with `P` loaded, gdb's lazy reads of
    debug sections (R7, above); the `/proc` scan sees them only in the
    link's own pid namespace and, for an unprivileged user, only that
    user's processes; containers sharing a build directory are invisible
    to it;
  - a process that runs `P` where the kernel does not deny writes to a
    running executable: `ETXTBSY` is measured on kernel 6.8 (overlay,
    virtiofs) and depends on the kernel and the file system (over NFS it
    is local to the client; whether a later kernel dropped exec's write
    denial is not verified: *argued, not run*); where it is absent, the
    `/proc` scan is the only protection;
  - a process that opens `P` after the scan and before W2.
  The copy path protects all of them (they keep the old inode).
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
  copies took 1.8–2.9 s (E5).

  **Clones** (the operator's target file system supports reflink). The
  call order, all through `std` (no `unsafe`, so no explicit `FICLONE`
  ioctl): (1) create `T` beside `P` (`O_CREAT|O_EXCL`, I-59); (2)
  `std::io::copy(&mut P, &mut T)`, which on Linux issues
  `copy_file_range` in a loop to end of file; for two files on one file
  system the kernel first tries the file system's `remap_file_range`
  (a clone: btrfs, XFS with `reflink=1`, bcachefs; NFS 4.2 clones on
  the server) and copies bytes only for what it cannot clone, and if
  `copy_file_range` fails (`EXDEV`, `ENOSYS`, `EOPNOTSUPP`, `EINVAL`),
  `std` falls back to a read/write loop: the byte copy measured above;
  (3) `pwrite` the dirty windows and the true header into `T` (on a
  clone, copy-on-write allocates new blocks only for them); (4)
  `fdatasync(T)`; (5) `rename(T, P)`; (6) `fsync` the directory; (7) the
  state (step 5 of IN-46). Whether step 2 cloned is not observable
  without `FIEMAP` (an ioctl, so `unsafe`); `--incremental-report`
  prints the copy's time, which shows it. Not verified here: this
  host's overlay and virtiofs refuse reflink (`cp --reflink=always`:
  "Operation not supported", measured by the coordinator on the
  workspace), and whether `std`'s copy reaches `copy_file_range` for
  `File` to `File` and the kernel's clone-first order are read, not
  run. *Estimate* on a cloning file system: step 2 costs O(extents)
  (milliseconds for a 0.5–2 GB file), not 0.6–0.8 ms/MB. What would
  measure it: R7's `write_paths.py` and E5's 1 GiB copy on btrfs or XFS
  (`reflink=1`), on the target machine or a loop-mounted image (WP-061).

  With a clone, nothing else changes: `T` is a new inode, so IN-21's
  `fstat` and stamps after the last write and IN-22a's stamps work as on
  a byte copy; the sync count is the same (`fdatasync(T)`, `fsync` of
  the directory), but `fdatasync(T)` now writes the dirty windows and the
  clone's extent metadata, O(change), not the whole file (*estimate*,
  not measured); and a debugger or a running process that holds `P`'s
  old inode keeps reading the old bytes, because copy-on-write never
  changes the shared blocks under it: the same guarantee as a byte copy.
  So on a cloning file system the copy path loses its O(output) term and
  "copy by default" is no longer a speed trade-off; in place
  (`--incremental-write=in-place`, IN-45) remains the option for file
  systems without clones. *Test:* the crash hook on the copy path:
  `P` is the old or the new file after a kill at every step; on a
  cloning file system in WP-061, the copy's time for 0.5 and 2 GB
  outputs.
- **IN-48 [v1] The state is written only by a successful link**
  (image.md I-100). A failed update removes the output (image.md I-66)
  and the state. *(Amended by WP-207: this covers a failure of the link
  proper, before U10's first write; a failure of a write, sync or rename
  is IN-77e's, which keeps a good old pair before W1/C2.)* *Test:* an update that introduces an undefined symbol
  leaves neither file; the next `--incremental` link reports `no-state`.
- **IN-49 [v1] The map** (`--mortar-map`, plumb.md §3.2) is rewritten
  whole (image.md I-99), with `hole` and `dead` lines (§8.2). *Test:*
  plumb reads every L10 step's map.
- **IN-49a [v1] `.symtab` and `.strtab`, per file** (amended by
  WP-202 under the supervisor's ruling on P-E3; it replaces "rewritten
  whole", S-112, in incremental mode). In an **incremental-mode output**
  (its map says `incremental`: the first link, every fall-back's full
  link and every update) the symbol tables have room per file; a **plain
  link has none of this** (S-73-S-88 unchanged, no filler, no slack).
  - **The local area** is S-74's blocks in S-74's order, each followed
    by **filler entries** up to the block's capacity, count + max(2,
    count/8) entries, and after the last block a **local tail reserve**
    of max(64, locals/32) fillers. `sh_info` counts every entry before
    the global area, fillers included.
  - **The filler has exactly one form**: `st_name` 0, `st_info`
    `STB_LOCAL`/`STT_SECTION` (0x03), `st_other` 0, `st_shndx` **1**,
    `st_value` **section 1's `sh_addr`** (amended by WP-207, review-in72
    M-3: with 0, eu-elflint reports "st_value out of bounds" for every
    filler), `st_size` 0, and 0 in `.symtab_shndx` when there is one.
    Section 1 is the output's first section header after the null one,
    whatever its name (`.note.gnu.build-id`, `.note.gnu.property`,
    `.interp`); every output with a `.symtab` has it, and it is the first
    allocated section, so no update moves it (IN-72 moves only what
    follows a grown section); were its address ever to change, every
    filler is rewritten with it. **The form is unambiguous**: a mortar
    output's `.symtab` has no other `STT_SECTION` entry and no other
    unnamed entry than entry 0 (S-79; measured by WP-207 on an
    incremental-mode and a plain static link of the same object on
    AArch64, x86-64 and i386: 0 and 0 in each), so the verifier tells a
    filler from every legitimate entry by its form alone (V-1's
    `near_filler_reported`). Measured tools (WP-207,
    `tests/incr-design/fillers.sh` and `fillcmp.py`, the output rebuilt
    by `symfill.py` with and without fillers, on AArch64, x86-64 and
    i386): the record's §M lists each; every tool reads the output as
    without the fillers, the symbol listings (`readelf -s`, `nm -a`,
    `objdump -t` and their LLVM and elfutils counterparts) differing only
    by the filler lines themselves, and eu-elflint has no complaint about
    a filler. perf is not claimed (not reproducible here, review-in72
    m-6). A filler is in the local area only: there is none after
    `sh_info`. (S-79's "no `STT_SECTION` symbols" excepts fillers.)
  - **An update** rewrites the block of every file whose local entries
    change (amended by WP-207, review-in72 M-1): a changed file; a
    drawn-in file (IN-72 item 2a: a section of it revived, killed, newly
    kept or no longer kept, S-76; one of its globals demoted to a local
    of its block by the merged visibility, S-74, or promoted back); its
    block is rewritten from its start from its symbol table as read. A
    demoted global leaves the global area as a removed one does (the
    area's last entry takes its index); a promoted one is appended to the
    area. What is left of the capacity becomes fillers. A block that does not fit
    moves to the local tail reserve, its old place filled with fillers;
    its entries keep S-74's order inside the block (P-E3 compares locals
    as a multiset and does not judge their place; the `STT_FILE` that
    precedes a moved block of a file that has none is another file's,
    as it already is in a full link for such a file). The tail reserve
    exhausted: `symtab-exhausted`.
  - **The global area** is dense: from `sh_info` to the end, no filler.
    A full link orders it by S-75. An update appends a new global at
    the area's end, and a removed global's entry is taken by the area's
    last entry (its index changes; no relocation of a static executable
    names a `.symtab` index). `.symtab` grows by `sh_size` into the file
    slack that follows it.
  - **File slack.** In an incremental-mode output `.symtab` and
    `.strtab` are each followed by max(4 KiB, size/16) bytes of zeros,
    in no section and listed in the map as `hole .symtab <sh_size> <n>`
    and `hole .strtab <sh_size> <n>` (`spec/plumb.md` §6.8 as amended:
    a hole past a non-allocated symbol table's end, up to the next
    section's offset). Each grows into its slack by `sh_size`; the
    slack exhausted: `symtab-exhausted`.
  - **`.strtab`** is S-86's fixed table, then one region per block with
    slack max(16, len/8) bytes, then the globals' names, which grow at
    its end. A changed file's names are rewritten in its region; a
    region that does not fit, and every name of a new global, goes to
    the end. **Bytes that no entry names are dead ranges** (IN-3a; map
    `dead .strtab <off> <size> strings`): they are the zeros of the
    slack and **old names**. A renamed or removed symbol's old name
    stays in the output's bytes until IN-3b's scrubbing; whoever must
    not ship an old name links in full.
  - **What an update reads and writes.** The `SYMTAB` and `SYMTAB_KEYS`
    rows (IN-18e) of the changed and drawn-in files and of the globals it
    moves, removes, demotes or promotes; the
    previous output's entries it moves (the area's last global) and the
    blocks of unchanged files whose sections moved (a growth step: their
    values remapped through the placement, the rest of each entry as
    read). Nothing else of the tables is read or written: neither the
    state's names nor the unchanged inputs (IN-14).
  - **How the update writes them** (U-5's plan, `SymtabPlan`: its
    `moves` `(from, to)`, `from` an index of the previous output and `to`
    of the new table; its `remap` ranges; its `fillers`; its `entries`;
    WP-217, WP-218 §E). For an index i of a remap range the source is the
    previous output's entry at `from` when a move has `to` = i, otherwise
    the entry at i; its value is mapped through the growth steps. The
    writer reads every move from the previous output before it writes,
    applies the remap over the table after the moves, then the fillers,
    then the entries; a table that shrinks is zeroed from its new end to
    its old end, and the update is refused (`state-corrupt`) if that
    range reaches another section. A plan that writes one index twice
    (two moves to one `to`, or a move and an entry at one index) is
    refused `state-corrupt` before any write: a self-contradictory plan
    has no reason of its own in v1, and the full link follows.
  - Cost (*estimate* from protoc's 163,381 locals and 76,402 globals,
    §1.4): about 20,000 fillers and the 64-entry reserves, 0.5 MB on 5.4
    MB of tables; on g10x WP-094 §4 measured 7 `.symtab` and 8 `.strtab`
    windows per update with the globals at their `SymbolId` places,
    which the dense global area keeps.
  *Test:* (B) of IN-1 (the replay builds the tables from the inputs:
  equal modulo fillers and order); `nm` of an update equals `nm` of a
  full link modulo values; `readelf -s` warning-free; gdb and lldb
  `info symbol`/`image lookup`, `bt` on an update; an edit that adds a
  local, one that removes a global, one that renames, and a series that
  exhausts each capacity in turn (`symtab-exhausted`); WP-207 adds
  `n2_visibility_demotes_unchanged_definer` (three hops: hidden, default,
  hidden), `gc_revived_section_has_its_locals`,
  `global_removed_twice_last_moved` and `filler_value_is_section_1_addr`
  (`eu-elflint --gnu-ld` with T-1's four allow patterns, on all three
  targets).

### 5.10 The update, end to end (WP-202)

This section is the update as WP-064's successors build it
(`spec/landed/incr/WP-202.md`, the cut "U" and "G"). It runs from "the
link starts" to "the output and the state are a consistent pair on
disk", for a static executable (IN-12). Every step names its cost class
and the basis of the class (a record that measured it, or *estimate*),
and every way it can fail names exactly ONE reason of §7. A reason is
always a full link in incremental mode (`--incremental=update`: an
error), decided before the first write (IN-13), except the writes' own
failures (IN-77), which leave the pair as IN-77 says. Cost classes:
**O(1)**; **O(change)**: the changed files' bytes and rows, plus the
rows of the units and keys they reach through the delta; **O(inputs)**:
one small operation per input (a `stat`, a `FILES` row); **O(state)**:
a pass over a whole table of the state; **O(output)**.

- **IN-70 [v1] The steps.** In this order; nothing is written before
  U9.

  | step | what | rules | cost class (basis) | failure → §7 reason |
  |---|---|---|---|---|
  | U1 | IN-11/IN-12; `S` and `P` opened once (IN-77a), `S` locked (IN-77b); the flag read first (review-in77 m-1); the state opened by range: header, directory, `META`; the plan hash; the output's binding through the descriptor; after a reboot with `output-synced` 0, the leaves (IN-77c) | IN-18c, IN-18b, IN-21, IN-77a-c | O(1) + the directory (WP-105 §4: 2 preads) + the lock (IN-77b: 3.5-352 µs); O(output) once after a reboot | the flag: `output-torn`; the lock: `state-busy`, `lock-unavailable`; no state: `no-state`; a hash or range fails: `state-corrupt`; another format: `state-version`; the plan: `options-changed`; the output: `output-changed`; `META`'s size: `small-output` |
  | U2 | every input `stat`ed (on the pool), the stamps, change detection; archives | IN-22, IN-22a, IN-24 | O(inputs) stats + O(changed bytes) hashed (WP-105 §5: 102 ms for 8,241 inputs on virtiofs, pool) | a changed path that is no file of the link: `options-changed`; two unchanged files reordered: `inputs-reordered` |
  | U3 | the positional set | IN-28a | O(inputs) over `FILES`/`STAT` | none (a file outside the set is translated by identity) |
  | U4 | the front over the changed, drawn-in and newly extracted files only: parse, split, scan; their descriptors `fstat`ed against `STAT` (IN-75); repeated with U5 until no file flips (IN-71) | IN-14, IN-28a cond. 2, IN-71b, IN-72 item 2a | O(change) (*estimate*: the floor's front over all inputs is what WP-061b measured) | a file the update read is not the file it judged: `input-changed` |
  | U5 | delta resolution and extraction; the ODR-affected keys | IN-71, IN-71b, IN-71d | O(change) (*estimate*; WP-105 §7 (iv): the whole-program resolve is 102.7 ms on g10x, the term this removes) | IN-71's list; `extraction-changed` |
  | U6 | liveness, under `--gc-sections` only | IN-29, IN-71a | O(stored edges) (*estimate*: 0.48 MB of rows on protoc, §5.5) | none |
  | U7 | the delta layout, `.symtab`/`.strtab` and thunk islands (fixed capacity, IN-72 item 9) included; then `DEFINED` from the `Placement`, final (IN-71c) | IN-72, IN-73, IN-49a, IN-71c | O(change) + the growth steps' moved sections (*estimate*) | IN-72's and IN-73's list |
  | U8 | the refill set, against the update's own layout | IN-27, IN-74 | O(change) + the referrers it reaches (WP-063: 46-54 ms on protoc against a full link's layout, an over-approximation) | over IN-16's bound: `too-much-changed` |
  | U9 | refill and image: dirty ranges, windows, the build id; then the state's patch planned, `mortar_incr::state5::plan_patch` (IN-46b: every block W5 will write read and verified, in place or compaction decided; WP-213) | IN-75, IN-46b | O(dirty windows) + O(leaves) for the build id (WP-094 §4: 0.48 MB written on g10x) + the patched blocks' reads (O(state) for a compaction) | a block that fails its hash: `state-corrupt` (a refill error is the link's own diagnostic, IN-5) |
  | U10 | writes: output, then state (W5: `state5::patch`, or `state5::compact` when the plan compacts; writes only); the crash protocol; the diagnostics (IN-75a) | IN-76, IN-77, IN-45, IN-78 | in place: 5 `fdatasync`s and 2 renames with 2 directory `fsync`s (IN-46: 2.3-8.2 ms) + the patch's (WP-105 §4: 154-191 ms under load) | IN-77 |

  Debug sections (§6) are handled by the G packages; until they merge,
  U2 ends the update with `update-not-implemented` naming `debug` when
  any changed, appeared or newly extracted file has a non-allocated
  section named `.debug_*` or a `DW_UT_skeleton` (IN-55a), before
  anything is read past the change detection. *Test:* each reason of
  this table by a named case (WP-202 §D); the order of the checks by
  one case per adjacent pair (two reasons at once give the earlier).

- **IN-71 [v1] Delta resolution, O(change)** (narrows the cost of IN-26,
  not its result; amended by WP-207). The update re-resolves only the
  **affected keys**: every global key a changed, appeared, gone, drawn-in
  (IN-72 item 2a), newly extracted or no longer extracted file mentions,
  in its old version (`MENTIONS`, the file's range) or its new one (its
  parsed symbol table); every linker-defined key IN-71c names; and every
  key whose common size or alignment, or whose winning common mention,
  changes (IN-72 item 11). For each
  affected key the resolution is recomputed over **all** its mentions, in
  load order: the stored mentions of every other file (`KEY_REFS`, by
  key: one row per (key, mentioning file), single mentioners, archive
  members not extracted and file 0 included, IN-18e) and the changed
  files' new ones. COMDAT groups the changed files name are re-elected
  the same way (`GROUPS`, by signature: the keeper is the copy whose
  file has the least `FILES.rank`, the update's new ranks of changed,
  appeared and drawn-in files used, once the changed files' rows replace
  their old ones; WP-215), and so is every group with a copy in a file whose extraction
  flips. A common's size, alignment and winning file are computed from
  the key's `COMMONS` rows the same way (resolve.md §3.3). Archive extraction is IN-71b.
  The result is IN-26's for every key (a key no file of the delta
  mentions has the stored resolution, which IN-26 would recompute
  equal). For an affected key whose mentioners change, `at_file` becomes
  the first live mentioner by current `FILES.rank` among `KEY_REFS`'s
  rows of live files and the changed files' (IN-18e's rule; a
  non-extracted member's row decides nothing), and `at_index` is that file's entry
  index among its non-local entries: from its new symbol table when it is
  a changed file, else from its `MENTIONS` row (whose position gives the
  index: the file's `first_global` plus the row's position in its range,
  IN-18e; WP-215 D7);
  `NONE` only when no live file mentions the key (review-in77 B-4). The
  differences are IN-27's causes. A difference in attributes (IN-26's
  list) for a key that an unchanged file references:
  - binding (weak ↔ global, `STB_GNU_UNIQUE`), visibility, a common
    becoming a definition in a section or the reverse: in a **static
    executable** these change which definition wins and its value, not
    a relocation's form (no preemption, no copy relocation, no PLT
    choice), so they are IN-27's "a global's definition moved or
    changed" row: the referrers are refilled (WP-064a §3: W1, W1b, n2,
    n4; verified by review-in72 §5 in `mortar_arch`'s class table). A
    visibility change that demotes a global to a local of its definer's
    block, or promotes it back (S-74), draws the definer in (IN-72 item
    2a, IN-49a). Dynamic outputs keep IN-26's rule (WP-066);
  - type to or from `STT_GNU_IFUNC` or `STT_TLS`, a definition becoming
    absolute or the reverse, `st_other` bits other than the visibility:
    `symbol-kind-changed` (an unchanged referrer's relocation types,
    which decide IPLT and TLS slots and relaxations, are not stored).
  A local becoming global (W10) is a key that appears; a global becoming
  local, one that disappears from the file's mentions; both are affected
  keys like any other. Extraction order that changes the relative order
  of two unchanged files: `inputs-reordered`. **U4 and U5 iterate**
  (review-in72 m-3): a file that IN-71b newly extracts, or IN-72 item 2a
  draws in, is read (U4) and its keys become affected (U5), until no
  file flips; each round only adds files, so it ends.
  **Values** (WP-215 seam 3): a changed file's `st_value` and `st_size`
  per mention are passed in (`ChangedFile::values`, made by the capture
  function that makes its `MENTIONS` rows; never stored). An unchanged
  file that becomes a key's definer or a group's keeper is **drawn in**
  (never for a common: item 2a): `resolve` returns the files to read
  (`Resolution::DrawIn`) instead of a delta, never a partial delta; the
  join reads them (`Change::DrawnIn`, IN-75's `STAT` check first) and
  calls `resolve` again. A drawn-in file's mentions equal its stored
  rows, so its keys are affected (its sections take new ids, so the
  `RESOLVED` rows naming them are rewritten); no candidate or winner
  changes, so the second call asks for nothing (WP-215 R.6 2, ruled)
  (at most `RESOLVE_CALLS` = 2 calls; a second draw-in means a drawn-in
  file's bytes disagree with its rows: `input-changed`). Drawn-in files
  count toward IN-16's bound.
  **Strings** (WP-215 seam 3): the changed files' `Str`s (section names,
  signatures) are of the update's id space: one buffer
  (`ChangedFiles::strings`, at or past the state's `STRINGS` length) that
  `StatePatch::strings` begins with, read by one function
  (`ChangedFiles::string`). Names are equal by **bytes**, never by `Str`
  value. Every update appends the changed files' names again (no lookup
  by content).
  **Options** (WP-215 D3-D5): the keys the options name (resolve.md
  §4.2's roots: the entry or `_start`, `-u`, `EXTERN`,
  `--require-defined`, the `--defsym` operands; `--wrap`; the `--defsym`
  keys) are **passed in, never stored** (`mortar_core::incr::OptionKeys`,
  built by the join from the plan and the scripts' directives as the
  full link's resolution reads them; an update has the stored link's
  options, else `options-changed`). `--wrap`'s redirection (resolve.md
  §7.1) is applied when a stored undefined row is read, by name.
  **Versions** (WP-215 D8): a `foo@@V` definition defines `foo` and,
  when the link has that key, `foo@V` (resolve.md's `also`), and the
  state stores neither which definition was `@@` nor its version. So an
  update is `update-not-implemented (versioned)`, before anything is
  written, when the stored link has a non-local symbol-table entry named
  with an `@` (`META.versioned` not 0) or a changed, appeared, drawn-in
  or newly extracted file has one in its new bytes
  (`ChangedFile::versioned`). This is sound: when both are 0, no file of
  the old link (every relocatable object and member, `LAZY` or not, is
  counted) and none of the new one (the old link's unchanged files and
  the changed files' new bytes) has such an entry, so no key has a
  version and no definition an `also` key in either link. Measured not
  to fire (WP-215): protoc (`cxx-medium-static`), a C and a Fortran
  hello, static, GCC 14 with glibc 2.41 (`libc.a`, `libm.a`,
  `libstdc++.a`, `libgfortran.a`, `libgcc.a`, `libgcc_eh.a`, the crt
  objects; AArch64; and the x86-64 and i386 `libc.a`): no non-local
  entry with an `@`. *Test:* WP-064a
  §3's cases, each now an update equivalent to the full link
  (`w-meaning.sh` W1, W1b, W9, W10, n2, n4; `ifunc-type` stays
  `symbol-kind-changed`); `common_first_referenced_by_changed_file` and
  `first_mentioner_drops_then_readds_key` (three hops, review-in77 B-4);
  `key_refs_single_mentioner`; a COMDAT keeper change both ways and
  `comdat_keeper_flips_twice`; `versioned_old_or_new_is_fallback`
  (WP-215: `foo`, `foo@V`, `foo@@V`, gained and lost).

- **IN-71b [v1] Archive extraction in an update** (WP-207; review-in72
  B-1; amended by WP-211, review-wp207 B-1). It is resolve.md §4.1's
  reachability recomputed, never a rule of its own: a member is extracted
  iff it is reachable from the non-archive files and the root options
  through §4.1's edges (a non-weak undefined mention of `K` → `first(K)`,
  the lowest `FileId` among **all** files with a defined or common
  mention of `K`, archive members included whether extracted or not; a
  common mention → `cfirst(K)`; a root option → `first(K)`), computed
  from the members' own symbol tables, never the armap. **Words.** In
  this rule *lazy* is not used. A **member** is resolve.md §4.1's lazy
  file: a file that came from an archive, which stays a member whether
  or not it is extracted. **`LAZY`** is the `FILES` flag of a member
  that is **not extracted**. Edges, `first`/`cfirst` and `cfirst`'s
  (a)/(b) split are by *member*; only "extracts" and "is no longer
  extracted" read or set `LAZY`. An object of a `--start-lib` region is
  a lazy file (resolve.md §2) but not a member; v1 does not update a
  link whose line has such a region: `--start-lib` joins the options
  incremental mode does not take (`incremental-unsupported`, a plain
  full link; IN-12), until U-3 shows them to be members under this rule
  (WP-212, F-9).
  - **Stored** (IN-18e): every member of every archive of the link has a
    `FILES` row, extracted or not (flag `LAZY` when not), at its place in
    load order; a `LAZY` member's defined and common mentions are its
    `MENTIONS` range and its `KEY_REFS` rows (with the attributes §4.1
    reads: definedness, binding, common, section or absolute); a `LAZY`
    member's common's size and alignment are not stored (`COMMONS` holds
    the live files' rows, IN-18e; WP-215 D6): an extracted member is read,
    and they come from its bytes; a `LAZY` member's undefined mentions
    are not stored (an extracted member's are), and decide nothing that
    is stored (as `at`, IN-71: the first LIVE mentioner).
    **Live keys** (WP-215 R.6 3, ruled): a global key is **live** in a
    state when a stored row gives it a mention by a live file (a
    `KEY_REFS` row of a live file, a definition, a common, a group), a
    `LAZY` member's definition (a `KEY_REFS` row of a `LAZY` file), or an
    option names it (`OptionKeys`); any other key is **dead**. A capture
    writes exactly the live keys. An update need not delete a key whose
    last mention went: it may leave it dead (its `SYMBOLS` row, its
    `RESOLVED` row undefined), like a dead string; IN-78's `canon` and a
    compaction drop dead keys by this rule. No reader treats a dead key
    as live: it is never reported undefined, never a draw-in's or a
    re-election's reason, never affected by its own deadness; a dead key
    that gets a mention again is found by name (`NAMES`) and its row
    reused, never a second row. Each
    extracted member's `FILES` row carries its **round**: a number such
    that some live file or root with an edge to it has a smaller round
    (non-archive files and roots 0): the **support property**. The first
    link writes the breadth-first round of §4.1's walk; an update keeps
    only the support property (addition and re-seeding touch only the
    targets of lost and gained edges), so after an update a round may be
    larger than the breadth-first one (review-wp207 M-1: its case R2-1).
    IN-78 checks the property, never the value.
    A full link already parses every member's symbol table (resolve.md
    §4.1; odr.md O-23), so the capture costs only the rows.
  - **Edges of a key.** For an affected key `K`, `first(K)` and
    `cfirst(K)` are read over **every** `KEY_REFS(K)` row (lowest rank
    among the rows with the right attributes, members' rows included,
    `LAZY` or not), and `cfirst`'s (a)/(b) split is by member, not by
    `LAZY`. Its edges are the live mentioners' undefined (common) rows
    pointing at `first(K)` (`cfirst(K)`) **when that file is a member
    (extracted or not)**, plus a root's. An edge to an extracted member
    is still an edge: the removal below needs it. The update computes each
    affected key's edge set before and after the changed files' rows are
    replaced: **lost** and **gained** edges.
  - **Iterations** (resolve.md §4.1, WP-200; WP-215). An update never
    iterates the closure. A link whose closure ran more than one
    iteration (`META.iterations`) is not updated: every update of its
    state is `closure-iterated`. Otherwise, after the extraction and the
    re-election, for every affected key `K` and each of `first(K)`,
    `cfirst(K)` that is a live relocatable object whose mention of `K` is
    defined in a section of a COMDAT copy the new keepers drop, the
    update ends with `closure-iterated` (the full link would iterate).
    Only an affected key can start an iteration
    (`spec/landed/incr/WP-215.md` §Q.2). Measured: protoc and
    `cxx-large-g` run one iteration. *Test (U-3):*
    `feedback_check_is_sound_random`, `stored_link_iterated_is_fallback`,
    `affected_key_feeds_back_is_fallback`.
  - **Removal (decremental).** A member that lost an edge keeps its
    extraction iff some edge from a live file of smaller round still
    reaches it (its in-edges: the keys it is `first`/`cfirst` of, from its
    own `MENTIONS` range, and their mentioners, from `KEY_REFS`). The
    members without such support, and transitively the members they
    alone supported, form a cone; the cone's members are re-seeded from
    the live files outside it in round order (a member reached gets the
    smallest supporting round + 1), and what is not reached is no longer
    extracted: a gone file for the update. This is the cycle case
    review-in72 R1b found: two members supporting each other have no
    smaller-round support once the outside edge is gone.
  - **Addition (forward).** A gained edge whose target is not extracted
    (`LAZY`) extracts it (round = source's + 1): it is an appeared file, read from its
    archive (below), and its undefined mentions add edges, followed
    breadth-first; an edge to a `first(K)` at a lower position than the
    key's current definer (review-in72 R1a: a strong member before a weak
    definition) is an edge like any other.
  - **Reading a member.** An unchanged archive's newly extracted member
    is read through the archive's descriptor at the member's stored
    offset, after the descriptor's `fstat` equals the archive's `STAT`
    row (IN-28a condition 2); else `input-changed`. A changed archive's
    members are per-member changed, appeared or gone files (IN-24). No
    unchanged archive is parsed: its members' rows are the state's.
  - **Bound** (`extraction-changed`): more than 1,024 members flipping, or
    a removal cone whose re-seeding reads more than 65,536 `KEY_REFS`
    rows, ends the update with `extraction-changed` before anything is
    written (a full link is cheaper there). The bound values are
    *argued, not run*.
  - **Modelled** (`tests/incr-design/model_extract.py`, written from this
    text): R1a and R1b give the full link's member set; 3,000 random
    programs (members interleaved with objects, weak, common and
    weak-undefined mentions, root keys) with four edit hops each, 12,000
    updates with 3,534 extraction flips: 0 differ from §4.1's closure
    from scratch. The reviewer's own model
    (`tests/incr-design/review-wp207/extract_mine.py`, written from the text alone)
    gave 0 of 4,000 differences when "lazy" was read as *member* and
    1,130 of 4,000 (R1b's cycle kept) when it was read as `LAZY`; the
    words above leave only the first reading. Re-run on WP-211's text
    (`tests/incr-design/extract_amended.py`: R-2's model in the *member*
    reading, rounds judged by the support property): R1a, R1b and R2-1
    ok; 3,000 programs, 12,000 updates, 3,657 flips, member set differs
    0, support property violated 0 (259 rounds differ from the
    breadth-first round, which is no longer a failure). Not modelled:
    `--whole-archive`, `--start-group` semantics beyond §4.1's, shared
    objects, `--wrap`, file 0's Assign candidates (WP-211's named tests,
    below).
  - **Cost.** State: a lazy member's defined or common mention costs a
    `MENTIONS` row (12 B) and a `KEY_REFS` row (12 B); measured here,
    `libstdc++.a` (GCC 14, AArch64) has 7,271 defined globals in 189
    members (38.5 a member), `libc.a` 4,568 in 2,022. At the operator's
    1,000-3,000 archives, *estimate* with libstdc++'s density and 50
    members an archive: 1.9-5.8 M lazy definitions, 46-139 MB if no
    member were extracted, less by the extracted share (whose rows exist
    anyway); not measured on the operator's code base. Per update: for
    each affected key a binary search of `KEY_REFS` (WP-105: 225 preads
    for `RESOLVED` on g10x), plus the cone's in-edge reads; no archive is
    opened except to read a newly extracted member.
  *Test (U-3):* `weak_definition_vs_later_member` (R1a),
  `member_cycle_dropped_together` (R1b), `newly_extracted_member`,
  `newly_extracted_member_pulls_another` (review-in72 m-3),
  `no_longer_extracted_member`, `member_flips_back_on_third_hop`, each
  judged by IN-79; `extraction_bound_is_extraction_changed`; the model's
  random series as a property test over hand-made `KEY_REFS`, built from
  resolve.md §4.1's closure (the oracle), not from this rule's words;
  `member_round_shortcut` (R2-1: the member set right, the round
  supported, not breadth-first); `extracted_member_keeps_in_edge` (an
  edge to an extracted member is counted in its support);
  `start_group_member_cycle`, `wrap_of_member_symbol`,
  `assign_candidate_vs_member` and `root_option_targets_member` (WP-211:
  the shapes outside both models).

- **IN-71c [v1] Linker-defined keys are affected keys** (WP-207;
  review-in72 B-2, review-in77 B-3 trace 2; amended by WP-211,
  review-wp207 M-6). Every update recomputes `DEFINED` (the values of the
  keys of resolve.md §8.1's table, the one table of linker-defined
  symbols, of `__start_`/`__stop_` keys, and of every `--defsym` key,
  alias or expression, resolve.md §7.2: `mortar_layout`'s `defined`
  already holds every `Def::Internal`) from its `Placement` at the end of
  U7, where it is final (IN-72 item 9: no later step moves a section) (O(rows of `DEFINED`): tens to a few hundred), and every
  key whose value, section or existence differs from the stored row is an
  affected key, whose referrers (`KEY_REFS`: `libc-start.o` for
  `__init_array_end`, `dl-minimal-malloc.o` for `_end`, measured by
  review-in72) are refilled. No list is consulted, so a linker-defined
  symbol added to §8.1's table later is covered without an edit here.
  Today that set is: `_GLOBAL_OFFSET_TABLE_`, `__ehdr_start`,
  `__executable_start`, `_etext`/`etext`/`__etext`, `_edata`/`edata`,
  `__bss_start`, `_end`/`end`, AArch64's `__data_start`, `__end__`,
  `__bss_start__`, `__bss_end__`, `_bss_end__`,
  `__init_array_start`/`_end`, `__preinit_array_start`/`_end`,
  `__fini_array_start`/`_end`, `__rela_iplt_start`/`_end` (i386
  `__rel_iplt_*`), `__start_SEC`/`__stop_SEC`, `_TLS_MODULE_BASE_`,
  `__GNU_EH_FRAME_HDR`; the ones a delta layout can move are all but
  `__ehdr_start` and `__executable_start`, but the rule does not depend
  on that judgment. *Test (U-3, U-10):* `init_array_grows`,
  `init_array_shrinks`, `init_array_grows_then_shrinks`,
  `start_stop_registry_grows_then_shrinks`,
  `eh_frame_overflow_then_edit_unchanged_file` (review-in77 B-3),
  `ifunc_added_moves_rela_iplt_end`, `local_ifunc_slot_dead_and_appended`
  (IN-28b's IPLT case, review-in72 m-5), `bss_grows_moves_end`, each a
  first update and a third hop; `defsym_follows_moved_symbol`
  (`--defsym x=_etext+4`, an edit that moves `_etext`: `x`'s referrers
  refilled; review-wp207 M-6).

- **IN-71d [v1] ODR checks and `.mason.meta` in an update** (WP-207;
  rewritten by WP-220). In the default `--odr=warn` (operator,
  2026-11-17) and in `--odr=error` the only checks of odr.md that run
  are W1 (`odr-size-mismatch`) and W2 (`odr-symbol-type-mismatch`)
  (O-47's register; O-42: no container is read); they read symbol
  tables, not `.mason.meta`, and an update computes them **exactly**:
  - **Affected keys**: every key a COMDAT copy of a changed, appeared,
    gone, drawn-in, newly extracted or no longer extracted file defines
    (old version: its `MENTIONS` rows whose section has `anchor_kind` 1;
    new version: `ChangedFile::odr.copies`), and every key whose resolved
    definition or COMDAT keeper changed (`ResolveDelta`).
  - **Per affected key**: the full link's comparator
    (`mortar_odr::foreign::compare_copies`, one function: the full link
    calls it per key) over the key's `COPIES` rows with the changed
    files' rows replaced by their new ones, and its resolved definition
    (`RESOLVED`, `KEY_REFS`). Copies are ordered by the files'
    **current** `FILES.rank` (the stored rank composed with the update's
    new ranks), never by `FILES` row number: "first", a class's first
    file and §9.1's key follow load order. A finding's subjects are the
    key and the files it names (`mortar_odr::foreign::diagnostic`, the
    same function for a full link's capture); findings of other keys are
    the stored ones (`DIAGS`, IN-75a).
  - **Everything else falls back, from the switches alone** before any
    row is read (`mortar_odr::register::update_fallback`, WP-220):
    `--odr=strict` (W3, W4 and every record check), `--odr-check-archives`,
    `--odr-table` other than `off`, `--odr-runtime` other than `off`,
    `--odr-stats`, and any check O-47 runs under the switches other than
    W1 and W2 (a check O-46 moves to on by default forces the fall-back
    until an update computes it) give `update-not-implemented` (detail
    `meta`) on every update. `--odr=off`: nothing runs, nothing is read.
    The record families (kinds 2, 3, 5, ENFORCE, isa, vtable-rtti) are
    WP-236's: reviewed adversarially before any table is written (their
    proposal: `spec/landed/incr/WP-220.md` §B-§D). An `--odr` mode change
    is `options-changed` (the plan hash covers it).
  - **`.mason.meta` in the output** (`--keep-mason-meta`, MT-32) is a
    non-allocated per-file contribution like a debug section: until
    G-1's contributions cover it, an update whose changed, appeared,
    gone or flipped files carry units under that option is
    `update-not-implemented` (detail `meta`).

  *Estimate* (WP-220 §F.1, measured counts): `COPIES` 1.34 MB on
  protoc, 2.35 MB on `cxx-large-g`. *Argued, not run* (no update code
  yet). *Test (U-8):* an update's W1/W2 findings equal the full link's
  of the same inputs (text, order, subjects) over three hops (introduced,
  removed, introduced again) for: a size mismatch of data copies in one
  group; a type mismatch; a copy in a changed file, in an unchanged file
  whose key's resolved definition changed, in a newly extracted and in a
  no longer extracted member; a keeper change without a change of the
  copies; one file with one key in two groups; GCC, Clang and kiln
  objects; on AArch64 and x86-64. `odr_unchanged_pair_made_live_by_changed_file`
  (two unchanged copies in conflict, made live by a changed file's call:
  the finding appears as in the full link); one test per switch of the
  fall-back and the register test (`update_knows_every_default_check`);
  `odr_runtime_note_under_update` asserts the fall-back by name.

- **IN-71a [v1] Liveness from stored edges** (*not built: WP-238*;
  until then `live` returns the empty delta without `--gc-sections`, read
  from `OptionKeys::gc_sections`, and `update-not-implemented (live)`
  with it, on every update, decided from the option alone: WP-215 seam
  3). IN-29's stored graph is the
  `EDGES` table (IN-18e: per file a range of rows, each IN-29's row
  kind). The update walks it from the roots with the changed files'
  ranges replaced by their fresh rows. A walk reads every row: its cost
  is the graph's (*estimate* 0.48 MB on protoc `-g`, §5.5), not
  O(change); it runs only under `--gc-sections`. *Test:* `gc-edge-kinds`
  and `lsda-edges` as updates; the mutation per edge kind (IN-63b).

- **IN-72 [v1] The delta layout** (amended by WP-207: items 2, 2a, 3, 6,
  10, 11 and the growth step; by WP-211: items 2a, 9, 11; by WP-216 seam 4:
  items 2a, 5, 6, 7, 11). From the stored layout, never a fresh one.
  Rows of unchanged files are read only where the delta reaches them
  (their `SECTIONS` rows by `FILE_INDEX` range; `HOLES` and `CHUNKS` by
  output section). Before item 1, IN-71's COMDAT election and IN-71a's
  liveness have run: "a file's sections" below means its live, kept
  sections only (review-in72 M-4 (f)). Sections are matched to their
  previous version by model.md §7.1's identity (anchor, name, ordinal).
  In order:
  1. **Gone sections** (of gone files, of sections no longer live, of a
     COMDAT copy no longer kept) become holes (IN-3) of their region;
     their fragments lose one reference (item 6). Their file, when it is
     not a changed file, is drawn in (item 2a).
  2. **Each changed or drawn-in file**, per (hole-tolerant output
     section, category) pair `(O, c)` (IN-30), in three phases; merged
     input sections (`SHF_MERGE`) are item 6's only and take no space in a
     region (M-4 (e)); every address is absolute and alignment padding
     counts against a region's capacity (M-4 (g)).
     - **Keep.** A section matched to its previous version, previously
       placed in `(O, c)`, whose new size is at most its old size and
       whose old address is a multiple of its new alignment keeps its
       address. (A section that shrank keeps its start; the bytes it no
       longer uses are a hole.)
     - **Place.** The other sections of the file in `(O, c)` (grown,
       realigned, new to the file, or matched to a section previously in
       no region), in input order, each at the first of: (i) its old
       address, if it was in `(O, c)` and the bytes from there to its new
       end are free and inside the same region (it grows in place); (ii)
       the lowest aligned address in the file's regions of `(O, c)`, in
       the regions' order (the first link's region, then each overflow
       region in the order it was opened), where it fits between the
       sections already kept or placed by this update; a hole of the
       file's own region is free space like its slack.
     - **Overflow.** The sections that phase 2 could not place are
       packed, in input order, into **one new region** of the file at
       `(O, c)`'s tail reserve, with IN-33's capacity over their packed
       size. A section in an `(O, c)` where the file had no region
       (review-in72 M-4 (a): `strutil_addfn.o`'s new `.data.rel.ro`)
       goes straight to this phase.
     The file keeps every region it has had in `(O, c)` until the next
     compaction: an old region a section left is reused by a later
     version through phase 2 (M-4 (c); IN-32's "reused only by its own
     file" stands). A section that keeps its address and whose bytes did
     not change is still refilled when its file is changed (the file is
     IN-27's unit); its **definitions keep their addresses**, so their
     referrers in other files are not refilled for this file's sake.
     Modelled (`tests/incr-design/model_region.py` over review-in72's
     protobuf 3.21.12 `strutil.cc` builds, 255 globals at `-O0 -g` and 86
     at `-O2 -ffunction-sections`, the same six hops): definitions whose
     address moves, this rule against WP-202's "laid out again from the
     region's start":

     | hop | `-O0 -g` | `-O2 -ffunction-sections` |
     |---|---|---|
     | grow1 | 82 against 252 of 255 | 1 against 83 of 86 |
     | grow2 | 80 against 252 of 255 | 0 against 83 of 86 |
     | rmfn | 79 against 251 of 254 | 0 against 82 of 85 |
     | addfn | 0 against 173 of 256 | 0 against 48 of 87 |
     | base (back) | 0 against 173 of 255 | 0 against 4 of 86 |

     At `-O0` the remaining ~80 are the floor of a one-`.text` object:
     the functions after the edit inside the grown section (WP-202's 80
     in-section moves); IN-31 removes them. One overflow region was
     opened at `-O0` (grow1, `.text`, 0x8e4c bytes of a 0x1795e tail) and
     one at `-O2` (addfn, `.data.rel.ro`, 0x50 of 0x1000); no growth step
     was reached.
  2a. **Drawn-in files** (WP-207; review-in72 B-3, M-1, review-in77 B-3
     trace 1). An unchanged file is **drawn in** when the update must
     change its sections or its `.symtab` block for a reason outside it:
     a section of it becomes live (IN-71a) or kept (IN-71's election), or
     stops being so; one of its globals is demoted to a local of its
     block by the merged visibility (S-74) or promoted back. **Never for
     a common** (WP-215 R.6 1, ruled): a winning common's `RESOLVED` row
     is given whole by the rows (kind `COMMON`, the size the largest over
     the key's live `COMMONS` rows, the value the winner's alignment, the
     type, binding and `st_other` its `MENTIONS` row), so its file is
     never read for it; and v1 places no common (item 11). [later] With
     item 11's placing rule, the old and the new winning files of a
     common are drawn in for their sections (review-wp207 M-3). A drawn-in
     file is processed exactly as a changed file whose bytes are its old
     bytes: it is opened, `fstat`ed and compared with its `STAT` row
     (IN-28a condition 2; else `input-changed`; review-in72 M-6) and read
     and parsed (U4, its relocations included), its sections go through
     item 2 (its unchanged sections keep their places by the keep phase;
     a revived section, previously placed nowhere or in a hole, is
     placed by phases 2-3), its `.symtab` block is rewritten from its
     symbol table (IN-49a), its definitions that gained or lost an
     address are IN-27's moved globals, and its rows are recaptured like
     a changed file's (IN-78). It counts toward IN-16's bound. So "comment
     a call out and back in" under `--gc-sections` (W8) is an update on
     the first hop even when the first link never placed the section
     (review-in72 R6), and a COMDAT keeper that moves to an unchanged
     file's copy (R8) is one too. Cost: one read and parse of each
     drawn-in file, O(its size), *argued, not run*.
  3. **Overflow and the growth step** (IN-32). Several files' new regions
     in one tail are opened in load order of the files, then in
     (output section, category) order (M-4 (b)). When `(O, c)`'s tail
     reserve cannot hold a new region, the output section grows by a
     **growth step** into its segment's reserve:
     `step = round_up(need + max(4 KiB, size/16), A)`, where `need` is
     what the tail could not hold, `size` is the output section's size
     and `A` is the largest alignment of the output sections that follow
     it in its `PT_LOAD` (so each moves by a multiple of its alignment);
     when the step moves the RELRO end (the section is in the RELRO part
     of its segment or before it), `A` is the maximum page size. The new
     tail's capacity is the old tail's free bytes plus `step − need`
     (M-4 (d)). Every output section after it in the segment moves by
     the step, in address and in file offset (a `NOBITS` section moves
     in address only): their sections are "moved sections of unchanged
     files" (IN-27), their files' `.symtab` entries are remapped (IN-49a),
     and the program and section header tables are rewritten whole
     (IN-75). The segment reserve exhausted: `reserve-exhausted`.
     A `DEFINED` row at the end of a chunk whose size the update changes
     moves to the chunk's new end, in the layout's `placement.defined`
     (layout.md §13.1 as a full link writes it); a row past the old end
     is `update-not-implemented (layout)` (WP-245).
  4. **Packed sections** (L-142, IN-34's rows) are rewritten whole in
     full-link order within their capacity; past it, a growth step; past
     the reserve: `reserve-exhausted`. A packed section whose order
     follows `SHF_LINK_ORDER` targets that moved: `link-order-moved`.
     `.rela.plt` is rewritten whole in the full link's order (review-in72
     m-4: IN-2 item 5 holds for `IRELATIVE`;
     `two_ifuncs_added_irelative_order`).
  5. **`.eh_frame`** by IN-34a (a range rewritten from its start; on
     overflow the whole section rewritten in full-link order). Input
     (WP-216 seam 4, `mortar_core::incr::EhFile`): per changed, drawn-in
     or gone file, its kept records as the full link's selection gives
     them (S-46..S-49 over the file alone, sizes by S-47) and its
     **current rank** (S-46's order is the load order: `FILES.rank`
     composed with the update's ranks, never the `FILES` row number);
     stored: the file's `.eh_frame` range (`FILES`' eh columns,
     `LayoutRows::eh_range_of`), with its FDE count (`eh_fdes`); for the
     whole rewrite every range by current rank (`LayoutRows::eh_ranges`,
     a named whole read of `FILES`, IN-18c). `.eh_frame_hdr` is sized
     without reading unchanged ranges: the stored chunk's FDE count,
     minus the changed files' stored `eh_fdes`, plus their new ones
     (WP-243, seam 6). Output (`LayoutDelta::eh`):
     the ranges rewritten (file, offset, capacity, each record's offset),
     or the section rewritten whole with every range. A range's unused
     tail is 0 bytes, 4, 8 or 12 bytes absorbed into its last record, or
     an `EhPad` hole of at least 16 bytes; the padding CIE's bytes and
     `.eh_frame_hdr` are U-6's. A file that appears between two stored
     ranges rewrites the section whole (every later range moves and its
     FDEs' PC-relative fields change; WP-239 R-3). Unchanged ranges are
     moved by IN-34a's "Pad and move"; an encoding it does not move is
     `update-not-implemented (eh)` before W1. Remaining fall-backs: the
     whole rewrite (overflow, an appearing file), `(eh)`.
  6. **Merged sections** (S-110, L-147): a fragment is identified by
     (output section, merge class, content hash), where the merge class
     is (`sh_flags`, `sh_entsize`, alignment) of the input pieces. A
     changed file's piece whose identity is in `FRAGS` takes that
     fragment; a new identity is appended at the section's tail; a
     fragment whose reference count (`FRAGS.refs`, IN-18e) falls to 0 is
     a dead range (IN-3a: a string keeps its bytes, a constant is
     filled). **A fragment whose count rises from 0 is rewritten from the
     piece** (a dirty range, IN-75; review-in72 M-8: a constant revived
     after its fill would otherwise read the fill). No fragment of an
     unchanged file ever moves, so the contents edits WP-064a refused
     (s4, W6) need no refill outside the changed files. Its tail
     exhausted: `reserve-exhausted`. Input (WP-216 seam 4,
     `mortar_core::incr::FilePieces`): per changed, drawn-in or gone file,
     its pieces as the full link's split gives them (S-6..S-10: section,
     offset, size, output section, merge class, L128); stored: `FRAGS` by
     the identity prefix `(out, hash)` (key-keyed, two binary searches;
     never a whole read of an output section's fragments) and the file's
     `FRAG_USE` rows (its old version's fragments). Output:
     `LayoutDelta::frags`. `FRAGS.refs` after an update is exact (IN-78
     compares it with the state computed afresh): a changed, gone or no
     longer extracted file, or a COMDAT copy losing its keep, subtracts
     one per row of its old `FRAG_USE` range; a changed, appeared, newly
     extracted file or a new keeper adds one per fragment its new pieces
     name (a new key appended at the section's tail with `refs` 1); the
     key is `FRAGS`' (out, class, hash) of WP-239 S4-1. Pieces of
     sections named `.debug_*` are not fragments here (R-1: their files
     are `(debug)`). Remaining fall-back: none of its own (the tail
     exhausted is `reserve-exhausted`).
  7. **Slots** (S-113): new GOT, IPLT and TLS slots appended within the
     slot region's capacity, dead slots written 0;
     `reserve-exhausted` past it. A slot keyed by a local of a changed
     file follows IN-28b. Input (WP-216 seam 4,
     `mortar_core::incr::FileDemands`): per changed, drawn-in or gone file,
     its slot demand (key and flags) from the same per-unit scan as the
     full link's; stored: `SLOTS` (whole, with `refs`) and the file's
     `KEY_REFS` rows with `demand` (what its old version demanded; never
     `INDEX`, WP-239 S4-4). An update's
     `SLOTS` is the full link's set (IN-78's `SLOTS` row; plumb.md P-E5
     item 5 compares the GOT's words outside the update's holes with the
     full link's as multisets, so a kept slot that no file demands is an
     extra word): a slot no live file demands is dead, written 0 and a
     hole. A slot keyed by a local of a changed file is dead (IN-28b: a
     local has no identity across versions) and the new version's demand
     appended. When that key (by IN-28b's identity) has a retired row,
     the row is revived at its index (`slots_revived`); one key never has
     two `SLOTS` rows of one kind (WP-245). A file's old demand
     subtracts one from each slot its `KEY_REFS.demand` needs, its new
     demand adds one; a slot at 0 is retired, a new one appended. The
     delta carries every `SLOTS` row it writes in four disjoint lists
     sorted by `(kind, index)`, each row as it is after the update
     (WP-242 B, seam 6): `slots_appended` (an
     index past its kind's stored count), `slots_dead` (refs above 0 → 0),
     `slots_revived` (0 → above 0: its old index, written again) and
     `slots_recounted` (one count above 0 → another: the row only, no
     byte). "A hole" is a `HOLES` row of kind `DeadSlot` over the retired
     slot's words (plumb.md P-E5 compares the GOT's words outside the
     update's holes); a revival removes it. **v1** (WP-245): retiring or
     reviving a PLT or IPLT slot, and any retire or revive in an output
     with `.dynamic`, is `update-not-implemented (layout)`; every new slot
     is `reserve-exhausted`. (P-5) **The scan reads a target's
     resolution only through one value, `mortar_arch::SymFacts`
     (`ScanTables::sym`, computed by `mortar_scan::view::View::facts`)**:
     when that value of a key changes and the key has a referrer in an
     unchanged file (`KEY_REFS`), that file's stored demand may be stale,
     and the update is `update-not-implemented (slots)` (WP-239 R-4),
     decided by the join comparing the value before and after. Remaining
     fall-back: `(slots)`.
  8. **TLS**: every TLS input section's TP offset under the new layout
     equals its stored one, or `tls-layout-changed` (IN-34).
  9. **Thunks and patches** (arch.md "Incremental links") for refilled
     and moved units, in their island's capacity, or
     `island-exhausted`. **Islands are placed here, in U7, with the
     fixed capacity the first link gave them** (the region rule applies to
     an island like any section; WP-211, review-wp207 M-6): U8's refill
     set and U9's refill fill thunks into that capacity and never grow
     an island, so no section moves after U7, `DEFINED` (IN-71c) is
     final at the end of U7 and U8 is one pass. A thunk or patch that
     does not fit its island is `island-exhausted` (the reason the table
     of §7 already names for exactly this; the reviewer's text said
     `reserve-exhausted`). *Test (U-4):* `thunk_growth_moves_etext`
     (AArch64, a far call added: the island is not grown, the thunk fits
     or the update ends `island-exhausted`; `_etext` never moves after
     U7).
  10. **Sections and segments**: a section that has no output section
      in the stored layout, or an output section or segment left empty:
      `new-output-section`. An allocated note section (`.note.gnu.property`,
      `.note.ABI-tag`, `.note.stapsdt`, any `SHT_NOTE`) changing size:
      `property-changed` (review-in72 m-2; the reason keeps its name;
      `note_abi_tag_size_change`). **The merged properties and the stack
      flag** (WP-214 seam 6, ruling 3; the join's): if no file of the
      change differs in `FILES.stack` or `FILES.properties` (the new
      bytes' values against its stored row) and no file appears, goes,
      is newly or no longer extracted, nothing is read. Otherwise one
      pass over the column of every live relocatable object (the named
      whole read of IN-18c) computes the merged value with the stored
      values and with the update's, by the SAME merge functions as the
      full link (S-95's over the files' properties; L-84's over their
      stack notes and the unchanged `-z execstack` option): unequal is
      `property-changed` (a full link); equal goes on. No counters; exact
      also when the only limiting file goes. *Tests (the capture package
      and the join, on real objects):* a file recompiled without
      `-fcf-protection` / `-mbranch-protection` falls back by name; the
      sole limiting file leaving falls back by name; an ordinary edit
      reads nothing. An
      output section's alignment growing past its stored address's:
      `reserve-exhausted`.
  11. **Commons.** **v1** (WP-216 R-2): commons stay in the commons
      chunk (layout.md L-88) in every mode, as the capture stores them,
      and an update places none. An update in which a winning common
      appears, goes, becomes a definition (or a definition a common), or
      changes size or alignment is `update-not-implemented (common)`,
      decided by the join (U-10) after U-3's re-election, from
      `ResolveDelta::commons` against the stored `COMMONS` rows, before
      the layout is called; a winning mention that only moves to another
      file changes nothing in the layout and is no fall-back. **[later]**
      (review-in72 M-2, option (a)): in an incremental-mode
      output each winning common is placed as a section of its **winning
      file: the file of its winning mention** (resolve.md §3.3: `def` is
      `Common{file, st_index}` of the least common mention by ≺; WP-211,
      review-wp207 M-3: `cfirst(K)` is §4.1's extraction target, a
      non-common definition, and undefined for a key with only common
      mentions), of the common's size and alignment (the largest over its
      live mentions, `COMMONS` rows), in that file's `.bss` region; the
      map's `common` line names the file (its fifth field, written in an
      incremental-mode map only: `spec/plumb.md` §3.2). When the winning mention moves
      to another file, the old and the new winning files are drawn in
      (item 2a): the common's section leaves the one (a hole) and is
      placed by item 2 in the other. A size or alignment change is a section of that file that
      grew or changed: item 2 places it (the file is drawn in when it is
      not changed), and its key is affected (IN-71). A plain link keeps
      the commons chunk as today.
  12. **`.symtab`, `.strtab`, `.symtab_shndx`**: IN-49a.
  **Determinism** (IN-6). The result is a function of the stored layout
  (the state's `Placement` rows) and the new inputs' sections (identity,
  size, alignment, order) only: files in load order, regions in their
  order, sections in input order, no iteration over a hash map's order,
  no dependence on the thread count. IN-61's replay of the next link
  reads the update's `Placement` from the state it wrote, so a replay
  reproduces it. The result is a `Placement` of the same shape as a full
  link's (a layout that `Layout::check` accepts), with `hole` and `dead`
  lines for the map, and the list of what moved: the input of IN-74.
  *Test (U-4):* hand-made layouts per item; `edit_moves_only_changed_sections`
  (the model's series as a test: the moved-definition counts above);
  `new_section_without_region_opens_tail_region`;
  `two_files_overflow_in_load_order`; `old_region_reused_after_overflow`;
  `growth_step_alignment_and_relro`; `merged_input_not_in_region`;
  `comdat_election_before_region`; `alignment_padding_counts`;
  `delta_layout_same_on_1_and_n_threads`; `gc_section_dies_then_revives`
  (first link without the section, too), `comdat_keeper_moves_to_unchanged_file`,
  `n2_visibility_demotes_unchanged_definer`,
  `merged_constant_dies_revives` (`n4_common_grows`, `common_appears`,
  `two_commons_one_grows` and `common_winner_moves_to_unchanged_file`,
  WP-211, are the join's in v1: WP-216 R-2),
  `hole_inside_kept_shrunk_section` (the freed tail of a kept section is
  filled and listed exactly as a region's slack, IN-3a, P-I5; WP-211 from
  review-wp207 §10), `link_order_target_dies` (an unchanged file's
  `SHF_LINK_ORDER` section whose target dies is drawn in or the update
  ends `link-order-moved`; WP-211), `cross_file_eh_frame_fde_target_dies`
  (an FDE of an unchanged file whose function section dies: the range
  drops the FDE as the full link does; WP-211),
  `alignment_growth_is_reserve_exhausted` (item 10),
  `region_with_three_sections_one_grows` (WP-211, review-wp207 §10: the
  region models were `.text`-only), each three hops; L10 grow and shrink steps past each capacity in turn
  (IN-32's test).

- **IN-73 [v1] What WP-064a refused, and where it now goes.** Every edit
  of WP-064a §3's list is an update or one named reason:

  | WP-064a §3 edit | v1 rule | reason when the rule cannot |
  |---|---|---|
  | a weak definition made global, or the reverse (W1, W1b) | IN-71 (binding), IN-27 global row | none in a static executable |
  | a call bound to another definition (W9) | IN-71, IN-27 | none |
  | a definition moved inside its section (n1) | IN-72 item 2, IN-27 global row (value) | `reserve-exhausted` |
  | visibility (n2) | IN-71 (static), IN-27 | none |
  | a function made an IFUNC (n3) | IN-71 | `symbol-kind-changed` |
  | a common made a definition (n4) | IN-71, IN-72 items 2 and 11 | `update-not-implemented (common)` in v1 (WP-216 R-2); [later] `reserve-exhausted` |
  | a local made global (W10) | IN-71 (a key appears), IN-49a | `symtab-exhausted` |
  | a `--gc-sections` liveness flip (W8) | IN-71a, IN-27 liveness row, IN-72 item 1 | `reserve-exhausted` |
  | merged contents changed (s4, W6) | IN-72 item 6 | `reserve-exhausted` |
  | a GOT slot or thunk keyed by a section symbol (s3) | IN-28b (section identity) | none |
  | a section's count, type, flags, size or alignment (`layout-changed`) | IN-72 items 1-3 | `reserve-exhausted`, `new-output-section` |
  | a file named twice on the line | every mention is a file (model §7.1); each is a changed file | none |

  `layout-changed` and `resolution-changed` are the test path's and are
  never an update's reason. *Test:* each row's case from
  `crates/mortar/tests/incr_review.rs` and `tests/incr-review/`, judged
  by IN-79.

- **IN-74 [v1] The refill set of an update.** `delta::compute` (WP-063)
  with the stored state as the old side and the update's own layout
  (IN-72) and resolution (IN-71) as the new side; IN-27's table is the
  rule. The referrers of a target are found without reading an
  unchanged input: by `INDEX` (IN-25), read by the referring file's
  range for the files that `KEY_REFS` lists for a key (every mentioning
  file, single mentioners and file 0's keys included, IN-18e), and by the
  target file's own range for its sections (IN-18e). The keys whose
  referrers are looked up are the affected keys of IN-71 whose
  resolution, value or size changed, the linker-defined keys of IN-71c
  (known after U7), and the globals defined in moved sections. The set's
  bytes over IN-16's bound: `too-much-changed`. *Test:* the IN-63a
  oracle on every update (IN-79 (c)); the mutation lane (IN-63b) with
  the update's layout. *Bench rows* (IN-64, WP-207): per update, the
  moved-definition count (definitions of changed and drawn-in files whose
  address moved) and the refill set's size (units, bytes, referrer
  files), so that M-5's cost is watched, not only judged.

- **IN-75 [v1] Refill, dirty ranges, build id** (amended by WP-207;
  review-in72 B-4, M-6, M-7). The units of the set are filled (image.md
  I-95) into windows, and only the **dirty ranges** are written. The rule
  is an invariant: **the dirty ranges cover every byte in which the
  update's output differs from the previous output**, and outside them
  the previous output's bytes are the update's. They include, and are
  not limited to: the refilled units' bytes; the holes they left (their
  fill, IN-3a); the packed sections IN-72 item 4 rewrote; the fragments
  appended or revived (item 6); the slots appended and the dead slots
  written 0, and `.rela.plt` (item 7); the `.eh_frame` range and its
  padding; `.eh_frame_hdr` (rewritten whole when an FDE address changed,
  S-111); the `.symtab`/`.strtab` entries and names IN-49a changes; the
  ELF header; **the program header table and the section header table,
  each whole, whenever the update changed any field of either** (a
  growth step changes `p_filesz`/`p_memsz` and every later section's
  `sh_addr`/`sh_offset`; IN-34's "rewritten whole"); every byte a growth
  step moved; `.debug_aranges` (G packages); and the build-id note. The
  enumeration is the implementation's; the invariant is the test.
  **Inputs read here** (review-in72 M-6): every input read for a refill,
  a placement or a drawn-in file (IN-72 item 2a) is opened, `fstat`ed and
  compared with its `STAT` row (IN-28a condition 2) before its first byte
  is used; a difference is `input-changed`. The build id is the tree
  hash of image.md I-76 over the patched image: the leaves of the blocks
  the update wrote are recomputed from the new bytes, every other leaf is
  the state's (`LEAVES`), and the tree is combined; the note's own bytes
  are hashed as I-76 says (zeroed). The new leaves go to the state's
  patch. The update's map (`--mortar-map`) is the full link's map grammar
  with the update's placement, `incremental`, and its `hole`/`dead`
  lines, written whole (IN-49): the judge reads it (IN-79 (b)). A
  synthetic chunk that contains a hole is written as one chunk line per
  part between its holes, so that every byte of the output section is
  covered by exactly one line of the map (plumb.md P-I1); `.got` with a
  `DeadSlot` hole is the case (WP-245). *Test
  (U-6):* the refilled windows of an update with an empty delta equal the
  old output's bytes; `dirty_ranges_cover_every_differing_byte` (the
  update's output against the previous output, byte by byte: every
  differing byte inside a dirty range); `growth_step_rewrites_headers`
  (readelf of both header tables against the full link of the same
  `Placement`, and IN-79 (a)); `input_rewritten_after_u4_is_input_changed`;
  the build id of an update equals the tree over the whole new output
  (`xtask plumb` recomputes it).

- **IN-75a [v1] Diagnostics equal to a full link's** (WP-207). An update
  emits the warnings the full link of the same inputs would emit, in the
  same order (amended by WP-211, review-wp207 M-5). The state's `DIAGS`
  table (IN-18e) holds every warning of the link that wrote it, and one
  row per `.gnu.warning.K` section of a linked file whose key no live
  reference reaches (stored, never printed; WP-221 §2.4), each with
  its **subjects**: **every** file and key the warning's text or its
  existence depends on, plus an output section or the link where the code
  depends on the layout, as `spec/diag.md`'s code table gives them for
  the code (plural: a reference-site warning's subjects are the referring
  file and the key; a `.gnu.warning.SYM` warning's are the file holding
  the warning section, every referring file and the key, since a changed
  referrer decides whether it appears and its "referenced by" text:
  `tests/incr-design/review-wp207/gnu-warning.sh`). A key subject is a
  `SYMBOLS` row or an ODR key (`ODR_CLASSES`' key, for the
  one-definition rule's record codes, WP-236; W1 and W2's subject is the
  `SYMBOLS` row, WP-220); `DIAGS`' row says which (WP-212, F-12).
  Key-implied files are not stored: a file the warning names or that
  decides it and that mentions the key is in IN-71's affected set
  through the key whenever it is touched, so the key subject stands for
  it, and a key referenced by 1,000 files is one subject, not 1,001
  (WP-221 F-6). A `DIAGS` row stores the warning's model
  (`mortar_diag::Diagnostic`: its kind's fields, its place and its
  context lines, as `mortar_diag::subject::stored` writes them), never a
  rendering: the program name, colour and the demangling hook are the
  invocation's (`Style`, diag.md §5.1), not in the plan hash, and are
  applied when the update renders its report; so a repeated row renders
  under the update's `Style` as the full link's would (WP-221 §1.1).
  An update drops every row with
  **any** subject that is a changed, appeared, gone, drawn-in, newly
  extracted or no longer extracted file, an affected or ODR-affected key
  (IN-71, IN-71d), or an output section or the link: recomputed every
  time, from the update's `Placement` or by the update's own front
  (U1-U4) for a warning the front raises. A reference-site row's
  references in a drawn-in file are dropped and re-read: a section
  killed or revived by a re-elected group is in a drawn-in file (IN-72
  item 2a), whose keys are affected (IN-71; WP-221 §2.2). A stored
  reference-site warning keeps at most 64 references and the count of
  all of them; a row whose count exceeds what it keeps cannot be rebuilt
  when its key is affected: the update falls back
  (`update-not-implemented`, detail `diags`). **A warning with the subject
  `link` that the update cannot raise again within O(change) is never
  repeated from the stored row: the update falls back
  (`update-not-implemented`, detail `diags`), by code, until U-9 shows
  per code how it is recomputed.** An option is not a reason for
  `link`: the options are in the plan hash, and a changed plan is a full
  link (`options-changed`) (WP-212, F-13). The update recomputes those
  subjects' warnings and emits the union in diag.md §6.2's report
  order. **The
  stored order** is diag.md §6.1's `Key` (`phase`, `unit`, `seq`) with
  `unit` stored as the file's `FILES` row (never as an ordinal, which an
  appeared file or a rank midpoint makes stale) and mapped at emission to
  the `unit` the full link of the update's inputs would use (the file's
  current position in `FILES.rank` order, IN-18e); then §6.2's text and
  field tie-breaks. A phase-20 row's unit is its argument; the row is
  dropped when that argument's input is not clean (IN-22, IN-24) and
  recomputed by U2's walk (WP-221 F-4). A warning code without
  subjects in diag.md's table is
  a spec defect that `spec_tables` reports (REQUEST to diag.md, WP-211
  §R.7). `--fatal-warnings` (diag.md §6.3) makes a warning an error, and
  a link with an error writes no state, so a state never holds a warning
  that option would make fatal; turning the option on or off changes the
  plan hash (`options-changed`). Errors are not stored: a link with an error writes no state
  (IN-48). *Test:* `warning_repeated_by_update` (an unchanged file that
  draws a warning in the full link: the update prints it too),
  `warning_gone_with_its_file`, `gnu_warning_reference_removed`
  (review-wp207's `gnu-warning.sh` as a test of U-9: an unchanged libc.a
  member's `.gnu.warning.mktemp`, a changed `c.o` that calls `mktemp`,
  stops, calls it again: the warning and its "referenced by" text equal
  the full link's on every hop), `warning_order_after_appeared_file`
  (an appeared file before a warned one: the order follows the new
  ranks), `fatal_warnings_update_fails_like_full_link` (exit status and
  no state written, as the full link), and the ODR three-hop series
  (IN-71d);
  every IN-79 run compares the update's standard error with the full
  link's, modulo the `incremental:` report line. *Argued, not run.*

- **IN-76 [v1] The write path.** In place (IN-46) is the default when
  the output the state describes is at least `small-output`'s bound
  (`mortar_incr::SMALL_OUTPUT`), and the copy path (IN-47) otherwise
  (an update below the bound runs only under `--incremental=update`);
  `--incremental-write=copy` forces the copy path. In place is taken
  only when every condition of IN-45 holds; when one does not, the
  update takes the copy path (not a fall-back) and the report says
  which (`Written::copy_because`, WP-219: the condition's sentence when
  one forced the copy path; `None` in place, and `None` when the copy
  path was the policy's default for the size or `--incremental-write=copy`,
  which the report tells apart by the size and the option). The
  output's new size is an explicit input of `write` (the join passes
  U-6's; W3 truncates or extends to it, image.md I-97). This amends IN-45's default (the operator's write-path
  decision, WP-094 §8 item 5: the copy costs 1.14 s on g10x and 3.96 s
  on `cxx-large-g` on this host's file systems, WP-105 §2). *Test:*
  one link test per IN-45 condition, the inode checked; the default
  above and below the bound with `--incremental=update`.

  *Decided (operator, 2026-11-17, through WP-207): in place by default
  at or above `SMALL_OUTPUT` is "fine for now"*, on the conditions
  review-in77 set (IN-77a's descriptor, IN-77b's lock, IN-45's list of
  unprotected readers, IN-77's aside name), all now in the text. The
  write path is a **policy constant** (`mortar_incr::WRITE_PATH`: the
  bound and the default path per side of it), so that **IN-76a** is a
  switch, not a rewrite.
- **IN-76a [later: package W-1, after U-10] Copy by clone where the file
  system clones** (operator's follow-up, 2026-11-17). A probe decides,
  per output directory, whether the copy path would be a clone; where it
  is, the copy path (IN-47) becomes the default at every size, since it
  protects every reader IN-45 lists. **The probe, in safe Rust and `std`
  only**: the output's `st_dev` (from U1's `fstat`) is looked up in
  `/proc/self/mountinfo` (field 3, `major:minor`), and the mount's file
  system type is read from it; `btrfs`, `xfs`, `bcachefs`, `ocfs2` and
  `zfs` are clone-capable types (XFS only with `reflink=1`, which
  mountinfo does not show, so XFS is confirmed by a timed trial: below).
  For a candidate type, the first incremental-mode full link times
  `std::io::copy` of its own fresh output into a temporary (it copies
  through `copy_file_range`, which clones first on these file systems,
  IN-47) and records in `META` (device, type, copy time per MiB); the
  copy is a clone when it costs under 5% of the byte-copy rate measured
  on this host (0.6-0.8 ms/MB, IN-47). **Cost per link**: one read of
  mountinfo (a few KiB, tens of µs, *estimate*) when the device differs
  from `META`'s, else nothing; the timed trial once per state.
  **When the probe lies**, nothing is wrong, only slow: a copy that was
  not a clone is still a correct copy (IN-47 is correct on any file
  system); `--incremental-report` prints the copy's time, and an update
  whose copy took more than twice the recorded time clears the record,
  so the next update goes back to in place. *Test (W-1):* the probe's
  parse of mountinfo samples (btrfs, xfs, overlay, virtiofs, nfs4); a
  loop-mounted btrfs or XFS (`reflink=1`) image in the system temp where
  the container allows it (WP-061), else *argued, not run*.

- **IN-77 [v1] The crash protocol: one order of writes for the output
  and the state** (amended by WP-207: one descriptor per file, one lock,
  the output set aside while torn, the boot id, failures). Let `S` be the
  state file (format 5, written in place by blocks, IN-46b) and `P` the
  output. The **flag** is `S`'s updating word (IN-18c: one aligned 4-byte
  word in the header, outside every hash; one sector, so a write of it is
  all or nothing). A state whose flag is set is never trusted:
  `output-torn`, a full link.

  **IN-77a [v1] One descriptor per file** (review-in77 B-1). U1 opens `S`
  read-write and `P` once (read-write for in place, read-only for the
  copy path) and makes IN-21's checks through those descriptors:
  `fstat` gives the tuple, and the build id and the racy leaves are read
  with positioned reads on the descriptor. Every later read and write of
  `P` (C1's copy, W2..W4) and of `S` (W1, W5, W6, the compaction's read)
  goes through them; no step reopens either by path. Immediately before
  W2 (in place) or C3 (copy), `stat(P)` by path must give the
  descriptor's `(dev, ino)`, size, `mtime` and `ctime` as U1's `fstat`
  gave them (the join hands U1's tuple to `write`, WP-219), else
  `output-changed`, nothing written (`T` removed); `write` also compares
  the descriptor's own `fstat` with U1's before W1. One action of the
  update itself changes `P`'s inode between U1 and W2: removing a
  leftover `A` that is a second name of `P` (IN-77d) changes its link
  count and `ctime`; after that removal the `ctime` compared is the one
  the descriptor gives once `A` is gone (the other four fields are
  still U1's). **The limit** (WP-219 F-7): a foreign write to `P` in
  place within the timestamp tick of U1's `fstat` that keeps the size
  (the tuple is then equal), or a write through a descriptor opened
  before W2, is not seen; IN-21 has the same limit for every link. W4's and C3's tuple is the descriptor's (in
  place) or `T`'s descriptor's (copy) `fstat`, taken by IN-21's binding
  (`fstat`, stamps until one is later, `fstat` again; review-in77 M-8,
  m-5). In place, the descriptor is open for writing **from U1 to W4**
  (U1 opens it read-write; WP-211, review-wp207 m-1), so from U1 an
  `exec` of `P` fails with `ETXTBSY` where the kernel denies it (measured
  on overlay and virtiofs, kernel 6.8, `tests/incr-design/review-wp207/aside.py`
  (1); IN-45 names where it does not).

  **IN-77b [v1] One lock** (review-in77 B-2; replaces "image.md I-59's
  lock" and IN-45's lock condition). Every link in incremental mode (full,
  update, `--incremental-report`) that finds a state takes an exclusive
  `flock` (`File::lock`, `std`) on `S`'s descriptor before it reads any
  byte of `S` past the header's magic, and holds it to its last write.
  After acquiring it, `stat(S)` by path must give the locked
  `(dev, ino)`; else it closes the descriptor and starts again (at most 8
  times, then `state-busy`, a full link that writes no state). A rename of
  `S` (the compaction, a full link's state write) happens while the lock
  is held, so a waiter wakes on the old inode, fails the check and
  retries on the new one. Where the probe of image.md I-59 says `flock`
  is not enforced on `S`'s file system, no update is attempted
  (`lock-unavailable`, a full link), on both write paths: the copy path
  does not make an unlocked update safe, because `S` is patched in place
  on both. That full link **writes no state, and removes a state at `S`
  by its exact name** (WP-219 rulings-2): IN-77d's temporary rests on
  its `try_lock`, which is not enforced there either, so two floors
  could write one temporary; and no later link could use the state,
  since each answers `lock-unavailable` before it reads a byte. Its
  `incremental:` line names the reason each time. A link that finds no state (the first link, or after
  `state-corrupt`) has nothing to lock; two such links racing are IN-21's
  post-rename check (the loser removes the state it wrote) and IN-77d's
  lock on the state's temporary. `ENOENT` on `S` is "no state", also
  where a file system shows it spuriously: measured on the workspace's
  virtiofs (WP-219), a process's `open(S)` while another renames a file
  over `S` gave `ENOENT` 12 times in 42738, with the name still listed;
  `stat(S)` and a second hard-link name of a renamed-over file never
  failed. Such a link is a full link that takes no lock, covered as two
  first links racing are: IN-21's post-rename check, the binding of `S`
  to `P`'s tuple (a state that describes another link's output is
  `output-changed` at the next link), and IN-77d's temporary lock; there
  is no retry by guess (tested: `full_link_during_compaction`). `S`
  with `st_nlink` > 1 or reached through a symbolic link is never
  patched: W5 is the compaction (a rename), so a hard-linked snapshot
  keeps its blocks (review-in77 m-3); W1 sets the flag in the inode it
  shares, so the snapshot reads as `output-torn` (safe: a full link;
  WP-219 F-2). The entry points (WP-219, `mortar::update::write`):
  `lock_state` (open, `flock`, the `(dev, ino)` check, at most 8
  rounds, I-59's probe), which the update's U1 and the floor
  (`mortar::incremental::start`, every mode that finds a state) call. **Cost**: a plain link opens no state and takes no
  lock (IN-11: it touches no state path), so it pays nothing; an
  incremental-mode link pays one open, `flock`, `fstat`, `stat` and
  unlock: median 352.5 µs (p90 1.15 ms) on the workspace's virtiofs and
  3.5 µs (p90 4.4 µs) on overlay, at load ~20 (WP-207
  `tests/incr-design/fsprobe.py`), against a full link's 0.18 s on
  protoc.

  **In place** (`A` = `dir(P)/.<base(P)>.mortar-torn`, the **aside name**, in `P`'s
  directory):
  - **W1** set the flag; `fdatasync(S)`.
  - **W2** a leftover `A` has been removed (IN-77d; `rename(P, A)` onto
    a second name of the same inode does nothing and would leave `P`
    named); `rename(P, A)`; `stat(A)` must give the descriptor's
    `(dev, ino)`, else the foreign file is put back by `link(A, P)` and
    `unlink(A)` (on `EEXIST` a newer file is at `P` and `A` is only
    unlinked; never `rename(A, P)`, as W4) and `output-changed` (the flag
    stays set: a full link); `fsync` the directory; zero `e_ident`
    through the descriptor; `fdatasync(P)`. From here `P`'s path names no
    file.
  - **W3** write the dirty ranges; truncate or extend (image.md I-97);
    `fdatasync(P)`.
  - **W4** write the true ELF header; `fsync(P)` (data and metadata, so
    the mtime the writes set is durable: review-in77 m-6); `link(A, P)`
    (`std::fs::hard_link`; WP-211, review-wp207 M-4: `rename(A, P)`
    would silently replace a file another link put at `P` while it named
    nothing, since a plain link takes no lock and a full link renames
    `T` over `P`); `fsync` the directory. **On `EEXIST`** the update
    leaves `P` as it is, removes `A` (`unlink`, `fsync` the directory),
    keeps the flag set (the next link is a full link) and fails with
    `output-changed`; nothing more is written. Otherwise `unlink(A)`;
    `fsync` the directory. `P` is the new output; its tuple is taken now
    (IN-77a).
  - **W5** write the state's patch: the changed blocks, their block-list
    entries, the directory entries, and the header except its flags word
    (bytes 16 to the end, review-in77 m-2), with the new binding (the
    tuple of W4, the build id, the new leaves, `output-synced` 1);
    `fdatasync(S)`.
  - **W6** clear the flag; `fdatasync(S)`.

  **Why the aside name** (review-in77 M-3, decided by WP-207 against the
  epoch mtime). A torn output must not look up to date to the build
  system, or the repairing full link never runs. Measured
  (`tests/incr-design/bsprobe.sh`, GNU Make 4.4.1, ninja 1.12.1): a torn
  output with a fresh mtime is relinked by ninja but **not by make**; an
  epoch-dated or missing one is relinked by both. The epoch mtime of
  review-in77's fix does not survive the writes: every `pwrite` after
  `utime(0)` sets the mtime to now (measured on virtiofs and overlay,
  `fsprobe.py`), so a kill inside W3 leaves a fresh torn file; the crash
  model with mtimes (`tests/incr-design/crash_model.py`) gives 2 fresh
  torn outcomes for the epoch variant and for W1..W6 as WP-202 wrote them,
  and 0 for the aside variant (kill and power loss at every prefix; 0
  trusted mixes in all three). Removing `P` on failure (I-66) covers
  failures only, not kills or power loss. With the aside name, after any
  crash in W2..W4 `P` is missing: make and ninja relink, `exec P` fails
  with `ENOENT`, and a reader that opens `P` in the window fails instead
  of copying a mix (it narrows review-in77 M-2 to readers that opened
  before W2). A debugger holding `P` keeps its descriptor on the same
  inode, as before (IN-45: unprotected); gdb's `run` in the window finds
  no file, and after W4 sees the new mtime and re-reads. Cost: a
  rename, a link, an unlink and three directory `fsync`s; review-wp207
  measured the whole W1..W6 with the lock (`syncprobe.py`, 50 MB output,
  four 1 MiB windows, two renames): median 26.7 ms (p90 37 ms, one 1.1 s
  outlier in 15 rounds) on virtiofs at load ~12 and 3.0 ms on overlay.
  **W4 as `link` + `unlink`** (WP-211, `crash_model.py` variants
  `inplace-aside-link`, `-link-foreign`, `-rename-foreign`; kill and
  power loss at every prefix, the link and the unlink each durable only
  after their own directory `fsync`): 0 trusted mixes and 0 fresh torn
  outputs in all three; a foreign output created at `P` inside the
  window is lost in 6 outcomes with `rename(A, P)` (one of them a
  trusted new pair over the lost file) and in 0 with `link`; a kill or
  power loss between the link and the unlink leaves `P` and `A` naming
  one inode (2 outcomes) with the flag set: the next link is a full link
  and the lock holder removes `A` (IN-77d) before anything else.

  By copy (`T` = image.md I-59's temporary beside `P`, created and
  `flock`ed by this link):
  - **C1** create `T`, copy `P`'s descriptor into it
    (`copy_file_range`, a clone where the file system clones, IN-47),
    write the dirty ranges and the true header; `fdatasync(T)`.
  - **C2** set the flag; `fdatasync(S)`.
  - **C3** check `P` by path (IN-77a); rename `T` over `P`; `fsync` the
    directory; take `T`'s descriptor's tuple.
  - **C4**, **C5**: W5 and W6.

  When the state's rows do not fit its slack (IN-18e), or `S` is
  hard-linked or a symbolic link (IN-77b), W5/C4 is the **compaction**
  instead (IN-18f): the whole new state, flag clear, written to `S`'s
  temporary `dir(S)/.<base(S)>.mortar-compact` (opened and `flock`ed by this link as IN-77d says),
  `fdatasync`ed, renamed over `S`, the directory `fsync`ed; W6/C5 is then
  nothing (the rename replaced the flag).

  **IN-77c [v1] The full link in incremental mode and the boot id**
  (review-in77 M-1, form (b)). A full link renames `P` unsynced (image.md
  I-61) and then writes `S` durably, so after a power loss `S` can be
  durable while `P`'s data is not. `META` carries the **boot id** (the 16
  bytes of `/proc/sys/kernel/random/boot_id`, read once per link) and an
  **`output-synced`** bit: a full link writes 0; W5/C4 writes 1 (`P` was
  synced at W4 or `T` at C1). U1, when the state's bit is 0 and its boot
  id differs from the current one, compares every leaf of `P` with
  `LEAVES` before it trusts `P` (IN-21's racy path, O(output), once per
  reboot: after a reboot the page cache is gone, so what it reads is what
  is on the disk); a leaf that differs is `output-changed`; when all
  match, the update proceeds and W5 writes the bit 1. Where the boot id
  cannot be read, a full link in incremental mode `fdatasync`s `T` before
  its rename (form (a)) and writes the bit 1; an **update** that cannot
  read it treats it as changed: with the bit 0 it compares the leaves
  (WP-211, review-wp207 m-2; with the bit 1 nothing is needed). The
  update's side is `mortar::update::write::boot_check(view, out, boot)`
  with `boot_id()` (WP-219): `META`'s `boot_id` and `output-synced`
  through the seam's accessor, the leaves read through `P`'s
  descriptor. In a container the boot id
  is the host's (not namespaced); in a virtual machine it is the guest's,
  which also changes on any host power loss: both are conservative.
  Modelled (`crash_model.py` variant `full-incremental-bootid`): 0
  trusted mixes, against 7 (build id) and 15 (`--build-id=none`) for
  today's full link; the torn outputs it leaves after a power loss are
  any linker's unsynced output (I-61), out of this rule's scope.

  What a kill (or a power loss: each sync orders the writes before it
  ahead of every write after it) leaves, and what the next link does
  (U1 reads the flag before any hash, review-in77 m-1; the leftover names
  are removed by the next lock holder, IN-77d):

  | killed after | `P` | `S` | next link |
  |---|---|---|---|
  | nothing written | old | old, flag clear | an update from the old pair |
  | W1 | old | old, flag set | `output-torn`: full link |
  | W2, W3 | missing (the torn file is at `A`, `e_ident` zero) | old, flag set | `output-torn`: full link; make and ninja relink |
  | W4 before the link | missing (`A` complete) | old, flag set | `output-torn`: full link |
  | W4 between the link and the unlink | new, also named `A` | old blocks, flag set | `output-torn`: full link; `A` removed by IN-77d |
  | W4 | new | old blocks, flag set | `output-torn`: full link |
  | W4, `P` taken by another link (`EEXIST`) | the other link's | old blocks, flag set | `output-torn`: full link (`A` removed by the update, else by IN-77d) |
  | W5 (part or all) | new | some blocks new, flag set | `output-torn`: full link |
  | W6 | new | new, flag clear | an update from the new pair |
  | C1 (part or all) | old (`T` beside it) | old, flag clear | an update from the old pair |
  | C2 | old | old, flag set | `output-torn`: full link |
  | C3 | new | old, flag set | `output-torn`: full link |
  | C4, C5 | as W5, W6 | | |
  | compaction, before the rename | new | old, flag set | `output-torn`: full link |
  | compaction, after the rename | new | new, flag clear | an update from the new pair |

  **Why never a silent mix.** A link trusts `S` only with the flag
  clear. The flag is clear in exactly two states of the protocol: before
  W1/C2, where `S` and `P` are the old pair (C1 writes only `T`), and
  after W6/C5 (or the compaction's rename), where W5 and every write to
  `P` were synced first, so they are the new pair. Every other state has
  the flag set. Independently of the flag, IN-21's binding is checked by
  U1 through the descriptor (IN-77a), so an output replaced by anyone
  between links, or between U1 and W2/C3, is `output-changed`; a `S`
  whose blocks are torn is `state-corrupt`; two links of one state are
  serialized by IN-77b; and a full link's unsynced output is checked once
  after a reboot (IN-77c). The power-loss half holds as far as the file
  system's `fdatasync`/`fsync` does (on a virtiofs mount whose host is
  macOS, plain `fsync` is not `F_FULLFSYNC`: *argued, not run*).

  **IN-77d [v1] Leftovers** (review-in77 m-4). The temporaries are `A`,
  `T` and `dir(S)/.<base(S)>.mortar-compact`; each is `flock`ed by its creator. The
  link that holds `S`'s lock removes each of them by its exact name before
  its own writes, after a `try_lock` on it succeeds (a held one belongs
  to a live link and is left). An `A` left between W4's link and unlink
  is a second name of `P`'s inode: removing it (`unlink`) leaves `P`
  intact (WP-211). No pid is in a name, so pid namespaces and
  pid reuse do not matter. Where the lock is not enforced nothing is
  removed (and no update runs, IN-77b). The floor's state temporary is
  the compaction's name, `dir(S)/.<base(S)>.mortar-compact` (WP-219;
  formerly `<S>.tmp-<pid>`), and every writer of that name follows one
  protocol (WP-219 F-8): it **opens the name without truncating it**
  (creating it if absent), `try_lock`s it, and checks that the name
  still names the locked file. A held lock, or a name that names another
  file once locked, is another live link's temporary: the floor writes
  no state (`state-busy`, the link itself succeeds; any state at `S` is
  left, as IN-21 leaves it when no state is written); the update's
  compaction fails (IN-77e: the flag stays set). Once locked, I-59's
  probe runs on the temporary (a second descriptor's `try_lock` must
  fail): where it does not, the lock is not enforced and the floor
  writes no state (`lock-unavailable`; the temporary is removed by its
  exact name, any state at `S` too, IN-77b); this covers the link that
  found no state, which has no `S` to probe. A free one is a
  leftover: truncated and reused. So two links that find no state
  never write one temporary at once, and the sweep above, which removes
  the name only once it holds the lock, never removes a live link's
  temporary: between a writer's open and its `try_lock` a sweep can
  take the lock and remove the name, and the writer's check after its
  lock then sees the name gone (`state-busy`).

  **IN-77e [v1] Failures** (review-in77 M-5; IN-48 covers failures of the
  link proper, before U10). Any error of a write, a sync, a rename or a
  `set_len` ends the update at once: no further write is issued to `P`
  or `S`, and **no sync is retried** (after `EIO` Linux marks the pages
  clean, so a retry can return 0 with the data lost).
  - Before C2 (copy): `T` is removed; `P` and `S` are left (the old
    pair); the link fails.
  - Before W1 (in place): nothing was written; the link fails.
  - **A write without a planned state patch is refused before W1 and
    before C1** (`state-corrupt`: a full link; batch 13, the junction of
    WP-214 and WP-219). W5/C4 is what binds the state to the new
    output; with the empty plan it would write nothing and W6/C5 would
    clear the flag over a state that still describes the old output.
    Every update changes the output's tuple, so every update has a
    plan (`write_refuses_the_empty_plan_before_w1`).
  - From W1/C2 on: the flag stays set (or is set: a failed W1 leaves it
    as the disk has it, and the next link reads it), except when the
    last sync fails (W6's or C5's `fdatasync`, or the compaction's
    directory `fsync` after its rename): the cleared word, or the new
    state, is then already what the kernel holds, and the next link may
    read it (WP-219 F-4). The invariant is what it always was: after any
    failure the next link sees the old pair, the new pair, or a named
    reason; never a trusted mix (`power_loss_every_prefix_and_subset`'s
    prefix ending at W6's or C5's flag write, with and without that
    write durable; `failed_sync_is_not_retried` at that sync). In place, the file at
    `A` (or at `P` after W4's link, `A` too) is removed (I-66: it is torn or
    unsynced), which frees its blocks for the next full link's `T`; by
    copy, `P` is the old or the new file and is kept. The link fails, and
    the next link is a full link.
  - The compaction's temporary failing: it is removed; the flag stays
    set.

  **IN-77f [v1] Readers of `S`** (review-in77 m-8). No consumer of `S`
  uses a row of a state whose flag is set, or whose blocks it has not
  hashed (IN-18c): mortar's links, `--incremental-report`, `incr_state`,
  and any later tool (a debugger's helper, Tuck). The verifier reads no
  state (IN-60).

  **Crash hooks.** A test-only environment variable,
  `MORTAR_INCR_CRASH=<point>`, aborts the process at `<point>`: after a
  step's last call (`W1`..`W6`, `C1`..`C5`, `compact-written`,
  `compact-renamed`), and **between a write and its sync**
  (`W2-renamed`, `W2-written`, `W3-written`, `W4-written`,
  `W4-linked`, `W4-unlinked`, `W5-written`, `C1-written`; review-in77
  m-7, WP-211); a binary
  built without `cfg(test)` or the `testhooks` feature does not read it.
  Power loss is tested without a device: U-7 routes every write, sync and
  rename through a `WriteOps` trait whose test implementation records
  them, and the test replays every prefix with every subset of the
  unsynced operations onto byte buffers, then runs the real reader and
  binding (`power_loss_every_prefix_and_subset`: old pair, new pair or a
  named reason, never a trusted mix). *Test (U-7 alone, then the join):*
  a kill at every point, then the next link: the reason of the table, an
  output equivalent to a full link's (IN-79); `P` missing after W2..W4
  and `make_relinks_after_kill_at_w3`;
  `output_replaced_between_check_and_write` (in place and copy, same
  inode and new inode: `output-changed`, nothing written);
  `two_updates_one_output_serialize`; `state_renamed_while_waiting_retries`;
  `hard_linked_state_is_compacted`; `failed_sync_is_not_retried`;
  `enospc_at_each_step_then_full_link`; `torn_state_with_flag_is_output_torn`;
  `leftovers_removed_only_when_unlocked`; `boot_id_changed_checks_leaves`;
  `kill_between_write_and_sync_<point>` (each point of the table that
  follows a write); `output_missing_after_kill_in_window`;
  `in_place_note_lists_readers` (IN-45); `output_not_racy_after_update`
  and `touched_input_stat_rewritten` (W4's binding, review-in77 M-8,
  m-5); `incr_state_refuses_flag_set` (IN-77f);
  `foreign_output_during_aside` (WP-211, review-wp207 M-4: a plain link
  writes `P` between W2 and W4: `P` is the plain link's, `A` removed,
  `output-changed`, the next link a full link); `leftover_aside_same_inode`
  (a kill between W4's link and unlink, then an update: `A` removed
  before W2); `exec_fails_from_u1` (ETXTBSY from U1, review-wp207 m-1);
  `flock_across_containers` (two containers sharing the mount, H-2,
  where the harness can start them; else *argued, not run*);
  and H-2's `full_link_power_loss_replay` (IN-77c).

- **IN-78 [v1] The state's writer's duty in an update: an invariant**
  (rewritten by WP-207; review-in77 B-3, B-4, M-6, M-7, M-8; amended by
  WP-211: review-wp207 B-2, M-1). The duty is
  not a list of tables; it is this equation, whoever's file a row
  belongs to:

  > **After W5 (or the compaction), `canon(S')` equals `canon(C(U))`.**

  - `S'` is the state the update wrote. `C(U)` is the state the **full
    link of the update's inputs writes when its layout is the update's
    `Placement` `P_U`**, where **`P_U` is the `Placement` the update
    computed: the one (b) verified against the image, read from the
    update's map** (IN-75, IN-79 (b)), **never from `S'`** (WP-211,
    review-wp207 B-2: read from `S'`, a stale decision row would be an
    input of `C(U)` and the equation would hold whatever it says).
    IN-61's replay is run with `P_U`'s **decision rows** (each section's
    place, `CHUNKS`' addresses, offsets and capacities, `ORDER`, `HOLES`,
    `SLOTS`, `PATCHES`, `REDIRECTS`, `FRAGS`' offsets, the `.eh_frame`
    ranges (`FILES`' eh columns, WP-239), `REGIONS` (`P_U`'s
    `region` record, plumb.md §3.2; WP-213), the `.symtab`
    blocks' places (each file's `(first, count, capacity)` and `.strtab`
    region)), with every other row computed afresh from the inputs by the
    full link's own front, resolution, liveness and capture: `FILES`,
    `FILE_INDEX.first_global`, `META`'s `iterations` and `versioned`
    (WP-215), `STAT`, `MENTIONS`, `KEY_REFS`, `RESOLVED`, `GROUPS`,
    `COMMONS`,
    `INDEX`, `EDGES`, `FRAG_USE`, `FILES`' eh columns, `FRAGS.refs`,
    `SLOTS.refs`, `KEY_REFS.demand` (WP-239), `FILES`' `eh_fdes`,
    `stack` and `properties`, `FILE_INDEX.locals`, `FRAGS.size` (seam 6),
    `SYMBOLS` (with `at_file`, `at_index`, `option`), `NAMES` (WP-213), `SYMTAB`'s entries, **`SYMTAB_KEYS`** (recomputed: it
    follows from the `.symtab` blocks' places and the resolution),
    `COPIES` (WP-220), `DIAGS`, and the derived layout rows (`DEFINED`,
    `SYM_PLACE`, `SIZES`, `SIZED`, `SEGMENTS`, `LAYOUT`, `OUTSECS`).
    **And every decision row of `S'` is compared with `P_U`'s** (the
    same list): a row that differs is a failure of (d), whatever the
    replay gives. So a section a growth step moved whose `SECTIONS` row
    keeps the old address fails on the hop that wrote it. `FILES.round`
    is not compared with a value: `canon` replaces it by whether the
    **support property** of IN-71b holds on `S'` (every extracted
    member has an in-edge from a live file or root of smaller round),
    checked, not compared (review-wp207 M-1: an update keeps rounds that
    are supported but not breadth-first, R2-1). The alternative, an
    update that maintains breadth-first rounds, would re-number every
    member reachable from a changed file: O(the archive closure), not
    O(change); not chosen. The replay runs `Layout::check` on `P_U` (no
    live section in a hole, none overlapping), and a live section that
    `P_U` does not place, or a dead one it places, is a replay failure.
  - `canon(X)` is `X`'s rows with: the slack rows of file-keyed ranges,
    the unused capacity of every table, tombstones (`GONE` symbols,
    deleted key-keyed rows) and the ranges and rows of gone files
    removed; the **dead keys** (IN-71b "Live keys": no stored mention by a
    live file, no `LAZY` definition, no option) removed with their
    `SYMBOLS`, `NAMES` and `RESOLVED` rows (WP-215 R.6 3: so a capture
    and a compaction agree without a list of tolerated keys); each
    key-keyed table sorted by its key (append order
    forgotten); every row reference replaced by the referenced row's
    canonical name: a file by its load-order position (`rank` order), a
    symbol by its key name (a local by (file, symbol-table index)), a
    section by (file, section index), a fragment by its identity, a
    chunk by (output section, ordinal, part), a string offset by the
    string's bytes.
  - **Excepted**, and checked otherwise: `LEAVES` and `META`'s binding
    (tuple, reference stamp, build id), which equal the leaves and the
    binding of the output the update wrote (the test re-hashes `P`); and
    `META`'s history fields (boot id, `output-synced`, the compaction
    count), which no full link computes.

  **The list, derived** (a consequence the test checks, not a second
  rule): the rows where `canon(S)` and `canon(C(U))` differ are written
  by W5; the causes and their rows are:

  | cause | rows that change |
  |---|---|
  | a changed, appeared, drawn-in or newly extracted file | its `FILES` row (hash, rank, flags, round (any supporting value, IN-71b); `POSITIONAL` recaptured or cleared, IN-28a), its `STAT` row (tuple re-taken), its ranges of `SECTIONS`, `MENTIONS`, `INDEX`, `EDGES`, `FRAG_USE`, `SYMTAB`; its `FILES` eh columns (WP-239; `eh_fdes`, `stack`, `properties`: seam 6); its `FILE_INDEX.locals` and the `SYMBOLS` rows of its old run marked `GONE` when its symbol table is replaced (seam 6); its `FILE_INDEX.first_global` (WP-215; from `ChangedFile::first_global`, the new version's `sh_info`, WP-215 seam 3; always written, never kept from the stored row: `update::view::FileRanges::first_global`, WP-239 seam 5, U-2's F-10); its `KEY_REFS`, `GROUPS`, `COMMONS`, `COPIES` (WP-220) and `DIAGS` rows |
  | a gone or no longer extracted file | its row marked gone (extracted → `LAZY` with its defined rows kept), its ranges gone, its key-keyed rows deleted, its locals' `SYMBOLS` run marked `GONE` (seam 6) |
  | an input `stat`ed again but hashed equal (touched) | its `STAT` row (review-in77 M-8) |
  | the device stamps (IN-22a) | the device rows of `STAT` |
  | an affected key | its `SYMBOLS` row (appended when new; `at_file`/`at_index`, IN-71), `RESOLVED`, `KEY_REFS`; a new key's `NAMES` row (WP-213); an ODR-affected key's `DIAGS` rows (IN-71d; its `COPIES` rows change only with their files, WP-220) |
  | a re-elected group | no row of its own (the keeper is the copy whose file has the least `FILES.rank`, not stored; WP-215 D2); the drawn-in files of IN-72 item 2a (review-in77 M-7) |
  | a section moved, killed or revived (IN-72 items 1-5, 11), in any file | its `SECTIONS` row, whether or not its file changed (review-in77 B-3); its file's `REGIONS` range when a region was opened, grown or moved (WP-213) |
  | a linker-defined key's value or a placed symbol's place | `DEFINED`, `SYM_PLACE` (IN-71c) |
  | a growth step, a packed rewrite, a TLS template's size, `-e` in a changed file | `CHUNKS`, `SIZES`, `SIZED`, `SEGMENTS`, `ORDER`, `LAYOUT`, `OUTSECS`; the `REGIONS` ranges of the files whose regions a growth step moved (WP-213) |
  | holes, slots, thunks, fragments | `HOLES`, `SLOTS` (with `refs`), `PATCHES`, `REDIRECTS`, `FRAGS` (`refs`) |
  | an `.eh_frame` range moved or the section rewritten whole (IN-34a) | the `FILES` eh columns of every range it moved (WP-239) |
  | a `.symtab` entry moved or a `.strtab` name moved (IN-49a's last global taking a removed one's place, names appended) | that key's `SYMTAB_KEYS` row (review-in77 M-6), the file's `SYMTAB` rows |
  | the output | `LEAVES`, `META`'s binding (IN-21's tuple with its reference stamp, the build id), `output-synced` 1 |

  Recapture is O(the file's rows); clearing `POSITIONAL` costs the next
  update one more file translated by identity. A field the update does
  not recompute is never written from a guess: a row it cannot recompute
  is the reason that row's rule names, before W1.

  **The test and why it is cheap.** `state_equals_capture_of_its_placement`
  runs after every update of every H-1 scenario, L10 step and crash
  series step (on the update's success): the replay above writes `C(U)`
  to a scratch path, and `incr_state canon` compares `canon(S')` with
  `canon(C(U))` table by table, printing the first differing row. IN-79
  (a) already runs the full link of the same inputs, so the replay is
  IN-61's replay entry, given `P_U`'s decision rows (the map's decision
  records read into a `LayoutRows`), run by a test binary the lane calls,
  with no output written: the added cost is the capture (part of every incremental-mode
  full link) and a linear comparison, O(state), about the cost of reading
  the state whole (WP-105: 0.56 s on g10x, 0.20 s on `cxx-large-g`), not
  a second link. `duty_list_covers_differences` checks the table above
  against the comparison: every differing row's table and cause is a row
  of the table. `growth_step_moves_unchanged_then_edit_referrer`
  (WP-211, review-wp207 B-2: a growth step moves an unchanged file's
  section, the next hop edits one of its referrers: (d) fails on the
  first hop if the `SECTIONS` row is stale, (c) on the second);
  `member_round_shortcut` (R2-1: (d) passes with a supported,
  non-breadth-first round). **Until U-2 lands** `incr_state canon`, H-1
  builds (d) against a stub that reports "(d) not run" and fails no
  test; (d) goes live in H-1's lane when U-2 merges (review-wp207 M-7).
  *Argued, not run* (no update code yet).

- **IN-79 [v1] How an update is judged** (the test packages' rule).
  Every update of a test, of L10 and of the crash series is judged three
  ways: (a) the program's run (exit status and output) equals the full
  link's of the same inputs; (b) `cargo xtask plumb --equiv X.map F.map
  X F` with the update's own map (IN-75) gives no finding; (c) the
  two-full-link oracle (IN-63a) covers every unit that differs. A
  second and a third update follow every first one (the review's
  finding was on the second hop). The targets are AArch64, x86-64 and
  i386 (qemu for the last two). *(Amended by WP-207: four ways.)* (d)
  the state the update wrote equals the capture of its `Placement`
  (IN-78, `state_equals_capture_of_its_placement`), and the update's
  standard error equals the full link's modulo the `incremental:` line
  (IN-75a). **What (d) needs from the map** (WP-211, review-wp207 B-2):
  H-1 reads `P_U` from the update's map, so the map of an
  incremental-mode link must carry every decision row of IN-78's list.
  **Today it does not**: image.md I-78's records give each section's
  address and size (`place`, `piece`), the synthetic chunks, thunks and
  patches (`chunk`), commons and file 0's values (`common`, `define`),
  and, in an update, `hole`/`dead` (IN-75); they do not give the
  regions and capacities (`CHUNKS`' capacities, the tail reserves),
  `ORDER`, `SLOTS`, `REDIRECTS`, `FRAGS`' identities, nor the `.symtab`
  blocks' places. WP-211 §R.12 requests the records (image.md I-78a,
  plumb.md §3.2); U-6 writes them (it owns `map.rs`, WP-211 §C); until
  they exist (d) compares the decision rows the map has and reports the
  others as "not compared". (b) runs with `check elflint` (harness.md, with the four
  allow patterns every static glibc output needs, review-in72 T-1). The
  dirty-range invariant (IN-75) is checked on every update against the
  previous output. (d) is what catches the stale-row class (a datum an
  update leaves stale) on the hop where it is written, before a later
  hop turns it into a wrong program.

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
  A dead `.debug_ranges` or `.debug_loc` contribution has every pair
  `(1, 1)` or a base selection of 1 followed by its offsets.
  Nothing live names it (IN-3a check 1). E6b and R9 left `.debug_line`,
  `.debug_str_offsets`, `.debug_addr` and `.debug_rnglists` this way and
  measured equal transcripts. When part of it is reused, the remainder
  takes IN-51b's form. **Tombstones** (`mortar_arch`'s `dead.rs`, read,
  not run; the re-review's n1): mortar writes 0 as the tombstone of
  `.debug_rnglists`, `.debug_loclists` and `.debug_addr` in **every**
  output, and on ELFCLASS32 0 in every debug section, `.debug_line`'s
  `DW_LNE_set_address` included. In an executable at a fixed address 0
  is not mapped, so a dead `[0, len)` names nothing. **In a PIE or
  shared object** address 0 is the unrelocated base, so a dead `[0,
  len)` entry names a real range; R11 could not test it because mortar
  refuses `-pie` on this branch. So in a PIE or shared object no dead
  entry of an address-bearing debug section (`.debug_rnglists`,
  `.debug_loclists`, `.debug_addr`, and on ELFCLASS32 `.debug_line`) is
  left as written: its contribution takes IN-51b's neutral form, until
  WP-066 runs R11. In the same outputs: a dead GOT slot gets no
  `RELATIVE` record; a reused slot's record is replaced by the new
  occupant's (IN-34, IN-41); and no `RELATIVE` record targets a hole or
  a dead range (IN-3a check 1). *Test:* `llvm-dwarfdump --verify` of
  every L10 step with `-g`; R11 in WP-066's gate, including an i386 PIE
  with a dead line sequence; a `RELATIVE` scan of every PIE step against
  the holes.
- **IN-54 [v1] `.debug_aranges` is rewritten whole.** gdb drops the
  whole section ("duplicate debug_info_offset, ignoring .debug_aranges")
  when two sets name one unit. It drops it too ("does not exist") when a
  set names an offset that is not a unit start (R9). So a dead set can
  be neither split nor left naming a reused range. Nothing refers into
  `.debug_aranges` by offset, so an update rewrites it whole from the
  live units' sets, in full-link order, as it rewrites `.eh_frame_hdr`.
  It has no dead ranges. One set per unit with -g: 0.68 MB in the
  bench's protoc `-g` output (`readelf -S`), so ~1.3 ms per update and
  O(symbols) in §1.5's scaling model.
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
  compilers; R14: 166 of protoc's 168 objects, the other two empty, and
  kiln++ 1.2.1). [later, WP-065, ordered into S5 because of split DWARF]
  either leave the GNU index out of incremental-mode outputs or
  neutralize the sets (a header with no entries, padded), once
  transcripts show it equivalent; rewriting it whole like IN-54 is ruled
  out by size (12–16.5 MB per update on protoc, §6 "Split DWARF"). Neither hod nor the
  bench's GCC and Clang defaults emit them (§8.3). *Test:* R10's objects
  in L10: each edit of one must report `debug-name-index`.
  *Decided (operator, 2026-11-17, through WP-207): option (A) for v1.*
  An incremental-mode output leaves `.debug_gnu_pubnames` and
  `.debug_gnu_pubtypes` out (G-2), so in that mode there is no prebuilt
  gdb name index and gdb indexes the skeletons at start-up (slower
  start-up, the same answers); neutralized sets (B) only if that start-up
  is measured too slow on the reference build.
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
  of fixed gdb and lldb sessions (E6's commands: break by name and line,
  run, backtrace, `info line`/`source list`, `print`/`frame variable`,
  `ptype`/`image lookup -t`, step, `info functions`/`image lookup -n`,
  `info variables`/`target variable`, `info scope`), with addresses and
  in-section offsets masked, equal for the update and a full link.
  *Test:* L10 runs them on its `-g` projects at every step on hosts with
  gdb and lldb.

**Split DWARF** (`-gsplit-dwarf`) **is the configuration the operator's
target is expected to use** (debug information always on; split DWARF
acceptable), so the go/no-go (§1.5) is measured in it as well as in
plain `-g`. kiln and hod implement it (DWARF 5, `hod/spec/dwarf.md`).

- *What still enters the link* (R10's objects, and measured on protoc
  below): per object a skeleton unit (`DW_UT_skeleton`, 0x30 bytes:
  `DW_AT_dwo_name`, `DW_AT_comp_dir`, `low_pc` and ranges,
  `DW_AT_addr_base`, `DW_AT_stmt_list`, `DW_AT_GNU_pubnames`; the
  `dwo_id` in the header) in `.debug_info` with its `.debug_abbrev`;
  `.debug_addr` (every address of the full unit); `.debug_line` (the
  whole line table stays in the object, MD5s included, so §8.3 request 2
  still applies); `.debug_str`/`.debug_line_str` and (Clang)
  `.debug_str_offsets` for the skeleton's strings; `.debug_rnglists`;
  `.debug_aranges` (GCC only); and **`.debug_gnu_pubnames` and
  `.debug_gnu_pubtypes` from every object with code** (166 of 168 protoc
  objects for both compilers, the other two are empty; kiln++ 1.2.1's
  `-gsplit-dwarf` object has them too). `.debug_loclists`, the types and
  the DIE trees are in the `.dwo`.
- *Measurement* (R14, final round; `split/` scripts in the workspace's
  `.build/review-incr/`): protoc 3.21.12 built static as the bench builds
  it, `-O0 -g -gsplit-dwarf`, by GCC 14.2 and by Clang 19 (build 49 s and
  32 s at `-j3`), linked by mortar with the bench's response file; plain
  `-g` re-linked in the same session for comparison. Full links: a first
  run and 7 warm runs each, load 5.5–6.1 on 12 cores. Floor
  (`floor.py`, §1.5's method): output and state on the overlay, three
  rounds interleaved plain/GCC/Clang, load 6.8–8.3; the median of the
  three rounds' medians is given, and the speed-up is against the full
  link's 223 / 145 / 127 ms. Two earlier rounds on the virtiofs workspace
  were discarded as noise: their I/O parts varied 2–3× between rounds.

  | | plain `-g` | GCC split | Clang split |
  |---|---|---|---|
  | debug bytes in the 168 objects (before merging) | 102.4 MB | 25.1 MB, of which the GNU name index 19.8 | 18.7 MB, of which the GNU name index 14.0 |
  | `.dwo` files beside the objects | — | 98.7 MB | 34.4 MB |
  | output | 50.3 MB | 32.3 MB | 26.6 MB |
  | its debug sections | 38.6 MB (`.debug_info` 25.9, `.debug_str` 8.1, `.debug_line` 2.7) | 20.6 MB (**`.debug_gnu_pubnames` 8.8 + `pubtypes` 7.6**, `.debug_line` 2.7, `.debug_aranges` 0.68, `.debug_addr` 0.56; `.debug_info` 6 KB) | 15.4 MB (**`pubnames` 9.8 + `pubtypes` 2.2**, `.debug_line` 2.9, `.debug_addr` 0.37; `.debug_info` 6 KB) |
  | `.symtab` + `.strtab` | 5.4 MB | 5.4 MB | 4.6 MB |
  | full link, first / warm median (min–max) | 211 / 218 ms (194–248) | 148 / 138 ms (132–146) | 122 / 123 ms (117–131) |
  | peak RSS (median) | 257 MB | 163 MB | 145 MB |
  | phases (warm median, ms): input, split, layout, image | 71, 16, 23, 75 | 53, 3, 23, 36 | 46, 3, 20, 32 |
  | floor, median file, thin, copy / in place | 100 ms (2.2×) / 68 ms (3.3×) | 88 ms (1.65×) / 66 ms (2.2×) | 63 ms (2.0×) / 52 ms (2.4×) |
  | floor, median file, regular archive, copy | 160 ms (1.4×) | 118 ms (1.2×) | 87 ms (1.5×) |
  | floor, `descriptor.cc.o`, thin, copy / in place | 109 ms (2.05×) / 79 ms (2.8×) | 97 ms (1.5×) / 70 ms (2.1×) | 82 ms (1.6×) / 63 ms (2.0×) |
  | dirty windows / MB refilled (median file) | 15 / 10.0 | 16 / 11.6 | 12 / 7.7 |
  | slack (`regions2.py`, IN-33, debug tail `size/32`; no tail) | +10.0% (5.0 MB); +7.6% | +12.0% (3.9 MB); +10.0% | +13.9% (3.7 MB); +12.0% |
  | state (`state_emul.py`) | 8.17 MB | 8.18 MB | 7.32 MB |

  The median live member is `importer.cc.o` (plain), `csharp_field_base.cc.o` (GCC)
  and `map_field_lite.cc.o` (Clang): the median is taken in each build. The
  program runs (`protoc --version`: `libprotoc 3.21.12` for all three).
  gdb finds every `.dwo`: `break main`, `run`, `bt` and `info source` name
  `main.cc:112`, with the compile directory taken from the skeleton.
  `info line`, `ptype`, `info scope` and `list` resolve through the
  `.dwo`.
- *What it shows.*
  1. **Split DWARF cuts the full link, not the update.** The full link
     is 37–44% faster and needs 37–44% less peak RSS. The update's floor
     falls only 12–37% in absolute terms, because its large parts do not
     depend on debug bytes: the state (O(symbols), unchanged at 7–8 MB),
     `.symtab`/`.strtab` (5.4 MB of windows on every update), the copy,
     and the whole resolve. So the ratio falls: thin, by copy, for the
     median file, 2.2× becomes **1.65× (GCC) and 2.0× (Clang)**,
     *measured floor*. Adding §1.5's omitted-phases bound, which split
     DWARF lowers only through its split phase (≤ 47 ms), gives
     1.1–1.65× (GCC) and 1.15–2.0× (Clang), an *estimate*. §1.5's
     go/no-go is therefore harder to meet in the reference configuration
     than in plain `-g`, and is decided in it (record (a) 6, 9).
  2. **The GNU name index is most of what is left.** It is 16.5 MB of
     GCC's 20.6 MB of output debug bytes (51% of the output) and 11.9 of
     Clang's 15.4 MB. One file's sets are large: `descriptor.cc.o`
     carries 1.0 + 0.47 MB (GCC). IN-55's candidate, "rewritten whole,
     as `.debug_aranges`" (IN-54), would write 12–16.5 MB on every update,
     more than the whole floor refills. So WP-065 must not adopt it.
     WP-065 chooses between two rules:
     - (A) **leave the GNU name index out of incremental-mode outputs**:
       the full link and the update both drop both sections, so IN-2
       holds between them;
     - (B) per-contribution allotments (IN-50) with a neutral dead set
       (a set header naming the neutralized unit, no entries, padded),
       tested as IN-51b's forms were.

     The measured basis for (A): with the two sections removed
     (`objcopy --remove-section`), a gdb session of 12 commands (break
     by name, run, `bt`, `info source`, `info line`, `ptype`,
     `info scope`, `list`, `info functions`, `info types`) and an lldb
     session (`image lookup -n`, `type lookup`) print the same
     transcripts as with them, with addresses masked, for both
     compilers. gdb and lldb do not read these sections. Their consumer
     is a linker's `--gdb-index` (gold, lld), and IN-58 already refuses
     that in incremental mode. (A) also shrinks GCC's output to 15.6 MB.
     (A) is the recommendation, pending WP-065's transcripts at L10
     scale. A plain link keeps the sections as it does today.
  3. **Skeletons are small and the slack grows in relative terms.**
     `.debug_info` is 6 KB in all, about 0x30 bytes per skeleton, so IN-51's block form (≥ 19
     bytes with `DW_AT_addr_base`) fits every skeleton. Slack grows from
     +10.0% to +12.0% (GCC) and +13.9% (Clang) of the output, but shrinks
     in bytes (5.0 to 3.9 and 3.7 MB): the GNU name index's
     contributions take their size-class rounding like any other.
- *What v1 does today*: it **falls back on every update** of a changed
  split-DWARF object, twice over: IN-55 (`debug-name-index`, the GNU
  name index every `-gsplit-dwarf` object with code carries, R10 and R14
  above, GCC, Clang and kiln) and IN-55a (`debug-unit-kind`, the
  skeleton unit). **Confirmed by R14**: in the reference configuration
  every protoc object would trigger both, so v1 is a full link on every
  update. The skeleton and GNU-name-index cases are therefore **ordered
  into S5 (WP-065), not after it** (record (b)). S5 tests skeleton
  neutralization (IN-51's block form on a skeleton) and rule (A) or (B)
  for `.debug_gnu_pubnames`/`pubtypes`. It lifts the two fallbacks for
  split objects only when its tests pass. WP-061's floor measurement
  does not consult the detectors (a test-only path), so it measures
  split DWARF from the start.
- *The `.dwo` files*: the update does nothing to them; the compiler
  rewrote the changed object's `.dwo`, and the unchanged objects' are
  untouched. The output names each `.dwo` through its skeleton
  (`DW_AT_dwo_name`, `DW_AT_comp_dir`) and matches it by `dwo_id`; the
  update keeps the skeleton of each unchanged object as it was and
  writes the changed object's new skeleton (new `dwo_id`), so the
  output's skeletons always match the `.dwo` files on disk, as after a
  full link. A `.dwp` package a user built (`dwp`, `llvm-dwp`) from an
  earlier output is **stale after any update**, as after a full link:
  the spec promises nothing about `.dwp` files; the debugger reports the
  `dwo_id` mismatch for the changed unit. `docs/` says so.
- *Not measured*: DWARF 4 split (`-gdwarf-4 -gsplit-dwarf`); `.dwp`;
  kiln on more than one file (kiln++ 1.2.1 compiled one test file with
  `-gsplit-dwarf`: a `DW_UT_skeleton`, a `.dwo`, and the GNU name index;
  protoc was not built with kiln); lldb running the program (lldb was
  run on the file only).

## 7. Fall-backs

Each is a full link in incremental mode (its output ready for the next
update), reported as `incremental: full (<reason>)` (IN-10). With
`--incremental=update` each is an error instead.

| reason | when | v1 or later |
|---|---|---|
| `no-state` | no state at the state path (the first link); reported only by `--incremental-report` | permanent |
| `state-corrupt` | the state fails validation (IN-18) | permanent |
| `state-version` | the state's format or mortar version differs | permanent |
| `options-changed` | the normalized plan differs: any option, the target, the emulation, the library search path, the input list's order (adding or removing an input, or an archive's members changing, is not a change of plan). The delta also raises it when a path whose content changed is no file of either link: a linker script, whose content the plan does not hash (WP-063). A thin archive's member file is its member's (IN-24), not such a path: a changed, added or removed member is a changed, appeared or gone file (WP-106) | permanent; [later] options that touch no layout (`-Map`, `--time-phases`, `--threads`, `--incremental-report` are excluded already) |
| `output-changed` | `P` is missing, or its stat tuple or build id differs from the state's, or it is racy and a build-id leaf differs (IN-21) | permanent |
| `output-torn` | the state's "updating" flag is set (IN-46, IN-77); read before any hash (WP-207) | permanent |
| `state-busy` | the state's path named another inode after its lock was acquired, 8 times in a row (IN-77b, WP-207): another link keeps replacing it | permanent |
| `lock-unavailable` | the file system does not enforce `flock` on the state (image.md I-59's probe), so two updates could interleave (IN-77b, WP-207); the full link writes no state and removes the one at the state path (IN-77b, IN-77d, WP-219) | permanent |
| `small-output` | the output the state describes (`META`'s output size) is smaller than 256 MiB (`mortar_incr::SMALL_OUTPUT`), checked after every row above. Not with `--incremental=update`, which updates whatever the size. **A policy constant, not a measured crossover**: an update was measured at 55 MB (0.67-1.15 of a plain link, saving at most 124 ms) and at 583-832 MB (0.70-0.94; 0.19-0.26 with the state read and written by range), and on no workload between (WP-094 §7). 256 MiB is a power of two between them, so that the mode can be told from `ls -l`; WP-105 measures a workload in that range and moves it if the crossover is elsewhere | permanent (the bound a policy constant) |
| `update-not-implemented` | the state is valid, the plan and the output match it, the output is not small, and the update path does not exist yet (WP-061 stage A: every incremental link is a full one after the checks above, in this order; `--incremental=update` fails with it whatever the size) | stage A only. Amended (WP-202, IN-70): after the update joins (U-10), only a changed, appeared, gone, newly extracted or no longer extracted file whose old or new version has debug sections or a split-DWARF skeleton gives it (`update-not-implemented (debug)`; WP-239 P-3, decided by the join), or with `.mason.meta` units under `--keep-mason-meta` (`update-not-implemented (meta)`, IN-71d, WP-207), or a stored `link`-subject warning the update cannot raise again within O(change) (`update-not-implemented (diags)`, IN-75a, WP-212), or a symbol-table entry named `foo@V` or `foo@@V` in the stored link or in a changed file's new bytes (`update-not-implemented (versioned)`, IN-71 Versions, WP-215; [later] the version of a `@@` definition stored), or a link under `--gc-sections` (`update-not-implemented (live)`, IN-71a, WP-215 seam 3; [later] WP-238), or a winning common that appears, goes, becomes a definition (or a definition a common), or changes size or alignment (`update-not-implemented (common)`, IN-72 item 11 as ruled for v1, WP-216 R-2: decided by the join from `ResolveDelta::commons` against the stored `COMMONS` rows; [later] item 11's placing rule), or an unchanged `.eh_frame` range to move holding a pointer encoding other than `0x1b`/`0x9b` (`update-not-implemented (eh)`, IN-34a, WP-239 S4-3: decided by the plan before W1), or a key whose `mortar_arch::SymFacts` (the one value the scan reads a target by) changes while a file left unchanged refers to it (`update-not-implemented (slots)`, IN-72 item 7, WP-239 R-4: decided by the join); the G packages delete the row (the `versioned`, `live` and `common` clauses stay until their [later]). Until the join, each entry point of the cut that has not landed gives its own word (WP-213: `state5`, `resolve`, `refill`, `layout`, `symtab`, `image`, `write`, `meta`, `debug`, `diags`; WP-215: `versioned`, `live`; WP-216: `common`; WP-239: `eh`, `slots`; the floor's stage-A reason prints none); the reasons are one type, `mortar_core::incr::IncrReason` |
| `extraction-changed` | IN-71b's bound: more than 1,024 archive members flip, or a removal cone's re-seeding reads more than 65,536 `KEY_REFS` rows (WP-207) | v1; [later] the bound measured and moved |
| `closure-iterated` | the stored link's closure ran more than one iteration, or the full link of the update's inputs would (resolve.md §4.1; IN-71b Iterations; WP-215) | v1; [later] an iteration-aware state |
| `inputs-reordered` | two unchanged inputs' relative order changed. The input list's order is in the plan (`options-changed`, checked first), so the delta meets this only through archive extraction order (WP-063: two unchanged `FILES` rows in a different relative order) | v1; [later] when no COMDAT keeper or placement order depends on it |
| `too-much-changed` | IN-16's threshold | permanent (it is the compaction rule) |
| `reserve-exhausted` | a segment reserve, a packed array's, `.eh_frame`'s, a slot region's or a dynamic table's capacity is exhausted (IN-32, IN-34) | permanent |
| `island-exhausted` | a new thunk or patch does not fit its island | permanent |
| `tls-layout-changed` | any TLS input section's TP offset would change, in any file (IN-34) | v1; [later] refill every TP-relative referrer of every moved TLS section through the index |
| `copy-size-changed` | a copy relocation's object changed size in its shared library (IN-34, IN-40) | v1 |
| `link-order-moved` | a region moved whose input section is the `sh_link` target of a live `SHF_LINK_ORDER` contribution (e.g. `__patchable_function_entries` with `-fpatchable-function-entry`): IN-2 item 5 keeps full-link order in packed sections, the gABI wants the order of the linked-to sections' addresses, and after a move the two differ | v1; [later] rewrite the packed section whole in the gABI order and make IN-2 item 5 say that order for `SHF_LINK_ORDER` sections; frequency not measured |
| `symbol-kind-changed` | a global that an unchanged file references changed type to or from `STT_GNU_IFUNC` or `STT_TLS`, became absolute or the reverse, or changed `st_other` bits other than the visibility (IN-26; in a static executable binding, visibility and common ↔ definition are refills, IN-71) | v1; [later] refill the referrers when their relocation types are stored |
| `symtab-exhausted` | a `.symtab` local block's move finds the local tail reserve full, or `.symtab`'s or `.strtab`'s file slack is exhausted (IN-49a) | v1; [later] rewrite both whole at the file's end (S-112), an update still |
| `new-output-section` | an output section or segment appears or disappears | v1; [later] when it can go at the end of its segment's reserve |
| `property-changed` | an allocated note section (`.note.gnu.property`, S-114, and any other `SHT_NOTE`: `.note.ABI-tag`, `.note.stapsdt`) changes size (IN-72 item 10; widened by WP-207, review-in72 m-2), or the merged `.note.gnu.property` value or `PT_GNU_STACK`'s `PF_X` differs from the stored link's (IN-72 item 10's one pass over `FILES.properties` and `FILES.stack`; WP-214 seam 6) | v1 |
| `dynamic-changed` | `DT_NEEDED`'s order changes or `.dynamic`'s spares run out (IN-42) | v1 |
| `debug-compaction` | IN-57 | v1; [later] the debug-only rewrite |
| `debug-name-index` | IN-55 | v1; [later] neutralized sets |
| `debug-unit-kind` | a changed file has type units or split-DWARF skeletons (IN-55a) | v1; [later] after S5's tests |
| `input-changed` | a file the update read is not the file change detection judged: the descriptor's `fstat` after the read (input.md §11.5) differs from the judged tuple, or the update read an inode it did not judge (IN-28a condition 2; the review's m3); WP-207: every input an update reads for a refill, a placement, a drawn-in file or a newly extracted member is checked so (IN-75, IN-71b) | permanent (WP-064a, on the test path; an update that reads no unchanged input meets it only for the files it reads) |
| `state-collision` | the capture of an incremental-mode link finds two different merged fragments with one `FRAGS` key (out, class, hash) (IN-18e, WP-239 R-2): the link is written, its stderr carries this `incremental:` line, no state is written, and the next link is a full one | permanent (a 128-bit hash collision; tested by an injected collision) |
| `state-limit` | the capture of an incremental-mode link finds a datum past its stored column: an `.eh_frame` chunk past `u32` (`FILES`' eh columns, IN-18e, WP-239 S4-3): the link is written, no state is written, the next link is a full one | permanent for that output |
| `layout-changed` | test path only (WP-064a): the changed file's section count, a section's type, flags, size or alignment, its group, the positions of its live merged fragments or the `FRAGS` rows they are (as a multiset) differ from the state's rows; or a row of the state does not translate ("a replay that does not fit") | test path only; never an update's reason: the delta layout (IN-72, IN-73) and the rows above replace it |
| `resolution-changed` | test path only (WP-064a): the global keys the changed file's symbol table names, or one's binding, type, visibility, definedness or section, or the resolution of one of them (kind, definer, section, value, size), differ from the state's `MENTIONS` and `RESOLVED` rows (the review's M4: a weak definition made global); or a `SLOTS` or thunk `PATCHES` row names a local of the changed file whose name the file gives to more than one local (a section symbol: §7.1's local identity is then a position; WP-064a's s3) | test path only; never an update's reason: delta resolution (IN-71), IN-28b and IN-27's refill set replace it |

Not fall-backs but plain links (IN-12; a warning, no state written):
`-r`, `--emit-relocs`, a `SECTIONS` script, `--compress-debug-sections`
other than `none`, `--gdb-index`, `--debug-names`, `--icf`. In place is
the default at or above `small-output`'s bound (IN-76); these take the
copy path instead of a fall-back: `ETXTBSY`, a process that maps or holds
the output, a hard-linked or symbolic-link output, a shared object, an
output that is an input, a file system without enforced locks (amended by WP-207: that one is `lock-unavailable`, a full link, IN-77b). Racy
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
and flags a difference above its noise band. (Not built: no feature exists.
The gates that show it today, WP-202: `cargo xtask static-identical`,
every plain static link's bytes against a stored baseline on the three
targets (2,993 links at WP-105); `a_plain_link_touches_nothing_of_incremental_mode`,
no state or report path opened, `stat`ed or written; and `cargo xtask
bench --check`'s plain rows. Every update package keeps its code behind
`incremental::start`'s first line, which returns before anything else
when `--incremental` is absent.) An incremental-mode full
link is allowed to cost more than a plain one, by at most 10% of wall
time and of peak RSS on protoc (*target*, S1 measures it).

### 8.2 The verifier and L10

- **IN-60 [v1] The verifier needs no state.** `mortar_plumb` checks an
  update's output from the output and the map alone, as for any link
  (`docs/VALIDATION.md` "independent verifier"). The map gains the
  `incremental` record, exactly once in every map of an
  incremental-mode layout and never in a plain one, and two line kinds
  (plumb.md §3.2, after the defines and before the dynamic records):
  `hole <output section> <offset> <size>` and `dead <output section>
  <offset> <size> neutral|contribution|strings|cie`. plumb checks that every
  byte of an allocated output section is covered exactly once by a
  `place`, `piece`, synthetic chunk, `hole` or `dead` line, and runs
  IN-3a's three checks on every `hole` and `dead` range. *Test:* plumb's
  mutation tests (plumb.md §9.2) gain: a live relocation aimed into a
  hole; a live unit's `DW_AT_stmt_list` aimed at a dead `.debug_line`
  range; a stale code byte in a code hole; a dead `.debug_info` unit not
  neutralized; a 12-byte CIE and a zero word in an `.eh_frame` hole
  (R5's negative rows); a hole in `.init` and a zero slot in a
  `__start_` array (R4). Each must be rejected. Two positive cases must
  be **accepted** (N3): an `arr+size` reference whose address is the
  first byte of the region's slack, and a gfortran `blk_-8` reference
  whose address is in the hole before the common block.
- **IN-61 [v1] The replay hook.** A test-only entry (a library
  function of the `mortar` crate, `replay_link`, that tests call; not an
  option and not an environment variable, which a shipped binary would
  read; model §7.3) runs a full link whose decision tables come from a
  state file, translated by identity to the link's ids (a row that does
  not translate is a named replay failure) and put in place of the
  computed ones before `Layout::validate`; the harness
  compares its output with the update's byte for byte outside the dead
  ranges, which it compares by IN-3a's checks (IN-1 B). Every plan the
  image writes from is made from the layout after the replay has put its
  tables in place (image.md §1, WP-086): `.eh_frame` and `.eh_frame_hdr`
  from `EH_RECORDS` (S-49), `.symtab`'s patch rows from `PATCHES` and its
  spans held to `SYMTAB_LOCALS` (S-88), `.got` from `SLOTS`, `.shstrtab`
  from `CHUNKS`, the merged chunks' order from `FRAGS`, and the erratum
  fixes (the branch at each erratum site and its patch's words) from the
  final placement and slots, so no stored row is ignored and none is
  refused for differing from the computed one (the WP-061 review's F8 is
  closed). A replay builds `.symtab` and `.strtab` with IN-49a's room
  (the blocks' capacities, the tail reserve, the regions' slack, the
  file slack), as the incremental-mode full link that wrote the state
  did, whether or not the replaying link was asked for incremental mode
  (`PatchFacts.room` is set by `env.slack || env.replay`); its spans are
  held to the stored `(first, capacity)` (WP-217). A table the rules do
  not admit fails by name (in `Layout::check`, or in the plan that
  holds it to a rule), or, for the few the checks do not cover, gives an
  output the verifier rejects.
  *Test:* every L10 step runs it;
  `replay_reproduces_and_each_table_matters` (with an erratum link and a
  link with range-extension thunks) changes each decision table: a valid
  change (two GOT slots, two merged strings, or two thunks of one island
  with their sites, trade places) gives other bytes the verifier accepts
  and a program that runs the same, an invalid one fails by name or is
  rejected by the verifier, and no change is ignored;
  `a_kept_equal_cie_is_followed` replays a list that keeps a CIE S-44
  would remove; `a_state_written_before_wp086_replays_identically` reads
  states written before.
- **IN-62 [v1] The equivalence check.** `cargo xtask plumb --equiv
  X.map F.map X F` (the verifier has no binary of its own;
  `mortar_plumb::equiv::check` in a test; `plumb.md` §6.9, §8) checks IN-2's items 1–6 between an update `X` and a full
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
  **unchanged** file, compare its bytes: in place when its placement is
  the same in both, and **by value when it moved** (the re-review's N4):
  the move is undone, every non-relocated byte must be equal, and every
  relocated field must hold the same value mapped through the two
  placements (a field naming input section `s` at offset `o` names `s`
  at `o` in both; a slot field is compared by the slot's symbol), as
  IN-62's equivalence check does. Each unit whose bytes differ beyond
  that must be in the refill set that `--incremental-report` (or
  WP-063's dry run) predicted. "Moved" is never by itself an excuse:
  a moved unit whose relocated values changed and that no row predicts
  fails the oracle. Bytes that
  differ outside every unit are named by output section and must belong
  to a synthetic chunk the update rewrote. It needs no state and no
  incremental code: it is the reviewer's experiment 3 made a test, and
  it is what keeps IN-27's table honest as rows are added. *Test:* R3's
  scenarios (body edit, COMDAT keeper both ways, gc kill and revive, TLS
  growth, a debug-only referrer) on GCC `-O0 -g` and `-O2 -g`; N1's
  FUNC → IFUNC pair (`ifunc-type`), a weak undefined that becomes
  defined, and `lsda-edges`.
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

  **Re-run with the by-value oracle** (IN-63a; the re-review's N4;
  `oracle.py` compares moved units by value; 28 directed pairs × `-O0`
  and `-O2` = 56 oracle runs, AArch64 and x86-64, the gc edge-kind pair
  g included; `oracle_run.sh`, results in the workspace's
  `.build/review-incr/results/final/`). **With every row in place, 56 of
  56 pass.** Each row dropped in turn:

  | row dropped | pairs whose oracle run fails (by value) |
  |---|---|
  | a file changed | none (as above) |
  | a global's definition changed (by identity: definer, section, offset, size, live) | `-O0` b0, c, cf; `-O0`/`-O2` e |
  | **symbol attributes (`symattr` → `symbol-kind-changed`)** | **none alone** (see below) |
  | a COMDAT keeper changed | `-O0` b0, b; `-O2` b |
  | liveness changed | `-O0`/`-O2` c, cf, l (`lsda-edges`), x86-64 lx, g (`gc-edge-kinds`) |
  | a section of an unchanged file moved | `-O0`/`-O2` a/grow, b0, b, w (weak undefined → defined), l, g (`-O2`), x86-64 w, lx |
  | a synthetic table changed | `-O0` a/grow, b0, w; `-O0`/`-O2` cf, g, x86-64 lx; `-O2` x86-64 w |
  | a merged fragment moved (non-allocated; oracle only) | `-O0`/`-O2` b |
  | TLS | x86-64 `-O0` dx |
  | `symattr` + `global` | as `global` alone |
  | `symattr` + `synth` | as `synth` alone |
  | `symattr` + `global` + `synth` | the union of `global` and `synth`, plus `f` and x86-64 `fx` at both levels, `w` at `-O2` and x86-64 `wx` at `-O0` |

  **The 6 failures of the previous run were oracle errors, not missing
  rows** (diagnosed from the bytes, final round):
  - `-O2` l, both directions (35 libc/libstdc++ units "differing beyond
    the move"): the oracle mapped a field naming an **allocated merged**
    fragment to the wrong fragment. Its lookup took the first piece
    whose *end* equals the offset before the piece that *contains* it.
    Example: `C-address.o`'s `R_AARCH64_ABS64` at `.data.rel.ro.local+0`
    names `C_name.o`'s `.rodata.str1.8`+0x18, the string `"C"` (map:
    `piece 107 4 18 415a98 2`). The de-duplicated empty string at 0x17
    (`piece 107 4 17 413d98 1`) ends at 0x18, so the oracle used
    0x413d99, and the residuals came out 7423 and 7431. The field is
    right in both links. The same error, with the empty string at
    0x1f before the string at 0x20, made g at `-O0` fail
    (`translated_number_width.o`'s `adrp x0` holds pages 0x414000 and
    0x415000 for fragments at 0x414ff0 and 0x415000, both correct; the
    oracle computed page residuals 1 and 2 against 0x4139a1 and 0x4139b1).
    The fix is in the oracle (a containing piece first, an end label
    only if none contains the offset), and with it every such unit is
    equal by value. No row is missing: an update never moves an
    allocated merged fragment (L-147, S-110), and an unchanged unit
    that names one is refilled only when one of its other targets moves.
  - x86-64 lx at `-O0` and `-O2`, both directions: `crtbeginT.o`'s
    `R_X86_64_32 .eh_frame+0`, offset 0 of an empty input `.eh_frame`
    that has no map line, holds the start of the output `.eh_frame` in
    each link (S-50 rule 1; 0x41d4a8 and 0x41d4c0). The oracle did not
    implement S-50, found no target and compared the raw words. It now
    does (rule 1), and the field is equal by value. Here the design
    *did* lack a statement: the moved-section row cannot see an S-50
    value move, because the target is no unit. IN-34a now stores S-50's
    value per reference, and IN-27 has the `.eh_frame`-reference row.
  In the previous run these 6 failed with every row in place, so every
  mutation was reported as caught on them, and the table claimed that
  `symattr` caught fx. With the oracle corrected, **`symattr` alone is
  caught by no pair**: in fx and f, the changed definer is `c.o`, and the
  oracle's `global` row compares a changed definer's *output* value,
  which moves to the IPLT entry when `f` becomes an IFUNC. Its `synth`
  row predicts the referrer too, through the new `.rela.iplt`/IPLT
  entry. IN-26's delta and IN-27's global row compare input values (N1),
  so in the design `symattr` is the only row that refills `u.o`. The
  re-review's reproducer shows this (`f(5)=4423704`). The oracle's
  predictor is wider than the design's rows there. So WP-069 must add a
  scenario that only `symattr` predicts, or align the oracle's `global`
  row with IN-26 (input value for a definer in a file that changed), and
  keep the N1 reproducer as a run test. Until then, the `symattr` row's
  mutation test is the reproducer itself, not the oracle.

  The TLS row still needs an AArch64 scenario where a TP offset changes
  at unchanged addresses (a `PT_TLS` alignment change; R3 could not
  produce one through layout padding, not verified); WP-069 adds it. The
  x86-64 programs now run under `qemu-x86_64` (the re-review's n5):
  both links of each pair print the same and exit 0 (dx `3`; fx
  `f(5)=10`; wx `-100` then `14`; lx `1 -1` then `1 -1 -2`), as the
  AArch64 runs do.

  A fall-back row may be not caught, conservative, with its reason: a
  full link is never wrong, and two full links cannot show that a
  fall-back was needed (only an update without it can, WP-064). WP-063's
  record lists `inputs-reordered`, `options-changed`, `property-changed`
  and `symbol-kind-changed` this way. A refill row (not a fall-back) not
  caught still needs a scenario: the `.eh_frame`-reference row is one
  (WP-069).

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
   **Its rank depends on the archives' size** (the operator's reference
   configuration, §1.5): with 1,000–3,000 archives of 0.6–7 MB a regular
   archive costs 0.4–5 ms of detection and a thin one costs 8–77 ms more
   `stat`s per update, so there it is worth nothing and should not be
   sent as a default; it pays for a few large archives (protoc's
   85–117 MB). Request 2 is then the first.
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
   request is worth most before WP-068. Cost to kiln's users: with the
   switch on, debuggers lose source checksums (gdb and lldb cannot tell
   a source file that changed since the build from the one compiled).
   (A per-file choice is not possible: a DWARF 5 line-table header has
   one file-entry format, so either every file has `DW_LNCT_MD5` or none
   does, which is why Clang drops them all when one file lacks one.)
3. **hod: tell mortar before emitting name indexes or type units**
   (`.debug_names`, `.debug_pubnames`, `DW_UT_type`): today it emits
   none, and v1 falls back on them (IN-51, IN-55).
4. **kiln's driver: `-gsplit-dwarf` in the development profile**, once
   S5's skeleton tests pass: it moves most debug bytes out of the link
   (the operator's reference configuration, §6 "Split DWARF").
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
| 7 | `plumb.md` §3.2, §6.8, §9 | the map's `hole` and `dead` lines; plumb's dead-range checker (IN-3a's three checks) and its mutation cases (IN-60); `--equiv` (IN-62) | — |
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

WP-061-WP-063 landed (S1-S3), with WP-091/WP-092 (the verifier's I and
E stages), WP-094/WP-105 (the measured floor, the state by range) and
WP-064a (position-exact rows). The old WP-064 (S4), WP-065 (S5) and the
rest of the old table are replaced by **WP-207's cut**
(`spec/landed/incr/WP-207.md` §C, which replaces WP-202 §C after the
reviews R-C and R-L: rule ids, dependencies, files owned, what each
package is tested against alone, sizes), **as amended by WP-211**
(`spec/landed/incr/WP-211.md` §C after the re-review R-2: U-1's
entry-point signatures and shared types, `map.rs` owned by U-6 alone,
H-1's (d) on a stub until U-2, the start conditions), **as built by
WP-213** (U-1: the seam compiled, `spec/landed/incr/WP-213.md` §S, the
table every package's brief quotes; where it and WP-211 §C differ, WP-213
is what the code says). In short:

| package | content | after |
|---|---|---|
| R-2 | re-review of WP-207's amendments (done: `briefs/reviews/review-wp207.md`; closed by WP-211) | WP-207 |
| H-3 | L10 (IN-63, WP-067) with WP-207 §D's edit shapes | at once |
| G-1, G-2 | debug contributions in place (IN-50-IN-54, IN-56, IN-57); split DWARF without the GNU name index (IN-55 option (A), decided, IN-55a) | at once (G-1's `.mason.meta` row after R-2) |
| U-0 | spec step 0: WP-211 §R, WP-207 §R where WP-211 does not replace it, and WP-202 §R.1, R.3, R.5 | WP-211 (at once) |
| U-1 | shared types and every later entry point with WP-211 §C's signatures, as an empty function; `WRITE_PATH` | WP-211 (at once; U-0 in parallel) |
| U-2 | state format 5 (IN-18c-IN-18f, IN-46b, IN-77c's fields, IN-78's `canon`); size gate 1.15 × format 4 | U-1 |
| U-3 | resolution, extraction, linker-defined keys, liveness, refill set (IN-71, IN-71a-c, IN-74, IN-72 item 2a's draw-in) | U-1 |
| U-4 | delta layout (IN-72, IN-30, IN-32, IN-28b's slots) | U-1 |
| U-5 | the per-file symbol tables (IN-49a) | U-1 |
| U-6 | refill, dirty ranges, header tables, input checks (IN-75); the update's map, every line of it (`map.rs`: U-5's `.symtab` holes, U-4's `common` file, IN-79's decision records) | U-1 |
| U-7 | writes, lock and crash protocol (IN-45, IN-76, IN-77, IN-77a-f) | U-1 |
| U-8 | ODR in an update (IN-71d; default `--odr=warn`) | U-1 |
| U-9 | diagnostics equal to a full link's (IN-75a) | U-1, U-0 |
| V-1 | the verifier: fillers (section 1's address), `.symtab`/`.strtab` holes, dead names | U-0 |
| H-1, H-2 | the judge (IN-79 (a)-(c); (d) on a stub until U-2 lands `canon`), crash injection | U-0 |
| U-10 | the join (static, no `-g`); **the go/no-go gate**: an update at most 1/2 of a full link on protoc, at most 1/3 on the large workload, as the **median and p90 over at least 20 updates** of a day of edits, a compaction's time reported separately (and amortized at its measured rate) and never inside an update's time (WP-211, review-wp207 §7); the first real update numbers | U-2-U-9, V-1, H-1, H-2 |
| B-1, W-1 | bench rows (moved definitions, refill set); the clone probe (IN-76a) | U-10 |
| G-3 | the `-g` join | U-10, G-1, G-2 |

**Decided by the operator (2026-11-17).** The first incremental-mode
link costs 1.6-1.8x a plain link; that is accepted on the premise that
the update ends as a measured win. U-10's gate is therefore hard: if it
is not met, the update does not ship as designed and the cost goes back
to the operator. Nothing turns `--incremental` on by default (kiln does
not pass it) until U-10's measurement is a win.

Not cut yet (each has its condition in its rule): dynamic outputs
(IN-40-IN-43, the old WP-066), section-level refill (IN-31, the old
WP-068), the hole free list, debug-only compaction, name-index
neutralization, scrubbing (IN-3b), the `symtab-exhausted` rewrite, the
first-link prototypes of WP-202 §B.7.

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
11. **`property-changed` on the size only** (WP-063). The trigger is the
    `.note.gnu.property` chunk's size; the note itself is refilled
    either way. Open for WP-064: whether a change of the property bits
    (BTI, IBT, SHSTK) at the same size changes anything else in the
    image (segment flags, the PLT's form). Default: the size; if
    something depends on the bits, the trigger becomes "the bits
    differ".

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
| R14 | split DWARF on protoc (GCC and Clang, `-O0 -g -gsplit-dwarf`) against plain `-g`: object and output debug bytes, full links (1 + 7 runs), floor on the overlay (3 interleaved rounds), slack, state, gdb/lldb transcripts with and without the GNU name index | `split/build.sh`, `split/link.sh`, `split/dbgsizes.py`, `split/summ.py`, `split/floor-overlay.sh` (`floor.py`), `regions2.py`, `state_emul.py` | §6 "Split DWARF", IN-55, `results/final/split/` | full links 5.5–6.1; floor 6.8–8.3 |
| R15 | the final by-value oracle (28 pairs × 2 levels) with scenario g, its mutation table, and the gc edge-kind drops | `oracle_scen.sh WORK`, `oracle_run.sh WORK`, `livemodel.py` | IN-27, IN-29, IN-34a, IN-63b, `results/final/` | not timed |

Incidents during the measurements, recorded for the reviewer: an early
run of E2 overwrote the bench cache's GNU-ld-built
`cxx-medium-g-static/build/protoc-3.21.12.0` (the response file's `-o`
had not been redirected); it was rebuilt by the cache's own GNU ld
command line and runs (`libprotoc 3.21.12`, 50,062,880 bytes), with a
new timestamp. An early build of mortar for the first draft wrote some
release artifacts into the main checkout's cargo target directory before
it was stopped and redirected. None in the revision.
