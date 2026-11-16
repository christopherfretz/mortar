- **Q21. The anchor+immediate gap (D-117, W44).** GCC AArch64 `-fPIE`
  code that reaches an exported datum as `adrp`/`add` of the section
  anchor plus a load or store immediate names only the anchor: no
  relocation-level rule sees it, and every reference links it into a
  mislinked library. Measured: 19 of 84 bound definitions in the
  corpus, each in an object the rule refuses for another use; one
  constructed object (`exp2/gapdemo/gap2.c`) whose only bound uses are of
  this form links. Options: (a) decode, in the scan, the instructions
  that use the register an anchor `ADD :lo12:` defines, up to the end of
  the basic block, and apply the address rule to `anchor + immediate`
  (exact, AArch64 only, costs a walk over code that names an anchor of a
  section holding exported data); (b) a warning on any non-GOT use of
  such an anchor (no false refusal, but noise: the over-approximation
  count, 2-6 correct `-fPIC` objects per corpus group, §F.1, shows it
  fires on correct code); (c) the `DW_AT_producer` hint (`-g` objects
  record `-fPIC`/`-fPIE` in their flags) to refuse objects compiled
  without `-fPIC`: exact for GCC with `-g`, absent without it.
  *Recommendation:* (a) in a package after S1j (it is a scan-side
  addition with its own corpus measurement); until then the gap is
  stated in D-117, in P-R8's table (not caught) and by W44's test,
  which asserts today's behaviour.
