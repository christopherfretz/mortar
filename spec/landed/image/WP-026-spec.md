# WP-026 stage 1 — `spec/image.md`: method and evidence

This document backs `spec/image.md` (draft). It gives the method, the
experiments the measured and the non-obvious rules rest on, the programs
that made the measurements (inline, or described precisely enough to
rewrite), and what could not be checked. The labels **E1**–**E15** are
the ones `image.md` cites.

## 1. Method

- **Sources.** The gABI and its note format; DWARF 4 and 5 (list
  encodings, the address tombstone); the GNU ld, lld and mold manuals and
  `--help` texts; the Rust 1.95 standard library's public API. No
  linker source was read; no code or comment was copied from one.
- **Oracle.** The observed behaviour of GNU ld 2.44 (`ld.bfd`), lld 19.1.7
  (`/usr/bin/ld.lld`) and 22 (`/usr/lib/llvm-22/bin/ld.lld`), and mold
  2.37.1, run directly or through `gcc -fuse-ld=…`; gdb 16.3, lldb 19.1.7,
  `llvm-dwarfdump-22`, `addr2line` and `eu-unstrip` as consumers.
- **Host.** AArch64 (implementer 0x61), 24 CPUs, 31 GB, Linux
  6.12.76-linuxkit, transparent huge pages `always`, `CLK_TCK` 100.
  Shared with other agents: the 1-minute load average was 23–36 during
  every measurement, and each result line records it. Timings are
  therefore given as medians of 3 with ranges, and the spec argues from
  ratios measured in the same batch.
- **File systems.** `/tmp` is the container's overlay (36 GB free); the
  workspace is a host share ("fakeowner", 87 GB free). Both were
  measured because the bench harness writes to the second (§10.6 of the
  spec). The workspace is mounted `noexec`, so every program was built
  and run in one directory, `/tmp/wp026-img`, removed at the end; the
  write matrix's workspace runs used
  `/Volumes/AgentWork/mortar-helper/.build/scratch-wp026`, also removed.
- **Limits.** At most 4 processes at a time; no package installs; no
  `strace`, `perf` or `ltrace` on the host.

## 2. Experiments

### E1 — What drivers pass, and what the workloads use

```sh
for d in gcc g++ gfortran clang clang++; do $d -### m.c -o x 2>&1 | …grep the link line…; done
gcc -dumpspecs | grep -o '%{!r:--build-id}'
```

Result (trimmed to the relevant flags of each link line):

```text
gcc:      --build-id --eh-frame-hdr "--hash-style=gnu" --fix-cortex-a53-843419
g++:      --build-id --eh-frame-hdr "--hash-style=gnu" --fix-cortex-a53-843419
gfortran: --build-id --eh-frame-hdr "--hash-style=gnu" --fix-cortex-a53-843419
clang:    "--hash-style=gnu" "--build-id" "--eh-frame-hdr"
clang++:  "--hash-style=gnu" "--build-id" "--eh-frame-hdr"
%{!r:--build-id}
```

No driver passes `--gc-sections` or a `--build-id` style. In the bench
workloads' recorded link lines (`docs/status/bench.md`), every workload
has `--build-id`, only `rust-self` has `--gc-sections` (rustc adds it),
and none has `-Map`.

### E2 — Output file handling

A small static program (`ok.c`: `int main(void){return 0;}`, and a
variant with an undefined reference) was linked with each of the three
linkers over an existing output in each situation of `image.md` §7.1;
`stat -c '%i %a %h'` before and after gave inode, mode and link count;
`ls -l` showed symbolic links; a hard link made beforehand was compared
with `cmp`. The read-only-directory case was run as an
unprivileged user (`setpriv --reuid=65534 --regid=65534 --clear-groups`)
in a directory `n/d` owned by that user with mode 555 and a writable
output inside; root ignores the directory's mode, so the first attempt
as root was discarded. "Running executable": the output was started
in the background and relinked; a shell redirect onto the running file
gave `Text file busy`.

Observations: the table of `image.md` §7.1. Messages seen:

