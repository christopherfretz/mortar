import sys
p = sys.argv[1]; s = open(p).read()
def rep(old, new, count=1):
    global s
    n = s.count(old)
    if n != count: raise SystemExit('expected %d got %d: %r' % (count, n, old[:80]))
    s = s.replace(old, new)

# D-100 intro: measured set
rep('''Appendix A, and `[A]`-`[F]` below cite the `dyn` area's parts.''',
'''Appendix A, and `[A]`-`[F]` below cite the `dyn` area's parts. The
second design pass (after the adversarial review) re-measured what it
changed with the scripts of `.build/target-wp-110/exp2/` (README there;
cited `exp2/<dir>`), on AArch64 natively and x86-64 under qemu-user.''')

# D-102 row "after 1" and rows 3-4
rep('''| after 1 | `DT_SONAME` iff `-soname` (the name as given), then `DT_SYMBOLIC` iff `-Bsymbolic` without `--dynamic-list` (D-112; GNU ld; lld writes `DF_SYMBOLIC` only, mold neither) |''',
'''| after 1 | `DT_SONAME` iff `-soname` (the name as given), then `DT_SYMBOLIC` iff the line's last `-Bsymbolic` form is `-Bsymbolic` and it has no `--dynamic-list` (D-112; GNU ld; lld writes `DF_SYMBOLIC` only, mold neither) |''')
rep('''| 3-4 | `DT_INIT`/`DT_FINI`: `-init`/`-fini` naming a symbol that is not defined is refused by name, `undefined-init-symbol` (all three silently omit both tags and the library's `_init`/`_fini` never run, [B] `b_init_undef`) |''',
'''| 3-4 | `DT_INIT`/`DT_FINI`: an explicit `-init`/`-fini` naming a symbol that is not defined is refused by name, `undefined-init-symbol` (all three silently omit both tags and the library's `_init`/`_fini` never run, [B] `b_init_undef`). Without the option, an undefined default `_init`/`_fini` (`-nostdlib`, no `crti.o`) omits the tag silently, as D-30 rows 3-4 |''')

# D-103 -Bsymbolic row
rep('''| `-Bsymbolic` (without `--dynamic-list`, D-112) | `DF_SYMBOLIC` | — | the library binds its own definitions |''',
'''| `-Bsymbolic` (the line's last `-Bsymbolic` form, without `--dynamic-list`, D-112) | `DF_SYMBOLIC` | — | the library binds its own definitions |''')

# D-110 item 2
rep('''2. linker-defined (file 0): exported only when `ref_dynamic`, as D-21
   item 2 says for executables. No reference exports one in `-shared`
   ([X-A] A1: none of `_end`, `_edata`, `__bss_start`); layout L-100's
   "exported `STB_GLOBAL STV_DEFAULT` when defined (GNU and lld)" is
   withdrawn (request R-L10);''',
'''2. linker-defined (file 0): exported only when `ref_dynamic`, as D-21
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
   (lld's W23 drops it);''')

# D-111 m-8
rep('''Fields: D-23's rows; an import whose shared definition is a
function gets `STT_FUNC` (lld and mold give `__cxa_finalize` `WEAK FUNC`;
GNU ld `NOTYPE`); an export keeps its `.symtab` type and binding, an
exported preemptible IFUNC `STT_GNU_IFUNC` at its resolver (D-121).''',
'''Fields: D-23's rows; an import whose shared definition is a
function gets `STT_FUNC`, and keeps the binding of the references to it:
`STB_WEAK` when every reference is weak (crtbeginS's `__cxa_finalize`:
`WEAK FUNC` with every reference, AArch64, x86-64 and i386, 9 census
libraries; the first pass's "GNU ld `NOTYPE`" did not reproduce), else
`STB_GLOBAL`; a weak undefined key with no shared definition is item
3's `WEAK NOTYPE` (`__gmon_start__`, `_ITM_*`); an export keeps its
`.symtab` type and binding, an exported preemptible IFUNC
`STT_GNU_IFUNC` at its resolver (D-121).''')
open(p, 'w').write(s)
