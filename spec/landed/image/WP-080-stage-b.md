# WP-080 stage B — name indexes for split-DWARF outputs (a measured proposal)

This is a proposal, not shipped code. mortar refuses `--gdb-index` and
`--debug-names` by name today (`crates/mortar_cli/src/rows.rs` 129–132;
`spec/image.md` I-54 leaves both to M4). Stage A's I-49a says a
split-DWARF link concatenates each object's name tables and never opens
a `.dwo` file. This document measures what gdb 16.3 and lldb 19.1.7 do
with each kind of index, what an index needs from the objects, and what
building one costs the link, full and incremental. It ends with a
recommendation and the package cut.

## 0. Method and inputs

The measurement scripts were run from a scratch directory, which has
been removed (`measure.py`, `linktest.py`, `complete.py`, `inputs.py`,
`gen.py`). Everything was measured on AArch64, 12 cores, shared with
other agents. The one-minute load was 3.5 to 7.8 for every measured
run: the harness waited while it was above 8.

Each number is the median of three runs after one warm-up. RSS is
`/usr/bin/time`'s maximum resident set size of the debugger or the
linker. The linker was driven by `g++ -fno-use-linker-plugin -static`
(mold with `--no-fork`, so that its work is counted).

**The debugger commands:**

| what | gdb | lldb |
|---|---|---|
| *start* | `gdb -nx -batch -ex 'set debuginfod enabled off' <prog>` | `lldb --no-lldbinit -b <prog>` |
| *bp* (start, then a breakpoint by name) | adds `-ex 'break <f>'` | adds `-o 'b <f>'` |
| *lookup* (start, then a type or variable) | `ptype <T>` / `print <v>` | `type lookup <T>` / `target variable <v>` |

Every breakpoint and lookup was checked to resolve before its time was recorded.

gdb 16 builds its whole "cooked index" when the program loads. It does so
on worker threads, and `-batch` waits for it to finish. So without an
index, *start* already pays the full scan, and *bp* adds little.

**The workloads:**

| workload | what | objects | `.dwo` | output (mortar) |
|---|---|---|---|---|
| protoc split, GCC 14.2 | protobuf 3.21.12 `protoc`, built as the bench builds it but with `-O0 -g -gsplit-dwarf`: 170 units, real C++ | 168 objects + 2 archives | 98.7 MB | 32.3 MB |
| protoc plain `-g` | the bench's `cxx-medium-g-static` objects | — | — | 50.3 MB |
| syn-gcc | the bench's `objects` shape (`xtask/src/bench/synth.rs`), compiled as C++, `-O1 -g -gsplit-dwarf`: 4000 units × 200 functions (800,000 functions) | 599 MB | 359 MB | 264 MB |
| syn-clang | the same, Clang 22 | 467 MB | 321 MB | 228 MB |
| syn-clangnames | the same, Clang 22 `-gpubnames` (per-unit `.debug_names`) | 571 MB | 321 MB | 294 MB |
| syn-kiln | the same, kiln++ (b6c546bd6) with hod 1.0.0, `-O1` | 415 MB | 146 MB | 215 MB |

**Why 4000 units.** g10x's shape at its full 8150 units, split, would
be about 1.9 GB of objects and `.dwo` files (234 KiB per unit, GCC).
That is over the 1 GB that needs your approval, so it was not created.
4000 units keeps each workload under 1 GB of inputs. Every workload was
deleted after its measurements.

At the operator's scale (outputs of several GB) the no-index costs below
grow roughly with the number of DIEs and names. Scaling by 10 is an
**estimate**, not a measurement.

## 1. What the debuggers do

The rows are ordered from no index to each kind of index. "+" marks the
row that uses that index.

### protoc, GCC split (98.7 MB of `.dwo`)