```text
ld.lld: error: cannot open output file d/out: Permission denied
mold: fatal: cannot open d/.out.<pid>: Permission denied     (pid elided)
ld.bfd: … input file 'in.o' is the same as output file
… Is a directory                                              (all three, -o DIR)
```

GNU ld removed the existing output after an undefined symbol and after a
missing input, and kept it after an unknown option. gcc's driver refuses
`-o in.o` with `in.o` an input before the linker runs, so that case was
run with the linkers directly.

### E3 — Output-writing strategies

**Programs.**

- `wb.rs` (Rust 1.95, `rustc -O`), usage `wb MODE SIZE_MIB THREADS OUT
  [SRC_FILE] [BLOCK_KIB] [setlen]`. It removes `OUT`, then (except for
  `stream` and `cfr`) builds a source buffer of `SIZE_MIB` MiB of
  xorshift64 output (seed `0x9E3779B97F4A7C15`, shifts 13/7/17, so no
  page is zero or shared), records `VmHWM`/`VmRSS` from
  `/proc/self/status`, starts the clock, creates `OUT.tmp` with
  `create_new(true).mode(0o777)`, optionally `set_len`s it, and runs
  `THREADS` scoped threads that take block indices from one atomic
  counter (blocks of `BLOCK_KIB`, default 1024):
  - `vec`/`vecpar`: `vec![0; n]` split into `chunks_mut(block)`, each
    chunk copied from the source by the worker that takes its index;
    then `vec` does one `write_all`, `vecpar` parallel `write_all_at`
    of 4 MiB pieces;
  - `chunk`: each worker owns a `vec![0; block]`, copies its block from
    the source into it and `write_all_at`s it at the block's offset;
  - `stream`: as `chunk`, but `read_exact_at` from `SRC_FILE` instead of
    copying, and flips byte 0 (a stand-in for applying a relocation);
  - `cfr`: each worker opens `SRC_FILE` and `OUT.tmp` itself, seeks both
    to the block's offset and runs `std::io::copy(&mut src.take(len),
    &mut out)`.
  It then drops the file, renames `OUT.tmp` to `OUT`, prints
  `fill_s`, `write_s`, `total_s` and the `VmHWM`/`VmRSS` before and
  after, and exits with `process::exit(0)` so the large buffers are not
  freed on the clock.
- `mw.c` (`gcc -O2 -pthread`), the mapping writer, for comparison:

  ```c
  // usage: mw MODE SIZE_MIB THREADS OUT [SRC_FILE]; MODE mmap | mmapf | mmapin
  // src: malloc'd xorshift bytes, or (mmapin) SRC_FILE mapped MAP_PRIVATE read-only.
  // t0; fd = open(OUT.tmp, O_RDWR|O_CREAT|O_EXCL, 0777);
  // mmapf: posix_fallocate(fd, 0, n); else ftruncate(fd, n);
  // dst = mmap(0, n, PROT_READ|PROT_WRITE, MAP_SHARED, fd, 0);
  // THREADS pthreads take 1 MiB block indices from an atomic counter and memcpy src -> dst;
  // t1; munmap; close; rename(OUT.tmp, OUT); t2; print copy_s, unmap_close_s, total_s, VmHWM/VmRSS.
  ```

- `run.sh FSDIR SIZE RUNS [THREADS]`: makes `FSDIR/srcSIZE` once (with
  `wb vec`), reads it once to warm the page cache, then per run executes,
  in order, `vec vecpar chunk chunk+setlen stream cfr mmap mmapf mmapin`,
  each under `/usr/bin/time -f 'ext_wall=%e ext_maxrss_kib=%M'`, and
  prints one line per program with the load average.

**Runs.** `run.sh /tmp/wp026-img S 3` and `run.sh <workspace scratch> S 3`
for `S` = 10, 200 and 2048, 8 threads; then the scaling runs (`wb chunk
2048 T`, `mw mmap 2048 T`, `mw mmapf 2048 T` for `T` = 1, 4, 16, and `wb
chunk 200 T`), and `wb chunk 2048 8 out 0 K` for `K` = 64, 256 and 4096.

**Every run** (`total_s`, three runs in order; load average of each):

```text
/tmp   10 vec      .006 .007 .006   /tmp   10 vecpar .008 .008 .007   /tmp   10 chunk  .008 .005 .005
/tmp   10 chunk+sl .006 .005 .005   /tmp   10 stream .010 .011 .007   /tmp   10 cfr    .006 .006 .006
/tmp   10 mmap     .006 .005 .007   /tmp   10 mmapf  .005 .004 .005   /tmp   10 mmapin .008 .005 .007   (load 30.7)
/tmp  200 vec      .278 .123 .108   /tmp  200 vecpar .188 .111 .132   /tmp  200 chunk  .096 .077 .085
/tmp  200 chunk+sl .090 .104 .085   /tmp  200 stream .085 .093 .082   /tmp  200 cfr    .085 .088 .079
/tmp  200 mmap     .085 .096 .082   /tmp  200 mmapf  .072 .073 .090   /tmp  200 mmapin .094 .091 .094   (load 31.1-31.3)
/tmp 2048 vec     6.210 1.470 1.597 /tmp 2048 vecpar 2.387 1.864 1.759 /tmp 2048 chunk 1.754 1.159 1.062
/tmp 2048 chunk+sl 1.321 1.134 1.016 /tmp 2048 stream 1.225 .815 1.681 /tmp 2048 cfr   .988 2.150 1.529
/tmp 2048 mmap    1.162 1.148 1.078 /tmp 2048 mmapf   .855 .766 1.097 /tmp 2048 mmapin 1.072 .962 1.343 (load 33.5-36.3)
ws     10 vec      .092 .085 .083   ws     10 vecpar .078 .083 .082   ws     10 chunk  .066 .105 .090
ws     10 chunk+sl .090 .079 .083   ws     10 stream .130 .095 .085   ws     10 cfr    .034 .030 .029
ws     10 mmap     .047 .084 .048   ws     10 mmapf  .037 .047 .040   ws     10 mmapin .069 .068 .067   (load 30.7-31.1)
ws    200 vec     2.880 1.487 1.628 ws    200 vecpar 2.142 1.575 1.368 ws    200 chunk 2.105 1.472 1.526
ws    200 chunk+sl 1.697 1.891 1.421 ws   200 stream 1.489 1.710 1.435 ws    200 cfr    .540 .330 .307
ws    200 mmap    3.686 4.560 4.090 ws    200 mmapf  1.513 3.064 1.757 ws    200 mmapin 2.081 1.678 1.913 (load 31.1-33.6)
ws   2048 vec    20.01 16.06 14.15  ws   2048 vecpar 17.21 17.46 15.43  ws   2048 chunk 17.04 16.60 14.32
ws   2048 chunk+sl 17.16 14.42 16.06 ws  2048 stream 15.60 14.93 14.92  ws   2048 cfr   4.065 5.966 3.685
ws   2048 mmap    8.482 8.500 10.42 ws   2048 mmapf  6.135 6.151 6.274 ws   2048 mmapin 11.55 7.682 12.32 (load 27.5-35.7)
```

`image.md` §10.2's tables are the medians. Memory (growth of `VmHWM`
over the program's base, which for `mw mmap`/`mmapf` includes the 2 GB
source buffer in the base): `chunk` and `stream` about 7 MiB; `vec`,
`vecpar` the output's size; `mmap` and `mmapf` 2048 MiB on `/tmp` and
599–1454 / 65–2048 MiB on the workspace; `mmapin` 4096 MiB on `/tmp`
(source and output mappings both resident) and 1721–2752 MiB on the
workspace; `cfr` nothing measurable.

The scaling runs gave the ranges quoted in §10.2 (three runs each, load
28–35). The block-size runs (`chunk`, 2 GB, 8 threads) were 1.0–1.3 s
for every block size, within the spread of the 1 MiB runs.

### E4 — The references on `cxx-medium-g`

The workload's response file (`resp-g.txt`: the bench's recorded line,
`docs/status/bench.md`, without `--dependency-file`), run from the
workload's build directory with `-o` on `/tmp` and on the workspace, 3
runs each, under `/usr/bin/time -f '%e %M'`:

| linker | `/tmp` wall (s) | workspace wall (s) | peak RSS (MB) |
|---|---|---|---|
| GNU ld 2.44 | 1.90–2.36 | 5.3–6.8 | 217 |
| lld 22 | 0.72–0.75 | 1.00–1.44 | 332 |
| mold, default (forks) | 0.63–0.88 | 1.27–1.51 | (8.5: the parent only) |
| mold `--no-fork` | 0.37–0.43 | 0.82–0.97 | 409–423 |

mold forks by default and its parent exits early, so `/usr/bin/time`
measured only the parent (8.5 MB, 0 CPU); `--no-fork` was added. lld's
output is 48,315,688 bytes; its section sizes (`readelf -S`):
`.debug_info` 0x18ae3af, `.debug_str` 0x7c3876, `.debug_line` 0x28aab6,
`.text` 0x2f8b8c, `.strtab` 0x36b1f1, `.symtab` 0x1330b0.

### E5 — What the inputs contain

`relcount.py` (Python 3, `-I`) walks every ELF64 member of every input
of the response file (extracted or not) and counts RELA records by the
class of the section they apply to and section bytes by class:

```python
# per member: for each SHT_RELA (4): target = shdr[sh_info];
#   .debug* -> debug_rel, .eh_frame -> eh_rel, SHF_ALLOC target -> alloc_rel, else other_rel; += sh_size // 24
# SHT_SYMTAB/SHT_STRTAB (2, 3) without SHF_ALLOC -> symtab_bytes; .debug* -> debug_bytes;
# other SHF_ALLOC and not SHT_NOBITS -> alloc_bytes; every header -> sections; every ELF member -> members
```

Result: 221,459 alloc, 1,913,788 debug and 64,636 `.eh_frame` relocation
records (2.2 M; 87% debug), 52.8 MB of relocation records; 102 MB of
debug bytes, 8.4 MB of alloc bytes, 21.5 MB of symbol and string tables;
143,907 section headers in 168 members.

### E6 — CPU costs: relocation walks and hashes

- `rb.rs` (`rustc -O`), usage `rb THREADS FILE…`: loads every RELA
  section of every ELF64 member into owned vectors, with the member's
  symbol base; builds a `sym_map`-like `Vec<u32>` (symbol `i` → `i ×
  2654435761 mod (nsym/4 + 1)`), a flag column of `AtomicU32`, a
  1024-entry class table (`t mod 7`), and an address column. Then, on 1
  and on `THREADS` threads (sections handed out by an atomic counter):
  `scan` decodes each record (offset, info, addend), maps the symbol,
  classifies the type and `fetch_or`s a class bit into the key's flag
  word; `fill` resizes a per-section buffer to the target's size, and
  writes `addr[key] + addend` as 8 bytes at `r_offset`. It prints
  ns/record. On `cxx-medium-g`'s inputs: `scan` 3.35–3.68 ns/record on
  1 thread and no faster on 8 (every record does a contended
  `fetch_or`); `fill` 2.0–3.4 ns/record.
- `hb.rs` (`rustc -O`), usage `hb SIZE_MIB THREADS BLOCK_KIB`:
  from-scratch SHA-1 and SHA-256 (FIPS 180-4, straightforward 80- and
  64-round loops on `[u32; 80]`/`[u32; 64]` schedules, no unrolling) and
  XXH64 (the published algorithm: four lanes over 32-byte stripes,
  merge, tail of 8/4/1 bytes, avalanche), checked against `sha1("abc")
  = a9993e36…`, `sha256("abc") = ba7816bf…`, `xxh64("", 0) =
  0xEF46DB3751D8E999`. It times each on 64 MiB on one thread, then a
  two-level tree over `SIZE_MIB` MiB of xorshift bytes: `THREADS`
  workers hash `BLOCK_KIB` leaves (atomic counter), and the concatenated
  leaf digests are hashed once. Results (load ~30): one thread SHA-1
  267–279 MB/s, SHA-256 183–188 MB/s, XXH64 13.1–13.5 GB/s; tree over 2
  GiB with 1 MiB leaves: SHA-1 1.10 s (8 threads), 0.66 s (16); SHA-256
  1.53 s, 1.06 s; XXH64 0.022 s, 0.017 s.

### E7 — Are relocation records sorted by offset?

```python
# relsort.py: per SHT_RELA section of every ELF64 member, is r_offset non-decreasing?
import struct, sys
st = {'sorted': 0, 'unsorted': 0, 'unsorted_big': 0, 'big': 0, 'maxsec': 0, 'unsorted_names': {}}
def obj(b, o):
    if b[o:o+4] != b'\x7fELF': return
    shoff, = struct.unpack_from('<Q', b, o + 0x28); shnum, shstr = struct.unpack_from('<HH', b, o + 0x3c)
    S = [struct.unpack_from('<IIQQQQIIQQ', b, o + shoff + 64 * i) for i in range(shnum)]
    so = S[shstr][4]
    name = lambda s: b[o + so + s[0]:b.index(b'\0', o + so + s[0])]
    for s in S:
        if s[1] != 4: continue
        t = S[s[7]]; n = s[5] // 24
        offs = [struct.unpack_from('<Q', b, o + s[4] + 24 * k)[0] for k in range(n)]
        ok = all(offs[k] <= offs[k + 1] for k in range(n - 1))
        big = t[5] > (1 << 20)
        st['maxsec'] = max(st['maxsec'], t[5]); st['big'] += big
        if ok: st['sorted'] += 1
        else:
            st['unsorted'] += 1; st['unsorted_big'] += big
            k = name(t).decode(); st['unsorted_names'][k] = st['unsorted_names'].get(k, 0) + 1
