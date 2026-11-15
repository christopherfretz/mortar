# WP-024 stage 1 — `spec/layout.md`: method and evidence

This document backs `spec/layout.md` (draft). It gives the method, the
experiments that the non-obvious rules rest on, and what could not be
checked. The `E-n` labels below are the ones `layout.md` cites.

## 1. Method

- **Sources.** The gABI and the Linux gABI extensions; AAELF64; the
  x86-64 and i386 psABIs; *ELF Handling For Thread-Local Storage*; the GNU
  ld manual and GNU ld's built-in default script (`ld --verbose`, public
  output); the documented behaviour of the Linux ELF loader and of glibc's
  static start-up and `_dl_protect_relro`. No linker source was read.
- **Oracle.** The observed output of GNU ld 2.44, lld 19.1, lld 22 and
  mold 2.37.1, all driven through `gcc -fuse-ld=...` on an AArch64 Debian
  host (4 KiB pages). x86-64 and i386 were linked with the
  `x86_64-linux-gnu-gcc` and `i686-linux-gnu-gcc` cross toolchains and run
  under `qemu-x86_64`/`qemu-i386`. "lld" means lld 19 and 22 where they
  agree. They do not agree on an exec `.note.GNU-stack` (E-10, E-20) or
  on where `.tbss`-only TLS goes (E-22); those points name the version.
  (The first draft said they agreed on every point; the review of
  2026-11-14 refuted that.)
- **Review fixes.** The experiments E-18 to E-24 were made for the
  review of 2026-11-14 (§5), with the same linkers, GCC 14, Clang 19
  and 22, gfortran 14, bare `ld`/`ld.lld`/`mold` as well as the driver,
  and the reviewer's scripts (`elfscan.py`, `laymodel.py`,
  `pagesim.py`), which are not part of the repository.
- **Tools checked.** gdb 16.3, lldb 19, binutils strip/objcopy/readelf
  2.44, eu-elflint (elfutils, `--gnu-ld`).
- **Programs.** Small C, C++ and Fortran programs: hello in C, a C++
  program with exceptions, a Fortran program; TLS with mixed alignment;
  constructors and destructors with priorities, `.ctors`/`.preinit_array`
  inputs; orphans with `__start_`/`__stop_`; a symbol probe that prints
  every linker-defined symbol; an exec and a missing `.note.GNU-stack`.
  The programs were linked `-static` (the M1 case), `-no-pie`, `-pie` and
  `-static-pie`, and some with `-shared`.
- **Harness.** One script linked each program with all four linkers:

```sh
# link.sh <name> <driver> <objs...> <flags>: out/<name>.<linker> and .map
for L in bfd lld lld22 mold; do
  case $L in bfd) F=-fuse-ld=bfd;; lld) F=-fuse-ld=lld;;
    lld22) F="-B$DIR/l22/ -fuse-ld=lld";; mold) F=-fuse-ld=mold;; esac
  $drv $F "$@" -o out/$name.$L -Wl,-Map=out/$name.$L.map 2>out/$name.$L.err
done
```

  Everything ran in one scratch directory under `/tmp`, which was removed
  at the end.

## 2. Experiments

### E-1 Segments, AArch64 static (`gcc -static hello.c`)

```
bfd   LOAD 0x000000 0x400000 filesz 0x07f5c4            R E 0x10000
      LOAD 0x08c160 0x48c160 0x0057e4 memsz 0x00acb8     RW  0x10000
      GNU_RELRO 0x48c160 size 0x3ea0  -> ends 0x490000 (64 KiB)
lld   LOAD 0x000000 0x200000 0x027b60                    R   0x10000
      LOAD 0x027b80 0x237b80 0x0579f0                    R E 0x10000
      LOAD 0x07f570 0x29f570 0x003e28 memsz 0x004a90     RW  0x10000
      LOAD 0x0833a0 0x2b33a0 0x001940 memsz 0x006e14     RW  0x10000
      GNU_RELRO 0x29f570 size 0x4a90 -> ends 0x2a4000 (4 KiB only)
      [17] .relro_padding NOBITS 0x2a3398 size 0xc68
mold  LOAD 0x000000 0x200000 0x029940                    R   0x10000
      LOAD 0x029940 0x239940 0x057b50                    R E 0x10000
      LOAD 0x081490 0x2a1490 0x003bb8 memsz 0x00eb70     RW  0x10000
      LOAD 0x085050 0x2b5050 0x001930 memsz 0x006e08     RW  0x10000
      GNU_RELRO 0x2a1490 size 0xeb70 -> ends 0x2b0000 (64 KiB)
      [24] .relro_padding NOBITS 0x2a5048 size 0xafb8
```

- The next-segment rule (L-70) checks out on every lld/mold boundary:
  - lld R end 0x227b60 → align_up(·, 0x10000) = 0x230000, and offset
    0x27b80 (aligned to 0x20, the `.text` alignment) gives
    0x230000 + 0x7b80 = 0x237b80;
  - mold RELRO end 0x2b0000, next offset 0x85050 → 0x2b5050.
