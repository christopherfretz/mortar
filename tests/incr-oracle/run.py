#!/usr/bin/env python3
"""run.py --mortar BIN --work DIR [--quick] [--mutations] [--only S,...] [-j N] [--keep] [--tool NAME=PATH]...

The two-full-link oracle lane of spec/incr.md IN-63a for mortar's dry run
(WP-063). For every scenario pair (old inputs -> new inputs):

  1. compile the scenario's sources (src/) for the pair's architecture and
     optimisation level, once per (cfg, arch);
  2. link the old inputs in a work directory W, with --incremental and
     --mortar-map (a full link that writes a state);
  3. snapshot W's program, map and inputs to A (the base link);
  4. replace in W the inputs whose bytes differ (as a build system would),
     and link again with --incremental-report=rep.txt: the dry run of
     W's state against the new inputs, then the full link;
  5. judge rep.txt with oracle.py (A against W): every unit that differs
     must be in the refill set, and the ratio of the set to the units
     that differ is printed (the over-approximation).

--mutations also judges every pair with each IN-63b row dropped from the
report (oracle.py ROWS, the rows of crates/mortar_incr/src/delta.rs) and
prints the mutation table: per row, the pairs that catch its absence.

A passing pair's directories are removed unless --keep; a failing
pair's stay under DIR/<cfg>/<scenario>/<old>-<new>/{a,w} for a look.

--quick runs the subset `cargo test -p mortar --test incr_oracle` runs
(QUICK below; O0 only); without it every pair at O0 and O2 (the nightly
set, `cargo xtask incr-oracle`).

Scenarios: R3's (the WP-060 review's refill oracle, its 28 pairs at O0
and O2: a as b0 b c cf d e f w l g on AArch64, dx fx wx lx on x86-64)
and WP-063's own, one per IN-27 row, on both architectures (an `x`
suffix is the x86-64 build): ob (body edit same size, grown, a new and a
removed function), os (a string literal), ot (a TLS variable added and
removed), oa (an archive member newly pulled, and dropped), ow (a symbol
becoming weak, hidden), oc (a common that grows), og (--gc-sections
liveness flips), oi (an inline function in a header: the COMDAT keeper
moves), ov (a vtable grows), oz (a static initializer added); and one per
fall-back trigger: or (two unchanged inputs swap), on (a new output
section), op (.note.gnu.property: branch protection on and off), ol (a
linker script input changes; mortar's scripts take no assignment yet).
"""
import os, sys, shutil, subprocess, platform, collections, concurrent.futures, time, hashlib

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, 'src')
sys.path.insert(0, HERE)
sys.dont_write_bytecode = True  # no __pycache__ in the source tree
import oracle  # noqa: E402

TRIPLE = {'a64': 'aarch64-linux-gnu', 'x64': 'x86_64-linux-gnu'}
HOST = {'aarch64': 'a64', 'arm64': 'a64', 'x86_64': 'x64', 'amd64': 'x64'}.get(platform.machine())
TOOLS = {}


def tool(arch, name):
    n = name if arch == HOST else '%s-%s' % (TRIPLE[arch], name)
    return TOOLS.get(n, n)


def r3_flags(n, arch):
    """R3's per-source flags (oracle_scen.sh), and the own scenarios'."""
    if n == 'o_pr0':
        return ['-mbranch-protection=none'] if arch == 'a64' else ['-fcf-protection=none']
    if n == 'o_pr1':
        return ['-mbranch-protection=standard'] if arch == 'a64' else ['-fcf-protection=full', '-mneeded', '-march=x86-64-v3']
    if n.startswith(('l_', 'g_')):
        return ['-ffunction-sections']
    if n == 'c_u':
        return ['-ffunction-sections', '-fno-exceptions', '-fno-asynchronous-unwind-tables', '-fno-unwind-tables']
    if n.startswith(('c_', 'cf_')):
        return ['-ffunction-sections']
    if n.startswith('e_l'):
        return ['-fdata-sections', '-fno-toplevel-reorder']
    if n.startswith('e_'):
        return ['-fdata-sections']
    if n.startswith(('o_gu', 'o_gc')):
        return ['-ffunction-sections', '-fdata-sections']
    if n.startswith(('o_cu', 'o_cm')):
        return ['-fcommon']
    return []


GC = ['-Wl,--gc-sections']
# scenario -> (arch, link flags, {variant: [(input name, source stem)]}, archives {name: [stems]})
SCEN = {}


def scen(name, arch, gc, variants, archives=None):
    SCEN[name] = (arch, gc, {v: [tuple(p.split('=')) for p in ps.split()] for v, ps in variants.items()}, archives or {})


