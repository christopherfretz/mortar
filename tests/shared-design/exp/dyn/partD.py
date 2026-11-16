# Part D: IFUNC in a shared object: records, .dynsym type, address equality.
import os, sys, re
exec(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'common.py')).read())
open_log('D.log')

TU1 = r'''
extern int puts(const char *);
static int impl_f(void) { return 1; }
static void *resolve_f(void) { return (void *)impl_f; }
int f_exp(void) __attribute__((ifunc("resolve_f")));                 /* exported IFUNC */
static int impl_g(void) { return 2; }
static void *resolve_g(void) { return (void *)impl_g; }
static int g_loc(void) __attribute__((ifunc("resolve_g")));          /* local IFUNC */
static int impl_h(void) { return 3; }
static void *resolve_h(void) { return (void *)impl_h; }
__attribute__((visibility("hidden"))) int h_hid(void) __attribute__((ifunc("resolve_h")));  /* hidden, used by TU2 */
void *ptr_f = (void *)f_exp;      /* data words */
void *ptr_g = (void *)g_loc;
void *ptr_h = (void *)h_hid;
void *addr_f(void) { return (void *)f_exp; }
void *addr_g(void) { return (void *)g_loc; }
void *addr_h1(void) { return (void *)h_hid; }
int call_f(void) { return f_exp(); }
int call_g(void) { return g_loc(); }
int call_h1(void) { return h_hid(); }
int plain_fn(void) { return 9; }
void *addr_plain(void) { return (void *)plain_fn; }
'''
TU2 = r'''
__attribute__((visibility("hidden"))) int h_hid(void);
void *addr_h2(void) { return (void *)h_hid; }
int call_h2(void) { return h_hid(); }
void *ptr_h2 = (void *)h_hid;
'''
HOST = r'''
#define _GNU_SOURCE
#include <stdio.h>
#include <dlfcn.h>
int f_exp(void); int plain_fn(void);
void *addr_f(void), *addr_g(void), *addr_h1(void), *addr_h2(void), *addr_plain(void);
int call_f(void), call_g(void), call_h1(void), call_h2(void);
extern void *ptr_f, *ptr_g, *ptr_h, *ptr_h2;
static int fails;
#define CK(c, ...) do { if (!(c)) { fails++; printf("FAIL " __VA_ARGS__); printf("\n"); } } while (0)
#ifdef INTERPOSE
int f_exp(void) { return 100; }      /* the executable's definition preempts the library's IFUNC */
#endif
int main(int argc, char **argv) {
  void *exe_f = (void *)f_exp;
  void *h = dlopen(argv[1], RTLD_NOW);
  void *dl_f = dlsym(h, "f_exp"), *dl_def = dlsym(RTLD_DEFAULT, "f_exp");
  printf("&f: exe %p lib %p libdata %p dlsym(handle) %p dlsym(DEFAULT) %p\n", exe_f, addr_f(), ptr_f, dl_f, dl_def);
#ifdef INTERPOSE
  CK(call_f() == 100 && addr_f() == exe_f && ptr_f == exe_f, "interposition: lib's call_f()=%d, lib &f %p data %p, exe &f %p", call_f(), addr_f(), ptr_f, exe_f);
#else
  CK(exe_f == addr_f() && addr_f() == ptr_f && ptr_f == dl_def, "exported IFUNC: exe/lib-code/lib-data/dlsym(DEFAULT) differ");
  CK(dl_f == exe_f, "exported IFUNC: dlsym(handle) %p != exe %p", dl_f, exe_f);
  void *pl_exe = (void *)plain_fn, *pl_dl = dlsym(h, "plain_fn");
  CK(pl_dl == pl_exe && addr_plain() == pl_exe, "control (plain function): dlsym(handle) %p lib %p exe %p", pl_dl, addr_plain(), pl_exe);
  CK(f_exp() == 1 && call_f() == 1 && ((int (*)(void))dl_f)() == 1, "f calls");
#endif
  CK(addr_g() == ptr_g, "local IFUNC: code %p data %p", addr_g(), ptr_g);
  CK(call_g() == 2 && ((int (*)(void))ptr_g)() == 2, "g calls");
#ifndef NO_TU2
  CK(addr_h1() == addr_h2() && addr_h2() == ptr_h && ptr_h == ptr_h2, "hidden IFUNC across TUs: tu1 %p tu2 %p data %p data2 %p", addr_h1(), addr_h2(), ptr_h, ptr_h2);
  CK(call_h1() == 3 && call_h2() == 3 && ((int (*)(void))ptr_h)() == 3, "h calls");
#else
  CK(addr_h1() == ptr_h && call_h1() == 3, "hidden IFUNC (one TU): code %p data %p", addr_h1(), ptr_h);
#endif
  printf("%s (%d failures)\n", fails ? "FAIL" : "OK", fails);
  return fails != 0;
}
'''

