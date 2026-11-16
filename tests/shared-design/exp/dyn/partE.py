# Part E: COPY never in a .so; non-PIC references; --gc-sections roots; -z defs; .eh_frame_hdr;
# section headers vs D-34; -static-libgcc into a .so.
import os, sys, glob as _g
exec(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'common.py')).read())
open_log('E.log')

DATA = 'int ext_data = 77; int ext_arr[4] = {1, 2, 3, 4};\n'
USE_PIC = r'''
extern int ext_data;
int get_ext(void) { return ext_data; }
int *addr_ext(void) { return &ext_data; }
int *ext_ptr = &ext_data;
'''
HOST_COPY = r'''
#include <stdio.h>
extern int ext_data;
int get_ext(void); int *addr_ext(void); extern int *ext_ptr;
int main(void) {
  int ok = get_ext() == 77 && addr_ext() == &ext_data && ext_ptr == &ext_data;
  ext_data = 5;
  ok = ok && get_ext() == 5;
  printf("%s get_ext=%d &exe=%p &lib=%p ptr=%p\n", ok ? "OK" : "FAIL", get_ext(), (void *)&ext_data, (void *)addr_ext(), (void *)ext_ptr);
  return !ok;
}
'''

GC = r'''
#include <stdio.h>
int exported_unref(void) { return 1; }
__attribute__((visibility("hidden"))) int hidden_unref(void) { return 2; }
int verlocal_unref(void) { return 3; }
static int static_unref(void) { return 4; }
__attribute__((visibility("protected"))) int protected_unref(void) { return 5; }
int exported_data_unref = 6;
__attribute__((visibility("hidden"))) int hidden_data_unref = 7;
static int ctor_ran, ctors_ran, init_ran;
__attribute__((constructor)) static void ctor_fn(void) { ctor_ran = 1; }
static void ctors_fn(void) { ctors_ran = 1; }
int flags(void) { return ctor_ran + 2 * ctors_ran + 4 * init_ran; }
__attribute__((section(".ctors"), used)) static void (*ctors_entry)(void) = ctors_fn;
void init_opt_fn(void) { }
__attribute__((visibility("hidden"))) void hidden_init_opt_fn(void) { init_ran = 1; }
__attribute__((section("cid_ref"), used)) int cid_ref_var = 8;
__attribute__((section("cid_unref"))) int cid_unref_var = 9;
__attribute__((section("cid_noref_used"), used)) int cid_noref_used_var = 10;
extern int __start_cid_ref[];
int read_cid(void) { return __start_cid_ref[0]; }
extern int undef_in_dead(void);
__attribute__((visibility("hidden"))) int dead_caller(void) { return undef_in_dead(); }
'''
GCHOST = r'''
#include <stdio.h>
#include <dlfcn.h>
int main(int argc, char **argv) {
  void *h = dlopen(argv[1], RTLD_NOW);
  if (!h) { printf("dlopen FAILED: %s\n", dlerror()); return 2; }
  int (*rc)(void) = (int (*)(void))dlsym(h, "read_cid");
  int (*fl)(void) = (int (*)(void))dlsym(h, "flags");
  if (!rc || !fl) { printf("dlsym FAILED read_cid=%p flags=%p\n", (void *)rc, (void *)fl); return 3; }
  int f = fl(), v = rc();
  printf("read_cid()=%d (%s) constructor=%d .ctors=%d -init=%d\n", v, v == 8 ? "OK" : "FAIL", f & 1, (f >> 1) & 1, (f >> 2) & 1);
  return 0;
}
'''
GCVERS = '{ global: exported_unref; protected_unref; exported_data_unref; read_cid; init_opt_fn; flags; local: *; };\n'

