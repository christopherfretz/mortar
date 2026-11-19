#!/usr/bin/env python3
"""incr-update: the named three-hop scenarios of the incremental update,
each hop judged IN-79's four ways (spec/harness.md "incr-update";
spec/incr.md IN-75, IN-75a, IN-78, IN-79; H-1, WP-223).

    lane.py --mortar M --hooked H --xtask X --work W [--quick] [--only S,...]
            [--target a64|x64|i386,...] [--compiler gcc|kiln,...]
            [--subject mortar|<name>|<path>] [--keep] [--self-test] [-j N]
            [--require-tools]

`cargo xtask incr-update` runs it (xtask/src/incr_update.rs). M is the
default mortar (the full links F and G), H the same crate built with
`--features testhooks` (the subject `mortar`: MORTAR_INCR_DIRTY gives an
update's dirty ranges), X the xtask binary (`plumb`, `plumb --equiv`).

A scenario (scenarios/<name>/scenario) is a first link in incremental
mode and three updates: hop 1 the edit, hop 2 its inverse, hop 3 the edit
again (`hops:` may say otherwise). Each hop installs the inputs (a changed
object as a new inode, an unchanged one untouched; H gets copies), and links three times:
F, the plain full link (in its own directory, same output name); G, the
incremental-mode full link (`--incremental=full`, IN-63a's link); X, the
SUBJECT (`--incremental --incremental-report=`, in the directory that
keeps its output and state from hop to hop). tests/l10/judge.py's
judge_step(..., hop=Hop) judges X: exit, stderr, dirty, verify, elflint,
equiv, run, oracle, state, duty; and `expect` after hop 0 when the
scenario says `expect-full:` (judge.expect_full). The work directory is
<W>/run/<scenario>.
<configuration> (removed when the run passes unless --keep).
"""

import concurrent.futures
import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, '..', 'l10'))
sys.dont_write_bytecode = True
import build  # noqa: E402  (tests/l10/build.py: configurations, the object cache, the captured link line)
import judge  # noqa: E402  (tests/l10/judge.py: the one judge)

SCENARIOS = os.path.join(HERE, 'scenarios')
SUBJECTS = os.path.join(HERE, 'subjects')
COMMON = os.path.join(SCENARIOS, '_common')
TARGETS = ('a64', 'x64', 'i386')
# the operator's ruling (2026-11-17): the two 64-bit targets are required;
# i386 runs only when asked for (`--target i386`) and only the scenario
# files as they are: an i386-only failure is parked, not chased
DEFAULT_TARGETS = ('a64', 'x64')
EMULATION = {'a64': 'aarch64linux', 'x64': 'elf_x86_64', 'i386': 'elf_i386'}
DEFAULT_HOPS = '-, EDIT, -, EDIT'
# the quick subset (one target, gcc): a scenario of each kind of shape
QUICK = ('gc_section_dies_then_revives', 'init_array_grows_then_shrinks', 'warning_mktemp_reference',
         'growth_step_moves_unchanged_then_edit_referrer', 's1_section_permute')
KEYS = ('proves', 'source', 'edit', 'hops', 'link', 'archive', 'thin-archive', 'options', 'cflags', 'compilers',
        'targets', 'not', 'nolibc', 'also-reversed', 'expect-full')