- GNU's RW start 0x48c160 leaves a file gap of 0x7f5c4 → 0x8c160.
- `-z separate-code` (`sc-separate-code`):
  - lld: R@0, RX at offset and address 0x30000/0x230000, RW at
    0x90000/0x290000, so both page-aligned in the file;
  - GNU: R, R E, R, RW (rodata after text);
  - mold matches lld.

### E-2 Image bases (static, first `PT_LOAD`)

```
              aarch64     x86_64      i386
bfd           0x400000    0x400000    0x08048000
lld           0x200000    0x200000    0x00400000
mold          0x200000    0x200000    0x00200000
```

### E-3 `.text` input order (`orphan.c`: functions in `.text.unlikely.x`, `.text.exit.z`, `.text.startup.z`, `.text.hot.y`, then `main` in `.text`; source order cold, hot, startup, exit)

```
bfd:  cold@400300 ex@4003ac st@400500 hot@4005cc main@400760
lld:  cold@237ce0 hot@237ce8 st@237cf0 ex@237cf8 main@237d00
mold: cold@239ba0 hot@239ba8 st@239bb0 ex@239bb8 main@239bc0
```

- GNU groups by category: unlikely, exit, startup, hot, then the rest,
  as its default script says.
- lld and mold keep input order. The GNU sections between groups are
  libc's own.

### E-4 init priorities (`init.c`)

```
bfd:  preinit ctor101 ctor200 ctor65000 ctors.00300 ctors-plain ctor-default main dtor101
lld:  preinit ctor101 ctor200 ctor65000 ctor-default main dtor101
mold: preinit ctor101 ctor200 ctor65000 ctors.00300 ctors-plain ctor-default main dtor101
```

- `.ctors.00300` runs after `ctor65000`: priority 65535 − 300 = 65235.
- The plain `.ctors` input runs in input order among the unnumbered
  entries.
- lld runs neither `.ctors` entry.

### E-5 `.ctors` reversal (`ctA.c`: two `.ctors` words A1, A2 and an `.init_array` entry; `ctB.c`: B1; `.dtors` AD1, AD2)

```
bfd:  A2 A1 A-init_array B1 main AD1 AD2
lld:  A-init_array main
mold: A2 A1 A-init_array B1 main AD1 AD2
```

- lld output: `.ctors` and `.dtors` are separate `PROGBITS` output
  sections, outside the init array bounds.
- `readelf -SW` on crt1.o, crti.o, crtbegin*.o, crtend*.o and crtn.o for
  all three targets shows no `.ctors` or `.dtors` section.

### E-6 Linker-defined symbols (`probe.c` prints `&sym` for weak references; static AArch64)

```
                       bfd        lld        mold
__executable_start     0x400000   base       base     (lld, mold: local hidden)
__ehdr_start           0x400000   base       base     shndx 1 in all
etext/_etext           0x459e00   0x28f930   0x291850   (end of the last exec section: bfd .fini,
                                                       lld .iplt; __etext: bfd only)
edata/_edata           0x4a1940   0x2b5198   0x2b6d54
__bss_start            0x4a1940   0x2b51a0   0x2b6d60   (= .bss start in all three)
end/_end               0x4a6e18   0x2ba664   0x2bc238
__end__, __bss_end__   0x4a6e18   undef      undef
__preinit_array_start  0x49c0a0   base/1     (nil)    (bfd: = __init_array_start,
                                                       in .tbss's index)
__rela_iplt_start/end  0x4001f0/0x400268 (bfd); equal pairs in PIE for mold only
_GLOBAL_OFFSET_TABLE_  .got start in all three
__start_mysec          protected  protected  local
__GNU_EH_FRAME_HDR     (nil) static: bfd none, mold defines
_DYNAMIC               (nil) in static outputs
```

- In `probe-static.bfd`:
  - `mysec` (WA) ends at 0x4a1940;
  - `.bss` starts at 0x4a1940, size 0x54d8, so it ends at 0x4a6e18.
- In lld, `.got.plt` ends at 0x2b5198 (= `_edata`) and `.bss` starts at
  0x2b51a0.
- GNU's default script (`ld --verbose`) has `_edata = .;` and
  `__bss_start = .;` before `.bss`, and `. = ALIGN(. != 0 ? 64 / 8 : 1);`
  before `_end`. `__bss_start__`, `_bss_end__` and `__bss_end__` are in
  the AArch64 script.
- `-shared` (`ss.c`): GNU and lld export `_end`, `_edata` and
  `__bss_start` `GLOBAL DEFAULT`.

### E-7 TLS (`tls2.c`: an 8-aligned initialised and a 128-aligned zeroed variable)

