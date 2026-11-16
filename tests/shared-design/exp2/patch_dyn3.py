import sys
p = sys.argv[1]; s = open(p).read()
def rep(old, new, count=1):
    global s
    n = s.count(old)
    if n != count: raise SystemExit('expected %d got %d: %r' % (count, n, old[:80]))
    s = s.replace(old, new)

rep('''Requests R-A10 (arch §4.3's `Shared` row: `TX` becomes `E`; `Pc` and
narrow absolute against `Local` resolved), R-A11 (a64), R-X10 (x64),
R-X86-10 (x86). *Tests:* `shared-reloc-table` (every row and kind, each
architecture, run plain and interposed; the refusals' messages).''',
'''Requests R-A10 (arch §4.3's `Shared` row: `TX` becomes `E`; `Pc` and
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
  `st_value + A` (A: the RELA addend, or the REL implicit addend), and,
  for an x86-64 or i386 PC-relative or `PLT32` field in an
  `SHF_EXECINSTR` section, `st_value + A + 4 + t`, where t is the least
  of 0, 1, 2, 4 for which that address is the `st_value` of a symbol of
  T with a non-zero `st_size` (the immediate after the displacement),
  else 0.
- A use whose named address lies in a preemptible data definition's
  interval is refused, `non-pic-in-shared-object`, naming the
  definition, the object, the relocation and its place, and the remedy
  ``recompile with -fPIC``. When the address is also the end of a local
  object of T (one past the end of a preceding `static` array, which is
  then the next definition's start) the note names `-fdata-sections` as
  the remedy: the use is ambiguous, and refused.

Not uses: debug and other non-alloc places (every `DW_OP_addr` of an
exported variable, 864k measured references in `.debug_*` against
functions and data), `.eh_frame` (FDE `PREL32` to functions: 44,869)
and the LSDA; uses against a function definition (an exported function
reached through `.L$local` or `.localalias` under
`-fno-semantic-interposition` is the compilers' documented contract,
and both compilers emit it in `-fPIC` code: 1,169 such uses, not
refused). Measured with `exp2/b1/relscan.py` (the rule as code) over
14,316 objects (`exp2/b1/analyze.sh`):

| corpus | objects | refused | what |
|---|---|---|---|
| GCC 14, Clang 22, AArch64 and x86-64, `-fPIC`, `-fPIC -fno-semantic-interposition`, `-fPIC -fvisibility-inlines-hidden`; GCC i386 `-fPIC` (Lua, zlib, googletest, the census sources, libstdc++-style C++) | 498 | 18 | only the constructed one-past-the-end words of `boundary.c` and `vague.cc`, every compiler and target; 0 with `-fdata-sections` (`exp2/b1/fds`) |
| the census's `-fPIC` objects (the 25 real lines: Lua modules, zlib, libtool and CMake objects) | 47 | 0 | |
| expat 2.6.4 and pcre2 10.44 by their own `configure`+`make`, libtool's `.libs` objects | 35 | 0 | |
| `liblapack_pic.a`; rustc 1.95's `std`, `core`, `alloc`, `compiler_builtins` rlibs (a `cdylib` links them) | 2,388 | 0 | |
| `libgcc.a`, `libgcc_eh.a`, `libstdc++.a`, `libsupc++.a`, `libgfortran.a`, `libatomic.a`, `libgomp.a` (`-static-lib*` in a library), AArch64 and x86-64 | 3,131 | 1 | `libgomp.a(env.o)`, a non-PIC member that binds its exported `gomp_def_allocator_envvar` (`.bss + 8`): a correct refusal |
| GCC AArch64 `-fPIE` (the default), the same sources | 54 | 7 | every object whose code reaches its own exported data (7 of 7, ground truth from the `-fPIC` twin, `exp2/b1/groundtruth.py`); 17 of the 84 bound definitions named individually, the other 48 by D-117 against the symbol, 19 not seen (the gap below) |
| GCC x86-64, Clang AArch64 and x86-64, GCC i386 `-fPIE` | 212 | 3 | the boundary cases; every bound definition (261) is named by symbol, refused by D-117's table already |
| distribution `libc.a`, `libm.a`, BLAS, LAPACK, `libcrypt.a`, `libresolv.a` (default, non-PIC) | 8,071 | 21 | non-PIC members (correct refusals) |

The **known gap**: GCC AArch64 `-fPIE` code that reaches a definition
through an anchor **and** an instruction immediate (`adrp x0, .data;
add x0, x0, :lo12:.data; ldr w1, [x0, #8]`) names only the anchor's
start: the rule sees `.data + 0` (a local `static`), not `g`. Ground
truth on the corpus: 19 of 84 bound definitions (18 in googletest, 1 in
`vague.cc`), every one in an object the rule refuses for another use,
except where an object's only bound uses are of this form (none in the
corpus; `exp2/probe/gap2.c` is one, linked silently by all three and by
this rule). A definition-level over-approximation (refuse every non-GOT
use of a section holding exported data) would refuse correct `-fPIC`
objects of zlib, Lua and googletest (record §F.1), so it is not the rule.
The gap is recorded as open question Q21 and as the class of mislink
§18.3's verifier cannot see (P-R8).

The rule depends on D-112: under `-Bsymbolic` (or a `--dynamic-list`
not listing it) the definition is bound and the same object links
correctly. It never changes a link of an executable. *Tests:*
`shared-nonpic-anchor` (GCC AArch64 `-fPIE`: refused, naming `g`;
`-fPIC`: links), `shared-nonpic-boundary` (the one-past-the-end word:
refused with the `-fdata-sections` note; with it: links),
`shared-nonpic-not-uses` (debug, `.eh_frame`, `$local` function
aliases: links), `shared-reloc-table` (every row and kind, each
architecture, run plain and interposed; the refusals' messages).''')

# D-122 restated per compiler and target
rep('''The same applies to a non-GOT use
of a preemptible definition of the library itself (D-112): x86-64
`-fPIE` code (`R_X86_64_PC32` against a default-visibility global) is
refused, AArch64 and i386 `-fPIE` code links as `-fPIC` (all three).
A text relocation is refused (D-117).''',
'''The same applies to a non-GOT use
of a preemptible definition of the library itself (D-112), which is what
`-fPIE` code gives, per compiler and target (`exp2/b1`, ground truth
from `-fPIC` twins; `review-110-adv/fpie`):

| `-fPIE` object | own exported data | own exported function's address | in `-shared` |
|---|---|---|---|
| GCC 14, AArch64 | through the section anchor (`.data + off`, local symbol), or by symbol | by symbol (`ADR_PREL_PG_HI21 f`) | refused: by D-117's address-level rule (anchor), by D-117's table (symbol); the anchor+immediate form is the gap (Q21) |
| Clang 22, AArch64 | by symbol (`ADR_PREL_PG_HI21 g`) | by symbol | refused by D-117's table (all three refuse it too) |
| GCC 14 and Clang 22, x86-64 | by symbol (`R_X86_64_PC32 g`) | by symbol | refused by D-117's table (all three refuse) |
| GCC 14, i386 | through the GOT (`GOT32X`, `GOTOFF` to locals only) | through the GOT | links as `-fPIC` (no own exported datum bound in 50 sources) |

So "AArch64 `-fPIE` links as `-fPIC`" holds for none of the 64-bit
compilers; GCC's i386 `-fPIE` is the one that does. A text relocation is
refused (D-117).''')
open(p, 'w').write(s)
