# WP-021 stage 1: the design of `spec/input.md`

Design only: no Rust in the workspace, no tests. This file records the
method, the experiments `spec/input.md` cites as `[En]`, and the decisions
with their reasons.

## Method

Machine: AArch64 host, 24 cores, Debian. References: GNU ld 2.44
(`ld.bfd`; cross `x86_64-linux-gnu-ld.bfd`, `i686-linux-gnu-ld.bfd`), lld
22.1.8 (`ld.lld-22`), mold 2.37.1. Objects from GCC 14 (`gcc`, the two
cross `gcc`s) and Clang 22; `llvm-objcopy-22`, GNU `ar`. The machine was
shared and loaded (load average 23-28) throughout.

Every experiment ran in a scratch directory, with small C files written by
`printf` (`f.c`: `int f(void){return 1;}`; `usef.c`: `int f(void); void
_start(void){ f(); for(;;); }`; `start.c`: `void _start(void){ for(;;); }`),
compiled with `gcc -c`. The helper `t3` ran the three linkers on one line:

```sh
#!/bin/sh
# t3 ARGS... : run ld.bfd, ld.lld-22, mold with ARGS -o out.<linker>; print status and stderr
for L in bfd lld mold; do
  case $L in bfd) P=ld.bfd;; lld) P=ld.lld-22;; mold) P=mold;; esac
  rm -f out.$L
  ERR=$($P "$@" -o out.$L 2>&1); S=$?
  printf '== %s (%d)\n' $L $S
  [ -n "$ERR" ] && printf '%s\n' "$ERR" | sed 's/^/   /' | head -${T3LINES:-6}
done
```

`need` printed the `DT_NEEDED` entries of each output:
`for L in bfd lld mold; do readelf -d out.$L | grep NEEDED; done`.

Below, `bfd (1)` is GNU ld's exit status; messages are quoted as printed,
shortened with `...`. No reference source was read.

## Experiments

### E1 File kinds

```sh
gcc -c start.c f.c && gcc -o exe start.c -nostdlib -static
clang-22 -flto -c f.c -o bc.o; clang-22 -flto=thin -c f.c -o thinbc.o
gcc -flto -c f.c -o glto.o; gcc -flto -ffat-lto-objects -c f.c -o gfat.o
clang-22 --target=x86_64-apple-darwin -c f.c -o macho.o
clang-22 --target=x86_64-pc-windows-msvc -c f.c -o coff.o
clang-22 --target=wasm32 -c f.c -o wasm.o
: > empty; mkdir dir; printf 'hello\0world' > bin.dat
cp f.o core.o; printf '\004\000' | dd of=core.o bs=1 seek=16 conv=notrunc
for f in exe core.o bc.o thinbc.o glto.o gfat.o macho.o coff.o wasm.o empty dir bin.dat; do t3 start.o $f; done
t3 empty
```

| input | bfd | lld | mold |
|---|---|---|---|
| `exe` (ET_EXEC) | 1: `multiple definition of '_start'` ... `cannot use executable file 'exe' as input to a link` | 1: `exe: unknown file type` | 1: `exe: unknown file type` |
| `core.o` | 1: `file format not recognized` | 1: `unknown file type` | 1: `unknown file type` |
| `bc.o`, `thinbc.o` | 1: `file format not recognized` | 0 | 1: `don't know how to handle this LTO object file because no -plugin option was given` |
| `glto.o` | 0: `glto.o: plugin needed to handle lto object` (twice) | 0 | 1: as `bc.o` |
| `gfat.o` | 0 | 0 | 0 |
| `macho.o`, `coff.o`, `wasm.o` | 1: `file format not recognized` | 1: `unknown file type` | 1: `unknown file type` |
| `empty` | 0 | 0 | 1: `empty: unknown file type` |
| `dir` | 1: `cannot find dir: file format not recognized` | 1: `cannot open dir: Is a directory` | 1: `dir: mmap failed: No such device` |
| `bin.dat` | 1: `file format not recognized` | 1: `bin.dat:1: unknown directive: hello` | 1: `unknown linker script token` |
| `empty` alone (no object) | 0: `cannot find entry symbol _start` | 1: `target emulation unknown: -m or at least one .o file required` | 1: `-m option is missing` |

Magics (`od -An -tx1 -N8`): `bc.o` `42 43 c0 de`; `macho.o` `cf fa ed fe`;
`coff.o` `64 86`; `wasm.o` `00 61 73 6d`. `readelf -sW glto.o`: `OBJECT
GLOBAL DEFAULT COM __gnu_lto_slim`, and its `.gnu.lto_*` sections have flag
`E` (`SHF_EXCLUDE`); `gfat.o` has the `.gnu.lto_*` sections and no
`__gnu_lto_slim`.

### E2 The target when there is no `-m`

```sh
x86_64-linux-gnu-gcc -c start.c -o xstart.o; x86_64-linux-gnu-gcc -c f.c -o xf.o
i686-linux-gnu-gcc -c f.c -o if.o; i686-linux-gnu-gcc -c start.c -o istart.o
x86_64-linux-gnu-gcc -shared -fPIC f.c -o libxf.so; gcc -shared -fPIC f.c -o libaf.so
ar rcs libxf.a xf.o; ar rcs libaf.a f.o
mkdir xd ad; cp libxf.a xd/libf.a; cp libaf.a ad/libf.a; cp libxf.so xd/libf.so; cp libaf.so ad/libf.so
x86_64-linux-gnu-ar rcs mixed2.a f.o xstart.o; printf 'GROUP ( libaf.so )\n' > grp.ld
```

