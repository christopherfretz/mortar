import sys
p = sys.argv[1]; s = open(p).read()
def rep(old, new):
    global s
    if s.count(old) != 1: raise SystemExit('count %d: %r' % (s.count(old), old[:70]))
    s = s.replace(old, new)
rep('''and both compilers emit it in `-fPIC` code: 1,169 such uses, not
refused).''', '''and both compilers emit it in `-fPIC` code: 913 code-place uses whose
address lies in a function, over the corpus, none refused; per-group
counts in `exp2/work/b1-analysis/<group>.txt`).''')
rep('''| GCC x86-64, Clang AArch64 and x86-64, GCC i386 `-fPIE` | 212 | 3 | the boundary cases; every bound definition (261) is named by symbol, refused by D-117's table already |''',
'''| GCC x86-64, Clang AArch64 and x86-64, GCC i386 `-fPIE` | 212 | 5 | the boundary cases; of the 262 bound definitions, 253 are named by symbol (refused by D-117's table already) and 9 by this rule; none unseen |''')
rep('`exp2/probe/gap2.c` is one', '`exp2/gapdemo/gap2.c` is one')
open(p, 'w').write(s)
