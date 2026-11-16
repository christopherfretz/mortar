# Part B: STV_PROTECTED function and datum defined in a library, address taken in both modules.
import sys, os, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *

work = init(sys.argv[1], 'B')
S = open(os.path.join(work, 'summary.txt'), 'w')
def out(s=''):
    S.write(s + '\n'); S.flush()

write('b_lib.c', r'''
__attribute__((visibility("protected"))) int pf(void){return 7;}
__attribute__((visibility("protected"))) int pv = 11;
void *lib_pf(void){return (void*)pf;}
void *lib_pv(void){return &pv;}
int lib_read_pv(void){return pv;}
''')
write('b_exe.c', r'''
#include <stdio.h>
extern int pf(void); extern int pv;
void *lib_pf(void); void *lib_pv(void); int lib_read_pv(void);
int main(){ void *ef=(void*)pf, *ev=(void*)&pv;
  printf("fn_eq=%d data_eq=%d ", ef==lib_pf(), ev==lib_pv());
  pv = 5; printf("write_seen=%d call=%d\n", lib_read_pv()==5, pf());
  return 0; }
''')

def objrel(o, names):
    return ','.join(sorted(set('%s:%s' % (t.replace('R_AARCH64_', '').replace('R_X86_64_', '').replace('R_386_', ''), s)
                               for sec, off, t, s in relocs(os.path.join(work, o)) if s in names)))

def dynrel(o, names):
    return ','.join('%s:%s' % (t.replace('R_AARCH64_', '').replace('R_X86_64_', '').replace('R_386_', ''), s)
                    for sec, off, t, s in relocs(os.path.join(work, o)) if s in names) or 'none'

LIBV = {'a64': [('gcc', 'gcc', []), ('clang', 'clang22', [])],
        'x64': [('gcc', 'gcc', []), ('clang', 'clang22', []), ('gcc_nodea', 'gcc', ['-mno-direct-extern-access'])],
        'i386': [('gcc', 'gcc', []), ('clang', 'clang22', []), ('gcc_nodea', 'gcc', ['-mno-direct-extern-access'])]}
EXEV = {'a64': [('gcc_pie', 'gcc', ['-fPIE'], []), ('gcc_nopic', 'gcc', ['-fno-pic'], ['-no-pie']),
                ('clang_pie', 'clang22', ['-fPIE'], []), ('clang_nopic', 'clang22', ['-fno-pic'], ['-no-pie']),
                ('clang_pie_dea', 'clang22', ['-fPIE', '-fdirect-access-external-data'], [])],
        'x64': [('gcc_pie', 'gcc', ['-fPIE'], []), ('gcc_nopic', 'gcc', ['-fno-pic'], ['-no-pie']),
                ('gcc_pie_nodea', 'gcc', ['-fPIE', '-mno-direct-extern-access'], []),
                ('clang_pie', 'clang22', ['-fPIE'], []), ('clang_nopic', 'clang22', ['-fno-pic'], ['-no-pie']),
                ('clang_pie_dea', 'clang22', ['-fPIE', '-fdirect-access-external-data'], [])],
        'i386': [('gcc_pie', 'gcc', ['-fPIE'], []), ('gcc_nopic', 'gcc', ['-fno-pic'], ['-no-pie']),
                 ('clang_pie', 'clang22', ['-fPIE'], []), ('clang_nopic', 'clang22', ['-fno-pic'], ['-no-pie'])]}

