#!/usr/bin/env python3
"""WP-110 census part 1: the linker lines compiler drivers produce for
`-shared`, captured through -B recording `ld` wrappers, linked by GNU ld
(the wrapper execs it), loaded by a GNU-ld-linked host with dlopen and run.

Run as: python3 -I census.py WORK SRCDIR MORTAR
Serial only.  Writes into WORK only.
"""
import json, os, re, shlex, shutil, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
WORK = os.path.abspath(sys.argv[1])
SRCD = os.path.abspath(sys.argv[2]) if len(sys.argv) > 2 else ''
MORTAR = os.path.abspath(sys.argv[3]) if len(sys.argv) > 3 else ''
ARCHES = ['aarch64', 'x86_64', 'i686']
TRIPLE = {'aarch64': 'aarch64-linux-gnu', 'x86_64': 'x86_64-linux-gnu', 'i686': 'i686-linux-gnu'}
QEMU = {'aarch64': [], 'x86_64': ['qemu-x86_64', '-L', '/usr/x86_64-linux-gnu'],
        'i686': ['qemu-i386', '-L', '/usr/i686-linux-gnu']}
ENV = dict(os.environ)
ENV['LC_ALL'] = 'C'
ENV.pop('CENSUS_ARGS', None)
ENV.pop('CENSUS_ARGS_DIR', None)
ENV['TMPDIR'] = os.path.join(WORK, 'tmp')


def run(cmd, env=None, cwd=None, timeout=120):
    try:
        p = subprocess.run(cmd, env=env or ENV, cwd=cwd or WORK, capture_output=True,
                           text=True, errors='replace', timeout=timeout)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return 'timeout', '', ''
    except FileNotFoundError as e:
        return 'notfound', '', str(e)


def w(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, 'w') as f:
        f.write(text)


DRIVERS = []  # (id, arch, lang, argv-prefix, family)
for arch in ARCHES:
    pre = '' if arch == 'aarch64' else TRIPLE[arch] + '-'
    DRIVERS.append((f'gcc/{arch}/c', arch, 'c', [pre + 'gcc'], 'gcc'))
    DRIVERS.append((f'gcc/{arch}/c++', arch, 'c++', [pre + 'g++'], 'gcc'))
    DRIVERS.append((f'gcc/{arch}/fortran', arch, 'fortran', [pre + 'gfortran'], 'gcc'))
for v in ['19', '22']:
    for arch in ARCHES:
        tgt = [] if arch == 'aarch64' else ['--target=' + TRIPLE[arch]]
        DRIVERS.append((f'clang{v}/{arch}/c', arch, 'c', ['clang-' + v] + tgt, 'clang'))
        DRIVERS.append((f'clang{v}/{arch}/c++', arch, 'c++', ['clang++-' + v] + tgt, 'clang'))

SRC = {'c': 'lib.c', 'c++': 'lib.cpp', 'fortran': 'lib.f90'}
CFLAV = {'pic': ['-fPIC'], 'nopic': [], 'fno-pic': ['-fno-pic']}
VMAP = os.path.join(SRCD, 'v.map')
DLIST = os.path.join(SRCD, 'dyn.list')
# (id, compile flavour, driver args before the object, extra inputs after it, langs)
VARIATIONS = [
    ('default', 'pic', [], [], None),
    ('nopic', 'nopic', [], [], None),
    ('fno-pic', 'fno-pic', [], [], None),
    ('soname', 'pic', ['-Wl,-soname,libx.so.1'], [], None),
    ('version-script', 'pic', ['-Wl,--version-script=' + VMAP], [], None),
    ('Bsymbolic', 'pic', ['-Wl,-Bsymbolic'], [], None),
    ('Bsymbolic-functions', 'pic', ['-Wl,-Bsymbolic-functions'], [], None),
    ('dynamic-list', 'pic', ['-Wl,--dynamic-list=' + DLIST], [], None),
    ('archive', 'pic', ['-Wl,-u,ar_func'], ['AR'], None),
    ('exclude-libs', 'pic', ['-Wl,--exclude-libs,ALL', '-Wl,-u,ar_func'], ['AR'], None),
    ('undef', 'pic', [], ['UNDEF'], None),
    ('z-defs', 'pic', ['-Wl,-z,defs'], [], None),
    ('z-defs+undef', 'pic', ['-Wl,-z,defs'], ['UNDEF'], None),
    ('no-undefined', 'pic', ['-Wl,--no-undefined'], [], None),
    ('no-undefined+undef', 'pic', ['-Wl,--no-undefined'], ['UNDEF'], None),
    ('z-nodelete', 'pic', ['-Wl,-z,nodelete'], [], None),
    ('z-initfirst', 'pic', ['-Wl,-z,initfirst'], [], None),
    ('z-interpose', 'pic', ['-Wl,-z,interpose'], [], None),
    ('z-now', 'pic', ['-Wl,-z,now'], [], None),
    ('z-relro', 'pic', ['-Wl,-z,relro'], [], None),
    ('as-needed', 'pic', ['-Wl,--as-needed'], [], None),
    ('rpath', 'pic', ['-Wl,-rpath,/x'], [], None),
    ('rpath+new-dtags', 'pic', ['-Wl,-rpath,/x', '-Wl,--enable-new-dtags'], [], None),
    ('rpath+old-dtags', 'pic', ['-Wl,-rpath,/x', '-Wl,--disable-new-dtags'], [], None),
    ('init-fini', 'pic', ['-Wl,-init,my_init', '-Wl,-fini,my_fini'], [], None),
    ('gc-sections', 'pic', ['-Wl,--gc-sections'], [], None),
    ('export-dynamic', 'pic', ['-Wl,--export-dynamic'], [], None),
    ('rdynamic', 'pic', ['-rdynamic'], [], None),
    ('nostdlib', 'pic', ['-nostdlib'], [], None),
    ('static-libgcc', 'pic', ['-static-libgcc'], [], None),
    ('shared-libgcc', 'pic', ['-shared-libgcc'], [], None),
    ('static-libstdc++', 'pic', ['-static-libstdc++'], [], ('c++',)),
    ('pthread', 'pic', ['-pthread'], [], None),
    ('static', 'pic', ['-static'], [], None),
    ('static-pie', 'pic', ['-static-pie'], [], None),
]

