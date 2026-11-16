# Part B: .dynamic of a -shared output: tag order, presence per option, loader effects.
import os, sys
exec(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'common.py')).read())
open_log('B.log')

LIB = r'''
#include <stdio.h>
int lib_data = 7;
int *lib_ptr = &lib_data;
static int counter;
__attribute__((constructor)) static void c(void) { }
int bump(void) { return ++counter; }
int g(void) { return 1; }              /* exported, may be preempted */
int callg(void) { return g(); }
int f(void) { puts("in f"); return 42; }
void myinit(void) { puts("myinit ran"); }
void myfini(void) { puts("myfini ran"); }
'''
CXX = r'''
#include <iostream>
#include <stdexcept>
#include <string>
struct B { virtual ~B() {} virtual int v() const { return 1; } };
struct D : B { int v() const override { return 2; } };
std::string s = "x";
int cxxf(int x) { try { if (x) throw std::runtime_error("e"); } catch (const std::exception &e) { std::cout << e.what() << "\n"; } B *b = new D; int r = b->v(); delete b; return r; }
'''
TEXTREL = r'''
static int x = 5;
int *const ro_ptr = &x;      /* -fno-pic: in .rodata: a RELATIVE at a read-only place */
int getx(void) { return *ro_ptr; }
'''
MISSING = r'''
extern int missing_fn(void);
int maybe(int c) { return c ? missing_fn() : 11; }
'''
VERS = 'V1 { global: f; bump; local: *; };\nV2 { global: g; } V1;\n'

def show(arch, lk, so, tag):
    log(f"== {arch} {lk} {tag}: {dyn_summary(so)}")
    log(f"     (.dynamic entries, DT_NULLs) = {dyn_counts(so)}")
    vals = {t: v for t, v in dynamic(so) if t in ('FLAGS', 'FLAGS_1', 'SONAME', 'RUNPATH', 'RPATH', 'INIT', 'FINI', 'TEXTREL', 'SYMBOLIC', 'RELACOUNT', 'RELCOUNT', 'VERDEFNUM', 'VERNEEDNUM', 'BIND_NOW')}
    if vals:
        log('     ' + '  '.join(f'{k}={v}' for k, v in vals.items()))
    dump(so, so + '.readelf')

def build(arch, lk, tag, srcs, ldflags=(), show_it=True, cxx=False, quiet=False):
    objs = []
    for name, text, cf, lang in srcs:
        o, d = compile_src(arch, name, text, cf, lang)
        if not o:
            log(f"== {arch} {lk} {tag}: COMPILE FAILED {first_line(d)}"); return None
        objs.append(o)
    so = os.path.join(adir(arch, lk), f'lib{tag}.so')
    rc, diag = link(arch, lk, so, objs, list(ldflags), cxx=cxx)
    if rc:
        log(f"== {arch} {lk} {tag}: LINK REFUSED rc={rc}: {first_line(diag)}")
        return None
    if diag and not quiet:
        log(f"   ({tag} link diag: {first_line(diag)})")
    if show_it:
        show(arch, lk, so, tag)
    return so

def runlog(arch, exe, args, tag, env=None):
    rc, o, e = run(arch, exe, args, env=env)
    log(f"   RUN {tag} rc={rc}: " + ' / '.join((o + e).splitlines()))
    return rc, o + e

C = lambda: [('libB', LIB, ['-fPIC'], 'c')]