LIBGCC = {
    'a64': '__int128 divit(__int128 a, __int128 b) { return a / b; }\n',
    'x64': '__int128 divit(__int128 a, __int128 b) { return a / b; }\n',
    'i386': 'long long divit(long long a, long long b) { return a / b; }\n',
}
LIBGCC_EH = '#include <unwind.h>\nstatic _Unwind_Reason_Code cb(struct _Unwind_Context *c, void *a) { ++*(int *)a; return _URC_NO_REASON; }\nint depth(void) { int n = 0; _Unwind_Backtrace(cb, &n); return n; }\n'
HOST_LIBGCC = r'''
#include <stdio.h>
int depth(void);
#ifdef I386
long long divit(long long, long long);
int main(void) { long long r = divit(1LL << 40, 3); printf("%s divit=%lld depth=%d\n", r == (1LL << 40) / 3 && depth() > 0 ? "OK" : "FAIL", r, depth()); return 0; }
#else
__int128 divit(__int128, __int128);
int main(void) { __int128 r = divit(((__int128)1) << 100, 3); printf("%s divit_hi=%llx depth=%d\n", r == (((__int128)1) << 100) / 3 && depth() > 0 ? "OK" : "FAIL", (unsigned long long)(r >> 64), depth()); return 0; }
#endif
'''

DYNSECS = ('.gnu.hash', '.hash', '.dynsym', '.dynstr', '.gnu.version', '.gnu.version_r', '.gnu.version_d',
           '.rela.dyn', '.rel.dyn', '.rela.plt', '.rel.plt', '.dynamic', '.got', '.got.plt')

