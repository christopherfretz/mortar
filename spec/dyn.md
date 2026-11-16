# spec: dynamic executables (`mortar_synth` dynamic tables; M2)

**Status: accepted (WP-040, after an adversarial review and a re-review); §11's requests applied to the owning specs and §15's questions ruled (WP-040r), Q1 measured and closed (WP-043).**

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
| `-export-dynamic`/`-E` | 22 | noop | supported (D-21); refused by name in a static-pie (D-1, WP-056 §3.1) |
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
M4). Of the variations a real build adds (`x-` in the census lane),
`-static-pie --export-dynamic` is refused by name (D-1; WP-056 §3.1).

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
| `.dynsym` | none | D-21 | D-21 | the null entry only: `--export-dynamic` is refused by name [M4], `--export-dynamic-symbol` accepted with no effect (WP-056 measured exports from a static-pie useless; §13.1) |
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
| `WeakDyn` | `STB_WEAK`, `STT_NOTYPE` | `STV_DEFAULT` | `SHN_UNDEF` | 0 | 0 |

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
usually `ELFOSABI_GNU`. Only GNU ld also writes `EI_OSABI` 3; lld and
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
| 25 | `DT_AARCH64_VARIANT_PCS` | a64 §10.3 (a `JUMP_SLOT` symbol is `STO_AARCH64_VARIANT_PCS`; all four references, E-dynamic §1) | 0 |
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
| `--export-dynamic` in a static-pie | exports every global (x86-64: the program dies at start-up) | exports every global | exports every global | refused by name (D-1); the exports are useless: a library the program `dlopen`s does not bind to them and `dlsym(RTLD_DEFAULT)` does not find them (WP-056, all three) |
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
