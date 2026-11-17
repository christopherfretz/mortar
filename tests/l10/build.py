"""L10's builds: configurations, the content-addressed object cache, the
captured link line, and the work directory's inputs (WP-067).

A configuration is `<compiler>-<arch>-<O0|O2>-<g|nog>-<fs|nofs>-<static|pie>`:
compiler gcc | clang | kiln (kiln++ with hod, C++ units only; its C units
by the target's gcc, Fortran by gfortran), arch a64 | x64 | i386 (the
non-host ones run under qemu), `fs` = `-ffunction-sections
-fdata-sections`.

Objects are compiled once per (compiler command, flags, unit text,
headers' text) into WORK/cache/<sha256>.o, each in a directory of its
own named by the key (so an object's DWARF names the same directory
every time it is built): a sequence costs one compile per changed unit,
and the same unit text is never compiled twice across sequences.
"""

import hashlib
import os
import platform
import shutil
import subprocess

HOST = {'aarch64': 'a64', 'arm64': 'a64', 'x86_64': 'x64', 'amd64': 'x64'}.get(platform.machine())
TRIPLE = {'a64': 'aarch64-linux-gnu', 'x64': 'x86_64-linux-gnu', 'i386': 'i686-linux-gnu'}
QEMU = {'a64': 'qemu-aarch64', 'x64': 'qemu-x86_64', 'i386': 'qemu-i386'}
COMPILE_TIMEOUT = 300
LINK_TIMEOUT = 120
RUN_TIMEOUT = 60


def which(name):
    if os.path.isabs(name):
        return name if os.access(name, os.X_OK) else None
    return shutil.which(name)


class Cfg:
    def __init__(self, name):
        parts = name.split('-')
        if len(parts) != 6:
            raise ValueError('configuration %r: want <compiler>-<arch>-<O0|O2>-<g|nog>-<fs|nofs>-<static|pie>' % name)
        self.compiler, self.arch, self.opt, g, fs, self.link = parts
        if self.compiler not in ('gcc', 'clang', 'kiln') or self.arch not in TRIPLE or self.opt not in ('O0', 'O2') \
                or g not in ('g', 'nog') or fs not in ('fs', 'nofs') or self.link not in ('static', 'pie'):
            raise ValueError('configuration %r: unknown part' % name)
        self.g, self.fs = g == 'g', fs == 'fs'
        self.name = name
        self.fortran = which(self.tool('gfortran')) is not None

    def tool(self, name):
        return name if self.arch == HOST else '%s-%s' % (TRIPLE[self.arch], name)

    def kiln_env(self):
        kiln, hod = os.environ.get('MASON_KILN'), os.environ.get('MASON_HOD')
        env = dict(os.environ)
        env.update({'KILN_BACKEND': 'hod', 'KILN_HOD': hod or '', 'KILN_HOD_STRICT': '1',
                    'PATH': os.path.dirname(hod or '/') + os.pathsep + env.get('PATH', '')})
        return kiln, env

    def missing(self):
        """Why this configuration cannot run here, or None."""
        if HOST is None:
            return 'unknown host architecture %s' % platform.machine()
        need = [self.tool('gcc'), self.tool('g++'), self.tool('ar')]
        if self.compiler == 'clang':
            need += ['clang', 'clang++']
        if self.compiler == 'kiln':
            kiln, env = self.kiln_env()
            if not kiln or not os.path.isfile(kiln) or not os.path.isfile(env['KILN_HOD']):
                return 'kiln/hod: set MASON_KILN and MASON_HOD'
            if self.arch == 'i386':
                return 'kiln has no i386 target'
            if self.opt != 'O0':
                return 'kiln+hod: -O0 only (IN-63)'
            if HOST != 'a64':
                return 'kiln runs on an AArch64 host here'
        if self.arch != HOST:
            need.append(QEMU[self.arch])
        for t in need:
            if which(t) is None:
                return '%s not found' % t
        return None

    def flags(self, lang):
        f = ['-' + self.opt]
        if self.g:
            f.append('-g')
        if self.fs:
            f += ['-ffunction-sections', '-fdata-sections']
        if self.link == 'pie':
            f.append('-fPIE')
        if lang == 'c++':
            f.append('-std=c++17')
        elif lang == 'c':
            f.append('-std=gnu11')
        return f

    def compiler_for(self, lang):
        """(argv prefix, env or None) of the compiler of `lang`."""
        t = '--target=' + TRIPLE[self.arch]
        if lang == 'fortran':
            return [self.tool('gfortran')], None
        if self.compiler == 'kiln' and lang == 'c++':
            kiln, env = self.kiln_env()
            return [kiln] + ([] if self.arch == HOST else [t]), env
        if self.compiler == 'clang':
            return ['clang++' if lang == 'c++' else 'clang', t], None
        return [self.tool('g++' if lang == 'c++' else 'gcc')], None

    def driver(self):
        """The link driver whose line is captured."""
        if self.compiler == 'kiln':
            return self.compiler_for('c++')
        return self.compiler_for('c++') if self.compiler == 'clang' else ([self.tool('g++')], None)

    def runner(self):
        """argv prefix to run a program of this configuration."""
        if self.arch == HOST:
            return []
        q = [QEMU[self.arch]]
        if self.link == 'pie':
            q += ['-L', '/usr/' + TRIPLE[self.arch]]
        return q


