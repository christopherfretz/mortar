# Part C: TLS in a shared object: models, relaxations, records, DF_STATIC_TLS, runs.
import os, sys, re
exec(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'common.py')).read())
open_log('C.log')

LIB = r'''
static __thread int s_tls = 11;
__attribute__((visibility("hidden"))) __thread int h_tls = 22;
__thread int e_tls = 33;
extern __thread int o_tls;      /* defined in libother.so */
extern __thread int x_tls;      /* defined in the executable */
#define ACC(n) int get_##n(void) { return n##_tls; } int *addr_##n(void) { return &n##_tls; } void set_##n(int v) { n##_tls = v; }
ACC(s) ACC(h) ACC(e) ACC(o) ACC(x)
int sum_local(void) { return s_tls + h_tls; }   /* two locals in one function: LD candidate */
'''
OTHER = '__thread int o_tls = 44;\nint *other_addr_o(void) { return &o_tls; }\n'
BIG = 'static __thread char big_tls[%d];\nint big_touch(int i) { big_tls[i] = 1; return big_tls[0] + %d; }\n'

HOST = r'''
#include <stdio.h>
#include <pthread.h>
#include <dlfcn.h>
#include <string.h>
__thread int x_tls = 55;
extern __thread int e_tls;
#ifndef DL
#define D(n) int get_##n(void); int *addr_##n(void); void set_##n(int);
D(s) D(h) D(e) D(o) D(x)
int *other_addr_o(void);
#else
static int (*get_s)(void), (*get_h)(void), (*get_e)(void), (*get_o)(void), (*get_x)(void);
static int *(*addr_s)(void), *(*addr_h)(void), *(*addr_e)(void), *(*addr_o)(void), *(*addr_x)(void);
static void (*set_s)(int), (*set_h)(int), (*set_e)(int), (*set_o)(int), (*set_x)(int);
static int *(*other_addr_o)(void);
static int *(*addr_e_exe)(void);
#endif
static int fails;
#define CHECK(c, ...) do { if (!(c)) { fails++; printf("FAIL " __VA_ARGS__); printf("\n"); } } while (0)
static void *work(void *arg) {
  int k = (int)(long)arg;
  CHECK(get_s() == 11 && get_h() == 22 && get_e() == 33 && get_x() == 55, "initial values thread %d: %d %d %d %d", k, get_s(), get_h(), get_e(), get_x());
  CHECK(get_o() == 44, "initial o thread %d: %d", k, get_o());
  set_s(100 + k); set_h(200 + k); set_e(300 + k); set_o(400 + k); set_x(500 + k);
  CHECK(get_s() == 100 + k && get_h() == 200 + k && get_e() == 300 + k && get_o() == 400 + k && get_x() == 500 + k, "after set thread %d", k);
  CHECK(x_tls == 500 + k, "exe sees x_tls thread %d: %d", k, x_tls);
  CHECK(addr_x() == &x_tls, "&x_tls lib %p exe %p", (void *)addr_x(), (void *)&x_tls);
  CHECK(addr_o() == other_addr_o(), "&o_tls lib %p other %p", (void *)addr_o(), (void *)other_addr_o());
#ifndef DL
  CHECK(addr_e() == &e_tls && e_tls == 300 + k, "&e_tls lib %p exe %p", (void *)addr_e(), (void *)&e_tls);
#endif
  CHECK(addr_s() != addr_h(), "distinct s/h");
  return 0;
}
static pthread_barrier_t bar;
static void *early(void *arg) { pthread_barrier_wait(&bar); pthread_barrier_wait(&bar); return work(arg); }
int main(int argc, char **argv) {
  pthread_t t[5];
  pthread_barrier_init(&bar, 0, 2);
  pthread_create(&t[4], 0, early, (void *)9);   /* a thread that exists before dlopen */
  pthread_barrier_wait(&bar);
#ifdef DL
  void *h = dlopen(argv[1], RTLD_NOW);
  if (!h) { printf("dlopen FAILED: %s\n", dlerror()); pthread_barrier_wait(&bar); pthread_join(t[4], 0); return 2; }
#define L(n) get_##n = dlsym(h, "get_" #n); addr_##n = dlsym(h, "addr_" #n); set_##n = dlsym(h, "set_" #n);
  L(s) L(h) L(e) L(o) L(x)
  other_addr_o = dlsym(h, "other_addr_o");
#endif
  pthread_barrier_wait(&bar);
  for (int i = 0; i < 4; i++) pthread_create(&t[i], 0, work, (void *)(long)i);
  for (int i = 0; i < 5; i++) pthread_join(t[i], 0);
  work((void *)7);
  printf("%s (%d failures)\n", fails ? "FAIL" : "OK", fails);
  return fails != 0;
}
'''
HOST_BIG = r'''
#include <stdio.h>
#include <dlfcn.h>
int main(int argc, char **argv) {
  void *h = dlopen(argv[1], RTLD_NOW);
  if (!h) { printf("dlopen FAILED: %s\n", dlerror()); return 2; }
  int (*b)(int) = dlsym(h, "big_touch");
  printf("dlopen ok, big_touch=%d OK\n", b(1));
  return 0;
}
'''

