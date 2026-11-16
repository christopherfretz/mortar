# Shared helpers for the WP-110 `export` experiments (run with python3 -I).
import os, subprocess, struct, sys, json, shutil, re

WORK = None          # set by init()
LOG = None
ARCHES = ['a64', 'x64', 'i386']
LINKERS = ['bfd', 'lld22', 'lld19', 'mold']
TRIPLE = {'a64': 'aarch64-linux-gnu', 'x64': 'x86_64-linux-gnu', 'i386': 'i686-linux-gnu'}
GCC = {'a64': 'gcc', 'x64': 'x86_64-linux-gnu-gcc', 'i386': 'i686-linux-gnu-gcc'}
GXX = {'a64': 'g++', 'x64': 'x86_64-linux-gnu-g++', 'i386': 'i686-linux-gnu-g++'}
QEMU = {'a64': [], 'x64': ['qemu-x86_64', '-L', '/usr/x86_64-linux-gnu'],
        'i386': ['qemu-i386', '-L', '/usr/i686-linux-gnu']}


def init(work, part):
    global WORK, LOG
    WORK = os.path.join(work, part)
    os.makedirs(WORK, exist_ok=True)
    LOG = open(os.path.join(WORK, 'cmds.log'), 'a')
    bindir = os.path.join(work, 'bin')
    for v in ('19', '22'):
        d = os.path.join(bindir, 'lld' + v)
        os.makedirs(d, exist_ok=True)
        p = os.path.join(d, 'ld.lld')
        if not os.path.islink(p):
            os.symlink('/usr/bin/ld.lld-' + v, p)
    return WORK


def log(s):
    LOG.write(s + '\n'); LOG.flush()


def sh(cmd, cwd=None, env=None, timeout=60):
    """Run cmd (list); return (rc, combined output). Logged."""
    cwd = cwd or WORK
    e = dict(os.environ); e.update(env or {})
    try:
        p = subprocess.run(cmd, cwd=cwd, env=e, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                           timeout=timeout)
        out = p.stdout.decode('utf-8', 'replace'); rc = p.returncode
    except subprocess.TimeoutExpired:
        out, rc = 'TIMEOUT', -999
    log('$ (cd %s) %s\n[rc=%d]\n%s' % (cwd, ' '.join(cmd), rc, out))
    return rc, out


def ldflags(linker):
    root = os.path.dirname(WORK)
    if linker == 'bfd':
        return ['-fuse-ld=bfd']
    if linker == 'mold':
        return ['-fuse-ld=mold']
    if linker.startswith('lld'):
        return ['-B', os.path.join(root, 'bin', linker), '-fuse-ld=lld']
    raise ValueError(linker)


def compiler(arch, comp, cxx=False):
    """comp in gcc, clang22, clang19"""
    if comp == 'gcc':
        return [GXX[arch] if cxx else GCC[arch]]
    v = comp[-2:]
    return ['clang%s-%s' % ('++' if cxx else '', v), '--target=' + TRIPLE[arch]]


def cc(arch, comp, src, out, flags=(), cxx=None):
    if cxx is None:
        cxx = src.endswith('.cc') or src.endswith('.cpp')
    return sh(compiler(arch, comp, cxx) + ['-c', src, '-o', out] + list(flags))


def link(arch, linker, inputs, out, flags=(), shared=True, cxx=False):
    if os.path.exists(os.path.join(WORK, out)):
        raise RuntimeError('link to a fresh path: ' + out)
    cmd = [GXX[arch] if cxx else GCC[arch]] + ldflags(linker) + (['-shared'] if shared else []) \
        + list(inputs) + ['-o', out] + list(flags)
    return sh(cmd)


def run(arch, exe, libdirs=(), preload=None, args=(), timeout=30):
    env = {}
    ld = ':'.join(os.path.join(WORK, d) for d in libdirs)
    if arch == 'a64':
        if ld: env['LD_LIBRARY_PATH'] = ld
        if preload: env['LD_PRELOAD'] = os.path.join(WORK, preload)
        return sh([os.path.join(WORK, exe)] + list(args), env=env, timeout=timeout)
    q = list(QEMU[arch])
    if ld: q += ['-E', 'LD_LIBRARY_PATH=' + ld]
    if preload: q += ['-E', 'LD_PRELOAD=' + os.path.join(WORK, preload)]
    return sh(q + [os.path.join(WORK, exe)] + list(args), timeout=timeout)


