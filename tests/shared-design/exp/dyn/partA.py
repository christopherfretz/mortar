# Part A: the output kind Shared (D-1 column), segments, layout, entry, page sizes.
import os, sys
exec(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'common.py')).read())
open_log('A.log')

LIB = r'''
#include <stdio.h>
int lib_data = 7;
int *lib_ptr = &lib_data;
static int ctor_ran;
__attribute__((constructor)) static void c(void) { ctor_ran = 1; }
extern int puts(const char *);
int f(void) { puts("in f"); return 42 + ctor_ran - 1 + (*lib_ptr - 7); }
'''
LIB_START = LIB + 'void _start(void) { }\n'
LIB_MAIN = LIB + 'int main(void) { return 3; }\n'
HOST = r'''
#include <stdio.h>
#include <dlfcn.h>
int f(void);
int main(int argc, char **argv) {
  printf("needed: f()=%d %s\n", f(), f() == 42 ? "OK" : "FAIL");
  void *h = dlopen(argv[1], RTLD_NOW);
  if (!h) { printf("dlopen FAIL %s\n", dlerror()); return 1; }
  int (*g)(void) = (int (*)(void))dlsym(h, "f");
  printf("dlopen: f()=%d %s same=%d\n", g(), g() == 42 ? "OK" : "FAIL", g == f);
  return 0;
}
'''

def segs_str(ph):
    return ' '.join(p['type'] + ('(' + p['flags'] + ')' if p['type'] == 'LOAD' else '') for p in ph)

def where(secs, ph, name):
    s = [x for x in secs if x['name'] == name]
    if not s:
        return '-'
    s = s[0]
    rel = [p for p in ph if p['type'] == 'GNU_RELRO']
    loads = [i for i, p in enumerate([q for q in ph if q['type'] == 'LOAD']) if p['vaddr'] <= s['addr'] < p['vaddr'] + max(p['memsz'], 1)]
    tag = ''
    for p in rel:
        lo, hi = p['vaddr'], p['vaddr'] + p['memsz']
        if lo <= s['addr'] < hi:
            end = s['addr'] + s['size']
            tag = '/RELRO' if end <= hi else f"/RELRO(first {hi - s['addr']:#x} of {s['size']:#x})"
    return f"L{loads[0] if loads else '?'}{tag} [{s['addr']:#x}+{s['size']:#x}]"

def describe(arch, lk, so, tag):
    ty, en = hdr(so)
    ph = phdrs(so)
    secs = sections(so)
    loads = [p for p in ph if p['type'] == 'LOAD']
    d = dynamic(so)
    def tv(t):
        v = [x for k, x in d if k == t]
        return v[0] if v else '-'
    rel = [p for p in ph if p['type'] == 'GNU_RELRO']
    relend = '-'
    if rel:
        e = rel[0]['vaddr'] + rel[0]['memsz']
        relend = f"{e:#x} (mod 64K={e % 0x10000:#x}, mod 4K={e % 0x1000:#x})"
    alloc = [s for s in secs if 'A' in s['flags'] and s['name']]
    alloc.sort(key=lambda s: (s['addr'], s['idx']))
    # start symbol
    st = [s for s in symtab(so) if s['name'] in ('_start', 'main', 'f', '_init')]
    log(f"== {arch} {lk} {tag}")
    log(f"  e_type={ty} e_entry={en} base(first PT_LOAD vaddr)={loads[0]['vaddr']:#x} PT_LOAD p_align={[hex(p['align']) for p in loads]}")
    log(f"  PT_INTERP={'yes' if any(p['type']=='INTERP' for p in ph) else 'no'} .interp={'yes' if any(s['name']=='.interp' for s in secs) else 'no'} PT_PHDR={'yes' if any(p['type']=='PHDR' for p in ph) else 'no'}")
    log(f"  SONAME={tv('SONAME')} DEBUG={tv('DEBUG')} FLAGS={tv('FLAGS')} FLAGS_1={tv('FLAGS_1')}")
    log(f"  segments: {segs_str(ph)}")
    log(f"  loads: " + ' | '.join(f"{p['flags']} off={p['off']:#x} va={p['vaddr']:#x} fs={p['filesz']:#x} ms={p['memsz']:#x} [{' '.join(p.get('secs', []))}]" for p in loads))
    log(f"  sections: {' '.join(s['name'] for s in alloc)}")
    log(f"  .dynamic={where(secs, ph, '.dynamic')} .got={where(secs, ph, '.got')} .got.plt={where(secs, ph, '.got.plt')} .data={where(secs, ph, '.data')}  RELRO end={relend}")
    log(f"  syms: " + ' '.join(f"{s['name']}={s['value']:#x}" for s in st))
    for p in loads:
        if p['off'] % p['align'] != p['vaddr'] % p['align']:
            log(f"  !! p_offset {p['off']:#x} != p_vaddr {p['vaddr']:#x} mod p_align {p['align']:#x}")
    dump(so, so + '.readelf')