LIB_LOCAL = r'''
static __thread int s_tls = 11;
__attribute__((visibility("hidden"))) __thread int h_tls = 22;
int sum_local(void) { return s_tls + h_tls; }
void bump(void) { s_tls += 100; }
'''
LEHOST = r'''
#include <stdio.h>
#include <pthread.h>
#include <dlfcn.h>
static int (*sum)(void); static void (*bump)(void);
static void *th(void *a) { int v = sum(); bump(); *(int *)a = (v == 33 && sum() == 133); return 0; }
int main(int argc, char **argv) {
  void *h = dlopen(argv[1], RTLD_NOW);
  if (!h) { printf("dlopen FAILED: %s\n", dlerror()); return 2; }
  sum = (int (*)(void))dlsym(h, "sum_local"); bump = (void (*)(void))dlsym(h, "bump");
  int m = sum(); bump(); int ok_main = (m == 33 && sum() == 133), ok_th = 0;
  pthread_t t; pthread_create(&t, 0, th, &ok_th); pthread_join(t, 0);
  printf("%s main=%d thread=%d (main sum %d)\n", ok_main && ok_th ? "OK" : "FAIL", ok_main, ok_th, m);
  return !(ok_main && ok_th);
}
'''

MODES = {
    'a64':  {'GD': ['-fPIC', '-mtls-dialect=trad'], 'DESC': ['-fPIC', '-mtls-dialect=desc'],
             'IE': ['-fPIC', '-ftls-model=initial-exec'], 'LE': ['-fPIC', '-ftls-model=local-exec']},
    'x64':  {'GD': ['-fPIC'], 'DESC': ['-fPIC', '-mtls-dialect=gnu2'],
             'IE': ['-fPIC', '-ftls-model=initial-exec'], 'LE': ['-fPIC', '-ftls-model=local-exec']},
    'i386': {'GD': ['-fPIC'], 'DESC': ['-fPIC', '-mtls-dialect=gnu2'],
             'IE': ['-fPIC', '-ftls-model=initial-exec'], 'LE': ['-fPIC', '-ftls-model=local-exec']},
}

def objrels(o):
    t = readelf(o, ['-r'])
    rows = []
    for line in t.splitlines():
        if re.match(r'^[0-9a-f]{8,16}\s', line):
            p = line.split(None, 4)
            if 'TLS' in p[2] or 'TPOFF' in p[2] or 'DTP' in p[2] or 'GOTTPOFF' in p[2] or 'TLSGD' in p[2] or 'TLSLD' in p[2] or 'DTPOFF' in p[2] or 'tls_get_addr' in (p[4] if len(p) > 4 else '') or 'TLSDESC' in p[2]:
                sym = (p[4] if len(p) > 4 else '').split()[0] if len(p) > 4 else ''
                rows.append(f"{p[2]}:{sym}")
    return rows

def tlsget_calls(so):
    rc, o, e = sh(['llvm-objdump-22', '-d', '--no-show-raw-insn', so])
    return len(re.findall(r'(call\w*|bl)\s+.*<_+tls_get_addr', o))

def summarize_relocs(so):
    out = []
    for sec, rs in relocs(so).items():
        tl = [r for r in rs if 'TLS' in r['type'] or 'TPOFF' in r['type'] or 'DTP' in r['type']]
        if tl:
            out.append(f"{sec}: " + ', '.join(f"{r['type']}@{r['off']:#x}[{r['sym'] or 'sym 0'}]" for r in tl))
    return out

def rel_addends(arch, so):
    """for REL (i386): the word in place at each TLS record's offset, and the word after it
    (a TLS_DESC keeps its addend in the second word; a DTPMOD32 is followed by its DTPOFF32 slot)."""
    if arch != 'i386':
        return ''
    secs = sections(so)
    data = open(so, 'rb').read()
    res = []
    for sec, rs in relocs(so).items():
        for r in rs:
            if 'TLS' in r['type']:
                for s in secs:
                    if s['type'] != 'NOBITS' and s['addr'] <= r['off'] < s['addr'] + s['size']:
                        fo = s['off'] + r['off'] - s['addr']
                        w0 = int.from_bytes(data[fo:fo+4], 'little'); w1 = int.from_bytes(data[fo+4:fo+8], 'little')
                        res.append(f"{r['type']}@{r['off']:#x}=[{w0:#x},next {w1:#x}]")
    return 'in-place words: ' + ', '.join(res)