# ------------------------------------------------------------- scenarios --
class Scenario:
    """scenarios/<name>/scenario: `key: value` lines (# comments)."""

    def __init__(self, name, reversed_=False):
        self.name = name + ('.reversed' if reversed_ else '')
        self.base = name
        self.dir = os.path.join(SCENARIOS, name)
        self.edits = {}           # macro -> [source file]
        self.archives = {}        # name -> (thin, [member units])
        self.notes = {}           # target or compiler -> why not
        self.proves = self.source = None
        self.link, self.options, self.cflags = [], [], []
        self.compilers, self.targets = ['gcc'], list(TARGETS)
        self.hops_text = DEFAULT_HOPS
        self.nolibc = False
        self.reversed_why = None
        self.expect_full = None   # (reason, detail) no later hop may be an update
        for n, line in enumerate(open(os.path.join(self.dir, 'scenario')), 1):
            line = line.strip()
            if not line or line.startswith('#'):
                continue
            key, _, value = line.partition(':')
            value = value.strip()
            word, _, arg = key.strip().partition(' ')
            if word not in KEYS or ':' not in line:
                raise ValueError('%s/scenario:%d: unknown line %r' % (name, n, line))
            if word == 'proves':
                self.proves = value
            elif word == 'source':
                self.source = value
            elif word == 'edit':
                self.edits[arg or 'EDIT'] = value.split()
            elif word == 'hops':
                self.hops_text = value
            elif word == 'link':
                self.link = value.split()
            elif word in ('archive', 'thin-archive'):
                # a member is `unit.o` or `unit=name.o` (its name in the
                # archive, when two members share one)
                ms = [m.partition('=') for m in value.split()]
                self.archives[arg] = (word == 'thin-archive', [u[:-2] if u.endswith('.o') else u for u, _e, _n in ms],
                                      {u[:-2] if u.endswith('.o') else u: n for u, _e, n in ms if n})
            elif word == 'options':
                self.options = value.split()
            elif word == 'cflags':
                self.cflags = value.split()
            elif word == 'compilers':
                self.compilers = value.split()
            elif word == 'targets':
                self.targets = value.split()
            elif word == 'not':
                self.notes[arg] = value
            elif word == 'nolibc':
                self.nolibc = value == 'yes'
            elif word == 'also-reversed':
                self.reversed_why = value
            elif word == 'expect-full':
                # `expect-full: <reason> (<detail>): <why>`: no hop after the
                # first may be an update (judge.expect_full)
                head, sep, why = value.partition('): ')
                r, _sp, d = head.partition(' (')
                if not sep or not d or not why:
                    raise ValueError('%s/scenario:%d: `expect-full: <reason> (<detail>): <why>`' % (name, n))
                self.expect_full = (r, d)
        if not self.proves or not self.source or not self.link:
            raise ValueError('%s/scenario: `proves:`, `source:` and `link:` are required' % name)
        self.hops = [frozenset() if h.strip() == '-' else frozenset(h.strip().split('+'))
                     for h in self.hops_text.split(',')]
        if reversed_:
            # the same series from the edited side: the first link never
            # has what the edit removes
            self.hops = [self.hops[1]] + [self.hops[0] if h == self.hops[1] else self.hops[1] for h in self.hops[1:]]
        for m in set().union(*self.hops):
            if m not in self.edits:
                raise ValueError('%s/scenario: hop macro %s has no `edit %s:` line' % (name, m, m))
        self.sources = sorted(f for f in os.listdir(self.dir) if f.endswith(('.c', '.cc', '.S')))
        self.headers = {f: open(os.path.join(self.dir, f)).read() for f in os.listdir(self.dir) if f.endswith('.h')}
        for f in os.listdir(COMMON):
            self.headers.setdefault(f, open(os.path.join(COMMON, f)).read())

    def unit_of(self, src):
        return src.rsplit('.', 1)[0]

    def lang(self, src):
        return {'c': 'c', 'cc': 'c++', 'S': 'asm'}[src.rsplit('.', 1)[1]]

    def why_not(self, compiler, target):
        if compiler not in self.compilers:
            return self.notes.get(compiler, 'not this scenario\'s compiler')
        if target not in self.targets:
            return self.notes.get(target, 'not this scenario\'s target')
        return None


def all_scenarios():
    out = []
    for name in sorted(os.listdir(SCENARIOS)):
        if name.startswith('_') or not os.path.isfile(os.path.join(SCENARIOS, name, 'scenario')):
            continue
        s = Scenario(name)
        out.append(s)
        if s.reversed_why:
            out.append(Scenario(name, reversed_=True))
    return out


def cfg_name(compiler, target):
    return '%s-%s-O0-nog-nofs-static' % (compiler, target)