| output (linker) | link | gdb start | gdb bp | lldb bp |
|---|---|---|---|---|
| no index (mortar) | 0.12 s | 0.12 s / 187 MiB | 0.16 s / 198 MiB | 0.18 s / 300 MiB |
| no index (GNU ld) | 0.49 s | 0.15 s / 187 MiB | 0.28 s / 202 MiB | 0.22 s / 302 MiB |
| + `.gdb_index`, lld `--gdb-index` (5.9 MB) | 0.09 s | 0.05 s / 67 MiB | 0.17 s / 87 MiB | 0.25 s / 302 MiB |
| + `.gdb_index`, mold `--gdb-index` (5.8 MB) | 0.04 s | 0.05 s / 71 MiB | 0.12 s / 84 MiB | 0.21 s / 306 MiB |
| + `.gdb_index`, `gdb-add-index` on mortar's output (8.0 MB; 0.33 s, 232 MiB) | — | 0.04 s / 66 MiB | 0.13 s / 82 MiB | 0.18 s / 300 MiB |
| plain `-g`, no index (mortar) | 0.26 s | 0.12 s / 133 MiB | 0.19 s / 150 MiB | 0.22 s / 255 MiB |
| plain `-g`, + `gdb-add-index` | — | 0.04 s / 66 MiB | 0.13 s / 83 MiB | 0.18 s / 255 MiB |

At protoc's size every row is below 0.3 s. An index changes gdb's memory
(about 3×) more than its time.

### syn, 4000 units (321–359 MB of `.dwo`)

| output | link | gdb start | gdb bp | lldb start | lldb bp |
|---|---|---|---|---|---|
| GCC, no index (mortar) | 0.81 s | 0.89 s / 1137 MiB | 0.85 s / 1139 MiB | 1.44 s / 1209 MiB | 1.43 s / 1210 MiB |
| GCC, no index (GNU ld) | 3.49 s | 0.86 s / 1138 MiB | 0.89 s / 1139 MiB | 1.50 s / 1208 MiB | 1.33 s / 1210 MiB |
| GCC, no index (lld) | 0.98 s | 0.87 s / 1139 MiB | 0.73 s / 1140 MiB | 1.23 s / 1209 MiB | 1.22 s / 1210 MiB |
| GCC, no index (mold) | 0.86 s | 0.65 s / 1137 MiB | 0.73 s / 1139 MiB | 1.34 s / 1211 MiB | 1.32 s / 1213 MiB |
| GCC + lld `--gdb-index` (57.9 MB) | 0.79 s | **0.24 s / 384 MiB** | 0.60 s / 434 MiB | 1.23 s / 1208 MiB | 1.23 s / 1210 MiB |
| GCC + mold `--gdb-index` (49.5 MB) | 1.05 s | 0.27 s / 384 MiB | 1.10 s / 426 MiB | 1.34 s / 1211 MiB | 1.35 s / 1214 MiB |
| GCC + `gdb-add-index` (51.5 MB; **3.34 s, 1296 MiB** after the link) | — | 0.32 s / 384 MiB | 0.81 s / 434 MiB | 1.43 s / 1208 MiB | 1.50 s / 1210 MiB |
| GCC, no index, gdb `index-cache` on, first run | — | — | 1.56 s / 1361 MiB (writes 50 MB) | — | — |
| GCC, no index, gdb `index-cache` on, later runs | — | — | **0.57 s / 434 MiB** | — | — |
| GCC, no index, gdb on 1 / 4 / all worker threads | — | — | 1.88 s / 0.80 s / 0.62 s (CPU 1.5–1.7 s each) | — | — |
| Clang, no index (mortar) | 0.72 s | 0.75 s / 1083 MiB | 0.78 s / 1083 MiB | 1.28 s / 1201 MiB | 1.18 s / 1201 MiB |
| Clang + lld `--gdb-index` | 0.84 s | 0.25 s / 388 MiB | 0.65 s / 438 MiB | 1.18 s / 1200 MiB | 1.20 s / 1202 MiB |
| Clang + mold `--gdb-index` | 0.90 s | 0.24 s / 387 MiB | 0.83 s / 429 MiB | 1.05 s / 1203 MiB | 1.12 s / 1205 MiB |
| Clang `-gpubnames`: per-unit `.debug_names` concatenated (mortar, as GNU ld and lld without the option) | 0.85 s | 0.66 s / 1152 MiB (gdb ignores it) | 0.72 s / 1152 MiB | **0.94 s / 639 MiB** | **0.94 s / 645 MiB** |
| Clang `-gpubnames` + lld `--debug-names` (merged) | 1.01 s | 0.69 s / 1152 MiB (gdb ignores it) | 0.67 s / 1151 MiB | 1.11 s / 598 MiB | 1.00 s / 604 MiB |
| kiln+hod `-O1`, no index (mortar) | 0.59 s | 0.63 s / 900 MiB | 0.66 s / 900 MiB | 1.14 s / 901 MiB | 1.13 s / 903 MiB |
| kiln+hod `-O1` + lld `--gdb-index` | 0.82 s | 0.26 s / 388 MiB | 0.60 s / 438 MiB | 1.15 s / 902 MiB | 1.15 s / 902 MiB |

