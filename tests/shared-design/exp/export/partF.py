# Part F: versions WRITTEN by a -shared output (.gnu.version_d, .gnu.version), version-script matching rules,
# .symver, base entry, verneed of a shared output; runs a host bound to foo@V1 and one bound to foo@@V2.
import sys, os, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *

work = init(sys.argv[1], 'F')
S = open(os.path.join(work, 'summary.txt'), 'w')
def out(s=''):
    S.write(s + '\n'); S.flush()

write('f_lib.c', r'''
#include <stdio.h>
int foo(void){return 1;} int foobar(void){return 2;} int fob(void){return 3;} int fo(void){return 4;}
int a(void){return 5;} int b(void){return 6;} int c(void){return 7;} int other(void){return 8;}
int cfun(void){return 9;}
int say(void){ return printf("x\n"); }
''')
write('f_cxx.cc', r'''
namespace ns { int f(){return 1;} int g(int x){return x;} int h(){return 3;} }
int baz(){return 4;}
''')
NAMES = ['foo', 'foobar', 'fob', 'fo', 'a', 'b', 'c', 'other', 'cfun', 'say', '_ZN2ns1fEv', '_ZN2ns1gEi',
         '_ZN2ns1hEv', '_Z3bazv']
SCRIPTS = {
    'single': 'V1 { global: a; b; local: *; };',
    'inherit': 'V1 { global: a; local: *; }; V2 { global: b; } V1;',
    'multi3': 'V1 { global: a; local: *; }; V2 { global: b; } V1; V3 { global: c; } V2;',
    'twoparents': 'V1 { global: a; local: *; }; V2 { global: b; }; V3 { global: c; } V1 V2;',
    'r1_exact_beats_wild': 'V1 { global: f*; }; V2 { global: foo; };',
    'r2a_lastwild': 'V1 { global: foo*; }; V2 { global: fo*; };',
    'r2b_lastwild': 'V1 { global: fo*; }; V2 { global: foo*; };',
    'r3_global_beats_local': 'V1 { global: fo*; local: foo*; };',
    'r3b_local_exact_vs_global_wild': 'V1 { global: fo*; local: foo; };',
    'r4a_exact_local_later': 'V1 { global: *; }; V2 { local: foo; };',
    'r4b_wild_local_later': 'V1 { global: foo; }; V2 { local: *; };',
    'r5_exact_twice': 'V1 { global: foo; }; V2 { global: foo; };',
    'r5w_wild_twice': 'V1 { global: fo*; }; V2 { global: fo*; };',
    'r6_star_vs_q': 'V1 { global: fo?; }; V2 { global: fo*; };',
    'localfirst': 'V1 { local: *; global: foo; };',
    'cxx': 'V1 { global: extern "C++" { "ns::f()"; ns::g*; }; extern "C" { cfun; }; local: *; };',
    'cxx_wild_all': 'V1 { global: extern "C++" { *; }; local: *; };',
    'anon': '{ global: a; local: *; };',
    'undefname': 'V1 { global: a; nosuch; local: *; };',
}

def vinfo(path):
    e = Elf(path)
    ds = e.dynsym()
    vs = e.versym()
    vd = e.verdef() or []
    byndx = {d['ndx']: d['names'][0] for d in vd}
    res = {}
    for i, s in enumerate(ds):
        if i == 0 or s['shndx'] == 0:
            continue
        if vs is None:
            res[s['name']] = 'nover'
            continue
        v = vs[i]
        nm = {0: 'LOCAL0', 1: 'GLOBAL1'}.get(v & 0x7fff, byndx.get(v & 0x7fff, '?%d' % v))
        res[s['name']] = ('%s%s' % (nm, '(hidden)' if v & 0x8000 else ''))
    return e, res, vd

def vdesc(e, vd):
    d = dict()
    tags = {}
    for t, v in e.dynamic():
        tags.setdefault(DT.get(t, hex(t)), []).append(v)
    parts = []
    for x in vd:
        hok = 'ok' if x['hash'] == elf_hash(x['names'][0]) else 'BAD(%#x)' % x['hash']
        parts.append('{ndx=%d flags=%#x cnt=%d hash=%s name=%s parents=%s}' % (
            x['ndx'], x['flags'], x['cnt'], hok, x['names'][0], x['names'][1:]))
    return 'VERDEFNUM=%s verdefs=[%s]' % (tags.get('VERDEFNUM', ['-'])[0], ' '.join(parts))