# main: archives walked member by member (60-byte headers, size at [48:58], 2-byte alignment)
```

| inputs | relocation sections | unsorted | target larger than 1 MiB | largest target |
|---|---|---|---|---|
| `cxx-medium-g` (`main.cc.o`, `libprotoc.a`, `libprotobuf.a`) | 36,449 | 0 | 1 | 1,137,035 B |
| `Scrt1.o`, `crti.o`, `crtbeginS.o`, glibc `libc.a`, GCC 14 `libstdc++.a`, `libgcc.a`, three Clang `-g` objects of E9 | 9,399 | 0 | 0 | 60,716 B |
| `us.c` at `-O2 -g` by Clang and by GCC | 13 | 0 | 0 | 222 B |

An assembler file with `.reloc 8, R_AARCH64_ABS64, x` before `.reloc 0,
R_AARCH64_ABS64, y` in `.data` gives records in that (unsorted) order
with both GNU as 2.44 and Clang's integrated assembler (`readelf -r`
lists offset 8 first). A second check found no relocation section
applying to an `SHF_MERGE` section in `cxx-medium-g`'s inputs, `libc.a`
or `libstdc++.a`.

### E8 — Compressed sections and map consumers

- `readelf -S` over Debian's crt objects, `libc.a`, `libm.a`,
  `libstdc++.a`, `libgcc.a` and `libgcc_eh.a`: no section has the `C`
  (`SHF_COMPRESSED`) flag (they carry no debug sections). `gcc -g -c`
  produces uncompressed `.debug_*`; `ld.bfd --help` gives
  `--compress-debug-sections` default `none`.
- Build tools on the host: CMake 3.31 only (no meson, bazel, scons,
  autotools). A search of CMake's modules for `-Map`/`--Map`/`-M` found
  map options only in the TI and ARM (ARMCC/ARMClang) compiler modules.
  Nothing installed parses a GNU map file.

### E9 — Tombstone values for references into a dropped COMDAT copy

Sources: `h3.h` declares `__attribute__((noinline)) inline int big(int
n)` whose body is 300 lines of `if (n > k) s = s * 3 + k;` on a
`volatile int s`; `e.cc` defines `fe(v) = big(v) + 1`; `f.cc` defines
`helper(x) = x * 7` and `main` calling `fe(argc)`, `big(argc + 2)` and
`helper`. (Smaller pairs `h.h`/`a.cc`/`b.cc` with `twice`, and
`h2.h`/`c.cc`/`d.cc` with `work`, were used first; in `d.cc` the call
must be `& 1`, since `& 0` let the compiler fold it away.) Compiled with
`g++-14` and `clang++` at `-O0 -g -gdwarf-4` and `-gdwarf-5`, with
`-fno-asynchronous-unwind-tables -fno-unwind-tables -fno-exceptions
-fno-dwarf2-cfi-asm` for GCC so that `.debug_frame` is produced. Linked
static `-nostdlib -e main` with `e.o` first (so `f.o`'s copy of `big`
is dropped) by each linker; also through the driver as a PIE and as a
static program, and in the **low** layout `-z max-page-size=4096 -z
noseparate-code`.

`tomb.py OUT IN1 IN2 …` (Python 3, `-I`): for the last input, every
relocation in a `.debug*` section whose symbol lies in a dropped group
member (`SHF_GROUP` and `SHF_EXECINSTR`) is located in the output
(the output section's offset plus the running offset of that input's
contribution, assuming input-order concatenation, which holds for all
three here) and the field's value printed (8 bytes for `R_AARCH64_ABS64`,
4 for `ABS32`).

Result, every compiler and DWARF version:

- lld 22 and mold: `0x1` in `.debug_loc` and `.debug_ranges`; `0x0` in
  `.debug_info`, `.debug_line`, `.debug_aranges`, `.debug_frame`,
  `.debug_loclists`, `.debug_rnglists`, `.debug_addr`.
- GNU ld: the kept copy's address plus the addend in every one of those
  sections when the dropped and kept copies have the same size; 0 when
  their sizes differ (a variant in which the two copies differ in
  size).

### E10 — What debuggers do with them

gdb script `g.gdb`: `set pagination off`, `set debuginfod enabled off`,
`break big`, `break helper`, `run`, `bt`, `info line *$pc`, `continue`,
`bt`, `continue`, `bt`, `info line *$pc`, `up`, `list`, `info scope
helper`. lldb script `l.lldb`: `settings set target.disable-aslr false`
(without it, `process launch` fails in the container with "personality
set failed"), `breakpoint set -n big`, `-n helper`, `process launch`,
`thread backtrace` ×3 with `continue`, `frame select 1`, `source list -l
5 -c 3`, `image lookup -a $pc`, `breakpoint set -f h3.h -l 5`,
`breakpoint list`. Transcripts were compared after masking addresses
and the executable's name.

- gdb: identical transcripts across the three linkers for every
  compiler, DWARF version and layout (default static, PIE, low).
- lldb, default layouts (lld/mold PIE `.text` at ≥ 0x10000; static at
  0x400000): no difference beyond addresses.
- lldb, low layout (lld's and mold's `.text` at 0x1740, inside the
  dropped copy's claimed `[0, 0x2a44)`): on lld's and mold's outputs,

  ```text
  < Breakpoint 1: where = EXE`big(int) + 8 at h3.h:3:16, address = ADDR    (GNU ld)
  > Breakpoint 1: where = EXE`big(int) + 8, address = ADDR                 (lld)
  < Breakpoint 2: where = EXE`helper(int) at f.cc:3:19, address = ADDR
  > Breakpoint 2: where = EXE`helper(int), address = ADDR
  <     frame #0: ADDR EXE`big(n=1) at h3.h:3:16
  >     frame #0: ADDR EXE`big(int) + 8
  ```

  and `main` likewise; GNU ld's output is correct.
- lld with `-z dead-reloc-in-nonalloc=.debug_info=0xffffffffffffffff`
  and the same for `.debug_line`, `.debug_aranges`, `.debug_addr`,
  `.debug_rnglists`, `.debug_loclists`, `.debug_frame`, in the low
  layout: lldb's transcript equals GNU ld's; gdb's is unchanged.
- `llvm-dwarfdump-22 --verify` on every output: "No errors."
  `addr2line -e OUT ADDR` for `big`, `helper`, `main`: correct file and
  line for every variant.

### E11 — Build-id styles

Each linker on `ok.o` with `-e main` and each style (bare, `sha1`,
`md5`, `sha256`, `fast`, `uuid`, `none`, `0xdeadbeef`), and the hex edge
cases; the note's descriptor read by `nd.py`:

```python
# nd.py: print the NT_GNU_BUILD_ID note's descsz and desc bytes
import struct, sys
b = open(sys.argv[1], 'rb').read()
shoff, = struct.unpack_from('<Q', b, 0x28); shnum, shstr = struct.unpack_from('<HH', b, 0x3c)
S = [struct.unpack_from('<IIQQQQIIQQ', b, shoff + 64 * i) for i in range(shnum)]
for s in S:
    if s[1] == 7:
        o = s[4]; nz, dz, ty = struct.unpack_from('<III', b, o)
        if ty == 3: print('descsz', dz, 'desc', b[o + 12 + ((nz + 3) & ~3): o + 12 + ((nz + 3) & ~3) + dz].hex(), 'note size', s[5]); sys.exit()