| line | bfd | lld | mold |
|---|---|---|---|
| `xstart.o xf.o` | 1: `xstart.o: Relocations in generic ELF (EM: 62)` ... `file in wrong format` | 0 | 0 |
| `istart.o if.o` | 1: `relocations in generic ELF (EM: 3)` | 0 | 0 |
| `start.o xf.o` | 1: `xf.o: Relocations in generic ELF (EM: 62)` | 1: `xf.o is incompatible with start.o` | 1: `xf.o: incompatible file type: arm64 is expected but got x86_64` |
| `libxf.so start.o` | 1: `libxf.so: ... file in wrong format` | 1: `start.o is incompatible with libxf.so` | 1: `start.o: incompatible file type: x86_64 is expected but got arm64` |
| `libxf.a start.o` (lazy member first) | 0 | 1: `start.o is incompatible` | 1: as above |
| `start.o libxf.a` (member unused) | 0 | 1: `libxf.a(xf.o) is incompatible with start.o` | 1: `xf.o: incompatible file type ...` |
| `usef.o mixed2.a` (other member x86, unused) | 0 | 1: `mixed2.a(xstart.o) is incompatible with usef.o` | 1: `xstart.o: incompatible file type ...` |
| `usef.o -Bstatic -Lxd -Lad -lf` | 0: `skipping incompatible xd/libf.a when searching for -lf` | 1: `xd/libf.a(xf.o) is incompatible with usef.o` | 0: `warning: xd/libf.a: skipping incompatible file: x86_64 (e_machine 183)` |
| `usef.o -Lxd -Lad -lf` | 0: skips `xd/libf.so`, `xd/libf.a` | 1: `xd/libf.so is incompatible with usef.o` | 0: warns for both |
| `-Lxd -Lad -lf usef.o` | 0: skips both | 1: `usef.o is incompatible with xd/libf.so` | 0: warns for both |
| `grp.ld xusef.o` | 1: `xusef.o: Relocations in generic ELF` | 1: `xusef.o is incompatible with libaf.so` | 1: skips `libaf.so`, then `library not found: libaf.so` |

mold only (`mold ... -o o; echo $?`): `-Lad -Lxd -lf xusef.o`: 0, skipping
`ad/libf.so` and `ad/libf.a` (arm64); `libaf.so xusef.o` and `libaf.a
xusef.o`: 1, `xusef.o: incompatible file type: arm64 is expected but got
x86_64`; `-Lad -lf`: 1, `-m option is missing`. lld `-Lad -lf`: 0 (warning:
no `_start`).

`--fatal-warnings` with `usef.o -Lxd -Lad -lf`: GNU ld 1 (the two skip
messages); mold 1 (`mold: error: xd/libf.so: skipping incompatible file`).

### E3 Search order

```sh
mkdir da dso both; gcc -c usef.c f.c; ar rcs da/libf.a f.o
gcc -shared -fPIC f.c -o dso/libf.so -Wl,-soname,libf.so; cp da/libf.a dso/libf.so both/
```

| line | all three | `DT_NEEDED` count |
|---|---|---|
| `usef.o -Lda -Ldso -lf` | 0 | 0 (the `.a`) |
| `usef.o -Ldso -Lda -lf` | 0 | 1 |
| `usef.o -Lboth -lf` | 0 | 1 |
| `usef.o -lf -Lda` | 0 | — |
| `usef.o -Bstatic -Ldso -Lda -lf` | 0 | — |
| `usef.o -Ldso -Lda -l:libf.a` | 0 | — |
| `usef.o --library-path=da -lf`, `--library-path da -lf` | 0 | — |

Not found: `usef.o -Lda -lnosuch`: bfd `cannot find -lnosuch: No such file
or directory`; lld `unable to find library -lnosuch`; mold `library not
found: nosuch`. `-l:nosuch.a`: bfd `cannot find -l:nosuch.a: ...`; lld
`unable to find library -l:nosuch.a`; mold `library not found:
:nosuch.a`. `-Bstatic -lnosuch`: bfd adds `have you installed the static
version of the nosuch library ?`.

Empty `-L` (in a directory holding `libf.a` and `libz9.a`):
`t3 usef.o -L '' -lf`, `-L '' -lz9`: bfd 1 `cannot find`, lld 0, mold 1
`library not found`. `-lz9` without `-L`: all three not found (the working
directory is not searched).

### E4 Candidates that are not plain libraries

```sh
mkdir brk dirso garb txt loop
ln -sf /nonexistent brk/libf.so; mkdir dirso/libf.so
printf '\177ELF\002\001\001garbagegarbagegarbage' > garb/libf.so
printf 'this is not a script\n' > txt/libf.so; ln -sf libf.so loop/libf.so
t3 usef.o -L<dir> -Lda -lf      # for each dir
```

| first candidate | bfd | lld | mold |
|---|---|---|---|
| dangling link | 0 | 0 | 0 |
| a directory | 0 | 1: `cannot open dirso/libf.so: Is a directory` | 1: `mmap failed: No such device` |
| garbage ELF | 1: `file format not recognized` | 1: `unknown file type` | 1: `unknown file type` |
| text, not a script | 1: `file format not recognized; treating as linker script` ... `syntax error` | 1: `unknown directive: this` | 1: `unknown linker script token` |
| a link to itself (`ELOOP`) | 0 | 0 | 1: `opening loop/libf.so failed: Too many levels of symbolic links` |

### E5 Default directories, environment, paths

```sh
ld.bfd --verbose | grep SEARCH_DIR      # and the two cross linkers
gcc -### usef.o 2>&1 | grep collect2 | tr ' ' '\n' | grep -E '^"?-L'
clang-22 -### usef.o 2>&1 | grep -E 'ld"' | tr ' ' '\n' | grep -E '^"?-L'
ld.bfd --print-sysroot
t3 usef.o -lc; t3 usef.o -nostdlib -lc
LIBRARY_PATH=$PWD/da t3 usef.o -lf; LD_LIBRARY_PATH=$PWD/da t3 usef.o -lf
t3 usef.o -Ldd libq.a           # dd/libq.a exists, ./libq.a does not
ld.bfd usef.o - -o o < f.o      # and lld, mold
```

GNU ld's built-in directories:

- aarch64: `=/usr/local/lib/aarch64-linux-gnu`, `=/lib/aarch64-linux-gnu`,
  `=/usr/lib/aarch64-linux-gnu`, `=/usr/local/lib`, `=/lib`, `=/usr/lib`,
  `=/usr/aarch64-linux-gnu/lib`;