for a in ARCHES:
    os.makedirs(os.path.join(work, a), exist_ok=True)
    out('=== B %s: object relocations' % a)
    for lv, comp, fl in LIBV[a]:
        cc(a, comp, 'b_lib.c', '%s/lib_%s.o' % (a, lv), ['-fPIC', '-O2'] + fl)
        out('  lib %-10s %s' % (lv, objrel('%s/lib_%s.o' % (a, lv), ('pf', 'pv'))))
    for ev, comp, fl, lf in EXEV[a]:
        cc(a, comp, 'b_exe.c', '%s/exe_%s.o' % (a, ev), ['-O2'] + fl)
        out('  exe %-13s %s' % (ev, objrel('%s/exe_%s.o' % (a, ev), ('pf', 'pv'))))
    out('=== B %s: links and runs' % a)
    lib_variants = [(lv, L, []) for lv, _, _ in LIBV[a] for L in LINKERS]
    if a != 'a64':
        lib_variants.append(('gcc', 'bfd', ['-Wl,-z,indirect-extern-access']))
    libdirs = []
    for lv, L, lfl in lib_variants:
        tag = '%s.%s%s' % (lv, L, '.ziea' if lfl else '')
        d = '%s/%s' % (a, tag)
        os.makedirs(os.path.join(work, d), exist_ok=True)
        rc, txt = link(a, L, ['%s/lib_%s.o' % (a, lv)], d + '/libp.so', ['-Wl,-soname,libp.so'] + lfl)
        if rc:
            out('  LIB %-22s REFUSED: %s' % (tag, first_diag(txt, 3)))
            continue
        libdirs.append(d)
        e, tags = dyn_summary(os.path.join(work, d + '/libp.so'))
        rd, ro = sh(['readelf', '-W', '-n', d + '/libp.so'])
        prop = 'NEEDED_INDIRECT_EXTERN_ACCESS' if 'needed: indirect external access' in ro else ''
        allrel = ','.join('%s:%s' % (t.split('_', 2)[-1] if t.startswith('R_X86_64') else t.replace('R_AARCH64_', '').replace('R_386_', ''), sy)
                          for sec, off, t, sy in relocs(os.path.join(work, d + '/libp.so')) if sec != '.rela.plt' and sec != '.rel.plt')
        out('  LIB %-22s ok%s dynrel(pf,pv)=%s all .rel[a].dyn=[%s] %s %s' % (
            tag, (' WARN: ' + first_diag(txt, 2)) if txt.strip() else '', dynrel(d + '/libp.so', ('pf', 'pv')),
            allrel, 'TEXTREL' if 'TEXTREL' in tags else '', prop))
        for ev, comp, fl, lfe in EXEV[a]:
            for XL in (['bfd'] if L == 'bfd' else ['bfd', L]):
                exe = '%s/x_%s.%s' % (d, ev, XL)
                rc, txt = link(a, XL, ['%s/exe_%s.o' % (a, ev), '-L' + d, '-lp'], exe, lfe, shared=False)
                if rc:
                    out('    exe %-13s %-5s REFUSED: %s' % (ev, XL, first_diag(txt, 2).replace(tag + '/', '')))
                    continue
                w = (' WARN: ' + first_diag(txt, 2)) if txt.strip() else ''
                rr, rt = run(a, exe, [d])
                out('    exe %-13s %-5s rc=%d dynrel=%s%s :: %s' % (ev, XL, rr, dynrel(exe, ('pf', 'pv')), w,
                                                                 rt.strip().replace('\n', ' | ')[:700]))
    # glibc's own behaviour: an executable linked against a DEFAULT-visibility stub of libp.so (COPY of pv,
    # canonical PLT of pf), run against each real (protected) libp.so.
    out('=== B %s: glibc load-time behaviour (exe linked by GNU ld against a default-visibility stub)' % a)
    write('b_stub.c', 'int pf(void){return 0;} int pv = 0; void *lib_pf(void){return 0;} void *lib_pv(void){return 0;} int lib_read_pv(void){return 0;}\n')
    os.makedirs(os.path.join(work, a, 'stub'), exist_ok=True)
    cc(a, 'gcc', 'b_stub.c', a + '/stub/stub.o', ['-fPIC'])
    link(a, 'bfd', [a + '/stub/stub.o'], a + '/stub/libp.so', ['-Wl,-soname,libp.so'])
    for ev, comp, fl, lfe in EXEV[a]:
        exe = '%s/stub/x_%s' % (a, ev)
        rc, txt = link(a, 'bfd', ['%s/exe_%s.o' % (a, ev), '-L' + a + '/stub', '-lp'], exe, lfe, shared=False)
        if rc:
            out('  exe %-13s link vs stub REFUSED: %s' % (ev, first_diag(txt, 2)))
            continue
        out('  exe %-13s records vs stub: %s' % (ev, dynrel(exe, ('pf', 'pv'))))
        for d in libdirs:
            rr, rt = run(a, exe, [d])
            out('    with %-22s rc=%d :: %s' % (d.split('/', 1)[1], rr, rt.strip().replace('\n', ' | ')[:700]))
S.close()