def sh(argv, cwd, env=None, timeout=COMPILE_TIMEOUT):
    p = subprocess.run(argv, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                       timeout=timeout, errors='replace')
    if p.returncode != 0:
        raise RuntimeError('%s (in %s): exit %d\n%s' % (' '.join(argv), cwd, p.returncode, p.stdout[-3000:]))
    return p.stdout


def object_key(cfg, unit, lang, text, headers, extra):
    argv, _env = cfg.compiler_for(lang)
    h = hashlib.sha256()
    h.update(repr((argv, cfg.flags(lang), extra, unit, text, sorted(headers.items()))).encode())
    return h.hexdigest()


class Cache:
    def __init__(self, work):
        self.dir = os.path.join(work, 'cache')
        os.makedirs(self.dir, exist_ok=True)
        self.compiled = 0
        self.hits = 0

    def path(self, key):
        return os.path.join(self.dir, key[:2], key + '.o')

    def have(self, key):
        return os.path.exists(self.path(key))

    def compile(self, cfg, unit, fname, lang, text, headers, extra, key):
        """Compiles one unit into the cache (idempotent; safe in parallel:
        the object is renamed into place)."""
        out = self.path(key)
        if os.path.exists(out):
            self.hits += 1
            return out
        d = os.path.join(self.dir, 'src', key)
        os.makedirs(d, exist_ok=True)
        for n, t in list(headers.items()) + [(fname, text)]:
            with open(os.path.join(d, n), 'w') as f:
                f.write(t)
        argv, env = cfg.compiler_for(lang)
        tmp = os.path.join(d, unit + '.o')
        sh(argv + cfg.flags(lang) + list(extra) + ['-c', fname, '-o', tmp], d, env)
        os.makedirs(os.path.dirname(out), exist_ok=True)
        os.replace(tmp, out + '.%d.tmp' % os.getpid())
        os.replace(out + '.%d.tmp' % os.getpid(), out)
        shutil.rmtree(d)
        self.compiled += 1
        return out


def capture(work, cfg):
    """The linker's words for this configuration, captured once from the
    driver through a `-B` wrapper: (words before the inputs, words after),
    with `-o` and its value removed. The placeholder input is l10-inputs.o."""
    d = os.path.join(work, 'capture', cfg.name)
    saved = os.path.join(d, 'ld.args')
    if not os.path.exists(saved):
        os.makedirs(os.path.join(d, 'bin'), exist_ok=True)
        wrapper = os.path.join(d, 'bin', 'ld')
        with open(wrapper, 'w') as f:
            f.write('#!/bin/sh\nfor a in "$@"; do printf "%s\\n" "$a"; done > "$L10_CAPTURE.tmp"\n')
        os.chmod(wrapper, 0o755)
        for ld in ('ld.lld', 'ld.bfd'):
            p = os.path.join(d, 'bin', ld)
            if not os.path.lexists(p):
                os.symlink('ld', p)
        open(os.path.join(d, 'l10-inputs.o'), 'wb').close()
        argv, env = cfg.driver()
        env = dict(env or os.environ)
        env['L10_CAPTURE'] = saved
        link = ['-static'] if cfg.link == 'static' else ['-pie']
        sh(argv + link + ['-B' + os.path.join(d, 'bin') + '/', 'l10-inputs.o', '-o', 'l10-out'], d, env)
        os.replace(saved + '.tmp', saved)
    words = open(saved).read().split('\n')[:-1]
    out = []
    i = 0
    while i < len(words):
        if words[i] == '-o':
            i += 2
            continue
        if words[i].startswith('-L') and words[i].endswith('/bin/') and 'capture' in words[i]:
            i += 1
            continue
        out.append(words[i])
        i += 1
    at = out.index('l10-inputs.o')
    return out[:at], out[at + 1:]


def write_if_changed(path, data, inplace=False):
    """Writes `data` at `path` unless it already holds it (an unchanged
    input keeps its stamp, as a build system leaves it). A new file is
    written beside and renamed over (a new inode, as a compiler writes),
    unless `inplace` and the sizes agree: then the same inode is rewritten.
    Returns True when the file changed."""
    try:
        with open(path, 'rb') as f:
            old = f.read()
    except OSError:
        old = None
    if old == data:
        return False
    if inplace and old is not None and len(old) == len(data):
        with open(path, 'r+b') as f:
            f.write(data)
        return True
    tmp = path + '.l10tmp'
    with open(tmp, 'wb') as f:
        f.write(data)
    os.replace(tmp, path)
    return True


def build_archive(cfg, stage, name, members, objs, thin, wdir):
    """The archive's bytes: members [(unit, object path)] in order. A thin
    archive names its members' files in `wdir` (they must be installed
    there first); a regular one is built from links in `stage`."""
    ar = cfg.tool('ar')
    if thin:
        tmp = os.path.join(wdir, name + '.l10new')
        if os.path.exists(tmp):
            os.remove(tmp)
        sh([ar, 'rcsDT', name + '.l10new'] + [m + '.o' for m in members], wdir)
    else:
        sd = os.path.join(stage, name)
        if os.path.isdir(sd):
            shutil.rmtree(sd)
        os.makedirs(sd)
        for m in members:
            os.symlink(objs[m], os.path.join(sd, m + '.o'))
        tmp = os.path.join(sd, name)
        sh([ar, 'rcsD', name] + [m + '.o' for m in members], sd)
    with open(tmp, 'rb') as f:
        data = f.read()
    os.remove(tmp)
    return data