ARITY2 = {'-plugin', '-dynamic-linker', '-m', '-o', '-z', '-rpath', '-e', '-soname', '-h',
          '-T', '-y', '-Map', '-rpath-link', '-L', '-l', '-I', '--sysroot', '-u', '-init', '-fini'}


def tokens(args):
    out, i = [], 0
    while i < len(args):
        a = args[i]
        if a.startswith('-') and a != '-':
            if a in ARITY2 and i + 1 < len(args):
                out.append(('opt', [a, args[i + 1]])); i += 2; continue
            out.append(('opt', [a])); i += 1; continue
        out.append(('file', [a])); i += 1
    return out


def optkey(raw):
    a = raw[0]
    if a == '-plugin':
        return '-plugin PLUGIN'
    if a.startswith('-plugin-opt='):
        return '-plugin-opt=...'
    if a == '-o':
        return '-o FILE'
    if a.startswith('-L'):
        return '-L DIR'
    if a.startswith('-l'):
        return '-l' + (raw[1] if a == '-l' else a[2:])
    if a in ('-z', '-m', '-dynamic-linker', '-init', '-fini', '-u'):
        return a + ' ' + raw[1]
    if a in ('-rpath', '-soname', '-h'):
        return a + ' ARG'
    if a.startswith('--version-script') or a.startswith('--dynamic-list'):
        return a.split('=')[0] + '=FILE'
    return a


def filekey(f):
    b = os.path.basename(f)
    if b.startswith('obj.') or b in ('libcensusar.a', 'undef.o'):
        return {'libcensusar.a': 'AR', 'undef.o': 'UNDEF'}.get(b, 'OBJ')
    return b


def setup():
    for d in ('obj', 'lines', 'out', 'mortar', 'wrap', 'hashcheck', 'host', 'tmp'):
        os.makedirs(os.path.join(WORK, d), exist_ok=True)
    for arch in ARCHES:
        d = os.path.join(WORK, 'wrap', arch)
        os.makedirs(d, exist_ok=True)
        names = ['ld', 'ld.bfd']
        for t in TRIPLE.values():
            names += [t + '-ld', t + '-ld.bfd']
        for n in names:
            p = os.path.join(d, n)
            shutil.copy(os.path.join(HERE, 'wrap_ld.sh'), p)
            os.chmod(p, 0o755)
    vers = []
    for t in ['gcc', 'g++', 'gfortran', 'x86_64-linux-gnu-gcc', 'x86_64-linux-gnu-g++',
              'x86_64-linux-gnu-gfortran', 'i686-linux-gnu-gcc', 'i686-linux-gnu-g++',
              'i686-linux-gnu-gfortran', 'clang-19', 'clang-22', 'ld.bfd',
              'x86_64-linux-gnu-ld.bfd', 'i686-linux-gnu-ld.bfd', 'qemu-x86_64', 'qemu-i386',
              'readelf']:
        rc, o, e = run([t, '--version'])
        vers.append(f'{t}: {(o or e).splitlines()[0] if (o or e) else rc}')
    rc, o, e = run([MORTAR, '--version'])
    vers.append(f'mortar {MORTAR}: {(o or e).strip()}')
    w(os.path.join(WORK, 'versions.log'), '\n'.join(vers) + '\n')