print('no build-id note')
```

```text
ld.bfd 0xabc  no build-id note        (warning: unrecognized --build-id style ignored)
ld.bfd 0x     no build-id note        (warning)
ld.bfd 0xABCD descsz 2 desc abcd note size 18
ld.bfd 0x00   descsz 1 desc 00 note size 17
ld.bfd 0x0    no build-id note        (warning)
ld.lld 0xabc  descsz 2 desc ab0c note size 18
ld.lld 0x     descsz 0 desc  note size 16
ld.lld 0xABCD descsz 2 desc abcd note size 18
ld.lld 0x00   descsz 1 desc 00 note size 17
ld.lld 0x0    descsz 1 desc 00 note size 17
ld.lld 0xzz   error: not a hexadecimal value: zz
mold   0xabc  fatal: invalid build-id: 0xabc     (also 0x, 0x0, 0xzz)
mold   0xABCD descsz 2 desc abcd note size 18
```

`readelf -n` (2.44) on the 2-byte note warns `note with invalid namesz
and/or descsz found at offset 0`. Lengths of the named styles: bare and
`sha1` 20 bytes in all three (lld 19 and 22 alike); `md5` 16; `fast`
lld 8, mold 32, GNU ld warning and no note; `sha256` mold 32, GNU ld
warning, lld 19 and 22 error; `uuid` 16 bytes, different on every link.

### E12 — What the references' ids hash

`h.py OUT`: finds the build-id note, zeroes its descriptor, and compares
the id with `H(whole file)` and with `H(H(block_0) ‖ H(block_1) ‖ …)` for
blocks of 1 MiB, 4 MiB and 64 KiB, `H` the hash of the id's length
(`sha1` for 20 bytes, `md5` for 16, `sha256` for 32), truncated to the
id's length. On the outputs of E11 for all three linkers: no match, for
every style. (So none is the plain digest of the zeroed file; what they
hash, or how they zero, was not determined.)

### E13 — Separate debug files by build id

For ids of 1, 2, 4, 8, 16 and 20 bytes (`--build-id=0x…`), the program
was split with `objcopy --only-keep-debug` / `--strip-debug` and the
debug file put at `DIR/.build-id/xx/rest.debug`. gdb with `-iex 'set
debug-file-directory DIR'` (with `-ex` the setting comes after the file
is loaded and has no effect) found it for ids of 2 bytes and more, not
1; lldb, given `DIR` as its debug-file search path, found it for 8 bytes
and more, not for 1 or 2 (what it does with 4 is not claimed here). `eu-unstrip -n -e` printed the id for
8 bytes and more; `eu-unstrip` merged the pair at every length tried.