The link times in this table are single rows taken between debugger
runs. §3 has the controlled ones.

### What this shows

1. **gdb reads only `.gdb_index`, and only a non-empty one.**
   - **The `.debug_names` gdb ignores.** gdb 16.3 ignores every
     `.debug_names` an LLVM tool writes, with one of two warnings:
     "`.debug_names` not created by gdb; ignoring" (lld's merged table),
     or "Section `.debug_names` … length … does not match section
     length, ignoring" (per-unit tables concatenated, as GNU ld, lld
     and mold leave them).
   - **The `.gdb_index` gdb ignores.** It also ignores a `.gdb_index`
     whose symbol table is empty (`maint print objfiles`: "Cooked index
     in use").
   - **What it reads.** gdb reads only a non-empty `.gdb_index`, or the
     `.debug_names` that gdb itself writes (augmentation `GDB3`).
2. **lldb reads only `.debug_names`, and per-unit tables are enough.**
   - **`.gdb_index` gives it nothing.** lldb 19 ignores `.gdb_index`
     entirely: its rows equal no-index.
   - **`.debug_names` helps.** It uses `.debug_names`: Clang's per-unit
     tables, as concatenated today, cut lldb from 1.2–1.3 s / 1.2 GB to
     0.94 s / 640 MiB.
   - **Merging adds nothing.** lld's merged table gives the same:
     1.0 s / 600 MiB.
3. **The gain of `.gdb_index` is memory more than time.**
   - **At 4000 units.** gdb's start drops from 0.65–0.89 s to 0.24 s.
     The first breakpoint drops from about 0.8 s to about 0.6 s, with
     mold's index slower at 1.1 s. Peak RSS drops from 1.1 GB to
     0.4 GB.
   - **Without an index, gdb spends cores.** It uses 1.5–1.7 s of CPU
     on all cores; on one worker thread it takes 1.9 s.
   - **At 10× this size (estimate).** Without an index, roughly 6–9 s
     and 11 GB per gdb start, against about 2.5 s and 4 GB with one.
4. **gdb's index cache is the debugger's own answer.** With
   `set index-cache enabled on`, the second and later starts of the
   same file equal a linked-in index (0.57 s / 434 MiB). It is keyed by
   build-id, so every relink pays one full scan (1.56 s here) on its
   first debugger start.
5. **`gdb-add-index` gives a complete index, at a price.** It costs
   3.3 s and 1.3 GB after every link at 4000 units: 4–6× the link
   itself. It rewrites the output, so it also breaks an incremental
   link's state (§3).

## 2. What an index needs, per producer, and what the linkers do without it

These results come from the census program (stage A) plus a third unit,
`c.cc`, that holds a type, a global and a namespaced function found
nowhere else. It was compiled `-O0 -gsplit-dwarf` by each producer and
linked by each linker.

| producer and flags | name sections in the object | lld / mold `--gdb-index` | lld `--debug-names` | gdb with it |
|---|---|---|---|---|
| GCC 14.2 `-gsplit-dwarf` (also with `-gno-pubnames` or `-gpubnames`) | `.debug_gnu_pubnames`/`pubtypes` always | complete (30 symbols) | nothing (needs input `.debug_names`) | uses it; complete (§2.1) |
| Clang 19/22 `-gsplit-dwarf` (default) or `-ggnu-pubnames` | `.debug_gnu_pubnames`/`pubtypes` | complete (23 symbols) | nothing | uses it |
| Clang `-gsplit-dwarf -gno-pubnames` | none | **2 symbols, none from the units** | nothing | ignores it, scans (gdb-add-index: 23) |
| Clang `-gsplit-dwarf -gpubnames` | per-unit `.debug_names` | built from them (30 symbols); the per-unit tables are kept too | merged (1 table, 23 names) | uses `.gdb_index`; ignores `.debug_names` |
| kiln++ + hod 1.0.0, `-O0` (with or without `-ggnu-pubnames`) | **none** (hod writes no `.debug_names`, no type units, and no GNU pubnames at `-O0`) | **2 symbols, none from the units** | nothing | ignores it, scans (gdb-add-index: 22) |
| kiln++ + hod, `-O1`/`-O2` | `.debug_gnu_pubnames`/`pubtypes` (38.6 MB at 4000 units) | complete | nothing | uses it |
| `gdb-add-index` (any producer) | reads the `.dwo` files | complete (gdb's own scan) | — | uses it |

lld and mold build `.gdb_index` from the GNU pubnames sets (or Clang's
per-unit `.debug_names`), plus each skeleton's address ranges. Neither
opens a `.dwo` file. Both drop the input `.debug_gnu_pubnames`/`pubtypes`
from an output that has an index. On protoc that took the output from
32.4 to 21.8 MB; at 4000 units the 58 MB index outweighs the 38.8 MB of
pubnames it replaces.

### 2.1 Completeness, and the one wrong answer

**Pure builds: complete.** On protoc (GCC), gdb was asked for 300
randomly sampled functions (`info line`) and 150 classes (`whatis`). It
resolved the same 351 names, and missed the same 99, with no index, with
lld's index, with mold's and with `gdb-add-index`'s. A linker-built
index of a build that is pure GCC, or pure default Clang, is as complete
as gdb's own scan.

**Mixed builds: wrong, silently.** Take a unit without pubnames — kiln
at `-O0`, Clang `-gno-pubnames`, or any prebuilt library compiled that
way — linked next to one that has them. lld's and mold's `.gdb_index`
is then non-empty, gdb trusts it, and that unit's names are missing
from it:
- `ptype OnlyInC` gives `No symbol "OnlyInC" in current context`.
- `print g_only_c` gives `'g_only_c' has unknown type`.
- Functions still resolve through the ELF symbol table.

The same program has no such failures without an index, with
`gdb-add-index`, or under lldb. Neither linker warns. It is the one
case where an index makes the debugger wrong rather than slow. A mortar
index must not reproduce it: see S-G3 below.

## 3. What building an index costs the link

Each configuration was linked 1 + 7 times, interleaved. The table gives
the median wall time (min–max) and the median RSS.

| workload | mortar (no index) | lld | lld `--gdb-index` | lld `--debug-names` | mold | mold `--gdb-index` |
|---|---|---|---|---|---|---|
| syn-gcc (264 MB output) | 0.55 s / 1023 MiB | 0.63 s (0.62–0.64) / 1257 MiB | **0.69 s (0.69–0.71) / 1381 MiB** | 0.63 s / 1254 MiB (no input tables: nothing to merge) | 0.76 s / 1045 MiB | **0.82 s (0.80–0.84) / 1179 MiB** |
| protoc-gcc (32 MB output) | 0.09 s / 159 MiB | 0.06 s / 210 MiB | 0.06 s / 209 MiB | 0.06 s / 210 MiB | 0.03 s / 181 MiB | 0.03 s / 189 MiB |

The Clang `-gpubnames` series (single rows, §1) put lld's merged
`.debug_names` at 0.76 → 1.01 s and 1382 → 1686 MiB.

**Full link.**
- **`.gdb_index`** costs lld and mold about 0.06 s (+8 to +10%) and
  120–135 MiB at 4000 units. That is about 7 ms and 15 KiB per 100,000
  names. It is below the noise on protoc.
- **lld's `.debug_names` merge** costs more: +0.25 s, +300 MiB.

**Incremental update.** Both indexes are whole-output tables.

- **`.gdb_index`**:
  - **What it holds.** A CU list (offset and length of every unit: 16
    bytes each), an address area (20 bytes per range), an open-address
    hash table sized to a power of two, and a constant pool of names and
    CU vectors.
  - **What a changed file disturbs.** Its unit's entry in the CU list
    moves when IN-50 moves the unit. Its address ranges change. Its
    names come and go, which can change which slot every other name
    lands in.
  - **What rewriting it costs.** A rewrite in full writes 5.9 MB
    (protoc) to 58 MB (4000 units) per update. That is 11–110 ms at
    IN-54's 1.9 ms/MB, plus hashing every name: about the 0.06 s the
    full link spends. protoc's whole update floor is 52–88 ms
    (`incr.md` R14), so the index alone would exceed it.
- **merged `.debug_names`**: a sorted bucket and hash array over all
  names, plus per-CU entry pools. It is the same whole-table rewrite,
  and larger.
- **Per-unit `.debug_names` and GNU pubnames, as I-49a concatenates
  them**: these are per-contribution. IN-55 can neutralize a dead set,
  and WP-065's option (B) does exactly that. Clang's `-gpubnames` tables
  are what lldb already uses at merged-table speed, so they are the one
  index that is incremental by construction.
- **Today.** IN-58 refuses `--gdb-index` and `--debug-names` in
  incremental mode, and IN-55 falls back on any name index. An update
  never has to rewrite either.

Three ways to keep `.gdb_index` in an incremental output:

| way | what an update does | cost |
|---|---|---|
| **(i) none** (today) | the index is refused in incremental mode | gdb scans, or uses its index cache: one scan per update |
| **(ii) rewrite whole** | rebuild from per-file name sets kept in the state: the pubnames are already in the inputs; the CU list and ranges come from the layout | O(all names) per update: 11–110 ms of writes plus hashing; the state grows by the name sets (about 40 MB at 4000 units, unless they are re-read from the output) |
| **(iii) slack and patch** | the hash table at a low load factor, with free space in the pool. A changed unit keeps its CU index; its old names are removed (tombstone slots) and its new ones inserted; overflow forces (ii) | O(changed names); a new format discipline to specify and test against gdb (IN-59 transcripts) |

## 4. Recommendation

**Alternatives considered:**

- **(a) Nothing: the debuggers scan.** It costs the link nothing. gdb
  pays about 1 GB and 0.6–0.9 s at 4000 units, and much more at the
  operator's scale (estimate §1.3). gdb's index cache recovers the
  index speed on every start after the first, per build-id.
- **(b) `.gdb_index` only.** gdb's memory falls 3× and its start time
  3–4×, for +8–10% link time. lldb gains nothing.
- **(c) merged `.debug_names` only.** It gives gdb 16 nothing (gdb
  ignores LLVM tables), and gives lldb nothing over the concatenated
  per-unit tables mortar already writes. GCC cannot produce the inputs.
  It costs more than (b).
- **(d) both:** (b)'s gain and (c)'s cost.

**Recommended: (b) `.gdb_index`, as an option, off by default** (as in
GNU ld, lld and mold). It would be built from the GNU pubnames sets and
the skeletons, never from the `.dwo` files (I-49a stands). It would be
refused by name whenever a debug-bearing unit has no name set, and in
incremental mode (IN-58 unchanged; way (i)). The reasons:

1. **The distinguishing risk is correctness.** A linker-built index is
   silently wrong for mixed builds (§2.1). mortar's version would be
   "complete or refused by name". That is a guarantee lld and mold do
   not give.
2. **The benefit is real but modest at this size.** It is mostly
   memory. It belongs to the full link, where it costs 8–10%. "A
   non-incremental link must not pay for incremental mode", and
   symmetrically an incremental link should not pay a whole-table
   rewrite.
3. **The `.debug_names` work stays as it is.** Concatenation (I-49a)
   already gives lldb the fast path for Clang `-gpubnames` objects, and
   it is incremental-friendly (IN-55, WP-065 (B)). `--debug-names`
   stays refused, with the reason; `--no-debug-names` becomes a no-op.
   Today `rows.rs` 132 refuses it, which is a small defect: it is the
   default.

### Rules a spec section would need (`spec/synth.md` §13, new; `image.md` I-54 amended)

| id | rule |
|---|---|
| S-G1 | `--gdb-index` writes one `.gdb_index` (version 7, the version lld and mold write and gdb 16 reads; version 8+ only if a test shows gdb needs it), a synthetic non-alloc chunk placed after the `.debug_*` sections |
| S-G2 | Its CU list is every `.debug_info` unit of the output in output order (offset, length). Its TU list is empty: type units are refused with the option until tested |
| S-G3 | **Every** live unit that has debug information must have a `.debug_gnu_pubnames` and a `.debug_gnu_pubtypes` contribution (or Clang's per-unit `.debug_names`). Otherwise the link is refused by name (`gdb-index-incomplete`, naming the first such object): a partial index hides names from gdb (§2.1) |
| S-G4 | Names come from the sets of the units, with GDB's attribute bits (kind, static) taken from the set's flag byte; duplicates within one CU vector are removed; names are hashed with gdb's `mapped_index_string_hash` (version 7) |
| S-G5 | The address area is every live unit's ranges as the output's skeleton (`DW_AT_low_pc`/`high_pc`, or `DW_AT_ranges` through `.debug_rnglists`/`.debug_addr`) and `.debug_aranges` state them after relocation; ranges at a tombstone (I-47) are left out |
| S-G6 | With `--gdb-index`, the input `.debug_gnu_pubnames`/`pubtypes` are not written to the output (what lld and mold do); without it, I-49a is unchanged |
| S-G7 | The bytes are a pure function of the layout and the name sets (deterministic: sorted CU order, the hash table's size = the next power of two above 4/3 of the name count, as lld does) |
| S-G8 | `--gdb-index` with `--incremental` stays refused (IN-58); with `-r` or `--strip-debug` it writes nothing |
| S-G9 | `--debug-names` stays refused by name, with the reason in the diagnostic; `--no-debug-names` is a no-op |

### The verifier's side

The verifier should judge an index independently, without reusing the
writer.

**Structure:**
- Parse the header and four areas, check the offsets are increasing and
  inside the section, and check the version.
- The CU list equals the units the verifier finds by walking
  `.debug_info`.
- Each address range lies within a live unit's ranges, from the skeleton
  as the verifier reads it.
- No range touches a tombstone.

**Content:**
- For every input name set of a live unit (from the map's debug
  contributions), each name looks up through the hash table (gdb's
  probe sequence) to a CU vector naming that unit with the set's
  attributes.
- Every entry in the table comes from some set.
- With S-G3, there is no unit without names.

**Oracle:**
- An L6 debugger test: gdb transcripts with and without the index are
  equal, for GCC, Clang and kiln at `-O1`.
- A mixed-producer test: the link is refused.

### The package cut

| package | what | size |
|---|---|---|
| WP-a | `.gdb_index` writer (S-G1–S-G9) in `mortar_synth`; rows.rs `--gdb-index` supported, `--no-debug-names` no-op; link tests in `tests/link/split/` (GCC, Clang, kiln `-O1`; mixed refused); debugger sessions with and without | 1,500–2,500 lines with tests, 1 session |
| WP-b | the verifier's index check (plumb, its owner) | 600–1,000 lines |
| WP-c (later, M7, only if (i) is too slow in practice) | way (iii), a patchable index with slack, behind IN-59 transcripts | 2,000+ lines; needs its own measurement on the operator's workload |

`.debug_names` merging is not proposed. A `.dwp` writer is not argued
for: the `.dwp` row of stage A shows the debuggers equal with or without
one, and an index does not need it.

## 5. Questions for the operator

1. Should the Mason development profile turn `--gdb-index` on for full
   links? It costs +8–10% link time and gives gdb 3× less memory. Or
   should it rely on gdb's `index-cache`? That is one scan per relink,
   and it changes nothing in the linker.
2. In incremental mode, is (i) "no index, gdb scans or caches"
   acceptable for v1, with (iii) a later package only if measured
   necessary? (ii) is ruled out by its cost at protoc's update floor.
3. hod writes GNU pubnames at `-O1`/`-O2` but not at `-O0`, even with
   `-ggnu-pubnames` (kiln sha b6c546bd6, hod 1.0.0). Under S-G3, a kiln
   development build at `-O0` would have its `--gdb-index` refused.
   Should hod be asked (it is open to requests since 1.0.0) to emit
   them at every level, or to honour `-ggnu-pubnames`?
4. Should S-G3's refusal be an error (correct-or-refused), or a warning
   that writes no index? The latter is still correct: gdb then scans.

## 6. What was not measured

- **The operator's full scale.** Nothing above 4000 units, about 0.94 GB
  of inputs, was measured; the 10× figures are linear estimates.
- **Other forms and setups:**
  - DWARF 4 split;
  - `.dwp`-packaged outputs with an index;
  - x86-64 and i386 (the host's debuggers are AArch64).
- **Debugger work beyond the first breakpoint:** breakpoints on
  templates, and `run` to the breakpoint.
- **lldb 22.** Only lldb 19 is installed.
- **GNU dwp** (not installed; stage A's R1).
- **mortar itself writing an index.** Nothing here is shipped code: the
  link costs are lld's and mold's, used as stand-ins.
