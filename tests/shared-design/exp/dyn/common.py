# common helpers for the WP-110 `dyn` area (shared-object output).
# Loaded by the part scripts with exec(); run with python3 -I.
import os, subprocess, hashlib, re, sys, json

WORK = os.path.abspath(sys.argv[1])
os.makedirs(WORK, exist_ok=True)
BIN = os.path.join(WORK, 'bin')

ARCHES = {
    'a64':  dict(cc='gcc', cxx='g++', qemu=None, clang_t='aarch64-linux-gnu', sysroot='/'),
    'x64':  dict(cc='x86_64-linux-gnu-gcc', cxx='x86_64-linux-gnu-g++',
                 qemu=['qemu-x86_64', '-L', '/usr/x86_64-linux-gnu'], clang_t='x86_64-linux-gnu',
                 sysroot='/usr/x86_64-linux-gnu'),
    'i386': dict(cc='i686-linux-gnu-gcc', cxx='i686-linux-gnu-g++',
                 qemu=['qemu-i386', '-L', '/usr/i686-linux-gnu'], clang_t='i686-linux-gnu',
                 sysroot='/usr/i686-linux-gnu'),
}
# linker -> gcc driver flags (each makes the driver's `ld` the named linker)
LINKERS = {
    'bfd':   [],                       # the driver's default: GNU ld 2.44
    'lld22': ['-B' + BIN + '/lld22/'],
    'lld19': ['-B' + BIN + '/lld19/'],
    'mold':  ['-B' + BIN + '/mold/'],
}
LK3 = ['bfd', 'lld22', 'mold']
LK4 = ['bfd', 'lld22', 'lld19', 'mold']

def setup_bin():
    for n, t in (('lld22', '/usr/bin/ld.lld-22'), ('lld19', '/usr/bin/ld.lld-19'), ('mold', '/usr/bin/mold')):
        d = os.path.join(BIN, n)
        os.makedirs(d, exist_ok=True)
        p = os.path.join(d, 'ld')
        if not os.path.lexists(p):
            os.symlink(t, p)
setup_bin()

LOGF = None
def log(*a):
    s = ' '.join(str(x) for x in a)
    print(s)
    if LOGF:
        LOGF.write(s + '\n'); LOGF.flush()

def open_log(name):
    global LOGF
    os.makedirs(os.path.join(WORK, 'logs'), exist_ok=True)
    LOGF = open(os.path.join(WORK, 'logs', name), 'w')

def sh(cmd, cwd=None, env=None, timeout=120):
    try:
        r = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True, timeout=timeout)
        return r.returncode, r.stdout, r.stderr
    except subprocess.TimeoutExpired:
        return 124, '', 'TIMEOUT'

def wfile(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, 'w') as f:
        f.write(text)
    return path

def adir(arch, *sub):
    d = os.path.join(WORK, arch, *sub)
    os.makedirs(d, exist_ok=True)
    return d

def compile_src(arch, name, text, flags=(), lang='c', compiler='gcc'):
    """compile to an object; returns (path or None, diag)."""
    A = ARCHES[arch]
    ext = {'c': '.c', 'c++': '.cc', 's': '.s', 'S': '.S'}[lang]
    h = hashlib.sha1((text + '|'.join(flags) + lang + compiler).encode()).hexdigest()[:10]
    d = adir(arch, 'obj')
    src = wfile(os.path.join(d, f'{name}-{h}{ext}'), text)
    out = os.path.join(d, f'{name}-{h}.o')
    if os.path.exists(out):
        return out, ''
    if compiler == 'gcc':
        cc = A['cxx'] if lang == 'c++' else A['cc']
        cmd = [cc]
    else:  # clang-22
        cc = 'clang++-22' if lang == 'c++' else 'clang-22'
        cmd = [cc, '--target=' + A['clang_t']]
    rc, o, e = sh(cmd + ['-c', '-O1', *flags, src, '-o', out])
    if rc:
        return None, e.strip()
    return out, e.strip()