- x86-64: `=/usr/local/lib/x86_64-linux-gnu`, `=/lib/x86_64-linux-gnu`,
  `=/usr/lib/x86_64-linux-gnu`, `=/usr/local/lib64`, `=/lib64`,
  `=/usr/lib64`, `=/usr/local/lib`, `=/lib`, `=/usr/lib`,
  `=/usr/x86_64-linux-gnu/lib64`, `=/usr/x86_64-linux-gnu/lib`;
- i386: the same with `i386-linux-gnu`, `lib32` and `/usr/i686-linux-gnu`.

The drivers' `-L`s: `gcc`: `/usr/lib/gcc/aarch64-linux-gnu/14`,
`.../14/../../../aarch64-linux-gnu`, `.../14/../../../../lib`,
`/lib/aarch64-linux-gnu`, `/lib/../lib`, `/usr/lib/aarch64-linux-gnu`,
`/usr/lib/../lib`, `.../14/../../..`. `clang-22`:
`/usr/lib/gcc/aarch64-linux-gnu/14`, `/lib/aarch64-linux-gnu`,
`/usr/lib/aarch64-linux-gnu`, `/lib`, `/usr/lib`. The cross `gcc`s and
`clang-22 --target=x86_64-linux-gnu` give the analogous lists under
`/usr/lib/gcc-cross/<triple>/14` and `/usr/<triple>/lib`. None passes
`/usr/local/lib`.

`--print-sysroot`: empty. `usef.o -lc`: bfd finds `libc` (fails later on
`f`); lld `unable to find library -lc`; mold `library not found: c`.
`-nostdlib -lc`: bfd `cannot find -lc: file format not recognized`, lld and
mold as before. `LIBRARY_PATH`, `LD_LIBRARY_PATH` pointing at `da`: all
three `-lf` not found. `-Ldd libq.a`: bfd `cannot find libq.a`, lld and
mold `cannot open libq.a`. `-`: bfd `cannot find -: No such file or
directory`; lld `cannot open -: ...`; mold `unknown command line option: -`.

### E6 The sysroot

```sh
mkdir -p sr/lib; cp libf.a sr/lib/; cp -r sr/lib srlib; S=$PWD/sr
mkdir -p '$SYSROOT/lib'; cp sr/lib/libf.a '$SYSROOT/lib/'
```

| line (all with `usef.o -nostdlib`) | bfd | lld | mold |
|---|---|---|---|
| `--sysroot=$S -L=/lib -lf` | 0 | 0 | 0 |
| `--sysroot=$S '-L$SYSROOT/lib' -lf` | 0 | 1: not found | 0 |
| `--sysroot=$S -L/lib -lf` | 1 | 1 | 1 (not prefixed) |
| `--sysroot=$S -L=lib -lf` | 0 (`-t`: `.../srlib/libf.a`) | 0 (`-t`: `.../sr/lib/libf.a(f.o)`) | 0 (`-t` prints nothing for it) |
| `--sysroot=$S =/lib/libf.a` | 0 | 1: `cannot open =/lib/libf.a` | 1: `cannot open =/lib/libf.a` |
| `--sysroot=$S /lib/libf.a` | 1 | 1 | 1 (not prefixed) |
| `--sysroot=$S -L=/lib -l:libf.a` | 0 | 0 | 0 |
| no sysroot, `'-L$SYSROOT/lib' -lf` (literal directory) | 1 | 0 | 0 |
| no sysroot, `-L=sr/lib -lf` (`-t`) | `sr/lib/libf.a` | `sr/lib/libf.a(f.o)` | not found |
| `--sysroot=sr -L=/lib -lf` (without `-nostdlib`) | 0 | 0 | 0 |

### E7 `DT_NEEDED` of shared objects

```sh
mkdir dg hid
printf 'int f(void); int g(void){return f();}\n' > g.c
printf 'int g(void); void _start(void){ g(); for(;;); }\n' > useg.c; gcc -c useg.c
gcc -shared -fPIC f.c -o hid/libf.so -Wl,-soname,libf.so
gcc -shared -fPIC g.c -o dg/libg.so -Lhid -lf -Wl,-soname,libg.so
gcc -shared -fPIC g.c -o dg/libg2.so -Lhid -lf -Wl,-soname,libg2.so -Wl,-rpath,'$ORIGIN/../hid'
```

| line | bfd | lld | mold |
|---|---|---|---|
| `useg.o -Ldg -lg` | 1: `warning: libf.so, needed by dg/libg.so, not found (try using -rpath or -rpath-link)`; `dg/libg.so: undefined reference to 'f'` | 0 | 0 |
| `useg.o -Ldg -Lhid -lg` | 1: the same | 0 | 0 |
| `useg.o -Ldg -lg -rpath-link hid` | 0 | 0 | 0 |
| `LD_LIBRARY_PATH=$PWD/hid`, `useg.o -Ldg -lg` | 0 | 0 | 0 |
| `usef.o -rpath-link hid -lf` | 1: not found | 1 | 1 |
| `useg.o -Ldg -lg --no-allow-shlib-undefined` | 1: as the first row | 0 | 0 |
| `useg.o -Ldg -lg2` (`RUNPATH $ORIGIN/../hid`) | 0 | 0 | 0 |
| `usegf.o -Ldg --copy-dt-needed-entries -lg -rpath-link hid` | 0, `NEEDED` `libg.so`, `libf.so` | 1: `unknown argument '--copy-dt-needed-entries'` | 1: `unknown command line option` |
| `usegf.o -Ldg -lg -rpath-link hid` (uses `f` directly) | 1: `undefined reference to symbol 'f'` ... `DSO missing from command line` | 1: `undefined symbol: f` | 1: `undefined symbol: f` |

### E8 Implicit scripts and positional state

```sh
gcc -shared -fPIC h.c -o libh.so -Wl,-soname,libh.so; gcc -shared -fPIC f.c -o libf.so -Wl,-soname,libf.so; ar rcs libh.a h.o
printf 'INPUT ( libh.so libf.so )\n' > s.ld
printf 'INPUT ( AS_NEEDED ( libh.so ) libf.so )\n' > s2.ld
printf 'INPUT ( libh.a )\n' > s3.ld
```