for arch in ARCHES:
    d = adir(arch, 'misc')
    log(f"\n######## {arch}")
    od, _ = compile_src(arch, 'edata', DATA, ['-fPIC'])
    libdata = os.path.join(d, 'libedata.so')
    link(arch, 'bfd', libdata, [od], ['-Wl,-soname,libedata.so'])
    # ---- E1: COPY: -fPIC and non-PIC references to an imported datum ----
    for model, cf in (('pic', ['-fPIC']), ('nopic', ['-fno-pic']), ('pie', ['-fPIE'])):
        ou, diag = compile_src(arch, 'euse' + model, USE_PIC, cf)
        for lk in LK4:
            so = os.path.join(adir(arch, 'misc', lk), f'libeuse_{model}.so')
            rc, diag = link(arch, lk, so, [ou, '-L' + d, '-ledata', '-Wl,--no-as-needed'])
            if rc:
                log(f"== {arch} copy {model} {lk}: LINK REFUSED: {first_line(diag)}"); continue
            rr = [f"{r['type']}@{r['off']:#x}[{r['sym']}]" for rs in relocs(so).values() for r in rs if 'ext_data' in r['sym']]
            dt = [t for t, v in dynamic(so) if t in ('TEXTREL', 'FLAGS')]
            warn = f" (diag: {first_line(diag)})" if diag else ''
            log(f"== {arch} copy {model} {lk}: records against ext_data: {rr} {dt}{warn}")
            exe = os.path.join(adir(arch, 'misc', lk), f'h_copy_{model}')
            rc, dg = host_exe(arch, exe, HOST_COPY, [so, libdata], name='h_copy')
            if rc:
                log(f"   host link FAILED {first_line(dg)}"); continue
            rc, out, err = run(arch, exe, [], env={'LD_LIBRARY_PATH': os.path.dirname(so) + ':' + d})
            log(f"   RUN rc={rc}: {' / '.join((out + err).splitlines())}")
    # ---- E2: --gc-sections roots ----
    og, _ = compile_src(arch, 'gc', GC, ['-fPIC', '-ffunction-sections', '-fdata-sections', '-fkeep-static-functions'])
    vs = wfile(os.path.join(d, 'gc.map'), GCVERS)
    gchost = os.path.join(d, 'h_gc')
    host_exe(arch, gchost, GCHOST, name='h_gc')
    for lk in LK4:
        dd = adir(arch, 'misc', lk)
        for tag, fl in (('nogc', []), ('gc', ['-Wl,--gc-sections']), ('gc_vers', ['-Wl,--gc-sections', '-Wl,--version-script=' + vs]),
                        ('gc_init', ['-Wl,--gc-sections', '-Wl,-init=hidden_init_opt_fn']),
                        ('gc_zdefs', ['-Wl,--gc-sections', '-Wl,-z,defs']), ('nogc_zdefs', ['-Wl,-z,defs']),
                        ('gc_startstop_nogc', ['-Wl,--gc-sections', '-Wl,-z,nostart-stop-gc']),
                        ('gc_vers_nossgc', ['-Wl,--gc-sections', '-Wl,--version-script=' + vs, '-Wl,-z,nostart-stop-gc']),
                        ('gc_vers_init', ['-Wl,--gc-sections', '-Wl,--version-script=' + vs, '-Wl,-init=hidden_init_opt_fn'])):
            so = os.path.join(dd, f'libgc_{tag}.so')
            rc, diag = link(arch, lk, so, [og], fl)
            if rc:
                log(f"== {arch} {lk} {tag}: LINK REFUSED rc={rc}: {first_line(diag)}"); continue
            names = set(s['name'] for s in symtab(so))
            dnames = set(s['name'] for s in dynsyms(so))
            secs = set(s['name'] for s in sections(so))
            keep = []
            for n in ('exported_unref', 'hidden_unref', 'verlocal_unref', 'static_unref', 'protected_unref', 'exported_data_unref',
                      'hidden_data_unref', 'ctor_fn', 'ctors_fn', 'init_opt_fn', 'hidden_init_opt_fn', 'cid_ref_var', 'cid_unref_var',
                      'cid_noref_used_var', 'read_cid', 'dead_caller'):
                keep.append(f"{n}={'K' if n in names else '-'}{'D' if n in dnames else ''}")
            rc, out, err = run(arch, gchost, [so])
            log(f"   RUN {tag} rc={rc}: {' / '.join((out + err).splitlines())}")
            log(f"== {arch} {lk} {tag}: {' '.join(keep)} | secs: {' '.join(s for s in sorted(secs) if s in ('.ctors', '.init_array', 'cid_ref', 'cid_unref', 'cid_noref_used'))} INIT={dynval(so, 'INIT')}" + (f" (diag: {first_line(diag)})" if diag else ''))
    # ---- E3: raw `ld -shared` (no driver flags): hash style, .eh_frame_hdr ----
    ob, _ = compile_src(arch, 'rawlib', 'extern int puts(const char*); int f(void){ puts("x"); return 1; }\n', ['-fPIC'])
    em = {'a64': 'aarch64linux', 'x64': 'elf_x86_64', 'i386': 'elf_i386'}[arch]
    for lk in LK4:
        ldbin = {'bfd': {'a64': 'ld.bfd', 'x64': 'x86_64-linux-gnu-ld', 'i386': 'i686-linux-gnu-ld'}[arch],
                 'lld22': 'ld.lld-22', 'lld19': 'ld.lld-19', 'mold': 'mold'}[lk]
        for tag, fl in (('raw', []), ('raw_ehhdr', ['--eh-frame-hdr'])):
            so = os.path.join(adir(arch, 'misc', lk), f'lib{tag}.so')
            if os.path.exists(so): os.unlink(so)
            rc, out, err = sh([ldbin, '-m', em, '-shared', ob, '-o', so, *fl])
            if rc:
                log(f"== {arch} {lk} {tag}: rc={rc} {first_line(out + err)}"); continue
            secs = [s['name'] for s in sections(so)]
            ph = [p['type'] for p in phdrs(so)]
            log(f"== {arch} {lk} {tag}: .hash={'.hash' in secs} .gnu.hash={'.gnu.hash' in secs} .eh_frame_hdr={'.eh_frame_hdr' in secs} PT_GNU_EH_FRAME={'GNU_EH_FRAME' in ph} .eh_frame={'.eh_frame' in secs} PT_GNU_STACK={'GNU_STACK' in ph} PT_GNU_RELRO={'GNU_RELRO' in ph} segs={' '.join(ph)}")
    # ---- E4: section headers of the dynamic sections (the b_versdef library of part B, else built here) ----
    oc, _ = compile_src(arch, 'shdrlib', 'extern int puts(const char*); int g_v = 1; int *gp = &g_v; int f(void){ puts("x"); return *gp; }\nint f2(void){return 2;}\n', ['-fPIC'])
    vmap = wfile(os.path.join(d, 'shdr.map'), 'V1 { global: f; g_v; gp; local: *; };\nV2 { global: f2; } V1;\n')
    for lk in LK4:
        so = os.path.join(adir(arch, 'misc', lk), 'libshdr.so')
        rc, diag = link(arch, lk, so, [oc], ['-Wl,--version-script=' + vmap, '-Wl,-soname,libshdr.so'])
        if rc:
            log(f"== {arch} {lk} shdr: LINK REFUSED {first_line(diag)}"); continue
        secs = sections(so)
        byidx = {s['idx']: s['name'] for s in secs}
        ds = dynsyms(so)
        firstglobal = next((s['num'] for s in ds if s['bind'] != 'LOCAL' and s['num'] > 0), None)
        nlocal_dyn = [s['name'] or f"#{s['num']}" for s in ds if s['bind'] == 'LOCAL' and s['num'] > 0]
        log(f"== {arch} {lk} shdr: .dynsym first non-local={firstglobal} local dynsyms besides null={nlocal_dyn}")
        for s in secs:
            if s['name'] in DYNSECS:
                log(f"     {s['name']:<15} {s['type']:<12} flags={s['flags']:<4} link={byidx.get(s['link'], s['link'])} info={byidx.get(s['info'], s['info']) if 'I' in s['flags'] else s['info']} entsize={s['es']} align={s['align']}")
    # ---- E5: -static-libgcc into a .so ----
    ol, _ = compile_src(arch, 'lgcc', LIBGCC[arch] + LIBGCC_EH, ['-fPIC'])
    for lk in LK4:
        dd = adir(arch, 'misc', lk)
        for tag, fl in (('shared_libgcc', ['-shared-libgcc']), ('static_libgcc', ['-static-libgcc'])):
            so = os.path.join(dd, f'liblgcc_{tag}.so')
            rc, diag = link(arch, lk, so, [ol], fl)
            if rc:
                log(f"== {arch} {lk} {tag}: LINK REFUSED {first_line(diag)}"); continue
            ds = dynsyms(so)
            st = symtab(so)
            want = ('__divti3', '__divdi3', '_Unwind_Backtrace', '_Unwind_GetIP', '__udivti3', '__udivmoddi4', '__divmoddi4')
            dline = [f"{s['name']}:{s['ndx']}/{s['bind']}/{s['vis']}" for s in ds if s['name'].split('@')[0] in want]
            sline = [f"{s['name']}:{s['bind']}/{s['vis']}" for s in st if s['name'] in want]
            log(f"== {arch} {lk} {tag}: NEEDED={[v.split('[')[1].rstrip(']') for v in dynval(so, 'NEEDED')]} dynsym={dline} symtab={sline}")
            exe = os.path.join(dd, f'h_lgcc_{tag}')
            oh, _ = compile_src(arch, 'h_lgcc', HOST_LIBGCC, ['-O1'] + (['-DI386'] if arch == 'i386' else []))
            rc, dg = link(arch, 'bfd', exe, [oh, so, '-Wl,-rpath,' + dd], shared=False)
            if rc:
                log(f"   host link FAILED {first_line(dg)}"); continue
            rc, out, err = run(arch, exe, [])
            log(f"   RUN rc={rc}: {' / '.join((out + err).splitlines())}")

# ---- E6: no COPY record in any shared object written by any part ----
log("\n######## COPY scan over every .so under WORK")
n = 0
bad = []
for root, dirs, files in os.walk(WORK):
    for f in files:
        if f.endswith('.so'):
            p = os.path.join(root, f)
            n += 1
            for rs in relocs(p).values():
                for r in rs:
                    if 'COPY' in r['type']:
                        bad.append(f"{p}: {r['type']} {r['sym']}")
log(f"   scanned {n} shared objects; COPY records: {bad if bad else 'none'}")