# ------------------------------------------------------------------- run --
class Run:
    """One scenario on one configuration with one subject."""

    def __init__(self, sc, compiler, target, opts, subject='mortar', wrong_at=None, label=None):
        self.sc, self.opts, self.subject_name, self.wrong_at = sc, opts, subject, wrong_at
        self.cfg = build.Cfg(cfg_name(compiler, target))
        self.compiler, self.target = compiler, target
        self.label = label or '%s.%s-%s' % (sc.name, compiler, target)
        self.base = os.path.join(opts['work'], 'run', self.label)
        self.cache = build.Cache(opts['work'])

    def subject_argv(self):
        s = self.subject_name
        if s == 'mortar':
            return [self.opts['hooked']]
        path = s if os.sep in s else os.path.join(SUBJECTS, s + ('' if s.endswith('.py') else '.py'))
        return [sys.executable, path] if path.endswith('.py') else [path]

    def words(self):
        if self.sc.nolibc:
            return ['-static', '-m', EMULATION[self.target]], []
        return build.capture(self.opts['work'], self.cfg)

    def compiles(self, macros):
        """[(unit, src, lang, text, extra, key)] of the hop's macros."""
        out = []
        for src in self.sc.sources:
            lang = self.sc.lang(src)
            text = open(os.path.join(self.sc.dir, src)).read()
            extra = list(self.sc.cflags) + ['-D%s' % m for m in sorted(macros) if src in self.sc.edits.get(m, ())]
            unit = self.sc.unit_of(src)
            out.append((unit, src, lang, text, extra, build.object_key(self.cfg, unit, lang, text,
                                                                        self.sc.headers, extra)))
        return out

    def objects(self, macros):
        """{unit: cached object path} for the hop's macros."""
        return {unit: self.cache.compile(self.cfg, unit, src, lang, text, self.sc.headers, extra, key)
                for unit, src, lang, text, extra, key in self.compiles(macros)}

    def install(self, W, objs):
        """The hop's inputs into W: [names] (objects, then archives)."""
        names = []
        for unit, path in sorted(objs.items()):
            build.write_if_changed(os.path.join(W, unit + '.o'), open(path, 'rb').read())
            names.append(unit + '.o')
        stage = os.path.join(self.base, 'stage')
        os.makedirs(stage, exist_ok=True)
        for name, (thin, members, renamed) in sorted(self.sc.archives.items()):
            in_ar = {m: renamed.get(m, m + '.o') for m in members} if renamed else None
            data = build.build_archive(self.cfg, stage, name, members, objs, thin, W, in_ar)
            build.write_if_changed(os.path.join(W, name), data)
            names.append(name)
        return names

    def link(self, argv0, cwd, pre, post, extra, env=None):
        argv = argv0 + pre + self.sc.options + self.sc.link + post + extra + ['-o', 'prog']
        try:
            p = subprocess.run(argv, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env,
                               timeout=build.LINK_TIMEOUT, text=True, errors='replace')
            return p.returncode, p.stderr
        except subprocess.TimeoutExpired:
            return 'timeout', 'the link timed out after %ds' % build.LINK_TIMEOUT

    def run(self):
        t0 = time.time()
        res = {'label': self.label, 'hops': [], 'failure': None, 'at': None, 'check': None, 'na': {},
               'reasons': {}}
        if os.path.isdir(self.base):
            shutil.rmtree(self.base)
        W = os.path.join(self.base, 'w')
        os.makedirs(W)
        pre, post = self.words()
        tools = judge.Tools(self.opts['xtask'], self.cfg.runner(), elflint=shutil.which('eu-elflint'))
        env = dict(os.environ)
        # WP-238 P0-7: every link runs the liveness rows' Rust oracle
        # (a testhooks build's; the others ignore the variable). It runs
        # where the state is captured under --gc-sections: the subject's
        # incremental-mode full links of such a scenario; a disagreement
        # fails the link, so the hop.
        env.update({'INCR_MORTAR': self.opts['hooked'], 'PYTHONDONTWRITEBYTECODE': '1',
                    'MORTAR_INCR_LIVE_CHECK': '1'})
        if self.wrong_at is not None:
            env['INCR_WRONG_AT'] = str(self.wrong_at)
        prev = None             # the previous successful hop's G (the oracle's base)
        prev_h = None
        prev_update = False     # ruling M2a2-1: the previous successful hop's subject link was an update
        for i, macros in enumerate(self.sc.hops):
            objs = self.objects(macros)
            names = self.install(W, objs)
            H = os.path.join(self.base, 'h%d' % i)
            Hf = os.path.join(H, 'f')
            os.makedirs(Hf)
            for n in names:
                # copies, not hard links: on the workspace's virtiofs a hard
                # link whose other name was just renamed over opens ENOENT
                # for a while (seen here: listed, then not found)
                shutil.copyfile(os.path.join(W, n), os.path.join(H, n))
                shutil.copyfile(os.path.join(W, n), os.path.join(Hf, n))
            prev_out = None
            if os.path.exists(os.path.join(W, 'prog')):
                prev_out = os.path.join(H, 'prev')
                shutil.copyfile(os.path.join(W, 'prog'), prev_out)
            state = os.path.join(W, 'prog.mortar-state')
            state_before = judge._sha(state)
            fs, ferr = self.link([self.opts['mortar']], Hf, pre, post, ['--mortar-map=prog.map'])
            gs, _gerr = self.link([self.opts['mortar']], H, pre, post, ['--incremental=full', '--mortar-map=prog.map'])
            senv = dict(env)
            senv['INCR_HOP'] = str(i)
            senv['MORTAR_INCR_DIRTY'] = os.path.join(H, 'dirty.txt')
            senv.update({'MORTAR_INCR_SMALL_OUTPUT': '0', 'MORTAR_INCR_STATE4': '1'})  # WP-225: G-13; the dry run's side state (M6)
            xs, xerr = self.link(self.subject_argv(), W, pre, post,
                                 ['--incremental', '--incremental-report=' + os.path.join(H, 'rep.txt'),
                                  '--mortar-map=prog.map'], senv)
            for l in xerr.splitlines():
                if 'incremental: live-check ' in l:
                    w = l.split('incremental: live-check ', 1)[1].split()
                    k = w[0] if w[0] != 'refused' else 'refused ' + w[1].rstrip(':')
                    res.setdefault('live', {})[k] = res.setdefault('live', {}).get(k, 0) + 1
            for src, dst in (('prog', 'x'), ('prog.map', 'x.map')):
                if os.path.exists(os.path.join(W, src)):
                    shutil.copy2(os.path.join(W, src), os.path.join(H, dst))
            x = judge.Link(H, 'x', 'x.map', xs, [], os.path.join(H, 'rep.txt'), stderr=xerr)
            f = judge.Link(Hf, 'prog', 'prog.map', fs, [], stderr=ferr)
            new = judge.Link(H, 'prog', 'prog.map', gs, []) if gs == 0 and fs == 0 else None
            hop = judge.Hop(prev=prev_out, ranges=os.path.join(H, 'dirty.txt'), state=state,
                            state_before=state_before, scratch=H, base_was_update=prev_update)
            v = judge.judge_step(tools, x, f, prev, new, hop)
            if ('oracle', 'n/a base-was-update') in v.checks:
                res['base_was_update'] = res.get('base_was_update', 0) + 1
            if self.sc.expect_full and i > 0:
                judge.expect_full(v, self.sc.expect_full, judge.report_detail(x.report))
            reason = v.reason or ('link-failed' if xs != 0 else 'no-report')
            res['reasons'][reason] = res['reasons'].get(reason, 0) + 1
            for c, r in v.checks:
                if r.startswith('n/a'):
                    res['na'][c] = res['na'].get(c, 0) + 1
            res['hops'].append('hop %d %-14s %s reason=%s%s' % (
                i, '+'.join(sorted(macros)) or '-', v.line(), reason,
                '' if fs == 0 else ' (F status %s: %s)' % (fs, _first(ferr))))
            res.setdefault('verdicts', []).append(v.checks)
            if not v.ok:
                res['failure'] = 'hop %d: %s' % (i, v.first_failure())
                res['at'], res['check'] = i, v.first_failure().split(':')[0]
                break
            if new is not None:
                if prev_h is not None and not self.opts.get('keep'):
                    shutil.rmtree(prev_h)
                prev, prev_h = new, H
                prev_update = xs == 0 and v.reason == 'update'
            elif not self.opts.get('keep'):
                shutil.rmtree(H)
        res['wall'] = time.time() - t0
        if res['failure'] is None and not self.opts.get('keep'):
            shutil.rmtree(self.base)
        return res