HOST_NODELETE = r'''
#include <stdio.h>
#include <dlfcn.h>
int main(int argc, char **argv) {
  void *h = dlopen(argv[1], RTLD_NOW); if (!h) { printf("dlopen FAIL %s\n", dlerror()); return 1; }
  int (*b)(void) = (int (*)(void))dlsym(h, "bump"); b(); b();
  dlclose(h);
  h = dlopen(argv[1], RTLD_NOW | RTLD_NOLOAD);
  printf("after dlclose: RTLD_NOLOAD handle %s\n", h ? "non-null (still mapped)" : "null (unmapped)");
  if (!h) h = dlopen(argv[1], RTLD_NOW);
  b = (int (*)(void))dlsym(h, "bump");
  printf("counter after reopen = %d (3: kept, 1: reloaded)\n", b());
  return 0;
}
'''
HOST_DLOPEN = r'''
#include <stdio.h>
#include <dlfcn.h>
int main(int argc, char **argv) {
  void *h = dlopen(argv[1], argc > 2 ? RTLD_NOW : RTLD_LAZY);
  printf("dlopen(%s) %s %s\n", argc > 2 ? "NOW" : "LAZY", h ? "ok" : "FAILED", h ? "" : dlerror());
  if (h) { int (*m)(int) = (int (*)(int))dlsym(h, "maybe"); if (m) printf("maybe(0)=%d\n", m(0)); }
  return 0;
}
'''
HOST_SYMB = r'''
#include <stdio.h>
int g(void) { return 99; }   /* exported from the executable (-rdynamic) */
int callg(void);
int main(void) { int r = callg(); printf("lib's callg() -> %d (%s)\n", r, r == 99 ? "preempted by exe" : "bound locally"); return 0; }
'''
INITLIB = lambda n: f'''
#include <stdio.h>
__attribute__((constructor)) static void c(void) {{ puts("ctor {n}"); }}
int sym_{n}(void) {{ return 1; }}
int which(void) {{ return {ord(n)}; }}
'''
HOST_ORDER = r'''
#include <stdio.h>
int sym_A(void); int sym_B(void); int which(void);
__attribute__((constructor)) static void c(void) { puts("ctor exe"); }
int main(void) { sym_A(); sym_B(); printf("which() -> %c\n", which()); return 0; }
'''
HOST_INITFINI = r'''
#include <stdio.h>
int f(void);
int main(void) { printf("f=%d\n", f()); return 0; }
'''
HOST_TEXTREL = r'''
#include <stdio.h>
int getx(void);
int main(void) { printf("getx()=%d %s\n", getx(), getx() == 5 ? "OK" : "FAIL"); return 0; }
'''

