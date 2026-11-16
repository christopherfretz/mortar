# Part G: exports of archive members (a user archive; libgcc.a members pulled by -shared through the gcc
# driver's static libgcc), and a symbol defined both in the output and in a needed shared input.
import sys, os, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *

work = init(sys.argv[1], 'G')
S = open(os.path.join(work, 'summary.txt'), 'w')
def out(s=''):
    S.write(s + '\n'); S.flush()

def short(t):
    for p in ('R_AARCH64_', 'R_X86_64_', 'R_386_'):
        t = t.replace(p, '')
    return t

def dsyms(path):
    e = Elf(os.path.join(work, path))
    return {s['name']: s for s in e.dynsym()[1:]}

def sdesc(s):
    if s is None:
        return 'absent'
    return '%s/%s/%s/%s' % ('UND' if s['shndx'] == 0 else 'DEF', BIND.get(s['bind'], s['bind']),
                            TYPE.get(s['type'], s['type']), VIS.get(s['vis'], s['vis']))

# ---- G1: user archive members
write('g_m1.c', 'int ga_used(void){return 1;} int ga_same_member(void){return 2;}\n'
                '__attribute__((visibility("hidden"))) int ga_hidden(void){return 3;}\n')
write('g_m2.c', 'int ga_unpulled(void){return 4;}\n')
write('g_main.c', 'int ga_used(void); int lib_entry(void){return ga_used();}\n')
# ---- G1b: libgcc.a helpers
write('g_libgcc.c', r'''
unsigned long long udiv(unsigned long long a, unsigned long long b){ return a / b; }
int pop(unsigned long long x){ return __builtin_popcountll(x); }
int counter;
int bump(void){ return __atomic_fetch_add(&counter, 1, __ATOMIC_ACQ_REL); }
''')
HELPERS = ['__udivdi3', '__popcountdi2', '__aarch64_ldadd4_acq_rel', '__aarch64_have_lse_atomics',
           'init_have_lse_atomics']
GFLAGS = {'a64': ['-moutline-atomics'], 'x64': ['-mno-popcnt'], 'i386': ['-mno-popcnt']}

# ---- G2: a symbol defined in the output and in a needed shared input
write('g_dep2.c', 'int dup_f(void){return 2;} int dup_w(void){return 2;} int dup_v = 2;\n')
write('g_out.c', r'''
__attribute__((noinline)) int dup_f(void){return 1;}
__attribute__((weak, noinline)) int dup_w(void){return 1;}
int dup_v = 1;
int call_dup(void){return dup_f();} int call_w(void){return dup_w();} int read_v(void){return dup_v;}
''')
write('g_host.c', r'''
#include <stdio.h>
int dup_f(void), dup_w(void), call_dup(void), call_w(void), read_v(void); extern int dup_v;
int main(){ printf("host:dup_f=%d dup_w=%d dup_v=%d lib:call_dup=%d call_w=%d read_v=%d\n",
                    dup_f(), dup_w(), dup_v, call_dup(), call_w(), read_v()); return 0; }
''')