def _first(text):
    return next((l for l in text.splitlines() if l.strip()), '')[:160]


def run_one(args):
    sc, compiler, target, opts, subject, wrong_at, label = args
    try:
        return Run(sc, compiler, target, opts, subject, wrong_at, label).run()
    except Exception as e:  # noqa: BLE001 - the lane's own error, reported as the run's failure
        import traceback
        return {'label': label or '%s.%s-%s' % (sc.name, compiler, target), 'hops': [], 'na': {}, 'reasons': {},
                'failure': 'lane error: %s: %s' % (type(e).__name__, e), 'at': None, 'check': 'lane',
                'trace': traceback.format_exc()[-2000:], 'wall': 0}


def pool(jobs, work):
    if jobs <= 1 or len(work) <= 1:
        return [run_one(w) for w in work]
    with concurrent.futures.ProcessPoolExecutor(max_workers=min(jobs, len(work))) as ex:
        return list(ex.map(run_one, work))


def _compile_one(args):
    sc_name, reversed_, compiler, target, opts, unit, src, lang, text, extra, key = args
    sc = Scenario(sc_name, reversed_)
    try:
        build.Cache(opts['work']).compile(build.Cfg(cfg_name(compiler, target)), unit, src, lang, text, sc.headers,
                                          extra, key)
        return None
    except Exception as e:  # noqa: BLE001 - reported by the run that needs the object
        return '%s %s: %s' % (cfg_name(compiler, target), src, str(e)[-600:])