for a in ARCHES:
    os.makedirs(os.path.join(work, a), exist_ok=True)
    cc(a, 'gcc', 'f_lib.c', a + '/lib.o', ['-fPIC'])
    cc(a, 'gcc', 'f_cxx.cc', a + '/cxx.o', ['-fPIC'])
    out('=== F %s: version scripts (per symbol: the version it gets; LOCAL = not in .dynsym)' % a)
    for sn, txt in SCRIPTS.items():
        write('%s.map' % sn, txt + '\n')
        out('--- %s: %s' % (sn, txt))
        flagsets = [('', [])]
        if sn == 'undefname':
            flagsets = [('', []), ('--undefined-version', ['-Wl,--undefined-version']),
                        ('--no-undefined-version', ['-Wl,--no-undefined-version'])]
        for fsn, fs in flagsets:
            for L in LINKERS:
                if L == 'lld19' and a != 'x64':
                    continue
                o = '%s/%s%s.%s.so' % (a, sn, fsn.replace('-', ''), L)
                rc, t = link(a, L, [a + '/lib.o', a + '/cxx.o'], o,
                             ['-Wl,--version-script=%s.map' % sn, '-Wl,-soname,lib%s.so' % sn] + fs, cxx=True)
                dg = first_diag(t, 2)
                if rc:
                    out('  %-5s %s REFUSED: %s' % (L, fsn, dg))
                    continue
                e, res, vd = vinfo(os.path.join(work, o))
                row = ' '.join('%s=%s' % (n, res.get(n, 'LOCAL')) for n in NAMES)
                out('  %-5s %s %s%s' % (L, fsn, row, (' DIAG: ' + dg) if dg else ''))
                out('  %-5s %s   %s' % ('', fsn, vdesc(e, vd)))

    # ---- .symver
    out('=== F %s: .symver in an object' % a)
    write('f_sym.c', r'''
int foo_v1(void){return 1;}
int foo_v2(void){return 2;}
__asm__(".symver foo_v1,foo@V1");
__asm__(".symver foo_v2,foo@@V2");
int other(void){return 8;} int other2(void){return 9;}
''')
    write('f_sym2.c', 'int foo_v2(void){return 2;}\n__asm__(".symver foo_v2,foo@@V2");\n')
    write('f_old.c', 'int foo(void){return 1;}\n')
    write('sym.map', 'V1 { global: other; local: *; }; V2 { global: other2; } V1;\n')
    write('old.map', 'V1 { global: foo; local: *; };\n')
    write('symfoo.map', 'V1 { global: other; foo; local: *; }; V2 { global: other2; } V1;\n')
    write('f_host.c', '#include <stdio.h>\nint foo(void);\nint main(){ printf("foo()=%d\\n", foo()); return 0; }\n')
    cc(a, 'gcc', 'f_sym.c', a + '/sym.o', ['-fPIC'])
    cc(a, 'gcc', 'f_sym2.c', a + '/sym2.o', ['-fPIC'])
    cc(a, 'gcc', 'f_old.c', a + '/old.o', ['-fPIC'])
    os.makedirs(os.path.join(work, a, 'old'), exist_ok=True)
    link(a, 'bfd', [a + '/old.o'], a + '/old/libv.so.1', ['-Wl,-soname,libv.so.1', '-Wl,--version-script=old.map'])
    sh(['ln', '-s', 'libv.so.1', a + '/old/libv.so'])
    sh([GCC[a], 'f_host.c', '-o', a + '/host_old', '-L' + a + '/old', '-lv'])
    out('  host_old binds: %s' % ' '.join(l.split()[-2] + l.split()[-1] for l in sh(['readelf', '-W', '--dyn-syms', a + '/host_old'])[1].splitlines() if ' foo' in l))
    for case, objs, fl in [('symver+script', [a + '/sym.o'], ['-Wl,--version-script=sym.map']),
                           ('symver+script(foo in V1)', [a + '/sym.o'], ['-Wl,--version-script=symfoo.map']),
                           ('symver,noscript', [a + '/sym.o'], []),
                           ('symver@@only,noscript', [a + '/sym2.o'], [])]:
        for L in LINKERS:
            if L == 'lld19' and a != 'x64':
                continue
            d = '%s/%s.%s' % (a, case.replace(',', '_').replace('+', '_').replace('@', 'A').replace('(', '_').replace(')', '').replace(' ', '_'), L)
            os.makedirs(os.path.join(work, d), exist_ok=True)
            rc, t = link(a, L, objs, d + '/libv.so.1', ['-Wl,-soname,libv.so.1'] + fl)
            dg = first_diag(t, 2)
            if rc:
                out('  %-22s %-5s REFUSED: %s' % (case, L, dg))
                continue
            sh(['ln', '-s', 'libv.so.1', d + '/libv.so'])
            e, res, vd = vinfo(os.path.join(work, d + '/libv.so.1'))
            ds = e.dynsym(); vs = e.versym()
            byndx = {x['ndx']: x['names'][0] for x in (vd or [])}
            ents = ['%s@%s%s' % (s['name'], byndx.get(vs[i] & 0x7fff, vs[i] & 0x7fff) if vs else '-',
                                 '(h)' if vs and vs[i] & 0x8000 else '') for i, s in enumerate(ds) if i and s['shndx']]
            out('  %-22s %-5s ok %s dynsym-defs=%s %s' % (case, L, ('DIAG: ' + dg) if dg else '', ents, vdesc(e, vd or [])))
            rv, tv = sh(['readelf', '-W', '-V', d + '/libv.so.1'])
            bad = [x for x in tv.splitlines() if 'arning' in x or 'rror' in x]
            out('  %-22s %-5s    readelf -V: rc=%d %s' % (case, L, rv, ('PROBLEMS: ' + ' | '.join(bad)) if bad else 'no warnings'))
            r_old = run(a, a + '/host_old', [d])[1].strip().replace('\n', ' | ')
            rc2, t2 = sh([GCC[a], 'f_host.c', '-o', d + '/host_new', '-L' + d, '-lv'])
            r_new = run(a, d + '/host_new', [d])[1].strip().replace('\n', ' | ') if rc2 == 0 else 'host link failed: ' + first_diag(t2, 2)
            bnd = ' '.join(l.split()[-2] + l.split()[-1] for l in sh(['readelf', '-W', '--dyn-syms', d + '/host_new'])[1].splitlines() if ' foo' in l) if rc2 == 0 else ''
            out('  %-22s %-5s    run host_old (built vs old, foo@V1 expected 1): %s ; host_new (built vs this, binds %s): %s' % (case, L, r_old, bnd, r_new))

    # ---- base entry naming: soname vs none, output path with a directory
    out('=== F %s: base version entry' % a)
    write('one.map', 'V1 { global: a; local: *; };\n')
    for L in LINKERS:
        if L == 'lld19' and a != 'x64':
            continue
        for sonm in ('soname', 'nosoname'):
            o = '%s/base_%s/libb.%s.so' % (a, sonm, L)
            os.makedirs(os.path.join(work, a, 'base_' + sonm), exist_ok=True)
            fl = ['-Wl,--version-script=one.map'] + (['-Wl,-soname,libbase.so.7'] if sonm == 'soname' else [])
            rc, t = link(a, L, [a + '/lib.o'], o, fl)
            if rc:
                out('  %-5s %-8s REFUSED %s' % (L, sonm, first_diag(t)))
                continue
            e, res, vd = vinfo(os.path.join(work, o))
            out('  %-5s %-8s (output path %s) %s' % (L, sonm, o, vdesc(e, vd)))

    # ---- verneed of a shared output (printf from libc; the weak __cxa_finalize)
    out('=== F %s: .gnu.version_r of a shared output (lib.o calls printf)' % a)
    for L in LINKERS:
        if L == 'lld19' and a != 'x64':
            continue
        o = '%s/vr.%s.so' % (a, L)
        rc, t = link(a, L, [a + '/lib.o'], o, [])
        e = Elf(os.path.join(work, o))
        vn = e.verneed()
        ds = e.dynsym(); vs = e.versym()
        imp = ['%s:%s%s' % (s['name'], vs[i] & 0x7fff if vs else '-', '' if s['bind'] == 1 else '(weak)')
               for i, s in enumerate(ds) if i and s['shndx'] == 0]
        needed = [v for t_, v in e.dynamic() if t_ == 1]
        out('  %-5s NEEDED=%d verneed=%s imports(versym index)=%s' % (L, len(needed), vn, imp))
S.close()
