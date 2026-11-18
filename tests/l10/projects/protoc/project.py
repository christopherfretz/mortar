"""protoc: the bench's protoc as an L10 project (spec/incr.md IN-63
"Projects"; WP-067), only by name: `cargo xtask l10 --project protoc`.

The objects are the bench's own (`.build/bench-work/workloads/
cxx-medium-g-static/build`: protobuf 3.21.12, g++ -O0 -g, AArch64,
linked -static), read in place and never written: `main.cc.o` and the
members of `libprotoc.a` and `libprotobuf.a`, which are byte for byte the
CMake objects under `CMakeFiles/<lib>.dir/` (prepare() maps each member,
in archive order, to its object and source, once, into WORK/protoc/
index.json). An edit recompiles ONE source of the bench's tree into the
lane's object cache with the bench's flags (`flags.make`) plus an appended
function `l10_edit_<unit>` whose constant is the edit's value: the first
edit of a file changes its size (and its DWARF's directory), later values
are the same size. The archives are rebuilt from the objects with `ar
rcsD` when a member changes (member names kept, duplicates included).
The program's transcript is `protoc --version`.

Only the configuration `gcc-a64-O0-g-nofs-static` on an AArch64 host,
and only when the bench has prepared its workload; otherwise skipped by
name (`missing`).

Knobs: edit.N (0: the bench's object; 1..7: recompiled with the edit),
N = 0 main.o, then libprotoc.a's members, then libprotobuf.a's;
ar_order.0/1 (an archive's members rotated by a third); gc
(--gc-sections).
"""

import json
import os
import shlex

NAME = 'protoc'
CONFIG = 'gcc-a64-O0-g-nofs-static'
WORKLOAD = 'cxx-medium-g-static'
LIBS = ('libprotoc.a', 'libprotobuf.a')
PREFIX = {'libprotoc.a': 'c', 'libprotobuf.a': 'b'}     # unit names: c000.., b000..
RUN_ARGS = ('--version',)
BODY = (0x1235, 0x2a57, 0x3c91, 0x4d1b, 0x5e63, 0x6f29, 0x70b5, 0x31cd)
GROW = (0,)

_INDEX = None


def bench_build():
    """The bench's build tree of protoc, or None."""
    root = os.environ.get('L10_BENCH_WORK')
    if not root:
        d = os.path.dirname(os.path.abspath(__file__))
        while d != os.path.dirname(d):
            if os.path.isdir(os.path.join(d, '.build', 'bench-work')):
                root = os.path.join(d, '.build', 'bench-work')
                break
            d = os.path.dirname(d)
    if not root:
        return None
    b = os.path.join(root, 'workloads', WORKLOAD, 'build')
    return b if os.path.isfile(os.path.join(b, 'libprotoc.a')) else None


def missing(cfg):
    if cfg.name != CONFIG:
        return 'protoc runs only as the bench built it (%s)' % CONFIG
    if bench_build() is None:
        return 'the bench has not prepared %s (.build/bench-work/workloads/%s/build)' % (WORKLOAD, WORKLOAD)
    return None


def ar_members(path):
    """[(name, offset of the data, size)] of a regular archive, in order."""
    out = []
    with open(path, 'rb') as f:
        if f.read(8) != b'!<arch>\n':
            raise ValueError('%s: not a regular archive' % path)
        longnames = b''
        while True:
            h = f.read(60)
            if len(h) < 60:
                break
            name, size = h[:16].decode().rstrip(), int(h[48:58].decode())
            off = f.tell()
            if name == '//':
                longnames = f.read(size)
            elif name.startswith('/') and name[1:].isdigit():
                i = int(name[1:])
                out.append((longnames[i:longnames.index(b'/\n', i)].decode(), off, size))
            elif name != '/':
                out.append((name.rstrip('/'), off, size))
            f.seek(off + size + (size & 1))
    return out


def _flags(build, target):
    """The compile flags of a CMake target (flags.make)."""
    v = {}
    with open(os.path.join(build, 'CMakeFiles', target + '.dir', 'flags.make')) as f:
        for line in f:
            if ' = ' in line and not line.startswith('#'):
                key, val = line.rstrip('\n').split(' = ', 1)
                v[key] = shlex.split(val)
    return v.get('CXX_DEFINES', []) + v.get('CXX_INCLUDES', []) + v.get('CXX_FLAGS', [])