def precompile(runs, opts):
    """Every object the runs need, each compiled once (two workers
    compiling one key would share build.Cache's directory for it)."""
    want = {}
    for sc, compiler, target, *_rest in runs:
        r = Run(sc, compiler, target, opts)
        for macros in sc.hops:
            for unit, src, lang, text, extra, key in r.compiles(macros):
                want.setdefault(key, (sc.base, sc.name != sc.base, compiler, target, opts, unit, src, lang, text,
                                      extra, key))
    jobs = max(1, opts['jobs'])
    if jobs == 1:
        errs = [_compile_one(w) for w in want.values()]
    else:
        with concurrent.futures.ProcessPoolExecutor(max_workers=jobs) as ex:
            errs = list(ex.map(_compile_one, want.values()))
    return [e for e in errs if e]


def precapture(runs, opts):
    """The configurations' link words, captured once before the workers
    start (two workers capturing one configuration would race)."""
    seen = set()
    for sc, compiler, target, *_rest in runs:
        name = cfg_name(compiler, target)
        if not sc.nolibc and name not in seen:
            seen.add(name)
            build.capture(opts['work'], build.Cfg(name))


# ------------------------------------------------------------- self test --
# (subject, scenario, wrong at hop, the check that must catch it first)
SELF_TEST = (
    ('exit_status', 'init_array_grows_then_shrinks', 2, 'exit'),
    ('drop_warning', 'warning_mktemp_reference', 3, 'stderr'),
    ('dup_warning', 'warning_mktemp_reference', 1, 'stderr'),
    ('unlisted_word', 'init_array_grows_then_shrinks', 2, 'dirty'),
    ('missing_range', 'init_array_grows_then_shrinks', 3, 'dirty'),
    ('no_ranges', 'init_array_grows_then_shrinks', 1, 'dirty'),
    ('flip_word', 'gc_section_dies_then_revives', 2, 'verify'),
    ('elflint_bad', 'init_array_grows_then_shrinks', 1, 'elflint'),
    ('wrong_local', 'gc_section_dies_then_revives', 2, 'verify'),
    ('prev_output', 'init_array_grows_then_shrinks', 2, 'verify'),
    ('no_exec', 'init_array_grows_then_shrinks', 1, 'run'),
    ('drop_delta', 'growth_step_moves_unchanged_then_edit_referrer', 1, 'oracle'),
)