# R3's (oracle_scen.sh `lk`, `lkx`)
scen('a', 'a64', [], {'base': 'm=a_m u1=a_u1 u2=a_u2 last=a_l0', 'same': 'm=a_m u1=a_u1 u2=a_u2 last=a_l1',
                      'grow': 'm=a_m u1=a_u1 u2=a_u2 last=a_l2'})
scen('as', 'a64', [], {'base': 'm=a_m u1=a_u1 u2=a_u2 last=as_l0', 'same': 'm=a_m u1=a_u1 u2=a_u2 last=as_l1',
                       'grow': 'm=a_m u1=a_u1 u2=a_u2 last=as_l2'})
scen('b0', 'a64', [], {'k_a': 'c=b0_c0 a=b0_a m=b0_m', 'k_c': 'c=b0_c1 a=b0_a m=b0_m'})
scen('b', 'a64', [], {'k_a': 'c=b_c0 a=b_a m=b_m', 'k_c': 'c=b_c1 a=b_a m=b_m'})
scen('c', 'a64', GC, {'live': 'm=c_m u=c_u last=c_l0', 'dead': 'm=c_m u=c_u last=c_l1'})
scen('cf', 'a64', GC, {'live': 'm=cf_m u=cf_u last=cf_l0', 'dead': 'm=cf_m u=cf_u last=cf_l1'})
scen('d', 'a64', [], {'base': 'c=d_c a=d_a0 b=d_b0 e=d_e d=d_d0', 'edit1': 'c=d_c a=d_a1 b=d_b0 e=d_e d=d_d0',
                      'edit2': 'c=d_c a=d_a0 b=d_b1 e=d_e d=d_d1'})
scen('e', 'a64', [], {'base': 'm=e_m u=e_u last=e_l0', 'moved': 'm=e_m u=e_u last=e_l1'})
scen('f', 'a64', [], {'v0': 'u=f_u c=f_c0', 'v1': 'u=f_u c=f_c1'})
scen('w', 'a64', [], {'v0': 'm=w_m u=w_u last=w_l0', 'v1': 'm=w_m u=w_u last=w_l1'})
scen('l', 'a64', GC, {'keep': 'm=l_m0 u=l_u', 'both': 'm=l_m1 u=l_u'})
scen('g', 'a64', GC, {'v0': 'm=g_m0 u=g_u', 'v1': 'm=g_m1 u=g_u'})
scen('dx', 'x64', [], {'base': 'c=d_c a=d_a0 b=d_b0 e=d_e d=d_d0', 'grow': 'c=d_c a=d_a0 b=d_b0 e=d_e d=d_d1'})
scen('fx', 'x64', [], {'v0': 'u=f_u c=fx_c0', 'v1': 'u=f_u c=fx_c1'})
scen('wx', 'x64', [], {'v0': 'm=w_m u=w_u last=w_l0', 'v1': 'm=w_m u=w_u last=w_l1'})
scen('lx', 'x64', GC, {'keep': 'm=l_m0 u=l_u', 'both': 'm=l_m1 u=l_u'})
R3_PAIRS = ('a/base:a/same a/base:a/grow as/base:as/same as/base:as/grow b0/k_a:b0/k_c b0/k_c:b0/k_a '
            'b/k_a:b/k_c b/k_c:b/k_a c/live:c/dead c/dead:c/live cf/live:cf/dead cf/dead:cf/live '
            'd/base:d/edit1 d/base:d/edit2 dx/base:dx/grow e/base:e/moved f/v0:f/v1 w/v0:w/v1 w/v1:w/v0 '
            'l/keep:l/both l/both:l/keep fx/v0:fx/v1 wx/v0:wx/v1 wx/v1:wx/v0 lx/keep:lx/both '
            'lx/both:lx/keep g/v0:g/v1 g/v1:g/v0').split()

