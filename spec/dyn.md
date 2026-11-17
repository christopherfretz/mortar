# spec: dynamic executables (`mortar_synth` dynamic tables; M2)

**Status: accepted (WP-040, after an adversarial review and a re-review); §11's requests applied to the owning specs and §15's questions ruled (WP-040r), Q1 measured and closed (WP-043).**

**§17-§18 (shared-object output, M4): draft (WP-110, `spec/landed/dyn/WP-110.md`); its requests are not yet applied and its questions (record §E) not yet ruled.**

## 1. Scope and status

M2 is the dynamic executable: `Exec` (`-no-pie` with an interpreter) and
`Pie` (every compiler driver's default), on AArch64 and x86-64, with
shared objects as inputs, linked from the lines GCC, Clang and kiln
produce by default (§2.1), run, and checked by the verifier. This spec
holds the rules that no other spec holds and rules on the places where
the owning specs contradict each other (§3); everything else it cites.
Rules are `D-n`, tagged `[M2]` or `[M4]`; a rule tagged `[M4]` is here
only because an M2 rule needs its boundary (i386 dynamic, `-shared`,
RELR, verdefs). Each rule names the experiment behind it (Appendix A);
`[unmeasured]`, `[unverified]`, `[Qn]` (a question of §15, now ruled) and `[open Q1]` mark the rest.

Not in M2: producing shared objects (`-shared`, verdefs, `DT_SONAME`,
symbol versioning of exports), RELR (`-z pack-relative-relocs`), i386
dynamic links (they follow `mortar_x86`'s static M1 work, request
R-X86-*), x86 IBT and AArch64 PAC PLTs, `--whole-archive` (M3 with
sanitizers), LTO. The hooks M6 (metadata and the ODR) and M7 (incremental)
need are in §14; they are not designed here.

Shared-object output (`-shared`) is §17, designed by WP-110 for M4:
the `Shared` column (D-100), exports, preemption and versions written
(D-110-D-118), TLS, PLT, IFUNC (D-119-D-122), layout, gc and tools
(D-123-D-125), ODR and incremental constraints (D-126, D-127), i386
(D-130); its requests to other specs, `plumb.md` included, are §18.

Owners. `mortar_resolve` (§5, D-21), `mortar_scan` (D-25's per-key
classes and decision, a pre-pass after liveness), the architecture crates
(§8's demand and slot rules and D-25's class of each type, through
arch.md), `mortar_synth` (the dynamic
chunks: §6, §7, D-40-D-42), `mortar_layout` (§9), `mortar_image` (bytes),
`mortar_plumb` (`spec/plumb.md` §12). Every M1 rule stands; a static link's output is
byte-identical before and after M2 (§D of the record,
`spec/landed/dyn/WP-040.md`).

## 2. Census (measured)

E-census: 22 driver configurations (gcc, g++, gfortran on three
architectures; clang 19 and clang 22, C and C++, three architectures;
kiln++ on AArch64) × 20 variations (`-pie`, `-no-pie`, `-rdynamic`,
`-pthread`, `-static-libgcc`, `-shared-libgcc`, `-static-libstdc++`,
`-Wl,` options, `-fuse-ld=lld|mold`, …): 429 driver invocations. Three
(kiln++ with `-shared-libgcc`, `-static-libgcc`, `-static-libstdc++`)
fail in the driver ("unknown argument"), leaving 426 linker lines,
captured through `-B` wrappers and linked by GNU ld: 408 linked and ran;
the 12 clang `-fsanitize=address` lines never linked (no
`libclang_rt.asan` on the host) and the 6 gcc asan outputs did not run
under qemu (asan is out of M2).

### 2.1 Default link lines

Abbreviations: `$G` the GCC library directory of the architecture, `$S`
its system library directory. gcc, AArch64, C (other architectures differ
only in the emulation, the interpreter and the `-L` list; x86 gcc adds
`--sysroot=/`):

```
-plugin … -plugin-opt=… (LTO plugin; noop)
--build-id --eh-frame-hdr --hash-style=gnu --as-needed
-dynamic-linker /lib/ld-linux-aarch64.so.1 -X -EL -maarch64linux
--fix-cortex-a53-843419 -pie -o OUT
$S/Scrt1.o $S/crti.o $G/crtbeginS.o -L… OBJ
-lgcc --push-state --as-needed -lgcc_s --pop-state -lc
-lgcc --push-state --as-needed -lgcc_s --pop-state
$G/crtendS.o $S/crtn.o
```

clang 19 and 22 (identical) and kiln++ (which runs clang++ 22):

```
-EL --hash-style=gnu --build-id --eh-frame-hdr -m aarch64linux -pie
-dynamic-linker /lib/ld-linux-aarch64.so.1 -o OUT
$S/Scrt1.o $S/crti.o $G/crtbeginS.o -L… OBJ
-lgcc --as-needed -lgcc_s --no-as-needed -lc -lgcc --as-needed -lgcc_s --no-as-needed
$G/crtendS.o $S/crtn.o
```

- C++ tail (gcc and clang): `-lstdc++ -lm -lgcc_s -lgcc -lc -lgcc_s -lgcc`
  (no `--as-needed` wrapping; gcc's global `--as-needed` still drops
  `libm.so.6`, clang's lines keep it).
- gfortran tail: `-lgfortran -lm -lgcc_s -lgcc [--as-needed -lquadmath
  --no-as-needed] -lm -lc -lgcc_s -lgcc` (`-lquadmath` on x86 only).
- Interpreters: `/lib/ld-linux-aarch64.so.1`, `/lib64/ld-linux-x86-64.so.2`,
  `/lib/ld-linux.so.2`; every driver passes `-dynamic-linker`.
- `-no-pie`: `-pie` dropped (no `-no-pie` is passed), `crt1.o`,
  `crtbegin.o`, `crtend.o`. `-rdynamic`: `-export-dynamic`.
  `-static-libgcc`: `-lgcc_eh` replaces `libgcc_s`. `-static-libstdc++`:
  `-Bstatic -lstdc++ -Bdynamic`. `-fuse-ld=lld|mold`: the same line.
- gcc puts `-dynamic-linker` before `-pie`, clang after.

### 2.2 Inputs on a default line

- `-lc` is the script `GROUP(libc.so.6 libc_nonshared.a
  AS_NEEDED(ld-linux-….so.N))` with absolute paths; `-lm` a script with
  `AS_NEEDED(libmvec.so.1)` (AArch64, x86-64) or a link (i386);
  `-lgcc_s` the script `GROUP(libgcc_s.so.1 -lgcc)`; libstdc++,
  libgfortran, libquadmath are ELF shared objects; `-lpthread` resolves
  to an 8-byte empty `libpthread.a` (glibc 2.34+).
- GNU ld `-t`: the group is scanned twice; the interpreter is reached only
  through `AS_NEEDED`, and is not needed by a hello.
- Archive members used by a default hello: none from `libgcc.a`; from
  `libc_nonshared.a` only `atexit` (C, E-corpus) or none (C++, Fortran).
- Needed objects of the outputs: C `libc.so.6`; C++ `libstdc++.so.6`,
  (`libm.so.6` on clang lines), `libgcc_s.so.1`, `libc.so.6`; Fortran
  `libgfortran.so.5`, `libc.so.6` (+ `libm`, `libgcc_s`, `libquadmath` on
  x86, whose tail switches `--as-needed` off again).
- mortar's reader already resolves `-l` to `.so`, expands the scripts,
  `AS_NEEDED` inside `GROUP` included, and opens the shared object: with
  the refused options stripped, every non-LTO line stops at
  `<first shared object>: unsupported feature: dynamic executable without
  -dynamic-linker`.

### 2.3 Options on the lines: their status then and now

"Then": statuses of `docs/status/options.md` (472 rows) on a mortar built
from main after WP-027e, when this census was taken (the shared release
binary of that day predated WP-027e and still refused `--push-state`,
`-z now`, `-E` and `-rpath`). "Now": re-measured by WP-056 on main
74feb7f with `cargo xtask census` (§2.1's drivers, every line of AArch64
and x86-64 linked, run and verified; `spec/landed/dyn/WP-056.md`).

| option | lines of 426 | status then | status now (WP-056) |
|---|---|---|---|
| `-dynamic-linker` | 426 | **refused** | supported (D-31; WP-053) |
| `-pie` | 404 | **refused** | supported (D-1) |
| `--hash-style=gnu` | 426 | noop "dynamic is refused" | supported; `sysv`/`both`/`none` refused by name (D-20, R-C1): the 30 `hash-sysv`/`hash-both` lines |
| `--as-needed`, `--no-as-needed` | 282, 155 | noop "shared inputs refused" | supported (D-11) |
| `--push-state`, `--pop-state` | 57 | supported | supported (they carry the as-needed state) |
| `--build-id` | 426 | partial (bare form) | partial (`=md5`, `=uuid` refused); the note is in the first `PT_LOAD` after `.interp` |
| `--eh-frame-hdr` | 426 | supported | supported; off unless given (GNU ld, lld; Q3, ruled) |
| `-export-dynamic`/`-E` | 22 | noop | supported (D-21); no effect in a static-pie (D-1; WP-056 §3.1, WP-088) |
| `--export-dynamic-symbol` | 0 | supported (matched literally) | supported, by name and by glob (resolve.md §4.2; WP-056 §2); no effect in a static-pie |
| `-rpath` | 22 | noop | supported (D-30 row 2); `--enable-new-dtags` default, `--disable-new-dtags` writes `DT_RPATH` |
| `-z now` | 22 | supported (static) | supported (D-49), PIE and `-no-pie` |
| `-z relro`, `-z norelro` | 22, 0 | supported | supported; RELRO is the default (all three); `-z norelro` per D-33, D-42, D-49 |
| `-z text` | 0 | supported | supported (the default, D-47) |
| `-Bdynamic` (`-call_shared`, `-dy`) | 9 | **refused** | supported: the inverse of `-Bstatic` |
| `-Bstatic` | 9 | supported | supported; a shared object found under it is never chosen |
| `-init`, `-fini` | 0 | refused | supported (D-30) |
| `--no-copy-dt-needed-entries` | 0 | — | noop; the positive form refused (D-15) |
| `--whole-archive` | 12 (asan) | refused | refused: out of M2 |
| `-plugin`, `-plugin-opt`, `-X`, `-EL`, `-m`, `--sysroot=/`, `--fix-cortex-a53-843419` | | supported / noop | unchanged |

No driver on this host passes `-z relro`, `-z now`, `-z text`,
`--no-undefined` or `--hash-style=both` by default; RELRO, lazy binding,
`-z text` and `--allow-shlib-undefined`-off are the linkers' own
defaults, and mortar's defaults are GNU ld's on each (D-13, D-47, D-49).
### 2.4 Relocation types sent into a dynamic context

E-corpus: C, C++ (exceptions, `thread_local` with dynamic initialization,
template statics, `&std::cout`, `&typeid`, `new(nothrow)`) and Fortran
(module variable, allocatable, common block), compiled by GCC 14,
Clang 19 and Clang 22 (gfortran for Fortran) at `-O0`/`-O2` in the modes
default, `-fno-pic`, `-fPIC`, `-fPIE`, `-fno-plt`, `-fPIC -fno-plt`, plus
`-mtls-dialect=trad`/`-mtls-size=48` (AArch64) and `gnu2` (x86); 75 954
relocation rows from every mode (8 compilations fail: Clang does not
support `-mtls-dialect=trad`). The 120 executables linked by the four
references and run, with identical output everywhere, are of the modes
default and `-fno-pic` only, one mode per link; the mixed-mode links
(`corpus/mixed.sh`) are reported below the table. Every compiler defaults to PIE
(`__PIE__ 2`).

| arch | types against a shared-object target, and where |
|---|---|
| AArch64 | `ABS64` (data, RELRO, init arrays: functions, IFUNCs, data, weak undefined); `ADR_PREL_PG_HI21` + `ADD_ABS_LO12_NC`/`LDST{8,32,64,128}_ABS_LO12_NC` against **shared data and functions** in `-fno-pic` code (copy relocation, canonical PLT); `ADR_GOT_PAGE` + `LD64_GOT_LO12_NC` (every mode); `CALL26`/`JUMP26` (functions, IFUNCs, **weak undefined**); `TLSDESC_*` (`-fPIC`, GCC's default dialect; also in `libgcc_eh.a`); `TLSIE_*` against **shared TLS** (libstdc++'s `__once_call`, from `std::call_once` in a PIE); `TLSGD_*` only under `-mtls-dialect=trad`; no `TLSLD_*` from any producer |
| x86-64 | `PC32` against **shared data in GCC's default PIE** (`stdout`, `environ`, `optind`, `std::cout`: GCC assumes copy relocations in a PIE) and in every `-fno-pic` object; `32`/`32S` in `-fno-pic` (`.eh_frame`/`.gcc_except_table` against `__gxx_personality_v0`, `_ZTISt9exception`); `64` in data and, from Clang `-O0 -fno-pic`, in `.text` (`movabs`); `REX_GOTPCRELX`, `GOTPCRELX` (`-fno-plt` calls), plain `GOTPCREL` (`-O2` weak tests); `PLT32` (functions, IFUNCs, weak undefined); `TLSGD`/`TLSLD`/`DTPOFF32` (`-fPIC`); `GOTPC32_TLSDESC`/`TLSDESC_CALL` (`gnu2`, with `_TLS_MODULE_BASE_`); `GOTTPOFF` against **shared TLS**; `TPOFF32` |
| i386 | `32` (everywhere, `.text` in `-fno-pic`, `.eh_frame` against functions); `PC32` (`-fno-pic`); `PLT32`; plain `GOT32` (Clang and G++ `-O2`; `Scrt1.o` against `main`/`__libc_start_main`); `GOT32X`, `GOTOFF`, `GOTPC`; `TLS_GD`, `TLS_LDM`, `TLS_LDO_32` (`-fPIC`); `TLS_GOTDESC`/`TLS_DESC_CALL` (`gnu2`); `TLS_GOTIE` (PIE); `TLS_IE` (`-fno-pic`); `TLS_LE` |

Targets beyond the obvious: `__dso_handle` (hidden, crtbegin), the hidden
`DW.ref.__gxx_personality_v0` in a COMDAT group, `STB_GNU_UNIQUE` template
statics (GCC; Clang makes them weak), a common block (gfortran), and
`atexit`, a `GLOBAL HIDDEN` definition in `libc_nonshared.a` — the only
archive member a default dynamic link extracts (C programs; C++ and
Fortran links extract none: `libgcc_s` is dynamic). The crt objects add
GOT references to `main` and `__libc_start_main` (`Scrt1.o`), weak
references to `__gmon_start__`, `_ITM_*` (crtbegin) and, in the non-PIE
`crtbegin.o` of x86-64, `R_X86_64_32`/`32S` against them.

Mixed-mode links (`corpus/mixed.sh`, added after the review): a main
and three translation units, each in its own mode (`-fno-pic`, `-fPIE`,
`-fPIC`, `-fno-plt`), against a `-fPIC` library; C and C++; GCC and Clang
22; AArch64 and x86-64; the three references; PIE and `-no-pie`; every
output run with address-equality checks (`puts`, `&stdout`, `strlen`,
library data, functions, read-only data, IFUNC and TLS, executable data
and TLS, a `puts` pointer in data) across the units, the library and
`dlsym`. Every `-no-pie` mix links everywhere and every equality held
(C++: 156 ok, 60 refused, 0 wrong). One `-fno-pic` unit changes the
whole image: in `-no-pie` it brings copies of `stdout` and library data
and canonical PLTs for `puts`, `strlen`, library functions and IFUNCs,
and the other units' GOT slots then point at those (as D-42/D-44 say).
In a PIE, a `-fno-pic` unit is refused on x86-64 by all three and on
AArch64 by GNU ld (D-44, D-47); lld links it on AArch64 with copies and
canonical PLTs; mold links all of them and writes `DT_TEXTREL` silently
in 8 (a wrong output of the D-47 kind, W6). GCC's `-fPIE` alone already
copies typeinfo and `std::cout` into an x86-64 PIE (D-43). The only
wrong outputs are IFUNC address inequalities (W3, extended) and an
executable IFUNC referenced from a library (W13), both in §13.2; D-45 is
the rule that avoids them (lld: 0 wrong).

Consequences for the rules: an **x86-64 PIE needs copy relocations**
(GCC's default code reads `stdout` PC-relatively; all three references
copy `stdout`, `stderr`, `optind`, `opterr`, `environ`, `std::cout`,
`std::cerr`, `std::nothrow` into GCC's PIEs), which arch.md §4.3's
`Pie`/`Pc`/`Shlib` = `E` and a64.md Q5 (formerly "no COPY in PIE") would refuse
(D-43, request R-A3); every default `printf`-to-`stdout` x86-64 program
hits it. AArch64 PIEs reach shared data through the GOT.

### 2.5 Dynamic features of the system's shared objects

E-corpus task 2, Debian 13 (glibc 2.41, GCC 14 runtime):

| feature | AArch64 | x86-64 | i386 |
|---|---|---|---|
| `libc.so.6` verdefs (max `vd_cnt`) | 23 (2), incl. `GLIBC_ABI_DT_RELR`, `GLIBC_PRIVATE` | 42 (2) | 53 (2) |
| hidden `versym` entries in libc | 475 | 531 | 689 |
| hash tables of libc | `DT_GNU_HASH` | both | both |
| libc `DT_FLAGS` / `FLAGS_1` | `BIND_NOW` / `NOW` | `BIND_NOW|STATIC_TLS` / `NOW` | same as x86-64 |
| `DT_RELR` | glibc's libraries and `ld.so`; not libstdc++, libgcc_s, libgfortran | same | same |
| IFUNC exports | libc 7, libm 0 | libc 63, libm 91 | libc 48, libm 27 |
| TLS exports | libc: `errno`, `__resp`, `__h_errno`, `__libc_dlerror_result` (all `GLIBC_PRIVATE`); libstdc++: `__once_callable`, `__once_call` | same | same |
| `STV_PROTECTED` exports | none in any system library | none | none |
| `STB_GNU_UNIQUE` | 106 objects in libstdc++ | 106 | 106 |
| libstdc++ exported objects | ~850 in `.rodata`, ~495 in `.data.rel.ro` (typeinfo, vtables: copied into a non-PIE as read-only data) | same | same |
| libgcc_s | `TLSDESC` 1 | a lone `DTPMOD64` (local-dynamic, offset 0) | a lone `DTPMOD32` |

Features that inputs present and the rules must take (all measured):
one name with a different `st_type` or `st_size` per version (x86-64
`memcpy@GLIBC_2.2.5` `FUNC`, `memcpy@@GLIBC_2.14` `GNU_IFUNC` at another
address; `_sys_errlist` four sizes); symbols that exist only under a
hidden version (`__cpu_model@GCC_4.8.0`, `__rtld_version_placeholder`,
i386 `_IO_stdout_`), which no unversioned reference may bind (resolve
§9.2); copy-relocation candidates with aliases at one address (`environ`,
`__environ`, `_environ`, of bindings `WEAK`, `GLOBAL`, `WEAK`); objects
whose value is more aligned than needed (`std::cout` at a 256-aligned
value in a 16-aligned section); libraries that bind to the executable's
copy through their own `GLOB_DAT` and `R_X86_64_64` (libc's `__environ`,
libstdc++'s 176 `R_X86_64_64` against
`_ZTVN10__cxxabiv120__si_class_type_infoE`, libgfortran's `stdout`), which
is why every alias of a copy must be exported (D-21). `libc.so` is
`GROUP(libc.so.6 libc_nonshared.a AS_NEEDED(ld-linux-*.so.*))` with
absolute paths (`/lib64/…` on x86-64); `libm.so` is a script with
`AS_NEEDED(libmvec.so.1)` on AArch64 and x86-64 and a symbolic link on
i386; `libgcc_s.so` is `GROUP(libgcc_s.so.1 -lgcc)`; libstdc++,
libgfortran and libquadmath are symbolic links. No `libc++.so` exists on
the host (Debian's is the script `INPUT(libc++.so.1 -lc++abi)`).
### 2.6 kiln's libc++ lane

kiln's libc++ test runner (`kiln/xtask/src/libcxx/run.rs`, `link_flags`)
links each test with clang++ 22, given a libc++ package tree, as
`-stdlib=libc++ -pthread -L<tree> -Wl,--as-needed -Wl,-Bstatic
-lc++experimental -lc++ -lc++abi -Wl,-Bdynamic`: a dynamic PIE against
glibc and libgcc_s with libc++ linked statically, so the lane needs
`-pie`, `-dynamic-linker`, `--as-needed`, `-Bstatic`/`-Bdynamic` and
nothing beyond the C++ target of §D. Without a tree it names
`-lc++abi` dynamically, and the host has no `libc++.so` (Debian's would
be a script, `INPUT(libc++.so.1 -lc++abi)`). mortar's Mason lane
(WP-034) links the suite `-static`; it moves to the dynamic line once
the C++ target is met, and the static and dynamic suites then run side
by side, which is M2's end-to-end acceptance on AArch64.

### 2.7 What is refused, ranked: then and now

Then (when the census was taken), by lines blocked (of 426):

1. `-dynamic-linker` (option): 426.
2. A dynamic output itself (shared-object inputs, `.dynamic`, every
   table of this spec): 426 (384 measured behind the options; the 42 LTO
   lines stop at LTO first).
3. `-pie` (option): 404.
4. i386 linking: 137 (every i686 line; static i386 is `mortar_x86`'s
   WP-030b, dynamic i386 is M4 here).
5. `-Bdynamic`: 9 (`-static-libstdc++`, and kiln's libc++ lane).
6. Out of M2: `--whole-archive` (12, asan), LLVM bitcode and GCC LTO
   objects (21, `-flto`).

The noop rows whose reason was "dynamic is refused" (`--hash-style`,
`--as-needed`, `--no-as-needed`, `-E`, `-rpath`) became supported with
items 1-3 (WP-M2-CLI).

Now (WP-056, `cargo xtask census`, mortar of main 74feb7f): of the 289
AArch64 and x86-64 lines, 237 link, run as GNU ld's and are verifier
clean (3 of them asan under qemu, failing alike with GNU ld's); the rest
are refused by name, each by a rule that puts it out of M2:

1. `--hash-style=sysv`, `--hash-style=both`: 30 (D-20).
2. LTO objects (`GCC LTO object`, `LLVM bitcode`): 14.
3. `--whole-archive` (Clang's asan runtime): 8; GNU ld cannot link these
   lines either on this host.

The 137 i686 lines stay refused (`dynamically linked output on i386`;
M4). The variations a real build adds (`x-` in the census lane) all
link; `-static-pie --export-dynamic` is accepted and has no effect (D-1;
WP-056 §3.1; the verifier judges the line: P-D14, WP-088).

## 3. Inventory of what the specs already say

Read at 7bfe1af. Status: **ok** complete and consistent; **unmeasured**
specified, not backed by an experiment in its spec (the column "here"
says which this package re-measured, §A); **incomplete** (what is
missing); **contradicts** (both rule ids). Line numbers are of 7bfe1af.
Only M2 rules and the M4 rules a dynamic executable also needs are
listed; rules that are right are cited by the rules of this spec, not
restated.

### 3.1 Input side: `resolve.md`, `input.md`, `cli.md`, `script.md`, `model.md`

| area | rules | status | here |
|---|---|---|---|
| Reading a shared object | input §8.1 (L730-763): `DT_SONAME`, `DT_FLAGS_1`, `DT_NEEDED`, `DT_RUNPATH`/`RPATH`, versioned `.dynsym`; `DF_1_PIE` input, no section headers, two sonames refused | ok; incomplete: which verdef/verneed fields matter (D-14) | E-shlib |
| `.dynsym` entries as mentions | resolve §9.1 (L1097-1115) | ok; unmeasured "hidden entries ignored" (L1112) | E-shlib |
| `versym` and keys | resolve §9.2-§9.4, model §2.3 | ok (E16 of resolve) | — |
| Rank of a shared definition | resolve §3.1 L113, §3.2 rules 2, 3, L161 | ok; incomplete: closure files are not excluded (D-12) | E-shlib |
| Soname units, needed names | input §8.2-§8.5 | ok | E-shlib |
| The `DT_NEEDED` closure | input §8.6 (L829-945) | ok for loading and search; incomplete: what a closure file's definitions satisfy (D-12); **contradicts** resolve §5.2 L769 on a missing indirect library (D-13) | E-shlib |
| `--as-needed` | resolve §9.5 L1196-1211, §4.4 | ok for the strong/weak rule; incomplete: merged soname state vs row state, order against `ref_dynamic` (D-11) | E-shlib |
| `--copy-dt-needed-entries` | resolve §9.5 L1213 "refused (M4)" | **contradicts** input §8.6 L928-934 and §15 L1645 "supported (M2)"; ruled in D-15 | E-shlib |
| `--no-allow-shlib-undefined` | resolve §5.2 L769 (lld's rule) | **contradicts** input §8.6 L940-945 (GNU's rule); ruled in D-13 | E-shlib |
| Export from an executable | resolve §9.6 L1224-1231 | ok; incomplete: closure files' references (D-12); `--export-dynamic-symbol` M2 here **contradicts** resolve §1.2 L63 (M4) | E-dynsym |
| `.dynsym` form of a `Shared` def | resolve §3.7 L315 (always an undefined entry) | **contradicts** §9.6 L1245 (a copy is defined) and the canonical-PLT form; ruled in D-23 | E-dynsym, E-reloc |
| Preemption in an executable | resolve §9.6 L1237 | ok | — |
| Copy-relocation inputs | resolve §9.6 L1245-1257, §7.3 aliases | ok; unmeasured: protected (L1256); incomplete: `STT_TLS`, `st_size` 0, IFUNC-typed (D-42) | E-reloc |
| Weak undefined, dynamic or not | resolve §5.4 L819 "arch.md says" | **contradicts** arch §2.1 L77 "resolve.md's" (circular); ruled in D-25 | E-reloc |
| `STB_GNU_UNIQUE` from a shared object | resolve §3.4 L251, §9.1 | incomplete: output binding of a copied unique, `EI_OSABI` (D-23) | E-corpus |
| `tls-mismatch` | resolve §3.4 L230-245 | incomplete: a shared `STT_TLS` definition vs a non-TLS object reference (D-16) | — |
| `_DYNAMIC`, `_TLS_MODULE_BASE_` | resolve §8.1 L1042, L1053 | ok; static-pie counts as dynamic (D-2, D-32) | E-dynamic |
| Output kind from the line | cli §3.2 L297-317 | ok (`-pie` without an interpreter is static-pie) | E-census |
| Default interpreter | input §8.4 L807-817 (refused: none) | **contradicts** synth S-106 "or the target's default"; ruled in D-31 | E-census |
| `plan.dynamic` fields | cli §3.6 L399-415 | incomplete: defaults of `new_dtags` and `hash_style` (D-30 row 2, D-20) | E-dynamic |
| `--hash-style` | cli §2.3 L107, options row L209 (noop, all values) | **contradicts** synth S-102 ("sysv/both refused (cli.md)"); ruled in D-20 | E-dynsym |
| Option rows | cli §4.2 L586-631, `docs/status/options.md` | the dynamic rows are noop only "because dynamic is refused": each re-judged in §2.3 | E-census |
| Implicit scripts | script §2.3-§2.5, §6; input §5 | ok (libc.so, libm.so, libgcc_s.so for the three targets) | E-corpus |
| Model columns | model §3.3 `file_live`, `def`; §3.7; §4.3 `Slots` | ok; incomplete: no `file_live` state for a closure file reached only from unneeded objects; two sources of export (`exported`, `EXPORT`) (D-12, D-21) | — |

### 3.2 Architecture: `arch.md`, `a64.md`, `x64.md`, `x86.md`

| area | rules | status | here |
|---|---|---|---|
| Output and target kinds | arch §2.1 | ok, except the weak-undefined circle above | — |
| Demand classes per output × target | arch §4.3 L792-808, a64 §5 | ok for a64 and x64; **contradicts** x86 X86-50 for an i386 IFUNC in a PIE (arch L793/797 vs X86-50 L1230-1253; request R-14 of x86.md unapplied) | — |
| Slot contents and dynamic record per output × target | arch §4.4 delegates; a64 §10.1 | ok for a64; **incomplete** for x64 (X-35 static only) and x86 (X86-46 static only) | E-reloc |
| Dynamic types | a64 §6.2; x64 §6.2; x86 §6.2 | a64 ok, except DTPMOD64/DTPREL64 "in shared" **contradicts** a64 §10.1 L1091 (exec/PIE unrelaxed GD); x64 lacks `DTPOFF64`, `TPOFF64`, `R_X86_64_64` as dynamic types and the word/`r_sym`/addend columns; x86 lacks the REL word contents (X86 §6.2 L693-700) | E-reloc |
| PLT, PLT0, `.got.plt` | a64 §10.2-§10.3 (measured bytes); arch A.8 (x86-64), B.4 (i386), X86-52 | a64 ok; x86-64 "not measured" (x64 §10.3); i386 IBT "as x64's" cannot be literal; lazy word of x86 and `-z now` unspecified everywhere | E-reloc |
| `.got.plt[0]` / `.got[0]` = `_DYNAMIC` | a64 §10.1 Q8, synth S-65 | x86-64 dynamic unspecified; S-65 **contradicts** a64 §10.1 L1084 for the static AArch64 header (M1) | E-dynamic |
| Canonical PLT `.dynsym` form | a64 §10.5 L1162 (`st_shndx` = the PLT's section) | **contradicts** the gABI convention (`SHN_UNDEF`, `st_value` = the entry; measured, D-23) | E-reloc |
| Copy target section | a64 §10.5 (`.data.rel.ro`) | **contradicts** layout L-80 and §8.2 (`.bss.rel.ro`); ruled in D-42 | E-reloc |
| TLS in Exec/PIE | a64 R1-R5, §10.1, §10.6 (TLSDESC eager); x64 X-23-X-32; x86 X86-32-37 | a64 ok (TLSDESC eager unmeasured, a64 Q10); **x64 and x86: every TLS relaxation is scoped to static and static-pie**; GD→IE and TLSDESC→IE have no bytes | E-reloc |
| GOT relaxation in Exec/PIE | a64 R5; x64 X-19 ("every M1 output") vs arch A.2 ("every output kind"); x86 X86-25 (static), X86-27 (candidates) | a64 ok; x64/x86 incomplete and mildly contradictory | E-reloc |
| IFUNC in a dynamic output | a64 §10.4 (`STT_FUNC` at the IPLT entry in `.dynsym`); synth S-71 (`STT_GNU_IFUNC` at the resolver in `.symtab`) | **contradicts** (S-71 vs a64 §10.4; only `.symtab` is S-71's); x86 Exec/PIE unspecified | E-reloc |
| AArch64 dynamic tags | a64 §10.3 L1124-1128 (BTI_PLT, PAC_PLT, VARIANT_PCS) | ok; incomplete: VARIANT_PCS for an IRELATIVE in `.rela.plt` | E-dynamic |
| RELR | a64 §6.3, arch §3.10 | eligibility "place aligned to word" is address-dependent on x86 (D-27, D-62; M4) | E-dynamic |
| Generic contract | arch §3.7 `PltShape`, §3.10 `DynRole` | incomplete: lazy-word target, `-z now` variant, per-arch dynamic tags, record format per role (REL vs RELA), WeakDyn decision (§11) | — |

### 3.3 Output side: `synth.md`, `layout.md`, `image.md`, `plumb.md`

| area | rules | status | here |
|---|---|---|---|
| Chunk list | synth S-3 rows 1, 4-11, 18 | incomplete: no `.plt.sec`, no `.hash`, `.interp` "dynamic output" includes static-pie (contradicts cli §3.2 L315), the `GLIBC_ABI_DT_RELR` need | E-dynamic |
| Division and sizes | synth S-4, S-108 | incomplete: no rows for any dynamic chunk | — |
| `.dynsym` | synth S-101 | incomplete: no nbuckets formula, "hashed" undefined, import `st_value`/`st_shndx` | E-dynsym |
| `.gnu.hash` | synth S-102 | incomplete: Bloom size and shift | E-dynsym |
| Versions | synth S-103 | incomplete: `vna_other`, `vna_hash`, `vn_file`, `VER_FLG_WEAK`; S-3 row 8 (verdef) tagged M2 **contradicts** resolve §9.7 / ROADMAP (M4) | E-dynsym |
| `.rela.dyn` | synth S-104 | order unmeasured; IRELATIVE "to `.rela.plt`" **contradicts** S-70 for static-pie; RELR eligibility contradicts S-107 on x86 | E-reloc |
| `.dynamic` | synth S-105 | **incomplete, the largest gap**: no `DT_INIT*`/`FINI*`, `PLTGOT`, `JMPREL`, `RELA*`, `VERSYM`, `VERNEED`, `GNU_HASH`, `SYMTAB` … and no presence rule per tag | E-dynamic |
| `.interp` | synth S-106 | **contradicts** input §8.4 (default) | E-census |
| IPLT word | synth S-69 (written 0) | **contradicts** a64 §6.2 L598, L605, §10.2 L1105 (`S+A`); M1, out of this package: listed in §15 | — |
| Section order | layout L-60 (ranks 1, 3, 4, 7, 8) | unmeasured for dynamic links (E-8 was static); no slot for `.plt.sec` or the writable copy area | E-dynamic |
| Segments | layout L-65, L-67, L-68, L-80 | ok for PIE base 0 and RELRO contents; `PT_INTERP` "dynamic, not -shared" includes static-pie (contradicts cli §3.2); phdr order measured on static only | E-dynamic |
| Milestones | layout §1 L52, L-100 | "M2 … and shared objects" **contradicts** ROADMAP (M4) | — |
| `.relr.dyn` in the fixed point | layout L-21, L-121; arch §6 | ok | — |
| Section headers of dynamic chunks | image I-40 | incomplete: `sh_link`/`sh_info`/`sh_entsize` of every dynamic section, `e_type` per kind | E-dynamic |
| Scan counts | image I-15 | incomplete: counts do not order or split RELATIVE/RELR | — |
| Debug tombstone in a PIE | image I-47 note | open M2 item (lldb lost lines at low addresses) | E-dynamic |
| Verifier | plumb §1 (stage 3 "not started"), S1 (`ET_EXEC` only), inputs `ET_REL` only, no copy or dynsym map records | **cannot verify any dynamic output**; requests in §12 | — |
## 4. Output kinds

**D-1 [M2] (the table).** The four executable kinds of arch.md §2.1 and
cli.md §3.2, side by side; other specs cite a row by its name. Measured
in E-dynamic (108 outputs: C, C++, Fortran hello × PIE, `-no-pie`,
`-static-pie` × three architectures × four references, all run).

| property | `Static` | `Exec` | `Pie` | `StaticPie` [rows M1; end-to-end gate M2: D-2, synth S-70, ROADMAP] |
|---|---|---|---|---|
| line | neither `-pie` nor `-dynamic-linker` | `-dynamic-linker` without `-pie` | `-pie -dynamic-linker` | `-pie` without `-dynamic-linker` (`-static-pie`, `--no-dynamic-linker`) |
| `e_type` | `ET_EXEC` | `ET_EXEC` | `ET_DYN` | `ET_DYN` |
| base (layout L-68) | 0x400000 (i386 0x08048000) | same | 0 | 0 |
| `.interp`, `PT_INTERP` | no | yes (D-31) | yes | no |
| `PT_PHDR` | no | yes | yes | no (GNU ld; lld and mold write one; layout L-67 "iff `PT_INTERP`") |
| `.dynamic`, `PT_DYNAMIC`, `_DYNAMIC` | no | yes | yes | yes |
| `DT_NEEDED`, versions | no | yes | yes | no (a shared input is refused, input §8.4) |
| `.dynsym` | none | D-21 | D-21 | the null entry only: `--export-dynamic` and `--export-dynamic-symbol` are accepted with no effect (WP-056 measured exports from a static-pie useless; §13.1) |
| `DT_DEBUG` | — | yes | yes | yes (all three) |
| `DT_FLAGS_1` | — | `DF_1_NOW` under `-z now` | `DF_1_PIE` (+ `DF_1_NOW`) | `DF_1_PIE` (+ `DF_1_NOW` under `-z now`: all three references; WP-052) |
| `RELATIVE` | never | never (absolute values are final) | every absolute word (GOT slots included) of a non-preemptible target that is an address of the executable; never one of an absolute symbol (`SHN_ABS`: the word holds the value, D-46 row `Abs`; GNU ld's AArch64 record there is A30, §13.2) | same as `Pie` |
| `COPY` | never | shared data reached by a non-GOT class | same, x86-64 GCC code (D-43) | never |
| canonical PLT | never | shared function whose address a non-GOT class takes | never (refused by name, D-44) | never |
| `IRELATIVE` | `.rela.plt`, bracketed by `__rela_iplt_*` (synth S-69) | last in `.rela.plt` (D-41) | last in `.rela.plt` | `.rela.dyn` (synth S-70) |
| text relocation | n/a | refused (D-47) | refused | refused |
| TLS against a shared definition | n/a | IE (D-48) | IE | n/a |
| verifier (plumb) | stage 1-2 | stage 3 (§12) | stage 3 | stage 3 |

*Test:* `output-kind-table`: one hello per column and architecture,
`readelf -hld` against the row.

**D-2 [M2] (static-pie is not a dynamic link).** `StaticPie` has a
`.dynamic` for the self-relocation of glibc's `_dl_relocate_static_pie`,
and nothing a loader reads: rules of this spec that say "dynamic
executable" mean `Exec` and `Pie`. The end-to-end static-pie gate the
ROADMAP gives M2 (x64.md X-9, X-56; x86.md R-15) needs only D-30's
`.dynamic` subset (`DT_RELA*`, `DT_RELACOUNT`, `DT_FLAGS_1`, `DT_DEBUG`,
the empty `.dynsym`/`.dynstr`/`.gnu.hash` triplet lld writes) and §8's
`RELATIVE` rules; it is cut as its own package (WP-M2-SPIE, §D of the
record).
## 5. Shared objects as inputs

Most of this is specified (input.md §8, resolve.md §9); E-shlib (116
experiments × 4 linkers, AArch64 and x86-64, every output run) confirmed
it rule by rule. The rules below fill the gaps, settle the two
contradictions and record what was measured. lld 19 and 22 agreed in
every experiment.

**D-10 [M2] (what is read; confirmed).** input §8.1 and resolve §9.1-§9.3
stand. Measured: one file per soname, a later file with that soname
ignored (E-shlib 1.5); the needed name rules of input §8.3 (1.2a-e);
`STB_GNU_UNIQUE` binds like `STB_GLOBAL` (5.9); a reference to `foo`
binds `foo@@V2`, `.symver` `foo@V1` binds the hidden `foo@V1`, a missing
version is undefined, a hidden-only version never binds an unversioned
reference (5.8a-d); an IFUNC defined in a shared object is an ordinary
function definition, address equality holding in PIE and non-PIE (5.10).
A `STV_HIDDEN` entry in a shared object's `.dynsym` is ignored (resolve
§9.1 L1112): all three references bind to it and their outputs fail to
load (`symbol lookup error: undefined symbol: vf`, 5.7, a wrong output
of all three; `patch_vis.py`); mortar reports the reference undefined.
*Test:* `shlib-hidden-dynsym` (the patched library; mortar refuses with
`undefined-symbol`, never writes an output that cannot load).

**D-11-D-13 setting.** Revised after the review (F1): `ld-linux` is
loaded by `libc.so`'s `AS_NEEDED(…)`, is not needed by a hello, and
defines the 22 (AArch64) / 21 (x86-64) strong references of `libc.so.6`
that nothing else defines (`__tls_get_addr`, `_rtld_global_ro`,
`__libc_stack_end`, `_dl_argv@GLIBC_PRIVATE`, …;
`review-m2-adv/ldso`, reproduced); the sets below make it *used*, so
every default line links. Notation:

- **C**: the shared objects of the command line and scripts, one per
  soname, each with its merged `as_needed` (input §8.2: on only if on at
  every mention) and the `FileId` of its first mention. **W**: the closure
  files of the `DT_NEEDED` walk (input §8.6). **Loaded** = C ∪ W.
- **Edges**: each `DT_NEEDED` entry of a loaded object leads to the
  loaded object with that soname, *whatever loaded it* (an as-needed
  member of C that is not needed satisfies the edge and stops the
  search: GNU ld, E-shlib 2.3a). `reach(S)` is `S` and every loaded object
  reachable from `S` along edges.
- **giver(k)**: none when the program defines `k` (strong or weak; a
  common gives way to a shared data definition, resolve §3.2 rule 2);
  otherwise the first member of C in `FileId` order with a visible
  definition of `k` at the version `k` asks for (resolve §9.2). Closure
  files never give `def` (D-12). `giver` decides **which objects are
  needed** and nothing else.
- **Loader order**: ld.so's breadth-first search, the executable first,
  then the members of N in `DT_NEEDED` order (D-19a), then the members of
  U in breadth-first order (each visited object's own `DT_NEEDED` entries
  in their order, an object counted at its first visit).
- **bind(k)** (review N1): none when the program defines `k`; otherwise
  the first member of C in N ∪ U, in loader order, with a visible
  definition of `k` at the version `k` asks for. **Every fact of a shared
  definition is taken from `bind(k)`**: the import's provider, the size
  and alignment of a `COPY` (D-42), the version and `Verneed` (D-27),
  `STV_PROTECTED` (D-18), `STT_GNU_IFUNC` and `STT_TLS` (D-16, D-23). For
  a key with a strong program reference `giver(k)` ∈ N (D-11 item 2), so
  `bind(k)` ∈ N, but not always `bind(k) = giver(k)`. A definition that
  lives only in a used object (U, not N) is bound at that object's place
  in the breadth-first part of the order: by a weak program reference
  (D-25 then does not apply: the key is defined) and by the shared
  objects' own references (D-13 asks existence there, and the loader
  binds them in the same order at run time).

**D-11 [M2] (needed).** N is the least set such that

1. every member of C whose merged `as_needed` is off is in N;
2. `giver(k)` is in N for every key `k` with a strong reference from a
   live relocatable object (any relocation or symbol-table mention,
   whether or not its section survives `--gc-sections`: needed is
   decided by resolution, before liveness; GNU ld and mold agree, lld
   drops the library, E-shlib 2.6a) or from file 0 (`-u`, `--defsym`,
   `ENTRY`); call the sets of items 1-2 **N0**;
3. if `x` ∈ N has a strong undefined reference to `k`, `s = giver(k)` is
   in C, and `s` ∉ `reach(N0) ∪ reach({x})`, then `s` ∈ N (an under-linked
   library: GNU ld makes the provider needed and the program runs; lld
   and mold drop it and the program fails with `undefined symbol:
   mid_fn`, a wrong output of both, E-shlib 2.3b).

Computed as `N₀ = N0`, `Nᵢ₊₁ = Nᵢ ∪ {s by item 3 for x ∈ Nᵢ}`: item 3
depends on N only through "`x` ∈ N", so the iteration is monotone and
reaches the least fixed point in at most |C| rounds, after `def`
(resolve §12). **Neededness uses first mention, binding uses D-19a
order**: N reads `giver` only, D-19a's order is computed from N, and
`bind` from that order, so the fixed point never depends on the binding
and the binding never feeds back. When the two differ, the program binds
to `bind(k)` and `giver(k)` is still needed. GNU ld binds in D-19a order
and decides neededness by its binding (an as-needed library that nothing
has referenced when it is read is unloaded, and a later library's
definition takes the key), so it then records one entry fewer.
`shlib/dupdef_needed.sh` (AArch64 natively `-no-pie`, x86-64 PIE under
qemu; `libnA` defines `int dv[4]` = 1s, `libnB` `int dv[8]` = 2s; `m.o`
prints `dv[0]`, `dv[7]`, a `COPY`):

| line | GNU ld | lld, mold | mortar |
|---|---|---|---|
| 1 `--as-needed -lnA --no-as-needed -lnB m.o --as-needed -lnA` | `nB`; copy 32 from `nB`; `2 2` | `nA nB`; copy 16 from `nA`; `1 0` | `nB nA` (`nA`'s keeping mention is the third, `m.o` before it); `bind(dv)` = `nB`, copy 32 |
| 2 `m.o --as-needed -lnA --no-as-needed -lnB` | `nA nB`; copy 16 from `nA`; `1 0` | the same | the same |
| 3 `--as-needed -lnA --no-as-needed -lnB m.o -lnA` (the review's `dupdef`) | `nB nA`; copy 32 from `nB`; `2 2` | `nA nB`; copy 16; `1 0` | GNU ld's image |

mortar's image of line 1 is GNU ld's image of line 3 (same `DT_NEEDED`
list, copy and binding; it runs on both architectures) and differs from
GNU ld's line 1 only by the unused `nA` entry: a deliberate superset.
The review's `dupdef` image, which took the binding from `giver` and the
order from D-19a, reads `nB`'s 32 bytes through a 16-byte copy (glibc's
size warning, `fb=8`); `bind` removes it, and P-D9 reports it (§12). Never a reason to be needed: a weak reference alone (GNU
ld and lld; mold differs, 2.2a; the key's view is then
`Undefined{weak: true}` and D-25 applies), a reference from a used
object (D-12) or a closure file, a reference from an as-needed object
outside N, and a weak definition that a strong object definition beats
(2.9). Position does not matter (resolve §9.5): GNU ld refuses
`--as-needed liba.so m.o` (2.8a), mortar links it (D-19a gives the
order). Both conditions of item 3 are needed: without `reach(N0)` a
provider already reached from the command line's needed set is added
twice-over (`libleaf.so` in 2.3b would be recorded); without
`reach({x})` a provider `x` itself lists is recorded (`libmid.so`, X2);
GNU ld's own test, "`s` ∉ `reach(N)`", gives the same verdicts on the
corpus but is not monotone. A used object's references never make an
object needed (GNU ld refuses X3: ``libmid2.so: undefined reference to
`leaf_fn'``; letting `x` range over used objects would link it, and that
output runs, X3c; mortar takes GNU ld's rule). A hello needs
`libc.so.6` alone (1.7); `ld-linux` becomes needed only when the
program's own reference binds a symbol only it defines
(`__tls_get_addr`, 1.8a). *Tests:* `as-needed-weak`,
`as-needed-underlinked` (runs), `as-needed-gc`, `as-needed-merge`,
`needed-ldso-tls`, `used-underlinks-refused` (X3), `needed-order-dup-def`
(the three lines above and the review's `dupdef`: `DT_NEEDED`, copy size
and run output; lines 2-3 equal to GNU ld's, line 1 to GNU ld's line 3).

**D-12 [M2] (used objects and closure files).** **U = reach(N) \ N**,
computed once after D-11 (so the two never recurse into each other): a
loaded object reached by a `DT_NEEDED` edge from a needed or used
object, whatever loaded it. On the default lines U is `ld-linux` (C),
`libm` and `ld-linux` (C++), `libm`, `libgcc_s`, `ld-linux` (Fortran on
AArch64) (`shlib/usedset.sh`). `file_live`: N → `Shared{needed: true}`,
U → `Shared{used: true}`, every other loaded object → `Dead` (request
R-M1). Then:

- a used object is never `DT_NEEDED` (input §8.6);
- a definition that exists only in a closure file (W, never in C) is
  never a candidate for a key referenced by a relocatable object or file
  0: such a reference is undefined, with the note `defined in <c>, which
  is not on the command line` (GNU ld: ``undefined reference to symbol
  'mid_fn'`` and `DSO missing from command line`; lld and mold:
  `undefined symbol: mid_fn`; E-shlib 2.10a). A member of C that is used
  but not needed gives `def` like any other member of C (if a program
  strong reference binds it, it is needed by item 2); which definition a
  key binds to is `bind(k)`, in loader order;
- definitions of used objects satisfy the shlib-undefined check (D-13);
- references (strong or weak) and definitions of members of N ∪ U set
  `ref_dynamic` and `defined_dynamic`: a program symbol referenced only
  by a used object is exported (GNU ld; lld and mold do not export and
  the program fails with `symbol lookup error: … undefined symbol: hook`,
  W8, E-shlib 4.2a). Objects outside N ∪ U count for neither (an
  executable definition that an unneeded as-needed library also defines
  is not exported: GNU ld and mold; lld exports it, harmless, 2.5a);
  these columns are computed after D-11 and D-12 (request R-R3).

Archive extraction by the references of shared objects (Q1, measured by
WP-043; resolve §4.4). Measured with GNU ld 2.44, lld 22 and mold 2.37.1,
AArch64 and x86-64 alike (`libu.so` refers to `g`; `libg.a(g.o)` defines
`g` and has a constructor; `m1.o` calls `libu.so`, `m0.o` does not):

| line | GNU ld | lld | mold |
|---|---|---|---|
| `m0.o libu.so libg.a`, `m1.o libu.so libg.a`, `m1.o --as-needed libu.so libg.a` | `g.o` extracted, exported | the same | the same |
| `m0.o libg.a libu.so`, `m1.o libg.a libu.so`, `m1.o --as-needed libg.a libu.so` | ``./libu.so: undefined reference to `g'`` | `g.o` extracted | `g.o` extracted |
| `m0.o --as-needed libu.so libg.a`, `m0.o --as-needed libg.a libu.so` (`libu.so` not needed) | not extracted, `libu.so` not needed | `g.o` extracted (its constructor runs, `g` exported) | not extracted |
| the same with `g.o` calling `u2` of `libu.so` | not extracted, not needed | extracted, `libu.so` needed | not extracted |

GNU ld and mold do not let an unneeded as-needed object extract; whether
a needed one extracts depends on its neededness, which is known only
after `def` (D-11). The rule, decided at load time:

- the undefined references of a member of C whose merged `as_needed`
  (D-11 item 1) is off are edges of resolve §4.1 like an object's (such
  an object is always needed, so its references are the program's);
  where the archive precedes the object, mortar extracts as lld and mold
  do (resolve §4.3; GNU ld refuses the line);
- the undefined references of a member of C whose merged `as_needed` is
  on, and of every closure file (W), are never edges: they extract
  nothing (GNU ld and mold when the object is not needed; lld differs);
- **`as-needed extraction`, refused by name**: after D-11, a member `s`
  of N whose merged `as_needed` is on and that has a strong undefined
  reference to a key `k` whose edge, were `s`'s references edges, would
  target a lazy file that is not live (resolve §4.1 `first(k)`, counting
  `s` as a live file), stops the link: `refused` (class `feature`, name
  ``archive member needed by an --as-needed shared object: `k' ``),
  phase 30, the unit and place of D-13, with the note `link the library
  with --no-as-needed, or name the member's definition earlier (an object
  or -u)`. All three references link such a line (`m1.o --as-needed libu.so
  libg.a`) and extract the member; mortar refuses it rather than decide
  extraction from a neededness that depends on it. An extension that
  lifts the refusal (extract in a second stage and refuse only when that
  changes N) is a follow-up, not M2.

The rule is the same for needed and unneeded objects at load time (the
references of an as-needed object never extract), so it is not
circular; neededness is computed after `def` (D-11).
*Tests:* `closure-export` (runs), `closure-not-on-command-line` (the
note), `closure-unused`, `shlib-undefined-ldso` (the default hello with
D-13 on: links, `ld-linux` used, not needed).

**D-13 [M2] (shared objects' undefined references).** In `Exec` and `Pie`
the check of resolve §5.2 is on by default (`--no-allow-shlib-undefined`;
GNU ld and lld; mold never checks, E-shlib 3.1a);
`--allow-shlib-undefined` turns it off. It **judges** every strong
undefined reference of every member of N ∪ U (GNU ld judges used objects:
X1, X3, 3.4a); objects outside N ∪ U, and what only they reach, are not
judged (an as-needed object that is not needed: GNU ld; lld judges it and
refuses a link that runs, 3.6). A judged reference is **satisfied** by a
non-hidden program definition (any version) or by a visible definition
in a member of N ∪ U with the version it requires (D-10). When an edge's
library is **not found** (`needed-not-found`, input §8.6), the
references that only it would satisfy are undefined: GNU ld's rule (3.5,
4.1); lld exempts such an object. This **replaces** resolve §5.2 L769's
exemption (request R-R1): the conservative rule is the one the default
`gcc` line already applies, and `--allow-shlib-undefined` or
`-rpath-link` resolves it. Message: `undefined-symbol` with the note of
resolve §5.3 and the needing file as the place (GNU ld:
``d/libx.so: undefined reference to `nobody'``). Model check
(`shlib/usedset.py`, an independent implementation of D-11-D-13 run on
the 116 E-shlib experiments, X1-X3c and the default gcc, g++ and gfortran
lines, AArch64 and x86-64: 254 links): 236 agree with GNU ld on verdict,
`DT_NEEDED` set and export; the 18 that differ are deliberate (2.8a
position, D-11; 2.10b, D-15; 4.4c/f/g native `LD_RUN_PATH`/
`LD_LIBRARY_PATH`, D-19; 4.4h/i the search of input §8.6; 5.6a/b/d
protected, D-18; 5.7 hidden entry, D-10). Every default line links,
judging 22/21 (C), 240/234 (C++) and 292/319 (Fortran) references, all
satisfied. *Tests:* `shlib-undefined`, `shlib-undefined-closure`,
`shlib-undefined-dropped`, `shlib-undefined-missing-closure`,
`shlib-undefined-ldso`, `used-underlinks-refused` (X3; lld and mold
link it, drop `libleaf.so` and the program fails at load, W16).

**D-14 [M2] (versions read, versions not checked).** From a shared object
mortar reads its `versym`, its version definitions (names, `VER_FLG_BASE`
for the base entry, the hash it writes into a need, D-27) and, of its
version needs, only the name each undefined entry requires (the version a
judged reference must find, D-13: with a strong `dep_fn@DEP_2` GNU ld and
lld refuse ``libvn.so: undefined reference to `dep_fn@DEP_2'``, and so
does mortar). A `Vernaux` that no judged reference requires is not
checked: a shared object's own needs of its dependencies are the
loader's to check (GNU ld, lld and mold do not check them; glibc does, at
load). `VER_FLG_WEAK` on an input definition has no meaning to the link.
*Test:* `version-needs-unchecked` (a library needing a version its
dependency lacks links with every reference and mortar).

**D-15 [M2] (`--copy-dt-needed-entries` refused).** Only GNU ld accepts it
(lld: `unknown argument`, mold: `unknown command line option`, E-shlib
2.10b), no census line passes it (§2), and GNU ld appends the closure
library after `libc.so.6`. It stays refused by name; input §8.6 L928-934
and input §15 L1645 ("supported (M2)") are withdrawn (request R-I1);
`--no-copy-dt-needed-entries` is the default and becomes `noop` with that
reason. *Test:* `copy-dt-needed-refused`.

**D-16 [M2] (TLS against a shared definition).** A TLS class relocation
against a key whose `def` is a non-`STT_TLS` shared definition, or a
non-TLS class against an `STT_TLS` shared definition, is `tls-mismatch`
(resolve §3.4 extended to shared definitions, request R-R4). The judged
mentions are those of live relocatable objects; the diagnostic is at the
first such mention of the other kind by `≺`, with `bind(k)`'s entry as
the first definition (text `TLS attribute mismatch: <name>`, diag.md
§3.1). GNU ld refuses both directions (measured by WP-051: ``tv: TLS
definition in ./libtv.so section .tdata mismatches non-TLS reference in
data.o``; ``nv: TLS reference in tls.o mismatches non-TLS definition in
./libnv.so section .data``); lld and mold link and the programs crash or
read garbage (W14, §13.2). *Tests:* `tls-mismatch-shared`,
`copy-tls-refused` (D-42).

**D-17 [M2] (common against a shared definition).** resolve §3.2 rule 2
stands: GNU ld and mold give the shared data definition (`5 5`), lld the
common (`0 0`), E-shlib 5.4.

**D-18 [M2] (protected definitions).** A `COPY` or `CANON_PLT` demand on a
key whose shared definition is `STV_PROTECTED` is refused by name,
`copy-relocation-against-protected` (scan, phase 70): all three refuse
the copy (GNU ld: ``copy relocation against non-copyable protected symbol
`pv' in d/libprot.so``; lld: `cannot preempt symbol: pv`; mold: `cannot
create a copy relocation for protected symbol 'pv'; recompile with
-fPIC`), and GNU ld and lld refuse the canonical PLT (GNU ld x86-64:
``non-canonical reference to canonical protected function `pf'``; lld
`cannot preempt symbol: pf`); mold links it and the program breaks
pointer equality (prints `9 0` and glibc warns ``direct reference to
protected function``), a wrong output of mold, E-shlib 5.6d. A PIE that
reaches a protected datum through the GOT needs neither and links
(AArch64, 5.6a); on x86-64 GCC's `-fPIE` reads extern data PC-relatively
(D-43), which needs a copy in a PIE too, and all three refuse it as above
(WP-051). GNU ld's AArch64 refusal of the canonical PLT is its copy
message (``copy relocation against non-copyable protected symbol
`pf'``). *Tests:* `protected-copy`, `protected-canon-plt`.

**D-19 [M2] (input forms; confirmed).** `-Bstatic` before a shared object
named by path is `static-link-of-shared-object` (GNU ld and lld; mold
links it dynamically, 6.1f); an empty `.so` is ignored (input §2.3; GNU
ld and lld; mold refuses, 6.5); a non-ELF text `.so` is read as a script
and fails there; `.so` before `.a` within one directory, first directory
wins (6.1). `LD_LIBRARY_PATH` and `LD_RUN_PATH`, which a *native* GNU ld
searches for closure libraries ahead of `DT_RUNPATH` (and the cross GNU
ld does not, 4.4), are never read (input §4.7): a native GNU ld link that
finds a closure library only through them fails in mortar with
`needed-not-found` and the D-13 consequence, which the note names.

**D-19a [M2] (`DT_NEEDED` order: GNU ld's).** `DT_NEEDED` order is the
loader's search order and the reverse of the constructor order (every run
of `dynamic/needed_order.sh`, both architectures), so mortar takes the
order of the reference of record, GNU ld. Definitions:

- The **mentions** are the shared-object entries of the command line
  with scripts expanded inline, depth first, at the script's own
  position (`INPUT`, `GROUP`, nested scripts, `-l` inside scripts); a
  mention's as-needed state is the one in force where it (or its
  enclosing script) is named, `AS_NEEDED(…)` forcing it on;
  `--push-state`/`--pop-state` restore it. Two mentions are of one
  object when they have one soname (or, without a soname, one name as
  written: `libnz.so` and `./libnz.so` are two entries in all three).
- `Slots.needed` is the set D-11 computes (which does not depend on
  positions). Each needed object `D` is placed at its **keeping
  mention**: the first mention `m` of `D` such that `m` is not as-needed,
  or `m` is as-needed and some reason of D-11 for `D` is a strong
  reference from a relocatable object at a position before `m` (or
  anywhere in the same `--start-group`/`GROUP` as `m`), or from a needed
  shared object whose own keeping mention is before `m` (D-11 item 3). If no mention qualifies (a line GNU ld
  refuses, below), the keeping mention is the first mention.
- `DT_NEEDED` lists the needed objects by the position of their keeping
  mentions (position, not the time GNU ld decided: a group rescan that
  keeps `nc` after `nd` still writes `nc` first, `grp_rescan_time_vs_pos`).

This is a total function of the command line and the symbol tables,
computed after D-11's fixed point by walking the mentions once in order.
It reproduces GNU ld on all 75 cases of `dynamic/needed_order.sh` that
GNU ld links, on AArch64 and x86-64 (identical there); for example
`na --as-needed nx --no-as-needed nb nx nc` with `nx` unreferenced gives
`na nb nx nc` (lld: `na nx nb nc`; mold drops `nx`, W9), and the
driver's default `--as-needed` with `nd na main.o nc nb nf ne na nd`
gives `nc nb nf ne na nd` (lld and mold: `nd na nc nb nf ne`). GNU ld
refuses lines where an as-needed library is referenced only by objects
(or by a needed shared object kept) after its last mention
(`obj_after_asn`, `drv_obj_after_all`: undefined reference); D-11 does
not depend on position, so mortar links them, each such library at its
first mention (the fallback above). That superset of GNU ld rests on an
executed image, not on lld's order: in `underlink-after`
(`mp.o --as-needed -lnq --no-as-needed -lnp -lnc`, `libnp` under-linked
against `libnq`) GNU ld refuses, lld and mold drop `nq` and their outputs
fail at load (`symbol lookup error: … fn_nq`, rc 127), and mortar's image
(`nq` needed at its first mention: `nq np nc`) runs; the review executed
it from a GNU ld link with `nq` forced
(`/Volumes/AgentWork/mortar-helper/.build/review-m2-adv/neededorder/run.sh`). lld and mold differ (§13.1): one entry per soname at its first
mention, needed if any mention is plain or any object anywhere
references it. *Tests:* `needed-order` (the 75 cases, run-order equal to
GNU ld's output; and `underlink-after`, which must run), `needed-order-as-needed-twice` (the review's F5 line,
constructor order compared), `needed-order-group`.
## 6. `.dynsym`, `.dynstr`, hash tables, versions

Owner: `mortar_synth` (synth.md S-101..S-103 are the outline these rules
complete; request R-S1 replaces them by citations of this section).
Measured in E-dynsym: 438 links, three architectures, PIE and non-PIE,
every hash table looked up name by name by `dyninfo.py`; the formulas of
each reference fitted on 1200 links.

**D-20 [M2] (hash style).** An executable gets `.gnu.hash` and no `.hash`.
`--hash-style=gnu` is supported; `sysv`, `both` and `none` are refused as
an option value (`unsupported-option-value`), which is what synth S-102
meant and what cli §2.3 and the options row must say (request R-C1:
`--hash-style` becomes `partial`). Every census driver passes
`--hash-style=gnu`, i686 included (§2.3); with no option GNU ld (Debian's
build) writes `.gnu.hash` only, lld and mold write both (E-dynsym,
`DEFAULT-HASH`), and mortar's default is GNU ld's. glibc since 2.5 reads
`DT_GNU_HASH`; a loader that needs `DT_HASH` is out of scope. *Test:*
`hash-style-sysv-refused`.

**D-21 [M2] (which symbols).** `.dynsym` of `Exec` and `Pie` holds, besides
the null entry:

1. **imports**: every key whose `def` is `Shared` (from a needed shared
   object) and which a live relocatable object mentions (`ref_regular`);
2. **exports**: every key `exported` (resolve §9.6, D-12), except
   linker-defined keys of file 0, which are exported only when
   `ref_dynamic` (lld and mold export none of `__bss_start`, `_edata`,
   `_end`, `__bss_end__`… under `-rdynamic`, GNU ld exports them; the
   set of linker-defined names is each linker's own, so exporting them
   would make the dynamic interface an artefact of the implementation;
   request R-R5 to resolve §9.6);
3. **copy aliases**: every key whose shared definition lies at the address
   of a copy allocation (`Slots` copy allocations, model §4.3), whether
   or not the program references it. All three export the referenced name;
   lld and mold export every alias (`environ`, `__environ`, `_environ`);
   GNU ld exports only one alias besides, and a library reading
   `_environ` then sees libc's stale variable after `setenv` (E-dynsym
   `envalias`, `lib&_environ=0`): a wrong output of GNU ld;
4. **dynamic weak undefined**: the `WeakDyn` keys of D-25.

Never: local or hidden symbols, section symbols (GNU ld writes two
`STT_SECTION` entries in an AArch64 PIE; lld and mold none), `_DYNAMIC`,
`_GLOBAL_OFFSET_TABLE_`, `__dso_handle`. The set equals lld's on the
corpus but in one place: in an x86-64 `-no-pie` link crtbegin.o reaches
`_ITM_deregisterTMCloneTable` and `_ITM_registerTMCloneTable` by
`R_X86_64_32` only, an unfillable use, so D-25 makes them `Weak0` and they
stay out (GNU ld's set), while lld exports them (WP-051). *Tests:*
`dynsym-set` (the whole set, both architectures), `environ-aliases`
(runs; the library sees `setenv`).

**D-22 [M2] (order).** Entry 0 is null. Then the **unhashed** entries (the
undefined entries whose `st_value` is 0) in `SymbolId` order; then the
**hashed** entries (every defined entry and every undefined entry with a
non-zero `st_value`, i.e. a canonical PLT) sorted by (bucket,
`SymbolId`), `bucket = gnu_hash(name) mod nbuckets`. `sh_info` is 1.
Hashing the canonical-PLT entries is required: glibc resolves a library's
`R_*_GLOB_DAT`/absolute reference to `puts` to the executable's PLT entry
only through the hash table, and all three hash them (`ptreq`:
`exp-vs-lib &puts equal=1` with every reference). Unhashed undefined
entries are never found by a lookup (GNU ld hashes an x86-64
`__cxa_finalize` with value 0; harmless, glibc skips value 0). All three
place the unhashed block first; their orders inside it are a hash order
(GNU ld), first appearance (lld) and the library's index (mold);
`SymbolId` order is a function of the inputs. `Slots.dynsym` stores
this order (model §4.3). *Test:* `dynsym-order` (stable under input
permutations that keep `SymbolId`s; `--threads=1` vs many).

**D-23 [M2] (entry fields).**

| entry | `st_info` | `st_other` | `st_shndx` | `st_value` | `st_size` |
|---|---|---|---|---|---|
| import, called or through the GOT | binding `STB_WEAK` iff `!ref_strong`, else `STB_GLOBAL`; type: `STT_FUNC` when the definition is `STT_FUNC` or `STT_GNU_IFUNC`, else the definition's type | `STV_DEFAULT` (plus `STO_AARCH64_VARIANT_PCS` when the definition has it; GNU ld, lld and mold keep it on a GOT-only import too, WP-053) | `SHN_UNDEF` | 0 | 0 |
| import with `CANON_PLT` | as above, `STT_FUNC` | `STV_DEFAULT` (plus `STO_AARCH64_VARIANT_PCS` when the definition has it) | `SHN_UNDEF` | the PLT entry's address | 0 |
| copy allocation (each alias) | binding `STB_WEAK` iff the shared definition is weak, else `STB_GLOBAL`; the definition's type | `STV_DEFAULT` | the copy's output section | the copy's address | the definition's `st_size` |
| export | the output binding and type of `.symtab` (an exported non-preemptible IFUNC: `STT_FUNC` at its IPLT entry, a64 §10.4) | the merged visibility (`STV_PROTECTED` kept), plus `STO_AARCH64_VARIANT_PCS` when the defining entry has it (GNU ld, lld and mold, WP-053) | its output section | its address | its size |
| `WeakDyn` | `STB_WEAK`, `STT_NOTYPE` | `STV_DEFAULT` (plus `STO_AARCH64_VARIANT_PCS` when a live reference has it, resolve §3.7 `RefFlags::VARIANT_PCS`: the loader reads the bit of the `JUMP_SLOT`'s symbol, so a lazy binding of a vector-PCS callee keeps its registers only if it is there; GNU ld and lld, WP-059) | `SHN_UNDEF` | 0 | 0 |

The canonical PLT is `SHN_UNDEF` with the entry's address in all three
references, on every architecture (E-dynsym §2, E-reloc §5); a64.md §10.5
L1162 ("`st_shndx` = the PLT's section") is wrong and is corrected
(request R-A1): a defined entry would make glibc bind the executable's own
`JUMP_SLOT` to its own PLT entry. An `STB_GNU_UNIQUE` shared definition
gives a copy `STB_GLOBAL` (the binding rule above), so a copy is never
unique in `.dynsym`; an exported unique definition of the executable
stays `STB_GNU_UNIQUE` there (the export row; GNU ld, lld and mold;
glibc's unique table then holds the executable's instance, which
`RTLD_DEEPBIND` libraries bind to; WP-054, WP-082's `unique` fixture).
`EI_OSABI` follows the arch-independent rule already ruled for static
links (a64.md Q11, x64 X-49): `ELFOSABI_GNU` when `.symtab` or `.dynsym`
of the output holds an `STT_GNU_IFUNC` or `STB_GNU_UNIQUE` symbol or the
output has an `IRELATIVE` record, else `ELFOSABI_NONE`. An executable's
own unique definitions (GCC template statics, function-local statics of
inline functions) stay `STB_GNU_UNIQUE` in `.symtab`, so a C++ PIE is
usually `ELFOSABI_GNU`; that is resolve.md §3.7's rule, the same for a
static output (WP-059 B: before it a static `.symtab` wrote them
`STB_GLOBAL`). Only GNU ld also writes `EI_OSABI` 3; lld and
mold write 0 (`review-m2-adv/unique`, both architectures; glibc accepts
both). *Test:* `dynsym-fields` (one case per row,
per architecture).

**D-24 [M2] (`.dynstr`).** Byte 0 is NUL; then each distinct string once,
in order of first use: the `.dynsym` names in `.dynsym` order, the
`DT_NEEDED` names in `Slots.needed` order, the version names of
`.gnu.version_r` in its order, `DT_RUNPATH`/`DT_RPATH`. No tail merging
(lld and mold; GNU ld tail-merges, e.g. `main` inside
`__libc_start_main`): the size is a function of the set of strings, and
the offsets are a prefix sum the verifier can recompute. *Test:*
`dynstr-layout`.

**D-25 [M2] (undefined weak in a dynamic executable; per symbol).** This
settles the circle of arch.md §2.1 L77 and resolve.md §5.4 L819 (requests
R-A2, R-R2). `target_kind` is a property of the **key**, never of the
relocation class (arch.md §2.1). For an undefined key that is weak (no
definition, or one only in a shared object that is not needed, D-11),
classify each live relocation that names it by this table (review N3),
which covers every type of the M1 and M2 sets of a64.md, x64.md and
x86.md; **a type not listed is unfillable**:

| class | AArch64 (a64 §7.1) | x86-64 (x64 X-57) | i386 (x86 X86-68) |
|---|---|---|---|
| **GOT** | `GOTPCREL32`, `GOT_LD_PREL19`, `LD64_GOTOFF_LO15`, `ADR_GOT_PAGE`, `LD64_GOT_LO12_NC`, `LD64_GOTPAGE_LO15` | `GOTPCREL`, `GOTPCRELX`, `REX_GOTPCRELX`, `GOT64` (relaxation, x64 §8.1, is off for a `WeakDyn` key) | `GOT32`, `GOT32X` (the same) |
| **absolute word in a writable place** | `ABS64` | `64` | `32` |
| **branch** | `CALL26`, `JUMP26`, `PLT32` | `PLT32`, `PLTOFF64`; `PC32` on a branch opcode | `PLT32`; `PC32` on a branch opcode |
| **unfillable** | `ABS64` in a non-writable place; `ABS32`, `ABS16`, `PREL64`/`32`/`16`, `MOVW_UABS_*`, `MOVW_SABS_*`, `MOVW_PREL_*`, `LD_PREL_LO19`, `ADR_PREL_LO21`, `ADR_PREL_PG_HI21[_NC]`, `ADD_ABS_LO12_NC`, `LDST{8,16,32,64,128}_ABS_LO12_NC`, `CONDBR19`, `TSTBR14`, `GOTREL64`, `GOTREL32`, every TLS type | `64` in a non-writable place; `32`, `32S`, `16`, `8`, `PC32` not on a branch opcode, `PC16`, `PC8`, `PC64`, `GOTOFF64`, every TLS type | `32` in a non-writable place (code); `16`, `8`, `PC32` not on a branch opcode, `PC16`, `PC8`, `GOTOFF`, every TLS type |
| (no use of the key) | `NONE` | `NONE`, `SIZE32`, `SIZE64` (the value is `Z + A`, 0), `GOTPC32`, `GOTPC64` (they name `_GLOBAL_OFFSET_TABLE_`) | `NONE`, `SIZE32`, `GOTPC` (the same) |

- A **writable place** is a place whose output section has `SHF_WRITE`
  after layout.md's section rules (known from the section mapping,
  before addresses); RELRO sections count as writable (the loader writes
  them before `mprotect`), `.text` and `.rodata` do not. The input
  section's own flags never decide: a read-only input section in an
  output section that another input makes writable (layout L-50, the OR
  of the inputs' flags) is a writable place, and its words get records
  (D-46). Before WP-056 mortar judged the input section (a refusal where
  this rule allows a record; WP-083 finding 39); test
  `writable-place-output`. GNU ld and mold judge the output section
  (GNU ld then writes the writable `.rodata` into an RWX segment with no
  `PT_GNU_RELRO`, mold sets `DT_TEXTREL`); lld judges the input section
  and refuses the PIE (§13.1).
- A **branch opcode** (x86-64 and i386 `PC32`, whose type alone does not
  say): the bytes before the place are `E8` (call) or `E9` (jmp) at
  offset −1, or `0F 80`…`0F 8F` (jcc rel32) at offset −2, inside the same
  input section; anything else, a `PC32` at the start of its section
  included, is not on a branch.
- AArch64 `CONDBR19` and `TSTBR14` are unfillable although they branch:
  no reference routes a conditional branch through a PLT, and a ±1 MiB
  / ±32 KiB range is not a guarantee a PLT entry meets.

Then:

- the key is **`WeakDyn`** in `Exec` and `Pie` when it has some GOT use or
  absolute word in a writable place, and no unfillable use. It is in `.dynsym` (`WEAK UND`,
  unhashed, D-22/D-23); its GOT slots get `GLOB_DAT`, its absolute words
  a symbolic record, and **every branch against it goes through a PLT
  entry** with a `JUMP_SLOT` (lld's form; GNU ld uses `.plt.got` on
  x86-64; D-49 has no `.plt.got`). The GOT test and the call then agree
  whether or not a library loaded later (`LD_PRELOAD`, `dlopen` with
  `RTLD_GLOBAL`) defines it;
- in `Exec` and `Pie`, a key with a TLS-class use (a thread-local
  undefined weak) is refused by name at M2, once per key (at its first
  TLS relocation in input order), `Refused{Feature, "thread-local undefined weak symbol in a dynamic executable"}`: every TLS type is
  unfillable, and the dynamic case is unmeasured (a64.md §7's
  thread-local `weak0` rules are static links'; supervisor's ruling
  after WP-040r);
- in `Pie` only, a key with an unfillable use is refused by name, once
  per key (at its first unfillable relocation in input order),
  `weak-undefined-pc-in-pie`, with ``relocation R_X86_64_PC32 against
  undefined weak symbol `v' cannot be used in a position-independent
  executable; recompile with -fPIE``. Measured (`reloc/weakpc.sh`: GCC 14,
  Clang 19, Clang 22 × AArch64, x86-64, i386 × default, `-fPIE`, `-fPIC`,
  `-fno-pic`, code models × `-O0`/`-O2` × 8 weak patterns, 1008 objects):
  only x86-64 `-fno-pic` objects (GCC at `-O0`/`-O2`, Clang at `-O2`;
  `-mcmodel=medium` too) produce one, on weak **data** (`movl v(%rip)`),
  and always together with an `R_X86_64_32` for `&v`, which all three
  references refuse in a PIE already; AArch64 (GOT in every mode,
  `-fno-pic` and `-mcmodel=tiny` included) and i386 never do. The
  refusal costs no input that any reference links;
- otherwise the key is **`Weak0`** for every class: in `Exec` when it has
  an unfillable use (its value `S` is then 0 everywhere, consistently: GNU
  ld on x86, mold; each relocation is written by the architecture's
  `weak0` table, a64 §7, x64 X-16, so an AArch64 `PC` instruction takes
  `S = P`: an `ADRP` gets `Page(P + A) − Page(P)` and an `ADRP`+`ADD`
  pair gives the instruction's page, not 0, as GNU ld and lld write,
  WP-051), and in both kinds when it has neither a GOT use nor an
  absolute word in a writable place (a branch alone: `__gmon_start__`
  style calls patched per a64 §7 / x64 X-16).

The references: GNU ld and lld make a GOT-used weak key dynamic in `Pie`
and in `Exec` and run the hook (`review-m2-adv/weaksplit`, reproduced:
GNU ld `GLOB_DAT` + PLT, `.plt.got` on x86-64; lld `GLOB_DAT` +
`JUMP_SLOT`); mold makes it 0 everywhere (the hook never runs under
`LD_PRELOAD`: consistent, but the idiom is lost). The former per-class
split (GOT `WeakDyn`, call `Weak0`) is a mislink: patched into GNU ld's
image it prints "hook non-null, called" and skips `hook` on AArch64 and
dies with `SIGSEGV` on x86-64. GNU ld on AArch64 and lld on all three
architectures make that split themselves in a `-no-pie` link that mixes
a `-fPIE` object (GOT) and an absolute use from `-fno-pic`/`-mcmodel=large`
code: under `LD_PRELOAD` they print `got non-null abs null` (W12;
`reloc/weakpc.sh` stage `weakmix`); the "no unfillable use" condition is
what prevents it. W12 is broader (review N4, `review-m2-adv/weaksplit2`):
in `-no-pie`, with `-fPIE` objects only, a data initializer
(`void (*hp)(void) = hook;`) beside a GOT use is split by GNU ld on
AArch64 **and** x86-64 and by lld (`hp==hook 0` under the preload): the
references write no record for the word. An absolute word in a writable
place alone makes the key dynamic **in a `Pie`**, as lld and GNU ld on
AArch64 do (GNU ld on x86 and mold give 0). **In `Exec`** D-25 makes it
`WeakDyn` too and writes a symbolic record on the word, which no
reference does (all three write no record in `-no-pie`, and `hp` stays
null under `LD_PRELOAD`): this behaviour is new to mortar. It was
executed (`weaksplit2`): lld's `-no-pie` output against a stub
`libst.so` that defines `hook` and `wv`, run against a `libst.so` without
them, has exactly D-25's records (`GLOB_DAT`, `ABS64`/`R_X86_64_64`,
`JUMP_SLOT`, `WEAK UND`) and runs correctly with and without
`LD_PRELOAD` on AArch64 natively and x86-64 under qemu (the review's
F2 check).

`-z nodynamic-undefined-weak` and `-z dynamic-undefined-weak` are refused
by name at M2 (R-C1): no census line passes them; GNU ld honours them on
x86 and ignores them with a warning on AArch64; lld 19 does not know
either (`warning: unknown -z value: dynamic-undefined-weak`, and the same
for `nodynamic-undefined-weak`; review N4, re-run for this revision),
lld 22 and mold accept them, and mold's `-z dynamic-undefined-weak -no-pie` refuses (x86-64,
`_ITM_deregisterTMCloneTable` copy) or asserts (i386). *Tests:*
`weak-undef-pie` (`__gmon_start__`, `_ITM_*`), `weak-undef-exec`,
`weak-undef-hook-preload` (the review's hook program and the call,
address and data patterns; AArch64 natively and x86-64 under qemu
`-E LD_PRELOAD=…`; with and without the preload; must match GNU ld and
lld: `null`/-1 without, `f: RAN`, `&f == dlsym(f)`, data 42 with),
`weak-undef-pc-pie-refused` (x86-64 `-fno-pic` weak data), `weak-mixed-exec`
(W12: `Weak0` everywhere, consistent), `weak-data-init-exec` (W12's
second form, `weaksplit2`: `WeakDyn`, a record on the word, `hp==hook`
under the preload), `weak-class-table` (one object per row and type of
the table above, the branch-opcode cases `E8`/`E9`/`0F 8x`/other and a
`PC32` at offset 0 or 1 of its section, `ABS64`/`64`/`32` in `.data`,
`.data.rel.ro` and `.text`: the class each gets).

**D-26 [M2] (`.gnu.hash`).** For `n` hashed entries (D-22), `W` the word
size in bits:

- `nbuckets = max(1, ⌊n/4⌋)`;
- `bloom_shift = 26`;
- `maskwords` = the least power of two strictly greater than `⌊12n/W⌋`
  (1 when that is 0);
- two Bloom bits per hashed entry (`h mod W` and `(h >> shift) mod W` of
  word `(h / W) mod maskwords`); buckets hold the least `.dynsym` index of
  the bucket, 0 for an empty one; chain words `h & !1`, with bit 0 set on
  the last entry of each bucket;
- `symoffset` = the index of the first hashed entry; with `n = 0`,
  `nbuckets = 1`, bucket 0, one zero mask word, `symoffset` = the
  `.dynsym` count, no chain.

This is lld's choice (fitted on 1200 links, E-dynsym §4). GNU ld uses a
prime below `n` and a size-dependent shift, mold `⌊n/8⌋+1`; all three
produce correct tables (every name found, absent names rejected). lld's
is a closed form of `n`, gives four entries per bucket on average, and
needs no table of primes; GNU ld's empty table has `symoffset = 1`
whatever the count (harmless to glibc). The section's `sh_entsize` is 0
on ELF64 and 4 on ELF32 (all three; elflint complains of the i386 value).
*Correctness:* the verifier looks up every hashed `.dynsym` name and a
fixed set of absent names (`spec/plumb.md` §12, P-D5). *Test:* `gnu-hash-sizes` (n = 0, 1, 2, 3, 10,
100, 1000, 5000: the table looks up every name).

**D-27 [M2] (versions).** `.gnu.version` exists when `.gnu.version_r`
does (a needed object whose definition of some imported key is
versioned); one `u16` per `.dynsym` entry: 0 for the null entry, 1 for
exports, `WeakDyn` and unversioned imports, the need's index for a
versioned import (resolve §9.3); bit 15 never set. `.gnu.version_r`:

- one `Verneed` per needed shared object that provides at least one
  version an import binds to, in `Slots.needed` order (lld; GNU ld uses a
  reverse first-use order, mold sorts by soname);
- each followed directly by its `Vernaux` entries (GNU ld and mold; lld
  writes all headers first, which elflint warns about), in the order of
  the library's own version definitions (lld and mold);
- `vn_file` the `.dynstr` offset of its `DT_NEEDED` name; `vna_hash` the
  ELF hash of the version name (the `vd_hash` the library wrote, D-14);
  `vna_flags` 0 (all three, a weak-only reference included);
  `vna_other` numbered from 2 in output order (mold);
- no `.gnu.version_d` in an executable (no reference writes one without a
  version script, which is M4; a `.symver` `foo@@V` definition that is
  exported is refused, `versioned-export (M4)`: GNU ld writes a verdef
  for it, lld and mold export it unversioned, E-dynsym §5).

`-z pack-relative-relocs` and its `GLIBC_ABI_DT_RELR` need are M4
(§2.3: no default line passes it; GNU ld adds the need only with
`DT_RELR`, lld also without, mold in non-PIE too). *Tests:*
`verneed-order`, `verneed-symver` (`memcpy@GLIBC_2.2.5` on x86-64 runs).

**D-28 [M7 hook] (incremental).** `.dynsym`'s order is a function of the
hash bucket and therefore of `nbuckets`, which changes with the count:
adding one export can move every hashed entry and every `r_sym` of the
dynamic relocations. An incremental link keeps `nbuckets` (a capacity in
`Slots`, model §4.3) while `n` stays within a bound it chooses, so that an
added export moves only its own bucket's entries, and reserves `.dynsym`,
`.dynstr` and `.gnu.version` capacity; nothing in this section prevents
it.
## 7. `.dynamic`, `.interp`, `_DYNAMIC`

Owner: `mortar_synth` (`.dynamic`, `.interp`) with the address-dependent
values filled by `mortar_image`. Measured in E-dynamic (tag order per
reference, presence per condition, loader behaviour).

**D-30 [M2] (tags, order, presence).** `.dynamic` of `Exec` and `Pie` holds
these tags in this order (GNU ld's order: the only references' outputs
that `eu-elflint --gnu-ld` passes clean are GNU ld's, E-dynamic §3, and
`readelf -d` of a mortar output then reads like the default toolchain's);
each is present exactly under its condition, which depends on no address:

| # | tag | present when | value |
|---|---|---|---|
| 1 | `DT_NEEDED` × n | per needed object, `Slots.needed` order (D-19a) | `.dynstr` offset of the needed name |
| 2 | `DT_RUNPATH` (or `DT_RPATH` under `--disable-new-dtags`) | some `-rpath`/`-R` | the paths joined by `:` in order (input §4.8), as written (`$ORIGIN` kept) |
| 3 | `DT_INIT` | `_init` is defined (crti), or `-init=SYM` names a defined symbol | its address |
| 4 | `DT_FINI` | `_fini`, or `-fini=SYM` | its address |
| 5-6 | `DT_PREINIT_ARRAY`, `DT_PREINIT_ARRAYSZ` | the output section `.preinit_array` exists | address, size |
| 7-8 | `DT_INIT_ARRAY`, `DT_INIT_ARRAYSZ` | `.init_array` exists | address, size |
| 9-10 | `DT_FINI_ARRAY`, `DT_FINI_ARRAYSZ` | `.fini_array` exists | address, size |
| 11 | `DT_GNU_HASH` | always | `.gnu.hash` |
| 12-15 | `DT_STRTAB`, `DT_SYMTAB`, `DT_STRSZ`, `DT_SYMENT` | always | `.dynstr`, `.dynsym`, size, 24 (16 on ELF32) |
| 16 | `DT_DEBUG` | always (executables) | 0 |
| 17 | `DT_PLTGOT` | `.got.plt` exists (some PLT or IPLT entry) | `.got.plt` |
| 18-20 | `DT_PLTRELSZ`, `DT_PLTREL`, `DT_JMPREL` | `.rela.plt` is not empty | size, `DT_RELA` (`DT_REL` on i386), address |
| 21-23 | `DT_RELA`, `DT_RELASZ`, `DT_RELAENT` (i386: `DT_REL`, `DT_RELSZ`, `DT_RELENT`) | `.rela.dyn` is not empty (mold; GNU ld and lld also write an empty one) | address, size, 24 (8) |
| 24 | `DT_AARCH64_BTI_PLT` | a64 §10.3 (the output has the BTI property) | 0 |
| 25 | `DT_AARCH64_VARIANT_PCS` | a64 §10.3 (a `JUMP_SLOT` symbol is `STO_AARCH64_VARIANT_PCS`, a `WeakDyn` one included by its D-23 row; all four references, E-dynamic §1) | 0 |
| 26 | `DT_FLAGS` | `-z now` | `DF_BIND_NOW` |
| 27 | `DT_FLAGS_1` | `Pie`, or `-z now` | `DF_1_PIE` if `Pie`, `DF_1_NOW` if `-z now` |
| 28-30 | `DT_VERNEED`, `DT_VERNEEDNUM`, `DT_VERSYM` | `.gnu.version_r` exists (D-27) | address, count, address |
| 31 | `DT_RELACOUNT` (i386 `DT_RELCOUNT`) | some `RELATIVE` record | their count (they lead `.rela.dyn`, D-40) |
| 32 | `DT_NULL` × 6 | always | 0: the terminator and five spare entries (mold; GNU ld's `--spare-dynamic-tags` default of 5 gives 5 or 6, lld 1) for post-link tools and for an incremental link that adds a tag in place (D-28) |

Never written in an executable: `DT_SONAME`, `DT_HASH` (D-20),
`DT_BIND_NOW` (no reference writes it; `DF_BIND_NOW` says the same),
`DT_TEXTREL` and `DF_TEXTREL` (D-47), `DF_STATIC_TLS` (GNU ld and lld
never; mold in every executable, E-reloc §7; it would forbid `dlopen` of
the executable), `DT_AARCH64_PAC_PLT` (`-z pac-plt` stays refused: the
outputs of GNU ld and lld die with `SIGILL` at `autia1716` on this host's
glibc, E-dynamic §5), `DT_X86_64_PLT*` (no reference writes it in an
executable), `DT_TLSDESC_PLT`/`GOT` (TLS descriptors are eager, D-48),
the `prelink` tags `DT_CHECKSUM`, `DT_GNU_PRELINKED`, `DT_GNU_CONFLICT*`,
`DT_GNU_LIBLIST*` (no reference writes them; nothing reads them). The
entry count is therefore a function of the inputs and options, known
before addresses (synth S-2). `-init`, `-fini` and `--spare-dynamic-tags`
are refused today: `-init`/`-fini` become supported with rows 3-4 (all
three honour them, E-dynamic §1), `--spare-dynamic-tags` stays refused
(request R-C1). *Tests:* `dynamic-tags` (the table per condition, each
architecture), `init-fini-option`, `variant-pcs-plt`.

**D-31 [M2] (interpreter).** `Exec` and `Pie` need `-dynamic-linker`; a
dynamic output without one is refused (input §8.4 stands, with its
diagnostic; synth S-106's "or the target's default" is withdrawn, request
R-S2). Measured: GNU ld's built-in default is `/lib/ld-linux-aarch64.so.1`
(right) on AArch64, but `/lib/ld64.so.1` on x86-64 and `/usr/lib/libc.so.1`
on i386, whose outputs fail to start (exit 255); lld and mold write no
`PT_INTERP` at all and the output crashes (exit 139), E-dynamic `interp`.
Every driver passes the option (§2.1). `.interp` holds the string and a
NUL; it is the first section of the first `PT_LOAD` after the headers
(layout L-60 rank 1; lld and mold; GNU ld puts the notes first). An
input section named `.interp` is refused by name in an executable that
writes one [unmeasured; no corpus input has one] (request R-L2: layout
L-39). *Test:* `no-dynamic-linker-refused`.

**D-32 [M2] (`_DYNAMIC` and the GOT headers).** `_DYNAMIC` is the address
of `.dynamic`, local and hidden (resolve §8.1), in `Exec`, `Pie` and
`StaticPie`. The first word of the GOT the architecture names holds it:
x86-64 and i386 `.got.plt[0]` (psABI; all three references), AArch64
`.got[0]` (a64 §10.1 Q8 stands: GNU ld writes it; lld writes it nowhere
and glibc 2.41 needs none of them, E-dynamic §2); the other reserved
`.got.plt` words are 0. `_GLOBAL_OFFSET_TABLE_` is a64.md's and x64.md's
(layout L-101). *Test:* `got-header`.

**D-33 [M2] (`.dynamic` is writable at load).** `.dynamic` is in a
`PT_LOAD` with `PF_W` and inside `PT_GNU_RELRO` (all three). glibc writes
the `r_debug` address into `DT_DEBUG` and, in a PIE, adds the load bias to
the address tags in place; with the segment read-only the program dies
(`SIGSEGV`), E-dynamic `dyncheck`. `PT_DYNAMIC` covers exactly `.dynamic`,
`p_align` 8 (4 on ELF32), `PF_R|PF_W`. Under `-z norelro` (supported;
all three link and run dynamic outputs with it, `review-m2-adv/dyntags`
variant `relro-off`) there is no `PT_GNU_RELRO`; `.dynamic` keeps its
place and stays writable for the whole run. *Test:* `dynamic-writable`,
`norelro-dynamic`.

**D-34 [M2] (section headers).** As GNU ld and lld write them
(`design/shdr.sh`; mold omits `SHF_INFO_LINK`):

| section | `sh_type` | `sh_flags` | `sh_link` | `sh_info` | `sh_entsize` | `sh_addralign` |
|---|---|---|---|---|---|---|
| `.interp` | `PROGBITS` | `A` | 0 | 0 | 0 | 1 |
| `.gnu.hash` | `GNU_HASH` | `A` | `.dynsym` | 0 | 0 (ELF32: 4) | word |
| `.dynsym` | `DYNSYM` | `A` | `.dynstr` | 1 | 24 (16) | word |
| `.dynstr` | `STRTAB` | `A` | 0 | 0 | 0 | 1 |
| `.gnu.version` | `GNU_versym` | `A` | `.dynsym` | 0 | 2 | 2 |
| `.gnu.version_r` | `GNU_verneed` | `A` | `.dynstr` | the `Verneed` count | 0 | 4 (lld and mold; GNU ld 8) |
| `.rela.dyn` | `RELA` (`REL`) | `A` | `.dynsym` | 0 | 24 (8) | word |
| `.rela.plt` | `RELA` (`REL`) | `A` + `SHF_INFO_LINK` | `.dynsym` | `.got.plt` | 24 (8) | word |
| `.dynamic` | `DYNAMIC` | `WA` | `.dynstr` | 0 | 16 (8) | word |

`e_type` per kind is D-1's. This fills image.md I-40 (request R-I2).
*Test:* `dyn-shdr`.
## 8. Dynamic relocations, copy relocations, canonical PLTs, TLS

Owner: the demand rules are arch.md §4.3 and the architecture specs; the
records are `mortar_synth`'s (sizes, order) and `mortar_image`'s (bytes).
Measured in E-reloc: 48 self-checking programs (3 architectures × PIE /
non-PIE × lazy / `-z now` × 4 references) comparing every address three
ways (in the executable, in a library, through `dlsym`), and 433 side
experiments, all run.

**D-40 [M2] (`.rela.dyn`: set and order).** The records are a function of
`Demands`, `Slots` and the target kinds, never of addresses: for each
relocation of a live section that needs a dynamic record (arch.md §4.3's
`R`, `D` cells and the GOT slots of §4.4), one record. Order:

1. every `RELATIVE`, by `r_offset` (all three references); their count is
   `DT_RELACOUNT` (GNU ld and lld write it, mold never);
2. then every other record except `IRELATIVE`, by (`.dynsym` index,
   `r_offset`) (lld and mold; GNU ld sorts by offset with the `COPY`s
   last; synth S-104 stands): `GLOB_DAT`, symbolic (`R_AARCH64_ABS64`,
   `R_X86_64_64`, `R_386_32`), `COPY`, `TLS_TPREL64`/`TPOFF64`/`TLS_TPOFF`,
   `DTPMOD`/`DTPOFF`, `TLSDESC`.

The order in step 1 is computable in parallel without a global sort:
sections are placed in increasing address order within each chunk
(model §4.4), so the `RELATIVE` region is the concatenation, in address
order of the placed sections and GOT slots, of each unit's records sorted
by offset; each unit's slice starts at a prefix sum of the per-section
counts the scan produced (model §3.7, image I-15). The step-2 records are
few (one per imported key and place) and are collected and sorted. This
is the divisible form request R-S3 asks synth S-4 to state. *Tests:*
`rela-dyn-order` (sorted, counted; `--threads=1` vs many), a large PIE
(10^6 `RELATIVE`) in the bench set.

**D-41 [M2] (`.rela.plt` and `IRELATIVE`).** `.rela.plt` holds one
`JUMP_SLOT` per PLT entry in PLT order, then one `IRELATIVE` per IPLT
entry in IPLT order. PLT order is the `SymbolId` order of the keys that
demand `PLT` or `CANON_PLT` (`Slots`, model §4.3; GNU ld uses a hash
order, lld first reference, mold its `.dynsym` order). In `Exec` and
`Pie`, `IRELATIVE` goes last in `.rela.plt` (GNU ld; synth S-69, a64
§6.2): glibc applies `.rela.dyn` before `.rela.plt`, so an IFUNC resolver
that calls a shared-object function through its PLT or GOT runs after
those are bound. lld and mold put `IRELATIVE` at the end of `.rela.dyn`,
and such a resolver crashes (`SIGSEGV` in 7 of 12 configurations each,
E-reloc `ifh_plt`); glibc documents the resolver as unsupported, so this
is a hazard of theirs rather than a wrong output, and mortar takes the
order that does not have it. `StaticPie` keeps S-70 (all `IRELATIVE` in
`.rela.dyn`; it has no `.rela.plt`). *Test:* `irelative-resolver-calls-plt`
(runs on the three architectures, lazy and `-z now`).

**D-42 [M2] (copy relocations).** A `COPY` demand (arch.md §4.3 `C` on a
shared data definition) gives one allocation per `(shared FileId,
st_value)` (model §4.3; every alias at that address shares it, resolve
§7.3, and is exported, D-21). The allocation:

- size: the defining entry's `st_size`; an `st_size` of 0 is refused,
  `copy-relocation-of-zero-size` (lld: `cannot create a copy relocation
  for symbol zsym`; GNU ld silently gives the symbol the address of
  another copy and the program reads the wrong object; mold makes a
  0-byte copy outside every section and the program reads 0: wrong
  outputs of both, E-reloc `copy/zero`);
- alignment: `min(sh_addralign of the defining section, 2^ctz(st_value))`
  (resolve §9.6; all three honour it, E-reloc `calign`);
- place: the chunk `.bss.rel.ro` (output section `.bss.rel.ro`, inside
  RELRO; layout L-80; lld's name and section type) when the defining
  address lies in a `PT_GNU_RELRO` or in a non-writable `PT_LOAD` of the
  shared object (lld's `isReadOnly` test), else the chunk `.dynbss`
  (output section `.bss`, its first chunk). Copied typeinfo and vtables
  (libstdc++ defines them in `.data.rel.ro`, a writable `PT_LOAD` inside
  `PT_GNU_RELRO`) therefore land inside RELRO, as with all three
  references (GNU ld `.data.rel.ro`, lld `.bss.rel.ro`, mold
  `.copyrel.rel.ro`; `review-m2-adv/copyro`, AArch64 and x86-64, all run).
  Under `-z norelro` there is no RELRO: `.bss.rel.ro` is still the chunk,
  placed as an ordinary writable section (layout L-80 without the RELRO
  run). a64.md §10.5's `.data.rel.ro` is corrected to this (request R-A4);
- order within each chunk: by the least `SymbolId` among the aliases;
- record: one `COPY` per allocation, `r_sym` the `.dynsym` entry of the
  alias with the least `SymbolId` among the aliases that a live
  relocatable object references (GNU ld and lld attach the record to the
  primary name, mold to an alias; all run), `r_offset` the allocation,
  addend 0. Every alias at that `(file, st_value)`, referenced or not, is
  exported at the copy's address (D-21 item 3; the fix of W2).

Refused by name: a copy of an `STT_TLS` definition (`tls-mismatch`, D-16:
the reference that would copy it is a non-TLS class against an `STT_TLS`
shared definition; GNU ld and lld refuse too, mold copies it and the
program reads the library's ELF header, W14), of an `STT_FUNC`/
`STT_GNU_IFUNC` definition (a function takes D-44 instead), of a
protected one (D-18),
and any copy under `-z nocopyreloc` (an option that stays refused today;
GNU ld, lld and mold refuse the link with it, E-reloc). A size mismatch
between link time and run time is not detectable at link time (glibc
warns and copies the link-time size). *Tests:* `copy-reloc` (stdout,
environ aliases, `std::cout`, a 64-aligned value), `copy-typeinfo-relro`
(`&typeid(std::bad_alloc)` and a vtable from `-fno-pic` code: the copy is
inside `PT_GNU_RELRO`), `copy-zero-size`, `copy-tls-refused`.

**D-43 [M2] (`COPY` in a `Pie`).** arch.md §4.3 row `Pc` against
`Shlib{func: false}` in `Pie` is `C`, not `E` (request R-A3; a64.md Q5
answered): GCC's x86-64 `-fPIE` code (its default) reads shared data with
`R_X86_64_PC32` (`stdout`, `std::cout`), and GNU ld, lld and mold all
copy it into the PIE, which runs (E-corpus §2.4, E-reloc §4; Clang reaches
the same data through `GOTPCRELX` unless `-fdirect-access-external-data`).
`Pc` against `Shlib{func: true}` in `Pie` stays `E` (D-44). *Test:*
`copy-in-pie-x86-64`.

**D-43a [M4; planned: WP-122] (a copy of a definition its library binds
locally).** Request R-D43 (§18.4), the rule of D-126 item 2, for `Exec`
and `Pie` (a `Shared` output never copies, D-122). When a `COPY` demand
(D-42, D-43) copies a definition whose library binds it locally, the
executable's copy and the library's own instance are two objects. "Binds
locally": the library has `DT_SYMBOLIC` or `DF_SYMBOLIC`, or a
`.note.mason` row (odr.md O-51, O-60) for the definition carries
`NONPREEMPTIBLE`. Decided from the library's program headers and
`.dynamic` only (the symbol count from `DT_GNU_HASH` or `DT_HASH`),
never its section headers; where the definition's `st_value` lies:

- in a `PT_LOAD` with `PF_W` and outside every `PT_GNU_RELRO`: refused,
  `copy-of-locally-bound-definition` (diag.md §3.1, WP-116), naming the
  symbol and the library;
- in a read-only `PT_LOAD` or inside a `PT_GNU_RELRO`: the same kind as
  a warning, with the note `{name} is read-only in {file}: two equal
  copies` (two instances of a constant: defined behaviour, only `&v`
  differs);
- in no `PT_LOAD`: refused (mortar cannot tell); so is any copy from
  a library one of whose `PT_LOAD`s ends past 2^64 (`p_vaddr + p_memsz`
  overflows), and a `PT_GNU_RELRO` whose end overflows covers nothing.

A `.note.mason` table that does not parse (O-51's causes) cannot say
which definitions are bound: every definition of that library counts as
bound locally, and a note says why.

The rule does not change D-18 (a protected definition is refused
already) nor the `Pie` and `Exec` rows of D-42/D-43 for a library that
does not bind locally. A mold `-Bsymbolic` library carries neither tag
and is not seen (a recorded gap); `NONPREEMPTIBLE` rows are read from
any table found (`mortar_meta::table`; mortar-made libraries carry one
once WP-129 writes the tables of shared objects). The verifier's side is P-R9, plumb.md P-D24 (§12.5, added to P-D11). *Test:*
`copy-of-symbolic` (refuse `.data`/`.bss`, warn `.rodata`/
`.data.rel.ro`, a GNU ld `-Bsymbolic` library; the plain library
unaffected; W30, anomaly A44), `copy-of-symbolic-rows` (a table's
`NONPREEMPTIBLE` rows, written by the test producer, and a table that
does not parse); outside every `PT_LOAD` and overflowing headers: unit
tests (`mortar_scan` `copies::tests`). An lld library and a mortar one
wait for WP-129 (the `shlib` step is GNU ld's).

**D-44 [M2] (canonical PLTs).** In `Exec`, a non-GOT address use of a
shared function (`C` against `Shlib{func: true}`: `R_AARCH64_ADR_PREL_PG_HI21`,
`ABS64` in a read-only place, `R_X86_64_32S`/`PC32`, `R_386_32` in code)
demands `CANON_PLT`: the key's address in the whole executable is its PLT
entry, and its `.dynsym` entry is `SHN_UNDEF` with that `st_value` (D-23).
All three references, every architecture, lazy and `-z now`: the
executable's, the library's and `dlsym`'s `&puts` agree (E-reloc §5). In
`Pie` such a use is refused by name, `canonical-plt-in-pie` (arch.md
§4.3 `E`), with the message ``relocation R_AARCH64_ADR_PREL_PG_HI21
against symbol `puts' cannot be used in a position-independent
executable; recompile with -fPIE, or link with -no-pie``. Measured
(`review-m2-adv/canonpie`, a `-fno-pic` object taking `&puts`, `&qsort`
in the default PIE line): on AArch64 GNU ld refuses (``… can not be used
when making a shared object; recompile with -fPIC``), lld and mold write
a canonical PLT in the PIE and address equality holds; on x86-64
(`R_X86_64_32`) GNU ld and mold refuse, and lld links a canonical PLT
whose 32-bit absolute word gets no record, so the `-fno-pic` code sees
the unrelocated link-time address (`pie=0 lib=0 dlsym=0`): a wrong
output of lld (W11, §13.2). mortar takes GNU ld's behaviour on both
architectures. A protected definition is D-18. kiln+hod `-fno-pic`
objects use `ADR_PREL_PG_HI21` against `puts` and
`_ZNSt13runtime_errorD1Ev` (`review-m2-adv/hodpie`), so the Mason lane's
`-fno-pic` cells link `-no-pie` once the lane goes dynamic (record §3,
WP-M2-LANE). *Tests:* `canonical-plt-equality`,
`canon-plt-in-pie-refused` (AArch64 and x86-64; W11).

**D-45 [M2] (IFUNCs defined in the executable).** A non-preemptible IFUNC
has one IPLT entry, which is its address everywhere in the executable
(arch.md §4.3 `I`; a64 §10.4): a GOT slot of it holds the IPLT entry's
address (`Exec`: the value; `Pie`: a `RELATIVE`), never an `IRELATIVE` of
its own; the IPLT entry's `.got.plt` word carries the only `IRELATIVE`.
This is lld's rule (one `IRELATIVE` per function); GNU ld and mold give
the GOT slot its own `IRELATIVE` to the implementation, so in an x86-64
PIE `&f` differs between a translation unit that takes it PC-relatively
and one that loads it from the GOT (`a != b`, E-reloc `ifeq`, a wrong
output of both). An exported IFUNC is `STT_FUNC` at its IPLT entry in
`.dynsym` (D-23). x86.md X86-50's static-pie rule (every address word an
`IRELATIVE` to the resolved function) does not extend to `Pie`: there
the IPLT entry is canonical as on the other architectures (request R-X86-1).
*Test:* `ifunc-equality-pie` (two translation units, three
architectures).

**D-46 [M2] (GOT slots and data words: which record).**

| target kind | GOT slot in `Exec` | GOT slot in `Pie` | absolute word, writable place, `Exec` | same, `Pie` |
|---|---|---|---|---|
| `Local` | `S`, no record | `S`, `RELATIVE` | value | `RELATIVE` |
| `Abs` | `S` | `S` | value | value |
| `Shlib`, `WeakDyn` | 0, `GLOB_DAT` | 0, `GLOB_DAT` | symbolic record, `r_sym` the key, addend `A` (RELA) | same |
| `Ifunc` | the IPLT entry | the entry, `RELATIVE` | the entry | the entry, `RELATIVE` |
| `Weak0` | 0 | 0 | `A` | `A`, no record |
| `Copy` (a key with a copy allocation, D-42) | 0, `GLOB_DAT`, as `Shlib` | 0, `GLOB_DAT`, as `Shlib` | symbolic record, `r_sym` the key, addend `A` (RELA), as `Shlib` | same |

All three agree (E-reloc §3), but on the `Copy` row: a word that points
at a copied key gets the key's symbolic record, as lld writes; GNU ld
writes the copy's address and no record (§13.1). Both run, since the
loader binds the key to the copy; the record keeps `LD_PRELOAD`
interposition of the copied object working, as for any import
(WP-054, supervisor's ruling in WP-083; `copy-reloc`, `exceptions-rtti`,
`-fno-pic -no-pie`). A GOT slot of a copied key is the `Shlib` slot:
GNU ld, lld, mold and mortar write 0 and `GLOB_DAT` against the key, in
`Exec` on both 64-bit targets and in the x86-64 `Pie` (WP-083's
measurement, `copy-got`). A word in a read-only place that would need
a record is a text relocation (D-47), except in `Exec` where arch.md §4.3
turns it into `COPY` or a canonical PLT. GOT relaxation in `Exec` and
`Pie` is the architecture's (a64 R5; x64 G1-G4 extended to `Exec` and
`Pie`, G5/G6 to `Exec`, request R-X1; i386 [M4]); every reference
relaxes the x86-64 local `GOTPCRELX` (E-reloc §3). Under RELA the place of
a `GLOB_DAT` or symbolic record holds 0, of a `JUMP_SLOT` the lazy target
(D-49; a64 §6.2, x64 §6.2), and of a `RELATIVE` `S + A` (a64 §6.2 Q6); under REL (i386, [M4]) the place holds the addend:
`S + A` for `RELATIVE`, `A` for `R_386_32`, 0 for `GLOB_DAT`, the lazy
target for `JUMP_SLOT` (all three, E-reloc §1, request R-X86-2).

**D-47 [M2] (text relocations refused).** A dynamic record whose place is
in a section without `SHF_WRITE` is refused by name, `text-relocation`,
with GNU ld's wording as the message: ``relocation R_X86_64_32S against
symbol `ro_pfa' can not be used when making a PIE object; recompile with
-fPIE``. `-z text` is the default and `supported` (noop today);
`-z notext` is refused (request R-C1). The references disagree three
ways (GNU ld writes `DT_TEXTREL` for a word a record can express: with a
warning on i386 and x86-64, `warning: creating DT_TEXTREL in a PIE`,
silently on AArch64, and refuses only a word no record expresses, such
as `R_X86_64_32S`; lld refuses unless `-z notext`; mold writes
`DT_TEXTREL` silently on all three, and its 64-bit programs run while
its i386 output crashes, E-reloc §8, W6; measured again for an
address-sized word in `.rodata` of a `-fPIE` object by WP-051,
`textrel-refused`). lld also writes a wrong output here: an absolute
word in a read-only section against a shared object's data, in a PIE,
links silently with a copy relocation and no record, and the program
crashes on the unrelocated pointer (12 of 12 cases, three architectures,
E-dynamic `lld_textrel_copyrel.sh`). *Tests:* `textrel-refused` (the
message), `textrel-ro-copy-pie` (mortar refuses where lld miswrites).

**D-48 [M2] (TLS).** In `Exec` and `Pie`:

- against a TLS definition of the executable: LE, every model relaxed
  (lld and mold; GNU ld keeps IE with a dynamic `TPOFF` against the
  executable's own symbol, valid but slower, E-reloc §7);
- against a shared object's TLS: IE. GD and TLSDESC sequences that the
  architecture recognises relax to IE (all three, every architecture); the
  slot is a `GOT_TPREL` slot with a `TLS_TPREL64`/`TPOFF64`/`TLS_TPOFF`
  record against the key, addend 0, place 0;
- a GD sequence that is not recognised keeps its module/offset pair:
  `DTPMOD` + `DTPOFF` records against the key (mold keeps i386 GD and
  AArch64 traditional GD this way, and they run); a TLSDESC sequence that
  is not recognised (AArch64's large form, a64 R2) keeps a `TLSDESC`
  record in `.rela.dyn`, applied eagerly: no `DT_TLSDESC_PLT`/`GOT` (a64
  §10.6; no reference writes them in an executable). Exception, x86-64:
  an unrecognised TLSDESC sequence in `Exec` or `Pie` is refused by name
  (x64.md X-28; `R_X86_64_TLSDESC`, type 36, stays M4, x64.md §6.2;
  supervisor's ruling after WP-040r);
- `DF_STATIC_TLS` is never set (D-30).

a64.md has every rule (R1-R5, §10.1), except that DTPMOD64/DTPREL64 are
listed "in shared" in its §6.2 (request R-A5). x64.md and x86.md scope
every TLS relaxation to static and static-pie: requests R-X2 (x86-64:
X-23, X-25, X-27, X-28, X-46, X-47 apply to `Exec` and `Pie`; GD→IE and
TLSDESC→IE bytes, which the references' outputs in E-reloc `tls/` give,
as rules of their own; `DTPOFF64`, `TPOFF64`, `R_X86_64_64` added to
§6.2 with the word, `r_sym` and addend columns) and R-X86-3 (i386, [M4]).
*Tests:* `tls-shared-ie`, `tls-exe-le`, `tls-gd-unrelaxed`,
`tls-desc-large` (AArch64), each run.

**D-49 [M2] (lazy binding and `-z now`).** Lazy binding is the default
(all three). The `.got.plt` header is three words (x86 `[0] = _DYNAMIC`,
D-32; AArch64 all 0), then one word per PLT entry, then one per IPLT
entry (a64 §10.2, synth S-107). A `JUMP_SLOT` word initially holds its lazy
target at the link-time address: AArch64 PLT0, x86-64 and i386 [M4] its own
entry plus 6 (the `push`; GNU ld and lld; mold uses PLT0 with the index in
`%r11d`); in a PIE the loader adds the load bias (every lazy PIE of
E-reloc runs). Under `-z now` the shapes stay (PLT0 and the lazy stubs are
written, as all three do), `.got.plt` moves into RELRO as its own chunk
(layout L-60 rank 7; lld and mold; GNU ld merges it into `.got`),
`DT_FLAGS = DF_BIND_NOW` and `DF_1_NOW` are set (D-30); under `-z now -z
norelro` `.got.plt` keeps its own chunk in the writable data and nothing
is read-only after relocation. No `.plt.got`
(lld; GNU ld and mold use it for keys with both a GOT and a PLT use):
such a key gets a PLT entry and a GOT slot. The x86-64 IBT shapes (`-z
ibt`, `-z ibtplt`, `.plt.sec`) stay refused at M2 (no census line passes
them; Debian's GCC does not enable `-fcf-protection`) (Q4, ruled:
refused at M2, a package of its own when a census line passes
`-fcf-protection`; the merged IBT property of all-IBT inputs goes with
it). *Tests:* `plt-lazy`, `plt-now`
(each architecture, run, `readelf -r`).

**D-50 [M2] (preemption).** No definition of an executable is preemptible
(resolve §9.6); a reference from the executable to its own exported
definition is bound at link time, and a shared object's reference to it
binds to the executable's at run time (E-shlib 5.1, 5.3, 5.8e).
## 9. Layout

layout.md specified most of this with M2 in view; E-dynamic measured it.

**D-60 [M2] (section order; confirmed).** layout L-60's ranks give, for a
PIE: headers, `.interp`, notes, `.gnu.hash`, `.dynsym`, `.dynstr`,
`.gnu.version`, `.gnu.version_r`, `.rela.dyn`, `.rela.plt`, read-only
data, `.eh_frame_hdr`, `.eh_frame` | `.init`, `.plt`, `.text`, `.fini` |
TLS, `.init_array`, `.fini_array`, `.data.rel.ro`, `.dynamic`, `.got`,
`.got.plt` (`-z now`), `.bss.rel.ro`, `.pad.relro` (RELRO) |
`.got.plt` (lazy), `.data`, `.bss` (with `.dynbss` first). That is lld's
order with GNU ld's placement of the dynamic tables (E-dynamic §2); the
two missing slots are added: `.dynbss` (D-42) and `.bss.rel.ro`'s copy
chunk (request R-L1). `.hash` is removed from rank 3 (D-20). The order is
D-60's own: no reference writes it whole (measured by WP-051, both
architectures: GNU ld puts the notes before `.interp` and `.rodata` after
`.fini`; lld puts `.gnu.hash` after `.gnu.version_r`, `.text` before
`.init` and `.fini_array` before `.init_array`; mold puts `.eh_frame`
before the read-only data, `.fini` and `.init` first in the code and the
arrays after `.dynamic`). *Test:* `dyn-section-order`.

**D-61 [M2] (segments; confirmed).** `PT_PHDR`, `PT_INTERP`, the
`PT_LOAD`s, `PT_DYNAMIC`, `PT_NOTE`s, `PT_TLS`, `PT_GNU_PROPERTY`,
`PT_GNU_EH_FRAME`, `PT_GNU_STACK`, `PT_GNU_RELRO`, in that order (layout
L-67; GNU ld's order, measured on dynamic links now). `PT_INTERP` and
`PT_PHDR` exist iff an interpreter is named (layout L-67's "dynamic, not
-shared" would give static-pie a `PT_INTERP`; request R-L1). The four
`PT_LOAD`s R, RX, RW (RELRO), RW (layout L-65; lld and mold; GNU ld writes
two on AArch64, RX with the read-only data and RW, and four on x86-64,
R, RX, R, RW, its `-z separate-code` default, layout L-71 Q2 stands).
lld puts `PT_TLS` before `PT_DYNAMIC`, `PT_GNU_RELRO` before
`PT_GNU_EH_FRAME` and the notes last, and writes no `PT_GNU_PROPERTY` on
x86-64; mold puts `PT_NOTE` after `PT_INTERP` (WP-051). PIE base 0
(L-68); `-no-pie` 0x400000 (lld and mold: 0x200000, §13.1). *Test:*
`dyn-segments`. The RELRO run ends on a maxpage boundary (L-64): 64 KiB on
AArch64, which mold also does and lld does not (lld's AArch64 RELRO ends
mid-page, unsafe on a 64 KiB-page kernel [not run: no such kernel]).

**D-62 [M2] (layout is a pure function).** Nothing in this spec makes a
size depend on an address: `.dynsym`, `.dynstr`, `.gnu.hash`,
`.gnu.version*`, `.rela.dyn`, `.rela.plt`, `.dynamic`, `.interp`, PLT and
GOT sizes are functions of `Resolution`, `Demands` and `Slots` (D-30's
presence rules, D-40's record set). The only address-dependent dynamic
chunk is `.relr.dyn`, which is M4 (D-27). The RELATIVE/symbolic split
depends on the target kind, not on addresses; a `RELATIVE` whose place is
not word-aligned is refused (a64 §6.2's `relocation-misaligned`, applied
to every architecture: GCC and Clang never emit one, E-corpus).

## 10. Debuggers and tools

**D-70 [M2] (what debuggers need).** gdb 16.3 and lldb 19 set `break main`
before `run`, stop there, and list the shared libraries on every
reference's PIE, non-PIE and static-pie, natively on AArch64 and through
`qemu -g` for x86-64 and i386 (E-dynamic §3). They need: `DT_DEBUG`
(present, D-30; with it patched out, native gdb and lldb list only the
loader and miss libc), `.dynamic` writable at load (D-33), `PT_PHDR` and
`PT_INTERP` (D-61), and `.symtab` (synth §7). Nothing else is read before
relocation; gdb computes the PIE's bias from the auxiliary vector.
lldb needs `settings set target.disable-aslr false` in this container
(personality is refused); that is the harness's, not the output's.

**D-71 [M2] (tools).** `ldd` lists every reference's output correctly
(static-pie: "statically linked"), natively on AArch64 and through
`qemu ld.so --list` on x86-64 and i386, except lld's i386 non-PIE
outputs, on which `ld.so --list` under qemu-i386 dies with `SIGSEGV`
while the programs themselves run (Appendix B). `eu-elflint --gnu-ld` passes GNU ld's outputs and mortar's
with nothing allowed; its complaints about lld and mold are about their
`.relro_padding` (`NOBITS`/`WA`: elflint takes a name that starts with
`.rel` for a relocation section), all-headers-first `Vernaux` layout,
`.gnu.hash` `sh_entsize` 4 on ELF32 and `.rodata` entry sizes. mortar
writes `Vernaux` interleaved (D-27) and the ELF32 `sh_entsize` 4 of every
reference; its padding chunk (model §4.4) is kept (Q5, ruled: model
§4.4, it is what makes RELRO end on maxpage cheaply) and is named
`.pad.relro` (layout L-156, WP-083), which elflint does not flag. The `prelink` fields nobody reads (D-30)
are never written. *Test:* `elflint-dynamic` (the complaints are a fixed
allow-list).

**D-72 [M2] (debug sections in a PIE).** image.md I-47's open M2 item (a
zero tombstone in a PIE, where address 0 is the file header): I-47's
rows stand in a PIE: its 0 rows stay 0 and `.debug_ranges`/`.debug_loc`
stay 1, as lld (its all-ones rows are unchanged), since no
code is at 0 in a PIE either: the first `PT_LOAD` holds the headers (Q6,
ruled; image.md I-47 states it: its rows stand in a PIE).
## 11. Requests to other specs

Every change this spec asks of another spec, by id. All were applied by
WP-040r (`spec/landed/dyn/WP-040r.md` lists where, and the ones applied
differently). "Text" is the
proposed wording or its substance; the owning spec's maintainer lands it
(step 0 of the record, §D, for the ones an interface depends on).

### 11.1 `arch.md` and the architecture specs

| id | target | request |
|---|---|---|
| R-A1 | a64 §10.5 L1162 | canonical PLT in `.dynsym`: `st_shndx = SHN_UNDEF`, `st_value` = the PLT entry (D-23), not the PLT's section |
| R-A2 | arch §2.1 L77; a64 Q4; a64 §4, §7.1; x64 §4, X-57; x86 X86-68 | "a weak undefined key is `WeakDyn` or `Weak0` by dyn.md D-25, per key" (ends the circle with resolve §5.4); each architecture spec carries its column of D-25's class table, with the branch-opcode test in x64 and x86 (`Arch::weak_use`, WP-040 step 0k); a64 Q4's claim that lld makes an undefined weak dynamic "only with `-z dynamic-undefined-weak` or for shared outputs" is corrected: lld 19 and 22 do it in a default PIE (`WEAK UND`, `GLOB_DAT` + `JUMP_SLOT`; `review-m2-adv/weaksplit`) |
| R-A3 | arch §4.3 (`Pie` row, `Pc` against `Shlib{func: false}`) | `C`, not `E` (D-43); a64.md Q5 closed |
| R-A4 | a64 §10.5 | copy allocations go to `.dynbss` / `.bss.rel.ro` (D-42), not `.data.rel.ro` |
| R-A5 | a64 §6.2 | `R_AARCH64_TLS_DTPMOD64` / `DTPREL64` are written in `Exec` and `Pie` for an unrelaxed GD (D-48), not "in shared" only |
| R-A6 | arch §3.7 `PltShape`, §3.10 `DynRole` | the generic contract gains: (a) the lazy target of a `JUMP_SLOT` word (PLT0, or entry + k); (b) a `-z now` flag that moves `.got.plt` but keeps shapes (D-49); (c) the per-architecture dynamic tags and their conditions (D-30 rows 24-25); (d) the record format per `DynRole` (RELA: place 0 or `S+A`; REL: the addend in the place, D-46). `target_kind` stays per key (arch §2.1); a branch against a `WeakDyn` key demands `PLT` (D-25) |
| R-A7 | arch A.2 vs x64 X-19 | one scope for GOT relaxation: "every output kind, per D-46" (A.2's wording) |
| R-X1 | x64 X-19-X-22 (G1-G6) | GOT relaxation applies to `Exec` and `Pie` (G1-G4 both, G5/G6 `Exec` only, a non-preemptible target only; D-46) |
| R-X2 | x64 X-23-X-32, X-46, X-47, §6.2 | TLS rules apply to `Exec` and `Pie`; add GD→IE and TLSDESC→IE byte rules (from E-reloc `tls/`); add `R_X86_64_DTPOFF64`, `TPOFF64`, `R_X86_64_64` to §6.2 with word, `r_sym` and addend (D-48) |
| R-X3 | x64 §10.3 | the PLT/PLT0/lazy-word bytes are measured (E-reloc §6: GNU ld and lld agree byte for byte, entry + 6 lazy target); "not measured" removed |
| R-X86-1 | x86 X86-50 | the static-pie IFUNC rule is not extended to `Pie`; an IPLT entry is canonical there (D-45; x86.md's own R-14) |
| R-X86-2 | x86 §6.2 L693-700 [M4] | REL place contents per record (D-46) |
| R-X86-3 | x86 X86-32-37 [M4] | TLS rules for `Exec`/`Pie` as R-X2 |

### 11.2 Input, resolution, CLI

| id | target | request |
|---|---|---|
| R-M1 | model §3.3 | `file_live` of a loaded shared object is `Shared{needed}`, `Shared{used}` (U of D-12: reached by a `DT_NEEDED` edge from a needed or used object, whatever loaded it) or `Dead`; `Slots.needed` is in D-19a's order |
| R-R1 | resolve §5.2 L769 | the shlib-undefined rule is D-13 (GNU's), replacing lld's exemption |
| R-R2 | resolve §5.4 L819 | cite D-25 for `WeakDyn` / `Weak0` |
| R-R3 | resolve §3.6, §12.1 | `ref_dynamic` and `defined_dynamic` count members of N ∪ U only and are computed after D-11 and D-12 |
| R-R4 | resolve §3.4 | `tls-mismatch` also between a shared `STT_TLS` definition and a non-TLS reference (D-16) |
| R-R5 | resolve §9.6 | linker-defined symbols of file 0 are exported only when `ref_dynamic` (D-21) |
| R-R6 | resolve §1.2 L63 vs L473 | `--export-dynamic-symbol` (and `--dynamic-list`) one milestone: M2 for the option, M4 for the list file (Q9) |
| R-R7 | resolve §3.7 L315 | the `.dynsym` form of a `Shared` key is D-23's (undefined, or defined for a copy, or `SHN_UNDEF` + value for a canonical PLT) |
| R-I1 | input §8.6 L928-934, §15 L1645 | `--copy-dt-needed-entries` withdrawn (refused; D-15) |
| R-C1 | cli §2.3, §4.2, `docs/status/options.md` | rows: `-pie`, `-dynamic-linker`, `-Bdynamic`, `--as-needed`, `--no-as-needed`, `-E`, `-rpath`, `-init`, `-fini`, `-z text` supported; `--hash-style` partial (`gnu` only); `--export-dynamic-symbol` supported (Q9; one more source of `exported`, D-21); `-z norelro` stays supported, its effect on dynamic outputs per D-33, D-42, D-49, P-D4; `-z notext`, `-z nocopyreloc`, `--spare-dynamic-tags`, `--copy-dt-needed-entries`, `--dynamic-list` (M4), `-z nodynamic-undefined-weak` and `-z dynamic-undefined-weak` refused by name at M2 (no census line passes them; lld 19 warns that it does not know them; D-25); `--no-copy-dt-needed-entries` noop (§2.3) |

### 11.3 Output side

| id | target | request |
|---|---|---|
| R-S1 | synth S-101-S-105 | replaced by citations of D-21-D-27, D-30, D-40, D-41 |
| R-S2 | synth S-106 | "or the target's default" withdrawn (D-31) |
| R-S3 | synth S-4, S-108; image I-15 | the parallel fill of `RELATIVE` records by per-section prefix sums (D-40); a size row per dynamic chunk |
| R-S4 | synth S-3 | row 8 (verdef) tagged M4; `.interp` only with an interpreter (not static-pie); add `.dynbss` and the `.bss.rel.ro` copy chunk; remove `.hash` |
| R-S5 | synth S-71 | S-71 is `.symtab`'s rule; `.dynsym` of an IFUNC is D-23 (`STT_FUNC` at the IPLT entry, as a64 §10.4) |
| R-S6 | synth S-107 | "static shapes = dynamic shapes" is false on AArch64 (the dynamic PLT0 and `.got.plt` header); cite D-49 |
| R-L1 | layout L-60, L-67 | `.dynbss` and the copy chunk of `.bss.rel.ro` placed (D-60); `.hash` removed; `PT_INTERP`/`PT_PHDR` iff an interpreter is named (D-61) |
| R-L2 | layout L-39 | an input section `.interp` in an output that writes one is refused (D-31) |
| R-L3 | layout §1 L52, L-100 | "M2 … shared objects" means shared objects as inputs; producing them is M4 (ROADMAP) |
| R-I2 | image I-40 | section-header fields of the dynamic sections are D-34; `e_type` per kind D-1 |
| R-P1 | plumb (all of §12) | stage 3 rules P-D1-P-D13 |

## 12. Requests to `plumb.md` (stage 3)

Moved (WP-040r, request R-P1): the stage-3 rules P-D1-P-D13, the table of
which rule reports each wrong output and hazard of §13.2, and their
mutations are normative in `spec/plumb.md` §12, with their ids
unchanged. Every `P-D` id this spec cites is that section's.

## 13. Where the references differ; wrong outputs

### 13.1 Disagreements and the choice made

| question | GNU ld 2.44 | lld 19/22 | mold 2.37 | mortar |
|---|---|---|---|---|
| `.dynamic` tag order | its order | its own | its own | GNU ld's (D-30) |
| `DT_NULL` entries | 5-6 | 1 | 6 | 6 (D-30) |
| empty `.rela.dyn` | written | written | omitted | omitted (D-30) |
| undefined weak, GOT-used, in a PIE | dynamic; call via `.plt.got` (x86-64) or PLT | dynamic; call via PLT | 0 everywhere | dynamic, call via PLT (D-25) |
| `DT_NEEDED` order | keeping mention (D-19a) | first mention of the soname | first mention; later plain mentions of a first-as-needed soname ignored (W9) | GNU ld's (D-19a) |
| a key defined in two needed libraries | binds in keeping-mention order; needed follows the binding | binds at first mention | binds at first mention | binds in loader order (`bind`); needed by first mention (`giver`): at most one unused extra entry (D-11, `needed-order-dup-def`) |
| `.dynsym` unhashed block order | hash order | first appearance | library index | `SymbolId` (D-22) |
| Bloom words, buckets | own formula | D-26 | own | lld's (D-26) |
| `Vernaux` layout | interleaved | headers first | headers first | interleaved (D-27; elflint) |
| writable place (D-25) of a read-only input section in a writable output section | the output section: links (RWX segment, no `PT_GNU_RELRO`) | the input section: refuses the PIE ("recompile with -fPIC") | the output section: links (`DT_TEXTREL`, A24) | the output section; records, RELRO kept (D-25, WP-056) |
| `--export-dynamic` in a static-pie | exports every global (x86-64: the program dies at start-up) | exports every global | exports every global | accepted, no effect: `.dynsym` stays the null entry (D-1, P-D14); the exports are useless: a library the program `dlopen`s does not bind to them and `dlsym(RTLD_DEFAULT)` does not find them (WP-056, all three) |
| `--export-dynamic-symbol` in a static-pie | exports the name | exports the name | exports the name | accepted, no effect: `.dynsym` stays the null entry (D-1, P-D14) |
| `--export-dynamic-symbol=GLOB` | glob | glob | glob | glob (resolve.md §4.2; WP-056 fixed a literal match) |
| `.gnu.version_r` align | 8 | 4 | 4 | 4 (D-34) |
| `IRELATIVE` in a PIE | last of `.rela.plt` | end of `.rela.dyn` | end of `.rela.dyn` | GNU ld's (D-41) |
| `.rela.dyn` non-`RELATIVE` order | offset, `COPY` last | (sym, offset) | (sym, offset) | (sym, offset) (D-40) |
| copy section, read-only | `.data.rel.ro` | `.bss.rel.ro` | `.copyrel.rel.ro` | `.bss.rel.ro` (D-42) |
| copy record `r_sym` | primary | primary | an alias | least referenced alias (D-42) |
| TLS of the executable's own symbol | IE + `TPOFF` on x86-64 when the symbol is exported; LE on AArch64, and on x86-64 otherwise (`tls-exe-le`) | LE | LE | LE (D-48) |
| `DF_STATIC_TLS` | never | never | always | never (D-30) |
| text relocations | `DT_TEXTREL` for a word a record expresses (a warning on i386 and x86-64, silent on AArch64); refused for one no record expresses (`R_X86_64_32S`) | refused | written silently (W6) | refused (D-47; `textrel-refused`) |
| `.got.plt` under `-z now` | merged into `.got` | own, RELRO | own, RELRO | own, RELRO (D-49) |
| `.plt.got` | yes | no | yes | no (D-49) |
| x86-64 lazy target | entry + 6 | entry + 6 | PLT0 | entry + 6 (D-49) |
| `.interp` position | after notes | first | first | first (D-31) |
| static-pie `PT_PHDR` | no | yes | yes | no (D-1) |
| default interpreter | wrong on x86 | none | none | refused without one (D-31) |
| hash style default | gnu | both | both | gnu; others refused (D-20) |
| `PT_GNU_EH_FRAME` without `--eh-frame-hdr` | no | no | yes | no (Q3) |
| RELRO end on AArch64 | maxpage | mid-page | maxpage | maxpage (D-61) |
| RELRO padding section | none | `.relro_padding` | `.relro_padding` | `.pad.relro` (layout L-156; eu-elflint takes a `.rel*` name for a relocation section) |
| an as-needed library reached only by a weak reference | not needed | not needed | needed | not needed (D-11; `as-needed-weak`) |
| undefined weak key reached by branches only | dynamic, `JUMP_SLOT` | dynamic, `JUMP_SLOT` | 0 | `Weak0`, no entry (D-25; `weak-class-table`) |
| `-no-pie` base | 0x400000 | 0x200000 | 0x200000 | 0x400000 (D-1, layout L-68; `dyn-segments`) |
| `PT_LOAD`s of a dynamic executable | AArch64 2 (RX, RW); x86-64 4 (R, RX, R, RW) | 4 (R, RX, RW, RW) | 4 | 4, lld's (D-61) |
| `PT_GNU_PROPERTY` on x86-64 | yes | no | yes | yes (D-61) |
| static-pie `IRELATIVE` | `.rela.plt`, with `DT_JMPREL` | `.rela.dyn` | `.rela.dyn` | `.rela.dyn` (synth S-70; `output-kind-table-static-pie`) |
| static-pie `DT_RELACOUNT` | yes | yes | none | yes (`output-kind-table-static-pie`) |
| binding of a copy of a weak shared definition | `STB_WEAK` | `STB_GLOBAL` | `STB_GLOBAL`, and a global alias may be made weak (`__environ`) | `STB_WEAK` (D-23; `dynsym-fields`, `environ-aliases`) |
| `DT_AARCH64_VARIANT_PCS` position | after the `RELA` group, before `DT_FLAGS_1` | after `DT_FLAGS_1` | after `DT_FLAGS_1` | GNU ld's (D-30; `variant-pcs-plt`) |
| AArch64 BTI property and `DT_AARCH64_BTI_PLT` | kept | kept | dropped, no tag | kept (D-30; `dynamic-tags`) |
| AArch64 word holding `_DYNAMIC` | `.got[0]` | none | `.got.plt[0]` | `.got[0]` (D-32; `got-header`) |
| AArch64 PIE `.dynsym` `sh_info` | 3 (two `STT_SECTION` entries) | 1 | 1 | 1 (D-21, D-34; `dyn-shdr`) |
| `SHF_INFO_LINK` on `.rela.plt` | yes | yes | no | yes (D-34; `dyn-shdr`) |
| `.plt` `sh_entsize` | 16 (8 for `.plt.got`) | 0 | 0 | 0 (WP-055; no loader or tool reads it) |
| `.tdata` `sh_addralign` | raised to `PT_TLS`'s (8 in `gxx-static-libstdcxx`) | the inputs' | the inputs' | the inputs' maximum, 4 there (layout L-76; `PT_TLS` and the addresses agree) |
| a program referencing `foo` and `foo@V` of one shared definition (no copy, no canonical PLT) | not measured | not measured | one `UND` entry | two `UND` entries, one name and version (the loader binds both alike; WP-053) |
| a writable word pointing at a copied key (D-46 `Copy`; its GOT slot is not a difference: all four write `GLOB_DAT`, `copy-got`) | the copy's address, no record | symbolic record | not measured | symbolic record (D-46; WP-054 §4) |
| AArch64 `-no-pie`: an `ABS64` in a writable word of a PIC or PIE object against shared data (`environ` in `.data.rel`) | refused (`unresolvable R_AARCH64_ABS64 relocation against symbol environ@@GLIBC_2.17`, as WP-053 quotes it) | symbolic record | not measured | symbolic record (D-46; the program runs; WP-053, E-corpus C, 44 links) |
| `IRELATIVE` before `JUMP_SLOT`s | no | in every output with an IFUNC and a PLT (H1) | in every output with an IFUNC and a PLT (H1) | no (D-41; plumb.md P-D10) |
| AArch64 traditional GD (`-mtls-dialect=trad`) in a static executable or a static-pie | R4's LE words | refused (`unknown relocation (513)`) | kept: GOT pair `1, DTPREL(S)` and the static `__tls_get_addr` | R4's LE words, GNU ld's bytes; a sequence outside R4's pattern refused (a64.md §8.4's static column; WP-090, `tls/gd-trad-static.test`) |

### 13.2 Wrong outputs found (each with its script)

All under `/Volumes/AgentWork/mortar-helper/.build/review-m2/`; "runs
wrong" means the program links without a diagnostic and then misbehaves.

| # | reference | finding | script |
|---|---|---|---|
| W1 | all three | a `.dynsym` entry made `STV_HIDDEN` is bound; the output fails to load | `shlib/run.sh` exp. 5.7, `shlib/patch_vis.py` |
| W2 | GNU ld | not every alias of a copied `environ` is exported; a library reading `_environ` sees libc's stale variable after `setenv` | `dynsym/run.sh` (envalias) |
| W3 | GNU ld, mold | `&ifunc` of an executable IFUNC differs between translation units: x86-64 PIE (GNU ld, mold) and `-no-pie` (GNU ld); AArch64 PIE with a `-fno-pic` unit (mold, and GNU ld: measured by WP-051) | `reloc/run.sh` (ifeq, `main.x86_64.pie`); `corpus/mixed.sh` (`all-pie`, `pie+nopic`, …); `tests/link/dyn/reloc/ifunc-equality-pie.test` |
| W4 | GNU ld, mold | copy of a zero-size symbol: GNU ld aliases another copy, mold makes a 0-byte copy outside any section; reads wrong values | `reloc/run.sh` (copy/zero) |
| W5 | mold | canonical PLT for a protected function: links, address inequality | `reloc/run.sh` (copy/pfn); `shlib` 5.6d |
| W6 | mold | text relocations written silently (`DT_TEXTREL`): i386, where the program crashes; AArch64 PIEs with a `-fno-pic` unit (8 mixes); on the 64-bit targets the programs run (`textrel-refused`) | `reloc/run.sh` (textrel, `t.i386.*.mold`); `corpus/mixed.sh` |
| W7 | lld 19, 22 | PIE: absolute word in read-only data against shared data links with a copy and no record; the program crashes (12/12) | `dynamic/lld_textrel_copyrel.sh` |
| W8 | lld, mold | a symbol referenced only by an under-linked closure library is not exported; load fails with an undefined symbol | `shlib/run.sh` exp. 4.2a, 2.3b |
| W9 | mold | a library first mentioned `--as-needed` (unreferenced) and later plainly is dropped from `DT_NEEDED` (mold keeps the first mention's flag); also a second `-lname` after `-Bstatic`/`-Bdynamic` is ignored | `dynamic/variants.sh` (needed_twice), `dynamic/needed_order.sh` (`asn_unref_then_plain`, `bstatic_first`) |
| W10 | GNU ld | built-in default interpreter wrong on x86-64 and i386 (exit 255); lld and mold write no `PT_INTERP` (exit 139) | `dynamic/interp.sh` |
| W11 | lld 19, 22 | x86-64 PIE: a `-fno-pic` `R_X86_64_32` against `puts` gets a canonical PLT and no record; the code sees the unrelocated address (`pie=0 lib=0 dlsym=0`) | `review-m2-adv/canonpie/run.sh` |
| W12 | GNU ld (AArch64; x86-64 for the second form), lld (all three; x86-64 and AArch64 measured for the second form) | `-no-pie`, a weak symbol reached through the GOT and absolutely: (1) the absolute use in a `-fno-pic`/`-mcmodel=large` unit; (2) with `-fPIE` objects only, a data initializer `hp = hook`. Under `LD_PRELOAD` the GOT sees the definition and the absolute use sees 0 (`got non-null abs null`, `hp==hook 0`) | `reloc/weakpc.sh` (stage `weakmix`); `review-m2-adv/weaksplit2/run.sh` |
| W13 | GNU ld, mold | x86-64 PIE: an executable IFUNC referenced from a library; the loader refuses (``IFUNC symbol 'main_ifunc' referenced in libmxi.so is defined in the executable and creates an unsatisfiable circular dependency``, rc 127); on AArch64, mold (PIE, all `-fno-pic`) and GNU ld (`-no-pie`) give the library a different `&main_ifunc` | `corpus/mixed.sh` (exe-ifunc-from-dso) |
| W14 | lld, mold | a TLS/non-TLS mismatch against a shared definition (D-16) links, and the program crashes (SIGSEGV) or reads garbage; mold also copies an `STT_TLS` definition (D-42) and the program reads the library's ELF header (`tlsx 1179403647`) | `tests/link/dyn/shlib/tls-mismatch-shared.test`, `tests/link/dyn/reloc/copy-tls-refused.test` (WP-051) |
| W15 | GNU ld (AArch64) | a traditional GD sequence whose fourth word is not a64 R4's `nop` is relaxed to IE anyway, overwriting the instruction after the call; the program reads a wrong address (`same 0`) | `tests/link/dyn/reloc/tls-gd-unrelaxed.test` (WP-051) |
| W16 | lld, mold | X3: a library referenced only by an under-linked used object is dropped from `DT_NEEDED`; load fails (``symbol lookup error: … undefined symbol: leaf_fn``, exit 127). GNU ld and mortar refuse (D-13) | `tests/link/dyn/shlib/used-underlinks-refused.test` (WP-051) |
| W17 | lld 19, 22 (x86-64) | general-dynamic access in the large code model (`-mcmodel=large -fPIC`, GCC 14) to the executable's own TLS links, and the program crashes (SIGSEGV) in a PIE and in `-no-pie`; GNU ld, mold and mortar run it (`GD ok 7`); x64.md X-25 records only the static case. Run under qemu-x86_64, where the other three run | WP-083's experiment: `__thread int own = 7;` printed by `main`, one `-fPIC -mcmodel=large` unit (record, §5) |
| W18 | GNU ld (AArch64) | `-static` and `-static-pie`: a traditional GD sequence whose TP offset is beyond 32 bits (4 GiB of `.tbss` before the variable) is relaxed to LE with `T[31:0]` and no diagnostic (`movk x0, #0x68` for `T` = 0x100000068): a wrong address. mold keeps GD and is right; mortar refuses (`relocation-out-of-range`, a64.md §8.4) | `tests/link/tls/gd-trad-static-refused.test` (WP-090) |
| A30 | GNU ld (AArch64) | static-pie and `-pie`: the GOT slot of an absolute symbol (`SHN_ABS`, glibc's `_nl_current_*_used`) gets a `RELATIVE` record, so the program reads the value plus the load bias; glibc's own words are only tested for non-zero, a program's absolute symbol read through the GOT is wrong (lld and mold write the value, no record) | `plumb.md` §10.2 A30; `crates/mortar_plumb/tests/spie.rs` (`absgot.c` exits 1) |
| A31 | GNU ld (x86-64) | static-pie: the `.plt.got` entry of an undefined weak function (`__cxa_finalize`, a `_ZTH` init guard) jumps through its GOT slot + 1, a misaligned word no record writes; dead code, each caller tests the slot for 0 first | `plumb.md` §10.2 A31 |
| A32 | GNU ld, mold (x86-64) | W3 in a static-pie: an IFUNC's address word is its own `IRELATIVE` slot while a PC-relative use takes the IPLT entry, so `fp == f` is false (lld right; on AArch64 every use of all three is the resolved function) | `plumb.md` §10.2 A32; `tests/link/dyn/spie/ifunc.test` |
| A33 | mold | a shared object named on a `-static-pie` line (after the driver's `-Bstatic`) is linked: `DT_NEEDED`, no `PT_INTERP`, nothing loads it and the call faults (SIGSEGV, both architectures); GNU ld and lld refuse, as D-19 and D-2 do | `plumb.md` §10.2 A33; `tests/link/dyn/spie/shlib-refused.test` |
| H1 | lld, mold | (hazard, not a wrong output) `IRELATIVE` before `JUMP_SLOT`s: a resolver calling through the PLT crashes | `reloc/run.sh` (ifh_plt) |

Every one is a test of §16 on which mortar must refuse or be right.

## 14. Hooks for M6 (metadata, ODR) and M7 (incremental)

Not designed here. These are **constraints** the later milestones must
work within, stated so that M2's rules do not close the door.

- **Stable indices.** Every dynamic order is a function of `SymbolId` and
  `FileId` (D-22, D-27, D-40, D-41, D-42) or of the command line (D-19a),
  never of addresses or hash iteration, so an incremental relink (M7)
  that keeps ids keeps indices.
- **M7: `.rela.dyn` slack must be `RELATIVE`.** D-40's `RELATIVE` prefix
  is contiguous and counted by `DT_RELACOUNT`; glibc applies the first
  `DT_RELACOUNT` records as `RELATIVE` without reading their type, so
  slack inside the prefix cannot be `R_*_NONE`. Slack there must be
  `RELATIVE` records aimed at a reserved scratch word (counted in
  `DT_RELACOUNT`); `NONE` slack can only follow the prefix. Adding a
  `RELATIVE` otherwise shifts every later record.
- **M7: `.dynsym` slack must be unhashed tail slots or bucket capacity.**
  D-22's bucket-sorted order renumbers every hashed entry, and so every
  symbolic record's `r_sym`, when the export set changes. Room to append
  is either D-28's bucket capacity or reserved unhashed slots (with
  `symoffset` fixed); both are off at M2 (sizes exactly D-26). D-30's
  five spare `DT_NULL`s let a relink add a tag in place.
- **M7: per-section record slices.** D-40's `RELATIVE` region is a
  concatenation of per-section slices, the unit M7 replaces.
- **M6: D-21 is not closed.** No reference exports an executable's
  vague-linkage or `STB_GNU_UNIQUE` definitions that nothing dynamic
  references, so a `dlopen`ed library gets its own instance (all three:
  `review-m2-adv/unique`, `counter=1` in both, two addresses). A load-time
  ODR check needs one more export class (for example "COMDAT-key and
  unique definitions recorded in an M6 table"); D-21's list is the M2
  set and gains classes by amendment, not by exception. D-21 is unchanged
  at M2; `spec/odr.md` Q8's class, if accepted, is added there by
  amendment.
- **M6: ODR data.** `.note.mason` (`spec/odr.md` O-52) is an allocated
  note of alignment 8, placed with the other notes (layout L-60 rank 2),
  written only with `--odr-table`; O-24 reads the tables of
  shared-object inputs that stay in `DT_NEEDED`, identifying rows by
  (name, version node). No runtime is added in M6 (`spec/odr.md` O-58).
  Implemented (WP-075): `--odr-table=exported` writes it in a dynamic
  executable (synthetic chunk 26, `spec/synth.md` S-3), in a `PT_NOTE`
  of alignment 8 after the 4-aligned notes' one; `eu-elflint` names its
  type as unknown (`spec/odr.md` O-52), so D-71's "no allowance" holds
  for an output without the table. O-24 reads input tables under
  `--odr=strict` only.

## 15. Open questions (each with a recommendation)

Every question is closed (supervisor's rulings, written into the text by
WP-040r; Q1 measured and written by WP-043).

- **Q1. Closed by measurement (WP-043).** Can the references of an
  `--as-needed` library extract archive members? Measured (D-12's table):
  GNU ld and mold do not extract for an unneeded as-needed object, lld
  does; all three extract for a needed one when the archive follows it.
  *Rule* (D-12): the references of a command-line shared object whose
  merged `as_needed` is off are edges; those of an as-needed one (and of
  every closure file) are not, and an as-needed object found needed with
  a strong reference that only an unextracted member would define is
  refused by name (`archive member needed by an --as-needed shared
  object`). Decided at load time, not circular.
- **Q2.** Withdrawn by the review (F2): `WeakDyn` is decided per symbol
  (D-25); there is no per-class split.
- **Q3. Closed: off.** `--eh-frame-hdr` off by default (GNU ld, lld) or on (mold)?
  Without it a C++ throw aborts in a dynamic program. *Recommendation:*
  off, as GNU ld; every driver passes it (426/426 lines). *Ruled as
  recommended*: §2.3, synth S-57.
- **Q4. Closed: refused at M2.** x86-64 IBT (`-z ibt`, `.plt.sec`). *Recommendation:* refused at
  M2; a package of its own when a census line passes `-fcf-protection`.
  *Ruled as recommended*: D-49.
- **Q5. Closed: kept.** `.relro_padding` draws an elflint complaint. *Recommendation:*
  keep it (model §4.4; it is what makes RELRO end on maxpage cheaply);
  add the message to the allow-list of `elflint-dynamic`. *Ruled as
  recommended*: D-71. Superseded in part by WP-083: the chunk is kept and
  renamed `.pad.relro` (layout L-156), which elflint does not flag; the
  allow-list is gone.
- **Q6. Closed: 0, as lld.** Debug tombstone in a PIE (image I-47). *Recommendation:* 0, as
  lld, for every `.debug_*` but `.debug_ranges`/`.debug_loc` (1).
  *Ruled as recommended*: D-72, image I-47 (applied to I-47's open item,
  the rows that write 0; its all-ones rows and the `.debug_aranges`
  empty tuple are unchanged).
- **Q7. Closed: `S+A`.** synth S-69 writes the IPLT word 0; a64 §6.2 and §10.2 say `S+A`
  (M1 contradiction). *Recommendation:* a64's `S+A` (GNU ld's bytes; glibc
  ignores it under RELA), fixed by `mortar_synth` before the M2 baseline is cut,
  because it changes static outputs' bytes. *Ruled*, per architecture
  as WP-041 implements it: AArch64 the resolver (`S+A`, a64.md §6.2's
  rule: glibc ignores the word under RELA and reads the addend, and the
  verifier accepts it); x86-64 0 (x64.md X-39; plumb F5(3)); i386 the
  resolver (REL: the word is the addend). synth S-69 and §13 state it;
  WP-041 changes the AArch64 bytes. The rule rests on a64.md, not on
  GNU ld: "GNU ld's bytes" above is wrong (WP-041 measured GNU ld 2.44,
  AArch64 static, writing the start of `.plt` into the word, with the
  resolver in `r_addend`; on x86-64 it writes the entry + 6).
- **Q8. Closed: M4.** layout §1 / L-100 and synth S-3 row 8 tag shared-object output
  and verdefs M2. *Recommendation:* the ROADMAP stands (M4); R-L3, R-S4.
  *Ruled as recommended*: layout §1, L-100; synth S-3 row 8.
- **Q9. Closed: M2 for the option.** `--export-dynamic-symbol`: M2 or M4? *Recommendation:* M2 for
  the option (one more source of `exported`, D-21), M4 for
  `--dynamic-list` files. *Ruled as recommended*: resolve §1.2, §9.6;
  cli §4.2.

## 16. Tests

Each test links with mortar and with GNU ld (the oracle of record),
runs the output (natively on AArch64 and on the hosted x86-64 lane,
`qemu-x86_64` for x86-64 on the AArch64 host), and
requires the verifier clean; tests marked (refuse) require mortar's named
refusal. "W" links to §13.2.

| area | tests |
|---|---|
| kinds | `output-kind-table`, `no-dynamic-linker-refused` (refuse) |
| shared inputs | `shlib-hidden-dynsym` (refuse; W1), `as-needed-weak`, `as-needed-closure`, `closure-export` (W8), `closure-not-on-command-line` (refuse), `shlib-undefined-missing-closure`, `version-needs-unchecked`, `copy-dt-needed-refused` (refuse), `tls-mismatch-shared` (refuse; W14), `protected-copy`, `protected-canon-plt` (refuse; W5), `needed-twice` (W9), `needed-order`, `shlib-undefined-ldso`, `used-underlinks-refused` (refuse; W16), `needed-order-as-needed-twice`, `needed-order-group`, `needed-order-dup-def` (N1; `shlib/dupdef_needed.sh`, `review-m2-adv/dupdef`), `underlink-after` (in `needed-order`; N5) |
| `.dynsym` | `dynsym-set`, `dynsym-order`, `dynsym-fields`, `environ-aliases` (W2), `weak-undef-pie`, `gnu-hash-sizes`, `gnu-hash-lookup`, `versions-verneed`, `versioned-export-refused` (refuse), `hash-style-sysv-refused` (refuse) |
| `.dynamic` | `dynamic-tags`, `init-fini-option`, `variant-pcs-plt`, `got-header`, `dynamic-writable`, `dyn-shdr`, `interp-default` (W10) |
| relocations | `rela-dyn-order`, `irelative-resolver-calls-plt` (H1), `copy-reloc`, `copy-zero-size` (refuse; W4), `copy-tls-refused` (refuse; W14), `copy-in-pie-x86-64`, `canonical-plt-equality`, `canon-plt-in-pie-refused` (refuse; W11), `ifunc-equality-pie` (W3), `textrel-refused` (refuse; W6), `textrel-ro-copy-pie` (refuse; W7), `tls-shared-ie`, `tls-exe-le`, `tls-gd-unrelaxed` (W15), `tls-desc-large`, `plt-lazy`, `plt-now`, `copy-typeinfo-relro`, `weak-undef-hook-preload`, `weak-undef-pc-pie-refused`, `weak-mixed-exec` (W12), `weak-data-init-exec` (W12), `weak-class-table` (D-25's table), `ifunc-from-dso` (W13), `mixed-mode` (the `corpus/mixed.sh` mixes), `norelro-dynamic` |
| layout | `dyn-section-order`, `dyn-segments`, `relro-maxpage`, `pure-function` (link twice with a perturbed address input; dynamic sizes equal) |
| tools | `gdb-break-main`, `lldb-break-main`, `ldd-lists`, `elflint-dynamic` |
| end to end | the default line of every driver of §2.1 for AArch64 and x86-64 (C, C++, Fortran; PIE and `-no-pie`; `-z now`; `-rdynamic`; `-static-libstdc++`) and kiln's libc++ lane |
| static | every M1 test byte-identical (the guard of §D) |

## 17. Shared-object output (`-shared`) [M4]

Designed by WP-110 (record `spec/landed/dyn/WP-110.md`; its §A is the
census, its §D the cut into packages). Every rule here is `[M4]`; the
M2 rules of §4-§10 apply to `Shared` unless a rule below replaces them
for that kind. Measured with GCC 14 objects, GNU ld 2.44, lld 19.1.7 and
22.1.8 (identical in every block measured) and mold 2.37.1 on AArch64,
x86-64 and i386, each library loaded and run by a host program
(`DT_NEEDED` and `dlopen`); the experiments are `E-shared-*` of
Appendix A, and `[A]`-`[F]` below cite the `dyn` area's parts. The
second design pass (after the adversarial review) re-measured what it
changed with the scripts of `tests/shared-design/exp2/` (README there;
cited `exp2/<dir>`), on AArch64 natively and x86-64 under qemu-user.
The third pass (after the re-review) measured D-117's x86 scope,
boundary rule and analysis, version-script syntax and rule 12, and
`-shared -static` with the scripts of `tests/shared-design/exp3/`
(README there; cited `exp3/<file>`).

### 17.1 The kind, the entry, `.dynamic`

**D-100 [M4] (the `Shared` column).** D-1's table gains a column; other
specs cite it as `Shared` (arch.md §2.1's kind):

| property | `Shared` |
|---|---|
| line | `-shared` (`-Bshareable`), by cli §3.2 unchanged (the last of `-shared`/`-pie`/`-no-pie` wins; `-r` with it is `conflicting-options`). `-dynamic-linker` with `-shared`: accepted, no `PT_INTERP` [unmeasured; no driver passes both, record §A]. `-shared -static`: gcc's driver turns it into `-Bstatic -shared … crtbeginT.o … --start-group -lgcc -lgcc_eh -lc --end-group` (no `--eh-frame-hdr`), and GNU ld's own `-static` is `-Bstatic`: `ld -shared -static s.o` and `ld -shared s.o -Bstatic -lc` behave alike (`exp3/ss`). So mortar does not refuse it by name (it cannot tell it from a legitimate `-Bstatic`): `-static` is `-Bstatic` on a `-shared` line too, archives only, and the members it pulls are judged by D-117, D-118 and D-122 like any input. Measured (AArch64): an object that needs nothing from `libc.a` links with all three (a `Shared` output with no `DT_NEEDED`), and so with mortar; one that calls `printf` pulls `libc.a(dl-support.o)`, whose non-PIC reference to `_start` GNU ld refuses (`undefined reference to _start`), and lld and mold refuse the LE TLS of `libc.a(malloc.o)` (`R_AARCH64_TLSLE_ADD_TPREL_*`), as mortar does (D-119; D-117 for `dl-support.o`) (`exp3/ss/run.sh`); on x86-64 GNU ld refuses `crtbeginT.o`'s `R_X86_64_32` (the re-review's `w28/`), as D-117 does. On i386 GNU ld links it and the C++ library aborts on a throw, mold links it and the C library crashes (W28, W29: WP-130's to reduce). `-shared -static-pie` links nowhere (gcc drops `-shared`; clang's line has no `_start`) |
| `e_type` | `ET_DYN` (all three, [A]) |
| base (layout L-68) | 0; `--image-base=N` sets it (GNU ld and lld; mold ignores it silently in `-shared`, [A]); `-Ttext-segment` stays refused (lld refuses it, mold fails on it) |
| `.interp`, `PT_INTERP` | no (all three) |
| `PT_PHDR` | no (GNU ld; lld and mold write one; glibc's `dlopen` reads the headers from the file and needs none, every GNU ld library of E-shared runs). layout L-67's "iff `PT_INTERP`" stands |
| `.dynamic`, `PT_DYNAMIC`, `_DYNAMIC` | yes; `_DYNAMIC` local and hidden (D-32) |
| `DT_NEEDED`, `DT_SONAME` | `DT_NEEDED` by D-118; `DT_SONAME` iff `-soname`/`-h` (all three) |
| `.dynsym` | D-110, D-111 |
| `DT_DEBUG` | never (all three) |
| `DT_FLAGS`, `DT_FLAGS_1` | only from options and `DF_STATIC_TLS` (D-103, D-119); no `DF_1_PIE` |
| `RELATIVE` | every absolute word (GOT slots included) whose target is a non-preemptible definition of the library (D-112); never one of an `SHN_ABS` symbol (D-46 row `Abs`) |
| symbolic records | every word against a preemptible definition of the library or an import (D-117) |
| `COPY`, canonical PLT | never; the uses that would demand them are refused (D-122) |
| `IRELATIVE` | last in `.rela.plt` (D-41 unchanged, D-121) |
| text relocation | refused (D-47 unchanged; D-117) |
| TLS | the input's model, no relaxation (D-119) |
| verifier (plumb) | stage 3, extended by §18.3's requests |

*Test:* `output-kind-table` gains the column (one C library per
architecture, `readelf -hld` against it).

**D-101 [M4] (entry).** layout L-102 applies to `Shared` as written: the
entry symbol is `-e`'s, else `_start`. When it is defined, `e_entry` is
its address; when it is not, `e_entry = 0` and nothing is reported for
the default `_start` (a library usually has none), while an undefined
symbol named by `-e` keeps L-103's warning `entry-not-found`. All three
write `&_start` when the library defines `_start` (GNU ld and lld warn
"cannot find entry symbol nosuchsym; not setting start address" for
`-e nosuchsym` and write 0; mold is silent), [A]. L-103's last sentence
("with `-shared` … `e_entry = 0`, silently") is replaced by this rule
(request R-L11); L-104's carve-out for `e_entry = 0` stays. *Test:*
`shared-entry` (no `_start`, `_start`, `-e f`, `-e nosuchsym`).

**D-102 [M4] (`.dynamic` of a shared object).** D-30's table, in its
order, with these changes for `Shared` ([B]: every tag below is
written under the same condition by all three; the order is GNU ld's
as in D-30):

| D-30 row | change for `Shared` |
|---|---|
| after 1 | `DT_SONAME` iff `-soname` (the name as given), then `DT_SYMBOLIC` iff the line's last `-Bsymbolic` form is `-Bsymbolic` and it has no `--dynamic-list` (D-112; GNU ld; lld writes `DF_SYMBOLIC` only, mold neither) |
| 2 | `DT_RUNPATH`/`DT_RPATH` unchanged (after `DT_SONAME`/`DT_SYMBOLIC`, GNU ld and mold; lld puts it first) |
| 3-4 | `DT_INIT`/`DT_FINI`: an explicit `-init`/`-fini` naming a symbol that is not defined is refused by name, `undefined-init-symbol` (all three silently omit both tags and the library's `_init`/`_fini` never run, [B] `b_init_undef`). Without the option, an undefined default `_init`/`_fini` (`-nostdlib`, no `crti.o`) omits the tag silently, as D-30 rows 3-4 |
| 5-6 | `DT_PREINIT_ARRAY*`: a `.preinit_array` in a shared object is refused by name, `preinit-array-in-shared-object` (glibc runs pre-initialisers of the executable only) [unmeasured on the references] |
| 16 | `DT_DEBUG`: never |
| 26-27 | `DT_FLAGS`/`DT_FLAGS_1`: D-103 |
| after 27 | `DT_VERDEF`, `DT_VERDEFNUM` iff `.gnu.version_d` exists (D-115), before `DT_VERNEED` (GNU ld) |
| 28-30 | `DT_VERSYM` also when only `.gnu.version_d` exists |
| 32 | 6 `DT_NULL`, unchanged |

`DT_TEXTREL`, `DT_TLSDESC_PLT`/`GOT` (TLS descriptors are eager, D-119;
GNU ld writes them, lld and mold do not), `DT_HASH` (D-20 stands: the
drivers pass `--hash-style=gnu`; `sysv` and `both` stay refused, record
§A), `DT_AUXILIARY`/`DT_FILTER` (`-f`/`-F`; no census line) and
`DT_RELR*` (M4, its own package) are not written. `DT_AARCH64_BTI_PLT`
and the BTI property are kept as D-30 row 24 (GNU ld and lld; mold
drops both, [A]). *Test:* `dynamic-tags` gains the `Shared` rows,
each architecture.

**D-103 [M4] (`DT_FLAGS`, `DT_FLAGS_1` options).** All three write the
same flags for each option, on every architecture, and the loader acts on
them identically ([B]: 12 of 12 runs per option):

| option | `DT_FLAGS` | `DT_FLAGS_1` | measured effect |
|---|---|---|---|
| `-z now` | `DF_BIND_NOW` | `DF_1_NOW` | a missing function fails `dlopen` with `RTLD_LAZY` too |
| `-Bsymbolic` (the line's last `-Bsymbolic` form, without `--dynamic-list`, D-112) | `DF_SYMBOLIC` | — | the library binds its own definitions |
| `-z nodelete` | — | `DF_1_NODELETE` | stays mapped after `dlclose` |
| `-z nodlopen` | — | `DF_1_NOOPEN` | `dlopen` fails, "shared object cannot be dlopen()ed" |
| `-z initfirst` | — | `DF_1_INITFIRST` | its constructors run first |
| `-z interpose` | — | `DF_1_INTERPOSE` | none as `DT_NEEDED`; under `LD_PRELOAD` as preloaded |
| `-z origin` | `DF_ORIGIN` | `DF_1_ORIGIN` | — |
| IE TLS present (D-119) | `DF_STATIC_TLS` | — | — |

`-Bsymbolic-functions` writes no tag (all three). Every one of these
options is `supported` with its `Shared` row only; with an executable
kind each keeps its M2 status (cli.md, request R-C10). `-z now` and
`-z norelro` behave as in D-49 and D-33. *Test:* `shared-dt-flags` (the
table, run).

### 17.2 Exports, preemption, versions, relocations

`[X-A]`-`[X-G]` cite the `export` area's parts (E-shared-export).

**D-110 [M4] (the export set).** A shared object exports every key
whose `def` is a definition of a live relocatable object with
`STV_DEFAULT` or `STV_PROTECTED` visibility (resolve §9.6), except one:

1. made local by a version script (D-114) or by `--exclude-libs`
   (defined by a member of a named archive, or of any archive for `ALL`;
   all three, [X-A], [X-G]);
2. linker-defined (file 0): exported only when `ref_dynamic`, as D-21
   item 2 says for executables; no reference exports `_end`, `_edata`
   or `__bss_start` in `-shared` ([X-A] A1); layout L-100's "exported
   `STB_GLOBAL STV_DEFAULT` when defined (GNU and lld)" is withdrawn
   (request R-L10). **Except** `__start_NAME` and `__stop_NAME` of a
   C-identifier output section that a live object references: exported
   `STB_GLOBAL STT_NOTYPE STV_PROTECTED` (GNU ld and lld, AArch64 and
   x86-64, with and without `--gc-sections`; mold exports neither, a
   difference, 17.6; `exp2/startstop`). Protected, they are never
   preempted: an `LD_PRELOAD` definition of both leaves the library's
   count unchanged with every reference (`count=1`). This agrees with
   layout L-96, which roots the section for gc and says nothing of
   export, and with D-124 item 4: the section is kept by the reference,
   not by the export, so a version script that makes the two symbols
   local (item 1) leaves the section in place and the symbols defined
   (lld's W23 drops it);
3. a version node's name: GNU ld exports each node name as an `SHN_ABS`
   `STT_OBJECT` entry (`V1@V1`; the system's `libz.so` has them); lld
   and mold do not and every library runs alike (zlib 48 cross-runs):
   mortar does not;
4. never: local, hidden, internal symbols; section symbols (GNU ld's
   AArch64 `STT_SECTION` entries); `_DYNAMIC`, `_GLOBAL_OFFSET_TABLE_`,
   `__dso_handle`; the hidden helpers of `libgcc.a`
   (`__aarch64_ldadd4_acq_rel`, `__popcountdi2`, `__udivdi3`: all three).

This includes weak definitions (`STB_WEAK` kept), C++ vague linkage
(inline functions `WEAK FUNC`, vtables and typeinfo `WEAK OBJECT`) and
`STB_GNU_UNIQUE` definitions (template and function-local statics; kept
unique, D-23), every member extracted from an archive whole (not only
the referenced definitions), and `main` when a library defines it
(googletest's `gtest_main`). `--export-dynamic`/`-E` and
`--export-dynamic-symbol` change nothing in `-shared`; `--dynamic-list`
changes preemption only (D-112) (all three, [X-A] A2). The set equals
every reference's on every library of the census (gtest: 3840 defined,
zlib, Lua) but for GNU ld's items 3 and 4. *Tests:* `shared-export-set`
(each item, each architecture), `shared-exclude-libs`.

**D-111 [M4] (`.dynsym` of a shared object: imports, order, fields).**
Besides the exports, `.dynsym` holds:

1. **imports**: every key that a live relocatable object references and
   whose `def` is `Shared` (D-21 item 1);
2. **undefined at link time**: every key strongly referenced by a live
   relocatable object with no definition (D-118: allowed without
   `-z defs`), `STB_GLOBAL`, `STT_NOTYPE`, `SHN_UNDEF`;
3. **weak undefined**: every weak undefined key that a live relocatable
   object references, whatever the class, `STB_WEAK STT_NOTYPE`
   `SHN_UNDEF` (all three, [X-A] A1: called, through the GOT, in a data
   word, call-only). D-25's `Weak0` does not exist in `Shared`: a use
   that would need the value 0 at link time (a PC-relative form) is
   refused by D-117.

No copy aliases and no canonical PLT entries (D-122). Order: D-22
(unhashed undefined entries by `SymbolId`, then the hashed entries by
(bucket, `SymbolId`); `sh_info` 1); `.gnu.hash`: D-26 (lld's geometry;
GNU ld and mold use their own, all valid, [X-A] A3; `sh_entsize` 4 on
ELF32 as D-34, which GNU ld writes and lld and mold do not in
`-shared`). Fields: D-23's rows; an import whose shared definition is a
function gets `STT_FUNC`, and keeps the binding of the references to it:
`STB_WEAK` when every reference is weak (crtbeginS's `__cxa_finalize`:
`WEAK FUNC` with every reference, AArch64, x86-64 and i386, 9 census
libraries; the first pass's "GNU ld `NOTYPE`" did not reproduce), else
`STB_GLOBAL`; a weak undefined key with no shared definition is item
3's `WEAK NOTYPE` (`__gmon_start__`, `_ITM_*`); an export keeps its
`.symtab` type and binding, an exported preemptible IFUNC
`STT_GNU_IFUNC` at its resolver (D-121).
`.gnu.version` of an unversioned import is 1 (GNU ld, mold, lld 19;
lld 22 writes 0 for weak ones, [X-F] F4; glibc accepts both). *Tests:*
`shared-dynsym-set`, `dynsym-order` and `gnu-hash-lookup` gain the
`Shared` rows.

**D-112 [M4] (preemption).** An exported definition is **preemptible**
(its uses go through a record the loader binds, so an executable, an
earlier library or `LD_PRELOAD` can interpose it) or **bound** (inside
the library at link time). `STV_PROTECTED` and version-local (D-114)
definitions are bound on every line; every other exported definition
follows this table, by the line's options and the key's type
(`exp2/bsym2`: 14 definitions, one per type × binding × visibility,
interposed by `LD_PRELOAD`; AArch64 natively, x86-64 under qemu; 17
lines × 3 references × 2 targets; the full answers per reference and
cell are in `exp2/bsym2/result.txt`, summarised in the record §F.2).
`P` preemptible, `B` bound; a weak definition is its type's column
(measured: every weak cell equals its global cell, all three):

| line (in order) | `FUNC` | `OBJECT` | `COMMON` | `NOTYPE` | `TLS` | `GNU_IFUNC` | the references |
|---|---|---|---|---|---|---|---|
| none | P | P | P | P | P | P | GNU ld, lld; mold binds `GNU_IFUNC` (W20) |
| `-Bsymbolic` | B | B | B | B | B | B | all three |
| `-Bsymbolic-functions` | B | P | P | P | P | P | lld; GNU ld also binds `NOTYPE`; mold also binds `GNU_IFUNC` (W20) |
| `--dynamic-list=F`, key not in F | B | B | B | B | B | B | lld on AArch64; GNU ld keeps `TLS` and `GNU_IFUNC` `P`; lld on x86-64 binds `TLS` half (W43); mold |
| `--dynamic-list=F`, key in F | P | P | P | P | P | P | GNU ld, lld; mold binds `GNU_IFUNC` (W20) |
| `--dynamic-list` with `-Bsymbolic` or `-Bsymbolic-functions`, either order | as `--dynamic-list` alone | | | | | | GNU ld (both forms), lld (`-Bsymbolic-functions`); lld binds every key with `-Bsymbolic` and writes `DF_SYMBOLIC` (W31); mold ignores `-Bsymbolic`, follows the list for `-Bsymbolic-functions` then `--dynamic-list`, and for `--dynamic-list` then `-Bsymbolic-functions` makes every data key `P` (`OBJECT`, `NOTYPE`, `TLS`), the listed function `P` and `GNU_IFUNC` `B` (`exp3/bsym3`, the re-review's `bsym/` on AArch64) |
| `--dynamic-list` then `-Bno-symbolic` | as `--dynamic-list` alone | | | | | | GNU ld, lld; mold makes every key `P` |
| `-Bsymbolic -Bno-symbolic`, `-Bsymbolic-functions -Bno-symbolic` | P | P | P | P | P | P | all three |
| `-Bno-symbolic -Bsymbolic`, `-Bsymbolic-functions -Bsymbolic` | B | B | B | B | B | B | all three |
| `-Bsymbolic -Bsymbolic-functions` | as `-Bsymbolic-functions` | | | | | | all three (the last form wins) |
| `STV_PROTECTED` (any line) | B | B | — | — | B | B | all three |
| `--dynamic-list-data`, `--dynamic-list-cpp-new`, `--dynamic-list-cpp-typeinfo` | refused by name | | | | | | GNU ld accepts all three (`-data` acts as `-Bsymbolic-functions` binding `NOTYPE` too; `-cpp-*` as a list of `operator new`/`delete` or typeinfo); lld refuses all three (`unknown argument`); mold accepts `-data` and changes nothing, refuses `-cpp-*` |
| `-Bsymbolic-non-weak-functions`, `-Bsymbolic-non-weak` | refused by name | | | | | | GNU ld does not know them (lld and mold accept) |

In words: without `--dynamic-list`, the last of `-Bsymbolic`,
`-Bsymbolic-functions` and `-Bno-symbolic` decides (`-Bsymbolic` binds
every type; `-Bsymbolic-functions` binds `STT_FUNC` only, lld's rule;
`-Bno-symbolic` none). With a `--dynamic-list`, the list decides alone
(every unlisted key bound, of every type, lld's rule; every listed key
preemptible) whatever `-Bsymbolic` form comes before or after it (GNU
ld's rule; mold's order dependence is in 17.8). `DT_SYMBOLIC`/
`DF_SYMBOLIC` are written iff the line has no `--dynamic-list` and its
last form is `-Bsymbolic` (D-102, D-103; request P-R7): glibc applies
them to every symbol of the library, so with a list they would bind the
listed keys too (W31); with every key bound at link time they change
nothing and tell a reader of the library (D-126 item 2) that it is
symbolic. A key bound by the table is `Local` (or `Ifunc`, `Abs`) for
the relocation rules (D-113), a preemptible one `Preemptible` (request
R-A10); a bound `STT_TLS` key's GD pair is one `DTPMOD` with `r_sym` 0
and a static offset (D-119), never lld's x86-64 mixture (W43).

Everything else exported is preemptible: default visibility, weak
definitions, IFUNCs (D-121), and a definition that a needed shared
input also defines (the library's own wins at link time, even weak over
the shared strong, and stays preemptible: with the other library first
in the loader's order its definition is used by the library too; all
three, [X-G] G2). A data definition that some input binds without a
record (D-117's address-level rule, non-PIC code) is not made bound: the
link is refused. The input property
`GNU_PROPERTY_1_NEEDED_INDIRECT_EXTERN_ACCESS` (`-mno-direct-extern-access`,
`-z indirect-extern-access`) is kept in the output when any input has
it, as GNU ld (lld and mold drop it, and a copy-relocating executable
then runs with two instances of protected data where glibc refuses GNU
ld's library: W41; glibc 2.41 refuses a protected `COPY` at run time only
when the library has the property, and otherwise warns ``copy
relocation against non-copyable protected symbol`` and runs with two
instances, `exp2/escape/run2.sh`) [x86-64 and i386 measured; the
AArch64 property unmeasured]. *Tests:* `shared-preemption` (the table:
every row and column, run under `LD_PRELOAD`, both targets),
`shared-protected` (executables against a protected definition:
`-fPIE` GOT ones run, COPY and canonical PLT ones refused, D-18),
`dynamic-list-symbolic` (W31), `dynamic-list-tls-x64` (W43).

**D-113 [M4] (where preemption lives).** Preemption is a property of a
key computed by resolution after the export set, before the relocation
scan (resolve §9.6), stored per key in `Resolution` (request R-M10:
model.md gains a `preemptible` bit per key, `false` for every key of an
executable). `target_kind` (arch.md §2.1) reads it: `Preemptible`
replaces arch §2.1's kind for an exported preemptible definition; a
definition that is not preemptible keeps §2.1's kind (`Local`, `Abs` for
an `SHN_ABS` key: never a `RELATIVE`, D-100; `Ifunc` for a
non-preemptible IFUNC: its IPLT entry, D-121); an import is `Shlib`; a
strong key that nothing defines is `Shlib{func: false}` (D-111 item 2:
it is reached exactly as an import, through a record naming it); every
weak undefined key is `WeakDyn`. Nothing else in the pipeline asks
whether the output is `Shared`.

**D-114 [M4] (version scripts applied).** resolve.md §9.7 is the rule,
restated by WP-110's third pass from two executed models
(`exp3/vs3`: `model_a.py` and the re-reviewer's `model_r.py`, both
corrected; 213 scripts, 2,249 (script, name) rows: the second pass's 53,
the re-review's 60 and this pass's 100 on glob syntax, duplicates and
`.symver` with and without `--export-dynamic`; in GNU mode both equal
GNU ld 2.44 on all 213; lld and mold differ where 17.6 says). mortar is
GNU ld wherever the references differ, with one deliberate exception
(`local:` before `global:`, below), and these rulings for `Shared`:

- matching: exact names first; then non-star globs, where a `global:`
  glob match in any node beats a `local:` one and, among `global:`
  glob matches, the last node wins; then `*` (`global: *` over `local:
  *`) (resolve §9.7's decision). The two scripts on which GNU ld and
  mold give V1 and lld local (`V1 { global: fo*; }; V2 { local: foo*;
  };` and the same with `local: *` in V1) follow GNU ld: a recorded
  difference;
- an entry is a glob iff it is unquoted and holds an unescaped `*`, `?`
  or `[`; quoted entries are exact, `"foo*"` included (GNU ld; lld and
  mold glob them); an unquoted exact entry loses its backslashes
  (`fo\o` is `foo`, `fo\*` is `fo*`; lld and mold keep the backslash of
  `fo\o`); globs
  are `fnmatch` without flags (resolve §9.7 "Entries", measured
  `exp3/vs3/globs.py`: `[!f]` and `[^f]` negate, `]` first in a class is
  a member, a `[` with no closing `]` is a plain `[`, a glob ending in a
  lone `\` matches nothing; lld matches nothing for `[!]]`; lld and mold
  refuse an unterminated `[`; mold reads `[!f]` as a class holding `!`);
- an exact name under `global:` in two nodes: the first, warning
  `version-reassigned` (GNU ld silent, lld warns, mold the last);
- the same entry under `global:` in one node and `local:` in another:
  refused, `duplicate-version-pattern`, as GNU ld (lld and mold accept
  with differing answers). Two entries are the same when they have the
  same language and are both exact with the same name (`foo`, `"foo"`
  and `fo\o` are one; `"fo*"` and `fo\*` are one) or both globs with
  the same text (`fo*` and `fo**` are not; `*` and `*` are)
  (`exp3/vs3/dups.py`, 17 cases);
- `local:` before `global:` in one node: accepted (lld, mold; GNU ld
  refuses with a syntax error); the `global:` entries still count first;
- a name in a script that no file defines: **accepted with the warning
  `version-script-undefined-name`** (GNU ld accepts silently; zlib's
  `configure` probe links `zlib.map` against one function, so under
  lld's refusal Clang+lld builds no shared zlib, record §A);
  `--no-undefined-version` refuses with `undefined-version` (GNU ld:
  `nosuch: undefined version: V1`); `--undefined-version` is a no-op;
- several parents (`V3 { … } V1 V2;`): accepted, every parent written
  (GNU ld; lld and mold refuse the syntax); a forward or missing parent,
  a node name used twice, an anonymous node with named ones: refused
  (GNU ld; script.md §9.1);
- several `--version-script` files: one script, in order (all three);
- comments: `#` to the end of the line and `/* */` (all three);
  `//` refused (all three; script.md §9.1);
- `extern "C++"` patterns need the demangler (diag.md §9): until it
  exists a script with `extern "C++"` is refused by name; `extern "C"`
  is plain names. `extern "C++" { *; }` with `local: *` exports every
  symbol (GNU ld, lld; mold exports none, W40);
- an anonymous script (`{ global: a; local: *; };`) exports the listed
  keys unversioned and writes no `.gnu.version_d` (all three);
- a `.symver` definition (`foo@V1` or `foo@@V1`) is matched by node V1
  only, and is local exactly when one of V1's `local:` entries matches
  `foo`, none of V1's `global:` entries (exact or glob) matches it, and
  the line has no `--export-dynamic` (resolve §9.7 rule 12; GNU ld,
  `exp3/vs3/expdyn.py`, `atat.py`, the re-review's s03, s04, s06, s09).
  mortar drops it as GNU ld, with the warning
  `versioned-definition-hidden` (an executable linked against the
  earlier library then fails to load, ``undefined symbol: foo, version
  V1``: the warning says so). lld hides `foo@V1` also when a `global:`
  glob of V1 matches and ignores `--export-dynamic`; mold never hides
  it (17.8). Q12 is closed this way (the re-review's M-3);
- `--export-dynamic` (`-E`) on a `-shared` line changes nothing but
  rule 12 (GNU ld: with it a `.symver` definition stays exported; the
  measured exact, glob, `*` and other-node `local:` entries
  hide unversioned keys with and without it, `exp3/vs3/expdyn.py`); lld
  and mold ignore it in `-shared`;
- `foo@V`/`foo@@V` with no node `V` in the script, or with no script:
  refused, `undefined-version` (all three, [X-F] F2; resolve §9.4
  confirmed; D-27's note on GNU ld writing a verdef holds for
  executables only).

The script grammar is script.md §9. *Tests:* `version-script-rules`
(every one of the 213 scripts of `exp3/vs3/cases.py`, with and without
`-E` where it has `.symver` definitions, each with its `readelf
--dyn-syms -V`; the models' mortar column is the expectation: the
second pass's 53, the re-review's 60 (r35 `[^f]*`, r50 `fo\*`, s03,
s04, s06, s09 included), the 43 glob patterns, the 17 duplicate cases,
the `.symver` cases with `@` and `@@`),
`version-script-zlib` (zlib.map; the 48 cross-runs),
`version-script-undefined-name`, `version-script-comments`,
`symver-old-and-default` (an old and a new executable run against one
library), `symver-hidden-by-own-node`.

**D-115 [M4] (`.gnu.version_d`).** Written iff a version script names at
least one version node (or a `.symver` definition names one). Entries,
in this order: index 1, `VER_FLG_BASE`, named `DT_SONAME`'s string, else
the output file's basename (GNU ld and mold; lld writes the path as
given, [X-F] F3); then one per node in script order, indices 2…,
`vd_flags` 0, `vd_hash` the ELF hash of the name, `vd_cnt` 1 + the
number of parents, the node's `Verdaux` first and its parents' after it
in the order written (GNU ld; lld and mold write no parents; glibc
ignores them; resolve §9.7's last paragraph). `Verdef` and its `Verdaux`
entries are interleaved, each `Verdef` followed by its own. `.gnu.version`
of an export: the node's index for `foo` or `foo@@V`; for `foo@V`
(non-default) the index with `VERSYM_HIDDEN` (0x8000); 1 for an export
matched by no node; the `Vernaux` indices (`vna_other`) continue after
the last `Verdef` index [unmeasured beyond `readelf -V` of [X-F]'s
outputs, which glibc loads]. Section header: `SHT_GNU_verdef`, `A`,
`sh_link` `.dynstr`, `sh_info` the `Verdef` count, `sh_addralign` 4
(lld and mold; GNU ld 8 on 64-bit), D-34's table gains the row. *Tests:*
`versions-verdef` (fields, parents, base name), `versym-hidden`.

**D-116 [M4] (`.dynstr` of a shared object).** D-24's order with the new
strings in place: the `.dynsym` names, the `DT_NEEDED` names, the
`DT_SONAME` string, the `Verdef` names (base first: the soname is then
already present, each distinct string once), the `Vernaux` names,
`DT_RUNPATH`/`DT_RPATH`. No tail merging. *Test:* `dynstr-layout`
gains a library.

**D-117 [M4] (relocation classes in a shared object; the illegal
table).** arch.md §4.3's `Shared` row, by target kind (D-113), measured
on hand-written forms against five kinds (local, hidden, preemptible,
import, weak undefined), each library run plain and interposed ([X-D]):

| class (examples) | `Local` (static, hidden, protected, bound) | `Preemptible`, `Shlib`, `WeakDyn` |
|---|---|---|
| GOT (`ADR_GOT_PAGE`+`LD64_GOT_LO12_NC`; `GOTPCREL`, `REX_GOTPCRELX`; i386 `GOT32`/`GOT32X` with a base register) | slot, `RELATIVE`; x86-64 relaxation to `lea` allowed (x64 G1-G4) | slot, `GLOB_DAT` against the key |
| Branch (`CALL26`, `JUMP26`; `PLT32`; i386 `PLT32` with `%ebx`) | direct | PLT entry (D-120) |
| AbsWord in a writable place (`ABS64`, `R_X86_64_64`, `R_386_32` in `.data`, `.data.rel.ro`) | `RELATIVE` | symbolic record, addend `A` |
| AbsWord in a read-only place (`.rodata`, `.text`) | refused, `text-relocation` | refused, `text-relocation` |
| AbsNarrow (`ABS32`, `MOVW_UABS_G*`; `R_X86_64_32`, `32S`) | refused, `non-pic-in-shared-object` | refused |
| Pc (`ADR_PREL_PG_HI21`, `ADR_PREL_LO21`, `LD_PREL_LO19`, `PREL32`/`PREL64` in data; `R_X86_64_PC32` on any instruction or in data; i386 `PC32` without `%ebx`) | resolved | refused, `non-pic-in-shared-object` |
| i386 `GOTOFF` | resolved | refused |
| i386 `GOT32X` without a base register | refused | refused |
| TLS | D-119 | D-119 |

The diagnostic names the relocation, the symbol and the remedy, with GNU
ld's wording (``relocation R_AARCH64_ADR_PREL_PG_HI21 against symbol
`pre_d' which may bind externally can not be used when making a shared
object; recompile with -fPIC``; for a `Local` target ``relocation
R_X86_64_32 against `.rodata' can not be used when making a shared
object; recompile with -fPIC``). This is lld's table, the only reference
that refuses every row a library cannot express. The others write wrong
libraries on the rows they accept: mold accepts the AArch64 `Pc` forms
against preemptible, imported and weak keys with no record (W32), and
takes an imported function's address from `-fno-pic` code as a PLT stub
inside the library (W38); GNU ld binds an AArch64 `PREL32` data word to
a preemptible key locally and gives a weak one a non-zero value (W33),
and writes an x86-64 dynamic `R_X86_64_PC32` that the loader truncates
to 32 bits (W34); on i386, mold calls through an `%ebx` PLT from code
that has not set `%ebx` (W35), links `GOT32X` without a base register
(W36), and GNU ld and mold bind `GOTOFF` to a preemptible key locally
(W37); lld and mold link `R_386_TLS_IE` in `.text` with no text
relocation (W39). Text relocations are refused as D-47 (`-z notext`
stays refused; GNU ld and mold write `DT_TEXTREL` and those libraries
run, lld refuses), so a library is never `DT_TEXTREL`. Two rows are
narrower than a reference: an x86-64 `PC32` call or jump to a
preemptible key (mold makes a PLT entry; GNU ld and lld refuse; no
compiler writes it in `-fPIC` code) and i386 `R_386_PC32` in data (GNU
ld and lld write a record that runs; mold refuses) are refused.
Requests R-A10 (arch §4.3's `Shared` row: `TX` becomes `E`; `Pc` and
narrow absolute against `Local` resolved), R-A11 (a64), R-X10 (x64),
R-X86-10 (x86).

**Bound use of exported data (the address-level rule; review B-1).** The
table above judges a use by the key its relocation names. Non-PIC code
can reach an exported datum without naming it: GCC 14's AArch64 `-fPIE`
(its default, and what a static library built without `-fPIC` holds)
reads its own exported `int g` through the section anchor,
`R_AARCH64_ADR_PREL_PG_HI21 .data + 8`; GNU ld, lld and mold link it
into `-shared` silently, export `g` preemptible, and a `-no-pie` host
that copies `g` gets two instances (`exp2/gapdemo`: `all=10 g=100`
where `all=107 g=7` is right, all three). The rule:

- A **preemptible data definition** is a definition D of a live object
  O, in an input section T of O without `SHF_EXECINSTR` (`STT_TLS`
  included), whose key this table's D-112 makes preemptible. Its
  **interval** is `[st_value, st_value + max(st_size, 1))`, in T's input
  offsets (before merging and layout).
- A **use** is a relocation of O, of any type but `R_*_NONE` and the
  `SIZE` types, whose place is in an `SHF_ALLOC` section of O other than
  `.eh_frame` and `.gcc_except_table*`, and whose symbol is local to O
  (`STB_LOCAL`: a section symbol, a named local, a GCC `.localalias` or
  Clang `$local` alias) defined in T. The address it **names** is
  `st_value + A` (A: the RELA addend, or the REL implicit addend).
- **Scope on x86 (the M-1 ruling).** On AArch64 every use is judged: the
  address a field names is exact. On x86-64 and i386 only uses whose
  named address is **exact** are judged: absolute words (`R_X86_64_64`,
  `32`, `32S`, `16`, `8`; `R_386_32`, `16`, `8`), the `GOTOFF` forms
  (`R_X86_64_GOTOFF64`, `R_386_GOTOFF`), and PC-relative fields in a
  place without `SHF_EXECINSTR` (`R_X86_64_PC32`, `PC64`, `PC16`, `PC8`,
  `R_386_PC32`, `PC16`, `PC8` in data, where the addend carries no
  instruction tail). A PC-relative field (`PC*`, `PLT32`, `GOTPCREL*`,
  `GOTPC*`) in an `SHF_EXECINSTR` place against a local or section
  symbol is **not judged on x86**: its addend is the target's offset
  minus the distance from the field to the end of its instruction, and
  that distance (the immediate that follows the displacement) is a
  property of the instruction, not of the relocation; guessing it (the
  first pass's `+4+t`) refused abseil's `format.cc` at GCC's default
  `-O2 -fPIC` and decided 11 of 25 ambiguous system and abseil uses
  wrong (the re-review's M-1). The case is recorded as **"not judged on
  x86: a section-relative PC-relative code use of exported preemptible
  data"**. Measured (`exp3/x86gt.py`, ground truth from `-fPIC` twins):
  no x86 producer reaches its own exported data that way: GCC 14 x86-64
  `-fPIE` names the symbol for 85 of 85 bound definitions, Clang 22
  for 86 of 87 (the other is the hand-written assembly label of
  `boundary.c`, which no rule sees), hod (kiln `-fkiln-backend=hod`,
  `-O0`, x86-64 `-fPIE` and `-fno-pic`, 10 translation units each,
  `exp3/hod/build.sh`) for 19 of 19, all refused by this table; GCC 14
  and Clang 19 and 22 i386 `-fPIE` reach it by `GOTOFF` against the
  symbol (21 uses per compiler over 6 units, refused by the table's
  `GOTOFF` row), by section-relative `GOTOFF` (2 per compiler: exact,
  judged by this rule) and through the GOT (`GOT32X`)
  (`exp3/i386how.py`). The exact scope refuses no object of the x86
  `-fPIC` corpora (abseil configs C and E, 324 objects; the author's 274;
  the constructed 120; 37 system archives; x86-64 `libc.a` and
  `libm.a`), and 3 non-PIC members of i686 `libc.a` (`ttyname.o`,
  `getsgent.o`, `getsgnam.o`: correct refusals) (`exp3/run_x86.sh`).
  **WP-127 repeats this measurement on its own corpus** (kiln with hod and
  with llc, GCC, Clang, at every `-O`) before x86-64 `Shared` ships, and
  WP-130 for i386; a producer found to reach exported data through a
  section-relative PC-relative code field reopens this clause.
- A judged use whose named address lies in a preemptible data
  definition's interval is a **bound use**, refused,
  `non-pic-in-shared-object`, naming the definition, the object, the
  relocation and its place, and the remedy ``recompile with -fPIC``.
- **The boundary (m-1).** An address a is a **boundary address** of D
  when a is D's `st_value` and also `st_value + st_size` of a local
  symbol of O defined in T with a non-zero `st_size` (one past the end
  of a preceding `static` object). A use whose named address is a
  boundary address is a bound use only when (i) it is an **access at
  the address**: on AArch64 the relocation is a load/store form,
  `LDST8_ABS_LO12_NC`, `LDST16_ABS_LO12_NC`, `LDST32_ABS_LO12_NC`,
  `LDST64_ABS_LO12_NC`, `LDST128_ABS_LO12_NC` (278, 284, 285, 286, 299)
  or `LD_PREL_LO19` (273); for the analysis below, a load or store whose
  access address is a; on x86 no relocation is an access (the exact
  forms do not say whether the instruction reads or computes the
  address); or (ii) O has **no self-GOT**: no relocation of a GOT form
  whose symbol is a non-local symbol named as one of O's own preemptible
  data definitions (GOT forms: AArch64 300-306, 309-313,
  `MOVW_GOTOFF_G*`, `GOT_LD_PREL19`, `LD64_GOTOFF_LO15`, `ADR_GOT_PAGE`,
  `LD64_GOT_LO12_NC`, `LD64_GOTPAGE_LO15`; x86-64 `GOT32`, `GOTPCREL`,
  `GOTPCRELX`, `REX_GOTPCRELX`, `GOT64`, `GOTPCREL64`, `GOTPLT64`; i386
  `GOT32`, `GOT32X`). Otherwise it is not a use. Reason: code that
  reaches its own exported data through the GOT was compiled `-fPIC`
  and an address-valued boundary in it is the end of the local object
  (Clang's `arr + 10` loop bound); non-PIC code (no self-GOT) or a load
  at the address is the next definition. The note of a boundary refusal
  names `-fdata-sections` as the remedy (each object in its own
  section: no boundary). Measured (`exp3/run_bnd.sh`, objects refused by
  the rule or the analysis, against "always a use" and "never"):

  | corpus | objects | always | refined (the rule) | never |
  |---|---|---|---|---|
  | the author's GCC and Clang AArch64 `-fPIC` | 224 | 10 | 0 | 0 |
  | the re-review's constructed boundary set (`cx/`, AArch64) | 170 | 78 | 1 | 0 |
  | 79 system archives, AArch64 | 11,699 | 43 | 37 | 29 |

  Refined against always: the 10 author objects and 6 sanitizer
  `cp-demangle.o` members (PIC archives, every one self-GOT, boundary
  only) link; Clang 22 `-O2 -fPIC` `c2.c` (`static int arr[10]; int
  after;`, a loop to `arr + 10`) links. Refined against never: 8 non-PIC
  members stay refused (`libc.a` `aio_misc.o`, `dcigettext.o`,
  `getdate.o`, `getsgent.o`; `libobjc.a` `class.o`, `selector.o`
  (`ldr w1, [x1, :lo12:.bss+0x20]`, a load at `__objc_selector_max_index`);
  `libobjc_gc.a` `class_gc.o`, `selector_gc.o`). The one constructed
  refusal left is `cx/c2.g-Os-fPIC_-fcommon.o` (an end-of-array address
  computed by the analysis in an object with no self-GOT): a correct
  `-fPIC` object refused, with the `-fdata-sections` note; none in real
  code.
- **Before the start of a local object: a known refusal of correct
  code (the supervisor's ruling, WP-115).** Correct `-fPIC` code that
  forms an address *before* the start of a local object (`double *p =
  tab - 1;`, the 1-based idiom: undefined behaviour in C, accepted by
  every compiler) names an address that, after the compiler's folding,
  can lie inside or at the start of an exported preemptible datum that
  precedes the local object in its section. Such a use is not a bound
  use (the code reaches the datum, if at all, through the GOT), but the
  rule and the analysis cannot tell it from one, and the boundary rule
  does not apply (the address is not one past the end of a *local*
  object). **The rule stays as it is**: a named refusal beats a silent
  bound use, and refining it by "the object has a GOT relocation against
  that same definition" would reopen the hole for `ld -r` mixes of PIC
  and non-PIC code. The refusal is `non-pic-in-shared-object` like any
  bound use; its note names the remedy as the boundary's does,
  `-fdata-sections` (each object in its own section: no preceding
  datum), and adds `do not form an address before an object`.
  Measured (WP-115, `tests/shared-design/sup/`: `c.c`, `static double
  tab[8]` between `int gvar[4]` and `int gcount`, and gfortran's
  1-based `SAVE` arrays beside module variables, `m.f90`, `l.f90`; every
  `-O` of `-O0`, `-O1`, `-O2`, `-O3`, `-Os`, `-fPIC`, through the oracle
  `s1d.py --hits`; 45 objects, 3 refused, every one self-GOT):

  | compiler, target | refused | how |
  |---|---|---|
  | GCC 14 AArch64 `-O0` | `c.c` | by the rule: `ADR_PREL_PG_HI21`/`ADD_ABS_LO12_NC` (275, 277) at `f+0xc`, `f+0x10` against `.data + 8`, inside `gvar` (`.data` 0-0x10; `tab` at 0x10) |
  | Clang 22 AArch64 `-O0` (and Clang 19) | `c.c` | by the analysis: `adrp`/`add` of `.data + 0x18` (`tab`), then `sub #8`, an address-valued `.data + 0x10`, the start of `gcount` (after `gvar`, a global, so not a boundary) |
  | GCC 14, Clang 22, Clang 19 AArch64 `-O1`-`-Os` | none | the locals are placed first or the address is formed exactly |
  | gfortran 14 AArch64, every `-O` | none | exact addresses (`m.f90`, `l.f90`) |
  | GCC 14, Clang 22, gfortran 14 x86-64, every `-O` | none | the before-start address is a code `R_X86_64_PC32` (`.data + 0x14`, GCC `-O0`; `.data + 0x1c` and a register subtract, Clang `-O0`): a PC-relative field in an `SHF_EXECINSTR` place, **not judged on x86** (the scope above); no absolute word and no PC-relative data word of this kind in any object |

  `[unmeasured]` for WP-117: the number of interior address-valued uses
  (rule or analysis hits that are not accesses) in self-GOT objects
  over its corpora at `-O0` (the design's corpora were mostly optimised
  builds), each listed in its record. *Test:* `shared-pic-before-start`
  (WP-117 for the rule's GCC case, WP-119 for the analysis's Clang
  case: refused with both notes; the `-O2` objects link).

Not uses: debug and other non-alloc places (every `DW_OP_addr` of an
exported variable, 864k measured references in `.debug_*` against
functions and data), `.eh_frame` (FDE `PREL32` to functions: 44,869)
and the LSDA; uses against a function definition (an exported function
reached through `.L$local` or `.localalias` under
`-fno-semantic-interposition` is the compilers' documented contract,
and both compilers emit it in `-fPIC` code: 913 code-place uses whose
address lies in a function, over the corpus, none refused; per-group
counts in `exp2/work/b1-analysis/<group>.txt`). Measured with
`exp2/b1/relscan.py` (the first pass's rule as code) over 14,316
objects (`exp2/b1/analyze.sh`); its x86 code-PC refusals are withdrawn
by the scope above and its boundary refusals of `-fPIC` code by the
boundary rule:

| corpus | objects | refused | what |
|---|---|---|---|
| GCC 14, Clang 22, AArch64 and x86-64, `-fPIC`, `-fPIC -fno-semantic-interposition`, `-fPIC -fvisibility-inlines-hidden`; GCC i386 `-fPIC` (Lua, zlib, googletest, the census sources, libstdc++-style C++) | 498 | 0 | the first pass refused 18 constructed one-past-the-end words of `boundary.c` and `vague.cc`; the boundary rule links them (every one self-GOT, none an access) |
| the census's `-fPIC` objects (the 25 real lines: Lua modules, zlib, libtool and CMake objects) | 47 | 0 | |
| expat 2.6.4 and pcre2 10.44 by their own `configure`+`make`, libtool's `.libs` objects | 35 | 0 | |
| `liblapack_pic.a`; rustc 1.95's `std`, `core`, `alloc`, `compiler_builtins` rlibs (a `cdylib` links them) | 2,388 | 0 | |
| `libgcc.a`, `libgcc_eh.a`, `libstdc++.a`, `libsupc++.a`, `libgfortran.a`, `libatomic.a`, `libgomp.a` (`-static-lib*` in a library), AArch64 and x86-64 | 3,131 | 1 | `libgomp.a(env.o)`, a non-PIC member that binds its exported `gomp_def_allocator_envvar` (`.bss + 8`): a correct refusal |
| GCC AArch64 `-fPIE` (the default), the same sources | 54 | 7 | every object whose code reaches its own exported data (7 of 7); of the 85 bound definitions, 49 by symbol (this table), 16 by this rule, 18 by the analysis below, 2 unseen (the residual hole) |
| GCC x86-64, Clang AArch64 and x86-64, GCC i386 `-fPIE` | 212 | 3 by this rule | `vague.cc`'s one-past-the-end words (Clang AArch64, GCC and Clang x86-64), each object also refused by the table; every bound definition but the assembly label is named by symbol |
| distribution `libc.a`, `libm.a`, BLAS, LAPACK, `libcrypt.a`, `libresolv.a` (default, non-PIC) | 8,071 | 21 | non-PIC members (correct refusals) |

**Bound use through a register (the analysis; package WP-119; AArch64;
the M-2 ruling).** GCC AArch64 `-fPIE` also forms the anchor's address
in a register and reaches the datum through an instruction immediate
(`adrp x0, .data; add x0, x0, :lo12:.data; ldr w1, [x0, #8]`): the only
relocation names `.data + 0`. On the twins this hid 18 of 85 bound
definitions of the author's corpus and 6 of 93 of abseil's, and 2
abseil objects at GCC's default (`raw_logging.cc`, `format.cc`) bound an
exported datum with no relocation-level refusal (the re-review's M-2).
The analysis follows such registers inside each function. It is
specified so that two implementations give the same verdict on every
object; its oracle is `exp3/s1d.py` (record §G.1). What it finds is a
bound use, refused by the same name, `non-pic-in-shared-object`, naming
the definition, the object, the function and offset of the instruction,
and the relocation that formed the address.

1. **When it runs.** Only for a `Shared` output, on an `EM_AARCH64`
   object O that has a preemptible data definition in some section T
   and a relocation in an `SHF_ALLOC|SHF_EXECINSTR` section of O whose
   symbol is a local symbol defined in such a T (the **pre-filter**).
   The pre-filter reads only the relocations the scan reads anyway; a
   link where no object passes it decodes nothing. Measured: it passes
   for 24 of 224 objects of the author's `-fPIC` corpus, 24 of 495 of
   abseil and fmt `-fPIC`, 16 of 163 abseil `-fPIE`, 3 of 577 busybox,
   0 of 83 arpack-ng, 51 of 11,699 system archive members; the decoded
   words are 381,784, 71,923, 26,993, 13,114, 0, 123,763.
2. **Functions.** Each distinct triple (section, `st_value`, `st_value
   + st_size`) of an `STT_FUNC` symbol of O with a non-zero `st_size`
   in an `SHF_EXECINSTR` section that is not `SHT_NOBITS` is analysed
   alone; its words are those at `st_value + 4k`, `k < st_size / 4`
   (rounded down). Code outside every such symbol is not analysed.
   A word is **data** when the last mapping symbol of its section at or
   before its offset is `$d` (or `$d.*`); `$x` (or `$x.*`), or none, is
   code.
3. **The state** at a word is a map from general registers x0-x30 to
   an address `(T, a)`: an input offset a in a section T that holds a
   preemptible data definition. Register 31 never has a state.
4. **The defining forms** (each with a relocation at the word naming a
   local symbol y defined in such a T; the value is `y.st_value + A`):
   - `ADD` (immediate), 64-bit, not setting flags, Rd not 31, with
     `ADD_ABS_LO12_NC` (277): Rd gets the value (the `ADRP` before it is
     not followed; it is the address rule's use);
   - `ADR` with `ADR_PREL_LO21` (274), Rd not 31;
   - the literal forms: a 64-bit `LDR Xt` (unsigned offset, size 3,
     opc 01, not SIMD) with `LDST64_ABS_LO12_NC` (286) naming any symbol
     z, or a 64-bit `LDR Xt` (literal) with `LD_PREL_LO19` (273) naming
     z, or without a relocation (z is then the word's own place plus
     `imm19 × 4`): when the word at z (`z.st_value + A`) carries an
     `ABS64` relocation naming a local y of such a T, Xt gets `(T,
     y.st_value + A')`.
   `ADRP` + load/store `:lo12:` used as a base names its exact address
   in its relocation: it is the address rule's use, and the analysis
   only kills the loaded registers.
5. **Decoding** (A64, bits of the word w; first match):

   | form | test | effect on the state |
   |---|---|---|
   | `B`, `BL` | `w[30:26] = 00101` | `BL`: kill x0-x18 and x30 |
   | `B.cond`, `BC.cond` | `w & 0xff000000 = 0x54000000` | none |
   | `CBZ`, `CBNZ` | `w[30:25] = 011010` | none |
   | `TBZ`, `TBNZ` | `w[30:25] = 011011` | none |
   | branch (register) | `w[31:25] = 1101011`, opc `w[24:21]` | opc 0000, 1000 (`BR`*): none; 0001, 1001 (`BLR`*): kill x0-x18, x30; 0010, 0100, 0101 (`RET`*, `ERET`, `DRPS`): none; else kill `w[4:0]` and x30 |
   | `ADD`/`SUB` (immediate) | `w[28:23] = 100010` | below |
   | `ADR`, `ADRP` | `w[28:24] = 10000` | `ADR`: below; `ADRP`: kill Rd |
   | `MOV` (register), 64-bit | `w & 0xffe0ffe0 = 0xaa0003e0` | Rd gets Rm's state (none if Rm untracked) |
   | load/store group | `w[28:25] & 0101 = 0100` | the five forms below; any other word of the group kills every register |
   | other system and branch-group words | `w[28:25] & 1110 = 1010` | kill `w[4:0]` and x30 |
   | unallocated | `w[28:25]` in 0000, 0001, 0011 | kill every register |
   | everything else (data processing, SIMD and FP, SVE) | | kill `w[4:0]` |

   Load/store forms (Rn `w[9:5]`, Rt `w[4:0]`, V `w[26]`, size
   `w[31:30]`, opc `w[23:22]`): unsigned offset, `w[29:24] & 111011 =
   111001`, offset `imm12 << scale` (scale: size, or 4 for a 128-bit
   SIMD access, V=1 size 0 opc 1x); `imm9` forms, `w[29:24] & 111011 =
   111000` with `w[21] = 0`, `w[11:10]` 00 unscaled, 01 post-index, 10
   unprivileged, 11 pre-index, offset `imm9` sign-extended; pair,
   `w[29:27] = 101`, `w[25] = 0`, `w[24:23]` 00 or 10 offset, 01 post,
   11 pre, offset `imm7 << scale` (scale 2 + opc for SIMD; for general
   registers 3 when opc is 10, 4 for `STGP`, else 2); exclusive,
   ordered and compare-and-swap, `w[29:24] = 001000`, no offset, kill
   Rt, Rt2 (`w[14:10]`), Rs (`w[20:16]`) and Rs+1; atomic memory
   operations, `w[29:24] & 111011 = 111000` with `w[21] = 1` and
   `w[11:10] = 00`, no offset, kill Rt (general registers only); `LDR`
   (literal), `w[29:24] & 111011 = 011000`, kills its general Rt (and is
   a defining form, item 4). A load of general registers (not V, opc not
   00, not `PRFM`) kills Rt (and Rt2 for a pair); a SIMD access changes
   no general register. `PRFM`/`PRFUM` (V=0, size 11, opc 10, unsigned
   offset or unscaled) access nothing.
6. **Propagation** (an instruction with no relocation at its word):
   - `ADD`/`SUB` (immediate) with sf 1 (`ADDS`/`SUBS` too), Rn not 31
     and tracked `(T, a)`, Rd not 31: Rd gets `(T, a ± (imm12 << (12 if
     sh)))`, and that address is **judged** (not an access); any other
     `ADD`/`SUB` (immediate) kills Rd.
   - a load or store whose Rn (not 31) is tracked `(T, a)`: its access
     address `a + offset` (offset and pre-index forms; `a` for
     post-index; `a` for the forms without offset) is **judged as an
     access** (not for `PRFM`); with pre- or post-index write-back Rn
     gets `(T, a + offset)`, judged (not an access); then the registers
     it loads are killed. With Rn untracked a write-back kills Rn.
   An instruction with a relocation at its word that is not a defining
   form kills what it writes (a relocated load kills its loaded
   registers, `ADD`/`ADR` their Rd); a relocated `BL` is a call (item
   7); a relocated `B` leaves the function.
7. **Calls and what ends a value.** A value ends when its register is
   written by anything but propagation: the kills of item 5. A call
   (`BL`, `BLR`) kills x0-x18 and x30 and keeps x19-x29 (AAPCS64: the
   callee saves them); measured, killing every register at a call loses
   2 of the author's 18 analysis-found definitions and finds nothing
   more. A data word or an undecodable word kills every register.
8. **Control flow.** Successors of a word: `B` without a relocation, its
   target when it lies in the function (else none); `B.cond`, `CBZ`,
   `CBNZ`, `TBZ`, `TBNZ`, the next word and, without a relocation, the
   target when it lies in the function; `BR`* and `RET`* (the opc values
   of item 5), none; any other word, the next word when it lies in the
   function. **Leaders**: the function's first word, every in-function
   target, and the word after any branch (`B`, conditional, `BR`*,
   `RET`*). The entry state of the first word is empty; in a function
   that contains a `BR`* every leader's entry state is empty (a computed
   jump's targets are not known); otherwise a word's entry state is the
   **meet** of its predecessors' exit states (a register keeps its value
   only when every predecessor has the same `(T, a)` for it), and a word
   without a predecessor starts empty. The states are the greatest
   fixed point of these equations: iterate the words in address order,
   ignoring predecessors not yet computed, until no exit state changes,
   then judge in one final pass in address order. The result does not
   depend on the iteration order; the oracle's is address order.
   Measured against simpler scopes (`exp3/run_modes.sh`): a linear scan
   with no control flow and a reset at every leader find the same
   definitions on the twins and one object fewer (27 against 28) of the
   system archives' analysis refusals; the fixed point is the rule.
9. **The verdict.** Every judged address that lies in a preemptible
   data definition's interval in T is a bound use (the boundary rule
   above applies, with "access" as item 6 says).

Measured (`exp3/gt3.py`, `exp3/run_s1d.sh`; bound definitions from the
`-fPIC` twins: own exported data the `-fPIC` object reaches through the
GOT; per definition the first of: named by symbol (this table), by the
address rule, by the analysis, unseen; per object: refused, or escapes):

| twins (`-fPIE` vs `-fPIC`) | objects | using own exported data | refused | escape | bound definitions | symbol | rule | analysis | unseen |
|---|---|---|---|---|---|---|---|---|---|
| the author's GCC AArch64 (`exp2/b1`) | 54 | 7 | 7 | 0 | 85 | 49 | 16 | 18 | 2 |
| the author's Clang 22 AArch64 | 54 | 7 | 7 | 0 | 87 | 86 | 0 | 0 | 1 |
| abseil 20240722 GCC AArch64 `-O2`, configs F against A | 163 | 32 | 32 | 0 | 93 | 78 | 9 | 6 | 0 |
| busybox 1.36.1 defconfig (`-fPIE` against `-fPIC`) | 577 | 2 | 2 | 0 | 3 | 3 | 0 | 0 | 0 |
| arpack-ng 3.9.1 gfortran | 83 | 0 | | | | | | | |
| hod (kiln `-O0`), AArch64 `-fPIE` and `-fno-pic` | 10, 10 | 6, 6 | 6, 6 | 0 | 19, 19 | 19, 19 | 0 | 0 | 0 |

False hits on `-fPIC` objects (refusals by the rule or the analysis):
0 in the author's 224, abseil and fmt's 495, arpack-ng's 83, busybox's
target objects, hod's `-fPIC` objects; busybox's three host tools
(`scripts/kconfig/conf.o`, `mconf.o`, `zconf.tab.o`, built by the host
compiler's default `-fPIE`) are true detections. In the system archives
the analysis refuses 11 members the rule does not (`libc.a`
`libc-tls.o`, `dcigettext.o`, `cxa_atexit.o`, `aio_misc.o`, `getdate.o`,
`getsgent.o`, `getsgnam.o`; `libobjc.a` `class.o`, `sarray.o`;
`libobjc_gc.a` `class_gc.o`, `sarray_gc.o`: non-PIC members), and
`exp2/gapdemo/gap2.c` (W44) is refused (`ldr` at `.data + 4`, `g`).

**The residual hole (named).** What still links silently, with every
reference too: AArch64 non-PIC code that reaches its own exported data
only through forms the analysis does not follow: a base spilled to the
stack or stored to memory and reloaded; an address combined with a
register (`add x0, x2, x0`, a computed index: `vague.cc`'s
`exported_table`) or carried in a 32-bit register or through any
instruction but `ADD`/`SUB` immediate and `MOV`; a value passed to
another function; code outside an `STT_FUNC` symbol with a size, and
hand-written assembly (`boundary.c`'s `asmlabel`); after a computed
jump (`BR`) at every leader. Measured share: 3 of 306 bound definitions
of the twin corpora (1%), and no object escapes (every one is refused
for another use); x86's "not judged" case above has 0 measured
instances. The `docs/status` row is in the record (§G.4); a Mason
object will close it for kiln and hod output (record §G.5, the request
to hod). A candidate opt-in diagnostic (not a default warning: it
fires on correct code), the **no-self-GOT signature**: an object that
defines exported data in a section its code addresses through local
symbols and has no GOT relocation against any own exported datum
flags 1 abseil config-A object, 2 config-D objects, 2 of the author's
GCC AArch64 `-fPIC` objects (`lapi.c`, `inftrees.c`) and 3 busybox
`-fPIC` objects (the re-review's `sig.py`), all correct; recorded for a
later `--warn-nonpic-suspect`, not designed.

The rule and the analysis depend on D-112: under `-Bsymbolic` (or a
`--dynamic-list` not listing it) the definition is bound and the same
object links correctly. Neither changes a link of an executable.
*Tests:* `shared-nonpic-anchor` (GCC AArch64 `-fPIE`: refused, naming
`g`; `-fPIC`: links), `shared-nonpic-boundary` (Clang `c2.c` `-fPIC`
links; GCC `boundary.c` `-fPIC` links; a load at the boundary in an
object with self-GOT and an address-valued boundary without self-GOT
are refused with the `-fdata-sections` note; `libobjc.a(selector.o)`
refused), `shared-nonpic-analysis` (WP-119: `gap2.c` W44 refused; abseil's
`raw_logging.cc` and `format.cc` `-fPIE` refused; their `-fPIC` twins
link; one case per item of the decoding table, each checked against
`exp3/s1d.py`), `shared-nonpic-x86-scope` (x86-64 `format.cc` `-fPIC`
links; a section-relative `PC32` in data is judged; a code `PC32` is
not), `shared-nonpic-not-uses` (debug, `.eh_frame`, `$local` function
aliases: links), `shared-reloc-table` (every row and kind, each
architecture, run plain and interposed; the refusals' messages).

**D-118 [M4] (undefined references, `-z defs`, `DT_NEEDED`).**

- A strong undefined reference from a live object to a key that nothing
  defines is accepted in `Shared` and left to the loader (D-111 item 2;
  all three; the Lua modules reference the interpreter's `lua_*` and
  each other this way). Under `-z defs`/`--no-undefined` it is refused
  per reference, `undefined-symbol`, as for executables (resolve §5),
  and a definition only in an indirect dependency (not on the command
  line) does not satisfy it (all three, [X-A] A4). A reference only from
  a section `--gc-sections` collected is not reported (D-124). Weak
  undefined references are never reported.
- A versioned reference (`vf@VA`, from `.symver`) needs a shared input
  that defines `vf` at `VA`, with or without `-z defs`; otherwise
  `undefined-symbol` naming the version (GNU ld and lld; mold links an
  unversioned reference, drops `DT_NEEDED`, and the library fails to
  load, W42).
- `--allow-shlib-undefined` is the default for `Shared` (GNU ld and lld):
  D-13's check of the shared inputs' own undefined references is off
  unless `--no-allow-shlib-undefined` turns it on (request R-R11 to
  resolve §5.2; D-13's text names `Exec` and `Pie` already).
- `DT_NEEDED`: D-11, D-12 and D-19a unchanged. A library that defines
  nothing a live object strongly references is dropped under
  `--as-needed` (GNU ld and lld; mold keeps `libc.so.6` for the weak
  references of crtbeginS, harmless).

*Tests:* `shared-undefined-allowed` (a Lua-module shape, loaded by a
host that defines the symbols), `shared-zdefs` (refuse; the indirect
dependency case), `shared-versioned-ref` (W42).

### 17.3 TLS, PLT, IFUNC, copies

**D-119 [M4] (TLS in a shared object).** No reference relaxes a TLS
sequence in `-shared`: the number of `__tls_get_addr` calls in each
output equals its input's, every linker and architecture ([C]). mortar
relaxes nothing either; each model keeps its records:

| model in the input | target | records (`r_sym`, place) |
|---|---|---|
| GD (`__tls_get_addr`, a module/offset pair) | preemptible definition (D-112) or import | `DTPMOD` + `DTPOFF` (a64 `DTPMOD64` + `DTPREL64`) against the key; places 0 (RELA), i386 0 |
| GD or LD | non-preemptible definition | one `DTPMOD` with `r_sym` 0 per pair; the offset word is the static `DTPOFF` value, no record (all three; [C] `GD`) |
| TLSDESC | preemptible or import | `TLSDESC` against the key, in `.rela.dyn`, eager (lld and mold; GNU ld puts them in `.rela.plt` with `DT_TLSDESC_PLT`/`GOT`, and on i386 without those tags); place 0 under RELA |
| TLSDESC | non-preemptible | `TLSDESC` with `r_sym` 0, the offset as addend (RELA), or in the descriptor's second word with the first 0 (i386 REL; all three) |
| IE | preemptible or import | `TPREL64`/`TPOFF64`/`TLS_TPOFF` against the key |
| IE | non-preemptible | the same type, `r_sym` 0, the offset as addend (RELA) or in place (REL) |
| LE | any | refused by name, `local-exec-tls-in-shared-object` |

LE is refused on every architecture with lld's and mold's reason
(``relocation R_AARCH64_TLSLE_ADD_TPREL_HI12 against s_tls cannot be
used with -shared``): the library's TLS block is placed by the loader,
not at a link-time offset. GNU ld refuses it against an exported or
imported symbol, accepts it on i386 as `R_386_TLS_TPOFF` text
relocations, and on AArch64 links a library whose TLS is only local with
the link-time offsets, no record and no diagnostic: it reads the wrong
thread's storage (W19). AArch64 traditional GD (`-mtls-dialect=trad`)
is accepted (a64 §8.4's sequence; GNU ld and mold accept it, lld 19 and
22 refuse `unknown relocation (513)`). `DF_STATIC_TLS` is set iff some
IE slot exists (lld and mold everywhere, GNU ld on x86; D-30's "never"
is the executables' rule). Such a library can be `dlopen`ed only while
its IE storage fits glibc's static surplus (measured 1024 bytes load,
4096 fail, with or without the flag: the flag has no measured effect on
glibc 2.41). x86-64 `R_X86_64_TLSDESC` (type 36) stays refused (x64.md
§6.2) until a compiler emits it. *Tests:* `tls-shared-gd`,
`tls-shared-desc`, `tls-shared-ie` (each run from `DT_NEEDED` and from
`dlopen` with threads, each architecture), `tls-shared-le-refused`
(refuse; W19).

**D-120 [M4] (PLT and `.got.plt`).** A call to an import or to a
preemptible definition of the library (D-112) demands `PLT` (arch.md
§4.3 row `Branch`); a call to a non-preemptible definition is direct.
D-49's shapes, lazy targets, `-z now` rule and `.got.plt` header apply
unchanged; `.got.plt` of a lazy library is outside RELRO (lld and mold;
GNU ld covers the three reserved words with RELRO, [A]). PLT order is
D-41's. No `.plt.got` (D-49). The i386 PLT of a shared object is the
`%ebx`-relative form (D-130). *Tests:* `plt-lazy`, `plt-now` gain the
`Shared` rows.

**D-121 [M4] (IFUNC defined in a shared object).**

- A **preemptible** IFUNC (exported, D-112) is exported `STT_GNU_IFUNC`
  with `st_value` the resolver; every use goes through a record against
  it: a call through the PLT (`JUMP_SLOT`), an address through a GOT slot
  (`GLOB_DAT`) or a symbolic word; the loader resolves it once
  (GNU ld and lld). mold binds an exported IFUNC inside the library and
  writes no record against it: the executable's interposing definition
  is not used and `&f` differs between the executable and the library
  (W20).
- A **non-preemptible** IFUNC (local, hidden, protected, or bound by
  D-112) has one IPLT entry, which is its address everywhere in the
  library: calls from every translation unit, GOT slots (`RELATIVE`) and
  address words (`RELATIVE`); the IPLT entry's `.got.plt` word holds the
  only `IRELATIVE`, last in `.rela.plt` (D-41, D-45's rule, lld's). Its
  `.dynsym` entry, when exported, is `STT_FUNC` at the IPLT entry (D-23)
  [unmeasured: no protected IFUNC was linked]. GNU ld and mold give a
  hidden IFUNC a PLT address in one translation unit and the resolved
  function in another (AArch64), and a static or hidden one different
  code and data addresses (x86-64): `&f` is not one value (W21).
- `EI_OSABI` is D-23's rule.
- i386: D-130.

*Tests:* `ifunc-shared-equality` (two translation units; hosts PIE,
`-no-pie`, and an interposing `-rdynamic` executable; W20, W21).

**D-122 [M4] (no copies, no canonical PLTs).** A shared object never
has a `COPY` or a canonical PLT (no reference writes one: 972 libraries
scanned, [E] E6). A use that would demand one (arch.md §4.3 `C` in the
`Exec` row) is, in `Shared`: a symbolic record when its place is a
writable address-sized word; otherwise refused by name, with the
references' wording: ``relocation R_AARCH64_ADR_PREL_PG_HI21 against
symbol `ext_data' which may bind externally can not be used when making
a shared object; recompile with -fPIC`` (`non-pic-in-shared-object`; all
three on AArch64 and x86-64, [E] E1). The same applies to a non-GOT use
of a preemptible definition of the library itself (D-112), which is what
`-fPIE` code gives, per compiler and target (`exp2/b1`, ground truth
from `-fPIC` twins; `review-110-adv/fpie`):

| `-fPIE` object | own exported data | own exported function's address | in `-shared` |
|---|---|---|---|
| GCC 14, AArch64 | through the section anchor (`.data + off`, local symbol), the anchor plus an instruction immediate, or by symbol | by symbol (`ADR_PREL_PG_HI21 f`) | refused: by D-117's address-level rule (anchor), by D-117's analysis (anchor+immediate, WP-119), by D-117's table (symbol); what neither follows is D-117's residual hole (2 of 85 definitions, `exp3/gt3.py`) |
| Clang 22, AArch64 | by symbol (`ADR_PREL_PG_HI21 g`) | by symbol | refused by D-117's table (all three refuse it too) |
| GCC 14 and Clang 22, x86-64 | by symbol (`R_X86_64_PC32 g`) | by symbol | refused by D-117's table (all three refuse) |
| GCC 14, Clang 19 and 22, i386 | by `GOTOFF` against the symbol, by `GOTOFF` against the section (exact), or through the GOT (`GOT32X`), per access (`exp3/i386how.py`, 6 units: 21, 2 and 2-3 uses per compiler); the 50 census sources bind none | through the GOT | refused: by D-117's table (`GOTOFF` against the symbol), by D-117's address rule (section-relative `GOTOFF`, exact on x86) |
| hod (kiln `-fkiln-backend=hod`), AArch64 and x86-64, `-fPIE` and `-fno-pic` | by symbol (19 of 19 bound definitions, each target and model, `exp3/hod/build.sh`) | by symbol | refused by D-117's table |

So "`-fPIE` links as `-fPIC`" holds for no measured compiler or target
(the second pass's "GCC's i386 `-fPIE` does" was measured on sources
that bind no own datum; `exp3/i386/` shows the `GOTOFF` forms). A text
relocation is refused (D-117). *Tests:*
`shared-nonpic-refused` (refuse, each architecture), `shared-fpie-objects`
(the five `-fPIE` rows of this table, each refused or linked as it says).

### 17.4 Layout, gc, tools

**D-123 [M4] (layout).** A shared object is laid out as a PIE (D-60,
D-61) without `.interp`, `PT_INTERP` and `PT_PHDR` (D-100):

- sections: D-60's order; `.gnu.version_d` follows `.gnu.version` and
  precedes `.gnu.version_r` (GNU ld; lld puts it after `.gnu.version`,
  mold after `.rela.plt`);
- segments: D-61's order and the four `PT_LOAD`s R, RX, RW (RELRO), RW
  (lld and mold write four on every architecture; GNU ld two on AArch64
  and four on x86, [A]); `-z separate-code`/`-z noseparate-code` keep
  their M2 status (layout L-71);
- RELRO ends on a max-page boundary (D-61; GNU ld and mold; lld only on
  4 KiB, 0x21000 on AArch64);
- `p_align` of each `PT_LOAD` is the max page size, and `p_offset ≡
  p_vaddr (mod p_align)` (all three, 4 KiB to 2 MiB). glibc 2.41 checks
  only the congruence modulo the page size: it accepts `p_align` 0x10 and
  maps a `p_offset` that is page-congruent but wrong, then the library
  crashes ([F]); the verifier, not the loader, must hold this
  (request P-R6);
- no padding of the file to a page end (lld, mold; GNU ld pads).

*Tests:* `shared-segments`, `shared-section-order`, `relro-maxpage`
gains the `Shared` row.

**D-124 [M4] (gc roots).** With `--gc-sections` the roots of a shared
object are image.md I-6's and resolve.md §4.7's, which already name
every key exported to `.dynsym`; for `Shared` they read:

1. every exported definition (D-110), after the version script (D-114):
   a version-local definition is not a root (all three collect it, [E]
   E2);
2. the functions named by `-init`/`-fini`, even when hidden (GNU ld and
   lld; mold collects a hidden `-init` function and writes
   `DT_INIT = 0`: the library crashes at load, W22). resolve §4.7 does
   not list them today; request R-R10 adds them for every kind;
3. the start-up tables of I-6 (`.init_array`, `.fini_array`, `.ctors`,
   `.dtors`, `.init`, `.fini`; all three);
4. a C-identifier section reached through `__start_NAME`/`__stop_NAME`
   from a live section (layout L-96: GNU ld's default
   `nostart-stop-gc`; lld drops such a section when a version script
   hides its symbols and leaves `__start_NAME` undefined in `.dynsym`:
   `dlopen` fails, W23; mold keeps every C-identifier section);
5. `SHF_GNU_RETAIN` sections (layout L-97).

An undefined reference only from a collected section is not reported
by `-z defs` (all three, [E] E2; D-118). A `.ctors` input section goes
into `.init_array` (layout L-56, already the executables' rule; GNU ld
and mold; lld keeps a `.ctors` output section whose constructor never
runs, W24). *Tests:* `shared-gc-roots` (each item, run), `shared-ctors`.

**D-125 [M4] (debuggers and tools).** Measured on every reference's
libraries (`E-shared-tools`, 21 libraries per architecture and linker):
gdb 16.3 and lldb 19 set a breakpoint in a library function before
`run`, stop there, print locals, step and list the library from a host
that loads it by `DT_NEEDED` and by `dlopen`, natively on AArch64 and
through `qemu -g` on x86 (lldb's wrong TLS and `this` values and gdb's
missing TLS under qemu are the tools' own, the same on every
reference). They need nothing beyond D-100's column: `.dynamic`,
`.symtab`, `PT_GNU_EH_FRAME`, the debug sections (I-47's tombstones as
in a PIE, D-72). `ldd` lists every library; `ld.so --verify` exits 2
for every library of every reference (it has no `PT_INTERP`; that is
the expected status). `eu-elflint --gnu-ld` passes GNU ld's libraries
clean and complains about lld's and mold's (`.relro_padding`, `Vernaux`
layout, `.rodata` entry sizes, lld 22's AArch64 `PURECODE` `.plt`,
mold's `.symtab` entries for `__ehdr_start`); a mortar library must pass
with no allowance (D-71). Without `--eh-frame-hdr` (never on a driver
line) every throw out of a library aborts (exit 134) with every
reference: `PT_GNU_EH_FRAME` is required, as Q3 ruled (D-102).
Constructors and destructors run in the order glibc defines (`DT_INIT`,
`.init_array` forward; `.fini_array` backward, `DT_FINI`), and each
library has its own hidden `__dso_handle` for `__cxa_finalize`, with
every reference ([E-shared-tools] C). *Tests:* `gdb-break-shared`,
`lldb-break-shared`, `ldd-shared`, `elflint-shared`, `eh-shared` (a
throw from the library caught in the host, and back).

**D-130 [M4] (i386).** i386 shared objects come from GCC and Clang only
(no kiln target). Their package follows the 64-bit ones and adds:
PLT entries `%ebx`-relative (the library's GOT base in `%ebx`, as
GNU ld, lld and mold write them); `R_386_GOTOFF` and `R_386_GOTPC`
against the library's own GOT; REL places hold the addend (D-46). An
`R_386_32`/`R_386_PC32` in a read-only place against an import or a
preemptible definition (non-PIC code) is refused as D-122 (GNU ld and
mold write `DT_TEXTREL` and run; lld refuses; GNU ld's text relocations
in `.eh_frame` of a Clang `-fno-pic` C++ object give a library that
crashes on a throw, W25). IFUNC in an i386 shared object is refused by
name (`ifunc-in-i386-shared-object`) until designed: an IPLT entry is
`%ebx`-relative, so it cannot be the canonical address for callers
outside the library; lld uses it as one and the program crashes (W26),
mold calls a hidden IFUNC through an `%ebx` stub from code that has not
set `%ebx` (W27), GNU ld refuses a non-PIC call to a hidden IFUNC. LE
TLS is D-119's refusal.

### 17.5 Metadata, ODR, incremental

**D-126 [M4] (ODR and metadata in a shared object).**

1. `--odr-table=exported|full` writes `.note.mason` in a shared object
   as in an executable (odr.md O-50-O-52, already written for both
   kinds), with one change: a row carries `NONPREEMPTIBLE` exactly when
   D-112 binds the definition inside the library (protected,
   `-Bsymbolic`, `-Bsymbolic-functions` for a function, `--dynamic-list`
   not listing it); a preemptible definition's row has it clear. The
   rows' versions are D-115's nodes: odr.md O-24's "a mortar-made
   output's definitions are unversioned (`--version-script` is refused)"
   becomes "a row's version is its `.dynsym` entry's version node"
   (request R-O10). The bit is truthful because D-117's address-level
   rule and its analysis (WP-119) refuse a library in which non-PIC code
   binds a preemptible datum without naming it: a row says
   `NONPREEMPTIBLE` clear only when every use of the definition goes
   through a record. The one exception is D-117's residual hole (named
   there: forms the analysis does not follow, 3 of 306 measured bound
   definitions): an object whose only bound use of a datum is of such a
   form links, and its row says preemptible while the library's code is
   bound. A consumer of the table (an ODR check across libraries, Tuck)
   must not treat a clear bit as proof of interposability for a datum
   defined by an object without a `reloc-model` row (record §G.5); for
   an object whose row says `pic` it is proof.
2. **A copy of a definition that its library binds locally** is a rule
   for `Exec` and `Pie`, not for `Shared`: it is request R-D43 against
   D-43 (§18.4), its own package (record §D, WP-122). Its substance: the
   executable's copy and the library's own instance are two objects, and
   whether that is wrong depends on where the library keeps the datum,
   which the executable's link reads from the library's program headers
   and the definition's `st_value` (no section header is needed):
   - in a `PT_LOAD` with `PF_W` and outside every `PT_GNU_RELRO`:
     **refused**, `copy-of-locally-bound-definition` (the library writes
     its own instance, the executable reads its copy: `exp2/q10b`, a
     `-no-pie` host copying `.data` and `.bss` data of a `-Bsymbolic`
     library MISLINKs with every reference, AArch64 and x86-64);
   - in a read-only `PT_LOAD` or inside `PT_GNU_RELRO` (`.rodata`,
     `.data.rel.ro`): **warned**, same kind as a warning (two instances
     of a constant with equal contents; the same run is correct with
     every reference: only `&v` differs between executable and library);
   - in no `PT_LOAD`: refused (mortar cannot tell).
   A definition is locally bound when its library has `DF_SYMBOLIC` or
   `DT_SYMBOLIC` (GNU ld and lld write them; mold does not, so its
   `-Bsymbolic` libraries are not seen: a recorded gap of the rule) or
   when its `.note.mason` row carries `NONPREEMPTIBLE`. `exp2/q10b`'s
   `classify.py` decides from the program headers alone and agrees with
   every run (4 kinds of datum × 3 references × 2 targets) and on the
   system's `libgdk_pixbuf-2.0.so` (`gdk_pixbuf_version`, `.data`:
   refused; the three `.rodata` version constants: warned). W30 is
   reclassified accordingly: the warned cases are defined behaviour
   (two equal constants), the refused ones a wrong output the rule
   catches. Protected data is already refused (D-18). The fix in the
   program is to reach the datum through the GOT (`-fPIC`, or Clang's
   default `-fPIE`), which all four then link correctly.
3. `--keep-mason-meta` (meta.md MT-32) keeps the units in a shared
   object as in an executable; the sections are non-alloc and change no
   loaded byte [unmeasured for `Shared`; MT-32's executable tests carry
   over] (request R-MT10).
4. Tuck: the debugger needs nothing of a library that D-125 does not
   already give (`.symtab`, `.debug_*`, `PT_GNU_EH_FRAME`); the build-id
   note (`--build-id`, on every driver line) is what lets it match a
   library to its separate debug file.

*Tests:* `odr-table-shared` (rows, `NONPREEMPTIBLE` per D-112 case,
versions); R-D43's `copy-of-symbolic` (refuse `.data`/`.bss`, warn
`.rodata`/`.data.rel.ro`, refuse outside every `PT_LOAD`; a GNU ld and
an lld `-Bsymbolic` library and a mortar one with a table; W30).

**D-127 [M4] (incremental, M7).** Constraints, not a design (§14's
form):

- A shared object is never written in place (incr.md IN-43): a process
  that maps it reads the file's pages.
- Its interface is what an incremental relink must keep or report:
  `.dynsym`'s export set and versions (D-110, D-115), `DT_SONAME`, the
  `NONPREEMPTIBLE` set. A relink that changes the interface changes what
  every dependent executable bound; an incremental relink must compare
  the interface and say so (`interface-changed`), never update in place
  a library whose exports changed under a running process.
- §14's slack rules hold (`RELATIVE` slack in the prefix, `.dynsym`
  slack as bucket capacity or unhashed tail); a library has more
  symbolic records than an executable (every preemptible use), so
  `.dynsym` stability matters more: D-111's order is a function of
  `SymbolId`, as D-22's.
- Nothing in §17 makes a size depend on an address (D-62 holds; the
  `NONPREEMPTIBLE` set and the export set are functions of resolution
  and options).
### 17.6 Where the references differ (`Shared`)

| question | GNU ld 2.44 | lld 19/22 | mold 2.37 | mortar |
|---|---|---|---|---|
| `PT_PHDR` | no | yes | yes | no (D-100) |
| `PT_LOAD`s | AArch64 2, x86 4 | 4 | 4 | 4 (D-123) |
| RELRO end | max page | 4 KiB | max page | max page (D-123) |
| `.got.plt` reserved words in RELRO (lazy) | yes | no | no | no (D-120) |
| `.dynamic` order | its own | its own | its own | GNU ld's (D-102) |
| `RUNPATH` position | after `SONAME` | first | after `NEEDED` | GNU ld's (D-102) |
| `DT_NULL`s | 5-6 | 1 | 6 | 6 (D-102) |
| `-Bsymbolic` tags | `DT_SYMBOLIC` + `DF_SYMBOLIC` | `DF_SYMBOLIC`, also with `--dynamic-list` (W31) | none | GNU ld's, never with `--dynamic-list` (D-112) |
| `--image-base` | honoured | honoured | ignored silently | honoured (D-100) |
| `-e nosuchsym` | warns, 0 | warns, 0 | silent, 0 | warns, 0 (D-101) |
| `-init=nosuch` | `DT_INIT`/`DT_FINI` both dropped silently | same | same | refused (D-102) |
| text relocations | `DT_TEXTREL` (x86 warns) | refused | `DT_TEXTREL` silently | refused (D-117) |
| TLSDESC records | lazy, `.rela.plt`, `DT_TLSDESC_*` | eager, `.rela.dyn` | eager, `.rela.dyn` | eager (D-119) |
| `DF_STATIC_TLS` with IE | x86 yes, AArch64 no | yes | yes | yes (D-119) |
| AArch64 traditional GD | links | `unknown relocation (513)` | links | links (D-119) |
| LE TLS | AArch64 links local-only (W19); i386 `DT_TEXTREL` | refused | refused | refused (D-119) |
| exported IFUNC | records | records | bound locally (W20) | records (D-121) |
| local IFUNC address | per use (W21) | one IPLT entry (64-bit) | per use (W21) | one IPLT entry (D-121) |
| `EI_OSABI` with IFUNC or unique | GNU | none | none | D-23's rule |
| `.ctors` input | into `.init_array` | own section, never run (W24) | into `.init_array` | into `.init_array` (D-124) |
| hidden `-init` function under gc | kept | kept | collected, `DT_INIT = 0` (W22) | kept (D-124) |
| C-identifier section hidden by a version script | kept | dropped, `__start_` undefined (W23) | every one kept | kept (D-124) |
| linker-defined exports | `__start_`/`__stop_` of a referenced C-identifier section, `PROTECTED` | same | none | GNU ld's and lld's (D-110 item 2; L-100's other exports withdrawn) |
| version-node name symbols | exported (`ABS`) | no | no | no (D-110) |
| `STT_SECTION` in `.dynsym` | AArch64 2 | none | none | none (D-110) |
| `.gnu.hash` geometry | own | D-26 | own | D-26 (D-111) |
| version script: exact name in two nodes | first, silent | first, warns | last | first, warns (D-114) |
| `local:` before `global:` | syntax error | accepted | accepted | accepted (D-114) |
| one entry under `global:` of one node and `local:` of another | refused (`duplicate expression`) | accepted, answers vary | accepted, answers vary | refused, `duplicate-version-pattern` (D-114) |
| quoted entry with `*` (`"foo*"`) | exact | glob | glob | exact (D-114) |
| unquoted `fo\o`; `fo\*` | `foo`; `fo*` | `fo\o`; `fo*` | `fo\o`; `fo\o` (sic) | GNU ld's (D-114; `exp3/vs3/globs.py` g15, g12) |
| glob `f[!o]o`, `f[^o]o`; `[!]]oo` | negated class; any one character but `]` | negated class; nothing | `[!o]` a class holding `!` and `o`, `[^o]` negated; nothing | GNU ld's (D-114; g00, g01, g08) |
| unterminated `[` (`[oo`) | the character `[` | refused | refused | the character `[` (D-114; g11) |
| `global: foo` in V1, `local: "foo"` in V2 | refused (`duplicate expression`) | accepted | accepted | refused (D-114) |
| `global: fo*` in V1, `local: "fo*"` in V2 | accepted (glob and exact differ) | accepted | accepted | accepted (D-114) |
| `global: fo*` in V1, `local: foo*` in V2 | V1 | local | V1 | V1 (D-114; resolve §9.7) |
| forward or missing parent; node name twice; anonymous with named nodes | refused | accepted; merged; refused | accepted; merged; accepted | refused (D-114; script.md §9.1) |
| empty version script | accepted | refused | accepted | accepted (script.md §9.1) |
| comments `#`, `/* */`; `//` | accepted; error | same | same | same (D-114) |
| several parents | accepted | refused | refused | accepted (D-114) |
| undefined name in a script | silent | refused | warns | warns; `--no-undefined-version` refuses (D-114) |
| `extern "C++" {*}; local: *` | all exported | all exported, warns | none (W40) | refused until the demangler, then all (D-114) |
| `.symver` `foo@V1` matched by V1's own `local:` | dropped | dropped | kept | dropped, warns `versioned-definition-hidden` (D-114; resolve §9.7 rule 12) |
| `.symver` `foo@@V1` matched by V1's own `local:` | dropped | dropped by an exact `local:`, kept by a glob or `*` | kept | dropped, warns (rule 12) |
| `.symver` `old@V1`, V1 `{ global: o*; local: old; }` | kept | dropped | kept | kept (rule 12; `exp3/vs3/expdyn.py` e09) |
| `.symver` `old@V1` hidden by V1, line has `--export-dynamic` | kept | dropped | kept | kept (rule 12; D-114; e06-e08) |
| `.symver` `old@V1` matched by another node's entry | kept at V1 | `local: *`: kept; an exact `global:` or `local:` entry: refused | kept at V1 | kept at V1 (resolve §9.7 rule 12; e05, e10, `atat.py` a05) |
| `Verdef` parents | written | not written | not written | written (D-115) |
| base `Verdef` without `-soname` | basename | the path as given | basename | basename (D-115) |
| `.gnu.version_d` alignment | 8 | 4 | 4 | 4 (D-115) |
| `NEEDED_INDIRECT_EXTERN_ACCESS` | kept | dropped (W41) | dropped (W41) | kept (D-112) |
| `-Bsymbolic-non-weak*` | unknown option | accepted | accepted | refused (D-112) |
| `-Bsymbolic-functions` binds | `FUNC`, `NOTYPE` | `FUNC` | `FUNC`, `GNU_IFUNC` | `FUNC` (D-112) |
| `--dynamic-list`: unlisted `TLS`, `GNU_IFUNC` | preemptible | bound (x86-64 `TLS` half: W43) | bound | bound (D-112) |
| `--dynamic-list` with a `-Bsymbolic` form | the list decides | the list, but `-Bsymbolic` binds all (W31) | the list, but list then `-Bsymbolic-functions` makes data preemptible | the list decides (D-112) |
| `--dynamic-list-data`, `--dynamic-list-cpp-*` | accepted | unknown | `-data` a no-op; `-cpp-*` unknown | refused (D-112) |
| versioned reference to an absent version | refused | refused | links, unversioned, `NEEDED` dropped (W42) | refused (D-118) |
| `--as-needed` with only weak references to libc | dropped | dropped | kept | dropped (D-118) |

### 17.7 Wrong outputs found (each with its script)

Continuing §13.2's numbering. Scripts under
`tests/shared-design/exp/` (WP-115's copy; the scripts name their
original `.build` paths); each runs from `run-all.sh WORK`.

| # | reference | finding | script |
|---|---|---|---|
| W19 | GNU ld (AArch64) | LE TLS against a library's own static/hidden TLS links with link-time offsets, no record, no diagnostic; `dlopen`ed, it reads another block (`FAIL main=0 thread=0`) | `dyn/partC.py` (`LE_local`) |
| W20 | mold | an exported IFUNC is bound inside the library (no record against it): interposition ignored (`call_f()=1`), `&f` differs between executable and library | `dyn/partD.py`; `export/partC.py` (`ifn=10`) |
| W21 | GNU ld, mold | a hidden IFUNC has a PLT address in one translation unit and the resolved function in another (AArch64); a static or hidden IFUNC's code and data addresses differ (x86-64) | `dyn/partD.py` |
| W22 | mold | `-init=<hidden function>` with `--gc-sections`: the function is collected, `DT_INIT = 0`, the library crashes at load | `dyn/partE.py` (`gc_init`) |
| W23 | lld 19, 22 | `--gc-sections` with a version script hiding a C-identifier section's symbols: the section is dropped, `__start_cid_ref` stays undefined in `.dynsym`, `dlopen` fails | `dyn/partE.py` (`gc_vers`) |
| W24 | lld 19, 22 | a `.ctors` input section stays a `.ctors` output section; its constructor never runs | `dyn/partE.py` |
| W25 | GNU ld (i386) | a Clang `-fno-pic` C++ object in `-shared`: text relocations in `.eh_frame`; the library crashes on a throw | `census/odd.sh` (A) |
| W26 | lld 19, 22 (i386) | an IFUNC's `%ebx`-relative IPLT entry is its canonical address; called from the executable it jumps through the wrong GOT (`SIGSEGV`) | `dyn/partD.py` |
| W27 | mold (i386) | `call h_hid` to a hidden IFUNC goes through an `%ebx` stub from code that has not set `%ebx` | `dyn/partD.py` |
| W28 | GNU ld + gcc (i386) | `-shared -static` C++: no `--eh-frame-hdr` on gcc's line, the library aborts on a throw | `census/odd.sh` (B) |
| W29 | mold (i386) | `-shared -static` C library links silently and crashes on the first call | `census/odd.sh` (B) |
| W30 | all four, mortar included | an executable copies (`COPY`) writable data of a `-Bsymbolic` library: the executable writes 99, the library reads 10 (x86-64 PIE and `-no-pie`, AArch64 `-no-pie`). Reclassified by the second pass: a copy of read-only or RELRO data is defined behaviour (two equal constants); only the writable case is wrong, refused by R-D43 (D-126 item 2) | `tools/part_d.sh`; `exp2/q10b/run.sh` |
| W31 | lld 19, 22 | `--dynamic-list` with `-Bsymbolic` writes `DF_SYMBOLIC`; glibc binds the listed symbol locally (not interposable) although it is exported with a record | `export/partC.py` (`dynlist-f+Bsym`) |
| W32 | mold (AArch64) | `ADR_PREL_LO21`, `LD_PREL_LO19`, `PREL32` against preemptible, imported and weak keys link with no record: wrong values | `export/partD.py` |
| W33 | GNU ld (AArch64) | `PREL32` in `.data` against a preemptible key binds locally (interposition lost); against a weak undefined key gives a non-zero value | `export/partD.py` |
| W34 | GNU ld (x86-64) | `R_X86_64_PC32` in `.data` against a preemptible, weak or imported key becomes a dynamic `PC32` record the loader truncates to 32 bits | `export/partD.py` |
| W35 | mold (i386) | a `PC32` call without `%ebx` (`-fno-pic`) goes through a PLT that needs `%ebx`: `SIGSEGV` | `export/partD.py` |
| W36 | mold (i386) | `GOT32X` without a base register links: local keys read a GOT offset, others crash | `export/partD.py` |
| W37 | GNU ld, mold (i386) | `GOTOFF` to a preemptible key binds locally; mold gives imported and weak keys garbage | `export/partD.py` |
| W38 | mold (AArch64) | `-fno-pic` `ADRP`+`ADD` of an imported function takes a PLT stub's address: `&f` differs | `export/partD.py` (`addrext`) |
| W39 | lld 19, 22, mold (i386) | `R_386_TLS_IE` in `.text` (`-fno-pic`) links without `DT_TEXTREL`; the library crashes | `export/partD.py` (`tls`) |
| W40 | mold | `extern "C++" { *; }; local: *;` exports nothing | `export/partF.py` (`cxx_wild_all`) |
| W41 | lld 19, 22, mold (x86) | `GNU_PROPERTY_1_NEEDED_INDIRECT_EXTERN_ACCESS` dropped: a copy-relocating executable then runs with two instances of protected data, where GNU ld's library is refused by glibc | `export/partB.py` |
| W42 | mold | a versioned reference `vf@VA` is linked unversioned and `DT_NEEDED` dropped: `dlopen` fails `undefined symbol: vf`; with `-z defs` the valid link is refused | `export/partA.py` |
| W43 | lld 22 (x86-64) | `--dynamic-list` not listing an `STT_TLS` definition: the GD pair gets `DTPMOD64` naming the key and a static `DTPOFF` of the library's block; under an interposer with a different TLS layout the library reads `tv=7` (another variable) | `exp2/bsym2/tvprobe.sh` |
| W44 | GNU ld, lld, mold (AArch64) | GCC `-fPIE` code reaching its own exported datum through the section anchor and an instruction immediate links into `-shared` silently; a `-no-pie` host that copies the datum runs with two instances (`all=10 g=100`) | `exp2/gapdemo/run.sh` |

Every one is a test on which mortar refuses or is right (the rules
cited in §17.6). W44 is refused by D-117's analysis (WP-119: `exp3/s1d.py`
finds the `ldr` at `.data + 4`, inside `g`); the forms the analysis
does not follow are D-117's residual hole, each a
`shared-nonpic-analysis` case asserting that it links.

### 17.8 Anomalies and recorded differences (not wrong outputs)

Behaviours of one reference that mortar does not follow and that do not
by themselves produce a wrong program (each measured, `exp2/`):

| reference | behaviour | mortar | script |
|---|---|---|---|
| mold | an exported `GNU_IFUNC` is bound on every line (W20 where it is interposed) | D-112's table | `exp2/bsym2/run.sh` |
| mold | `--dynamic-list` then `-Bsymbolic-functions`: every data key (`OBJECT`, `NOTYPE`, `TLS`) preemptible, the listed function preemptible, `GNU_IFUNC` bound; `-Bsymbolic-functions` then `--dynamic-list`, and `-Bsymbolic` either way: the list decides | the list decides | `exp2/bsym2/run.sh`, `exp3/bsym3/run.sh` |
| mold | `--dynamic-list` then `-Bno-symbolic`: every key preemptible | the list decides | `exp2/bsym2/run.sh` |
| mold | `--dynamic-list-data`: accepted, no effect | refused | `exp2/bsym2/run.sh` |
| mold | `__start_`/`__stop_` of a referenced section not exported | exported `PROTECTED` | `exp2/startstop/run.sh` |
| mold | `.symver` `foo@V1` or `foo@@V1` matched by V1's `local:` kept | dropped, warned | `exp2/vscript2/run.sh`, `exp3/vs3/atat.py` |
| lld | `.symver` `foo@V1` hidden by V1's `local:` also when a `global:` glob of V1 matches it, and with `--export-dynamic`; `foo@@V1` hidden only by an exact `local:` | GNU ld's rule 12 | `exp3/vs3/expdyn.py`, `atat.py` |
| lld, mold | glob and escape syntax: both keep the `\` of `fo\o` and refuse an unterminated `[` and a glob ending in `\` (g37); lld matches nothing for `[!]]` and `[\]]`; mold reads `!` first in a class as a member and matches nothing with `global: *; local: *` (g40) | GNU ld's `fnmatch` | `exp3/vs3/globs.py` |
| lld | a `.symver` name (`old` of `old@V1`) named by an exact entry of another node: refused | kept at V1 (GNU ld, mold) | `exp3/vs3/expdyn.py` e10, `atat.py` a05 |
| mold | vmatch2 S16 (`V1 { global: *; local: *; }`) and X7 (`extern "C++" { *; }; local: *`, W40): no name exported; S19 (an exact name in two nodes): the last node; X6 (an exact `extern "C++"` name in V1, `_Z*` in V2): V2 | GNU ld's answer | `exp2/vmatch2/result.txt` |
| mold | an anonymous node combined with named ones accepted | refused | `exp2/vscript2/run.sh` |
| lld | quoted entries with `*` are globs (vmatch2 Q1, Q4) | exact | `exp2/vmatch2/result.txt` |
| lld | `global: fo*` in V1 and `local: foo*` (or `*`) in V2: local (vmatch2 S2, S11) | V1 (GNU ld, mold) | `exp2/vmatch2/result.txt` |
| lld, mold | a forward or missing parent accepted; a node name twice merged | refused | `exp2/vscript2/run.sh` |
| lld | an empty version script refused | accepted | `exp2/vscript2/run.sh` |
| GNU ld | `--dynamic-list` keeps unlisted `TLS` and `GNU_IFUNC` preemptible | bound (lld) | `exp2/bsym2/run.sh` |
| GNU ld | `-Bsymbolic-functions` also binds `NOTYPE` definitions | `FUNC` only (lld) | `exp2/bsym2/run.sh` |
| GNU ld | `local:` before `global:` a syntax error | accepted | `exp2/vmatch2/result.txt` |

## 18. Requests from §17 (WP-110)

Proposed wording or substance; the owning spec's maintainer lands it.
The packages of the record's §D name the requests each needs. **Every
request below is applied** (WP-115, `spec/landed/dyn/WP-115.md`): each
row opens with the owning spec and the id it now has, and the text
after it is the request as made.

### 18.1 Architecture specs

| id | target | request |
|---|---|---|
| R-A10 | arch §2.1, §4.3 (`Shared` row) | **applied: arch §2.1 (the `Shared` bullet), §3.4, §4.3, §4.4, §4.5.** `target_kind` reads D-113's `preemptible` bit; the `Shared` row is D-117's table: AbsWord in a read-only place `E` (was `D`+`TX`), AbsNarrow `E` for every kind, `Pc` resolved against `Local` and `E` against `Preemptible`/`Shlib`/`WeakDyn`, Branch `PLT` for those three; `C` and `CANON_PLT` never (D-122); no TLS relaxation (D-119) |
| R-A11 | a64 §5, §8 (not §11) | **applied: a64 §5.1.** a64's `Shared` cells as D-117 and D-119 (traditional GD kept with `DTPMOD64`/`DTPREL64`; TLSDESC eager in `.rela.dyn`; LE refused); a non-preemptible IFUNC's IPLT entry canonical in `Shared` (D-121) |
| R-X10 | x64 §4, §6.2 | **applied: x64 X-59-X-62 (§5.1).** the same for x86-64: GOT relaxation only against `Local`; `PC32` against a non-`Local` key `E` whatever the opcode; `DTPMOD64`/`DTPOFF64`/`TPOFF64`/`TLSDESC` with `r_sym` 0 for a local target; D-117's address rule judges only exact fields on x86-64 (absolute words, `GOTOFF64`, PC-relative fields in non-code places): a section-relative PC-relative code use is not judged, and WP-127 repeats `exp3/x86gt.py`'s measurement before x86-64 `Shared` ships |
| R-X86-10 | x86 (M4) | **applied: x86 X86-69-X86-73 (§5.1).** i386 `Shared`: D-130 (PLT with `%ebx`; `GOTOFF` against `Local` only; `GOT32X` without a base register refused; IFUNC refused); D-117's exact scope on i386 (`R_386_32`, `16`, `8`, `GOTOFF`, PC-relative fields in non-code places), section-relative `GOTOFF` to own exported data judged (GCC and Clang `-fPIE` emit it, `exp3/i386how.py`); WP-130 repeats the measurement |

### 18.2 Input, resolution, model, CLI, layout, synth, ODR, metadata

| id | target | request |
|---|---|---|
| R-R10 | resolve §4.7 | **applied: resolve §4.7 (the roots).** the keys named by `-init`/`-fini` are roots, every kind (D-124) |
| R-R11 | resolve §5.2 | **applied: resolve §5.2 (the D-118 rows, the D-13 row).** `--allow-shlib-undefined` is the default in `Shared`; `-z defs`/`--no-undefined` refuse undefined references in `Shared` with an indirect-dependency definition not counting (D-118); a versioned reference needs its version (D-118) |
| R-M10 | model §3 (`Resolution`) | **applied: model §3.3 (the `preemptible` key bit, check (8)).** a `preemptible` bit per key (D-113) |
| R-C10 | cli §2.3, §4.2, `docs/status/options.md` | **applied: cli §3.2, §3.6 (`bsymbolic`), §4.2 (the M4 shared-object block); `docs/status/options.md` by each package (WP-117 first).** per package (record §D): `-shared`, `-soname`/`-h`, `--version-script`/`-version-script`, `--no-undefined-version`, `--undefined-version`, `-z defs`/`--no-undefined`, `--whole-archive`, `-Bsymbolic`, `-Bsymbolic-functions`, `--dynamic-list`, `-Bno-symbolic`, `--exclude-libs`, `-z nodelete`/`nodlopen`/`initfirst`/`interpose`/`origin`, `--allow-shlib-undefined` supported; several `--version-script` options read as one script, in order (resolve §9.7 rule 13; all three, m-4); `-Bsymbolic-functions` and `-z now` in the first packages (`-Bsymbolic-functions` WP-117, `-z now` WP-118: Python's `LDSHARED` line and rustc's `cdylib` line carry them, record §A.4); `-Bsymbolic-non-weak*`, `--dynamic-list-data`, `--dynamic-list-cpp-new`, `--dynamic-list-cpp-typeinfo`, `-z notext`, `-f`/`-F`, `--spare-dynamic-tags` refused (class `option`, D-112); the `-Bsymbolic` and `-z defs` no-op rows written for executables keep their meaning there |
| R-L10 | layout L-100 | **applied: layout L-100.** withdrawn: no linker-defined symbol is exported from a shared object unless `ref_dynamic` (D-110) |
| R-L11 | layout L-103 | **applied: layout L-103.** last sentence replaced by D-101 (`_start` or `-e` in `Shared`; 0 silently when the default `_start` is undefined) |
| R-L12 | layout L-60 | **applied: layout L-60 rank 3.** `.gnu.version_d` between `.gnu.version` and `.gnu.version_r` (D-123) |
| R-S10 | synth S-3 | **applied: synth S-3 row 8, S-128.** row 8 (verdef) written by D-115; `.dynamic` of `Shared` per D-102; `DT_SONAME`, `DT_VERDEF*`, `DT_SYMBOLIC` rows |
| R-I10 | image I-40 | **applied: image I-123.** section header of `.gnu.version_d` (D-115) |
| R-O10 | odr O-24, O-50, O-51 | **applied: odr O-24 (amended), O-50, O-60.** a row's version is its `.dynsym` version node (D-115); `NONPREEMPTIBLE` is D-112's bound set, truthful but for D-117's residual hole (D-126 item 1); a consumer of a row (odr.md's cross-library checks) reads `NONPREEMPTIBLE` as R-D43 does (D-126 item 2) |
| R-MT10 | meta MT-32 | **applied: meta MT-32.** `--keep-mason-meta` applies to `Shared` (D-126) |
| R-N10 | incr IN-12 | **applied: incr IN-12a.** `-shared` stays an unsupported incremental kind until D-127's interface comparison is designed |

### 18.3 Requests to `plumb.md` (stage 3 for a shared object)

plumb.md §12 (P-D1) accepts only executables. A shared object cannot be
judged "resolves as the program assumed" from the library alone: what a
preemptible reference binds to depends on the process (D-112).

**What stands in for the host.** P-D9's loader model runs with a
synthetic first module, the **interposer**, in place of an executable:
it defines **every key the verifier computes as exported** (D-110, the
whole export set, preemptible and bound alike) **and every key the
inputs import**, each at a fresh address (a function at a fresh code
address, data at a fresh allocation of its `st_size`, a TLS key at a
fresh offset in a second module's block), as a preloaded library
would. The sets and each key's binding are recomputed from the inputs
and the options (D-110, D-112), never read from the output's `.dynsym`
or `DF_SYMBOLIC`. Two views: (A) no interposer: a defined key resolves
to the library's own definition, an import to its definition in the
shared inputs (P-D2); (B) the interposer first. The loader model
resolves every record of the output as glibc would, by the symbol it
names. Each input relocation's place must hold, in both views, the value
the input computes with `S` the **expected** definition: the library's
own for a key the verifier computes as bound, in both views and
whatever the interposer defines; the interposer's for a preemptible key
or an import in view B. So a record that names a bound key resolves to
the interposer in view B and its place differs from the expected value
(the re-review's B-1: with only the preemptible keys defined such a
record passed both views), and P-R10 names the cause. This is P-D9's
view 3 generalised from `WeakDyn` keys to every exported and imported
key, and it is what turns P-D9's equality into a check of D-112.

| id | request |
|---|---|
| P-R1 | **applied: plumb P-D16 (§12.5).** P-D1 accepts `ET_DYN` without `DF_1_PIE` and without `PT_INTERP` as `Shared`; P-D2-P-D13 apply with this section's changes; P-D9 runs the two views above in place of its views 1 and 3 (view 2, a second load bias, stays) |
| P-R2 | **applied: plumb P-D17 (§12.5).** for every relocation of an `SHF_ALLOC` place of a live input (`.eh_frame` and the LSDA included: they are checked by value, not by the address rule) the views agree with the input. Plus D-117's address-level clause: a use whose named address lies in a preemptible data definition is reported even when both views agree on its value, the address computed by the verifier's own implementation, from D-117's text, of (a) the relocation-level rule with its scope (exact fields; on x86 no PC-relative code field is judged, so no x86 instruction decoder), its boundary rule, and (b) on AArch64 the analysis (WP-119's def-use decoder over the input's code: D-117 items 1-9), each checked against the oracle `exp3/s1d.py` on the record's corpora. Not checked: non-alloc places (debug sections hold link-time addresses by design); the residual hole D-117 names |
| P-R3 | **applied: plumb P-D18 (§12.5).** every use of one IFUNC key, in every place and both views, yields one address: the IPLT entry for a bound key, the interposer's or the resolver's record for a preemptible one (W21); an i386 `Shared` output has no `STT_GNU_IFUNC` and no `IRELATIVE` (D-130) |
| P-R4 | **applied: plumb P-D19 (§12.5).** `.dynsym`'s defined set equals the verifier's export set (D-110, `__start_`/`__stop_` `PROTECTED` included), with each entry's binding, type, visibility and version (D-115) |
| P-R5 | **applied: plumb P-D20 (§12.5).** no `COPY`, no canonical PLT, no `DT_TEXTREL`, no dynamic record whose place is in a `PT_LOAD` without `PF_W` (outside `PT_GNU_RELRO`: such a record would write read-only text), no LE TLS form in the output's code (D-117, D-119, D-122), no output section `.ctors` or `.dtors` (their input goes to `.init_array`/`.fini_array`, D-124). i386: every place the output resolves to a PLT entry or GOT slot comes from an input relocation whose form sets `%ebx` (`PLT32`, `GOT32X` with a base register; D-130) |
| P-R6 | **applied: plumb P-D21 (§12.5).** each `PT_LOAD` has `p_align` the max page size and `p_offset ≡ p_vaddr (mod p_align)` (glibc checks only the page, D-123) |
| P-R7 | **applied: plumb P-D22 (§12.5).** `DT_SYMBOLIC` and `DF_SYMBOLIC` present iff the line's last `-Bsymbolic` form is `-Bsymbolic` and it has no `--dynamic-list` (D-112, D-102, D-103); `DT_INIT`/`DT_FINI` point at defined, live code (D-124); `DT_SONAME`, `DT_VERDEF*` consistent with `.gnu.version_d`; `GNU_PROPERTY_1_NEEDED_INDIRECT_EXTERN_ACCESS` present iff an input has it (D-112) |
| P-R8 | **applied: plumb P-D23 (§12.5).** mutations: each wrong output of §17.7 written into a correct mortar library (or, where it concerns the executable, a host) is reported as the table below says, and its test says so |
| P-R9 | **applied: plumb P-D24 (§12.5).** (executables, with R-D43) P-D11 gains: a `COPY` of a definition its library binds locally (`DT_SYMBOLIC`/`DF_SYMBOLIC`, or a `NONPREEMPTIBLE` row) whose `st_value` lies in a writable `PT_LOAD` of that library outside `PT_GNU_RELRO`, or in none, is reported |
| P-R10 | **applied: plumb P-D25 (§12.5).** no dynamic record of a `Shared` output names (`r_sym` not 0) a key the verifier computes as bound (D-112: hidden, protected, local by a version script, `-Bsymbolic`, `-Bsymbolic-functions` for a function, unlisted by `--dynamic-list`); a use of a bound key that needs a record has one of the kinds with `r_sym` 0: AArch64 `R_AARCH64_RELATIVE` (1027), `IRELATIVE` (1032), `TLS_DTPMOD64` (1028), `TLS_TPREL64` (1030), `TLSDESC` (1031); x86-64 `R_X86_64_RELATIVE` (8), `IRELATIVE` (37), `DTPMOD64` (16), `TPOFF64` (18), `TLSDESC` (36, refused as an input form by x64.md §6.2 until a compiler emits it); i386 `R_386_RELATIVE` (8), `IRELATIVE` (42, never in i386 `Shared`: D-130), `TLS_DTPMOD32` (35), `TLS_TPOFF` (14), `TLS_DESC` (41). Every other kind with `r_sym` 0 is reported (P-D9's `RELATIVE`/`IRELATIVE` symbol-0 rule stays) |
| P-R11 | **applied: plumb P-D26 (§12.5).** P-D12 for `Shared` (it replaces P-D12's executable forms; D-119): per model and binding, the records of a live TLS use are: GD and LD against a preemptible key or an import: `DTPMOD` + `DTPOFF` (AArch64 `TLS_DTPMOD64` + `TLS_DTPREL64`, x86-64 `DTPMOD64` + `DTPOFF64`, i386 `TLS_DTPMOD32` + `TLS_DTPOFF32`) both naming the key; GD and LD against a bound key: one `DTPMOD` with `r_sym` 0 per pair, the offset word the static offset with no record (a `DTPOFF` record with `r_sym` 0 holding the same offset is accepted from a reference); TLSDESC against a preemptible key or an import: one `TLSDESC` naming the key (in `.rela.dyn` for mortar, D-119; in `.rela.plt` from a reference); against a bound key: `TLSDESC` with `r_sym` 0 and the offset as addend (RELA) or in the descriptor's second word (i386 REL); IE against a preemptible key or an import: `TPREL64`/`TPOFF64`/`TLS_TPOFF` naming the key; against a bound key: the same type with `r_sym` 0 and the offset as addend (RELA) or in place (REL); LE: none (refused, P-R5). No relaxation: the number of `__tls_get_addr` calls equals the input's (D-119). `DF_STATIC_TLS` iff some IE slot exists. The values: in view B a preemptible key's module and offset are the interposer's, a bound key's the library's (P-R2). P-D12's D-16 direction (W14) applies unchanged |

P-R8's table (the P-D rules are main's plumb.md §12, applied through
P-R1; redone by the third pass against the interposer above, P-R10 and
P-R11; the last four rows are the wrong decisions of the re-review's
attack 2, each written into a correct mortar library):

| wrong output | reported by |
|---|---|
| W19 (LE TLS against own TLS, AArch64) | P-R5 (LE form in a shared object's code); P-R11 (a live TLS use without its records) |
| W20 (exported IFUNC bound) | P-R2, view B; P-R3 |
| W21 (two addresses of one IFUNC) | P-R3 |
| W22 (`DT_INIT = 0`, the function collected) | P-R7 |
| W23 (`__start_` undefined, section dropped) | P-R2 (the input's relocation against `__start_cid_ref` computes the section's address; the output's place holds 0); not P-R4: the version script makes `__start_cid_ref` local, so it is outside the export set |
| W24 (`.ctors` kept as an output section) | P-R5 |
| W25 (text relocations in `.eh_frame`, i386) | P-R5 (record in a non-writable `PT_LOAD`; `DT_TEXTREL`) |
| W26 (`%ebx`-relative IPLT entry as canonical address, i386) | P-R3 (no IFUNC in i386 `Shared`) |
| W27 (hidden IFUNC called through an `%ebx` stub, i386) | P-R3 |
| W28 (`-shared -static` C++ without `--eh-frame-hdr`, i386) | NOT caught: the image is what the line asks (no `--eh-frame-hdr`, so no `PT_GNU_EH_FRAME`); needs a check of the line, not the image (a `-shared` line with C++ unwind tables and no `--eh-frame-hdr`), proposed for WP-130 (i386), which reduces the root cause the record lists as untraced (D-100's `-shared -static` cell) |
| W29 (`-shared -static` C library crashes, mold i386) | NOT caught: the cause was not isolated (record §B); WP-130 (i386) reduces it to a record or a PLT form before a rule can name it |
| W30 (a `COPY` of writable locally bound data) | P-R9 (executables) |
| W31 (`DF_SYMBOLIC` with `--dynamic-list`) | P-R7; view B (P-R2) |
| W32 (`ADR_PREL_LO21`, `LD_PREL_LO19`, `PREL32` without a record, AArch64) | P-R2, view B |
| W33 (`PREL32` in `.data` bound; weak undefined non-zero) | P-R2, view B; P-D9 (P-R1) for the weak key |
| W34 (`PC32` dynamic record truncated, x86-64) | P-R2: view A with P-D9's second bias gives a truncated value |
| W35 (`PC32` call through an `%ebx` PLT, i386) | P-R5 (i386 clause) |
| W36 (`GOT32X` without a base register, i386) | P-R5 (i386 clause) |
| W37 (`GOTOFF` to a preemptible key, i386) | P-R2, view B |
| W38 (`&f` of an import is a PLT stub, AArch64) | P-R2: view A's value is the import's definition, the place holds the stub |
| W39 (`R_386_TLS_IE` in `.text` without `DT_TEXTREL`) | P-R5 (record in a non-writable `PT_LOAD`) |
| W40 (`extern "C++" {*}` exports nothing) | P-R4 |
| W41 (`NEEDED_INDIRECT_EXTERN_ACCESS` dropped) | P-R7 |
| W42 (versioned reference unversioned, `DT_NEEDED` dropped) | P-D6, P-D8 (P-R1) |
| W43 (TLS GD pair half bound, x86-64) | P-R10 (the `DTPMOD64` names `tv`, which `--dynamic-list` leaves bound) and P-R11 (a bound GD pair is `DTPMOD` with `r_sym` 0); P-R2, view B: the interposer defines `tv` (it is exported), the loader model resolves the `DTPMOD64` to the interposer's module while the offset is the library's, and the expected value is the library's own `tv` |
| W44 (anchor+immediate bound use, AArch64) | P-R2's address clause (b): the verifier's analysis derives `.data + 4`, inside `g`, from the `ldr` that follows the anchor (both views agree on the value, the clause reports it). What D-117's residual hole names stays not caught, by the verifier as by the linker |
| a use of a preemptible key bound locally through a named symbol (a `PREL32`, `ABS64` or `ADRP`/`ADD` resolved statically, no record) | P-R2, view B: the interposer defines the key, the place holds the library's own address |
| an export missing from `.dynsym` | P-R4 (the defined set differs from the verifier's export set); P-R2, view B, for every record that should have named it |
| a wrong version index on a defined entry or an import | P-R4 (a defined entry's version, D-115); P-D8 (an import's `.gnu.version` index against its Verneed and the shared definition's version, through P-R1) |
| a `JUMP_SLOT` naming the wrong symbol index | P-D10 (the slot's record is against another key than its PLT entry's); P-R2, view B (the interposer defines both keys at distinct addresses, the call place reaches the wrong one); P-R10 too when the named key is bound |

### 18.4 Requests to `diag.md` and to this spec

| id | target | request |
|---|---|---|
| R-D10 | diag §3.1 (symbols and relocations areas), §3.2 | **applied: diag §3.4 (planned kinds; §3.1 rows by WP-116).** new kinds: `non-pic-in-shared-object` (`NonPicInSharedObject { r_type, name, file }`, error, ``relocation {r_type} against symbol `{name}' which may bind externally can not be used when making a shared object; recompile with -fPIC``, GNU ld's; for D-117's address-level rule `{name}` is the definition and a note names the relocation's own symbol and place, and `-fdata-sections` at a boundary (D-117's boundary rule); for D-117's analysis the note names the function, the instruction's offset and the relocation that formed the address); `duplicate-version-pattern` (`DuplicateVersionPattern { pattern, first, second }`, error, `{pattern} is listed as global in version {first} and local in version {second}`, own; GNU ld: `duplicate expression`); `versioned-definition-hidden` (`VersionedDefinitionHidden { name, version }`, warning, `{name}@{version} is made local by version {version}'s local: entries; executables linked against {name}@{version} will not load`, own); `copy-of-locally-bound-definition` (`CopyOfLocallyBoundDefinition { name, file }`, error, `copy relocation against {name}, which {file} binds to itself; the program and {file} would use two different objects`, own; warning when R-D43 says so, with the note `{name} is read-only in {file}: two equal copies`); `version-script-undefined-name` (`VersionScriptUndefinedName { name, version }`, warning, `version script assigns {name} to {version}, but no file defines it`, own). `version-reassigned` exists (§3.1) and is used by resolve §9.7 rule 7 |
| R-D43 | this spec, D-43 (and D-18's neighbourhood) | **applied: this spec, D-43a.** the copy rule of D-126 item 2 for `Exec` and `Pie`: refuse `copy-of-locally-bound-definition` when the copied definition's library binds it locally and its `st_value` lies in a `PF_W` `PT_LOAD` outside `PT_GNU_RELRO` or in none; warn when it lies in a read-only `PT_LOAD` or in `PT_GNU_RELRO`. "Binds locally": `DT_SYMBOLIC`/`DF_SYMBOLIC`, or a `.note.mason` row with `NONPREEMPTIBLE`. Decided from the library's program headers and `.dynamic` (the symbol count from `DT_GNU_HASH` or `DT_HASH`), never its section headers. Its own package (record §D, WP-122); plumb request P-R9 |

## Appendix A. Experiments

Scripts: `/Volumes/AgentWork/mortar-helper/.build/review-m2/`
(uncommitted); `run-all.sh WORK` runs them all into a fresh directory,
each area into `WORK/<area>`, serially: 570 s measured by the review
(the census's mortar build dominates). The review's reproducers are in
`/Volumes/AgentWork/mortar-helper/.build/review-m2-adv/` (README there),
and this spec cites them by directory (`review-m2-adv/<dir>`).

| id | script | what | size |
|---|---|---|---|
| E-census | `census/run.sh` | 426 driver lines captured, linked by GNU ld, run; mortar (fresh build of main) on each line and on static probes; linker defaults | 22 drivers × 20 variations |
| E-corpus | `corpus/run.sh` | relocation census of C, C++, Fortran objects from GCC 14, Clang 19/22 in 8 modes; system shared-object features | 75 954 rows; 120 executables run |
| E-dynsym | `dynsym/run.sh` | `.dynsym` set, order, fields, `.gnu.hash` formulas (`fit.py`), versions, aliases | 4 references × 3 architectures |
| E-dynamic | `dynamic/run.sh` (`matrix.sh`, `variants.sh`, `interp.sh`, `hashstyle.sh`, `debug.sh`, `lld_textrel_copyrel.sh`) | `.dynamic` tags and order, segments, loader behaviour, gdb/lldb, ldd, elflint | 108 outputs run, plus variants |
| E-reloc | `reloc/run.sh` | dynamic records, copies, canonical PLTs, IFUNCs, TLS, text relocations, lazy and `-z now`, self-checking programs | 48 programs × 4 references; 433 side experiments |
| E-shlib | `shlib/run.sh` | needed/as-needed, closure, shlib-undefined, versions, protected, hidden entries | 3 architectures × 3 references |
| E-shdr | `design/shdr.sh` | section headers of the dynamic sections | 3 references |
| E-usedset | `shlib/usedset.sh` (`usedset.py`) | an independent model of D-11-D-13 against GNU ld on every E-shlib experiment, X1-X3c and the default lines (F1) | 254 links, 2 architectures |
| E-dupdef | `shlib/dupdef_needed.sh` | two needed libraries define one datum: GNU ld's neededness and binding against first mention and D-19a order (N1) | 3 lines × 2 architectures × 3 linkers, run |
| E-needed | `dynamic/needed_order.sh` | GNU ld's `DT_NEEDED` order: duplicates, as-needed, scripts, groups, `-Bstatic`, under-linking; constructor order (F5) | 75 cases × 2 architectures × 4 linkers |
| E-weakpc | `reloc/weakpc.sh` (`weakcls.py`) | weak undefined: which compilers produce which classes; the references' handling in PIE and `-no-pie`; `-z [no]dynamic-undefined-weak`; the hook program with `LD_PRELOAD`; mixed GOT/absolute uses (F2) | 1008 objects; links per combo |
| E-mixed | `corpus/mixed.sh` | links mixing `-fno-pic`, `-fPIE`, `-fPIC`, `-fno-plt` objects; equality checks | C and C++, GCC and Clang 22, 2 architectures, 3 references, PIE and `-no-pie` |
| review | `/Volumes/AgentWork/mortar-helper/.build/review-m2-adv/{ldso,weaksplit,copyro,canonpie,unique,dyntags,…}/run.sh` | the adversarial review's reproducers; `ldso`, `weaksplit`, `copyro`, `canonpie`, `unique`, `dyntags` re-run for the first fix pass; `dupdef`, `weaksplit2`, `neededorder` (`underlink-after`) cited from the re-review | — |
| E-shared-census | `tests/shared-design/exp/census/run.sh` (`census.py`, `realbuilds.sh`, `tools.sh`, `odd.sh`, `survey.py`) | §17: 723 `-shared` driver lines (21 drivers × 35 variations), the survey's 19 lines, zlib, Lua modules, googletest, libtool-shaped and CMake lines, by four references, run | about 80 s |
| E-shared-export | `tests/shared-design/exp/export/run.sh` (`partA.py`-`partG.py`) | §17: export set, protected, preemption under `LD_PRELOAD`, relocation classes × target kinds, versions written, archives | 3 architectures × 4 references; about 2 min |
| E-shared-dyn | `tests/shared-design/exp/dyn/run.sh` (`partA.py`-`partF.py`) | §17: the kind's headers, `.dynamic` per option and its run-time effect, TLS, IFUNC, other rules, `p_align` and the loader | 3 architectures × 4 references; about 45 s |
| E-shared-tools | `tests/shared-design/exp/tools/run.sh` (`part_a.sh`-`part_e.sh`) | §17: gdb, lldb, ldd, `ld.so --verify`, elflint on libraries; `dlopen`, TLS, EH, Fortran; which definition a library's own references reach; mortar-linked hosts | 21 libraries per architecture and reference; about 70 s |
| E-shared-exp2 | `tests/shared-design/exp2/` (README; `b1/`, `gapdemo/`, `escape/`, `bsym2/`, `vmatch2/`, `vscript2/`, `comments/`, `startstop/`, `q10b/`, `dynorder/`, `maxpage/`, `ldlines/`) | §17's second pass: the address-level rule over about 14,400 objects; the anchor gap; the protected escape; preemption per line; version-script model and grammar; `__start_`; copies of locally bound data; orders; max page; generators' lines (record §F) | each under a minute but `b1/` (about 10 min) |
| E-shared-exp3 | `tests/shared-design/exp3/` (README; `s1d.py` the analysis's oracle, `gt3.py`, `x86gt.py`, `i386how.py`, `run_s1d.sh`, `run_modes.sh`, `run_x86.sh`, `run_bnd.sh`, `hod/build.sh`, `i386/build.sh`, `vs3/run.sh`, `ss/run.sh`, `bsym3/run.sh`) | §17's third pass: D-117's analysis (WP-119) and its detection table, its control-flow variants, the x86 exact scope, hod and i386 producers, the boundary rule; version-script globs, duplicates, rule 12 with `@@` and `-E`, both models over 213 scripts; `-shared -static`; mold's `--dynamic-list` order (record §G) | background, at most 4 jobs; `run_s1d.sh` and `run_bnd.sh` over the 11,699 system members are the longest |

`tests/shared-design/` holds the scripts, sources, lists and small
results of these experiments (WP-115; its README says what was left
behind: objects, third-party trees, reference outputs, the largest
result files); `exp/run-all.sh WORK` runs the four into a fresh
directory. The scripts still name the `.build/target-wp-110/` paths
they were run from.

Tools: GCC/G++/gfortran 14.2.0-19 (native and cross), clang 19.1.7 and
22.1.8, GNU ld 2.44, ld.lld 19.1.7 and 22.1.8, mold 2.37.1, qemu-user
10.0.13, gdb 16.3, lldb 19.1.7, readelf 2.44, llvm-readelf 19.1.7,
eu-elflint 0.192, Python 3.13.5, glibc 2.41 (Debian 13).

## Appendix B. Not verified

- mortar's own dynamic outputs were measured from WP-053 (AArch64) and
  WP-055 (x86-64) on: `tests/link/dyn` links, runs and verifies them on
  both architectures. Test names a rule cites that `tests/link/dyn` does
  not have (`dynstr-layout` of D-24; `shlib-undefined`,
  `shlib-undefined-closure`, `shlib-undefined-dropped` of D-13) are
  measured on the references only.
- A 64 KiB-page AArch64 kernel (D-61's RELRO end): none available.
- An input section named `.interp` (D-31) and a canonical PLT in a PIE
  (D-44): no corpus input produces them.
- Q1's refusal (`archive member needed by an --as-needed shared
  object`): measured on the six-line experiment of D-12 only; no census
  line was found that reaches it.
- Large-model TLSDESC kept eager (D-48): no compiler writes it, so
  `tls-desc-large` writes it by hand; mortar's eager `TLSDESC` record runs
  under glibc (a64 Q10 closed), but no reference runs that form (GNU ld
  relaxes it to IE and LE, lld and mold refuse relocations 565-568,
  a64 §8.2).
- i386: `qemu-i386 ld.so --list` dies with `SIGSEGV` (and dumps core) on
  lld's i386 non-PIE outputs, which run; whether native i386 `ldd` lists
  them was not checked (no i386 kernel).
- Clang Fortran (no flang); clang `-fsanitize=address` (no runtime);
  gcc asan outputs under qemu.
- Natively run i386 programs: they ran under qemu-user. x86-64 programs
  run natively on the hosted x86-64 lane (WP-055: `tests/link/dyn`, gdb
  and lldb on mortar's PIE included); on the AArch64 host they run under
  qemu-user, where the harness skips the debugger steps.
- lldb under qemu for x86: run by hand only (WP-055: lldb 19 with
  `platform select qemu-user` stops at `main`; `image list` shows only
  the executable, GNU ld's too).
- E-usedset takes the live relocatable objects from GNU ld's `-t -t`
  trace, not from its own archive model; a multi-round chain of
  under-linking (D-11 item 3 more than once) and the tie-break of
  `giver(k)` between an earlier as-needed member of C and an object in
  `reach(N0)` are not in the corpus.
- D-19a's group rule rests on two cases (`grp_rescan_*`); i386, an
  as-needed library made needed by an archive member extracted later,
  and versioned references as the reason were not measured.
- E-weakpc and E-mixed: no i386 or Fortran or kiln+hod objects in the
  mixed links; `weakpc.sh` was last run stage by stage (the `weakmix`
  stage separately), not end to end; the cause of GNU ld's x86-64 IFUNC
  inequality (W3) is not diagnosed.
- D-25's class table (N3) is read from the M1/M2 type sets of a64.md,
  x64.md and x86.md, not measured row by row: what each reference does
  with a weak key under `CONDBR19`/`TSTBR14`, `MOVW_PREL_*`, the
  `*_ABS_LO12_NC` halves, `GOTOFF`/`GOTREL*` and a `PC32` on a jcc was not
  linked; the table is the conservative side (unfillable) for each.
- `bind` (N1) is measured on one datum and three lines
  (`shlib/dupdef_needed.sh`); a duplicate definition with different
  versions, protected visibility or IFUNC type in two needed libraries,
  and a binding into U, were not linked. mortar's line-1 image was not
  built for this measurement; it is GNU ld's line-3 image by
  construction (same `DT_NEEDED` list and copy), which runs.
- N5's executed image is the review's (`neededorder/run.sh`,
  `underlink-after` with `nq` forced), not re-run for this revision.
