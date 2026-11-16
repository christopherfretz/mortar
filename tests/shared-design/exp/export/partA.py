# Part A: which definitions enter .dynsym of a -shared output; -z defs; .dynsym order / .gnu.hash.
import sys, os, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *

work = init(sys.argv[1], 'A')
S = open(os.path.join(work, 'summary.txt'), 'w')
def out(s=''):
    S.write(s + '\n'); S.flush()

write('a_main.c', r'''
int def_f(void){return 1;}
__attribute__((visibility("hidden"))) int hid_f(void){return 2;}
__attribute__((visibility("internal"))) int int_f(void){return 3;}
__attribute__((visibility("protected"))) int prot_f(void){return 4;}
__attribute__((visibility("default"))) int expl_def_f(void){return 5;}
static int stat_f(void){return 6;}
__attribute__((weak)) int weak_f(void){return 7;}
int def_v = 1;
__attribute__((visibility("hidden"))) int hid_v = 2;
__attribute__((visibility("protected"))) int prot_v = 3;
static int stat_v = 4;
__attribute__((weak)) int weak_v = 5;
int com_v;
__thread int tls_v = 9;
__attribute__((visibility("hidden"))) __thread int tls_hid = 9;
static int ifn_impl(void){return 8;}
static void *ifn_res(void){return (void*)ifn_impl;}
int ifn(void) __attribute__((ifunc("ifn_res")));
extern int ar_f(void);
extern int imp_f(void);
extern int imp_v;
extern int imp_unused(void);
__attribute__((weak)) extern int wk_call(void);
__attribute__((weak)) extern int wk_got;
__attribute__((weak)) extern int wk_word;
__attribute__((weak)) extern int wk_unused;
__attribute__((weak)) extern int wk_callonly(void);
void *wk_word_p = &wk_word;
int use(void){
  int r = stat_f() + hid_f() + int_f() + stat_v + hid_v + ar_f() + imp_f() + imp_v + tls_v + tls_hid + com_v;
  if (wk_call) r += wk_call();
  if (&wk_got) r += wk_got;
  return r;
}
int never(void){ return wk_callonly(); }
''')
write('a_cxx.cc', r'''
struct B { virtual ~B(){} virtual int g(){return 1;} };
inline int inl(int x){ static int cnt; return x + ++cnt; }
template<class T> struct S { static int sv; static T get(){ return T(sv); } };
template<class T> int S<T>::sv = 7;
int cxx_use(){ B *b = new B; int r=b->g(); delete b; return r + inl(1) + S<int>::get() + (int)S<long>::get(); }
''')
write('m1.c', 'int ar_f(void){return 10;} int ar_g(void){return 11;} int ar_v = 12;\n')
write('m2.c', 'int ar_unref(void){return 13;}\n')
write('dep.c', 'int imp_f(void){return 20;} int imp_v = 21; int imp_unused(void){return 22;}\n')
write('host_a.c', r'''
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdio.h>
#include <string.h>
int main(int argc, char **argv){
  void *h = dlopen(argv[1], RTLD_NOW|RTLD_LOCAL);
  if(!h){ printf("DLOPEN-FAIL %s\n", dlerror()); return 1; }
  for(int i=2;i<argc;i++){
    void *p = dlsym(h, argv[i]); Dl_info di;
    const char *who = "-";
    if(p && dladdr(p,&di) && di.dli_fname){ who = strrchr(di.dli_fname,'/'); who = who? who+1 : di.dli_fname; }
    printf("%s=%s ", argv[i], p? who : "-");
  }
  printf("\n"); return 0;
}
''')
write('vs_global.map', 'V1 { global: def_f; def_v; ar_f; local: *; };\n')
write('vs_anon.map', '{ global: def_f; def_v; local: *; };\n')
write('dl.list', '{ def_f; };\n')