| line | all three, `NEEDED` |
|---|---|
| `usef.o --as-needed s.ld` | `libf.so` |
| `usef.o s.ld` | `libh.so libf.so` |
| `usef.o s2.ld` | `libf.so` |
| `usef.o --as-needed s2.ld` | `libf.so` |

`usef.o f.o --whole-archive s3.ld --no-whole-archive`: `h` is linked by
all three (`nm out.X | grep -ci ' t h$'` is 1; mold makes it local, `t`);
without `--whole-archive`, by none.

System scripts (`grep -v '^/\*\|^ \*'`):

- `/usr/lib/aarch64-linux-gnu/libc.so`: `OUTPUT_FORMAT(elf64-littleaarch64)
  GROUP ( /lib/aarch64-linux-gnu/libc.so.6
  /usr/lib/aarch64-linux-gnu/libc_nonshared.a AS_NEEDED (
  /lib/ld-linux-aarch64.so.1 ) )`;
- `libm.so`: `GROUP ( /lib/aarch64-linux-gnu/libm.so.6 AS_NEEDED (
  /lib/aarch64-linux-gnu/libmvec.so.1 ) )`;
- `/usr/i686-linux-gnu/lib/libc.so` and `/usr/x86_64-linux-gnu/lib/libc.so`:
  the same form with `elf32-i386` / `elf64-x86-64`;
- `/usr/lib/gcc/aarch64-linux-gnu/14/libgcc_s.so`: `GROUP ( libgcc_s.so.1
  -lgcc )`.

### E9 Nested scripts and cycles

```sh
printf 'INPUT ( n2.ld )\n' > n1.ld; printf 'INPUT ( libf.so )\n' > n2.ld
printf 'INPUT ( c2.ld )\n' > c1.ld; printf 'INPUT ( c1.ld )\n' > c2.ld
printf 'INPUT ( self.ld )\n' > self.ld
t3 usef.o n1.ld
for P in ld.bfd ld.lld-22 mold; do timeout 10 $P usef.o f.o c1.ld -o o; done
T3LINES=3 timeout 20 t3 usef.o f.o self.ld
```

`n1.ld`: all three 0. `c1.ld`: GNU ld killed by `timeout` (124); lld
`PLEASE submit a bug report ... Stack dump:` (status 141); mold killed by
`timeout` (124). `self.ld`: the whole `t3` was killed by the 20 s timeout
with no output from GNU ld.

### E10 `SEARCH_DIR`

```sh
mkdir dd; cp libf.a dd/libq.a; printf 'SEARCH_DIR(dd)\n' > s.ld
t3 usef.o s.ld -lq; t3 usef.o -lq s.ld
```

`s.ld -lq`: bfd 0, lld 0, mold 1 (`SEARCH_DIR(dd) ^ unknown linker script
token`). `-lq s.ld`: all three not found.

### E11 Archive members

```sh
echo 'some text' > text.txt; ar rcs inner.a k.o; gcc -shared -fPIC k.c -o k.so
clang-22 -flto -c k.c -o kbc.o; gcc -flto -c k.c -o klto.o
gcc -nostdlib -static k.c -o kexe -Wl,-e,k; : > empty.o
for m in text.txt inner.a k.so kbc.o klto.o kexe empty.o; do
  rm -f a.a; ar rcS a.a f.o $m; ranlib a.a
  t3 usef.o a.a; t3 usef.o --whole-archive a.a --no-whole-archive
done
```

| member | lazy: bfd / lld / mold | `--whole-archive`: bfd / lld / mold |
|---|---|---|
| `text.txt` | 0 / 0 `warning: a.a: archive member 'text.txt' is neither ET_REL nor LLVM bitcode` / 0 | 1 `member a.a(text.txt) in archive is not an object` / 1 `a.a(text.txt): not an ELF file` / 0 |
| `inner.a` | 0 / 0 the same warning / 0 | 1 / 1 / 0 |
| `k.so` | 0 / 0 warning / 0 `warning: a.a(k.so): shared object file in an archive is ignored` | 0 / 1 `unknown section type 0xb` ... / 0 the warning |
| `kbc.o` | 0 / 0 / 1 `don't know how to handle this LTO object` | 1 not an object / 0 / 1 |
| `klto.o` | 0 / 0 / 1 | 0 `plugin needed to handle lto object` / 0 / 1 |
| `kexe` | 0 / 0 warning / 0 | 1 `cannot use executable file 'a.a(kexe)' as input to a link` / 0 / 0 |
| `empty.o` | 0 / 0 warning / 0 | 1 / 1 / 0 |

Members of another machine: E2 (`mixed2.a`). A scan of every regular
archive under `/usr/lib`, `/usr/aarch64-linux-gnu`, `/usr/x86_64-linux-gnu`,
`/usr/i686-linux-gnu` (95 archives; a Python script walking the `ar`
headers, reporting members other than `/`, `//`, `/SYM64/` that do not
start with `\x7fELF`) found none.

### E12 The symbol index

```sh
ar rcS noidx.a f.o; t3 usef.o noidx.a; mkdir ni; cp noidx.a ni/libf.a; t3 usef.o -Lni -lf
# A.a: m.o defining faa, B.a: m.o defining fbb (same length; ar rcsD)
# S.a: A.a's magic and index, then B.a's members (index says faa, member defines fbb)
sz=$(head -c 68 A.a | tail -c 60 | cut -c49-58 | tr -d ' '); end=$((8+60+sz+(sz%2)))
head -c $end A.a > S.a; tail -c +$((end+1)) B.a >> S.a
t3 usefbb.o S.a; t3 usefaa.o S.a
cp A.a bad.a; printf '\377\377\377\377' | dd of=bad.a bs=1 seek=68 conv=notrunc; t3 usefaa.o bad.a
```