def _source_root(build):
    inc = [a[2:] for a in _flags(build, 'libprotobuf') if a.startswith('-I') and a.endswith('/src')]
    return os.path.dirname(inc[0])


def prepare(work):
    """WORK/protoc/index.json: the units in order, each with its object in
    the bench, its source and its flags. Read only in the bench."""
    global _INDEX
    if _INDEX is not None:
        return
    path = os.path.join(work, 'protoc', 'index.2.json')
    build = bench_build()
    if build is None:
        return
    if os.path.exists(path):
        with open(path) as f:
            _INDEX = json.load(f)
        return
    root = _source_root(build)
    units = [{'unit': 'main', 'lib': None, 'name': 'main.cc.o',
              'obj': os.path.join(build, 'CMakeFiles/protoc.dir/src/google/protobuf/compiler/main.cc.o'),
              'src': os.path.join(root, 'src/google/protobuf/compiler/main.cc'), 'flags': _flags(build, 'protoc')}]
    for lib in LIBS:
        target = lib[:-2]
        tdir = os.path.join(build, 'CMakeFiles', target + '.dir')
        by_name = {}
        for d, _dirs, files in os.walk(tdir):
            for fn in files:
                if fn.endswith('.o'):
                    by_name.setdefault(fn, []).append(os.path.join(d, fn))
        flags = _flags(build, target)
        with open(os.path.join(build, lib), 'rb') as f:
            for i, (name, off, size) in enumerate(ar_members(os.path.join(build, lib))):
                cands = [c for c in by_name.get(name, []) if os.path.getsize(c) == size]
                if len(cands) > 1:
                    f.seek(off)
                    data = f.read(size)
                    cands = [c for c in cands if open(c, 'rb').read() == data]
                if len(cands) != 1:
                    raise RuntimeError('protoc: %s member %d %s: %d matching objects' % (lib, i, name, len(cands)))
                rel = os.path.relpath(cands[0], tdir)[:-2]       # src/google/protobuf/....cc
                units.append({'unit': '%s%03d' % (PREFIX[lib], i), 'lib': lib, 'name': name, 'obj': cands[0],
                              'src': os.path.join(root, rel), 'flags': flags})
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path + '.tmp', 'w') as f:
        json.dump(units, f, indent=0)
    os.replace(path + '.tmp', path)
    _INDEX = units


def _index():
    if _INDEX is None:
        raise RuntimeError('protoc: prepare(work) was not called')
    return _INDEX


def knobs():
    k = {}
    for i in range(len(_index())):
        k['edit.%d' % i] = 0
    k.update({'ar_order.0': 0, 'ar_order.1': 0, 'gc': False})
    return k


def member_names():
    return {u['unit']: u['name'] for u in _index()}


def all_units(k):
    return [u['unit'] for u in _index()]


def _edited(k):
    return [(i, u) for i, u in enumerate(_index()) if k['edit.%d' % i]]


def sources(k, cfg):
    units = {}
    for i, u in _edited(k):
        with open(u['src']) as f:
            text = f.read()
        text += '\n// l10: an edit (tests/l10/projects/protoc)\nextern "C" int l10_edit_%s(void) { return %#x; }\n' % (
            u['unit'], BODY[k['edit.%d' % i]])
        units[u['unit']] = (os.path.basename(u['src']), 'c++', text)
    return units, {}


def unit_flags(name):
    u = next(u for u in _index() if u['unit'] == name)
    # the bench's flags last (its -std wins); quoted includes beside the source
    return list(u['flags']) + ['-iquote', os.path.dirname(u['src'])]


def units_needed(k, cfg):
    return [u['unit'] for _i, u in _edited(k)]


def prebuilt(k, cfg):
    return {u['unit']: u['obj'] for u in _index()}


def plan(k, cfg):
    idx = _index()
    inputs = [('o', 'main')]
    for a, lib in enumerate(LIBS):
        members = [u['unit'] for u in idx if u['lib'] == lib]
        r = k['ar_order.%d' % a] * len(members) // 3
        inputs.append(('a', lib, members[r:] + members[:r], False))
    return inputs, (['--gc-sections'] if k['gc'] else [])