```
bfd  .tdata 0x49c100 size 0x38 align 128 | .tbss 0x49c180 size 0x28 | .init_array 0x49c180
     TLS 0x49c100 filesz 0x38 memsz 0xa8 align 0x80
lld  .tdata 0x29f300 size 0x38 align 8   | .tbss 0x29f380 size 0x28 | .fini_array 0x29f338
     TLS 0x29f300 filesz 0x38 memsz 0xa8 align 0x80
mold .tdata 0x2a1200 size 0x38 align 8   | .tbss 0x2a1280 size 0x21 | .data.rel.ro 0x2a1240
     TLS 0x2a1200 filesz 0x38 memsz 0xa1 align 0x80
```

- `p_vaddr` is 128-aligned in all three.
- GNU raises `.tdata`'s alignment, and the next section starts at the
  `.tbss` address. lld and mold start it at `.tdata`'s end.
- mold's unpadded `.tbss` gives memsz 0xa1: the exact end, which is what
  L-78 takes.
- TLS symbol values are offsets from the template start in all three.

### E-8 Output section order (`orphan.c`, static; alloc part, as `readelf -SW` lists it)

```
bfd:  .note.gnu.build-id .rela.plt .init .plt .text .fini .rodata myro
      .eh_frame .gcc_except_table .note.ABI-tag .tdata .tbss .init_array .fini_array
      .data.rel.ro .got .got.plt .data .retained .weird.name mysec .bss
lld:  .note.ABI-tag .note.gnu.build-id .rela.dyn .rodata myro .gcc_except_table .eh_frame
      .text .init .fini .iplt .tdata .tbss .fini_array .init_array .data.rel.ro .got
      .relro_padding .data .tm_clone_table .retained .weird.name mysec .got.plt .bss
mold: notes .rela.dyn .rodata.cst8 .rodata.str1.16 .rodata.str1.8 .rodata.str4.8 myro ...
      .fini .init .plt.got .text .tdata .tbss .data.rel.ro .fini_array .init_array .got
      .relro_padding .data .got.plt .retained .tm_clone_table .weird.name mysec .bss
```

- The non-alloc part is `.comment`, then `.symtab`/`.strtab`/`.shstrtab`
  in various orders; the section header table is last in all three.
- lld also keeps `.gnu.warning.dlopen` and `.gnu.warning.dlmopen` as
  orphans (E-17).
- In every output, the section headers of allocated sections are in
  address order.

### E-9 Program header order

```
bfd  pie:    PHDR INTERP LOAD LOAD DYNAMIC NOTE NOTE GNU_EH_FRAME GNU_STACK GNU_RELRO
     static: LOAD LOAD NOTE NOTE TLS GNU_STACK GNU_RELRO
lld  pie:    PHDR INTERP LOAD×4 DYNAMIC GNU_RELRO GNU_EH_FRAME GNU_STACK NOTE
     static: PHDR LOAD×4 TLS GNU_RELRO GNU_STACK NOTE
mold pie:    PHDR INTERP NOTE LOAD×4 DYNAMIC GNU_EH_FRAME GNU_STACK GNU_RELRO
     static: PHDR NOTE LOAD×4 TLS GNU_EH_FRAME GNU_STACK GNU_RELRO
```

GNU emits no `PT_PHDR` in a static output or in a shared object.

### E-10 Stack (corrected)

The first draft's E-10 was wrong in two points: lld 22 does not behave
as lld 19, and "no note: RW in all three" holds only through the
driver, whose crt files carry notes. Through the driver
(`gcc -static`):

```
xstack.o (.note.GNU-stack "x"):
  bfd     GNU_STACK RWE 0x10 + "warning: xstack.o: requires executable stack
          (because the .note.GNU-stack section is executable)"
  lld 19  GNU_STACK RW 0 (silent)
  lld 22  error: xstack.o: requires an executable stack, but -z execstack is not specified
  mold    GNU_STACK RW 0x1 + "warning: xstack.o: this file may cause a segmentation
          fault because it requires an executable stack"
nostack.o (no note) + crt files ("" notes): RW in all three (bfd 0x10, lld 0, mold 0x1).
```

Bare `ld` with no note in any input: see E-20.

### E-11 Clang inputs

- `clang -c` objects carry `.llvm_addrsig` (`SHF_EXCLUDE`, type
  `SHT_LLVM_ADDRSIG`). None of the three references keeps it in the
  output.
- `-gz` (zlib-compressed `.debug_*`): all three write the debug sections
  uncompressed (no `C` flag).

### E-12 eu-elflint --gnu-ld

- On lld and mold outputs:
  `section [17] '.relro_padding': ... expected RELR`.
  This is elfutils matching names by the prefix `.relr`. Renaming the
  section to `_relro_padding` (same type and flags, with a python
  header-patch script) cleared the finding.
