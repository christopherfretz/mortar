# WP-022 stage 1: the record behind `spec/resolve.md`

Design only: no product code. This file keeps the method, the experiments
that back every claim `spec/resolve.md` makes about GNU ld, lld and mold,
and the decisions with their reasons. "E*n*" in the spec is section E*n*
here.

## Method

Host: AArch64 Linux container. Linkers: GNU ld (`ld.bfd`) 2.44,
`ld.lld-22` 22.1.8, mold 2.37.1. Compilers: GCC 14 (`gcc`, `g++`,
`gfortran`), GNU as; `x86_64-linux-gnu-*` and `i686-linux-gnu-*` cross
binutils for the per-architecture symbol tables. Objects were built with
`gcc -c` (distribution defaults), `-fcommon` where a common symbol was
wanted, `-fPIC` for shared-object sources, and `as` for `.s` files. Shared
objects: `gcc -shared -fPIC -nostdlib` (with `-Wl,--version-script=` where
named). Inspection: `readelf -sSV --wide`, `nm`, `objdump -dr`.

Every static link ran through one driver, which links the same arguments
with each linker and runs the result (exit status = `main`'s return):

```sh
#!/bin/sh
# L <out> <args...>: link with bfd, lld, mold; print status and stderr; run if linked
out=$1; shift
for l in bfd lld mold; do
  case $l in bfd) LD=ld.bfd;; lld) LD=ld.lld-22;; mold) LD=mold;; esac
  rm -f $out.$l
  err=$($LD "$@" -o $out.$l 2>&1); st=$?
  printf '%s: link=%s' $l $st
  if [ -f $out.$l ] && [ $st = 0 ] && [ -z "$NORUN" ]; then ./$out.$l; printf ' run=%s' $?; fi
  echo
  [ -n "$err" ] && echo "$err" | sed 's/^/    /' | head -${LINES_MAX:-8}
done
```

with `start.s` as the only start file (no libc):

```
.globl _start
_start: bl main
 mov x8,#93
 svc #0
```

so a link line is `L o -static start.o <inputs>`. Dynamic experiments (E4,
E12, E16, E17, E20) add `-dynamic-linker /lib/ld-linux-aarch64.so.1` and
run with `LD_LIBRARY_PATH=.`. No reference linker's source was read; only
their behavior, their `--help` and GNU ld's manual.

## E1. Archive member choice and back-references

Sources: `a.c: int pick(void){return 1;}`, `c.c: …return 2;`,
`b.c: int pick(void); int user(void){return pick();}`,
`a2.c: int dup = 7; int pick(void){return 11;}`,
`main.c: int user(void); int dup = 5; int main(void){return user();}`.
Archives `libA=[a.o] libB=[b.o] libC=[c.o] libABC=[a.o b.o c.o]
libBAC=[b.o a.o c.o] libA2=[a2.o]`.

```
$ L o1 -static start.o main.o libA.a libB.a libC.a
bfd: link=0 run=2
lld: link=0 run=1
mold: link=0 run=1
$ L o2 -static start.o main.o --start-group libA.a libB.a libC.a --end-group
bfd: link=0 run=2
lld: link=0 run=1
mold: link=0 run=1
$ L o3 -static start.o main.o libABC.a
bfd: link=0 run=2
lld: link=0 run=1
mold: link=0 run=1
$ L o5 -static start.o main.o libBAC.a          # all three: run=1
$ L o4 -static start.o main.o libA2.a libB.a libC.a
bfd: link=0 run=2
lld: link=1
    ld.lld-22: error: duplicate symbol: dup
    >>> defined at main.c
    >>>            main.o:(dup)
    >>> defined at a2.c
    >>>            a2.o:(.data+0x0) in archive libA2.a
mold: link=1
    mold: error: duplicate symbol: libA2.a(a2.o): main.o: dup
$ ld.lld-22 --warn-backrefs -static start.o main.o libA.a libB.a libC.a -o w
(no output; status 0)
$ ld.lld-22 --warn-backrefs -static start.o main.o libA.a libB.a -o wb
ld.lld-22: warning: backward reference detected: pick in libB.a(b.o) refers to libA.a(a.o)
```

Reading: GNU ld takes a later definer when the lowest one was passed
before the reference, across archives, inside one archive (its index scan
continues after it adds a member) and inside `--start-group`. lld's
`--warn-backrefs` is silent when a later archive also defines the key.

## E2. Breadth first versus static owners

`m1.c: int K(void){return 1;}`, `m2.c: int K(void){return 2;} int
J(void){return 100;}`, `n.c: int K(void); int U(void){return K()*10;}`,
`main1.c: int U(void); int J(void); int main(void){return U()+J();}`.

```
$ L q -static start.o main1.o libX.a libY.a     # libX=[m1.o] libY=[n.o m2.o]
bfd: link=0 run=120
lld: link=1
    ld.lld-22: error: duplicate symbol: K
mold: link=1
    mold: error: duplicate symbol: libY.a(m2.o): libX.a(m1.o): K
$ L q -static start.o main1.o lib1.a lib2.a     # lib1=[m1.o] lib2=[m2.o n.o]
bfd: link=0 run=120
lld: link=0 run=120
mold: link=1
    mold: error: duplicate symbol: lib2.a(m2.o): lib1.a(m1.o): K
$ L q -static start.o main1.o lib1.a lib2r.a    # lib2r=[n.o m2.o]
bfd: link=0 run=120
lld: link=1
    ld.lld-22: error: duplicate symbol: K
mold: link=1
    mold: error: duplicate symbol: lib2r.a(m2.o): lib1.a(m1.o): K
```

Reading (revised after the review of 2026-11-14): lld's result depends on
member order; mold fixes each key's provider before extraction. Under
ruling A (`spec/resolve.md` §4.1) `first(K)` is `m1.o`, the lowest
definer, and `n.o`'s reference demands it: mortar reports the duplicate
`K` on all three lines, as mold does. For GNU ld, `n.o`'s reference to
`libX.a`/`lib1.a` is a back-reference it never acts on (§4.3 case 2). The
first draft's breadth-first rounds, which linked these lines as GNU ld,
are withdrawn: their "not yet defined" test made the result depend on the
order of extraction (findings 1, 4, 5).

## E3. Weak and common definitions against lazy ones

`mc.c: int v; int main(void){return v;}` (`-fcommon`), `mw.c:
__attribute__((weak)) int v = 1; int main(void){return v;}`, `lv.c: int
v = 9;`, `lvf.c: int v(void){return 9;}`, `lvc.c: int v; int
other(void){return 3;}` (`-fcommon`), and one-member archives
`lib_<shape>.a` whose member defines `v` as: `STT_OBJECT` in `.data`,
`.rodata`, `.bss`, `.text`; `STT_NOTYPE` in `.data` and `.text`; absolute;
`STT_FUNC`; `STT_TLS`; weak `STT_OBJECT`.

```
$ L a -static start.o mw.o liblv.a        # weak def vs lazy strong
all three: run=1 (no extraction)
$ L b -static start.o mc.o liblv.a        # common vs lazy data definition
bfd: run=9   lld: run=0   mold: run=9
$ ld.lld-22 --fortran-common -static start.o mc.o liblv.a -o b2; ./b2   # 9
$ ld.lld-22 --help | grep fortran-common
  --fortran-common        Search archive members for definitions to override COMMON symbols (default)
$ L c -static start.o mc.o liblvf.a       # common vs lazy function
bfd: run=0   lld: run=0   mold: run=32 (reads the code bytes)
$ L lc -static start.o mc.o liblvc.a      # common vs lazy common: no extraction (all, run=0)
$ L lw -static start.o lw.o lib_weakobj.a  # strong undefined ref vs lazy weak: extracted (all)
```

The `lib_<shape>` series under GNU ld: extraction for object (data,
rodata, bss, text), notype (data, text), absolute and TLS (then
``TLS definition in … mismatches non-TLS reference``); none for `STT_FUNC`
and weak. Lazy commons and lazy weak definitions are extracted for a
strong undefined reference by all three.

## E4. Shared objects against archives; `--as-needed`

`so.c: int f(void){return 1;}` → `libs.so`; `ar.c: …return 2;` →
`liba.a`; `ms.c: int f(void); int main(void){return f();}`; `mw.c:
__attribute__((weak)) int f(void); int main(void){return f ? f() : 3;}`.

| line (`start.o` first, dynamic) | bfd | lld | mold |
|---|---|---|---|
| `ms.o libs.so liba.a` | 1 | 1 | 1 |
| `ms.o liba.a libs.so` | 2 | 2 | 2 |
| `--as-needed ms.o liba.a libs.so` | 2, `libs.so` not in `DT_NEEDED` | same | same, output crashes at start (no `.dynamic`; not investigated) |
| `mw.o libs.so liba.a` | 1 | 1 | 1 |
| `mw.o liba.a libs.so` | 1 | 1 | 1 |
| `--as-needed mw.o libs.so liba.a` | 3, not needed | 3, not needed | 1, needed |
| `liba.a libs.so ms.o` | 1 | 1 | 2 |
| `liba.a ms.o libs.so` | 1 | 2 | 2 |
| `--as-needed libs.so ms.o` | undefined reference to `f` | 1 | 1 |

`so2.c: int g(void); int f(void){return g();}` → `libs2.so`, `g.c` →
`libg.a`: `ms.o libs2.so libg.a` extracts `g.o` and exports `g` (all three;
`readelf --dyn-syms`).

## E6. Commons, duplicates, TLS, unique, absolute, visibility

Sources (`.s`): `c4: .comm x,4,4`, `c64: .comm x,64,16`, `c64a: .comm
x,64,32`, `c8a: .comm x,8,64`, `d1`/`d2`: `.data; .globl x; x: .word
1`/`2`, `d8`/`d16`: sized `.data` objects aligned 8, `w`: weak `.data`
`x`, `dh`/`di`/`dp`: hidden/internal/protected `x`, `abs1`/`abs2`: `.set
x,0x400000`, `abs3`: `0x400004`, `t`: `x` in `.tdata`, `u1`/`u2`:
`%gnu_unique_object`, `m`: `main` loads `x` (`adrp`/`ldr`), `mh`: the same
with `.hidden x`, `mo`: a typed `STT_OBJECT` undefined `x`, `mt`: a TLS
`tprel` reference to `x`.

```
$ L o -static start.o m.o d1.o d2.o
bfd: … (.data+0x0): multiple definition of `x'; d1.o:(.data+0x0): first defined here
lld: error: duplicate symbol: x  >>> defined at d1.o:(.data+0x0) >>> defined at d2.o:(.data+0x0)
mold: error: duplicate symbol: d2.o: d1.o: x
$ L o -static start.o m.o d2.o d1.o --allow-multiple-definition   # all: run=2
$ L o -static start.o m.o d2.o d1.o -z muldefs                    # all: run=2
$ L o -static start.o m.o c4.o w.o      # bfd: 0, lld: 0 (common), mold: 3 (weak)
$ L o -static start.o m.o c4.o c64.o    # all: x size 64 (nm -S)
$ L o -static start.o m.o c64.o c8a.o   # bfd, lld: x 64-aligned; mold: 32 mod 64
$ L o -static start.o m.o c64.o d8.o --warn-common
bfd: warning: size of symbol `x' changed from 64 in c64.o to 8 in d8.o
     warning: alignment 8 of normal symbol `x' in d8.o is smaller than 16 used by the common definition in c64.o
     warning: definition of `x' overriding common
lld: warning: common x is overridden
mold: (none)
$ L o -static start.o m.o d8.o c64.o    # bfd: no size warning in this order
$ L o -static start.o m.o c4.o c64.o --warn-common
bfd: warning: common of `x' overriding smaller common
lld: warning: multiple common of x
mold: warning: multiple common symbols: x
$ L o -static start.o m.o abs1.o abs2.o # bfd, lld: link; mold: duplicate symbol
$ L o -static start.o m.o abs1.o abs3.o # all: duplicate
$ L o -static start.o m.o u1.o u2.o     # all: duplicate
$ L o -static start.o m.o t.o
bfd: x: TLS definition in t.o section .tdata mismatches non-TLS reference in m.o
lld: (links: untyped reference)        mold: warning: symbol type mismatch: x
$ L o -static start.o mo.o t.o          # lld: error: TLS attribute mismatch: x
$ L o -static start.o mt.o d1.o
bfd: x: TLS reference in mt.o mismatches non-TLS definition in d1.o
lld: links    mold: warning
$ L h -static start.o mh.o d1.o; readelf -s h.*  | grep ' x$'
bfd: GLOBAL HIDDEN    lld: LOCAL HIDDEN    mold: LOCAL HIDDEN
```

## E9. References into a dropped COMDAT copy

`a.s`: group `g` (`.text.g`, `g` returns 1), `main: b f2`. `b.s`: group
`g` with `g` returning 2 and a local `lg`, second member `.text.h` with
global `hg`; `f2: b lg`; `.data` holds `.quad hg`. `c.s`: group `g`;
`f2: b g`; `.quad .text.g` from `.debug_info`, `.eh_frame`,
`.gcc_except_table`, `.rodata.x`. `d.s`: as `c.s` with
`.gcc_except_table: .quad .text.g + 4` only. `a3.s`: `a.s` with a longer
`.text.g`.

```
$ L o2 -static start.o a.o b.o
bfd: `lg' referenced in section `.text' of b.o: defined in discarded section `.text.g[g]' of b.o
     b.o:(.data+0x0): undefined reference to `hg'
lld: error: relocation refers to a symbol in a discarded section: lg
     error: relocation refers to a symbol in a discarded section: hg
mold: error: … hg refers to a discarded COMDAT section probably due to an ODR violation
      then Segmentation fault
$ L o5 -static start.o a.o c.o   # bfd, lld: error for .rodata.x; mold: links
$ L o6 -static start.o a.o d.o
bfd: links; .gcc_except_table field = 0
lld: error: relocation refers to a discarded section … (.gcc_except_table)
mold: links; field = 4 (the addend)
```

`.debug_info` and `.eh_frame` references were accepted by all three
(`.eh_frame`'s FDE for the dropped copy is removed). With
`a3.o b.o` (different sizes) GNU ld still reports `lg`.

## E10. COMDAT keeper

`m.cc: inline int g(){return 1;} int B(){return g();}`, `l.cc: inline int
g(){return 2;} int B(); int A(){return g()*10+B();}`, `main.cc: int A();
extern "C" int main(){return A();}`. `libm=[m.o] liblate=[l.o]
libml=[m.o l.o]`. `l.o` is extracted first (for `A`), `m.o` then (for `B`),
but `m.o` has the lower `FileId`.

```
$ L q -static start.o main.o --start-group libm.a liblate.a --end-group
bfd: run=22   lld: run=22   mold: run=11
$ L q -static start.o main.o libml.a
bfd: run=22   lld: run=22   mold: run=11
$ L q -static start.o main.o m.o l.o           # all: run=11
$ L q -static start.o main.o l.o m.o           # all: run=22
```

mortar's rule (lowest live `FileId`) gives 11 in the first three lines,
22 in the last.

## E11. `.gnu.linkonce` and non-COMDAT groups

`a.s`/`b.s`: `foo` in `.gnu.linkonce.t.foo` in both, `main: b foo`.
`c.s`/`d.s`: a group `g` with flag word 0 (no `comdat`) in both, defining
`g` and `g2`.

```
$ L p -static start.o a.o b.o
bfd: run=1 (second section dropped by name)
lld: error: duplicate symbol: foo          mold: error: duplicate symbol: b.o: a.o: foo
$ L o -static start.o c.o d.o              # all: link, both sections kept
$ for f in /usr/lib/aarch64-linux-gnu/*.a /usr/x86_64-linux-gnu/lib/*.a \
     /usr/i686-linux-gnu/lib/*.a /usr/lib/gcc*/*/*/*.[ao]; do
     readelf -SW "$f" 2>/dev/null | grep -l linkonce; done      # nothing
```

The glob skipped the `crt*.o` files of the libc directories; the review
re-checked every `crt*.o` and `*.a` under `/usr/lib/aarch64-linux-gnu`,
`/usr/{x86_64,i686}-linux-gnu/lib`, `/usr/lib/gcc*` and
`/usr/lib/gcc-cross`: zero `.gnu.linkonce` sections (finding 19). The
claim holds on all three architectures.

## E12. Commons, weak definitions and shared objects

`so.c: int x = 50; int f(void){return 1;}` → `libso.so`. `c4`, `c64`, `w`,
`m`, `mh` as in E6; `cf.s`: a common named `f` and a reference to it.

```
$ L o start.o m.o w.o libso.so             # all: 3 (weak object def beats DSO)
$ L o start.o m.o c64.o libso.so
bfd: 50 (copy relocation)  warning: size of symbol `x' changed from 64 in c64.o to 4 in libso.so
lld: 0 (common kept, exported)      mold: 50
$ L o start.o m.o c4.o w.o libso.so        # bfd: line-order dependent; one order:
                                           #   unresolvable R_AARCH64_ADR_PREL_PG_HI21
                                           # lld: common (0); mold: weak (3)
$ L o start.o cf.o libso.so                # bfd, lld: the common f wins
$ L o start.o mh.o libso.so
bfd: hidden symbol `x' isn't defined     lld: error: undefined hidden symbol: x
```

## E13. `--wrap`, `--defsym`, roots

`fdef.c: int f(void){return 1;} int g(void){return f()*100;}`, `main.c:
int f(void); int g(void); int main(void){return f()+g();}`, `wrap.c: int
__real_f(void); int __wrap_f(void){return 10+__real_f();}`.

```
$ L o -static start.o main.o fdef.o wrap.o --wrap=f
bfd: run=111   lld: run=87   mold: run=111
```

(111 = `__wrap_f()` 11 + `g()` 100 with `g`'s call unwrapped; 87 is
1111 mod 256: `g`'s call wrapped too.) With `__wrap_f` and `f` in archives
(`libw.a`, `libf.a`): `mf.o libw.a libf.a --wrap=f` extracts both through
`__wrap_f` and `__real_f` (all three); with `w2.c` (a `__wrap_f` that does
not call `__real_f`), `libf.a` is not extracted (all three: `nm`).

```
$ L o -static start.o m2.o libh.a -u h     # h.o extracted (all, nm)
$ L o -static start.o m3.o libh.a -e h     # h.o extracted (all)
$ ld.lld-22 --require-defined=nosuch …     # error: unknown argument '--require-defined=nosuch'
$ ld.bfd --require-defined=nosuch …        # required symbol `nosuch' not defined
$ mold --require-defined=nosuch …          # --require-defined: undefined symbol: nosuch
$ L o -static start.o m3.o -e nosuch
bfd: warning: cannot find entry symbol nosuch; defaulting to 0000000000400078
lld: warning: cannot find entry symbol nosuch; not setting start address
mold: warning: entry symbol is not defined: nosuch
$ L o -static start.o m2.o libk.a --defsym f=k   # k.o extracted; f = k (all)
$ L o -static start.o m2.o libk.a --defsym f=k+4 # nm: bfd `A f`, lld `T f`;
                                                 # mold: --defsym: undefined symbol: k+4
$ L o -static start.o m2.o --defsym f=0x1234     # all: `A f`
$ L o -static start.o m3.o --defsym f=0x1234     # all: no duplicate, f = 0x1234
$ ld.bfd --start-lib                              # unrecognized option '--start-lib'
```

`--defsym f=k+4` was not tried with `-pie`: "not verified" in the spec.

## E14. Linker-defined symbols

`refs.s` (and `r_x86_64.s`, `r_i686.s` with the cross assemblers and
`ld.bfd -m elf_x86_64` / `elf_i386`, `ld.lld-22`, `mold -m …`): `main`
plus a `.data` table of `.quad` weak references to each name of
`spec/resolve.md` §8.1 and a section `foo`; then `readelf -s` per output
(defined / undefined, binding, visibility). The matrix is §8.1 there; the
`__rela_iplt_*` cell for mold is "undefined when there is no IRELATIVE".

`u.s`: defines one name strongly with a local alias `mine:` at the same
place and references it; a linker "wins" when the reference's value
differs from `mine`. Repeated for each name (`_end`, `_edata`,
`__bss_start`, `etext`, `__data_start`, `_GLOBAL_OFFSET_TABLE_`,
`__ehdr_start`, `__executable_start`, `__init_array_start`, …):

```
_end, _edata, __bss_start : bfd linker's, lld object's, mold linker's
_GLOBAL_OFFSET_TABLE_     : bfd linker's, lld: error: u.o cannot redefine linker defined symbol, mold linker's
etext, __data_start       : all object's
others                    : bfd object's, lld object's, mold linker's
$ ld.bfd --verbose | grep -E '_end|_edata|__bss_start'   # assigned without PROVIDE
```

## E15. Undefined symbols

`m.c`: six functions referencing `u` (and one `v`). `n.s`: `.globl zz`
never used. `g.s`: `dead` in its own section references `zz`.

```
$ L o -static start.o m.o
bfd: one line per reference: m.c:(.text+0x8): undefined reference to `u' …
lld: error: undefined symbol: u
     >>> referenced by m.c / m.o:(a1) … (3 places) >>> referenced 3 more times
mold: error: undefined symbol: u >>> referenced by m.o:(a1) … (3) >>> referenced 3 more times
$ L o -static start.o m.o --warn-unresolved-symbols         # warnings, links (all)
$ L o -static start.o m.o --unresolved-symbols=ignore-all   # links (all)
$ L o -static start.o n.o                                    # links; zz absent from .symtab (all)
$ L o -static start.o g.o --gc-sections                      # links (all)
$ L o.so -shared -z defs n.o                                 # links (all)
```

## E16. Versions

`lib.c` defines `foo@V1` (hidden) and `foo@@V2` with `lib.map: V1 {
global: *; }; V2 { global: foo; } V1;` → `libv.so` (also built by lld and
mold: `libv.lld.so`, `libv.mold.so`). `h.c` + `h.map` → `libh.so`, whose
only `foo` is `foo@V1` hidden. `m.c` references `foo`; `m1.c` references
`foo@V1` through `.symver`. Corrupted copies `libv_9.so`, `libv_0.so`,
`libv_32771.so` (versym of `foo` set to 9, 0, and `0x8003` = V2 hidden) by
patching `.gnu.version` with `dd`.

```
$ L o start.o m.o libv.so; readelf -V o.* | sed -n '/Version needs/,$p' | grep 'Name: V'
all: V2 needed, run=2
$ L o start.o m1.o libv.so      # all: V1 needed, run=1
$ L o start.o m.o libh.so       # all: undefined symbol foo
$ L o start.o m.o libv_9.so
bfd: libv_9.so: foo: invalid version 9 (max 3)
lld: error: corrupt input file: version definition index 9 for symbol foo is out of bounds
mold: Segmentation fault
$ L o start.o m.o libv_0.so     # bfd: links; lld: corrupt input file; mold: undefined foo
$ L o start.o m.o libv_32771.so # all: undefined foo
$ readelf -V libv.so | grep Parent        # bfd-built: "Parent 1: V1"; lld-, mold-built: none
```

Version-script precedence: `s.c` (`foo`, `foobar`, `fob`, `_Z3bazv`)
linked `-shared` with each script below; `readelf --dyn-syms` shows the
assigned version.

```
V1 { global: f*; }; V2 { global: foo; };               foo@V2 (all)
V1 { global: foo; }; V2 { global: f*; };               foo@V1 (all)
V1 { global: foo*; }; V2 { global: fo*; };             foo@V2 (all)
V1 { global: fo*; }; V2 { global: foo*; };             foo@V2 (all)
V1 { global: fo*; local: foo*; };                      foo global (all)
V1 { global: *; }; V2 { local: foo; };                 foo local (all)
V1 { global: foo; }; V2 { global: foo; };              bfd V1; lld V1 + warning: attempt to reassign
                                                       symbol 'foo' of version 'V1' to version 'V2'; mold V2
V1 { global: extern "C++" { "baz()"; }; local: *; };   baz() exported (all); also with baz*
V1 { local: foo; global: *; };                         bfd: syntax error in VERSION script; lld, mold accept
```

## E17. Copy relocations

`so.s`: `x` (8 bytes) in `.data` after a 64-aligned pad, so `st_value` is
8 mod 64; `y` in `.rodata`, `yalias` set equal to `y`. `m.s` loads `x`,
`y`, and takes `yalias`'s address (non-PIC: copy relocations).

```
$ L o start.o m.o libc1.so -no-pie; readelf -SW -r o.*
x copy:  bfd at 0 mod 64 (section alignment); lld, mold at 8 mod 64 (value-capped)
y copy:  bfd .data.rel.ro, lld .bss.rel.ro, mold .copyrel.rel.ro (all in PT_GNU_RELRO)
yalias:  bfd: a second R_AARCH64_COPY (two copies); lld, mold: one copy, both symbols on it
```

## E18. Section kinds

`x.s`: `main`, an allocated `SHF_EXCLUDE` section `.ex`, `.note.GNU-stack`,
`.llvm_addrsig`, `.comment`, a `SHF_EXCLUDE` `.gnu.lto_.foo`.
`c.s`/`d.s`/`e.s`: COMDAT groups whose signature is the section symbol
(`.section .foo,"aG",@progbits,.foo,comdat` assembles the signature as
`STT_SECTION`): `.foo` in `c` and `d`, `.bar` in `e`. `a.s`/`b.s`: the same
for `.stapsdt.base`. `lto.c` compiled `gcc -flto -c` (slim).

```
$ L o -static start.o x.o; readelf -SW o.*
.ex:   bfd dropped, lld dropped, mold kept.  .llvm_addrsig, .note.GNU-stack, .gnu.lto_.foo: dropped (all)
$ L q -static start.o c.o d.o e.o; readelf -x .foo -x .bar q.*
bfd: .foo = 01, .bar = 02    mold: same    lld: .foo = 01, no .bar
$ L r -static start.o a.o b.o    # all: one .stapsdt.base byte (01)
$ L p -static start.o lto.o
bfd: lto.o: plugin needed to handle lto object
lld: error: undefined symbol: main
mold: fatal error (GCC LTO object without plugin)
```

## E19. gfortran `BLOCK DATA` in an archive

`bd.f`: `block data init; common /blk/ a, b; data a, b /7, 9/`, archived
as `libbd.a`; `p.f` calls `sub` and stops 1 or 2 if `a`, `b` are not 7, 9;
`s.f` declares `/blk/ a, b, c` (12 bytes) and sets `c`. Linked
`p.o s.o libbd.a` with each linker against the system's gfortran runtime
(static, the C runtime start files and `-lgfortran -lm -lc` as the
`gfortran -v` link line names them).

```
bfd: runs, exit 0; warning: size of symbol `blk_' changed from 12 in s.o to 8 in libbd.a(bd.o)
lld: STOP 1
mold: runs, exit 0
$ nm s.o | grep blk_      # C blk_ (gfortran emits COMMON blocks as commons)
```

## E20. Shared objects' references; linker symbols in shared objects

`l.c` → `libl.so` references `cb` and `missing`; `cbso.c` → `libcb.so`
defines `cb`; `m.c` defines `cb`; `mh.c` defines `cb` hidden and
`missing`; `ds.c` → `libds.so` defines `end`, `etext`, `_end`,
`__start_foo`, `__ehdr_start`; `r.s` references those five.

```
$ L o start.o m.o libl.so
bfd: libl.so: undefined reference to `missing'
lld: error: undefined reference: missing >>> referenced by libl.so (disallowed by --no-allow-shlib-undefined)
mold: links
$ L o start.o m.o libl.so; readelf --dyn-syms o.* | grep cb    # cb exported (all)
$ L o start.o mh.o libl.so
bfd: hidden symbol `cb' in mh.o is referenced by DSO
lld: error: non-exported symbol 'cb' in 'mh.o' is referenced by DSO 'libl.so'
mold: links
$ L o start.o mh.o libl.so libcb.so            # all: link
$ L o start.o r.o libds.so; readelf -r o.*     # which names get dynamic relocations against libds.so
bfd: __ehdr_start   lld: none   mold: end, etext
```

## E21. `.gnu.warning`

`w.s`: `foo` with `.section .gnu.warning.foo; .string "foo is dangerous"`,
archived as `libw.a`; `m.s`: `main: b foo`.

```
$ L o -static start.o m.o libw.a
bfd: m.o: in function `main': (.text+0x0): warning: foo is dangerous   (section not in output)
lld: links, no warning, .gnu.warning.foo copied
mold: links, no warning, .gnu.warning.foo copied
$ readelf -SW /usr/lib/aarch64-linux-gnu/libc.a | grep -c '\.gnu\.warning'   # 112
```

## Library survey for `archive-choice` (spec §4.6)

For each archive, the keys defined (global, non-common) by more than one
member, split by binding, with `nm -A --defined-only`:

| archive | strong keys in ≥2 members | weak |
|---|---|---|
| `libc.a` | 0 (AArch64, x86-64); i386: 7 | (not counted) |
| `libstdc++.a` (GCC 14) | 1 (AArch64); i386: 8 | 130 |
| `libgcc.a`, `libgcc_eh.a`, `libm.a`, `libgfortran.a` | 0; i386 `libgcc.a`: 7 | — |

Corrected by the review (finding 18): the one AArch64 `libstdc++.a` key is
`_ZZNSt19_Sp_make_shared_tag5_S_tiEvE5__tag`, `STB_GNU_UNIQUE`, in COMDAT
groups of 5 members; the i386 keys are the `__x86.get_pc_thunk.*`
helpers, all in COMDAT groups. Group keys are §6's, so none of them can
make `archive-choice` fire. Across archives, `__clz_tab` is defined
strongly outside any group in both `libgcc.a(_clz.o)` and
`libc.a(mp_clz_tab.o)`; no link tried references it. The survey matters
less now: `archive-choice` is opt-in (ruling B) and comes from a
simulation, not from a predicate over these keys.

## Re-run under ruling A (review of 2026-11-14)

The reviewer's executable model (a Python reading of the first draft) was
changed to the rule of `spec/resolve.md` §4.1: `first(K)` and `cfirst(K)`
over all files, edges from non-weak undefined mentions to `first`, from
commons to `cfirst`, from root options to `first`; the live set is
reachability; `def` is computed over live files only. A second function
simulates GNU's walk (§4.6). Each line's real `ld` argument vector came
from `gcc -###` (the reviewer's `compare.py`), was linked by `ld.bfd -t
-t`, `ld.lld-22 --trace` and `mold -M`, and the members were compared as
sets. Columns: members each linker loads; the model's; whether it equals
`ld.bfd`'s list; the simulation's; whether that equals it.

| line | bfd | lld | mold | model | model vs bfd | simulation | vs bfd |
|---|---|---|---|---|---|---|---|
| chello | 381 | 381 | 381 | 381 | equal | 381 | equal |
| clang_c | 381 | 381 | 381 | 381 | equal | 381 | equal |
| spie | 382 | 382 | 382 | 382 | equal | 382 | equal |
| mth | 466 | 466 | 466 | 466 | equal | 466 | equal |
| cxx | 578 | 578 | 578 | 578 | equal | 578 | equal |
| clang_cxx | 578 | 578 | 578 | 578 | equal | 578 | equal |
| fort | 558 | 557 | 558 | 558 | equal | 558 | equal |
| tpl | 431 | 431 | 431 | 431 | equal | 431 | equal |
| c64 | 430 | 430 | 430 | 430 | equal | 430 | equal |
| spie64 | 431 | 431 | 431 | 431 | equal | 431 | equal |
| mth64 | 533 | 533 | 533 | 533 | equal | 533 | equal |
| cxx64 | 653 | 653 | 653 | 653 | equal | 653 | equal |
| fort64 | 617 | 616 | 617 | 617 | equal | 617 | equal |
| c32 | 379 | 379 | 379 | 379 | equal | 379 | equal |
| spie32 | 380 | 380 | 380 | 380 | equal | 380 | equal |
| cxx32 | 599 | 599 | 599 | 599 | equal | 599 | equal |

(No suffix: AArch64; `64`: x86-64; `32`: i386. `chello`, `clang_c`: `gcc`
and `clang -static`; `spie`: `-static-pie`; `mth`: `-lm -lpthread -ldl`;
`cxx`, `clang_cxx`: `g++`, `clang++ -static`; `fort`: `gfortran
-static`; `tpl`: user templates.) No `archive-*`, `duplicate-symbol` or
`comdat-mismatch` diagnostic fires on any of them. With commons left out
of `first` (the ruling's literal text), the sixteen lines are also equal;
R5 below is where that text fails.

The reviewer's cases, `L n -static start.o …` (spec §4.3):

```
t1   t1m.o libt1k.a t1w.o   bfd run=2  lld run=2  mold run=1   model: libt1k.a(t1k.o) extracted (2)
t1s  t1m.o libt1k.a t1s.o   bfd multiple definition of `K'  lld duplicate symbol: K  mold run=3
                            model: duplicate K
t2   m2.o libA.a libCB.a    bfd run=3  lld run=1  mold run=1   model: a.o (1); simulation: c.o, archive-choice pick
t2b  m2.o libAB.a libC.a    all run=1                          model: a.o; simulation: a.o; no diagnostic
t10  m10.o libG.a           bfd run=140  lld run=140  mold undefined symbol: hx
                            model: keeper gl.o, comdat-mismatch hx
t12  m0.o libst.a (no -e)   all three extract the _start member (mold: local `t _start`)
```

## Ruling B: the simulation against `ld.bfd -t`

The simulated member sets equal `ld.bfd -t`'s on: `main1 libX libY`,
`main1 lib1 lib2` (E2); `m2 libA libCB`, `m2 libAB libC`, `m2 libA2 libB1
libC1`, and its `--start-group` variant (E1, finding 3); `m10 libG` (t10);
t1; `emain --start-group libml liblate --end-group` (E10). One fix was
needed on the way: a common after a weak definition makes the key
`common` (GNU ld: a common beats a weak definition, so the next archive
pass extracts a data definition for it). On E2's `main1 libX libY` GNU
ld links and mortar reports the duplicate `K`; under `--warn-backrefs`
the simulation adds that GNU ld would not load `libX.a(m1.o)`.

## R1–R15: experiments for the review

`L` and `start.o` as in Method; AArch64. Sources: `mk.c: int K(void); int
main(void){return K();}`, `mkw.c: __attribute__((weak)) int K(void); int
main(void){return K ? K() : 7;}`, `ks.c: int K(void){return 2;}`, `kw.c:
weak … return 1`, `uk.c: int K(void); int user2(void){return K();}`,
`mu.c: … main(void){return user2();}`, `m0.c: int main(void){return 0;}`;
`mc.c: int v; int main(void){return v;}` (`-fcommon`), `mv.c: extern int
v; int main(void){return v;}`, `lv.c: int v = 9;`, `lvc.c: int v; int
other(void){return 3;}` (`-fcommon`), `lvf.c: int v(void){return 9;}`,
`wv.c: __attribute__((weak)) int v = 4;`, `fv.c: int v(void){return 4;}`.
Archives: `libKS=[ks.o]`, `libKW=[kw.o]`, `libUK=[uk.o]`, `libLV=[lv.o]`,
`libLC=[lvc.o]`, `libLF=[lvf.o]`.

```
R1  = t1, t1s above (a definition after the archive; finding 1)
R2  mkw.o libKS.a uk.o                   bfd undefined reference to `K'   lld run=2  mold run=2
R3  mk.o libKW.a libKS.a                 bfd run=1  lld run=1  mold run=2
    mk.o libKW.a ks.o                    all run=2
R4  mk.o libKS.a --whole-archive libKW.a bfd run=2  lld run=2  mold run=1
    mk.o --whole-archive libKW.a --no-whole-archive libKS.a   all run=1
R4b -u K m0.o libKS.a                    all three extract ks.o
R5  mv.o libLC.a                         all three extract lvc.o (`other` in the output)
    mv.o libLC.a libLV.a                 bfd both, run=9  lld lvc.o only, run=0  mold lv.o only, run=9
R6  mc.o libLF.a                         no extraction: bfd run=0 lld run=0; mold extracts, run=32,
                                         warning about the symbol type
    mc.o libLV.a                         bfd run=9  lld run=0  mold run=9
    wv.o mc.o libLV.a / mc.o libLV.a wv.o   bfd extracts, run=9  lld run=0  mold extracts, run=4
    fv.o mc.o libLV.a                    no extraction (all, run=128)
```

R5 refutes ruling A's "non-common definition" in `first(K)`: without the
common, `mv.o`'s strong reference demands nothing and is left undefined,
where all three extract. The spec counts commons in `first` and states
the common edge separately (`cfirst`, §4.5). R6 is why weak definitions
do not block `cfirst`.

```
R7  (adv) mt.s: TLS reference to x (tprel); mn.s: .quad x (NOTYPE); t.s: x in .tdata
    mt.o mn.o     bfd: x: TLS reference in mt.o mismatches non-TLS reference in mn.o
                  lld links  mold links
    m.o t.o       (m.s: NOTYPE reference)  bfd error  lld links  mold links, silent
    mo.o t.o      (mo.s: .type x,%object)  bfd error  lld TLS attribute mismatch  mold warning
R8  mh2.o libnoidx.a (ar rcS)   bfd: archive has no index; run ranlib to add one
                                lld run=7  mold run=7
R9  = t12 (m0.o libst.a, no -e: _start extracted by all three)
R10 libk.a --defsym K=k         bfd: undefined symbol `k' referenced in expression
    --defsym K=k libk.a         bfd links;  lld and mold link in both positions
R11 libs.so: K returns 5 (dynamic link, -dynamic-linker as Method)
    mk.o libs.so libKS.a        all 5
    mk.o libKS.a libs.so        all 2
    libKS.a libs.so mk.o        bfd 5  lld 5  mold 2
    libKS.a mk.o libs.so        bfd 5  lld 2  mold 2
    mkw.o libs.so libKS.a uk.o  all 5
R12 mv2.o: reference foo@V2 (.symver); dloc.o: foo@@V2 = 9; dv.o: foo@V2 = 4; mf.o: plain foo
    mv2.o dloc.o   bfd run=9  lld undefined symbol: foo@V2 (did you mean foo@@V2)  mold undefined symbol: foo
    mv2.o dv.o     all run=4
    mf.o dloc.o    all run=9
    mf.o dv.o      all undefined
    survey: for f in <every *.a of the three targets' libc and GCC directories>; do nm -A "$f" | grep @; done
            -> only libmcheck.a: __malloc_initialize_hook@GLIBC_2.17 / @GLIBC_2.0 / @GLIBC_2.2.5
R13 a.s: COMDAT g, main: b g; bN.s: COMDAT g with a local lg, and `.section <name>,"",@progbits; .quad lg`
    L o -static start.o a.o bN.o, one name per bN (non-alloc unless noted):
    accepted by all three: .debug .debug_info .debug_foo .debugx .zdebug_info .zdebugx .stab .stabstr
      .stab.foo .stabx (objcopy --rename-section) .line .line.x .linex .gnu.debuglto_.debug_info
      .gnu.debuglto_.debug_x .gnu_debuglink .gnu.linkonce.wi.foo .gcc_except_table .debug_info.dwo
    bfd error, lld and mold accept: .comment .comment.x .note.x .gcc_except_table.foo .lin
      .gnu_debugaltlink .gnu.debuglto_x .gnu.debuglto_.debug .gnu.debuglto_.debugx .gnu.debuglto_.line
      .debu .gnu.linkonce.wix .gnu_debuglinkx
    alloc .debug_x ("a"): bfd error  lld error  mold accepts
R14 hz.s: main, `.hidden zz; .globl zz`, no use
    start.o hz.o   bfd: hidden symbol `zz' isn't defined (also with --unresolved-symbols=ignore-all)
                   lld links  mold links
R15 x86_64-linux-gnu-objdump -dr libstdc++.a (eh_globals.o): R_X86_64_PLT32 __tls_get_addr-0x4
    x86_64-linux-gnu-nm libc.a | grep -c ' T __tls_get_addr'   -> 0
    the cxx64 and cxx32 lines above link with ld.bfd (the call is relaxed away, finding 6)
```

The three link tests added (`tests/link/resolve/weak-after-archive.test`,
`comdat-mismatch.test`, `entry-default-root.test`) were run by hand with
the GCC driver and `-B` wrappers for each linker, on AArch64: t1 and t1s,
t10 and t12 give the results above (`entry-default-root`: `ld -static
m0.o -L. -lst`, `_start` and `marker` present for all three, entry not 0).

## The COMDAT keeper against GNU ld (the reviewer's table, ruling C)

Signatures present in two or more live files of a link, and for how many
of them each linker keeps another file's copy than the lowest live
`FileId` (mortar's rule):

| link | signatures | GNU ld differs | lld differs | mold differs |
|---|---|---|---|---|
| `g++ -static`, AArch64 | 61 | 40 | 6 | 0 |
| `g++ -static`, x86-64 | 60 | 40 | 6 | 0 |
| `g++ -static`, i386 | 67 | 45 | 10 | 0 |
| `clang++ -static`, AArch64 | 60 | 35 | 3 | 0 |
| user templates in two archives | 17 | 16 | 14 | 0 |

Sampled differing copies were byte-identical and the programs' outputs
equal. The keeper is kept as the lowest live `FileId`: a minimum over a
set, needed by §12 and §13; GNU's "first loaded" depends on the walk.

## Decisions and reasons

1. **Position-independent extraction (supervisor's decision): confirmed;
   the rule replaced by ruling A.** Reason: reachability over a graph that
   is a function of the inputs is parallel, order-independent and
   recomputable from per-file summaries; GNU's walk is a sequential
   simulation with state. The first draft's breadth-first rounds are
   withdrawn (findings 1, 4, 5). The observable differences from GNU ld
   (another member; duplicates; new undefined references) need a
   back-reference (spec §4.3); the COMDAT difference is case 4.
   Diagnostics for them are opt-in under `--warn-backrefs`, from a
   simulation of GNU's walk (ruling B). `--start-group` changes nothing
   in the closure.
2. **COMDAT keeper = lowest live `FileId`: kept (ruling C)** (E10). It
   equals mold; it differs from GNU ld and lld when a lower-`FileId` copy
   became live later, which is frequent on system lines (the keeper table
   below) but harmless when the copies agree; differing contents are an
   ODR violation (`mortar_odr`'s job); differing symbol sets are the error
   `comdat-mismatch` (t10).
3. **Definition order** Assign < Strong < Common < Weak < Deferred (lazy
   and shared together, by `FileId`) < Provide, with a shared data
   definition beating commons alone and Provide beating shared
   definitions: chosen per E3, E4, E6, E12, E14, E20 to agree with the
   majority and with Fortran's needs (E19).
4. **Commons extract lazy data definitions** (GNU ld's table, E3), not
   functions (mold's result is a wrong program).
5. **TLS mismatch is an error in both directions** (GNU ld, E6).
6. **Hidden keys are `STB_LOCAL` in `.symtab`** (gABI; lld, mold).
7. **`--wrap` applies to undefined mentions only** (GNU ld's manual text,
   GNU ld and mold, E13).
8. **Linker symbols**: the object's definition wins where GNU ld and lld
   agree; the names GNU ld forces are refused when an object defines them
   (E14); a shared object never defines them (E20, lld).
9. **Versions** (supervisor's three questions): hidden-only versions bind
   only versioned references (E16, all three); unknown and out-of-range
   indices and `VER_NDX_LOCAL` on a definition are `malformed-input`
   (lld's strictness; GNU ld errors on the first, mold crashes); verdef
   parents are written (GNU ld's output, E16). Copy relocations: alignment
   `min(sh_addralign, 2^ctz(st_value))`, read-only when the address is in
   a non-writable `PT_LOAD` (E17); aliases share a copy (lld, mold).
10. **Sections**: section-symbol group signatures use the section's name
    (E18, settles WP-004's question); `.gnu.linkonce` refused (E11, no
    system library has one); `.gnu.warning` warned and dropped (E21);
    slim GCC LTO refused (E18); `SHF_EXCLUDE` dropped (E18).

## Review of 2026-11-14 and dispositions

An independent adversarial review found 22 problems in the first draft;
the supervisor's rulings A–F decided them. One line per finding, with
where the spec now answers it:

1. Definition after the archive missed by §4.3: accepted (ruling A); new
   rule §4.1, consequences §4.1, case R1, §4.3 claim and proof, §16, test
   `weak-after-archive.test`.
2. COMDAT keeper rejects links GNU ld accepts: accepted in part (ruling
   C): the keeper stays the lowest live `FileId`; the case is §4.3 case 4,
   §6.1, §16; new error `comdat-mismatch` §6.3, §14; keeper table above;
   test `comdat-mismatch.test`.
3. `archive-choice` predicate is not GNU's choice: accepted (ruling B):
   opt-in under `--warn-backrefs`, a simulation of GNU's walk, §4.6
   (specified, cost stated, checked above); §14, §17 Q1.
4. Commons never extract under §4.1 as written: accepted (ruling A):
   `cfirst` and the common edge, §4.1, §4.5. Ruling A's own text (commons
   left out of `first`) is **refuted** by R5 and amended: commons count in
   `first`.
5. "Deferred is never final" false: accepted (ruling A): `def` over live
   files only, Undefined when none, §3.1, §3.2, §4.1.
6. `g++ -static` on x86-64/i386 would fail on `__tls_get_addr`: accepted
   (ruling D): the scan decides per surviving reference after sequence
   recognition, §5.1 (R15); §18 `spec/arch.md`.
7. TLS reference/reference mismatch undefined; mold cell wrong: accepted
   (ruling E): §3.4 (all mentions agree), §16 (mold NOTYPE silent, OBJECT
   warning; the new reference/reference row), R7.
8. §5.1 against §5.2: accepted (ruling D): hidden unused is not an error,
   hidden used is one whatever the options, §5.1, §5.2 (R14), §16.
9. Versioned references in static links: accepted (ruling E): the second
   lookup applies to objects, §9.4 (R12 and the survey), §16.
10. Slim LTO slips past §10: accepted (ruling F): the file row is first in
    §10.
11. Referring-section rules under-specified: accepted (ruling E): exact
    `.gcc_except_table`; GNU ld's debug names measured (R13), §6.3, §16.
12. `__start_<sec>` not decidable in resolution: accepted (ruling D):
    conditional `Provide` candidates confirmed by Assignment, §5.1, §8.1,
    §18 `spec/model.md` §3.4.
13. Index or member tables unstated: accepted (ruling F): member tables,
    §4.1 (R8), §4.6, §16.
14. Default `_start` a root?: accepted (ruling E): yes, §4.2 (R9), §5.5;
    test `entry-default-root.test`.
15. `Inputs` lacks the group region: accepted (ruling F): §4.6 names the
    column; §18 `spec/model.md` §3.2.
16. Incremental claims unsound: accepted (ruling F): §13 rewritten as
    dependencies (type and binding changes move `cfirst`; keeper moves
    demote other files' mentions).
17. `--defsym` position: accepted (ruling F): right-hand sides are roots
    in any position, §4.2 (R10), §16.
18. Library survey wrong on i386 and the libstdc++ key: accepted as
    written: survey corrected above.
19. E11 glob skipped `crt*.o`: accepted as written: note in E11.
20. `model.md` still says "a total order": accepted as written: §18 asks
    for the wording (graph, reachability, `def` over live files, not one
    total order). The rules delegated to specs that do not exist yet stay
    delegated; §18 lists the requests.
21. "Strong-shaped" undefined; "two" absolutes: accepted as written: the
    term is gone (§4.6 names the states); §3.8 states the set rule (any
    number of equal absolutes; absolute and section definitions clash).
22. Undefined wording not flagged: accepted (ruling F, policy): messages
    contain GNU's phrases verbatim, §3.8, §5.3, §14, §17 Q4, §18
    `spec/diag.md`.

## Not verified

Extraction through the second version lookup for a lazy `foo@@V2` member
(§9.4); runs of the x86-64 and i386 cases (only their link lines and
member sets were compared: the cross qemu was not used for the R series);
the three new link tests on x86-64 and i386 (run on AArch64 only);
`--defsym a=b+N` in a PIE; `--export-dynamic-symbol` extraction under GNU
ld; `SHT_LLVM_CALL_GRAPH_PROFILE`, `.deplibs`, `SHT_AARCH64_ATTRIBUTES`,
`.note.gnu.property` merging, fat GCC LTO objects; two groups of one
signature in one file; `SHF_LINK_ORDER` with dropped groups;
`--copy-dt-needed-entries` defaults; `--no-undefined-version` defaults;
hidden entries in a shared object's `.dynsym`; version names in a static
executable's `.symtab`; weak common symbols (no tool tested emits one);
a copy relocation against a protected definition; `DT_NEEDED`-unknown
shared objects under `--no-allow-shlib-undefined`; extraction for a
`--gc-sections`-collected member.