def compile_all():
    log = []
    for arch in ARCHES:
        pre = '' if arch == 'aarch64' else TRIPLE[arch] + '-'
        cc = pre + 'gcc'
        for cmd in ([cc, '-fPIC', '-c', os.path.join(SRCD, 'undef.c'), '-o', f'obj/{arch}.undef/undef.o'],
                    [cc, '-fPIC', '-c', os.path.join(SRCD, 'ar.c'), '-o', f'obj/{arch}.ar.o'],
                    ['ar', 'rcs', f'obj/{arch}.ar/libcensusar.a', f'obj/{arch}.ar.o'],
                    [cc, os.path.join(SRCD, 'host.c'), '-o', f'host/host.{arch}']):
            for d in (f'obj/{arch}.undef', f'obj/{arch}.ar', 'host'):
                os.makedirs(os.path.join(WORK, d), exist_ok=True)
            rc, o, e = run(cmd)
            log.append(f'rc={rc} {" ".join(cmd)}\n{e.strip()}')
    for did, arch, lang, drv, fam in DRIVERS:
        for fl, flags in CFLAV.items():
            o = os.path.join(WORK, 'obj', 'obj.' + did.replace('/', '_') + '.' + fl + '.o')
            cmd = drv + flags + ['-c', os.path.join(SRCD, SRC[lang]), '-o', o]
            rc, out, err = run(cmd, cwd=os.path.join(WORK, 'obj'))
            log.append(f'{did} {fl} rc={rc} {" ".join(cmd)}\n{err.strip()}')
    w(os.path.join(WORK, 'compile.log'), '\n'.join(log) + '\n')


def norm_args(args, arch):
    wd = os.path.join(WORK, 'wrap', arch)
    out = []
    for a in args:
        if a in ('-L' + wd, '-L' + wd + '/'):
            continue
        a = re.sub(r'-fresolution=\S+\.res', '-fresolution=TMP.res', a)
        out.append(a)
    return out


def elf_facts(path):
    f = {}
    rc, h, _ = run(['readelf', '-hdlW', path])
    m = re.search(r'Type:\s+(\S+)', h); f['e_type'] = m.group(1) if m else '?'
    f['interp'] = (re.search(r'Requesting program interpreter: ([^\]]+)\]', h) or [None, ''])[1]
    f['phdrs'] = sorted(set(re.findall(r'^\s+(LOAD|DYNAMIC|INTERP|GNU_RELRO|GNU_EH_FRAME|GNU_STACK|NOTE|TLS|PHDR|GNU_PROPERTY)\s', h, re.M)))
    f['tags'] = re.findall(r'^\s*0x[0-9a-f]+\s+\((\w+)\)', h, re.M)
    f['soname'] = (re.search(r'\(SONAME\)\s+Library soname: \[([^\]]+)\]', h) or [None, ''])[1]
    f['needed'] = re.findall(r'\(NEEDED\)\s+Shared library: \[([^\]]+)\]', h)
    f['runpath'] = re.findall(r'\((RUNPATH|RPATH)\)\s+Library \w+: \[([^\]]+)\]', h)
    f['flags'] = (re.search(r'\(FLAGS\)\s+(.*)', h) or [None, ''])[1].strip()
    f['flags_1'] = (re.search(r'\(FLAGS_1\)\s+Flags:\s*(.*)', h) or [None, ''])[1].strip()
    rc, d, _ = run(['readelf', '--dyn-syms', '-W', path])
    defs, undefs = [], []
    for l in d.splitlines():
        m = re.match(r'\s+\d+:\s+\S+\s+\d+\s+(\w+)\s+(\w+)\s+(\w+)\s+(\S+)\s+(\S+)', l)
        if not m:
            continue
        name = m.group(5)
        if m.group(4) == 'UND':
            undefs.append(name)
        else:
            defs.append(name)
    f['dyn_defs'] = defs
    f['dyn_undefs'] = undefs
    rc, v, _ = run(['readelf', '-VW', path])
    f['verdefs'] = re.findall(r'Rev: \d+\s+Flags: (\S+)\s+Index: \d+\s+Cnt: \d+\s+Name: (\S+)', v)
    rc, n, _ = run(['readelf', '-nW', path])
    f['build_id'] = 'NT_GNU_BUILD_ID' in n
    return f