PROBE = ['def_f', 'hid_f', 'int_f', 'prot_f', 'expl_def_f', 'weak_f', 'def_v', 'hid_v', 'prot_v', 'weak_v',
         'com_v', 'tls_v', 'ifn', 'ar_f', 'ar_g', 'ar_v', 'ar_unref', 'use', 'cxx_use', '_Z3inli', '_ZZ3inliE3cnt',
         '_ZN1SIiE2svE', '_ZN1SIiE3getEv', '_ZTV1B', '_ZTI1B', '_ZTS1B', '_end', '_edata', '__bss_start',
         '_DYNAMIC', '_GLOBAL_OFFSET_TABLE_', '__dso_handle', '_init', '_fini', '__TMC_END__']

VARIANTS = [('default', 'o', []), ('vishidden', 'h', []), ('exclALL', 'o', ['-Wl,--exclude-libs,ALL']),
            ('exclar', 'o', ['-Wl,--exclude-libs,libar.a']), ('exclother', 'o', ['-Wl,--exclude-libs,libzz.a']),
            ('vs_global', 'o', ['-Wl,--version-script=vs_global.map']),
            ('vs_anon', 'o', ['-Wl,--version-script=vs_anon.map']),
            ('dynlist', 'o', ['-Wl,--dynamic-list=dl.list']), ('exportdyn', 'o', ['-Wl,--export-dynamic'])]

results = {}
for arch in ARCHES:
    a = arch
    os.makedirs(os.path.join(work, a, 'dep'), exist_ok=True)
    cc(a, 'gcc', 'a_main.c', a + '/a_main.o', ['-fPIC', '-O1', '-fcommon'])
    cc(a, 'gcc', 'a_main.c', a + '/a_main_h.o', ['-fPIC', '-O1', '-fcommon', '-fvisibility=hidden'])
    cc(a, 'gcc', 'a_cxx.cc', a + '/a_cxx.o', ['-fPIC', '-O0'])
    cc(a, 'gcc', 'a_cxx.cc', a + '/a_cxx_h.o', ['-fPIC', '-O0', '-fvisibility=hidden'])
    cc(a, 'gcc', 'm1.c', a + '/m1.o', ['-fPIC'])
    cc(a, 'gcc', 'm2.c', a + '/m2.o', ['-fPIC'])
    sh(['ar', 'rcs', a + '/libar.a', a + '/m1.o', a + '/m2.o'])
    cc(a, 'gcc', 'dep.c', a + '/dep.o', ['-fPIC'])
    link(a, 'bfd', [a + '/dep.o'], a + '/dep/libdep.so', ['-Wl,-soname,libdep.so'])
    sh([GCC[a], 'host_a.c', '-o', a + '/host_a'])
    for vname, objs, fl in VARIANTS:
        for L in LINKERS:
            if L == 'lld19' and vname not in ('default', 'exclALL', 'exclar', 'vs_global', 'dynlist', 'exportdyn'):
                continue
            o = '%s/%s.%s.so' % (a, vname, L)
            suf = '_h' if objs == 'h' else ''
            rc, txt = link(a, L, [a + '/a_main%s.o' % suf, a + '/a_cxx%s.o' % suf, '-L' + a, '-lar',
                                  '-L' + a + '/dep', '-ldep'], o, fl, cxx=True)
            r = dict(rc=rc, diag=first_diag(txt))
            if rc == 0:
                e = Elf(os.path.join(work, o))
                ds = e.dynsym()
                r['dynsym'] = [symdesc(s) for s in ds[1:]]
                r['osabi'] = e.osabi
                rc2, rt = run(a, a + '/host_a', [a + '/dep'], args=[os.path.join(work, o)] + PROBE)
                r['dlsym'] = rt.strip()
            results[(a, vname, L)] = r

json.dump({'|'.join(k): v for k, v in results.items()}, open(os.path.join(work, 'results.json'), 'w'), indent=1)

