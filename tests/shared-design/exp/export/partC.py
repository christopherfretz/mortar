# Part C: preemption inside a shared output: which internal references bind locally, per option; proven
# by interposition (LD_PRELOAD of a library that defines the same names).
import sys, os, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *

work = init(sys.argv[1], 'C')
S = open(os.path.join(work, 'summary.txt'), 'w')
def out(s=''):
    S.write(s + '\n'); S.flush()

write('c_lib.c', r'''
static volatile int one = 1;
__attribute__((noinline)) int f(void){return one;}
int v = 1;
__attribute__((weak, noinline)) int wf(void){return one;}
__attribute__((weak)) int wv = 1;
#ifndef NO_IFN
static int ifn_impl(void){return 1;}
static void *ifn_res(void){return (void*)ifn_impl;}
int ifn(void) __attribute__((ifunc("ifn_res")));
#else
int ifn(void){return 1;}
#endif
__attribute__((visibility("protected"))) int pf(void){return 1;}
int call_f(void){return f();}     int read_v(void){return v;}
void *addr_f(void){return (void*)f;} void *addr_v(void){return &v;}
int call_wf(void){return wf();}   int read_wv(void){return wv;}
void *addr_wf(void){return (void*)wf;} void *addr_wv(void){return &wv;}
int call_ifn(void){return ifn();} void *addr_ifn(void){return (void*)ifn;}
int call_pf(void){return pf();}   void *addr_pf(void){return (void*)pf;}
''')
write('c_pre.c', 'int f(void){return 3;} int v=3; int wf(void){return 3;} int wv=3; int ifn(void){return 3;} int pf(void){return 3;}\n')
write('c_host.c', r'''
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdio.h>
extern int call_f(void),read_v(void),call_wf(void),read_wv(void),call_ifn(void),call_pf(void);
extern void *addr_f(void),*addr_v(void),*addr_wf(void),*addr_wv(void),*addr_ifn(void),*addr_pf(void);
#define G(n) dlsym(RTLD_DEFAULT, n)
int main(){
  printf("f=%d%d v=%d%d wf=%d%d wv=%d%d ifn=%d%d pf=%d%d\n",
    call_f(), addr_f()==G("f"), read_v(), addr_v()==G("v"), call_wf(), addr_wf()==G("wf"),
    read_wv(), addr_wv()==G("wv"), call_ifn(), addr_ifn()==G("ifn"), call_pf(), addr_pf()==G("pf"));
  return 0;
}
''')
write('vslocal.map', '{ global: call_*; read_*; addr_*; local: *; };\n')
write('dl_f.list', '{ f; };\n')

COMPS = [('gcc', 'gcc', []), ('gcc_nsi', 'gcc', ['-fno-semantic-interposition']), ('clang22', 'clang22', []),
         ('clang22_si', 'clang22', ['-fsemantic-interposition']), ('clang22_nsi', 'clang22', ['-fno-semantic-interposition']), ('clang19', 'clang19', []), ('gcc_noifunc', 'gcc', ['-DNO_IFN'])]
OPTS = [('default', []), ('Bsymbolic', ['-Wl,-Bsymbolic']), ('Bsym-func', ['-Wl,-Bsymbolic-functions']),
        ('Bsym-nwf', ['-Wl,-Bsymbolic-non-weak-functions']), ('Bsym-nw', ['-Wl,-Bsymbolic-non-weak']),
        ('vs-local', ['-Wl,--version-script=vslocal.map']), ('dynlist-f', ['-Wl,--dynamic-list=dl_f.list']),
        ('dynlist-f+Bsym', ['-Wl,--dynamic-list=dl_f.list', '-Wl,-Bsymbolic'])]
NAMES = ('f', 'v', 'wf', 'wv', 'ifn', 'pf')

def short(t):
    for p in ('R_AARCH64_', 'R_X86_64_', 'R_386_'):
        t = t.replace(p, '')
    return t