| line | bfd | lld | mold |
|---|---|---|---|
| `noidx.a`, `-Lni -lf` | 1: `error adding symbols: archive has no index; run ranlib to add one` | 0 | 0 |
| `usefbb.o S.a` | 1: `undefined reference to 'fbb'` | 0 | 0 |
| `usefaa.o S.a` | 1: `undefined reference to 'faa'` | 1: `undefined symbol: faa` | 1: the same |
| `usefaa.o bad.a` (index count 0xffffffff) | 1: `bad.a: file format not recognized; treating as linker script` | 0 | 0 |

`nm -s S.a`: `Archive index: faa in m.o`, `m.o: T fbb`. `nm -s bad.a`:
`file format not recognized`.

### E13 Thin archives

```sh
(cd sub/objs && gcc -c f.c k.c); (cd sub && ar rcsT libt.a objs/f.o objs/k.o); ar t sub/libt.a
t3 usef.o sub/libt.a     # then with k.o removed; f.o removed; k.o or f.o rebuilt larger
```

`ar t sub/libt.a` (run outside `sub`): `sub/objs/f.o`, `sub/objs/k.o`.

| case | bfd | lld | mold |
|---|---|---|---|
| intact | 0 | 0 | 0 |
| unneeded `k.o` missing | 0 | 1: `sub/libt.a: could not get the buffer for a child of the archive: 'objs/k.o': No such file or directory` | 1: `cannot open sub/objs/k.o` |
| needed `f.o` missing | 1: `sub/libt.a(sub/objs/f.o): error opening thin archive member` | 1: as above for `objs/f.o` | 1: `cannot open sub/objs/f.o` |
| `k.o` or `f.o` replaced by a larger file (1240 → 1320 bytes) | 0 | 0 | 0 |

### E14 `--start-lib`

```sh
gcc -c k.c k2.c       # both define k
t3 usef.o --start-lib f.o k.o --end-lib
t3 usef.o f.o k.o --start-lib k2.o --end-lib
t3 usef.o f.o --whole-archive --start-lib k.o --end-lib --no-whole-archive
for P in ld.lld-22 mold; do
  $P usef.o --start-lib d1/libf.so --end-lib -o o; readelf -d o | grep -c NEEDED
  $P usef.o --start-lib libf.a --end-lib -o o
  $P usef.o f.o --start-lib inun.ld --end-lib -o o; nm o | grep -ci unused   # inun.ld: INPUT(un.o)
done
```

GNU ld: `unrecognized option '--start-lib'` on every line. lld and mold: 0
on every line; `k` absent from the output of the first and third line
(`nm | grep -ci ' t k$'` is 0); no duplicate definition on the second; the
shared object in a region gives one `NEEDED`; the archive links; the
object named by a script in the region is not linked (0).

### E15 The same object or archive twice

```sh
t3 usef.o f.o f.o; t3 usef.o f.o ./f.o
ar rcs libf.a f.o; ln -sf libf.a link.a
t3 usef.o libf.a libf.a
t3 usef.o --whole-archive libf.a libf.a --no-whole-archive
t3 usef.o --whole-archive libf.a link.a --no-whole-archive
t3 usef.o --whole-archive libf.a --no-whole-archive libf.a
t3 usef.o libf.a --whole-archive libf.a --no-whole-archive
```

| line | bfd | lld | mold |
|---|---|---|---|
| `f.o f.o`, `f.o ./f.o` | 1: `multiple definition of 'f'` | 1: `duplicate symbol: f` | 1: `duplicate symbol: f.o: f.o: f` |
| `libf.a libf.a` | 0 | 0 | 0 |
| whole `libf.a libf.a`, whole `libf.a link.a` | 1 | 1 | 1 |
| whole `libf.a`, then lazy `libf.a` | 0 | 0 | 0 |
| lazy `libf.a`, then whole `libf.a` | 1 | 1 | 0 |

### E16 Shared objects: sonames and needed names

```sh
mkdir d1 d2 ns; gcc -shared -fPIC f.c -o d1/libf.so -Wl,-soname,libf.so.1
cp d1/libf.so d2/libf.so; gcc -shared -fPIC f.c -o ns/libf.so; ln -sf d1/libf.so sym.so
```

| line | `NEEDED`, all three unless noted |
|---|---|
| `d1/libf.so d1/libf.so`; `d1/libf.so d2/libf.so`; `d1/libf.so sym.so` | `libf.so.1` (once) |
| `ns/libf.so` | `ns/libf.so` |
| `$PWD/ns/libf.so` | the absolute path |
| `./ns/../ns/libf.so` | `./ns/../ns/libf.so` |
| `-Lns -lf`; `-Lns -l:libf.so` | `libf.so` |
| script `INPUT ( ns/libf.so )` | bfd `./ns/libf.so`; lld, mold `ns/libf.so` |
| `scr/scr2.ld` = `INPUT ( libf.so )`, `scr/libf.so` exists | `scr/libf.so` |
| the same, found through `-Lns` | bfd, mold `ns/libf.so`; lld `libf.so` |
| `INPUT ( -lf )` with `-Lns` | bfd, lld `libf.so`; mold `ns/libf.so` |
| `start.o --as-needed d1/libf.so --no-as-needed d1/libf.so` (unused) | bfd, lld `libf.so.1`; mold none |
| `start.o d1/libf.so --as-needed d1/libf.so` | `libf.so.1` |
| `start.o ns/libf.so d1/libf.so` | `ns/libf.so libf.so.1` |

Same soname, other contents: `d2/libf.so` rebuilt from `fg.c` (defines `f`
and `g`) with soname `libf.so.1`; `usefg.o d1/libf.so d2/libf.so`,
`usefg.o d1/libf.so d2/libother.so` and a `d2/libg.so` defining only `g`:
all three fail with `g` undefined (lld: `did you mean: f, defined in:
d1/libf.so`).

### E17 Shared objects in static links; the interpreter

```sh
t3 usef.o d1/libf.so; readelf -lW out.X | grep -A1 INTERP
t3 -static usef.o d1/libf.so; t3 -Bstatic usef.o d1/libf.so
t3 usef.o -Bstatic d1/libf.so -Bdynamic; t3 usef.o -Bstatic -Lns -l:libf.so
mold -static usef.o d1/libf.so -o o; nm o | grep ' f$'; readelf -d o
```