# WP-063's own: one scenario per IN-27 row, on both architectures
OWN_PAIRS = []
for arch, sfx in (('a64', ''), ('x64', 'x')):
    def own(name, gc, variants, pairs, archives=None):
        scen(name + sfx, arch, gc, variants, archives)
        OWN_PAIRS.extend('%s/%s:%s/%s' % (name + sfx, a, name + sfx, b) for a, b in (p.split(':') for p in pairs.split()))
    own('ob', [], {'base': 'm=o_m u=o_u last=o_body0', 'same': 'm=o_m u=o_u last=o_body1',
                   'grow': 'm=o_m u=o_u last=o_body2', 'fn': 'm=o_m u=o_u last=o_fn1'},
        'base:same base:grow base:fn fn:base')
    own('os', [], {'v0': 'm=o_m u=o_su last=o_str0', 'v1': 'm=o_m u=o_su last=o_str1'}, 'v0:v1')
    own('ot', [], {'v0': 'm=o_m u=o_tu last=o_tls0', 'v1': 'm=o_m u=o_tu last=o_tls1'}, 'v0:v1 v1:v0')
    own('oa', [], {'v0': 'm=o_m u=o_u last=o_ar0 lib.a', 'v1': 'm=o_m u=o_u last=o_ar1 lib.a'}, 'v0:v1 v1:v0',
        {'lib.a': ['o_ap', 'o_aq']})
    own('ow', [], {'base': 'm=o_m u=o_wu last=o_wk0', 'weak': 'm=o_m u=o_wu last=o_wk1',
                   'hidden': 'm=o_m u=o_wu last=o_vis1'}, 'base:weak base:hidden')
    own('oc', [], {'v0': 'm=o_m u=o_cu last=o_cm0', 'v1': 'm=o_m u=o_cu last=o_cm1'}, 'v0:v1 v1:v0')
    own('og', GC, {'v0': 'm=o_m u=o_gu last=o_gc0', 'v1': 'm=o_m u=o_gu last=o_gc1'}, 'v0:v1 v1:v0')
    own('oi', [], {'v0': 'last=o_inl0 u=o_xu m=o_xm', 'v1': 'last=o_inl1 u=o_xu m=o_xm'}, 'v0:v1 v1:v0')
    own('ov', [], {'v0': 'm=o_xm u=o_vu last=o_vt0', 'v1': 'm=o_xm u=o_vu last=o_vt1'}, 'v0:v1')
    own('oz', [], {'v0': 'm=o_xm u=o_zu last=o_si0', 'v1': 'm=o_xm u=o_zu last=o_si1'}, 'v0:v1')
    # the fall-backs' own triggers
    own('or', [], {'ab': 'm=o_m u1=o_u1 u2=o_u2 last=o_body0', 'ba': 'm=o_m u2=o_u2 u1=o_u1 last=o_body0'}, 'ab:ba')
    own('on', [], {'v0': 'm=o_m u=o_u last=o_ns0', 'v1': 'm=o_m u=o_u last=o_ns1'}, 'v0:v1 v1:v0')
    own('op', [], {'v0': 'm=o_m u=o_u last=o_pr0', 'v1': 'm=o_m u=o_u last=o_pr1'}, 'v0:v1 v1:v0')
    own('ol', [], {'v0': 'm=o_m u=o_lu last=o_body0 script=o_ls0', 'v1': 'm=o_m u=o_lu last=o_body0 script=o_ls1'},
        'v0:v1 v1:v0')
# AArch64's erratum 843419 patch (IN-27 row 8): an ADRP at page offset
# 0xff8 in a page-aligned `.text`; padding before it moves the site and its
# patch
scen('pt', 'a64', ['-Wl,--fix-cortex-a53-843419'], {'v0': 'm=o_pm last=o_pt0 u=o_pu',
                                                     'v1': 'm=o_pm last=o_pt1 u=o_pu'})
OWN_PAIRS.extend(['pt/v0:pt/v1', 'pt/v1:pt/v0'])

# the quick subset (ci --quick, O0): every IN-27 row's own scenario on the
# host's architecture and the other, and R3's COMDAT, gc, TLS and IFUNC pairs
QUICK = ('ob/base:ob/grow obx/fn:obx/base os/v0:os/v1 otx/v0:otx/v1 oa/v0:oa/v1 ow/base:ow/weak '
         'oc/v0:oc/v1 ogx/v0:ogx/v1 oi/v0:oi/v1 ovx/v0:ovx/v1 oz/v0:oz/v1 b/k_a:b/k_c '
         'cf/live:cf/dead l/both:l/keep dx/base:dx/grow fx/v0:fx/v1 oa/v1:oa/v0 orx/ab:orx/ba '
         'on/v0:on/v1 op/v0:op/v1 opx/v0:opx/v1 ol/v0:ol/v1').split()