def self_test(opts, log):
    """The lane's own test: each wrong subject is found by its check at its
    hop and by no earlier check; the units of the judge (the dirty-range
    rule, the stderr comparison, elflint's allow patterns, the (d)
    machinery around the stub, the duty table against the spec)."""
    import selftest
    lines = selftest.units(log)
    target = build.HOST or 'a64'
    work = []
    for subject, name, at, _check in SELF_TEST:
        sc = Scenario(name)
        work.append((sc, 'gcc', target, opts, subject, at, 'self.%s' % subject))
    precapture(work, opts)
    precompile(work, opts)
    for (subject, name, at, check), r in zip(SELF_TEST, pool(opts['jobs'], work)):
        if r['failure'] is None:
            lines.append('INCR-UPDATE SELF-TEST %s on %s wrong at hop %d: NOT FOUND (the judge passed a wrong '
                         'subject)' % (subject, name, at))
        elif r['at'] != at or r['check'] != check:
            lines.append('INCR-UPDATE SELF-TEST %s on %s wrong at hop %d: WRONG CHECK %s (want %s at hop %d)%s' % (
                subject, name, at, r['failure'], check, at, (' ' + r['trace']) if r.get('trace') else ''))
        else:
            lines.append('INCR-UPDATE SELF-TEST %s on %s: found by %s at hop %d: %s' % (
                subject, name, check, at, r['failure'].split(': ', 2)[-1][:200]))
    for l in lines:
        log(l)
    return all(('NOT' not in l and 'WRONG' not in l and 'FAILS' not in l) for l in lines)


# ------------------------------------------------------------------ main --
def parse(argv):
    o = {'quick': False, 'keep': False, 'jobs': 1, 'self_test': False, 'only': None, 'targets': None,
         'compilers': None, 'subject': 'mortar', 'require_tools': False}
    i = 0
    while i < len(argv):
        a = argv[i]
        if a in ('--mortar', '--hooked', '--xtask', '--work'):
            o[a[2:]] = os.path.abspath(argv[i + 1]); i += 2
        elif a == '--only':
            o['only'] = argv[i + 1].split(','); i += 2
        elif a == '--target':
            o['targets'] = argv[i + 1].split(','); i += 2
        elif a == '--compiler':
            o['compilers'] = argv[i + 1].split(','); i += 2
        elif a == '--subject':
            o['subject'] = argv[i + 1]; i += 2
        elif a == '-j':
            o['jobs'] = max(1, int(argv[i + 1])); i += 2
        elif a in ('--quick', '--keep', '--self-test', '--require-tools'):
            o[a[2:].replace('-', '_')] = True; i += 1
        else:
            sys.exit('lane.py: unknown argument %r\n%s' % (a, __doc__))
    for need in ('mortar', 'hooked', 'xtask', 'work'):
        if need not in o:
            sys.exit('lane.py: --%s is required\n%s' % (need, __doc__))
    return o


