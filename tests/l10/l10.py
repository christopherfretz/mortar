#!/usr/bin/env python3
"""l10.py: L10, the random edit-sequence lane of incremental linking
(spec/incr.md IN-63; spec/harness.md "L10"; WP-067). Run it through
`cargo xtask l10`, which builds mortar and passes --mortar, --xtask,
--work and -j (the jobserver tokens it holds).

  l10.py --mortar BIN --xtask BIN --work DIR [-j N]
         [--quick] [--seed N]... [--steps N] [--hops 2|3] [--project P,...]
         [--compiler C,...] [--target A,...] [--config NAME,...]
         [--edits ROW,...] [--subject mortar|floor|PATH] [--floor BIN]
         [--keep] [--replay FILE]... [--minimize FILE] [--self-test]
         [--regressions]

A SEQUENCE is (project, configuration, seed, steps). Its steps come from
edits.generate (seeded; no seed = the fixed list SEEDS). For each step:
the edit is applied to the project's knobs, the sources regenerated, the
changed units compiled (build.Cache: content-addressed, so only what
changed), the inputs installed in the work directory W (an unchanged
input keeps its stamp), and then, in W:

  F  the plain full link            (mortar ... -o full --mortar-map=full.map)
  G  the incremental-mode full link (mortar ... --incremental=full -o g)
  X  the SUBJECT                    (SUBJECT ... --incremental
                                     --incremental-report=rep.txt -o prog)

and judge.judge_step(X, F, G of the previous step, G) decides. The first
link of a sequence (step "base") is judged too. A failing sequence
stops at its first failing step and is written to WORK/failed/; `--minimize`
reduces it by delta debugging (over steps, then over each step's knobs
and action) into tests/l10/regressions/.

The subject is a command line that takes the linker's words: `mortar`
(the product path, `mortar --incremental`), `floor` (the test path's
same-size update, subjects/floor.py), or any executable (subjects/ holds
the lane's deliberately wrong ones). It gets L10_STEP (the step's id),
L10_MORTAR and L10_FLOOR in its environment.

Output lines (every one names its seed):
  L10 RESULT project=.. config=.. seed=.. subject=.. steps=.. verdict=PASS|FAIL [at=<step> <check>]
  L10 STATS  ... updates=.. fallbacks=<reason>:<n>,... failed-links=.. [FLAGGED only fall-backs]
  L10 COST   ... wall=..s compiles=.. links=..   (not part of the result: it varies)
RESULT lines are deterministic: the same seed gives the same lines on
any host and any -j. STATS can differ where a fall-back's reason
depends on a timestamp tick (output-changed vs small-output after
`output-rewrite`, IN-21's racy rule).
"""

import collections
import concurrent.futures
import importlib.util
import json
import os
import random
import resource
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.dont_write_bytecode = True
import build  # noqa: E402
import edits  # noqa: E402
import judge  # noqa: E402

PROJECTS = ('gen200', 'corpus40')          # the nightly run's
FLAGGED_PROJECTS = ('protoc', 'gonogo')     # only by name (--project), IN-63's large ones
SEEDS = (1, 2, 3)
THREE_HOP_SEEDS = (3,)   # the nightly run generates these in the three-hop mode (WP-207 §D 14)
STEPS = 30
SUBJECTS = os.path.join(HERE, 'subjects')
REGRESSIONS = os.path.join(HERE, 'regressions')

# The pre-merge smoke (IN-63: 5 minutes of wall time on the pool; WP-067
# measured it, spec/harness.md "L10"): (project, configuration, seed, steps).
QUICK = (
    ('gen200', 'gcc-HOST-O0-g-fs-static', 1, 20),
    ('corpus40', 'gcc-HOST-O0-nog-nofs-static', 1, 12),
    ('gen200', 'clang-x64-O0-nog-fs-static', 2, 12),
)


def nightly_configs():
    """The nightly matrix: every (compiler, target) at -O0 -g with
    sections and at -O2 (kiln: -O0) without, static; and PIE lines."""
    out = []
    for comp in ('gcc', 'clang', 'kiln'):
        for arch in ('a64', 'x64') if comp == 'kiln' else ('a64', 'x64', 'i386'):   # kiln: no i386
            out.append('%s-%s-O0-g-fs-static' % (comp, arch))
            out.append('%s-%s-%s-nog-nofs-static' % (comp, arch, 'O0' if comp == 'kiln' else 'O2'))
    out += ['gcc-a64-O0-nog-nofs-pie', 'clang-x64-O2-g-fs-pie', 'kiln-a64-O0-g-nofs-pie', 'gcc-i386-O0-g-fs-pie']
    return out


_PROJECTS = {}


def load_project(name):
    """The project's module, loaded once per process (protoc keeps its index)."""
    if name not in _PROJECTS:
        _PROJECTS[name] = _load_project(name)
    return _PROJECTS[name]