def write(name, text):
    p = os.path.join(WORK, name)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, 'w') as f:
        f.write(text)
    return name


def first_diag(out, n=3):
    """The error/warning lines of a linker output (driver noise removed)."""
    keep = []
    for l in out.splitlines():
        if 'collect2' in l or l.startswith('clang') or not l.strip():
            continue
        l = l.replace(WORK + '/', '')
        l = re.sub(r'/usr/lib/gcc-cross/[^ :]*/bin/', '', l)
        l = l.replace('/usr/bin/ld.bfd', 'ld.bfd')
        keep.append(l.strip())
    return ' | '.join(keep[:n])


# ---------------------------------------------------------------- ELF reader
class Elf:
    def __init__(self, path):
        with open(path, 'rb') as f:
            self.b = f.read()
        b = self.b
        assert b[:4] == b'\x7fELF'
        self.c64 = b[4] == 2
        self.e = '<'
        if self.c64:
            (self.type, self.machine, _, self.entry, self.phoff, self.shoff, _, _, self.phentsize,
             self.phnum, self.shentsize, self.shnum, self.shstrndx) = struct.unpack_from('<HHIQQQIHHHHHH', b, 16)
        else:
            (self.type, self.machine, _, self.entry, self.phoff, self.shoff, _, _, self.phentsize,
             self.phnum, self.shentsize, self.shnum, self.shstrndx) = struct.unpack_from('<HHIIIIIHHHHHH', b, 16)
        self.osabi = b[7]
        self.sh = []
        for i in range(self.shnum):
            o = self.shoff + i * self.shentsize
            if self.c64:
                n, t, fl, a, off, sz, lk, inf, al, es = struct.unpack_from('<IIQQQQIIQQ', b, o)
            else:
                n, t, fl, a, off, sz, lk, inf, al, es = struct.unpack_from('<IIIIIIIIII', b, o)
            self.sh.append(dict(nameoff=n, type=t, flags=fl, addr=a, off=off, size=sz, link=lk, info=inf,
                                align=al, entsize=es))
        if self.shnum:
            so = self.sh[self.shstrndx]['off']
            for s in self.sh:
                s['name'] = self.cstr(so + s['nameoff'])

    def cstr(self, o):
        e = self.b.index(b'\0', o)
        return self.b[o:e].decode('latin1')

    def sec(self, name=None, type=None):
        for i, s in enumerate(self.sh):
            if (name and s['name'] == name) or (type and s['type'] == type):
                return i, s
        return None, None

    def data(self, s):
        return self.b[s['off']:s['off'] + s['size']]

    def dynsym(self):
        i, s = self.sec(type=11)  # SHT_DYNSYM
        if s is None:
            return []
        stro = self.sh[s['link']]['off']
        es = 24 if self.c64 else 16
        out = []
        for k in range(s['size'] // es):
            o = s['off'] + k * es
            if self.c64:
                nm, info, other, shndx, val, size = struct.unpack_from('<IBBHQQ', self.b, o)
            else:
                nm, val, size, info, other, shndx = struct.unpack_from('<IIIBBH', self.b, o)
            out.append(dict(name=self.cstr(stro + nm), value=val, size=size, bind=info >> 4, type=info & 15,
                            vis=other & 3, other=other, shndx=shndx))
        return out

    def versym(self):
        i, s = self.sec(type=0x6fffffff)
        if s is None:
            return None
        d = self.data(s)
        return list(struct.unpack('<%dH' % (len(d) // 2), d))

    def dynstr_off(self):
        i, s = self.sec(type=11)
        return self.sh[s['link']]['off']

    def verdef(self):
        i, s = self.sec(type=0x6ffffffd)
        if s is None:
            return None
        stro = self.sh[s['link']]['off']
        out = []
        o = s['off']
        while True:
            ver, flags, ndx, cnt, h, aux, nxt = struct.unpack_from('<HHHHIII', self.b, o)
            names = []
            a = o + aux
            for j in range(cnt):
                nm, anext = struct.unpack_from('<II', self.b, a)
                names.append(self.cstr(stro + nm))
                a += anext
            out.append(dict(version=ver, flags=flags, ndx=ndx, cnt=cnt, hash=h, names=names))
            if not nxt:
                break
            o += nxt
        return out

    def verneed(self):
        i, s = self.sec(type=0x6ffffffe)
        if s is None:
            return None
        stro = self.sh[s['link']]['off']
        out = []
        o = s['off']
        while True:
            ver, cnt, fil, aux, nxt = struct.unpack_from('<HHIII', self.b, o)
            auxes = []
            a = o + aux
            for j in range(cnt):
                h, fl, other, nm, anext = struct.unpack_from('<IHHII', self.b, a)
                auxes.append(dict(hash=h, flags=fl, other=other, name=self.cstr(stro + nm)))
                if not anext:
                    break
                a += anext
            out.append(dict(file=self.cstr(stro + fil), aux=auxes))
            if not nxt:
                break
            o += nxt
        return out

    def dynamic(self):
        i, s = self.sec(type=6)
        if s is None:
            return []
        d = self.data(s)
        f = '<qQ' if self.c64 else '<iI'
        es = 16 if self.c64 else 8
        out = []
        for k in range(len(d) // es):
            t, v = struct.unpack_from(f, d, k * es)
            out.append((t, v))
            if t == 0:
                break
        return out

    def gnu_hash(self):
        i, s = self.sec(type=0x6ffffff6)
        if s is None:
            return None
        nb, symoff, bloom, shift = struct.unpack_from('<IIII', self.b, s['off'])
        return dict(nbuckets=nb, symoffset=symoff, maskwords=bloom, shift=shift, entsize=s['entsize'])

    def has_sec(self, name):
        return self.sec(name=name)[1] is not None


DT = {1: 'NEEDED', 14: 'SONAME', 16: 'SYMBOLIC', 22: 'TEXTREL', 30: 'FLAGS', 0x6ffffffb: 'FLAGS_1',
      0x6ffffffc: 'VERDEF', 0x6ffffffd: 'VERDEFNUM', 0x6ffffffe: 'VERNEED', 0x6fffffff: 'VERNEEDNUM',
      0x6ffffff0: 'VERSYM', 24: 'BIND_NOW', 0x6ffffef5: 'GNU_HASH', 4: 'HASH'}
BIND = {0: 'LOCAL', 1: 'GLOBAL', 2: 'WEAK', 10: 'UNIQUE'}
TYPE = {0: 'NOTYPE', 1: 'OBJECT', 2: 'FUNC', 3: 'SECTION', 4: 'FILE', 5: 'COMMON', 6: 'TLS', 10: 'IFUNC'}
VIS = {0: 'D', 1: 'I', 2: 'H', 3: 'P'}


def dyn_summary(path):
    """Compact description of the dynamic-symbol-relevant parts of an output."""
    e = Elf(path)
    d = e.dynamic()
    tags = {}
    for t, v in d:
        tags.setdefault(DT.get(t, hex(t)), []).append(v)
    return e, tags


def symdesc(s):
    if s['type'] == 3 and not s['name']:
        s = dict(s); s['name'] = 'SECTION#%d' % s['shndx']
    return '%s:%s/%s/%s%s' % (s['name'], BIND.get(s['bind'], s['bind']), TYPE.get(s['type'], s['type']),
                              VIS[s['vis']], '/UND' if s['shndx'] == 0 else '')


def gnu_hash_fn(name):
    h = 5381
    for c in name.encode():
        h = (h * 33 + c) & 0xffffffff
    return h


def elf_hash(name):
    h = 0
    for c in name.encode():
        h = ((h << 4) + c) & 0xffffffff
        g = h & 0xf0000000
        if g:
            h ^= g >> 24
        h &= ~g & 0xffffffff
    return h


def relocs(path):
    """readelf -W -r parsed: list of (section, offset, type, symname)."""
    p =subprocess.run(['readelf', '-W', '-r', path], stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    sec = None
    res = []
    for l in p.stdout.decode().splitlines():
        if l.startswith('Relocation section'):
            sec = l.split("'")[1]
            continue
        f = l.split()
        if len(f) >= 3 and f[2].startswith('R_'):
            sym = f[4] if len(f) >= 5 and not f[4].startswith('+') else ''
            res.append((sec, f[0], f[2], sym))
    return res