def sh(cmd, cwd):
    p = subprocess.run(cmd, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    if p.returncode != 0:
        raise RuntimeError('%s (in %s): exit %d\n%s' % (' '.join(cmd), cwd, p.returncode, p.stdout[-4000:]))
    return p.stdout


def source(stem):
    for ext in ('.cc', '.c', '.s', '.ld'):
        if os.path.exists(os.path.join(SRC, stem + ext)):
            return stem + ext
    raise RuntimeError('no source for %s' % stem)


def obj(work, cfg, arch, stem):
    """The object of `stem` for (cfg, arch), compiled again only when its
    source, a header or its command changed (a work directory is reused)."""
    d = os.path.join(work, cfg, 'obj-' + arch)
    o = os.path.join(d, stem + '.o')
    s = source(stem)
    cc = tool(arch, 'g++' if s.endswith('.cc') else 'gcc')
    flags = [] if s.endswith(('.s', '.ld')) else ['-' + cfg, '-g'] + r3_flags(stem, arch)
    key = hashlib.sha256(repr([cc, flags, open(os.path.join(SRC, s), 'rb').read()]
                              + [open(os.path.join(SRC, h), 'rb').read() for h in sorted(os.listdir(SRC)) if h.endswith('.h')]
                              ).encode()).hexdigest()
    try:
        if open(o + '.key').read() == key and os.path.exists(o):
            return o
    except OSError:
        pass
    os.makedirs(d, exist_ok=True)
    tmp = o + '.%d.tmp' % os.getpid()
    if s.endswith('.ld'):
        shutil.copyfile(os.path.join(SRC, s), tmp)
    else:
        sh([cc] + flags + ['-c', os.path.join(SRC, s), '-o', tmp], d)
    os.replace(tmp, o)
    with open(o + '.key', 'w') as f:
        f.write(key)
    return o


def inputs(work, cfg, name, variant):
    """[(input file name, bytes)] of one variant, in link order."""
    arch, _gc, variants, archives = SCEN[name]
    out = []
    for p in variants[variant]:
        if len(p) == 1:      # an archive
            stems = archives[p[0]]
            out.append((p[0], ('ar', [obj(work, cfg, arch, s) for s in stems])))
        else:
            ext = '.ld' if source(p[1]).endswith('.ld') else '.o'
            out.append((p[0] + ext, open(obj(work, cfg, arch, p[1]), 'rb').read()))
    return out


def place(d, files, arch):
    """Writes each input whose bytes differ (an untouched input keeps its
    stamp, as a build system leaves it)."""
    for n, b in files:
        path = os.path.join(d, n)
        if isinstance(b, tuple):
            # an archive: built once per directory; its members never change
            if not os.path.exists(path):
                sh([tool(arch, 'ar'), 'rcs', n] + b[1], d)
            continue
        if os.path.exists(path) and open(path, 'rb').read() == b:
            continue
        with open(path + '.tmp', 'wb') as f:
            f.write(b)
        os.replace(path + '.tmp', path)


def link(d, name, cfg, files, mortar_dir, report):
    arch, gc, _v, _a = SCEN[name]
    cxx = any(s.endswith('.cc') for s in (source(p[1]) for p in SCEN[name][2][next(iter(SCEN[name][2]))] if len(p) > 1))
    cc = tool(arch, 'g++' if cxx else 'gcc')
    cmd = [cc, '-static', '-B' + mortar_dir] + gc + ['-Wl,--incremental', '-Wl,--mortar-map=prog.map']
    if report:
        cmd.append('-Wl,--incremental-report=rep.txt')
    cmd += [n for n, _b in files] + ['-o', 'prog']
    sh(cmd, d)


def run_pair(args):
    work, mortar_dir, cfg, pair, drops, keep = args
    a, b = pair.split(':')
    name, va = a.split('/')
    _, vb = b.split('/')
    arch = SCEN[name][0]
    t0 = time.time()
    base = os.path.join(work, cfg, name, va + '-' + vb)
    W, A = os.path.join(base, 'w'), os.path.join(base, 'a')
    for d in (W, A):
        if os.path.isdir(d):
            shutil.rmtree(d)
        os.makedirs(d)
    try:
        fa, fb = inputs(work, cfg, name, va), inputs(work, cfg, name, vb)
        place(W, fa, arch)
        link(W, name, cfg, fa, mortar_dir, False)
        for n in ['prog', 'prog.map'] + [n for n, _b in fa]:
            shutil.copy2(os.path.join(W, n), os.path.join(A, n))
        for n, _b in fa:
            if n not in {m for m, _ in fb}:
                os.remove(os.path.join(W, n))
        place(W, fb, arch)
        link(W, name, cfg, fb, mortar_dir, True)
        F = oracle.Facts(A, W)
        R = oracle.Report(os.path.join(W, 'rep.txt'))
    except Exception as e:  # noqa: BLE001 - reported as the pair's failure
        return (cfg, pair, None, str(e), {})
    if not R.ended and R.reason == 'update-not-implemented':
        return (cfg, pair, None, 'the report has no `delta end`', {})
    ok, npred, lines = oracle.judge(F, R, set())
    res = {
        'ok': ok, 'differing': F.differing(), 'predicted': npred, 'lines': lines,
        'fallbacks': R.fallbacks or ([R.reason] if R.reason not in (None, 'update-not-implemented') else []), 'policy': R.policy, 'units': len(R.units), 'holes': len(R.holes),
        'chunks': len(R.chunks), 'arch': arch, 'secs': time.time() - t0,
    }
    caught = {}
    for drop in drops:
        dok, _n, dlines = oracle.judge(F, R, drop)
        caught[','.join(sorted(drop))] = (not dok, dlines[0] if dlines and not dok else '')
    if ok and not keep:
        # two static programs, their maps and inputs: a few MB per pair
        shutil.rmtree(base)
    return (cfg, pair, res, None, caught)


def main():
    argv = sys.argv[1:]
    mortar, work, quick, mutations, only, jobs, keep = None, None, False, False, None, 4, False
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == '--mortar':
            mortar = argv[i + 1]; i += 2
        elif a == '--work':
            work = argv[i + 1]; i += 2
        elif a == '--quick':
            quick = True; i += 1
        elif a == '--keep':
            keep = True; i += 1
        elif a == '--mutations':
            mutations = True; i += 1
        elif a == '--only':
            only = set(argv[i + 1].split(',')); i += 2
        elif a == '-j':
            jobs = int(argv[i + 1]); i += 2
        elif a == '--tool':
            k, v = argv[i + 1].split('=', 1); TOOLS[k] = v; i += 2
        else:
            sys.exit('run.py: unknown argument %s\n%s' % (a, __doc__))
    if not mortar or not work:
        sys.exit(__doc__)
    if HOST is None:
        sys.exit('run.py: unknown host architecture %s' % platform.machine())
    work = os.path.abspath(work)
    mdir = os.path.join(work, 'bin')
    os.makedirs(mdir, exist_ok=True)
    ld = os.path.join(mdir, 'ld')
    if os.path.lexists(ld):
        os.remove(ld)
    os.symlink(os.path.abspath(mortar), ld)
    pairs = QUICK if quick else R3_PAIRS + OWN_PAIRS
    if only:
        pairs = [p for p in pairs if p.split('/')[0] in only]
    cfgs = ['O0'] if quick else ['O0', 'O2']
    drops = []
    if mutations:
        drops = [{r} for r in oracle.ROWS] + [{f} for f in sorted(oracle.ROWS['fallbacks'][1])]
    # compile serially first (one compile per object), then judge in parallel
    for cfg in cfgs:
        for p in pairs:
            name = p.split('/')[0]
            for v in (p.split(':')[0].split('/')[1], p.split(':')[1].split('/')[1]):
                try:
                    inputs(work, cfg, name, v)
                except RuntimeError as e:
                    sys.exit('run.py: %s' % e)
    jobsl = [(work, mdir, cfg, p, drops, keep) for cfg in cfgs for p in pairs]
    fails = 0
    caught_by = collections.defaultdict(list)
    survivors_any = set()
    with concurrent.futures.ProcessPoolExecutor(max_workers=jobs) as ex:
        results = list(ex.map(run_pair, jobsl))
    for cfg, pair, res, err, caught in results:
        if res is None:
            fails += 1
            print('PAIR %s %s ERROR %s' % (cfg, pair, err))
            continue
        if not res['ok']:
            fails += 1
        print('PAIR %s %-22s %s %s differing=%d refill=%s ratio=%s units=%d holes=%d chunks=%d fallbacks=%s policy=%s (%.1fs)'
              % (cfg, pair, res['arch'], 'PASS' if res['ok'] else 'FAIL', res['differing'], res['predicted'],
                 oracle.ratio(res['predicted'], res['differing']), res['units'], res['holes'], res['chunks'],
                 ','.join(res['fallbacks']) or '-', res['policy'] or '-', res['secs']))
        for l in res['lines'][: (0 if res['ok'] else 20)]:
            print('    ' + l)
        for row, (c, why) in caught.items():
            if c:
                caught_by[row].append('%s:%s' % (cfg, pair))
            survivors_any.add(row)
    if mutations:
        print('MUTATIONS (row dropped -> pairs whose judgement fails)')
        for drop in drops:
            row = ','.join(sorted(drop))
            c = caught_by.get(row, [])
            print('MUTATION %-22s %s %d %s' % (row, 'caught' if c else 'NOT-CAUGHT', len(c), ' '.join(c[:6])))
    print('ORACLE %d pairs, %d failed' % (len(results), fails))
    sys.exit(1 if fails else 0)


if __name__ == '__main__':
    main()