def select(o):
    """[(scenario, compiler, target)] to run, and the skips [(label, why)]."""
    scs = all_scenarios()
    if o['only']:
        names = set(o['only'])
        unknown = names - {s.name for s in scs} - {s.base for s in scs}
        if unknown:
            sys.exit('lane.py: no scenario %s' % ', '.join(sorted(unknown)))
        scs = [s for s in scs if s.name in names or s.base in names]
    elif o['quick']:
        scs = [s for s in scs if s.name in QUICK]
    host = build.HOST or 'a64'
    targets = o['targets'] or ([host] if o['quick'] else list(DEFAULT_TARGETS))
    compilers = o['compilers'] or (['gcc'] if o['quick'] else ['gcc', 'kiln'])
    runs, skips = [], []
    for s in scs:
        for c in compilers:
            for t in targets:
                label = '%s.%s-%s' % (s.name, c, t)
                why = s.why_not(c, t)
                if why is None:
                    cfg = build.Cfg(cfg_name(c, t))
                    why = cfg.missing()
                    if s.nolibc and why and c == 'kiln':
                        why = None
                    if why is None and shutil.which('eu-elflint') is None:
                        why = 'eu-elflint not found'
                    if why is not None:
                        why = 'TOOL ' + why
                if why is None:
                    runs.append((s, c, t))
                else:
                    skips.append((label, why))
    return runs, skips


def main(argv):
    o = parse(argv)
    os.makedirs(o['work'], exist_ok=True)
    log = lambda s: print(s, flush=True)
    if o['self_test']:
        ok = self_test(o, log)
        log('incr-update: self-test %s' % ('passes' if ok else 'FAILS'))
        return 0 if ok else 1
    t0 = time.time()
    runs, skips = select(o)
    if o['quick']:
        # the self test's fast units ride along with the quick subset
        import selftest
        lines = selftest.units(log)
        for l in lines:
            log(l)
        if any(('NOT' in l or 'WRONG' in l or 'FAILS' in l) for l in lines):
            log('incr-update: the self test\'s units FAIL')
            return 1
    for label, why in skips:
        if why.startswith('TOOL '):
            log('INCR-UPDATE SKIP %s: %s' % (label, why[5:]))
    if o['require_tools'] and any(w.startswith('TOOL ') for _l, w in skips):
        log('incr-update: a configuration cannot run here and MORTAR_REQUIRE_TOOLS=1')
        return 1
    work = [(s, c, t, o, o['subject'], None, None) for s, c, t in runs]
    precapture(work, o)
    for e in precompile(work, o):
        log('INCR-UPDATE COMPILE-ERROR %s' % e)
    results = pool(o['jobs'], work)
    hops = npass = nfail = 0
    na = {}
    base_upd = sum(r.get('base_was_update', 0) for r in results)
    for r in results:
        for l in r['hops']:
            log('  %s %s' % (r['label'], l))
        hops += len(r['hops'])
        nfail += 1 if r['failure'] else 0
        npass += len(r['hops']) - (1 if r['failure'] else 0)
        for c, k in r['na'].items():
            na[c] = na.get(c, 0) + k
        log('INCR-UPDATE RESULT %s verdict=%s%s reasons=%s wall=%.1fs' % (
            r['label'], 'FAIL at=' + r['failure'] if r['failure'] else 'PASS', '',
            ','.join('%s:%d' % kv for kv in sorted(r['reasons'].items())) or '-', r['wall']))
        if r.get('trace'):
            log(r['trace'])
    live = {}
    for r in results:
        for k, n in r.get('live', {}).items():
            live[k] = live.get(k, 0) + n
    log('incr-update: live-check (WP-238 P0-7): %d links: %s' % (
        sum(live.values()), ' '.join('%s:%d' % kv for kv in sorted(live.items())) or '-'))
    names = {s.name for s, _c, _t in runs}
    log('incr-update: %d scenarios, %d hops: %d PASS, %d FAIL, %d n/a (%s), %d base-was-update; %d runs, '
        '%d not here (scenario: %d, tools: %d); %.0fs' % (
            len(names), hops, npass, nfail, sum(na.values()),
            ' '.join('%s:%d' % kv for kv in sorted(na.items())) or '-', base_upd, len(runs), len(skips),
            sum(1 for _l, w in skips if not w.startswith('TOOL ')),
            sum(1 for _l, w in skips if w.startswith('TOOL ')), time.time() - t0))
    return 1 if nfail else 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