def one(arch, lk, tag, objs, zn, hostdefs):
    d = adir(arch, 'ifunc', lk)
    so = os.path.join(d, f'lib{tag}.so')
    rc, diag = link(arch, lk, so, objs, ['-Wl,-soname,lib%s.so' % tag, *zn])
    log(f"\n== {arch} {lk} {tag}")
    if rc:
        log(f"   LINK REFUSED: {first_line(diag)}"); return
    dump(so, so + '.readelf')
    ds = {s['name']: s for s in dynsyms(so)}
    st = {s['name']: s for s in symtab(so)}
    osabi = re.search(r'OS/ABI:\s+(.*)', readelf(so, ['-h'])).group(1)
    log(f"   OS/ABI={osabi}; .dynsym f_exp: type={ds.get('f_exp', {}).get('type')} value={ds.get('f_exp', {}).get('value', 0):#x}; .symtab f_exp={st.get('f_exp', {}).get('type')}@{st.get('f_exp', {}).get('value', 0):#x} g_loc={st.get('g_loc', {}).get('type')}@{st.get('g_loc', {}).get('value', 0):#x} h_hid={st.get('h_hid', {}).get('type')}/{st.get('h_hid', {}).get('vis')}@{st.get('h_hid', {}).get('value', 0):#x} resolve_f={st.get('resolve_f', {}).get('value', 0):#x} resolve_g={st.get('resolve_g', {}).get('value', 0):#x} resolve_h={st.get('resolve_h', {}).get('value', 0):#x}")
    for sec, rs in relocs(so).items():
        interesting = [r for r in rs if 'IRELATIVE' in r['type'] or 'f_exp' in r['sym'] or 'JUMP_SLOT' in r['type']]
        if interesting:
            log(f"   {sec}: " + ', '.join(f"{r['type']}@{r['off']:#x}[{r['sym']}]" for r in interesting))
    secs = {s['name']: s for s in sections(so)}
    log(f"   sections: " + ' '.join(f"{n}@{s['addr']:#x}+{s['size']:#x}" for n, s in secs.items() if n in ('.plt', '.iplt', '.plt.got', '.got', '.got.plt', '.data', '.data.rel.ro', '.data.rel')))
    for pie, pf, extra in (('pie', [], []), ('nopie', ['-fno-pie'], ['-no-pie']), ('pie_interpose', ['-DINTERPOSE'], ['-rdynamic'])):
        exe = os.path.join(d, f'h_{tag}_{pie}')
        oh, _ = compile_src(arch, 'ifhost' + pie, HOST, ['-O1', *pf, *hostdefs])
        rc, dg = link(arch, 'bfd', exe, [oh, so, '-Wl,-rpath,' + d, *extra], shared=False)
        if rc:
            log(f"   host {pie} link FAILED: {first_line(dg)}"); continue
        er = relocs(exe)
        fr = [f"{r['type']}[{r['sym']}]" for rs in er.values() for r in rs if 'f_exp' in r['sym']]
        log(f"   host {pie} records for f_exp: {fr}")
        rc, out, err = run(arch, exe, [so])
        log(f"   RUN host {pie} rc={rc}: " + ' / '.join((out + err).splitlines()))

for arch in ARCHES:
    o1, d1 = compile_src(arch, 'ifunc1', TU1, ['-fPIC'])
    o2, d2 = compile_src(arch, 'ifunc2', TU2, ['-fPIC'])
    if not o1 or not o2:
        log(f"== {arch}: COMPILE FAILED {d1} {d2}"); continue
    for lk in LK4:
        for zn in ([], ['-Wl,-z,now']):
            one(arch, lk, 'ifunc' + ('_now' if zn else ''), [o1, o2], zn, [])
        one(arch, lk, 'ifunc_tu1', [o1], [], ['-DNO_TU2'])