def run_host(arch, so):
    rc, o, e = run(QEMU[arch] + [os.path.join(WORK, 'host', 'host.' + arch), so], timeout=60)
    return {'rc': rc, 'out': o.strip().replace('\n', ' | '), 'err': e.strip()[:300]}


def capture():
    res = {}
    for did, arch, lang, drv, fam in DRIVERS:
        for vid, fl, vargs, extra, langs in VARIATIONS:
            if langs and lang not in langs:
                continue
            lid = did.replace('/', '_') + '.' + vid
            obj = os.path.join(WORK, 'obj', 'obj.' + did.replace('/', '_') + '.' + fl + '.o')
            ex = [os.path.join(WORK, 'obj', f'{arch}.ar/libcensusar.a') if x == 'AR'
                  else os.path.join(WORK, 'obj', f'{arch}.undef/undef.o') for x in extra]
            out = os.path.join(WORK, 'out', lid + '.so')
            argsf = os.path.join(WORK, 'lines', lid + '.args')
            env = dict(ENV)
            env['CENSUS_ARGS'] = argsf
            cmd = drv + ['-B' + os.path.join(WORK, 'wrap', arch) + '/', '-shared'] + vargs + [obj] + ex + ['-o', out]
            rc, o, e = run(cmd, env=env)
            r = {'id': lid, 'driver': did, 'arch': arch, 'lang': lang, 'fam': fam, 'var': vid,
                 'cmd': cmd, 'link_rc': rc, 'link_err': (o + e).strip()[-1500:]}
            if os.path.exists(argsf):
                raw = open(argsf).read().splitlines()
                r['ld_name'] = raw[0]
                r['args'] = norm_args(raw[1:], arch)
            else:
                rc2, o2, e2 = run(drv + ['-###', '-shared'] + vargs + [obj] + ex + ['-o', out])
                r['ld_name'] = 'NOT-WRAPPED'
                r['driver_err'] = e2.strip()[-600:]
            if rc == 0 and os.path.exists(out):
                r['elf'] = elf_facts(out)
                r['run'] = run_host(arch, out)
                r['size'] = os.path.getsize(out)
                if vid != 'default':
                    os.remove(out)  # keep the area small; run.sh re-creates every output
            res[lid] = r
    return res


def hash_check(res):
    """The same line by -### and by the wrapper, for one gcc and one clang case per arch."""
    log = []
    for did, arch, lang, drv, fam in DRIVERS:
        if not (did.endswith('/c') and (fam == 'gcc' or did.startswith('clang22'))) and did != 'gcc/aarch64/c++':
            continue
        lid = did.replace('/', '_') + '.default'
        r = res[lid]
        obj = os.path.join(WORK, 'obj', 'obj.' + did.replace('/', '_') + '.pic.o')
        rc, o, e = run(drv + ['-B' + os.path.join(WORK, 'wrap', arch) + '/', '-###', '-shared', obj,
                              '-o', os.path.join(WORK, 'out', lid + '.so')])
        last = [l for l in e.splitlines() if l.startswith(' "') or l.startswith(' /')][-1]
        argv = norm_args(shlex.split(last), arch)
        prog, hargs = argv[0], argv[1:]
        same = hargs == r['args']
        log.append(f'== {did}: -### program {prog}; wrapper saw {r["ld_name"]}; args identical: {same}')
        if not same:
            import difflib
            for d in difflib.unified_diff(hargs, r['args'], '-###', 'wrapper', lineterm='', n=0):
                log.append('   ' + d)
    w(os.path.join(WORK, 'hashcheck.log'), '\n'.join(log) + '\n')


def mortar_runs(res):
    out = {}
    for lid, r in res.items():
        if 'args' not in r:
            continue
        args = list(r['args'])
        o = os.path.join(WORK, 'mortar', lid + '.so')
        for i, a in enumerate(args):
            if a == '-o':
                args[i + 1] = o
        rc, so, se = run([MORTAR] + args)
        out[lid] = {'rc': rc, 'stderr': se.strip()[:2000]}
    return out


def main():
    setup()
    compile_all()
    res = capture()
    hash_check(res)
    mres = mortar_runs(res)
    json.dump(res, open(os.path.join(WORK, 'capture.json'), 'w'), indent=1)
    json.dump(mres, open(os.path.join(WORK, 'mortar.json'), 'w'), indent=1)
    print('lines', len(res), 'wrapped', sum(1 for r in res.values() if 'args' in r),
          'linked', sum(1 for r in res.values() if r['link_rc'] == 0),
          'ran OK', sum(1 for r in res.values() if r.get('run', {}).get('rc') == 0))


if __name__ == '__main__':
    main()