for arch in ARCHES:
    d0 = adir(arch, 'tls')
    oo, _ = compile_src(arch, 'other', OTHER, ['-fPIC'])
    other = os.path.join(d0, 'libother.so')
    link(arch, 'bfd', other, [oo], ['-Wl,-soname,libother.so'])
    for mode, cf in MODES[arch].items():
        o, diag = compile_src(arch, f'tls{mode}', LIB, cf)
        if not o:
            log(f"== {arch} {mode}: COMPILE FAILED {first_line(diag)}"); continue
        log(f"\n######## {arch} {mode} cflags={' '.join(cf)}")
        log(f"  object TLS relocs: {' '.join(objrels(o))}")
        for lk in LK4:
            d = adir(arch, 'tls', lk)
            so = os.path.join(d, f'libtls{mode}.so')
            rc, diag = link(arch, lk, so, [o], ['-L' + d0, '-lother', '-Wl,--no-as-needed', '-Wl,-soname,libtls%s.so' % mode])
            if rc:
                log(f"== {arch} {mode} {lk}: LINK REFUSED rc={rc}: {first_line(diag)}")
                for l in diag.splitlines()[1:4]:
                    log(f"     | {l.strip()}")
                continue
            if diag:
                log(f"   ({mode} {lk} link diag: {first_line(diag)})")
            dump(so, so + '.readelf')
            wfile(so + '.dis', sh(['llvm-objdump-22', '-d', so])[1])
            dt = dynamic(so)
            fl = [v for t, v in dt if t in ('FLAGS',)]
            tdesc = [f"{t}={v}" for t, v in dt if 'TLSDESC' in t]
            ptls = [p for p in phdrs(so) if p['type'] == 'TLS']
            ncall = tlsget_calls(so)
            log(f"== {arch} {mode} {lk}: FLAGS={fl} {' '.join(tdesc)} PT_TLS={'memsz=%#x align=%#x' % (ptls[0]['memsz'], ptls[0]['align']) if ptls else 'none'} __tls_get_addr calls in output={ncall}")
            for s in summarize_relocs(so):
                log(f"     {s}")
            if arch == 'i386':
                log(f"     {rel_addends(arch, so)}")
            # run: DT_NEEDED
            exe = os.path.join(d, f'h_need_{mode}')
            rc, dg = host_exe(arch, exe, HOST, [so, other, '-rdynamic'], name='tlshost')
            if rc:
                log(f"   host link FAILED: {first_line(dg)}")
            else:
                rc, out, err = run(arch, exe, [], env={'LD_LIBRARY_PATH': d + ':' + d0})
                log(f"   RUN needed rc={rc}: " + ' / '.join((out + err).splitlines()[-3:]))
            exe2 = os.path.join(d, f'h_dl_{mode}')
            od, _ = compile_src(arch, 'tlshostdl', HOST, ['-O1', '-DDL'])
            rc, dg = link(arch, 'bfd', exe2, [od, '-rdynamic', '-pthread', '-Wl,-rpath,' + d + ':' + d0], shared=False)
            if rc:
                log(f"   dl host link FAILED: {first_line(dg)}")
            else:
                rc, out, err = run(arch, exe2, [so], env={'LD_LIBRARY_PATH': d + ':' + d0})
                log(f"   RUN dlopen rc={rc}: " + ' / '.join((out + err).splitlines()[-3:]))
    # LE against local TLS only (no o_tls/x_tls): is it accepted in -shared, and does it run when dlopened?
    ol, diag = compile_src(arch, 'tlsLElocal', LIB_LOCAL, MODES[arch]['LE'])
    lh = os.path.join(d0, 'h_le_local')
    host_exe(arch, lh, LEHOST, name='h_le_local')
    log(f"\n######## {arch} LE_local (static + hidden TLS only) cflags={' '.join(MODES[arch]['LE'])}")
    log(f"  object TLS relocs: {' '.join(objrels(ol))}")
    for lk in LK4:
        d = adir(arch, 'tls', lk)
        so = os.path.join(d, 'libtlsLElocal.so')
        rc, diag = link(arch, lk, so, [ol], ['-Wl,-soname,libtlsLElocal.so'])
        if rc:
            log(f"== {arch} LE_local {lk}: LINK REFUSED rc={rc}: {first_line(diag)}"); continue
        dump(so, so + '.readelf')
        fl = [v for t, v in dynamic(so) if t == 'FLAGS']
        log(f"== {arch} LE_local {lk}: FLAGS={fl} records={summarize_relocs(so)}" + (f" (diag: {first_line(diag)})" if diag else ''))
        rc, out, err = run(arch, lh, [so])
        log(f"   RUN dlopen rc={rc}: {' / '.join((out + err).splitlines())}")
    # IE library dlopened: static TLS surplus
    for size in (64, 1024, 4096, 65536):
        for lk in LK3:
            d = adir(arch, 'tls', lk)
            o, diag = compile_src(arch, f'big{size}', BIG % (size, size), ['-fPIC', '-ftls-model=initial-exec'])
            so = os.path.join(d, f'libbig{size}.so')
            rc, diag = link(arch, lk, so, [o])
            if rc:
                log(f"== {arch} IE-dlopen size={size} {lk}: LINK REFUSED {first_line(diag)}"); continue
            exe = os.path.join(d, 'h_big')
            if not os.path.exists(exe):
                host_exe(arch, exe, HOST_BIG, name='h_big')
            rc, out, err = run(arch, exe, [so])
            log(f"== {arch} IE-dlopen static TLS {size} bytes {lk}: FLAGS={dynval(so, 'FLAGS')} rc={rc}: {' / '.join((out + err).splitlines())}")