for a in ARCHES:
    os.makedirs(os.path.join(work, a), exist_ok=True)
    cc(a, 'gcc', 'c_pre.c', a + '/pre.o', ['-fPIC'])
    link(a, 'bfd', [a + '/pre.o'], a + '/pre.so')
    cc(a, 'gcc', 'c_host.c', a + '/host.o', [])
    out('=== C %s: relocations the compilers emit in c_lib.o (-fPIC -O2), .text only' % a)
    for cn, comp, fl in COMPS:
        o = '%s/lib_%s.o' % (a, cn)
        cc(a, comp, 'c_lib.c', o, ['-fPIC', '-O2'] + fl)
        rl = relocs(os.path.join(work, o))
        txt = ' '.join('%s:%s' % (short(t), s) for sec, off, t, s in rl if sec in ('.rela.text', '.rel.text'))
        rs, syms = sh(['readelf', '-W', '-s', o])
        loc = sorted(set(l.split()[-1] for l in syms.splitlines() if ('$local' in l or 'localalias' in l)))
        ra, asm = sh(compiler(a, comp) + ['-S', '-o', '-', 'c_lib.c', '-fPIC', '-O2'] + fl)
        calls = sorted(set(l.strip().split('#')[0].split('//')[0].strip() for l in asm.splitlines() if l.strip().split('#')[0].split('//')[0].strip().startswith(('call', 'jmp', 'bl\t', 'b\t'))))
        out('  %-10s %s' % (cn, txt))
        out('  %-10s branch instructions in the assembly: %s' % ('', ' ; '.join(calls)))
        out('  %-10s local aliases in .symtab: %s' % ('', loc or 'none'))
    out('=== C %s: link, records, DT flags, runs (cell f=<call><addr==dlsym>: call 1=library\'s own, 3=preloaded; '
        'addr 1=same as the global definition)' % a)
    for cn, comp, fl in COMPS:
        for on, ofl in OPTS:
            for L in LINKERS:
                if L == 'lld19' and on not in ('default', 'Bsym-nwf', 'Bsym-nw', 'dynlist-f+Bsym'):
                    continue
                d = '%s/%s.%s.%s' % (a, cn, on, L)
                os.makedirs(os.path.join(work, d), exist_ok=True)
                rc, txt = link(a, L, ['%s/lib_%s.o' % (a, cn)], d + '/libc1.so', ['-Wl,-soname,libc1.so'] + ofl)
                if rc:
                    out('  %-11s %-15s %-5s REFUSED: %s' % (cn, on, L, first_diag(txt, 2)))
                    continue
                e, tags = dyn_summary(os.path.join(work, d + '/libc1.so'))
                fl_ = []
                if 'SYMBOLIC' in tags: fl_.append('DT_SYMBOLIC')
                if 'FLAGS' in tags: fl_.append('DT_FLAGS=%#x' % tags['FLAGS'][0])
                if 'FLAGS_1' in tags: fl_.append('FLAGS_1=%#x' % tags['FLAGS_1'][0])
                rl = relocs(os.path.join(work, d + '/libc1.so'))
                recs = ' '.join('%s:%s' % (short(t), s.split('@')[0]) for sec, off, t, s in rl
                                if s.split('@')[0] in NAMES) or '-'
                nirel = sum(1 for sec, off, t, s in rl if 'IRELATIVE' in t)
                ds = {s['name']: s for s in e.dynsym()}
                exp = ''.join(n + ('' if n in ds else '!') for n in [])
                notexp = [n for n in NAMES if n not in ds]
                ex = '%s/host.%s' % (d, 'x')
                rc2, t2 = link(a, 'bfd', [a + '/host.o', '-L' + d, '-lc1'], ex, [], shared=False)
                r0 = run(a, ex, [d])[1].strip()
                r1 = run(a, ex, [d], preload=a + '/pre.so')[1].strip()
                warn = (' WARN: ' + first_diag(txt, 1)) if txt.strip() else ''
                out('  %-11s %-15s %-5s plain: %s | preload: %s | %s noexport=%s IREL=%d recs=[%s]%s' % (
                    cn, on, L, r0, r1, ','.join(fl_) or '-', ','.join(notexp) or '-', nirel, recs, warn))
S.close()