### E14 — What was not verified

- **x86-64 and i386.** Every experiment ran on AArch64. The x86 trap fill
  (`0xCC`), `max_reach` for x86-64 (from `spec/arch.md` Appendix A's
  `TLSGD` window, not measured) and the low-address tombstone case on
  x86-64's 4 KiB pages are inferences.
- **GNU ld's, lld's and mold's build-id algorithms** (E12 says only what
  they are not).
- **Garbage collection.** The roots and edges of §3.2 for notes, group
  members and non-debug non-alloc sections were not compared with the
  references (`spec/a64.md` §13.2 measured the GC tombstones only);
  `--print-gc-sections`'s format in lld and mold; whether the
  references keep unreferenced merge pieces without `--gc-sections`.
- **Output files.** Whether the references `fsync`; their behaviour on
  FIFOs and sockets as outputs; ENOSPC; what happens to a `-Map` file
  when the link fails. Non-root permission cases beyond the read-only
  directory.
- **Parallel scaling** of the scan with load-before-or (I-16), and the
  combined cost of hashing and writing (I-74). Neither can be measured
  before the code exists; §16.5 of the spec gates them.
- **The cause of the write plateau** (§10.3): no `strace`/`perf` on the
  host.
- `.debug_names`, `.gdb_index`, compressed input sections (none exist in
  the system libraries), compressed output.