def link(arch, linker, out, objs, flags=(), shared=True, cxx=False):
    """link through the gcc driver; returns (rc, diag). out is a fresh path."""
    A = ARCHES[arch]
    cc = A['cxx'] if cxx else A['cc']
    if os.path.exists(out):
        os.unlink(out)
    cmd = [cc, *LINKERS[linker], *(['-shared'] if shared else []), *objs, *flags, '-o', out]
    rc, o, e = sh(cmd)
    return rc, (o + e).strip()

def host_exe(arch, out, src_text, libs=(), flags=(), cxx=False, name='host'):
    """the host program: compiled and linked by GNU ld on the default driver line."""
    o, d = compile_src(arch, name, src_text, ['-O1'], 'c++' if cxx else 'c')
    if not o:
        return 1, d
    ld = os.path.dirname(out)
    return link(arch, 'bfd', out, [o, *libs], ['-Wl,-rpath,' + ld, *flags, '-pthread'], shared=False, cxx=cxx)

def run(arch, exe, args=(), env=None, timeout=60):
    A = ARCHES[arch]
    env = dict(env or {})
    ldp = env.pop('LD_LIBRARY_PATH', os.path.dirname(exe))
    if A['qemu']:
        cmd = list(A['qemu'])
        cmd += ['-E', 'LD_LIBRARY_PATH=' + ldp]
        for k, v in env.items():
            cmd += ['-E', f'{k}={v}']
        cmd += [exe, *args]
        e = dict(os.environ)
    else:
        cmd = [exe, *args]
        e = dict(os.environ); e['LD_LIBRARY_PATH'] = ldp; e.update(env)
    rc, o, er = sh(cmd, env=e, timeout=timeout)
    return rc, o, er

# ---------------- readelf parsing ----------------
def readelf(path, opts):
    rc, o, e = sh(['readelf', '-W', *opts, path])
    return o + e

def hdr(path):
    t = readelf(path, ['-h'])
    ty = re.search(r'Type:\s+(\S+)', t).group(1)
    en = re.search(r'Entry point address:\s+(\S+)', t).group(1)
    return ty, en

def phdrs(path):
    t = readelf(path, ['-l'])
    res = []
    on = False
    for line in t.splitlines():
        if line.strip().startswith('Type') and 'Offset' in line:
            on = True; continue
        if on:
            if not line.strip():
                break
            if line.strip().startswith('[Requesting'):
                continue
            p = line.split()
            # Type Offset VirtAddr PhysAddr FileSiz MemSiz Flg... Align
            ty = p[0]
            flg = ' '.join(p[6:-1])
            res.append(dict(type=ty, off=int(p[1], 16), vaddr=int(p[2], 16), filesz=int(p[4], 16),
                            memsz=int(p[5], 16), flags=flg.replace(' ', ''), align=int(p[-1], 16)))
    # section mapping
    maps = []
    on = False
    for line in t.splitlines():
        if 'Segment Sections' in line:
            on = True; continue
        if on:
            m = re.match(r'\s+(\d+)\s*(.*)$', line)
            if not m:
                break
            maps.append(m.group(2).split())
    for i, m in enumerate(maps):
        if i < len(res):
            res[i]['secs'] = m
    return res

def sections(path):
    t = readelf(path, ['-S'])
    res = []
    for line in t.splitlines():
        m = re.match(r'\s*\[\s*(\d+)\]\s+(\S*)\s+(\S+)\s+([0-9a-f]+)\s+([0-9a-f]+)\s+([0-9a-f]+)\s+([0-9a-f]+)\s+(\S*)\s+(\d+)\s+(\d+)\s+(\d+)$', line)
        if m:
            res.append(dict(idx=int(m.group(1)), name=m.group(2), type=m.group(3), addr=int(m.group(4), 16),
                            off=int(m.group(5), 16), size=int(m.group(6), 16), es=int(m.group(7), 16),
                            flags=m.group(8), link=int(m.group(9)), info=int(m.group(10)), align=int(m.group(11))))
    return res