- On all three:
  - `__ehdr_start` and `__executable_start` "st_value out of bounds"
    (the symbol is at the file header, before section 1);
  - `.tbss` symbols "out of bounds" (false positives).
- lld only:
  - `.rodata` "merge flag set but entry size is zero";
  - `.plt`/`.iplt` processor flag 0x20000000.
- mold only: `_TLS_MODULE_BASE_` and TLS symbols (A3 in `plumb.md`).
- AArch64 `IRELATIVE` records: "invalid type" on all three, because
  elfutils' AArch64 backend lacks the type. Filtered out.

### E-13 `__start_`/`__stop_` under `--gc-sections`

```
gcc -static -Wl,--gc-sections orphan.o
  bfd, mold: links; prints the section sizes
  lld:       ld.lld: error: undefined symbol: __stop_myro
             >>> referenced by orphan.c ... orphan.o:(main)
```

lld 19 and lld 22 both behave this way; the cause is lld's default
`-z start-stop-gc`.

### E-14 strip and RELRO padding

- On an lld static output, `strip` and `objcopy` kept all program
  headers unchanged.
- After the `.relro_padding` section header's `sh_size` was patched to 0,
  `strip` rewrote the RELRO `PT_LOAD`'s `p_memsz` to 0x1d8, the end of
  the last real section, while `PT_GNU_RELRO` kept 0x710: the two
  segments no longer agree.
- Conclusion: tools rebuild segment sizes from section headers, so
  padding in memory must be a section (L-9, L-151).

### E-15 Swapped section headers

- `.rodata` and `.eh_frame` (both allocated) were swapped in the section
  header table of a GNU static `-g` output (python header patch).
- Results:
  - the program ran;
  - gdb `break main`, `run` and `bt` worked;
  - `strip` produced a running program;
  - `objcopy` kept the swapped order;
  - eu-elflint reported nothing new.
- Address order of section headers is therefore a choice (L-153).

### E-16 Debuggers

- gdb 16.3: `break main`, `run` and `bt` worked on the static `-g` C
  outputs of all three linkers.
- lldb 19: `b main` resolved on all three. `run` needed
  `settings set target.disable-aslr false` in this container (most
  likely because the container does not allow disabling ASLR; not
  investigated further).

### E-17 `.gnu.warning` and glibc orphans

- In a static hello (glibc of the host), `libc.a` members bring
  `.gnu.warning.dlopen` and `.gnu.warning.dlmopen`, which say "Using
  'dlopen' in statically linked applications requires...".
- GNU consumes them; lld copies them as non-allocated orphans; mold
  makes one `.gnu.warning`. No linker printed a warning, because nothing
  referenced `dlopen`.
- This glibc has no `__libc_*` orphan sections in the hello link.
- Consequence: M1 must accept `.gnu.warning*`. The first draft refused
  it, and E-17 reversed that.

### E-18 AArch64 build attributes (`h.c`, hello)

```
compiler, -mbranch-protection=   .note.gnu.property      .ARM.attributes
clang-22 standard                BTI, PAC, GCS           SHT_AARCH64_ATTRIBUTES (0x70000003), 0x24 bytes
clang-22 bti                     BTI                     same type
clang-22 pac-ret                 PAC                     same type
clang-22 none                    none                    none
gcc-14   standard                BTI, PAC                none
```

Static driver link of the clang-22 `standard` object:

- GNU: output note "BTI, PAC" (GCS is dropped by the AND with `crt1.o`,
  which has BTI, PAC); no `.ARM.attributes` in the output.
- lld: the same note; `.ARM.attributes` copied into the output.
- mold: no property note at all; no `.ARM.attributes`.
- All three ran the program ("hi").

### E-19 Section types in compiler output (the elfscan approach)

- Corpus: `a.cc` (C++ with EH and TLS), `b.c` (C with orphans and TLS),
  `f.f90` (Fortran, AArch64 only), compiled by GCC 14, Clang 19 and
  Clang 22 for AArch64, x86-64 and i686, in ten flag sets each: `-O0`,
  `-O2`, `-O2 -g`, `-O2 -gz`, `-fPIC`, `-ffunction-sections
  -fdata-sections`, `-fpatchable-function-entry=2`,
  `-fstack-protector-strong`, `-fno-asynchronous-unwind-tables`,
  `-Os -g -fcommon`; plus `-mbranch-protection=standard` (GCC, AArch64)
  and `-fcf-protection` variants. 210 objects.
- Scanned with the reviewer's `elfscan.py` together with the crt files,
  libgcc, libgcc_eh and, on x86, libc.a.
- The only type conflict within an output section: x86-64 `.eh_frame`,
  `SHT_PROGBITS` (0x1, GCC and its runtime) against
  `SHT_X86_64_UNWIND` (0x70000001, Clang 19 and 22). None on AArch64 or
  i686. No alloc/non-alloc or TLS/non-TLS conflict anywhere.
