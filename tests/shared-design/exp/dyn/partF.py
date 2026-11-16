# Part F: PT_LOAD p_align / congruence in shared objects, and what glibc's loader refuses.
import os, sys, struct, shutil
exec(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'common.py')).read())
open_log('F.log')

rc, o, e = sh(['getconf', 'PAGESIZE'])
log(f"host page size (getconf PAGESIZE) = {o.strip()}; qemu-user x86 guests report: see run lines")

HOST = r'''
#include <stdio.h>
#include <dlfcn.h>
#include <unistd.h>
int main(int argc, char **argv) {
  printf("pagesize=%ld ", sysconf(_SC_PAGESIZE));
  void *h = dlopen(argv[1], RTLD_NOW);
  if (!h) { printf("dlopen FAILED: %s\n", dlerror()); return 2; }
  int (*f)(void) = (int (*)(void))dlsym(h, "f");
  printf("dlopen ok f()=%d\n", f());
  return 0;
}
'''
LIB = 'int data = 5; int *p = &data; int f(void) { return *p + 37; }\n'

def phdr_table(path):
    data = bytearray(open(path, 'rb').read())
    cls = data[4]
    if cls == 2:
        phoff, = struct.unpack_from('<Q', data, 0x20); phentsize, phnum = struct.unpack_from('<HH', data, 0x36)
    else:
        phoff, = struct.unpack_from('<I', data, 0x1c); phentsize, phnum = struct.unpack_from('<HH', data, 0x2a)
    return data, cls, phoff, phentsize, phnum

def loads(path):
    data, cls, phoff, es, n = phdr_table(path)
    res = []
    for i in range(n):
        b = phoff + i * es
        if cls == 2:
            ty, fl, off, va, pa, fs, ms, al = struct.unpack_from('<IIQQQQQQ', data, b)
        else:
            ty, off, va, pa, fs, ms, fl, al = struct.unpack_from('<IIIIIIII', data, b)
        res.append((i, ty, off, va, fs, ms, al, b))
    return res

def patch_align(src, dst, newalign, which_load=1):
    """set p_align of the which_load-th PT_LOAD (0-based) to newalign."""
    shutil.copyfile(src, dst)
    data, cls, phoff, es, n = phdr_table(dst)
    k = 0
    for i, ty, off, va, fs, ms, al, b in loads(dst):
        if ty == 1:
            if k == which_load:
                if cls == 2:
                    struct.pack_into('<Q', data, b + 48, newalign)
                else:
                    struct.pack_into('<I', data, b + 28, newalign)
                open(dst, 'wb').write(data)
                return off, va, al
            k += 1

def patch_offset(src, dst, delta, which_load=1):
    """shift p_offset of a PT_LOAD by delta (breaks p_offset == p_vaddr mod p_align)."""
    shutil.copyfile(src, dst)
    data, cls, phoff, es, n = phdr_table(dst)
    k = 0
    for i, ty, off, va, fs, ms, al, b in loads(dst):
        if ty == 1:
            if k == which_load:
                if cls == 2:
                    struct.pack_into('<Q', data, b + 8, off + delta)
                else:
                    struct.pack_into('<I', data, b + 4, off + delta)
                open(dst, 'wb').write(data)
                return off, va, al
            k += 1

for arch in ARCHES:
    log(f"\n######## {arch}")
    o, _ = compile_src(arch, 'flib', LIB, ['-fPIC'])
    hexe = os.path.join(adir(arch, 'pagesz'), 'h_pg')
    host_exe(arch, hexe, HOST, name='h_pg')
    for lk in LK4:
        d = adir(arch, 'pagesz', lk)
        for tag, fl in (('default', []), ('mp4k', ['-Wl,-z,max-page-size=0x1000']), ('mp16k', ['-Wl,-z,max-page-size=0x4000']),
                        ('mp64k', ['-Wl,-z,max-page-size=0x10000']), ('mp2m', ['-Wl,-z,max-page-size=0x200000'])):
            so = os.path.join(d, f'libpg_{tag}.so')
            rc, diag = link(arch, lk, so, [o], fl)
            if rc:
                log(f"== {arch} {lk} {tag}: LINK REFUSED {first_line(diag)}"); continue
            ls = [x for x in loads(so) if x[1] == 1]
            cong = all((off - va) % al == 0 for i, ty, off, va, fs, ms, al, b in ls)
            log(f"== {arch} {lk} {tag}: " + ' | '.join(f"off={off:#x} va={va:#x} align={al:#x}" for i, ty, off, va, fs, ms, al, b in ls) + f"  congruent={cong} filesize={os.path.getsize(so):#x}")
            rc, out, err = run(arch, hexe, [so])
            log(f"   RUN rc={rc}: {(out + err).strip()}")
        # loader checks on a patched copy of the default output
        base = os.path.join(d, 'libpg_default.so')
        if not os.path.exists(base):
            continue
        for tag, fn, arg in (('align_0x10', patch_align, 0x10), ('align_0x800', patch_align, 0x800),
                             ('offset_plus_0x10', patch_offset, 0x10), ('offset_plus_0x1000', patch_offset, 0x1000)):
            dst = os.path.join(d, f'libpg_patched_{tag}.so')
            before = fn(base, dst, arg)
            rc, out, err = run(arch, hexe, [dst])
            log(f"   patched {tag} (second PT_LOAD was off={before[0]:#x} va={before[1]:#x} align={before[2]:#x}): RUN rc={rc}: {(out + err).strip()}")