- **Q22. An opt-in escape for B-1's refusal (the ruling's question).**
  The address rule refuses objects the references link: on the corpus 3
  of 54 GCC AArch64 `-fPIE` objects (`anchorgap.c`, the census
  `lib.c`, zlib's `zutil.c`) and `libgomp.a(env.o)`
  (`exp2/b1/onlyrule.py`); no census or survey `-shared` line (all
  `-fPIC`) and no rustc rlib. Candidate escape: export the bound
  definition `STV_PROTECTED` (what the code assumes) with a warning.
  Measured (`exp2/escape`): an executable that reaches a protected datum
  through a `COPY` is refused at link by all three (GNU ld ``copy reloc
  against protected `g' is dangerous`` on AArch64, ``copy relocation
  against non-copyable protected symbol`` on x86-64; lld ``cannot
  preempt symbol: g``; mold ``cannot create a copy relocation for
  protected symbol``) when it is linked against the protected build; a
  `-pie` host runs on AArch64 (GOT) and is refused on x86-64 (GCC's
  x86-64 `-fPIE` copies, D-43). An executable linked earlier against the
  default build and run against the protected one: glibc 2.41 warns
  ``copy relocation against non-copyable protected symbol`` and runs
  with two instances (`all=3 g=100`, AArch64 and x86-64); with the W41
  property (`-mno-direct-extern-access`, x86-64) the load is refused
  (``error due to GNU_PROPERTY_1_NEEDED_INDIRECT_EXTERN_ACCESS``)
  [AArch64 property unmeasured]. *Recommendation:* not in the first
  package (the ruling); if a real codebase needs it, an explicit option
  that exports such definitions protected, sets the W41 property where
  the target has one, and warns per definition, so that an old
  executable is refused by the loader instead of mislinked.

## F. Second-pass measurements

Every script is under
`/Volumes/AgentWork/mortar-helper/.build/target-wp-110/exp2/` (README
there; Appendix A), every result file next to it.

### F.1 B-1: the address-level rule (`b1/`)

`corpus.sh` compiles Lua 5.4.7, zlib 1.3.1, googletest 1.15.2, the
census library, libstdc++-style C++ and the constructed cases
(`src/boundary.c`, `src/anchorgap.c`, `src/vague.cc`) with GCC 14 and
Clang 22 for AArch64 and x86-64 (GCC for i386) in four modes (764
objects); `libtool-twins.sh` builds expat 2.6.4 and pcre2 10.44 by their
own `configure` and `make` (35 `.libs` `-fPIC` objects and their 35
default twins); `analyze.sh` runs `relscan.py` (the rule as code) over
those, the census objects, the toolchain's static runtime libraries,
`liblapack_pic.a`, distribution static libraries and rustc 1.95's
rlibs: about 14,400 objects. Refused objects per group
(`work/b1-analysis/<group>.txt`, line `RULE-objects-refused`):

| group | objects | refused |
|---|---|---|
| GCC, Clang × AArch64, x86-64 × `-fPIC`, `-fPIC -fno-semantic-interposition` | 54 each | 1-2 each: the constructed one-past-the-end words |
| the same, `-fvisibility-inlines-hidden` (C++ only) | 4 each | 1 each, the same |
| GCC i386 `-fPIC` and `-fPIE` | 50 each | 0 |
| census `-fPIC` / census default | 47 / 54 | 0 / 0 |
| libtool twins `-fPIC` / default | 35 / 35 | 0 / 0 |
| `liblapack_pic.a` | 1,983 | 0 |
| rustc std, core, alloc, compiler_builtins | 405 | 0 |
| static runtimes AArch64 / x86-64 | 1,635 / 1,496 | 1 (`libgomp.a(env.o)`) / 0 |
| GCC AArch64 `-fPIE` | 54 | 7 |
| GCC x86-64, Clang AArch64, Clang x86-64 `-fPIE` | 54 each | 1, 2, 2 |
| distribution `libc.a`, `libm.a`, BLAS, LAPACK, `libcrypt.a`, `libresolv.a` AArch64 / `libc.a`, `libm.a` x86-64 | 5,002 / 3,069 | 17 / 4 |

Every refusal in a `-fPIC` group is a one-past-the-end word of the
constructed cases (36 hits, every one tagged `start=end-of-local`); with
`-fdata-sections` all of them disappear (`b1/fds/run.sh`, GCC and Clang,
AArch64 and x86-64). In real code the one-past-the-end form appears only
in the non-PIC x86-64 `libc.a` (6 hits, 2 objects with nothing else).

Ground truth (`groundtruth.py`: the `-fPIC` twin names through the GOT
every exported datum its code uses; in the default twin those not
reached through the GOT are bound): every default object whose code
uses its own exported data is refused, by this rule or by D-117's table
against the symbol: GCC AArch64 7/7 (84 bound definitions: 17 named by
the rule, 48 by symbol, 19 unseen: the gap), GCC x86-64 7/7 (85: 3 and
82), Clang AArch64 7/7 (86: all by symbol), Clang x86-64 7/7 (86: 6 and
80), GCC i386 4/4 (5, by symbol); the libtool twins: no object uses its
own exported data (`gt.twins.txt`). Objects refused by the rule alone
(no symbol-named use, so the references link them): 3 of the 7 GCC
AArch64 ones and `libgomp.a(env.o)` (`onlyrule.py`, `onlyrule.txt`).

A definition-level over-approximation (refuse any non-GOT use of a
section that holds exported data) would refuse correct `-fPIC` objects:
`overapprox-objects` is 5, 5, 5, 6 in the four 64-bit `-fPIC` groups, 3
in GCC i386 `-fPIC`, 2 in the census `-fPIC` objects. So it is not the
rule.

### F.2 D-112: preemption per line (`bsym2/`)

14 definitions (`FUNC`, `OBJECT`, common, `NOTYPE`, `TLS`, `GNU_IFUNC`,
each global and weak, plus protected data, function and TLS) in one
library; a host calls through each and an `LD_PRELOAD` library defines
them all (`pre.c`; `pre3.c` places the TLS variable at another offset).
17 lines × GNU ld 2.44, lld 22, mold 2.37 × AArch64 and x86-64
(`result.txt`). `B`/`P` as D-112; `-` the reference refuses the line:

| line | GNU ld | lld | mold |
|---|---|---|---|
| none | all `P` | all `P` | all `P` but `GNU_IFUNC` `B` |
| `-Bsymbolic` | all `B`; `DT_SYMBOLIC`, `DF_SYMBOLIC` | all `B`; `DF_SYMBOLIC` | all `B`; no tag |
| `-Bsymbolic-functions` | `FUNC`, `NOTYPE` `B` | `FUNC` `B` | `FUNC`, `GNU_IFUNC` `B` |
| `--dynamic-list` (one function listed) | unlisted `B` but `TLS`, `GNU_IFUNC` `P` | unlisted `B` (x86-64 `TLS` half: W43) | unlisted `B` |
| list with `-Bsymbolic` (either order) | as the list | all `B`, the listed one too; `DF_SYMBOLIC` (W31) | as the list |
| list with `-Bsymbolic-functions` | as the list | as the list | the last of the two |
| `-Bsymbolic` or `-Bsymbolic-functions`, then `-Bno-symbolic` | all `P` | all `P` | all `P` (`GNU_IFUNC` `B`) |
| `-Bno-symbolic` or `-Bsymbolic-functions`, then `-Bsymbolic` | all `B` | all `B` | all `B` |
| `-Bsymbolic -Bsymbolic-functions` | as `-Bsymbolic-functions` | same | same |
| list, then `-Bno-symbolic` | as the list | as the list | all `P` |
| `--dynamic-list-data` | as `-Bsymbolic-functions` | - | no change |
| `--dynamic-list-cpp-new`, `-cpp-typeinfo` | data and functions `B`, `TLS`, `GNU_IFUNC` `P` | - | - |

Protected definitions are `B` on every line, every reference. W43
(`tvprobe.sh`): lld 22 x86-64, list not naming `tv`: the GD pair is
`DTPMOD64` naming `tv` and a static offset; with `pre3.c` the library
reads `tv=7` (the interposer's other variable). The same probe with GNU
ld and mold, and with lld on AArch64, reads the library's own `tv`.

### F.3 Version scripts (`vmatch2/`, `vscript2/`, `comments/`)

`vmatch2/model.py` is resolve §9.7 restated, written from the text; run
on 53 scripts × 3 names (`scripts.txt`), its GNU column equals GNU ld
on 159 of 159 rows; its mortar column differs from GNU ld only where
D-114 rules (`local:` before `global:` accepted: S8, L1, L2, E6). lld and
mold differ from GNU ld on S2, S11 (lld local), S12, S13, D2-D4, E5 (both
accept the duplicate), S16, X7 (mold none), S19, X6 (mold V2), Q1, Q4
(both glob a quoted entry), S8, L1, L2, E6 (both accept). `vscript2/`:
`.symver` `foo@V1` with V1's own `local: *`: GNU ld and lld drop it,
mold keeps it; with the `local:` in V2 (`*` or `foo`): all three keep
it; two script files: one script (all three); an empty script: lld
refuses; a forward or missing parent: GNU ld refuses (``unable to find
version dependency``), lld and mold accept; a node name twice: GNU ld
``duplicate version tag``, lld and mold merge; an anonymous node with
named ones, either order: GNU ld and lld refuse, mold accepts; one exact
name under `global:` in V1 and `local:` in V2: GNU ld ``duplicate
expression``. `comments/`: `#` (whole line, trailing, mid-entry: `foo;
#bar;` lists only `foo`) and `/* */` accepted by all three in version
scripts and dynamic lists; `//` an error in all three.

### F.4 `__start_`/`__stop_` (`startstop/`)

GNU ld and lld export both `GLOBAL PROTECTED`, with and without
`--gc-sections`, AArch64 and x86-64; mold exports neither; an
`LD_PRELOAD` definition of both never changes the library's count; the
section is kept in every case.

### F.5 Copies of locally bound data (`q10b/`)

A `-no-pie` host copies four data of a `-Bsymbolic` library (`.rodata`,
`.data.rel.ro`, `.data`, `.bss`): the first two run correctly, the
last two MISLINK, with every reference on both targets. `classify.py`
(program headers and `st_value` only) answers WARN, WARN, REFUSE,
REFUSE for each library, the runs' split, and on the system's
`libgdk_pixbuf-2.0.so` refuses `gdk_pixbuf_version` (`.data`) and warns
on the three `.rodata` constants.

### F.6 The protected escape (`escape/`)

As Q22 states (`run.sh`: link-time answers per host kind; `run2.sh`:
glibc's run-time answer with and without the W41 property).

### F.7 The anchor gap (`gapdemo/`)

`gap2.c` (`static int h1, h2; int g; static int h3`; `all()` reads
them, `set()` writes them) at GCC's default `-O2 -fPIE`, linked
`-shared` by each reference, with a `-no-pie` host that copies `g`:
AArch64, all three link it and the host prints `all=10 g=100` (right:
`all=107 g=7`); x86-64, all three refuse (`R_X86_64_PC32` against `g`);
`relscan.py` refuses the x86-64 object (by symbol) and not the AArch64
one (the gap).

### F.8 Orders the review did not check (`dynorder/check.py`)

`hash` mode: 18 lld libraries (up to 3,840 hashed symbols, ELF32
included): `.dynsym`'s partition and bucket order and the `.gnu.hash`
bytes equal D-26's model. `dynamic` mode: 13 census GNU ld libraries and
37 libraries of the `dyn` area (with `SYMBOLIC`, `FLAGS`, `RUNPATH`,
`VERDEF`, BTI): `.dynamic` in D-102's order. 0 mismatches.

### F.9 The max page (`maxpage/`)

AArch64's default max page is 64 KiB with every reference; libraries
linked with `-z max-page-size=4096` and `65536` by GNU ld, lld and mold
all load and run on this 4 KiB kernel; every `PT_LOAD` is congruent
modulo its `p_align`; lld's RELRO end (0x21000) is not 64 KiB-aligned,
GNU ld's (0x20000) and mold's (0x30000) are. A 64 KiB kernel is not
available (qemu-user cannot change the host's page size): Appendix B.

### F.10 Generators' lines (`ldlines/`)

As §A.4 item 7: Python's `LDSHARED` and rustc 1.95's `cdylib` line.