No `-dynamic-linker`: GNU ld writes `[Requesting program interpreter:
/lib/ld-linux-aarch64.so.1]`; lld and mold write no `PT_INTERP`; all three
`EXEC`. The four static lines: bfd 1 ``attempted static link of dynamic
object `d1/libf.so'`` (`ns/libf.so` for `-l:`); lld 1 `attempted static
link of dynamic object d1/libf.so`; mold 0, and its output has `U f` and
`There is no dynamic section in this file`.

### E18 PIE input, no section headers, `DT_SONAME` twice

```sh
gcc -pie -fPIE pie.c -o pie.exe; readelf -h pie.exe | grep Type; readelf -d pie.exe | grep FLAGS_1
t3 usef.o pie.exe
readelf -l /lib/aarch64-linux-gnu/libc.so.6 | grep -c INTERP; readelf -d /lib/aarch64-linux-gnu/libc.so.6 | grep FLAGS_1
gcc -shared -fPIC f.c -o libns.so -Wl,-soname,libns.so; llvm-objcopy-22 --strip-sections libns.so libss.so
t3 usef.o libss.so
gcc -shared -fPIC f.c -o lib2.so -Wl,-soname,libfirst.so -Wl,-z,nodelete -lc
python3 -I patch.py lib2.so     # DT_FLAGS_1 entry -> DT_SONAME pointing 3 bytes into "libfirst.so"
for P in ld.bfd ld.lld-22 mold; do $P usef.o lib2.so -o o.$P; readelf -d o.$P | grep NEEDED; done
```

`patch.py` (Python, stdlib): read the section headers, find `SHT_DYNAMIC`,
take the `DT_SONAME` value `v`, and overwrite the `DT_FLAGS_1` entry with
`(DT_SONAME, v + 3)`; `readelf -d` then shows `SONAME [libfirst.so]` and
`SONAME [first.so]`.

- `pie.exe`: `DYN (Position-Independent Executable file)`, `FLAGS_1 Flags:
  PIE`. bfd 1 `cannot use executable file 'pie.exe' as input to a link`;
  lld 1 and mold 1 `undefined symbol: f`; no output has a `NEEDED`.
- `libc.so.6`: one `INTERP`; `FLAGS_1 Flags: NOW`.
- `libss.so` (`There are no sections in this file`): bfd 1 `libss.so: error
  adding symbols: file in wrong format`; lld 1, mold 1 `undefined symbol:
  f`; no `NEEDED`.
- `lib2.so`: bfd and lld `NEEDED [first.so]` (the last); mold
  `[libfirst.so]` (the first).

### E19 `-b` / `--format`

| line | bfd | lld | mold |
|---|---|---|---|
| `start.o -b binary bin.dat -b default` | 0 | 0 | 1: ``mold does not support `-b binary` `` |
| `--format=elf start.o` | 1: ``invalid BFD target `elf' `` | 0 | 1: `unknown command line option: -b elf` |
| `-b elf64-littleaarch64 start.o` | 0 | 1: `unknown --format value ... (supported formats: elf, default, binary)` | 1 |
| `-b elf64-x86-64 start.o` | 1: `invalid BFD target` | 1 | 1 |

### E20 Several bad inputs

```sh
printf 'garbage\001' > g1.o
t3 usef.o nosuch1.o -lnosuch nosuch2.o g1.o ../e1/xf.o
```

bfd: `cannot find nosuch1.o`, `cannot find -lnosuch`, `cannot find
nosuch2.o`, `g1.o: file not recognized` (and stops). lld: `cannot open
nosuch1.o`, `unable to find library -lnosuch`, `cannot open nosuch2.o`.
mold: `cannot open nosuch1.o` only.

### E21 The dependency file

```sh
for P in ld.bfd ld.lld-22 mold; do $P usef.o -L./d1/../d1 -lf sym.so libf.a --dependency-file=dep.$P -o o; done
for P in ...; do $P usef.o -Ld1/ -lf --dependency-file=dep.$P -o o; done
for P in ...; do $P usef.o scr1.ld libf.a --dependency-file=dep.$P -o o; done   # scr1.ld: INPUT ( ns/libf.so )
```

| case | bfd | lld | mold |
|---|---|---|---|
| `-L./d1/../d1 -lf` | `./d1/../d1/libf.so` | `d1/libf.so` | `d1/libf.so` |
| `-Ld1/ -lf` | `d1//libf.so` | `d1/libf.so` | not checked (mold writes one line; the probe read line 3) |
| `scr1.ld` | `scr1.ld` twice, `./ns/libf.so` | `scr1.ld` once, `ns/libf.so` | `scr1.ld` once, `ns/libf.so` |

`sym.so` stays `sym.so` in all three.

### E22 Read against map

Inputs: the `file` lines of the bench stamps (`.build/bench-work/workloads/<w>/stamp`,
`cut -f4`): `cxx-medium-g` 19 files, 210,582,942 bytes (two archives,
`libprotoc.a` 116.8 MB and `libprotobuf.a` 84.7 MB); `synthetic-objects`
1013 files, 85,973,817 bytes; `synthetic-archives` 113 files, 15,717,545
bytes. `getconf PAGESIZE`: 4096.

`rb.rs` (std only; `rustc -O --edition 2021 rb.rs`; the `mmap` call
declared `extern "C"`), arguments `MODE THREADS LIST [STRIDE]`: workers
take files from an atomic counter; `read`: `File::open`, `metadata().len()`,
`vec![0u8; len]`, `read_exact`, one more `read` must return 0, touch one
byte per `STRIDE`, keep the `Vec`; `mmap`: `mmap(NULL, len, PROT_READ,
MAP_PRIVATE, fd, 0)`, touch, keep the mapping; the descriptor is closed
after each file. It prints wall time and `VmHWM`, `RssAnon`, `RssFile`
from `/proc/self/status`.