- **`std::io::copy`'s use of `copy_file_range`.** The toolchain ships no
  rendered documentation or library sources; the claim rests on symbol
  names in `libstd` (`kernel_copy::…::copy_regular_files`,
  `probe_copy_file_range_support`) and on the behaviour measured in E3
  (`cfr` 3.7–5.2× faster than `stream` on the host share, with no bytes
  through the process). It is platform behaviour of std, not a
  documented guarantee.
- **readelf on a padded short build-id note** (I-71): no mortar output
  exists yet.
- A faster scalar SHA-1 (unrolled) was not written; the "up to 2×" in
  I-74 is an estimate.

### E15 — Process statistics from safe std

`ph.rs` (`rustc -O`):

```rust
// allocate and touch 300 MiB, free it; print VmHWM/VmRSS and /proc/self/stat utime,stime
// (fields 14, 15: the fields after the ')' of the command name, indices 11 and 12);
// write "5" to /proc/self/clear_refs; print VmHWM/VmRSS again;
// busy-loop 0.5 s on the main thread and 0.3 s on each of 4 threads; print the ticks;
// print /proc/self/schedstat.
```

Output, run under `/usr/bin/time -f 'ext_maxrss_kib=%M'`:

```text
after 300MiB: VmHWM:   308756 kB | VmRSS:     1684 kB | utime,stime ticks (2, 0)
after reset: VmHWM:     1688 kB | VmRSS:     1688 kB
after 0.5 s + 4x0.3 s of threads: ticks (171, 0) (expect ~170)
ext_maxrss_kib=1616
```

So: `utime + stime` counts every thread (171 ticks for 1.7 s of CPU);
the clear succeeds from an ordinary process; and after it, the parent's
`ru_maxrss` is the post-reset peak (1.6 MB), not the real 308 MB, which
is why `--time-phases` never resets (I-90). `/proc/self/schedstat`
covers the main thread only. `getconf CLK_TCK` prints 100.

## 3. Ruling addressed

The supervisor's ruling of 2026-11-14 (events from `arch.scan`, one
grouped diagnostic per key by the driver, the same shape for
`comdat-mismatch` and `discarded-section-reference`, a parallel
deterministic merge) is `image.md` §4.4–§4.5 (I-18–I-24), with the
`spec/arch.md` §3.4 text change as request R-1. `spec/diag.md` was read
from `main` after the WP-020d merge (`46714d0`): its `undefined-symbol`,
`comdat-mismatch`, `discarded-section-reference` and `gnu-warning`
entries already describe per-key emission, which `image.md` implements;
it has no kind for a failed write, hence R-7.
