import sys
p = sys.argv[1]; s = open(p).read()
def rep(old, new):
    global s
    if s.count(old) != 1: raise SystemExit('count %d: %r' % (s.count(old), old[:70]))
    s = s.replace(old, new)

# D-122 tests
rep('''refused (D-117). *Tests:*
`shared-nonpic-refused` (refuse, each architecture), `shared-fpie-objects`.''',
'''refused (D-117). *Tests:*
`shared-nonpic-refused` (refuse, each architecture), `shared-fpie-objects`
(the four `-fPIE` rows of this table, each refused or linked as it says).''')

# D-126 items 1 and 2
rep('''   (request R-O10).
2. **A copy of a definition that its library binds locally is refused**
   in an executable (`copy-of-locally-bound-definition`; a new rule for
   `Exec` and `Pie` that needs no `Shared` output). The executable's
   copy and the library's own instance are then two objects: the
   executable writes `v = 99`, the library reads 10 (`E-shared-tools`
   D, x86-64 PIE and `-no-pie`, AArch64 `-no-pie`; every reference links
   it silently, mortar included: W30). mortar knows a definition is
   bound locally when its library has `DF_SYMBOLIC` (GNU ld and lld write
   it; mold does not, so its `-Bsymbolic` libraries are not caught) or
   when its `.note.mason` row carries `NONPREEMPTIBLE` (any mortar-made
   library with a table, `-Bsymbolic-functions` excluded since it binds
   functions only). Protected data is already refused (D-18). The fix in
   the program is to reach the datum through the GOT (`-fPIC`, or
   Clang's default `-fPIE`), which all four then link correctly.''',
'''   (request R-O10). The bit is truthful because D-117's address-level
   rule refuses a library in which non-PIC code binds a preemptible
   datum without naming it: a row says `NONPREEMPTIBLE` clear only when
   every use of the definition goes through a record. The one exception
   is the anchor+immediate gap (D-117, Q21): a GCC AArch64 `-fPIE`
   object whose only bound use of a datum is of that form links, and its
   row says preemptible while the library's code is bound. A consumer of
   the table (an ODR check across libraries, Tuck) must not treat a clear
   bit as proof of interposability until Q21 is closed.
2. **A copy of a definition that its library binds locally** is a rule
   for `Exec` and `Pie`, not for `Shared`: it is request R-D43 against
   D-43 (§18.4), its own package (record §D, S-R43). Its substance: the
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
   default `-fPIE`), which all four then link correctly.''')
rep('''*Tests:* `odr-table-shared` (rows, `NONPREEMPTIBLE` per D-112 case,
versions), `copy-of-symbolic-refused` (refuse; W30; a GNU ld
`-Bsymbolic` library and a mortar one with a table).''',
'''*Tests:* `odr-table-shared` (rows, `NONPREEMPTIBLE` per D-112 case,
versions); R-D43's `copy-of-symbolic` (refuse `.data`/`.bss`, warn
`.rodata`/`.data.rel.ro`, refuse outside every `PT_LOAD`; a GNU ld and
an lld `-Bsymbolic` library and a mortar one with a table; W30).''')

# 17.6 rows
rep('''| linker-defined exports | none | none | none | none (D-110; L-100 withdrawn) |''',
'''| linker-defined exports | `__start_`/`__stop_` of a referenced C-identifier section, `PROTECTED` | same | none | GNU ld's and lld's (D-110 item 2; L-100's other exports withdrawn) |''')
rep('''| `local:` before `global:` | syntax error | accepted | accepted | accepted (D-114) |''',
'''| `local:` before `global:` | syntax error | accepted | accepted | accepted (D-114) |
| one entry under `global:` of one node and `local:` of another | refused (`duplicate expression`) | accepted, first | accepted, varies | refused, `duplicate-version-pattern` (D-114) |
| quoted entry with `*` (`"foo*"`) | exact | glob | glob | exact (D-114) |
| `global: fo*` in V1, `local: foo*` in V2 | V1 | local | V1 | V1 (D-114; resolve §9.7) |
| forward or missing parent; node name twice; anonymous with named nodes | refused | accepted; merged; refused | accepted; merged; accepted | refused (D-114; script.md §9.1) |
| empty version script | accepted | refused | accepted | accepted (script.md §9.1) |
| comments `#`, `/* */`; `//` | accepted; error | same | same | same (D-114) |''')
rep('''| `.symver` `foo@V1` matched by `local:` | dropped | dropped | kept | kept, warns (D-114, Q12) |''',
'''| `.symver` `foo@V1` matched by V1's own `local:` | dropped | dropped | kept | dropped, warns `versioned-definition-hidden` (D-114; resolve §9.7 rule 12) |
| `.symver` `foo@V1` matched by another node's `local:` | kept | kept | kept | kept (resolve §9.7 rule 12) |''')
rep('''| `-Bsymbolic-non-weak*` | unknown option | accepted | accepted | refused (D-112) |''',
'''| `-Bsymbolic-non-weak*` | unknown option | accepted | accepted | refused (D-112) |
| `-Bsymbolic-functions` binds | `FUNC`, `NOTYPE` | `FUNC` | `FUNC`, `GNU_IFUNC` | `FUNC` (D-112) |
| `--dynamic-list`: unlisted `TLS`, `GNU_IFUNC` | preemptible | bound (x86-64 `TLS` half: W43) | bound | bound (D-112) |
| `--dynamic-list` with a `-Bsymbolic` form | the list decides | the list, but `-Bsymbolic` binds all (W31) | the last option | the list decides (D-112) |
| `--dynamic-list-data`, `--dynamic-list-cpp-*` | accepted | unknown | `-data` a no-op; `-cpp-*` unknown | refused (D-112) |''')