def _load_project(name):
    path = os.path.join(HERE, 'projects', name, 'project.py')
    spec = importlib.util.spec_from_file_location('l10_project_' + name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


GONOGO_TODO = ('the go/no-go workload (spec/incr.md §1.5, the bench\'s cxx-large-g: some 1.1 GB of output) '
               'needs per step three outputs of the step and three of the previous one, over the 2 GB the lane '
               'may use, and the Python oracle reads both outputs whole; a follow-up (a streaming oracle, '
               'snapshots of the outputs only when a check needs them)')


def project_missing(name, cfg, work):
    """Why the project cannot run in this configuration here, or None
    (protoc: only the bench's own configuration, and only when the bench
    has its objects); prepares the project when it can."""
    proj = load_project(name)
    if hasattr(proj, 'missing'):
        why = proj.missing(cfg)
        if why:
            return why
    if hasattr(proj, 'prepare'):
        proj.prepare(work)
    return None


def host_cfg(name):
    return name.replace('HOST', build.HOST or 'a64')


# ------------------------------------------------------------- sequences --
class Seq:
    """A sequence to run: everything needed to reproduce it."""
    def __init__(self, project, config, seed, steps, subject='mortar', label=None, env=None, rows=None, hops=None):
        self.project, self.config, self.seed, self.steps = project, config, seed, steps
        self.subject, self.env, self.rows, self.hops = subject, dict(env or {}), rows, hops
        self.label = label or '%s.%s.%s%s' % (project, config, seed, '.h3' if hops == 3 else '')

    def to_json(self, extra=None):
        d = {'lane': 'l10', 'version': 1, 'project': self.project, 'config': self.config, 'seed': self.seed,
             'subject': self.subject, 'env': self.env, 'rows': self.rows, 'hops': self.hops, 'steps': self.steps}
        d.update(extra or {})
        return d

    @staticmethod
    def from_json(d, label=None):
        if d.get('lane') != 'l10':
            raise ValueError('not an L10 sequence file')
        return Seq(d['project'], d['config'], d['seed'], d['steps'], d.get('subject', 'mortar'),
                   label, d.get('env'), d.get('rows'), d.get('hops'))


def describe(s):
    return 'project=%s config=%s seed=%s%s subject=%s' % (s.project, s.config, s.seed, ' hops=3' if s.hops == 3 else '',
                                                         os.path.basename(s.subject))


# ------------------------------------------------------------- compiling --
def unit_jobs(seq, cache):
    """[(key, compile args)] of every object the sequence needs."""
    proj = load_project(seq.project)
    cfg = build.Cfg(seq.config)
    if hasattr(proj, 'prepare'):
        proj.prepare(os.path.dirname(cache.dir))
    jobs = {}
    k = proj.knobs()
    for i in range(-1, len(seq.steps)):
        if i >= 0:
            k.update(seq.steps[i]['set'])
        units, headers = proj.sources(k, cfg)
        for u in proj.units_needed(k, cfg):
            fname, lang, text = units[u]
            extra = proj.unit_flags(u)
            key = build.object_key(cfg, u, lang, text, headers, extra)
            if key not in jobs and not cache.have(key):
                jobs[key] = (seq.config, u, fname, lang, text, headers, extra, key)
    return jobs


def _compile(args):
    work, (config, u, fname, lang, text, headers, extra, key) = args
    try:
        build.Cache(work).compile(build.Cfg(config), u, fname, lang, text, headers, extra, key)
        return None
    except Exception as e:  # noqa: BLE001 - a compile error ends the lane with its text
        return '%s %s: %s' % (config, fname, str(e)[-1500:])


def precompile(seqs, work, jobs):
    cache = build.Cache(work)
    todo = {}
    for s in seqs:
        todo.update(unit_jobs(s, cache))
    if not todo:
        return 0, []
    errs = []
    with concurrent.futures.ProcessPoolExecutor(max_workers=jobs) as ex:
        for r in ex.map(_compile, [(work, j) for j in sorted(todo.values(), key=lambda j: j[-1])]):
            if r:
                errs.append(r)
    return len(todo), errs


# --------------------------------------------------------------- running --
class Runner:
    """One sequence in its own directory WORK/seq/<label>."""

    def __init__(self, seq, opts):
        self.seq, self.opts = seq, opts
        self.proj = load_project(seq.project)
        self.cfg = build.Cfg(seq.config)
        self.cache = build.Cache(opts['work'])
        self.base = os.path.join(opts['work'], 'seq', seq.label)
        self.W = os.path.join(self.base, 'w')
        self.stage = os.path.join(self.base, 'stage')
        self.links = 0
        self.archives = {}       # (name, thin, ((member, object path), ...)) -> bytes
        self.installed = {}      # name in W -> digest of the bytes the lane last wrote there
        self.objbytes = {}       # cache object path -> bytes
        self.pre, self.post = build.capture(opts['work'], self.cfg)
        if hasattr(self.proj, 'prepare'):
            self.proj.prepare(opts['work'])
        sub = seq.subject
        if sub == 'mortar':
            self.subject = [opts['mortar']]
        elif sub == 'floor':
            self.subject = [sys.executable, os.path.join(SUBJECTS, 'floor.py')]
        elif os.sep not in sub and os.path.exists(os.path.join(SUBJECTS, sub)):
            self.subject = [sys.executable, os.path.join(SUBJECTS, sub)]
        else:
            self.subject = [sub]

    # inputs ---------------------------------------------------------------
    def objects(self, k):
        units, headers = self.proj.sources(k, self.cfg)
        objs = {}
        for u in self.proj.units_needed(k, self.cfg):
            fname, lang, text = units[u]
            extra = self.proj.unit_flags(u)
            key = build.object_key(self.cfg, u, lang, text, headers, extra)
            objs[u] = self.cache.compile(self.cfg, u, fname, lang, text, headers, extra, key)
        if hasattr(self.proj, 'prebuilt'):
            # objects the project does not compile (protoc: the bench's, read only)
            for u, path in self.proj.prebuilt(k, self.cfg).items():
                objs.setdefault(u, path)
        return objs

    def wanted(self, k):
        """[(file name in W, bytes)] in install order (thin members before
        their archive), and the link's input words and options."""
        objs = self.objects(k)
        inputs, options = self.proj.plan(k, self.cfg)
        files, words = [], []
        read = self.read_object
        for inp in inputs:
            if inp[0] == 'o':
                files.append((inp[1] + '.o', read(objs[inp[1]])))
                words.append(inp[1] + '.o')
            else:
                _a, name, members, thin = inp
                if thin:
                    files += [(m + '.o', read(objs[m])) for m in members]
                files.append((name, (members, thin)))
                words.append(name)
        return files, words, options, objs

    def install(self, files, objs, inplace=False):
        """Writes the inputs into W; returns the names that changed."""
        changed = []
        for name, data in files:
            path = os.path.join(self.W, name)
            if isinstance(data, tuple):
                members, thin = data
                # the cache's objects are content-addressed: their paths name the archive's bytes
                akey = (name, thin, tuple((m, objs[m]) for m in members))
                data = self.archives.get(akey)
                if data is None:
                    names = getattr(self.proj, 'member_names', lambda: None)()
                    data = build.build_archive(self.cfg, self.stage, name, members, objs, thin, self.W, names)
                    # bounded: protoc's archives are 85 and 117 MB
                    while self.archives and (len(self.archives) >= 16 or sum(
                            len(d) for d in self.archives.values()) + len(data) > 320 << 20):
                        self.archives.pop(next(iter(self.archives)))
                    self.archives[akey] = data
            if build.write_if_changed(path, data, inplace, self.installed.get(name)):
                changed.append(name)
            self.installed[name] = build.digest(data)
        return changed

    def read_object(self, path):
        """A cache object's bytes (content-addressed: never changes)."""
        data = self.objbytes.get(path)
        if data is None:
            with open(path, 'rb') as f:
                data = self.objbytes[path] = f.read()
        return data

    def remove_stale(self, files):
        keep = {n for n, _d in files}
        for n in os.listdir(self.W):
            if (n.endswith('.o') or n.endswith('.a')) and n not in keep:
                os.remove(os.path.join(self.W, n))
                self.installed.pop(n, None)

    # links ----------------------------------------------------------------
    def link(self, argv0, words, options, out, extra, env=None):
        argv = argv0 + self.pre + options + words + self.post + extra + ['-o', out]
        self.links += 1
        try:
            p = subprocess.run(argv, cwd=self.W, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env,
                               timeout=build.LINK_TIMEOUT, text=True, errors='replace')
            return p.returncode, p.stdout
        except subprocess.TimeoutExpired:
            return 'timeout', 'the link timed out after %ds' % build.LINK_TIMEOUT

    @staticmethod
    def diags(text, out):
        import re
        lines = []
        for l in text.splitlines():
            if 'incremental' in l or not l.strip():
                continue
            lines.append(re.sub(r'(?<![\w./-])%s(?![\w.-])' % re.escape(out), '<out>', l))
        return lines

    def snapshot(self, d, files, copy_names):
        """S_i: the inputs (hard links; copies of the names in copy_names,
        which the next step rewrites in place), G's output as prog/prog.map,
        X's as x/x.map, F's as full/full.map, X's report."""
        os.makedirs(d)
        for n, _data in files:
            src = os.path.join(self.W, n)
            if n in copy_names:
                shutil.copy2(src, os.path.join(d, n))
            else:
                os.link(src, os.path.join(d, n))
        for src, dst in (('g', 'prog'), ('g.map', 'prog.map'), ('prog', 'x'), ('prog.map', 'x.map'),
                         ('full', 'full'), ('full.map', 'full.map'), ('rep.txt', 'rep.txt')):
            if os.path.exists(os.path.join(self.W, src)):
                shutil.copy2(os.path.join(self.W, src), os.path.join(d, dst))
        # thin archives name their members relative to themselves: present
        return d

    def touch(self, unit, files):
        names = {n for n, _d in files}
        if unit + '.o' in names:
            os.utime(os.path.join(self.W, unit + '.o'))
            return
        for n, d in files:
            if isinstance(d, tuple) and unit in d[0]:
                os.utime(os.path.join(self.W, n))
                return

    def run(self, stop_at_failure=True):
        """Runs the sequence: a result dict."""
        t0 = time.time()
        if os.path.isdir(self.base):
            shutil.rmtree(self.base)
        os.makedirs(self.W)
        os.makedirs(self.stage)
        tools = judge.Tools(self.opts['xtask'], self.cfg.runner(), args=getattr(self.proj, 'RUN_ARGS', ()))
        env = dict(os.environ)
        env.update({'L10_MORTAR': self.opts['mortar'], 'L10_FLOOR': self.opts.get('floor') or '',
                    'L10_WORK': self.base, 'PYTHONDONTWRITEBYTECODE': '1'})
        env.update(self.seq.env)
        k = self.proj.knobs()
        steps = [None] + list(self.seq.steps)
        res = {'steps': len(self.seq.steps), 'reasons': collections.Counter(), 'failed_links': 0,
               'updates': 0, 'failure': None, 'at': None, 'rows': collections.defaultdict(collections.Counter),
               'lines': [], 'phase': collections.Counter(), 'hops': collections.defaultdict(collections.Counter)}
        ph = res['phase']
        prev = None          # S_{i-1}: the previous successful step's snapshot
        preinstalled = False
        for i, step in enumerate(steps):
            sid = 'base' if step is None else step['id']
            if step is not None:
                if step.get('fault'):
                    res['failure'] = 'step %s: faults are H-2\'s (WP-068): %s' % (sid, step['fault'])
                    res['at'] = sid
                    break
                k.update(step['set'])
            t = time.time()
            files, words, options, objs = self.wanted(k)
            if not preinstalled:
                self.install(files, objs)
            preinstalled = False
            self.remove_stale(files)
            action = step.get('action') if step else None
            if action and action.startswith('touch:'):
                self.touch(action.split(':', 1)[1], files)
            for n in ('full', 'full.map', 'g', 'g.map', 'rep.txt'):
                if os.path.exists(os.path.join(self.W, n)):
                    os.remove(os.path.join(self.W, n))
            ph['inputs'] += time.time() - t
            t = time.time()
            fs, fo = self.link([self.opts['mortar']], words, options, 'full', ['--mortar-map=full.map'])
            gs, _go = self.link([self.opts['mortar']], words, options, 'g',
                                ['--incremental=full', '--mortar-map=g.map'])
            senv = dict(env)
            senv['L10_STEP'] = str(sid)
            xs, xo = self.link(self.subject, words, options, 'prog',
                               ['--incremental', '--incremental-report=rep.txt', '--mortar-map=prog.map'], senv)
            ph['links'] += time.time() - t
            if step is None and fs != 0 and 'unsupported feature' in fo:
                # the configuration is refused by name (spec: correct or refused):
                # nothing of incremental linking to judge
                res['refused'] = next((l for l in fo.splitlines() if 'unsupported feature' in l), '?')[:160]
                break
            t = time.time()
            # the next step's actions that must follow this link at once
            nxt = steps[i + 1] if i + 1 < len(steps) else None
            copy_names = set()
            if nxt is not None and nxt.get('action') in ('inplace', 'output-rewrite') and not nxt.get('fault'):
                if nxt['action'] == 'output-rewrite' and os.path.exists(os.path.join(self.W, 'prog')):
                    p = os.path.join(self.W, 'prog')
                    with open(p, 'rb') as f:
                        data = f.read()
                    with open(p, 'r+b') as f:
                        f.write(data)
                elif nxt['action'] == 'inplace':
                    k2 = dict(k)
                    k2.update(nxt['set'])
                    files2, _w, _o, objs2 = self.wanted(k2)
                    # the snapshot keeps this step's bytes of the files rewritten in place
                    keep = {n: open(os.path.join(self.W, n), 'rb').read() for n, _d in files
                            if os.path.exists(os.path.join(self.W, n))}
                    # an in-place write reaches every hard link of the file: the
                    # previous snapshot must own its inputs before it happens
                    if prev is not None:
                        unshare(prev.dir)
                    copy_names = set(self.install(files2, objs2, inplace=True))
                    preinstalled = True
            S = os.path.join(self.base, 's%s' % i)
            if fs == 0:
                self.snapshot(S, files, copy_names)
                for n in copy_names:
                    if n in keep:
                        with open(os.path.join(S, n), 'wb') as f:
                            f.write(keep[n])
            else:
                os.makedirs(S)
            x = judge.Link(S, 'x', 'x.map', xs, self.diags(xo, 'prog'), os.path.join(S, 'rep.txt'))
            f = judge.Link(S, 'full', 'full.map', fs, self.diags(fo, 'full'))
            new = judge.Link(S, 'prog', 'prog.map', gs, []) if gs == 0 and fs == 0 else None
            if xs != 0 and fs != 0:
                x.dir = f.dir = self.W   # nothing to read: both failed
            ph['snapshot'] += time.time() - t
            v = judge.judge_step(tools, x, f, prev, new)
            for c, dt in v.times.items():
                ph[c] += dt
            reason = v.reason or ('link-failed' if xs != 0 else
                                  'incremental-unsupported' if ('incremental-unsupported' in xo or
                                                               '--incremental is not supported with' in xo)
                                  else 'no-report')
            res['reasons'][reason] += 1
            if reason == 'update':
                res['updates'] += 1
            elif step is not None and xs == 0 and os.path.exists(x.report):
                for l in open(x.report):
                    if l.startswith('l10: the floor refused: '):
                        why = l.split('failed: ', 1)[-1].split(':')[0].strip()
                        res.setdefault('floor_refused', collections.Counter())[why] += 1
            if xs != 0 and fs != 0:
                res['failed_links'] += 1
                res.setdefault('fail_diag', (f.diags[0][:160] if f.diags else 'status %s' % fs))
            row = 'base' if step is None else step['edit']
            res['rows'][row][reason] += 1
            if step is not None:
                res['hops'][row][step.get('hop', 1)] += 1
            res['lines'].append('step %s %-22s %s reason=%s%s' % (
                sid, row, v.line(), reason,
                (' (F: %s)' % (f.diags[0][:160] if f.diags else 'status %s' % fs)) if fs != 0 else ''))
            if not v.ok:
                res['failure'] = 'step %s (%s) %s' % (sid, row, v.first_failure())
                res['at'] = sid
                res['check'] = v.first_failure().split(':')[0]
                if stop_at_failure:
                    break
            if new is not None:
                if prev is not None and not self.opts.get('keep'):
                    shutil.rmtree(prev.dir)
                prev = new
        res['wall'] = time.time() - t0
        res['links'] = self.links
        if res['failure'] is None and not self.opts.get('keep'):
            shutil.rmtree(self.base)
        res['rows'] = {r: dict(c) for r, c in res['rows'].items()}
        res['hops'] = {r: dict(c) for r, c in res['hops'].items()}
        res['reasons'] = dict(res['reasons'])
        res['phase'] = dict(res['phase'])
        return res


def unshare(d):
    """Replaces each hard-linked file in d by a copy of its own."""
    for n in os.listdir(d):
        p = os.path.join(d, n)
        if os.path.isfile(p) and not os.path.islink(p) and os.stat(p).st_nlink > 1:
            tmp = p + '.unshare'
            shutil.copy2(p, tmp)
            os.replace(tmp, p)


def run_one(args):
    seq, opts = args
    try:
        return seq, Runner(seq, opts).run()
    except Exception as e:  # noqa: BLE001 - the lane's own error, reported as the sequence's failure
        import traceback
        return seq, {'failure': 'lane error: %s: %s' % (type(e).__name__, e), 'at': None, 'check': 'lane',
                     'trace': traceback.format_exc()[-2000:], 'steps': len(seq.steps), 'reasons': {},
                     'failed_links': 0, 'updates': 0, 'rows': {}, 'lines': [], 'wall': 0, 'links': 0}


# ------------------------------------------------------------- reporting --
def result_line(seq, r):
    v = 'PASS' if r['failure'] is None else 'FAIL at=%s %s' % (r['at'], r.get('check', '?'))
    if r.get('refused') and r['failure'] is None:
        v = 'REFUSED (the base link: %s)' % r['refused']
    return 'L10 RESULT %s steps=%d verdict=%s' % (describe(seq), r['steps'], v)


def stats_line(seq, r):
    fb = ','.join('%s:%d' % (k, n) for k, n in sorted(r['reasons'].items()) if k != 'update') or '-'
    flag = ' FLAGGED(only fall-backs)' if r['updates'] == 0 and r['failure'] is None and not r.get('refused') else ''
    if r['failure'] is None and r['failed_links'] == r['steps'] + 1:
        flag = ' FLAGGED(every link failed, F too: %s)' % r.get('fail_diag', '?')
    return 'L10 STATS  %s steps=%d updates=%d fallbacks=%s failed-links=%d%s' % (
        describe(seq), r['steps'], r['updates'], fb, r['failed_links'], flag)


def cost_line(seq, r):
    ph = ' '.join('%s=%.1f' % (n, r.get('phase', {}).get(n, 0.0))
                  for n in ('inputs', 'links', 'snapshot', 'exit', 'verify', 'equiv', 'run', 'oracle'))
    return 'L10 COST   %s wall=%.1fs links=%d load=%s [%s]' % (describe(seq), r['wall'], r['links'],
                                                               open('/proc/loadavg').read().split()[0], ph)


def write_failure(seq, r, work):
    d = os.path.join(work, 'failed')
    os.makedirs(d, exist_ok=True)
    path = os.path.join(d, seq.label + '.json')
    with open(path, 'w') as f:
        json.dump(seq.to_json({'failure': r['failure'], 'check': r.get('check')}), f, indent=1)
        f.write('\n')
    return path


# ------------------------------------------------------------- minimizer --
def minimize(seq, opts, check, log=print):
    """Delta debugging (Zeller's ddmin) over the steps of a failing
    sequence, then over each remaining step's knobs and action. A
    candidate "fails" when its run fails at the same check. Returns the
    minimized Seq and its result."""
    tries = [0]

    def fails(steps):
        tries[0] += 1
        s = Seq(seq.project, seq.config, seq.seed, steps, seq.subject, seq.label + '.min', seq.env, seq.rows)
        _s, r = run_one((s, opts))
        return r['failure'] is not None and r.get('check') == check, r

    ok, r = fails(seq.steps)
    if not ok:
        log('L10 MINIMIZE seed=%s: the sequence does not fail at `%s` (it %s)' % (
            seq.seed, check, r['failure'] or 'passes'))
        return None, r
    # steps after the failing one cannot matter
    if r['at'] not in (None, 'base'):
        c = [s for s in seq.steps if s['id'] <= r['at']]
    else:
        c = list(seq.steps)
    if fails([])[0]:
        c = []
    n = 2
    while len(c) >= 2:
        chunk = (len(c) + n - 1) // n
        parts = [c[j:j + chunk] for j in range(0, len(c), chunk)]
        reduced = False
        for p in parts:
            if fails(p)[0]:
                c, n, reduced = p, 2, True
                break
        if not reduced:
            for p in parts:
                comp = [s for s in c if s not in p]
                if fails(comp)[0]:
                    c, n, reduced = comp, max(n - 1, 2), True
                    break
        if not reduced:
            if n >= len(c):
                break
            n = min(2 * n, len(c))
    if len(c) == 1 and fails([])[0]:
        c = []
    # parameters: drop knobs of multi-knob steps, drop actions
    for idx in range(len(c)):
        for key in sorted(c[idx]['set']):
            if len(c[idx]['set']) < 2:
                break
            t = dict(c[idx], set={a: b for a, b in c[idx]['set'].items() if a != key})
            cand = c[:idx] + [t] + c[idx + 1:]
            if fails(cand)[0]:
                c = cand
        if c[idx].get('action'):
            t = dict(c[idx], action=None)
            cand = c[:idx] + [t] + c[idx + 1:]
            if fails(cand)[0]:
                c = cand
    ok, r = fails(c)
    out = Seq(seq.project, seq.config, seq.seed, c, seq.subject, seq.label + '.min', seq.env, seq.rows)
    log('L10 MINIMIZE seed=%s: %d -> %d step(s) in %d runs: %s' % (seq.seed, len(seq.steps), len(c), tries[0],
                                                                   r['failure']))
    return out, r


def write_regression(seq, r, into):
    os.makedirs(into, exist_ok=True)
    name = '%s.%s.seed%s.json' % (seq.project, seq.config, seq.seed)
    path = os.path.join(into, name)
    with open(path, 'w') as f:
        json.dump(seq.to_json({'failure': r['failure'], 'check': r.get('check')}), f, indent=1)
        f.write('\n')
    return path


# ------------------------------------------------------------- self test --
def self_test(opts, log=print):
    """The lane's own test (IN-63's last sentence): wrong subjects are
    found and minimized to at most 3 steps, a dropped diagnostic is found,
    and the floor on same-size edits passes with its steps as updates."""
    cfg = host_cfg('gcc-HOST-O0-nog-nofs-static')
    proj = load_project('gen200')
    out = []
    # rows that change a unit's code or data (the wrong subjects need one)
    changing = {'e1', 'e2', 'e3', 'e5', 'e6', 'e9', 'e10', 'member-replace', 'global-var', 'const-remove',
                'many-files', 'static-global', 'common-grow', 'tls-grow-shrink'}

    def target(steps, seed):
        rng = random.Random('l10-self-test/%d' % seed)
        start = rng.randrange(len(steps) // 2)
        found = [s['id'] for s in steps[1:] if s['edit'] in changing and s['set']]
        # the first changing step at or after a seeded start, else the first one
        return next((i for i in found if i >= start), found[0] if found else None)

    cases = []
    for name, sub, seed in (('prev-output', 'prev_output.py', 11), ('flip-word', 'flip_word.py', 12)):
        steps = edits.generate(proj, build.Cfg(cfg), seed, 10)
        at = target(steps, seed)
        cases.append((name, Seq('gen200', cfg, seed, steps, sub, 'self.%s' % name, {'L10_WRONG_AT': str(at)}), at))
    rows = ['undef-fix']    # the error row alone: every sequence then has diagnostics to drop
    steps = edits.generate(proj, build.Cfg(cfg), 13, 4, rows)
    cases.append(('drop-diag', Seq('gen200', cfg, 13, steps, 'drop_diag.py', 'self.drop-diag', {}, rows), None))
    precompile([c[1] for c in cases], opts['work'], opts['jobs'])
    with concurrent.futures.ProcessPoolExecutor(max_workers=max(1, min(opts['jobs'], len(cases)))) as ex:
        results = list(ex.map(run_one, [(c[1], opts) for c in cases]))
    for (name, seq, at), (_s, r) in zip(cases, results):
        if r['failure'] is None:
            out.append('L10 SELF-TEST %s seed=%s wrong-at=%s: NOT FOUND (the lane passed a wrong subject)' % (
                name, seq.seed, at))
            continue
        if name == 'drop-diag':
            good = r.get('check') == 'exit'
            out.append('L10 SELF-TEST %s seed=%s: %s at step %s: %s' % (
                name, seq.seed, 'found' if good else 'WRONG CHECK', r['at'], r['failure']))
            continue
        m, mr = minimize(seq, opts, r['check'], log=lambda *_: None)
        n = len(m.steps) if m else None
        good = m is not None and n <= 3
        out.append('L10 SELF-TEST %s seed=%s wrong-at=%s: found at step %s (%s), minimized to %s step(s)%s' % (
            name, seq.seed, at, r['at'], r['check'], n, '' if good else ' NOT <= 3'))
    # the floor on same-size edits
    if opts.get('floor'):
        rows = ['e1', 'member-replace', 'touch']
        steps = edits.generate(proj, build.Cfg(cfg), 14, 8, rows)
        seq = Seq('gen200', cfg, 14, steps, 'floor', 'self.floor', {}, rows)
        precompile([seq], opts['work'], opts['jobs'])
        _s, r = run_one((seq, opts))
        refused = r.get('floor_refused', collections.Counter())
        # every step is an update, or the floor refused it by name (WP-061's
        # floor takes one changed loose object or thin member, same size)
        good = r['failure'] is None and r['updates'] >= 1 and r['updates'] + sum(refused.values()) == len(steps)
        out.append('L10 SELF-TEST floor seed=14: %s, %d of %d steps updates, %d refused by the floor (%s)%s' % (
            'passes' if r['failure'] is None else 'FAILS: ' + r['failure'], r['updates'], len(steps),
            sum(refused.values()), ','.join('%s:%d' % kv for kv in sorted(refused.items())),
            '' if good else ' UNEXPECTED'))
    else:
        out.append('L10 SELF-TEST floor: SKIPPED (no --floor)')
    for l in out:
        log(l)
    return all(('NOT' not in l and 'WRONG' not in l and 'UNEXPECTED' not in l and 'FAILS' not in l) for l in out)


# ------------------------------------------------------------------ main --
def parse(argv):
    o = {'hops': None, 'seeds': [], 'steps': None, 'projects': None, 'compilers': None, 'targets': None, 'configs': None,
         'quick': False, 'keep': False, 'jobs': 1, 'replay': [], 'minimize': None, 'subject': 'mortar',
         'edits': None, 'self_test': False, 'regressions': False, 'floor': None, 'require_tools': False}
    i = 0
    val = lambda: argv[i + 1]
    while i < len(argv):
        a = argv[i]
        if a in ('--mortar', '--xtask', '--work', '--floor'):
            o[a[2:]] = os.path.abspath(val()); i += 2
        elif a == '--seed':
            o['seeds'].append(int(val())); i += 2
        elif a == '--steps':
            o['steps'] = int(val()); i += 2
        elif a == '--hops':
            o['hops'] = int(val()); i += 2
            if o['hops'] not in (2, 3):
                sys.exit('l10.py: --hops 2|3')
        elif a in ('--project', '--compiler', '--target', '--config', '--edits'):
            key = {'--project': 'projects', '--compiler': 'compilers', '--target': 'targets',
                   '--config': 'configs', '--edits': 'edits'}[a]
            o[key] = [x for x in val().split(',') if x]; i += 2
        elif a == '-j':
            o['jobs'] = max(1, int(val())); i += 2
        elif a == '--subject':
            o['subject'] = val(); i += 2
        elif a == '--replay':
            o['replay'].append(os.path.abspath(val())); i += 2
        elif a == '--minimize':
            o['minimize'] = os.path.abspath(val()); i += 2
        elif a in ('--quick', '--keep', '--self-test', '--regressions', '--require-tools'):
            o[a[2:].replace('-', '_')] = True; i += 1
        else:
            sys.exit('l10.py: unknown argument %s\n%s' % (a, __doc__))
    for need in ('mortar', 'xtask', 'work'):
        if not o.get(need):
            sys.exit('l10.py: --%s is required\n%s' % (need, __doc__))
    if o['edits']:
        bad = [e for e in o['edits'] if e not in edits.BY_NAME]
        if bad:
            sys.exit('l10.py: unknown edit row(s) %s; rows: %s' % (','.join(bad), ','.join(edits.BY_NAME)))
    return o


def plan_sequences(o, log):
    """The sequences of a run, and the configurations skipped (with why)."""
    seqs, skipped = [], []
    if o['quick'] and not (o['projects'] or o['configs'] or o['compilers'] or o['targets']):
        lines = [(p, host_cfg(c), s, n) for p, c, s, n in QUICK]
        if o['seeds']:
            lines = [(p, c, s2, n) for p, c, _s, n in lines for s2 in o['seeds']]
    else:
        configs = o['configs'] or nightly_configs()
        configs = [host_cfg(c) for c in configs]
        if o['compilers']:
            configs = [c for c in configs if c.split('-')[0] in o['compilers']]
        if o['targets']:
            configs = [c for c in configs if c.split('-')[1] in o['targets']]
        seeds = o['seeds'] or ([SEEDS[0]] if o['quick'] else list(SEEDS))
        lines = [(p, c, s, o['steps'] or (12 if o['quick'] else STEPS))
                 for p in (o['projects'] or PROJECTS) for c in configs for s in seeds]
    for p, c, s, n in lines:
        if o['steps']:
            n = o['steps']
        if p not in PROJECTS + FLAGGED_PROJECTS:
            sys.exit('l10.py: unknown project %s (%s)' % (p, ', '.join(PROJECTS + FLAGGED_PROJECTS)))
        if p == 'gonogo':
            skipped.append('gonogo %s: TODO (not in the lane): %s' % (c, GONOGO_TODO))
            continue
        cfg = build.Cfg(c)
        why = cfg.missing() or project_missing(p, cfg, o['work'])
        if why:
            skipped.append('%s %s: %s' % (p, c, why))
            continue
        proj = load_project(p)
        if o['edits'] and not edits.rows_for(p, o['edits']):
            skipped.append('%s %s: none of the edit rows applies' % (p, c))
            continue
        hops = o['hops'] or (3 if s in THREE_HOP_SEEDS and not o['quick'] and not o['seeds'] else None)
        hops = None if hops == 2 else hops
        steps = edits.generate(proj, cfg, s, n, o['edits'], hops)
        seqs.append(Seq(p, c, s, steps, o['subject'], rows=o['edits'], hops=hops))
    return seqs, skipped


def edit_table(results, log):
    """Per edit row: how many steps applied it, and the reasons seen."""
    count = collections.Counter()
    seen = collections.defaultdict(collections.Counter)
    hops = collections.defaultdict(collections.Counter)
    for _seq, r in results:
        for row, reasons in r['rows'].items():
            for reason, n in reasons.items():
                count[row] += n
                seen[row][reason] += n
        for row, hc in r.get('hops', {}).items():
            hops[row].update(hc)
    for row in ['base'] + [r.name for r in edits.ROWS]:
        R = edits.BY_NAME.get(row)
        extra = ''
        if R is not None:
            extra = ' hops=%s' % '/'.join(str(hops[row][h]) for h in (1, 2, 3))
            if R.shape:
                extra += ' shape="%s"' % R.shape
            if R.wp207:
                extra += ' wp207=%s' % ','.join(map(str, R.wp207))
        log('L10 EDIT %-22s steps=%-4d reasons=%s%s' % (row, count[row], ','.join(
            '%s:%d' % kv for kv in sorted(seen[row].items())) or '-', extra))


def main():
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    o = parse(sys.argv[1:])
    os.makedirs(o['work'], exist_ok=True)
    log = lambda s: print(s, flush=True)
    opts = {k: o[k] for k in ('mortar', 'xtask', 'work', 'keep', 'floor', 'jobs')}
    if o['self_test']:
        sys.exit(0 if self_test(opts, log) else 1)
    if o['minimize']:
        seq = Seq.from_json(json.load(open(o['minimize'])), 'minimize')
        _s, r = run_one((seq, opts))
        if r['failure'] is None:
            log('L10 MINIMIZE seed=%s: the sequence passes; nothing to minimize' % seq.seed)
            sys.exit(1)
        precompile([seq], o['work'], o['jobs'])
        m, mr = minimize(seq, opts, r['check'], log)
        if m is None:
            sys.exit(1)
        log('L10 REGRESSION %s' % write_regression(m, mr, REGRESSIONS))
        sys.exit(0)
    seqs, skipped = [], []
    if o['replay'] or o['regressions']:
        files = list(o['replay'])
        if o['regressions']:
            files += sorted(os.path.join(REGRESSIONS, f) for f in os.listdir(REGRESSIONS) if f.endswith('.json'))
        for path in files:
            seqs.append(Seq.from_json(json.load(open(path)), 'replay.' + os.path.basename(path)[:-5]))
    else:
        seqs, skipped = plan_sequences(o, log)
    for s in skipped:
        log('L10 SKIP %s' % s)
    if skipped and o['require_tools']:
        log('L10 SUMMARY %d configuration(s) skipped under MORTAR_REQUIRE_TOOLS=1' % len(skipped))
        sys.exit(1)
    if not seqs:
        log('L10 SUMMARY sequences=0 (nothing to run here)')
        sys.exit(0)
    t0 = time.time()
    for c in sorted({s.config for s in seqs}):
        build.capture(o['work'], build.Cfg(c))
    ncomp, errs = precompile(seqs, o['work'], o['jobs'])
    if errs:
        for e in errs:
            log('L10 COMPILE-ERROR %s' % e)
        sys.exit(1)
    log('L10 COMPILED %d object(s) in %.1fs (cache: %s)' % (ncomp, time.time() - t0, os.path.join(o['work'], 'cache')))
    with concurrent.futures.ProcessPoolExecutor(max_workers=min(o['jobs'], len(seqs))) as ex:
        results = list(ex.map(run_one, [(s, opts) for s in seqs]))
    fails = flagged = refused = 0
    for seq, r in sorted(results, key=lambda sr: (sr[0].project, sr[0].config, sr[0].seed)):
        log(result_line(seq, r))
        log(stats_line(seq, r))
        log(cost_line(seq, r))
        if r['failure'] is not None:
            fails += 1
            for l in r['lines'][-3:]:
                log('    ' + l)
            log('    %s' % r['failure'])
            if r.get('trace'):
                log(r['trace'])
            path = write_failure(seq, r, o['work'])
            log('    sequence: %s (cargo xtask l10 --minimize %s)' % (path, path))
        elif r.get('refused'):
            refused += 1
        elif r['updates'] == 0:
            flagged += 1
    edit_table(results, log)
    log('L10 SUMMARY sequences=%d failed=%d flagged=%d refused=%d%s wall=%.0fs' % (
        len(results), fails, flagged, refused,
        ' (every passing sequence only fell back: no update on the product path yet)'
        if flagged == len(results) - fails - refused and flagged else '', time.time() - t0))
    sys.exit(1 if fails else 0)


if __name__ == '__main__':
    main()