`rbp.rs` (std only, no `unsafe`), arguments `THREADS LIST [CHUNK=4 MiB]`:
phase 1, workers open and `fstat` each file and read a file of at most
`CHUNK` bytes whole; phase 2, each larger file gets one `vec![0u8; len]`,
split by `chunks_mut(CHUNK)` into tasks `(Arc<File>, offset, &mut [u8])`
taken from a queue by the workers, each `read_exact_at`.

Each configuration ran 7 times; minimum and median (4th) wall time:

```sh
for r in 1 2 3 4 5 6 7; do ./rb MODE THREADS W.list STRIDE; done | sort -t= -k6 -n   # rb
for r in 1 2 3 4 5 6 7; do ./rbp THREADS W.list; done | sort -t= -k5 -n            # rbp
```

`rb`, series 1 (load 23.3 → 24.6):

| workload | mode, threads, stride | min ms | median ms | VmHWM KiB | RssAnon KiB | RssFile KiB |
|---|---|---|---|---|---|---|
| cxx-medium-g | read 1 4096 | 383.2 | 466.0 | 207552 | 205796 | 1756 |
| cxx-medium-g | read 8 4096 | 480.6 | 591.1 | 207640 | 205884 | 1756 |
| cxx-medium-g | read 16 4096 | 321.1 | 412.2 | 207692 | 205936 | 1756 |
| cxx-medium-g | mmap 1 0 | 1.4 | 2.0 | 1876 | 120 | 1756 |
| cxx-medium-g | mmap 8 0 | 1.1 | 1.3 | 1968 | 212 | 1756 |
| cxx-medium-g | mmap 1 4096 | 322.5 | 389.6 | 207576 | 124 | 207452 |
| cxx-medium-g | mmap 8 4096 | 212.7 | 247.6 | 207664 | 212 | 207452 |
| cxx-medium-g | mmap 16 4096 | 228.4 | 264.4 | 207688 | 236 | 207456 |
| synthetic-objects | read 1 4096 | 718.5 | 919.2 | 86012 | 84256 | 1756 |
| synthetic-objects | read 8 4096 | 161.4 | 194.8 | 86116 | 84360 | 1756 |
| synthetic-objects | read 16 4096 | 119.0 | 129.7 | 86204 | 84468 | 1736 |
| synthetic-objects | mmap 1 0 | 309.0 | 356.9 | 2008 | 248 | 1760 |
| synthetic-objects | mmap 8 0 | 66.7 | 69.4 | 2092 | 336 | 1756 |
| synthetic-objects | mmap 1 4096 | 773.0 | 896.1 | 87244 | 248 | 86996 |
| synthetic-objects | mmap 8 4096 | 128.6 | 174.0 | 87328 | 332 | 86996 |
| synthetic-objects | mmap 16 4096 | 80.4 | 88.6 | 87436 | 440 | 86996 |
| synthetic-archives | read 1 / 8 / 16 | 65.3 / 17.6 / 12.5 | 68.9 / 18.1 / 14.0 | 17236-17452 | 15500-15704 | ~1740 |
| synthetic-archives | mmap 8 0 | 8.5 | 8.6 | 1980 | 224 | 1756 |
| synthetic-archives | mmap 1 / 8 / 16, 4096 | 50.0 / 12.2 / 10.2 | 58.0 / 14.0 / 10.4 | 17496-17688 | 140-328 | 17360 |

`rbp` and `rb`, series 2 and 3 (load 23.2-23.8):

| workload | program, threads | min ms | median ms | VmHWM KiB | RssAnon KiB |
|---|---|---|---|---|---|
| cxx-medium-g | rbp 8 (series 2 / 3) | 132.1 / 174.5 | 145.3 / 197.4 | 207708 | 205888 |
| cxx-medium-g | rbp 16 | 113.8 / 150.7 | 121.4 / 167.7 | 207804-207824 | 205996-206000 |
| cxx-medium-g | rbp 1 (series 3) | 518.9 | 661.2 | 207620 | 205800 |
| synthetic-objects | rbp 8 | 163.1 / 187.6 | 192.1 / 196.5 | 86104-86120 | 84356-84360 |
| synthetic-objects | rbp 16 | 225.1 / 146.9 | 451.1 / 177.3 | 86220 | 84460 |
| cxx-medium-g | rb read 1 (series 3) | 351.9 | 442.5 | — | — |
| cxx-medium-g | rb mmap 1 / 8 / 16, 4096 (series 3) | 309.9 / 174.9 / 209.8 | 425.6 / 195.5 / 246.2 | — | — |

(Series 2 sorted `rbp`'s output on the wrong field, so its "min" for one
thread is not a minimum; its one-thread rows are not used.)

The bench results on the same machine (`.build/bench-work/results.tsv`,
5 runs, `noisy yes`): `cxx-medium-g` lld default median 933,842 µs, peak
RSS 332,248 KiB; mold default 724,882 µs, 411,156 KiB. `synthetic-objects`:
lld 1,236,068 µs, 272,740 KiB; mold 1,718,352 µs, 217,756 KiB.

Composition of `cxx-medium-g`'s archives (`readelf -SW` over every
member, summed by a Python filter; section headers counted at 64 bytes):

| archive | members | sections | `.symtab` + `.strtab` + `.shstrtab` | section headers | `.debug*` + `.rela.debug*` | file |
|---|---|---|---|---|---|---|
| `libprotobuf.a` | 84 | 68401 | 10.0 MB | 4.4 MB | 59.3 MB | 84.7 MB |
| `libprotoc.a` | 83 | 75457 | 11.5 MB | 4.8 MB | 88.5 MB | 116.8 MB |

### E23 A zeroed buffer under transparent huge pages

```sh
cat /sys/kernel/mm/transparent_hugepage/enabled     # [always] madvise never
# z.rs: let mut v = vec![0u8; 512 << 20]; print VmRSS; write v[i] = 1 every 1 MiB; print VmRSS
rustc -O z.rs -o z && ./z
```