# 17.7: W30 reclassified, W43, W44, 17.8
rep('''| W30 | all four, mortar included | an executable copies (`COPY`) data of a `-Bsymbolic` library: the executable writes 99, the library reads 10 (x86-64 PIE and `-no-pie`, AArch64 `-no-pie`) | `tools/part_d.sh` |''',
'''| W30 | all four, mortar included | an executable copies (`COPY`) writable data of a `-Bsymbolic` library: the executable writes 99, the library reads 10 (x86-64 PIE and `-no-pie`, AArch64 `-no-pie`). Reclassified by the second pass: a copy of read-only or RELRO data is defined behaviour (two equal constants); only the writable case is wrong, refused by R-D43 (D-126 item 2) | `tools/part_d.sh`; `exp2/q10b/run.sh` |''')
rep('''| W42 | mold | a versioned reference `vf@VA` is linked unversioned and `DT_NEEDED` dropped: `dlopen` fails `undefined symbol: vf`; with `-z defs` the valid link is refused | `export/partA.py` |

Every one is a test on which mortar refuses or is right (the rules
cited in §17.6).''',
'''| W42 | mold | a versioned reference `vf@VA` is linked unversioned and `DT_NEEDED` dropped: `dlopen` fails `undefined symbol: vf`; with `-z defs` the valid link is refused | `export/partA.py` |
| W43 | lld 22 (x86-64) | `--dynamic-list` not listing an `STT_TLS` definition: the GD pair gets `DTPMOD64` naming the key and a static `DTPOFF` of the library's block; under an interposer with a different TLS layout the library reads `tv=7` (another variable) | `exp2/bsym2/tvprobe.sh` |
| W44 | GNU ld, lld, mold (AArch64) | GCC `-fPIE` code reaching its own exported datum through the section anchor and an instruction immediate links into `-shared` silently; a `-no-pie` host that copies the datum runs with two instances (`all=10 g=100`) | `exp2/gapdemo/run.sh` |

Every one is a test on which mortar refuses or is right (the rules
cited in §17.6), except W44: mortar links it too (D-117's known gap,
Q21), and its test asserts the current behaviour until Q21 is closed.

### 17.8 Anomalies and recorded differences (not wrong outputs)

Behaviours of one reference that mortar does not follow and that do not
by themselves produce a wrong program (each measured, `exp2/`):

| reference | behaviour | mortar | script |
|---|---|---|---|
| mold | an exported `GNU_IFUNC` is bound on every line (W20 where it is interposed) | D-112's table | `exp2/bsym2/run.sh` |
| mold | `--dynamic-list` with `-Bsymbolic-functions`: whichever comes last decides; with `-Bsymbolic` the list decides | the list decides | `exp2/bsym2/run.sh` |
| mold | `--dynamic-list` then `-Bno-symbolic`: every key preemptible | the list decides | `exp2/bsym2/run.sh` |
| mold | `--dynamic-list-data`: accepted, no effect | refused | `exp2/bsym2/run.sh` |
| mold | `__start_`/`__stop_` of a referenced section not exported | exported `PROTECTED` | `exp2/startstop/run.sh` |
| mold | `.symver` `foo@V1` matched by V1's `local:` kept | dropped, warned | `exp2/vscript2/run.sh` |
| mold | vmatch2 S16, X7 (a C++ `*` in one node, C names in another): no name exported; S19, X6: the last node | GNU ld's answer | `exp2/vmatch2/result.txt` |
| mold | an anonymous node combined with named ones accepted | refused | `exp2/vscript2/run.sh` |
| lld | quoted entries with `*` are globs (vmatch2 Q1, Q4) | exact | `exp2/vmatch2/result.txt` |
| lld | `global: fo*` in V1 and `local: foo*` (or `*`) in V2: local (vmatch2 S2, S11) | V1 (GNU ld, mold) | `exp2/vmatch2/result.txt` |
| lld, mold | a forward or missing parent accepted; a node name twice merged | refused | `exp2/vscript2/run.sh` |
| lld | an empty version script refused | accepted | `exp2/vscript2/run.sh` |
| GNU ld | `--dynamic-list` keeps unlisted `TLS` and `GNU_IFUNC` preemptible | bound (lld) | `exp2/bsym2/run.sh` |
| GNU ld | `-Bsymbolic-functions` also binds `NOTYPE` definitions | `FUNC` only (lld) | `exp2/bsym2/run.sh` |
| GNU ld | `local:` before `global:` a syntax error | accepted | `exp2/vmatch2/result.txt` |''')
open(p, 'w').write(s)