def dynamic(path):
    t = readelf(path, ['-d'])
    res = []
    for line in t.splitlines():
        m = re.match(r'\s*0x[0-9a-f]+\s+\((\S+)\)\s+(.*)$', line)
        if m:
            res.append((m.group(1), m.group(2).strip()))
    return res

def relocs(path):
    t = readelf(path, ['-r'])
    res = {}
    cur = None
    for line in t.splitlines():
        m = re.match(r"Relocation section '(\S+)'", line)
        if m:
            cur = m.group(1); res[cur] = []; continue
        if cur and re.match(r'^[0-9a-f]{8,16}\s', line):
            p = line.split(None, 4)
            if len(p) > 4:
                sym = p[4].strip()
            elif len(p) == 4:
                sym = 'addend ' + p[3]      # RELA record without a symbol: the addend
            else:
                sym = ''
            res[cur].append(dict(off=int(p[0], 16), type=p[2], sym=sym))
    return res

def dynsyms(path):
    t = readelf(path, ['--dyn-syms'])
    res = []
    for line in t.splitlines():
        m = re.match(r'\s*(\d+):\s+([0-9a-f]+)\s+(\S+)\s+(<OS specific>: \d+|\S+)\s+(\S+)\s+(\S+)\s+(\S+)\s*(.*)$', line)
        if m:
            res.append(dict(num=int(m.group(1)), value=int(m.group(2), 16), size=m.group(3), type=m.group(4),
                            bind=m.group(5), vis=m.group(6), ndx=m.group(7), name=m.group(8)))
    return res

def symtab(path):
    t = readelf(path, ['-s'])
    res = []
    sect = None
    for line in t.splitlines():
        if line.startswith("Symbol table '"):
            sect = line.split("'")[1]
        m = re.match(r'\s*(\d+):\s+([0-9a-f]+)\s+(\S+)\s+(<OS specific>: \d+|\S+)\s+(\S+)\s+(\S+)\s+(\S+)\s*(.*)$', line)
        if m and sect == '.symtab':
            res.append(dict(value=int(m.group(2), 16), type=m.group(4), bind=m.group(5), vis=m.group(6),
                            ndx=m.group(7), name=m.group(8)))
    return res

def dump(path, dest):
    """keep the full readelf output of an output for the reviewer."""
    wfile(dest, readelf(path, ['-hlSdr', '--dyn-syms', '-V']))

def tagnames(path):
    return [t for t, v in dynamic(path)]

def dyn_summary(path):
    """tag order with NULLs collapsed to NULLxN"""
    tags = tagnames(path)
    out = []
    for t in tags:
        if out and out[-1][0] == t and t == 'NULL':
            out[-1][1] += 1
        elif out and out[-1][0] == t and t == 'NEEDED':
            out[-1][1] += 1
        else:
            out.append([t, 1])
    return ' '.join(t if n == 1 else f'{t}x{n}' for t, n in out)

def dynval(path, tag):
    return [v for t, v in dynamic(path) if t == tag]

def first_line(s):
    for l in s.splitlines():
        if l.strip() and 'collect2' not in l:
            return l.strip()
    return s.strip()

def dyn_counts(path):
    """(entries in the .dynamic section by sh_size/sh_entsize, DT_NULL entries) read from the bytes."""
    secs = [s for s in sections(path) if s['name'] == '.dynamic']
    if not secs:
        return None
    s = secs[0]
    data = open(path, 'rb').read()[s['off']:s['off'] + s['size']]
    es = s['es']
    w = es // 2
    n = s['size'] // es
    nulls = sum(1 for i in range(n) if int.from_bytes(data[i*es:i*es+w], 'little') == 0)
    return n, nulls