# Table: per arch, variant: for every name in the union, each linker's entry.
for a in ARCHES:
    for vname, _, _ in VARIANTS:
        out('=== A %s %s' % (a, vname))
        ents = {}
        for L in LINKERS:
            r = results.get((a, vname, L))
            if r is None:
                continue
            if r['rc'] != 0:
                out('  %s: LINK FAILED: %s' % (L, r['diag']))
                continue
            if r['diag']:
                out('  %s: diag: %s' % (L, r['diag']))
            for d in r['dynsym']:
                n, rest = d.split(':', 1)
                ents.setdefault(n, {})[L] = rest
        for n in sorted(ents):
            row = ents[n]
            vals = set(row.values())
            present = [L for L in LINKERS if (a, vname, L) in results and results[(a, vname, L)]['rc'] == 0]
            if len(vals) == 1 and all(L in row for L in present):
                out('  %-28s all: %s' % (n, vals.pop()))
            else:
                out('  %-28s ' % n + '  '.join('%s=%s' % (L, row.get(L, 'ABSENT')) for L in present))
        for L in LINKERS:
            r = results.get((a, vname, L))
            if r and r['rc'] == 0:
                out('  RUN %s: %s' % (L, r['dlsym']))

out('\n=== A-cmp (each variant\'s .dynsym name set vs the default variant, same arch and linker)')
for a in ARCHES:
    for vname, _, _ in VARIANTS[1:]:
        for L in LINKERS:
            r = results.get((a, vname, L)); r0 = results.get((a, 'default', L))
            if not r or r['rc'] or not r0:
                continue
            s1 = set(d.split(':')[0] for d in r['dynsym']); s0 = set(d.split(':')[0] for d in r0['dynsym'])
            out('  %s %-10s %-5s removed=%s added=%s' % (a, vname, L, sorted(s0 - s1), sorted(s1 - s0)))

# ------------------------------------------------------------------ -z defs / --no-undefined
write('z_nosuch.c', 'extern int nosuch(void); int z(void){return nosuch();}\n')
write('z_imp.c', 'extern int imp_f(void); int z(void){return imp_f();}\n')
write('z_ind.c', 'extern int ind_f(void); int z(void){return ind_f();}\n')
write('z_weak.c', '__attribute__((weak)) extern int wk(void); int z(void){return wk? wk():0;}\n')
write('z_ver.c', '__asm__(".symver vf_ref,vf@VB"); extern int vf_ref(void); int z(void){return vf_ref();}\n')
write('z_vok.c', '__asm__(".symver vf_ref,vf@VA"); extern int vf_ref(void); int z(void){return vf_ref();}\n')
write('z_data.c', 'extern int nosuch_v; int z(void){return nosuch_v;}\n')
write('ind.c', 'int ind_f(void){return 1;}\n')
write('depi.c', 'extern int ind_f(void); int depi(void){return ind_f();}\n')
write('ver.c', 'int vf(void){return 1;}\n')
write('ver.map', 'VA { global: vf; local: *; };\n')
write('zhost.c', '#include <dlfcn.h>\n#include <stdio.h>\nint main(int c,char**v){void*h=dlopen(v[1],RTLD_NOW);'
                 'if(!h){printf("dlopen FAILED: %s\\n",dlerror());return 1;}int(*z)(void)=(int(*)(void))dlsym(h,"z");'
                 'printf("z()=%d\\n",z());return 0;}\n')