`after vec![0; 512 MiB]: VmRSS=1620 kB`; `after writing one page per MiB:
VmRSS=164880 kB`.

### E24 `-z max-page-size` (for `spec/layout.md`)

```sh
for v in 0x10000 65536 64K 64k 3 0 010000 -4096 4096x; do
  for P in ld.bfd ld.lld-22 mold; do $P usef.o -z max-page-size=$v --defsym f=0 -o o; done; done
```

`0x10000`, `65536`, `010000`: all accept. `64K`, `64k`, `-4096`, `4096x`:
all reject (bfd `invalid maximum page size`; lld `invalid max-page-size`;
mold `not a number` or `value must be a power of 2`). `3`: all reject (lld,
mold: not a power of 2). `0`: bfd accepts; lld and mold reject. How each
reads `010000` (octal or decimal) was not checked.

### E25 `-R`

```sh
for P in ...; do $P usef.o d1/libf.so -R d1 -o o; readelf -d o | grep -E 'RPATH|RUNPATH'; done
gcc -nostdlib -static -Wl,-e,f f.c -o fexe
for P in ...; do $P usef.o -R fexe -o o; done
```

`-R d1` with a shared object: `RUNPATH [d1]` in all three (with no shared
input, no dynamic section). `-R fexe`: bfd prints nothing (`f` taken from
`fexe`'s symbols; the status was not captured); lld `undefined symbol: f` (read as an rpath); mold `-R as an
alias for --just-symbols is not supported`.

### E26 `--exclude-libs`

```sh
gcc -c fx.c -o fx.o; mkdir ex; ar rcs ex/libex.a fx.o
for v in libex.a ex/libex.a libex ALL; do for P in ...; do
  $P -shared usef.o ex/libex.a --exclude-libs=$v -o o.so; nm -D o.so | grep -c ' T f$'; done; done
```

`f` in `.dynsym` (1 = exported, 0 = excluded):

| value | bfd | lld | mold |
|---|---|---|---|
| `libex.a` | 0 | 0 | 0 |
| `ex/libex.a` | 1 | 1 | 1 |
| `libex` | 0 | 1 | 1 |
| `ALL` | 0 | 0 | 0 |

## Decisions and reasons

The ranked list for review; each with the references in one line.

1. **Target without `-m`: the first ELF file named by path, else the first
   in load order** (§3.1). GNU ld: built-in default; lld: first ELF in load
   order; mold: first ELF named by path. Rule 2 is known before any search,
   so multilib skipping works; lld's rule picks the wrong target for
   `-Lx86 -Larm -lf main.o` [E2].
2. **No built-in search directories; the environment is never read**
   (§4.2, §4.6). GNU ld: 7-11 built-in directories, reads
   `LD_LIBRARY_PATH` for `DT_NEEDED`; lld and mold: none, and neither
   reads the environment for `-l` [E5, E7]. The drivers pass complete
   lists; a link must not depend on the host or the shell. `-nostdlib`
   becomes a no-op with that reason.
3. **The archive symbol index is never used** (§7.4). GNU ld trusts it
   (fails without one, extracts wrongly when stale); lld and mold decide
   from the members [E12]. mortar reads every member's symbols anyway.
4. **Incompatible `-l` candidates are skipped with a warning** (§4.4).
   GNU ld and mold skip and warn; lld errors [E2].
5. **Shared objects: one per soname; `DF_1_PIE` refused; no section
   headers refused; `DT_SONAME` twice is malformed** (§8.1, §8.2; the
   supervisor's three questions). PIE: GNU ld refuses, lld and mold read it
   as a library and find nothing; `PT_INTERP` cannot be the test because
   `libc.so.6` has one [E18]. No section headers: GNU ld refuses, lld and
   mold accept and find no symbols [E18]; the `PT_DYNAMIC` path of WP-006's
   reader has no oracle. Duplicate soname: GNU ld and lld take the last,
   mold the first [E18]; the reader already refuses a repeated tag.
6. **Archive members that are not objects: warning when lazy, error under
   `--whole-archive`; LTO and other-machine members refused even when
   lazy** (§7.3). The three disagree on nearly every cell [E11].
7. **Slim GCC LTO and LLVM bitcode refused by name** (§2.1, §2.2). GNU ld
   and lld (slim GCC) link without the code, exit 0; mold refuses [E1].
8. **Store: read, never map; chunked parallel reads; changed files are
   errors** (§11). Chunked reads cost about what map-and-touch costs on
   this machine [E22]; mapping is `unsafe` and turns truncation into
   `SIGBUS`.
9. **Every mention is its own file and its own `ArchiveId`, except shared
   objects** (§9). All three treat repeated objects and archives as
   separate inputs [E15, E16]; model.md's "first appearance" is changed.
10. **Errors and refusals where references misbehave**: script cycles
    (GNU ld and mold hang, lld crashes [E9]); `=dir` without `/` under a
    sysroot (three readings [E6]); `-L ''` (lld alone searches the working
    directory [E3]); shared object with `-static` (mold writes a broken
    executable [E17]); no `-dynamic-linker` with shared objects (GNU ld
    invents an interpreter, lld and mold write none [E17]); soname-less
    shared object named by a script (three different `NEEDED` names
    [E16]); `--exclude-libs` without `.a` (GNU ld alone matches [E26]).
11. **Keys: `seq = k × 2^40 + offset`** (§12.2): one argument can reach many
    files (`-lc` reaches three), so the file's ordinal within the unit goes
    in front of the offset.

## What could not be verified

- Which directory mold uses for `-L=lib` under a sysroot (`-t` prints
  nothing for it) [E6].
- GNU ld's use of `LD_RUN_PATH`; sysroot prefixing of `-rpath-link`,
  `-rpath` and `DT_RUNPATH` in the `DT_NEEDED` search.
- `copy_dt_needed` merged over several mentions of one soname.
- LLVM fat LTO objects (`clang -ffat-lto-objects`) in the references.
- The references on FIFOs and devices as inputs.
- How each reference reads `-z max-page-size=010000`.
- Whether `STARTUP` behaves in GNU ld as its manual says (not run).