def runhost(arch, lk, so, tag):
    d = os.path.dirname(so)
    exe = os.path.join(d, f'host-{tag}')
    rc, diag = host_exe(arch, exe, HOST, [so])
    if rc:
        log(f"  host link FAILED: {first_line(diag)}"); return
    rc, o, e = run(arch, exe, [so])
    log(f"  run rc={rc}: " + ' / '.join(l for l in (o + e).splitlines() if l != 'in f'))

def variant(arch, lk, tag, src, cflags=('-fPIC',), ldflags=(), host=True):
    o, diag = compile_src(arch, 'liba', src, list(cflags))
    d = adir(arch, lk)
    so = os.path.join(d, f'lib{tag}.so')
    rc, diag = link(arch, lk, so, [o], list(ldflags))
    if rc:
        log(f"== {arch} {lk} {tag}: LINK REFUSED rc={rc}: {first_line(diag)}")
        return None
    if diag:
        log(f"   ({arch} {lk} {tag} link diag: {first_line(diag)})")
    describe(arch, lk, so, tag)
    if host:
        runhost(arch, lk, so, tag)
    return so

for arch in ARCHES:
    for lk in LK4:
        log(f"\n######## {arch} {lk}")
        variant(arch, lk, 'base', LIB, ldflags=['-Wl,-soname,libbase.so'])
        variant(arch, lk, 'nosoname', LIB, host=False)
        variant(arch, lk, 'start', LIB_START)
        variant(arch, lk, 'main', LIB_MAIN)
        variant(arch, lk, 'e_f', LIB, ldflags=['-Wl,-e,f'], host=False)
        variant(arch, lk, 'e_undef', LIB, ldflags=['-Wl,-e,nosuchsym'], host=False)
        variant(arch, lk, 'sepcode', LIB, ldflags=['-Wl,-z,separate-code'])
        variant(arch, lk, 'nosepcode', LIB, ldflags=['-Wl,-z,noseparate-code'])
        variant(arch, lk, 'mp4k', LIB, ldflags=['-Wl,-z,max-page-size=0x1000'])
        variant(arch, lk, 'mp64k', LIB, ldflags=['-Wl,-z,max-page-size=0x10000'])
        variant(arch, lk, 'cp4k', LIB, ldflags=['-Wl,-z,common-page-size=0x1000'], host=False)
        variant(arch, lk, 'norelro', LIB, ldflags=['-Wl,-z,norelro'])
        variant(arch, lk, 'now', LIB, ldflags=['-Wl,-z,now'])
        variant(arch, lk, 'imagebase', LIB, ldflags=['-Wl,--image-base=0x10000000'])
        variant(arch, lk, 'ttextseg', LIB, ldflags=['-Wl,-Ttext-segment=0x20000000'])
        # TLS so PT_TLS shows
        variant(arch, lk, 'tls', LIB + '__thread int tv = 5; int gettv(void){return tv;}\n', host=False)
        # GNU property note (x86 CET / aarch64 BTI)
        if arch == 'a64':
            variant(arch, lk, 'bti', LIB, cflags=['-fPIC', '-mbranch-protection=standard'], host=True)
        else:
            variant(arch, lk, 'cet', LIB, cflags=['-fPIC', '-fcf-protection=full'], host=True)