out('\n=== A-zdefs (-z defs / --no-undefined in -shared)')
for a in ARCHES:
    d = a + '/z'
    os.makedirs(os.path.join(work, d), exist_ok=True)
    cc(a, 'gcc', 'ind.c', d + '/ind.o', ['-fPIC'])
    link(a, 'bfd', [d + '/ind.o'], d + '/libind.so', ['-Wl,-soname,libind.so'])
    cc(a, 'gcc', 'depi.c', d + '/depi.o', ['-fPIC'])
    link(a, 'bfd', [d + '/depi.o', '-L' + d, '-lind'], d + '/libdepi.so', ['-Wl,-soname,libdepi.so'])
    cc(a, 'gcc', 'ver.c', d + '/ver.o', ['-fPIC'])
    link(a, 'bfd', [d + '/ver.o'], d + '/libver.so', ['-Wl,-soname,libver.so', '-Wl,--version-script=ver.map'])
    cc(a, 'gcc', 'dep.c', d + '/dep.o', ['-fPIC'])
    link(a, 'bfd', [d + '/dep.o'], d + '/libdep.so', ['-Wl,-soname,libdep.so'])
    cc(a, 'gcc', 'zhost.c', d + '/zhost.o', [])
    link(a, 'bfd', [d + '/zhost.o', '-ldl'], d + '/zhost', [], shared=False)
    for case, extra in [('nosuch', []), ('data', []), ('imp', ['-ldep']), ('ind', ['-ldepi']), ('weak', []),
                        ('ver', ['-lver']), ('vok', ['-lver'])]:
        cc(a, 'gcc', 'z_%s.c' % case, d + '/z_%s.o' % case, ['-fPIC'])
        for flag in ['', '-Wl,-z,defs', '-Wl,--no-undefined']:
            if case not in ('nosuch',) and flag == '-Wl,--no-undefined':
                continue
            for L in LINKERS:
                o = '%s/z_%s%s.%s.so' % (d, case, flag.replace('-Wl,', '').replace(',', '').replace('-', ''), L)
                rc, txt = link(a, L, [d + '/z_%s.o' % case, '-L' + d] + extra, o, [flag] if flag else [])
                dd = ''
                if rc == 0:
                    e = Elf(os.path.join(work, o))
                    und = [symdesc(s) for s in e.dynsym() if s['shndx'] == 0 and s['name'] in
                           ('nosuch', 'imp_f', 'ind_f', 'wk', 'vf', 'nosuch_v')]
                    dd = ' und=' + ','.join(und)
                out('  %s %-6s %-18s %-5s rc=%d %s%s' % (a, case, flag or '(none)', L, rc, first_diag(txt, 4), dd))
                if rc == 0 and case in ('ver', 'vok'):
                    # load the output (RTLD_NOW) from a host and call z(): proves whether the import still binds
                    rn, dn = sh(['readelf', '-W', '-d', '--dyn-syms', o])
                    need = [l.split('[')[1].rstrip(']') for l in dn.splitlines() if '(NEEDED)' in l]
                    vfl = [' '.join(l.split()[-3:]) for l in dn.splitlines() if l.split() and l.split()[-1].startswith('vf')]
                    r2 = run(a, d + '/zhost', [d], args=[os.path.join(work, o)])[1].strip()
                    out('      NEEDED=%s vf-entry=%s | dlopen+call: %s' % (need, vfl, first_diag(r2, 2)))

# ------------------------------------------------------------------ .dynsym order and .gnu.hash
out('\n=== A-hash (.dynsym order and .gnu.hash of the default variant)')
for a in ARCHES:
    for L in LINKERS:
        o = os.path.join(work, '%s/default.%s.so' % (a, L))
        if not os.path.exists(o):
            continue
        e = Elf(o)
        ds = e.dynsym()
        gh = e.gnu_hash()
        hasold = e.has_sec('.hash')
        firstdef = next((i for i, s in enumerate(ds) if i and s['shndx'] != 0), len(ds))
        undafter = [s['name'] for i, s in enumerate(ds) if i >= gh['symoffset'] and s['shndx'] == 0]
        defbefore = [s['name'] for i, s in enumerate(ds) if 0 < i < gh['symoffset'] and s['shndx'] != 0]
        n = len(ds) - gh['symoffset']
        buckets = [gnu_hash_fn(s['name']) % gh['nbuckets'] for s in ds[gh['symoffset']:]]
        sorted_ok = buckets == sorted(buckets)
        W = 64 if e.c64 else 32
        mw = 1
        while mw <= (12 * n) // W:
            mw *= 2
        lldf = (max(1, n // 4), mw, 26)
        # tie order inside a bucket: by what?
        out('  %s %-5s dynsym=%d symoffset=%d firstdef=%d hashed=%d nbuckets=%d maskwords=%d shift=%d entsize=%d '
            '.hash=%s sorted_by_bucket=%s lldformula(nb,mw,shift)=%s match=%s UNDhashed=%s DEFunhashed=%s' % (
                a, L, len(ds), gh['symoffset'], firstdef, n, gh['nbuckets'], gh['maskwords'], gh['shift'],
                gh['entsize'], hasold, sorted_ok, lldf, lldf == (gh['nbuckets'], gh['maskwords'], gh['shift']),
                undafter, defbefore))
        out('      order: ' + ' '.join(s['name'] for s in ds[1:]))
S.close()
