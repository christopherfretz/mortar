import sys
p = sys.argv[1]; s = open(p).read()
def rep(old, new):
    global s
    if s.count(old) != 1: raise SystemExit('count %d: %r' % (s.count(old), old[:70]))
    s = s.replace(old, new)

rep('''the requests to other specs, `spec/plumb.md` included, are `spec/dyn.md`
§18.

Status: draft for review (WP-110).''',
'''the requests to other specs, `spec/plumb.md` and `spec/diag.md`
included, are `spec/dyn.md` §18.

Status: second pass after the adversarial review
(`briefs/reviews/review-110-shared.md`: B-1, B-2, M-1-M-8, m-1-m-9);
every changed rule is backed by a script of `exp2/` (Appendix A) whose
results are in §F.''')

rep('''| default `-fPIE` object (`nopic`) | 7 (all i386) / 7 | AArch64 `R_AARCH64_ADR_PREL_PG_HI21 … may bind externally … recompile with -fPIC`; x86-64 `R_X86_64_PC32 … can not be used when making a shared object`; i386 `DT_TEXTREL` with warnings |''',
'''| default `-fPIE` object (`nopic`) | 7 (all i386) / 7 | AArch64 `R_AARCH64_ADR_PREL_PG_HI21 … may bind externally … recompile with -fPIC`; x86-64 `R_X86_64_PC32 … can not be used when making a shared object`; i386 `DT_TEXTREL` with warnings. The census object names its datum by symbol; a GCC AArch64 `-fPIE` object that reaches its own exported datum through the section anchor links with all three, silently (W44, `exp2/gapdemo`): AArch64 `-fPIE` is neither always refused nor "as `-fPIC`" (dyn.md D-122's per-compiler table, §F.1) |''')

rep('''12 of 19 need nothing but `-shared`; 4 need `-soname` (and
`--dependency-file`, accepted); 3 need `-soname` and `--version-script`.''',
'''12 of 19 need nothing but `-shared` on the default line; among them
the 10 Lua module lines leave undefined references to load time, which
is D-118's `Shared` default (in S1a), not an option: they need nothing
beyond what the first package delivers. 4 need `-soname` (and
`--dependency-file`, accepted); 3 need `-soname` and `--version-script`.''')

rep('''7. Seen on census lines only, can stay refused in the first package:
   `-Bsymbolic*`, `--dynamic-list`, `--exclude-libs`, `-z nodelete`/
   `initfirst`/`interpose`, `-u`, `-init`/`-fini` with `-shared`
   (`-init`/`-fini` and `-z now` are already supported for executables).''',
'''7. **Two generators outside the survey, measured** (`exp2/ldlines`):
   - Python 3.13's `sysconfig` `LDSHARED`, the line of every setuptools
     extension module: `aarch64-linux-gnu-gcc -shared -Wl,-O1
     -Wl,-Bsymbolic-functions -Wl,-z,relro`. So `-Bsymbolic-functions`
     is on a common default line and moves into the first package
     (S1a); `-O1` is already a no-op (`docs/status/options.md`), `-z
     relro` the default.
   - rustc 1.95's `cdylib` line: `--version-script` (an anonymous
     script: `{ global: rs_add; local: *; };`), `--no-undefined-version`,
     `--as-needed`, the `std` rlibs under `-Bstatic`, `-Bdynamic`,
     `--eh-frame-hdr`, `-z noexecstack`, `--gc-sections`, `-shared`,
     `-z relro,-z now`, `-O1`, `-nodefaultlibs`; the output has
     `BIND_NOW` and `DF_1_NOW`. So `-z now` in `Shared` moves into S1b
     (D-49 unchanged); the anonymous script and `--no-undefined-version`
     are S3, after which a Rust `cdylib` links. Its rlibs pass D-117's
     address-level rule (405 objects, 0 refused, §F.1).
8. Seen on census lines only, can stay refused in the first package:
   `-Bsymbolic`, `--dynamic-list`, `--exclude-libs`, `-z nodelete`/
   `initfirst`/`interpose`, `-u`, `-init`/`-fini` with `-shared`
   (`-init`/`-fini` are already supported for executables).''')
rep('''8. Refused by name, never linked: text relocations (i386 non-PIC: GNU ld
   links 14 census lines, W25 crashes), non-PIC 64-bit objects (all
   three refuse), LE TLS (D-119).''',
'''9. Refused by name, never linked: text relocations (i386 non-PIC: GNU ld
   links 14 census lines, W25 crashes), non-PIC 64-bit objects (all
   three refuse when the relocation names the symbol; mortar also when
   it names an address inside an exported datum, D-117), LE TLS
   (D-119).''')

# B.3 m-3
rep('''| Chunk list | synth S-3 row 8 (`.gnu.version_d`, "a version script defines one", M4) | incomplete: the condition is a version node with an exported member, or a `.symver` definition | D-115 |''',
'''| Chunk list | synth S-3 row 8 (`.gnu.version_d`, "a version script defines one", M4) | incomplete: the condition is D-115's, a script that names at least one version node (even one that matches nothing: GNU ld and mold write it, lld refuses an empty node set, `review-110-adv/vscript` `empty.map`), or a `.symver` definition | D-115 |''')

# C
rep('''| versions | D-114-D-116; `spec/resolve.md` §9.4, §9.7 amended (rules 5, 8, 9 ruled; 10-12 added); `spec/script.md` §9 (grammar) |''',
'''| versions | D-114-D-116; `spec/resolve.md` §9.4, §9.7 restated from the vmatch2 model (rules 6-13); `spec/script.md` §9 (grammar, comments, several files) |''')
rep('''| relocation classes, the illegal table | D-117 |''',
'''| relocation classes, the illegal table, the address-level rule | D-117 |''')
rep('''| disagreements, wrong outputs | dyn.md §17.6, §17.7 (W19-W42) |
| requests to other specs, `plumb.md` included | dyn.md §18 (R-A10-R-X86-10, R-R10-R-N10, P-R1-P-R8) |''',
'''| disagreements, wrong outputs, anomalies | dyn.md §17.6, §17.7 (W19-W44), §17.8 |
| requests to other specs, `plumb.md` and `diag.md` included | dyn.md §18 (R-A10-R-X86-10, R-R10-R-N10, P-R1-P-R9, R-D10, R-D43) |''')
open(p, 'w').write(s)