- Allocated orphans seen: `__patchable_function_entries` (flags 0x283
  and 0x83, `SHF_LINK_ORDER`), `myset` (the test's own), `.gnu.offload_*`
  (AArch64), `rodata.cst32` (x86-64 libc).
- Mixed link, x86-64 static: Clang 22 C++ EH object with GCC's crt files
  and libgcc_eh: GNU, lld and mold all ran it ("ctor x", the exception
  caught), and the output `.eh_frame` was `PROGBITS` in all three.
- Clang-only object linked with bare `ld`/`ld.lld`/`mold`: GNU's output
  `.eh_frame` is `SHT_X86_64_UNWIND`; lld's and mold's are `PROGBITS`.

### E-20 `PT_GNU_STACK` with bare linkers (`n`, `r`, `x` objects: no note, a `""` note, an `"x"` note; identical results on AArch64, x86-64 and i686)

```
GNU ld 2.44 (bare):
  no note in any input             no PT_GNU_STACK
  no note, -z execstack            RWE 0x10, no warning
  no note, -z noexecstack          RW  0x10
  "" note                          RW  0x10
  "" note + a no-note object       RW  0x10, no warning
  "x" note                         RWE 0x10, "ld: warning: x-<arch>.o: requires executable
                                   stack (because the .note.GNU-stack section is executable)"
  "x" note, -z execstack           RWE 0x10, no warning
  "x" note, -z noexecstack         RW  0x10, no warning
  two "x" objects                  RWE 0x10, one warning, naming the first file in load order
                                   (both orders tried)
lld 19: RW 0 in every case, silent.
lld 22: "x" note: "ld.lld-22: error: x-<arch>.o: requires an executable stack,
        but -z execstack is not specified"; otherwise RW 0.
mold:   "x" note: RW 0x1 + "this file may cause a segmentation fault because it
        requires an executable stack. See https://github.com/rui314/mold/tree/main/docs/execstack.md";
        otherwise RW 0x1.
Driver + a no-note object: GNU RW 0x10, lld RW 0, mold RW 0x1, no warnings.
```

GNU's `--help` lists `--warn-execstack` (the default) and
`--error-execstack`, with their negations.

### E-21 `-z max-page-size` and large alignment (GNU ld 2.44, static)

```
N           aarch64 base  x86_64 base  i386 base     note
0           default       default      default       0 means the default
0x1, 0x800  0x400000      0x400000     0x08048000    accepted; p_align = N, except AArch64 at 0x1: 4 (section align)
0x1000      0x400000      0x400000     0x08048000
0x10000     0x400000      0x400000     0x08040000
0x200000    0x400000      0x400000     0x08000000
0x400000    0x400000      0x400000     0x08000000
0x800000    0             0            0x08000000
0x1000000   0             0            0x08000000
0x3000      rejected: "invalid maximum page size `0x3000'"
```

So GNU's base is `align_down(default base, N)`. From the review: lld
accepts 2 and 4 MiB; mold accepts 2 MiB and refuses 4 MiB (the base is
not a multiple).

`al.c` (an `_Alignas(0x40000)` read-only and an `_Alignas(0x40000)`
writable object), linked at base 0: all three raise the `p_align` of
each segment that holds one to 0x40000 and keep `p_vaddr ≡ p_offset
(mod 0x40000)`; the other segments keep 0x10000:

```
lld   R    off 0x000000 vaddr 0x000000 align 0x40000
      R E  off 0x080100 vaddr 0x090100 align 0x10000
      RW   off 0x080300 vaddr 0x0a0300 align 0x10000   (RELRO)
      RW   off 0x0c0000 vaddr 0x0c0000 align 0x40000
GNU   R E  off 0x000000 vaddr 0x000000 align 0x10000
      R    off 0x040000 vaddr 0x040000 align 0x40000   (the read-only data, own segment)
      RW   off 0x08fdc8 vaddr 0x09fdc8 align 0x10000
      RW   off 0x0c0000 vaddr 0x0c0000 align 0x40000
```

mold has the same shape as lld (E-23 lists its addresses).

### E-22 TLS of `.tbss` only (`tbo.c`, bare link) and `PT_GNU_RELRO` `p_filesz`

```
GNU     .tbss 0x41ffc0 (64-aligned), .init_array 0x41ffc0
        TLS off 0xffc0 vaddr 0x41ffc0 filesz 0 memsz 0x18 align 0x40
        GNU_RELRO filesz 0x2c memsz 0x40
lld 22  .tbss at 0x210280, inside the RX segment's address range
mold    .tbss at 0x220300, after the start of .init_array
```

`PT_GNU_RELRO` `p_filesz`: lld and mold give the file part only (an lld
static output: filesz 0x8, memsz 0xd98); GNU gives `p_filesz =
p_memsz` in a normal static link, and less in the `tbo` case above.

### E-23 The amended rules, re-checked by model

`mymodel.py` (scratch, not kept) loads the reviewer's `laymodel.py`,
replaces its address assignment with the amended L-67, L-69, L-70,
L-76 and L-77 (segment `p_align` P = max(maxpage, largest chunk
alignment); the first TLS chunk's effective alignment T; a new segment
at `o = align_up(file_end, a)`, `A` least ≥ `align_up(prev_end,
maxpage)` with `A ≡ o (mod P)`; `.tbss`-only per GNU; `.relro_padding`
to maxpage), and checks L-1 modulo `p_align`, L-5, L-23, the RELRO end
at maxpage, L-8 at 4, 16 and 64 KiB, and that no two segments share a
page.

- Inputs: the sections of 15 reference links: `tb` (TLS, aligned 64),
  `tbo` (`.tbss` only), `al` (a 0x40000-aligned section), `a` (C++ EH),
  the symbol probes `pr-a64`, `pr-x64`, `pr-386`, and the mixed x86-64
  EH link, from GNU, lld and mold.
- Result: all 15 OK at the default maxpage (64 KiB AArch64, 4 KiB x86).
  For example `tbo`: RELRO at 0x420240, offset 0x240 (a multiple of 64);
  `al`: R and RW segments with `p_align` 0x40000, RX and RELRO 0x10000.
- AArch64 `tb`, `tbo`, `al`, `a` at maxpage 4 KiB, 16 KiB and 2 MiB:
  all OK (at 2 MiB the base stays 0x400000).
- `pagesim.py` on the reference outputs that already follow the amended
  rules (`tbo` and `al` from GNU, lld 22 and mold): OK at 4, 16 and
  64 KiB; lld's leave a 0x1000 RELRO tail writable at 16 and 64 KiB
  (A7, the 4 KiB RELRO end).
- Not done: a binary written to the amended layout; mortar has no
  layout code yet (WP-027). The reviewer's lld-22 builds with
  `--image-base=0x400000 -z max-page-size=MP -z common-page-size=MP`
  are the nearest equivalent and passed run, strip, objcopy, gdb, lldb
  and pagesim on all three targets.

### E-24 Linker-defined symbols without runtime files, GOT headers, fill

- `pr.c` probes weak references to `_GLOBAL_OFFSET_TABLE_`,
  `__start_nosuchsec`, `__stop_nosuchsec`, `__tdata_start`,
  `__data_start`, `__dso_handle`, `__rela_iplt_start`/`_end`. (On x86 the
  printed values were unreliable, weak references under `-fno-pic`
  static; only the section sizes were used there.)
- `__start_nosuchsec`/`__stop_nosuchsec`: nil in all three.
- `__tdata_start`: GNU's scripts on all three targets have
  `PROVIDE_HIDDEN(__tdata_start = .)`; with no `.tdata` GNU still defines
  it at the location counter; lld and mold leave it undefined weak.
- `__data_start`: `PROVIDE(__data_start = .)` at the start of `.data`
  is in GNU's AArch64 script only.
- `__dso_handle` with no crt file (`dso.c`): GNU reports an undefined
  reference; lld defines it `LOCAL HIDDEN` at the base, index 1; mold
  defines it `LOCAL` at the start of the first section.
- `_GLOBAL_OFFSET_TABLE_` with no GOT on AArch64: GNU makes `.got`
  8 bytes and `.got.plt` 0x18; lld an empty `.got` (size 0) with the
  symbol at it, `LOCAL HIDDEN`; mold `.got` 8 and `.got.plt` 0x18.
- Static `.got.plt` on x86: x86-64 GNU and lld 0xc8 (3 reserved words
  and the IRELATIVE slots), mold 0x18; i386 GNU and lld 0x40, mold 0xc.
- Fill: GNU's AArch64 script (`ld --verbose`) ends `.init`, `.text` and
  `.fini` with `=0x1f2003d5` (NOP); its x86-64 and i386 scripts have no
  fill expression.

## 3. Derivations, not experiments

- **L-8 (RELRO safety) at 64 KiB.** No 64 KiB-page kernel was available.
  The rule follows from glibc's documented rounding (`[align_down(s),
  align_down(e))`). lld's 4 KiB end (E-1) leaves `0x2a0000..0x2a4000`
  writable under 64 KiB pages. That range is RELRO data only, because
  the next segment starts in a later 64 KiB page, so it is safe but
  unprotected. The rule's real danger, non-RELRO data inside the rounded
  range, would need a RELRO start inside a page shared with writable
  data. L-70 prevents that for mortar.
- **L-22 cost.** The in-chunk offset pass is an aligned scan over
  16-byte rows: sequential within a chunk, parallel across chunks. An
  aligned scan is not an additive prefix sum, so `par::prefix_sum`
  serves only the bucket offsets and section totals. A 10⁶-section
  `.text` makes the sequential scan the long pole; the blocked residue
  scan of L-22 is the parallel option. Nothing was measured (WP-027).
- **L-112 arithmetic.** With `S = 2²⁷ − 2²² − 32`, padding ≤ 16, an
  alignment gap ≤ 3 and the cap `C = 2²²`, the farthest patch of an
  island is at most `2²⁷ − 25` bytes from a site of its part, under the
  forward branch reach `2²⁷ − 4`. 4 MiB / 12 bytes per AArch64 thunk is
  about 350 000 thunks; no link was made with more than 128 MiB of code.
- **Condition (C) for L-111.** Islands are one per exec body part, from
  stage C sizes, so fixed before round 1. A new thunk of a key goes to
  island *k* only when no thunk of the key is in reach, and every byte of
  island *k* is in reach of part *k* (L-112), so island *k* never gets a
  second thunk of a key. An erratum site's patch in island *k* is always
  in range, so the site is never re-patched. Hence arch.md §6's bound
  `N ≤ islands × (thunk keys + erratum keys)`.
- **Stage D as a function of the patch set (L-21).** Stage D reads the
  stage C output (sizes and alignments that no round changes), the
  patch chunks' sizes (a function of the patch set, L-115) and `Facts`,
  and makes one sequential pass in `order`; each gap, each 843419 pad
  and the `.relro_padding` size is a function of the address reached so
  far. So the addresses are a function of the patch set, which is all
  arch.md §6 assumes. The pads are not monotone (a pad can shrink when
  an island before it grows), and arch.md §6's termination argument
  does not need them to be.

## 4. What was not verified

- No 64 KiB-page kernel: the 64 KiB rules (L-8, L-64, L-70) are checked
  by arithmetic only (E-23, pagesim).
- x86-64 and i386 ran only under qemu-user, which ignores some loader
  details (it does not enforce `PT_GNU_STACK` or RELRO protection).
- The kernel's default stack with no `PT_GNU_STACK` (executable on
  i386 through `READ_IMPLIES_EXEC`, not on x86-64 ≥ 5.8 or arm64) is
  from the kernel source as documented, not run. L-84 does not depend
  on it: mortar always emits the header.
- That a non-PIE output with base 0 (GNU at `-z max-page-size` ≥ 8 MiB)
  fails to load was not run; L-68 refuses it from `vm.mmap_min_addr`.
- L-68's rounding of the base by a section alignment larger than
  maxpage, as opposed to by `-z max-page-size`, was observed only at
  base 0 (E-21).
- glibc's order of IRELATIVE application and RELRO protection in a
  static non-PIE start-up was not confirmed by experiment. L-62 keeps
  `.got.plt` writable, so mortar does not depend on it.
- `perf`, `valgrind`, `dwz` and `debugedit` are not installed: no
  timing, no memory profile, and no check that dwz and debugedit accept
  the layout. Both rewrite `.debug_*` and are used by distribution
  packaging, not by daily builds.
- Range-extension thunks and 843419 padding were not exercised: no
  program with more than 128 MiB of code was linked, and mortar has no
  layout code yet. The island rules (L-110 to L-118) rest on AAELF64,
  `spec/arch.md` §6 and `spec/a64.md` §9 and §11, and the arithmetic of
  §3.
- No binary was written to the amended layout (E-23).
- mold's acceptance of 2 MiB and refusal of 4 MiB max pages, and lld's
  and mold's undefined-entry behaviour, are the reviewer's results, not
  re-run here.
- lld's and mold's code fill was not examined (L-82).
- `-z keep-text-section-prefix`, `--sort-section` and `--symbol-ordering-file`
  were not examined; they stay refused.
- Fortran and C++ outputs were checked for running correctly and for
  section order, not in every detail of their symbols.
- No link tests were added; the existing `tests/link/layout/*.test`
  already cite the section names `layout.md` uses ("Segments",
  "Thread-local storage", "Initialization and termination", "Entry point",
  "Static executables", "Garbage collection").

## 5. Review of 2026-11-14 and dispositions

An independent review of head 92ae3f9 reported 19 findings and
commented on the open questions; the coordinator ruled on each. One line
per item: the disposition, and where it is fixed.

1. `SHT_AARCH64_ATTRIBUTES` refused: **accepted.** Accepted and dropped;
   feature marking only from the AND-merged property note; what is lost
   stated; full merge later. L-35 (table and detail), §20.6, §20.7
   (resolve.md §10, §17 item 9); E-18.
2. x86-64 `.eh_frame` UNWIND vs `PROGBITS`, "other mixtures refused":
   **accepted.** Compatible; output UNWIND iff every live input is
   (GNU); every other refused mixture is absent from the scan of 210
   compiler objects and the runtime libraries on all three targets.
   L-49, L-47; E-19.
3. a64.md §11.1 item 2 start rule missing: **accepted.** L-118 (new),
   L-69 (`pad_k`), L-112 (in the reach), §20.5.
4. `PT_LOAD` `p_align` always maxpage: **accepted.** L-67 (`P`), L-19
   [G] (new), L-1, L-70, L-125; E-21.
5. First TLS chunk unsatisfiable: **accepted.** L-23 [G] (new); L-69
   effective alignment; L-70 `o = align_up(file_end, a)`; L-76; L-128;
   §20.1 item 4; re-checked in E-23.
6. L-21 vs address-dependent paddings: **accepted.** L-21 rewritten
   (stage D outputs: gaps, 843419 pad, `.relro_padding`); L-64 (chunk
   always exists, size set by D); L-145 (cost restated); §3; §20.5 (the
   `sized` state at M2).
7. L-111/L-114/L-115 vs arch.md §6: **accepted.** §14 cites arch.md §6
   as the one text and adds only the island policy satisfying (C)
   (L-111), the reach arithmetic (L-110, L-112) and the in-island order
   by key (L-115); L-114 withdrawn; L-113, L-116, L-117 restated.
8. L-62 "no code page readable as data": **accepted.** L-62 reworded
   (separate-code only).
9. Slack vs section headers and symbols: **accepted.** L-86 (`sh_size`
   to `size`), L-92 (every end symbol at `size`), L-141 (where slack
   lives; no slack in a `PT_LOAD`'s last chunk), L-142, L-143, L-147
   (merged chunks, append-only, new), §13.1 table, §20.1 item 5.
10. E-10 wrong: **accepted.** E-10 corrected, E-20 added, §1 "lld"
    definition corrected; L-84 rewritten (always emitted, GNU's flags
    per the table, GNU's warning text, one per link); §21 rows.
11. `-z max-page-size` limit 2²⁰: **accepted, with one exception.**
    L-74: any power of two from 4096 up (2 and 4 MiB included), 0 = the
    default; L-68: GNU's base rounding. Exception: GNU also accepts
    powers of two below 4096; mortar refuses them, because the output
    would break L-1 (contiguous offsets, mapped at 4 KiB pages). E-21.
12. Symbol table entries: **accepted.** §13.1 rows `__dso_handle`
    (lld and mold define it), `__data_start` (AArch64 `PROVIDE`),
    `__tdata_start` (new, L-105), `__rela_iplt_*` (mold); L-99; E-24.
13. `.tbss`-only TLS: **accepted.** GNU's placement (it satisfies L-10,
    L-23, L-1). L-77; §21; E-22, E-23.
14. Unstated values: **accepted.** `PT_GNU_RELRO` `p_filesz` (L-67);
    `_GLOBAL_OFFSET_TABLE_` tied to `GOT_BASE` with AArch64's `.got`
    header word and x86's 3-word static `.got.plt` (L-101, §13.1,
    §20.5); absent `__start_SEC` undefined weak = 0 (§13.1); L-47
    conflicts count live inputs only.
15. Q6 ruling follow-ons: **accepted.** L-103 (warning `entry-not-found`,
    `e_entry = 0`, exit 0; `-shared`/`-r` silent), L-104 carve-out,
    §20.3 S7, §20.7 (resolve.md §5.5), §20.1 item 7.
16. i386 limit: **accepted.** L-73: the class's limit, 2³² for
    ELFCLASS32, refused by name; kernel splits not the linker's concern.
17. Rule classes: **accepted.** L-6 and L-12 now [C]; L-16 reduced to
    "at most one" with the rest in L-84; L-19 and L-23 added as [G];
    §17 table.
18. References to absent rules: **accepted.** L-41 cites resolve.md §10
    (elf.md has no list); L-33 cites resolve.md's `gnu-warning`.
    arch.md and a64.md are on the branch since the merge of main.
19. Nits: **accepted.** `.relro_padding` kept, eu-elflint's `.rel*`
    complaint recorded and expected in the harness (L-156, §21); NOBITS
    `sh_offset` undisplaced (L-69); L-62 `.eh_frame` wording; L-96
    option names; trap fill labelled a choice, GNU's NOP stated (L-82);
    L-22 parallel steps stated (`par::prefix_sum` for bucket offsets,
    in-chunk scan sequential, blocked scan a WP-027 option).

Open questions:

- Q1 decided: RELRO padded to maxpage (L-64).
- Q2 decided: no separate-code on any target; GNU differs on x86-64 and
  i386 (L-71, §21).
- Q3 decided: see 19 (L-156).
- Q5 decided: GNU's bases (L-68).
- Q6 decided, overruling the draft: warning, `e_entry = 0` (L-103).
- Q7 decided: GNU's `.text` categories; it affects every `-O2` link
  (L-53).
- Q9 decided: see 10 (L-84).
- Q4 and Q8 stay open for M7 (§22).

Nothing in the review was refuted. Not established by experiment: the
items of §4 that the dispositions rely on (the kernel default without
`PT_GNU_STACK`, base 0 failing to load, a binary written to the amended
layout, a thunk link).