for a in ARCHES:
    os.makedirs(os.path.join(work, a), exist_ok=True)
    out('=== G %s: user archive libga.a (m1: ga_used ga_same_member ga_hidden[hidden]; m2: ga_unpulled), '
        'output references ga_used only' % a)
    cc(a, 'gcc', 'g_m1.c', a + '/m1.o', ['-fPIC'])
    cc(a, 'gcc', 'g_m2.c', a + '/m2.o', ['-fPIC'])
    cc(a, 'gcc', 'g_main.c', a + '/main.o', ['-fPIC'])
    sh(['ar', 'rcs', a + '/libga.a', a + '/m1.o', a + '/m2.o'])
    for L in LINKERS:
        for vn, fl in (('plain', []), ('exclude-libs=libga.a', ['-Wl,--exclude-libs,libga.a'])):
            o = '%s/ga.%s.%s.so' % (a, vn.replace('=', '_').replace('.', '_'), L)
            rc, t = link(a, L, [a + '/main.o', '-L' + a, '-lga'], o, fl)
            if rc:
                out('  %-5s %-22s REFUSED: %s' % (L, vn, first_diag(t, 2)))
                continue
            ds = dsyms(o)
            out('  %-5s %-22s %s' % (L, vn, ' '.join('%s=%s' % (n, sdesc(ds.get(n))) for n in
                                                   ('ga_used', 'ga_same_member', 'ga_hidden', 'ga_unpulled', 'lib_entry'))))

    out('=== G %s: libgcc.a members pulled into a -shared output (gcc driver: -shared-libgcc by default for '
        '-shared? compare with -static-libgcc); helper symbols looked up in .dynsym and .symtab' % a)
    o_obj = a + '/libgcc_user.o'
    cc(a, 'gcc', 'g_libgcc.c', o_obj, ['-fPIC', '-O2'] + GFLAGS[a])
    undef = sorted(set(sy for sec, off, t, sy in relocs(os.path.join(work, o_obj)) if sy.startswith('__')))
    out('  object references (relocation symbols starting with __): %s' % undef)
    for L in LINKERS:
        for vn, fl in (('default', []), ('-static-libgcc', ['-static-libgcc'])):
            o = '%s/lg.%s.%s.so' % (a, vn.strip('-').replace('-', '_'), L)
            rc, t = link(a, L, [o_obj], o, fl)
            if rc:
                out('  %-5s %-15s REFUSED: %s' % (L, vn, first_diag(t, 2)))
                continue
            ds = dsyms(o)
            rs, st = sh(['readelf', '-W', '-s', o])
            symtab = {}
            for l in st.splitlines():
                f = l.split()
                if len(f) >= 8 and f[-1] in HELPERS and f[6] != 'UND':
                    symtab[f[-1]] = '%s/%s' % (f[4], f[5])
            needed = [v for t_, v in Elf(os.path.join(work, o)).dynamic() if t_ == 1]
            rn, dn = sh(['readelf', '-W', '-d', o])
            nlibs = [l.split('[')[1].rstrip(']') for l in dn.splitlines() if '(NEEDED)' in l]
            out('  %-5s %-15s NEEDED=%s dynsym: %s | symtab-defs: %s' % (
                L, vn, nlibs, ' '.join('%s=%s' % (n, sdesc(ds.get(n))) for n in HELPERS if n in ds or n in symtab or n in undef) or '-',
                symtab or '-'))

    out('=== G %s: dup_f/dup_w(weak)/dup_v defined in the output libout.so AND in its needed input libdep2.so '
        '(link: g_out.o -ldep2); host runs with two NEEDED orders' % a)
    os.makedirs(os.path.join(work, a, 'dep2'), exist_ok=True)
    cc(a, 'gcc', 'g_dep2.c', a + '/dep2/dep2.o', ['-fPIC'])
    link(a, 'bfd', [a + '/dep2/dep2.o'], a + '/dep2/libdep2.so', ['-Wl,-soname,libdep2.so'])
    cc(a, 'gcc', 'g_out.c', a + '/out.o', ['-fPIC', '-O2'])
    cc(a, 'gcc', 'g_host.c', a + '/ghost.o', [])
    for L in LINKERS:
        for vn, fl in (('default', []), ('-Bsymbolic', ['-Wl,-Bsymbolic'])):
            d = '%s/out.%s.%s' % (a, vn.strip('-'), L)
            os.makedirs(os.path.join(work, d), exist_ok=True)
            rc, t = link(a, L, [a + '/out.o', '-L' + a + '/dep2', '-Wl,--no-as-needed', '-ldep2'], d + '/libout.so',
                         ['-Wl,-soname,libout.so'] + fl)
            if rc:
                out('  %-5s %-10s REFUSED: %s' % (L, vn, first_diag(t, 2)))
                continue
            ds = dsyms(d + '/libout.so')
            rl = relocs(os.path.join(work, d + '/libout.so'))
            recs = ' '.join('%s:%s' % (short(t_), s.split('@')[0]) for sec, off, t_, s in rl
                            if s.split('@')[0] in ('dup_f', 'dup_w', 'dup_v')) or '-'
            w = (' DIAG: ' + first_diag(t, 2)) if t.strip() else ''
            out('  %-5s %-10s output dynsym: %s | records: %s%s' % (
                L, vn, ' '.join('%s=%s' % (n, sdesc(ds.get(n))) for n in ('dup_f', 'dup_w', 'dup_v')), recs, w))
            for order, libs in (('-lout -ldep2', ['-lout', '-ldep2']), ('-ldep2 -lout', ['-ldep2', '-lout'])):
                ex = '%s/host_%s' % (d, order.replace(' ', '').replace('-l', '_'))
                rc2, t2 = link(a, 'bfd', [a + '/ghost.o', '-L' + d, '-L' + a + '/dep2', '-Wl,--no-as-needed'] + libs,
                               ex, [], shared=False)
                if rc2:
                    out('      host %s: link failed %s' % (order, first_diag(t2, 2)))
                    continue
                r = run(a, ex, [d, a + '/dep2'])[1].strip()
                out('      host NEEDED %-13s %s' % (order, r))
S.close()
