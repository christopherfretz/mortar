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
  under `qemu-x86_64`/`qemu-i386`. lld 22 agreed with lld 19 on every
  point below, so "lld" means both.
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

### E-10 Stack

```
xstack.o (.note.GNU-stack with "x"):
  bfd  GNU_STACK RWE 0x10 + "warning: xstack.o: requires executable stack
       (because the .note.GNU-stack section is executable)"
  lld  GNU_STACK RW 0 (silent)
  mold GNU_STACK RW 0x1 + "warning: xstack.o: this file may cause a segmentation
       fault because it requires an executable stack"
nostack.o (no note): RW in all three (bfd 0x10, lld 0, mold 0x1).
```

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

## 3. Derivations, not experiments

- **L-8 (RELRO safety) at 64 KiB.** No 64 KiB-page kernel was available.
  The rule follows from glibc's documented rounding (`[align_down(s),
  align_down(e))`). lld's 4 KiB end (E-1) leaves `0x2a0000..0x2a4000`
  writable under 64 KiB pages. That range is RELRO data only, because
  the next segment starts in a later 64 KiB page, so it is safe but
  unprotected. The rule's real danger, non-RELRO data inside the rounded
  range, would need a RELRO start inside a page shared with writable
  data. L-70 prevents that for mortar.
- **L-22 cost.** The offset pass is a prefix sum over 16-byte rows; the
  2-5 ms per 10⁶ sections is an estimate from memory bandwidth, not a
  measurement (WP-027 measures).
- **L-112 capacity.** 4 MiB / 12 bytes per AArch64 thunk is about
  350 000 thunks; no link was made with more than 128 MiB of code.

## 4. What was not verified

- No 64 KiB-page kernel: the 64 KiB rules (L-8, L-64, L-70) are checked
  by arithmetic only.
- x86-64 and i386 ran only under qemu-user, which ignores some loader
  details (it does not enforce `PT_GNU_STACK` or RELRO protection).
- glibc's order of IRELATIVE application and RELRO protection in a
  static non-PIE start-up was not confirmed by experiment. L-62 keeps
  `.got.plt` writable, so mortar does not depend on it.
- `perf`, `valgrind`, `dwz` and `debugedit` are not installed: no
  timing, no memory profile, and no check that dwz and debugedit accept
  the layout. Both rewrite `.debug_*` and are used by distribution
  packaging, not by daily builds.
- Range-extension thunks were not exercised: no program with more than
  128 MiB of code was linked. The island rules (L-110 to L-113) rest on
  AAELF64 and `spec/arch.md`.
- `-z keep-text-section-prefix`, `--sort-section` and `--symbol-ordering-file`
  were not examined; they stay refused.
- mold's behaviour with an undefined entry symbol was not observed.
- Fortran and C++ outputs were checked for running correctly and for
  section order, not in every detail of their symbols.
- No link tests were added; the existing `tests/link/layout/*.test`
  already cite the section names `layout.md` uses ("Segments",
  "Thread-local storage", "Initialization and termination", "Entry point",
  "Static executables", "Garbage collection").