for arch in ARCHES:
    for lk in LK4:
        log(f"\n######## {arch} {lk}")
        d = adir(arch, lk)
        base = build(arch, lk, 'b_base', C(), ['-Wl,-soname,libb_base.so'])
        build(arch, lk, 'b_cxx', [('libcxx', CXX, ['-fPIC'], 'c++')], ['-Wl,-soname,libb_cxx.so'], cxx=True)
        build(arch, lk, 'b_rpath', C(), ['-Wl,-rpath,/opt/x:$ORIGIN/../lib', '-Wl,-rpath,/opt/y'])
        build(arch, lk, 'b_rpath_old', C(), ['-Wl,-rpath,/opt/x', '-Wl,--disable-new-dtags'])
        build(arch, lk, 'b_rpath_new', C(), ['-Wl,-rpath,/opt/x', '-Wl,--enable-new-dtags'])
        build(arch, lk, 'b_now', C(), ['-Wl,-z,now'])
        build(arch, lk, 'b_nodelete', C(), ['-Wl,-z,nodelete'])
        build(arch, lk, 'b_initfirst', C(), ['-Wl,-z,initfirst'])
        build(arch, lk, 'b_interpose', C(), ['-Wl,-z,interpose'])
        build(arch, lk, 'b_nodlopen', C(), ['-Wl,-z,nodlopen'])
        build(arch, lk, 'b_origin', C(), ['-Wl,-z,origin'])
        build(arch, lk, 'b_bsym', C(), ['-Wl,-Bsymbolic'])
        build(arch, lk, 'b_bsymf', C(), ['-Wl,-Bsymbolic-functions'])
        build(arch, lk, 'b_now_nodelete_origin', C(), ['-Wl,-z,now', '-Wl,-z,nodelete', '-Wl,-z,origin', '-Wl,-Bsymbolic'])
        build(arch, lk, 'b_initfini', C(), ['-Wl,-init=myinit', '-Wl,-fini=myfini'])
        build(arch, lk, 'b_init_undef', C(), ['-Wl,-init=nosuch', '-Wl,-fini=nosuch2'])
        build(arch, lk, 'b_nostdlib', C(), ['-nostdlib'])
        build(arch, lk, 'b_nostdlib_initfini', [('nolib', 'int f(void){return 42;}\nint my_i(void){return 0;}\n', ['-fPIC'], 'c')], ['-nostdlib', '-Wl,-init=my_i'])
        build(arch, lk, 'b_versdef', C(), ['-Wl,--version-script=' + wfile(os.path.join(d, 'v.map'), VERS), '-Wl,-soname,libb_versdef.so'])
        build(arch, lk, 'b_textrel', [('tr', TEXTREL, ['-fno-pic'], 'c')], ['-Wl,-z,notext'])
        build(arch, lk, 'b_textrel_default', [('tr', TEXTREL, ['-fno-pic'], 'c')], [])
        build(arch, lk, 'b_hash_sysv', C(), ['-Wl,--hash-style=sysv'])
        build(arch, lk, 'b_hash_both', C(), ['-Wl,--hash-style=both'])
        build(arch, lk, 'b_nohashflag', C(), ['-Wl,--hash-style=gnu'])
        build(arch, lk, 'b_relr', C(), ['-Wl,-z,pack-relative-relocs'])
        build(arch, lk, 'b_combreloc_no', C(), ['-Wl,-z,nocombreloc'])
        # as-needed: a library unreferenced by the output
        dep = build(arch, lk, 'b_dep', [('dep', 'int depf(void){return 1;}\n', ['-fPIC'], 'c')], ['-Wl,-soname,libb_dep.so'], show_it=False)
        if dep:
            for t, fl in (('b_asneeded_default', []), ('b_asneeded', ['-Wl,--as-needed']), ('b_noasneeded', ['-Wl,--no-as-needed'])):
                so = build(arch, lk, t, C(), [*fl, '-L' + d, '-lb_dep'], show_it=False)
                if so:
                    log(f"== {arch} {lk} {t}: NEEDED={dynval(so, 'NEEDED')}")
        # sysv-hash default? (show GNU_HASH/HASH presence when the driver is bypassed: plain ld -shared)
        o, _ = compile_src(arch, 'libB', LIB, ['-fPIC'])
        raw = os.path.join(d, 'libb_rawld.so')
        ldbin = {'bfd': {'a64': 'ld.bfd', 'x64': 'x86_64-linux-gnu-ld', 'i386': 'i686-linux-gnu-ld'}[arch],
                 'lld22': 'ld.lld-22', 'lld19': 'ld.lld-19', 'mold': 'mold'}[lk]
        em = {'a64': 'aarch64linux', 'x64': 'elf_x86_64', 'i386': 'elf_i386'}[arch]
        if os.path.exists(raw): os.unlink(raw)
        rc, out, err = sh([ldbin, '-m', em, '-shared', o, '-o', raw])
        if rc == 0:
            log(f"== {arch} {lk} b_rawld (plain `ld -shared`, no driver flags): {dyn_summary(raw)}")
        else:
            log(f"== {arch} {lk} b_rawld: rc={rc} {first_line(out + err)}")

        # ---- effects ----
        exe = os.path.join(d, 'h_nodelete')
        host_exe(arch, exe, HOST_NODELETE, name='h_nodelete')
        for t in ('b_base', 'b_nodelete'):
            p = os.path.join(d, f'lib{t}.so')
            if os.path.exists(p): runlog(arch, exe, [p], f'nodelete-effect {t}')
        exe = os.path.join(d, 'h_dlopen')
        host_exe(arch, exe, HOST_DLOPEN, name='h_dlopen')
        for t in ('b_base', 'b_nodlopen'):
            p = os.path.join(d, f'lib{t}.so')
            if os.path.exists(p): runlog(arch, exe, [p], f'nodlopen-effect {t}')
        for t, fl in (('b_lazy_missing', []), ('b_now_missing', ['-Wl,-z,now'])):
            p = build(arch, lk, t, [('miss', MISSING, ['-fPIC'], 'c')], fl, show_it=False)
            if p:
                log(f"   {t}: FLAGS={dynval(p, 'FLAGS')} FLAGS_1={dynval(p, 'FLAGS_1')} rela.plt={[r['type'] for r in relocs(p).get('.rela.plt', relocs(p).get('.rel.plt', []))]}")
                runlog(arch, exe, [p], f'now-effect {t} RTLD_LAZY')
                runlog(arch, exe, [p, 'now'], f'now-effect {t} RTLD_NOW')
        for t in ('b_base', 'b_bsym', 'b_bsymf'):
            p = os.path.join(d, f'lib{t}.so')
            if not os.path.exists(p): continue
            exe = os.path.join(d, f'h_symb_{t}')
            rc, dg = host_exe(arch, exe, HOST_SYMB, [p], ['-rdynamic'], name='h_symb')
            if rc: log('   host link failed ' + first_line(dg)); continue
            runlog(arch, exe, [], f'Bsymbolic-effect {t}')
        # initfirst and interpose: two libs A, B; exe needs A then B.
        for mode in ('plain', 'initfirstB', 'interposeB', 'initfirstA'):
            fa = ['-Wl,-z,initfirst'] if mode == 'initfirstA' else []
            fb = {'plain': [], 'initfirstB': ['-Wl,-z,initfirst'], 'interposeB': ['-Wl,-z,interpose'], 'initfirstA': []}[mode]
            la = build(arch, lk, f'oA_{mode}', [('oA', INITLIB('A'), ['-fPIC'], 'c')], [*fa, f'-Wl,-soname,liboA_{mode}.so'], show_it=False)
            lb = build(arch, lk, f'oB_{mode}', [('oB', INITLIB('B'), ['-fPIC'], 'c')], [*fb, f'-Wl,-soname,liboB_{mode}.so'], show_it=False)
            exe = os.path.join(d, f'h_order_{mode}')
            rc, dg = host_exe(arch, exe, HOST_ORDER, [la, lb], name='h_order')
            if rc: log('   host link failed ' + first_line(dg)); continue
            log(f"   exe NEEDED={dynval(exe, 'NEEDED')} B FLAGS_1={dynval(lb, 'FLAGS_1')} A FLAGS_1={dynval(la, 'FLAGS_1')}")
            runlog(arch, exe, [], f'order-effect {mode}')
            if mode == 'interposeB':
                runlog(arch, exe, [], f'order-effect {mode} with LD_PRELOAD=B', env={'LD_PRELOAD': lb})
        # -init/-fini effect
        for t in ('b_base', 'b_initfini'):
            p = os.path.join(d, f'lib{t}.so')
            if not os.path.exists(p): continue
            exe = os.path.join(d, f'h_if_{t}')
            rc, dg = host_exe(arch, exe, HOST_INITFINI, [p], name='h_if')
            if rc: log('   host link failed ' + first_line(dg)); continue
            syms = {s['name']: s['value'] for s in symtab(p) if s['name'] in ('myinit', 'myfini', '_init', '_fini')}
            log(f"   {t}: INIT={dynval(p, 'INIT')} FINI={dynval(p, 'FINI')} syms={ {k: hex(v) for k, v in syms.items()} }")
            runlog(arch, exe, [], f'initfini-effect {t}')
        for t in ('b_textrel', 'b_textrel_default'):
            p = os.path.join(d, f'lib{t}.so')
            if not os.path.exists(p): continue
            exe = os.path.join(d, f'h_tr_{t}')
            rc, dg = host_exe(arch, exe, HOST_TEXTREL, [p], name='h_tr')
            if rc: log('   host link failed ' + first_line(dg)); continue
            runlog(arch, exe, [], f'textrel-effect {t}')
