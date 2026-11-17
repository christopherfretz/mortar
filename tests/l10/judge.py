"""How one L10 step is judged (spec/incr.md IN-1, IN-62, IN-63, IN-63a,
IN-79; WP-067).

    judge_step(tools, x, f, base, new) -> Verdict

  tools  Tools: the xtask binary (`xtask plumb`, `xtask plumb --equiv`)
         and the program runner (an argv prefix: [] or qemu)
  x      Link: the SUBJECT's link of the step (the update under test;
         today `mortar --incremental`, or any command put in its place)
  f      Link: the plain full link F of the same inputs and options
  base   Link or None: the incremental-mode full link of the PREVIOUS
         inputs (IN-63a's first link), in a directory holding its inputs
  new    Link or None: the incremental-mode full link of these inputs
         (IN-63a's second link), likewise

A Link is the link's directory, output and map file names, its exit
status, its diagnostics (normalised: output path and incremental-mode
notes removed) and, for the subject, its `--incremental-report` file.

The checks, in order (each `pass`, `FAIL <why>` or `n/a <why>`):
  exit     x exits as f does, with the same diagnostics (IN-4)
  verify   `xtask plumb` accepts x's output with its map (IN-1 (A))
  equiv    `xtask plumb --equiv x.map f.map x f` has no finding (IN-1 (C),
           IN-62); for a subject that linked plainly (no `incremental` in
           its map: an option IN-12 refuses) x and f must be byte-equal
  run      x's program's transcript (status and output) equals f's (IN-79 (a))
  oracle   IN-63a between base and new judged against x's report (IN-79 (c)):
           n/a without a previous full link or without a dry run's
           prediction in the report
and the report's first line is the step's REASON (`update`, or the
fall-back's name).

Nothing here is L10-specific: H-1 (IN-79's three-way judge of one
update) calls the same function with its own links.
"""

import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, '..', 'incr-oracle'))
sys.dont_write_bytecode = True
import oracle  # noqa: E402  (tests/incr-oracle/oracle.py, the IN-63a oracle, as a library)

CHECKS = ('exit', 'verify', 'equiv', 'run', 'oracle')


def _thin_member(archive, off):
    """The file a thin archive's member header at `off` names (relative to
    the archive), or None for a regular archive."""
    with open(archive, 'rb') as f:
        if f.read(8) != b'!<thin>\n':
            return None
        f.seek(off)
        name = f.read(16).decode('latin1').rstrip()
        if name.startswith('/') and name[1:].isdigit():
            f.seek(8)
            while True:          # the `//` long-name table
                h = f.read(60)
                if len(h) < 60:
                    return None
                size = int(h[48:58].decode().strip())
                if h[:3] == b'// ':
                    table = f.read(size).decode('latin1')
                    at = int(name[1:])
                    name = table[at:table.index('\n', at)]
                    break
                if h[:1] == b'/':
                    f.seek(size + (size & 1), 1)
                else:
                    return None
        name = name.rstrip('/')
    return os.path.join(os.path.dirname(archive), name)


_file_bytes = oracle.Link.file_bytes


def _file_bytes_thin(self, key):
    """oracle.Link.file_bytes, reading a thin archive's member from its
    own file (tests/incr-oracle reads members at their offset in the
    archive, which holds only headers for a thin one: WP-067 finding)."""
    path, mem, off = key
    if mem is not None:
        off = self.real_offset.get(key, off)
        key = (path, mem, off)
    if mem is not None:
        p = path if os.path.isabs(path) else os.path.join(self.d, path)
        member = _thin_member(p, off)
        if member is not None:
            with open(member, 'rb') as f:
                return f.read()
    return _file_bytes(self, key)


oracle.Link.file_bytes = _file_bytes_thin
_link_init = oracle.Link.__init__


def _link_init_by_name(self, d):
    """oracle.Link.__init__, with an archive member identified by (archive,
    member name, ordinal among the members of that name) instead of its
    offset: a member that an earlier member's growth moves is the same
    file, as mortar's change detection (IN-24) and its report (which names
    path, member, section) see it. tests/incr-oracle's archives never
    change, so its identity by offset never met this (WP-067 finding)."""
    _link_init(self, d)
    seen, remap, self.real_offset = {}, {}, {}
    for fid in sorted(self.files):
        path, mem, off = self.files[fid]
        if mem is None:
            continue
        n = seen.get((path, mem), 0)
        seen[(path, mem)] = n + 1
        new = (path, mem, n)
        remap[(path, mem, off)] = new
        self.real_offset[new] = off
    if not remap:
        return
    rk = lambda k: remap.get(k, k)
    self.files = {i: rk(k) for i, k in self.files.items()}
    self.units = {(rk(k), kind, sec): v for (k, kind, sec), v in self.units.items()}
    self.ranges = {(rk(k), kind, sec): v for (k, kind, sec), v in self.ranges.items()}
    self.discard = {(rk(k), sec) for (k, sec) in self.discard}
    self.keys = set(self.files.values())


oracle.Link.__init__ = _link_init_by_name
PLUMB_TIMEOUT = 300


class Tools:
    def __init__(self, xtask, runner, run_timeout=60):
        self.xtask, self.runner, self.run_timeout = xtask, runner, run_timeout


class Link:
    def __init__(self, d, out, map_, status, diags, report=None):
        self.dir, self.out, self.map, self.status, self.diags, self.report = d, out, map_, status, diags, report

    def path(self, name):
        return os.path.join(self.dir, name)


class Verdict:
    def __init__(self):
        self.checks = []          # (name, 'pass' | 'FAIL ...' | 'n/a ...')
        self.reason = None        # 'update' | fall-back name | None

    def add(self, name, result):
        self.checks.append((name, result))

    @property
    def ok(self):
        return not any(r.startswith('FAIL') for _n, r in self.checks)

    def first_failure(self):
        return next(('%s: %s' % (n, r) for n, r in self.checks if r.startswith('FAIL')), None)

    def line(self):
        return ' '.join('%s=%s' % (n, r.split()[0]) for n, r in self.checks)


def report_reason(path):
    """The report's reason: `update` or the fall-back's name; None without
    a report."""
    try:
        lines = open(path).read().splitlines()
    except OSError:
        return None
    for line in lines:
        t = line.split()
        if len(t) >= 2 and t[0] == 'incremental:':
            if t[1] == 'update':
                return 'update'
            if t[1] == 'full' and len(t) >= 3:
                return t[2].strip('():')
    return None


def _run(argv, cwd, timeout):
    try:
        p = subprocess.run(argv, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=timeout,
                           text=True, errors='replace')
        return p.returncode, p.stdout
    except subprocess.TimeoutExpired:
        return None, 'timed out after %ds' % timeout


def _tail(text, n=2):
    lines = [l for l in text.splitlines() if l.strip()]
    return ' | '.join(lines[-n:])[:300]


def run_program(tools, link):
    """(status, stdout) of the program, run in its directory."""
    argv = tools.runner + ['./' + link.out]
    try:
        p = subprocess.run(argv, cwd=link.dir, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                           timeout=tools.run_timeout)
        return p.returncode, p.stdout
    except subprocess.TimeoutExpired:
        return 'timeout', b''


def judge_step(tools, x, f, base, new):
    v = Verdict()
    v.reason = report_reason(x.report) if x.report else None
    # exit: same status, same diagnostics
    if x.status != f.status:
        v.add('exit', 'FAIL status %s, the full link %s' % (x.status, f.status))
    elif x.diags != f.diags:
        missing = [d for d in f.diags if d not in x.diags]
        extra = [d for d in x.diags if d not in f.diags]
        v.add('exit', 'FAIL diagnostics differ: missing %r extra %r' % (missing[:2], extra[:2]))
    else:
        v.add('exit', 'pass')
    if x.status != 0 or f.status != 0:
        for c in CHECKS[1:]:
            v.add(c, 'n/a the link failed')
        return v
    # verify
    rc, out = _run([tools.xtask, 'plumb', x.out, '--map', x.map, '--map-format', 'mortar'], x.dir, PLUMB_TIMEOUT)
    v.add('verify', 'pass' if rc == 0 else 'FAIL plumb exit %s: %s' % (rc, _tail(out)))
    # equiv
    incremental = any(l.startswith('incremental') for l in open(x.path(x.map)))
    if incremental:
        rc, out = _run([tools.xtask, 'plumb', '--equiv', x.map, f.map, x.out, f.out], x.dir, PLUMB_TIMEOUT)
        v.add('equiv', 'pass' if rc == 0 else 'FAIL equiv exit %s: %s' % (rc, _tail(out)))
    else:
        same = open(x.path(x.out), 'rb').read() == open(f.path(f.out), 'rb').read()
        v.add('equiv', 'pass' if same else 'FAIL a plain link (no `incremental` in its map) differs from F in bytes')
    # run
    rx, rf = run_program(tools, x), run_program(tools, f)
    if rx != rf:
        a, b = rx[1].decode('latin1').splitlines(), rf[1].decode('latin1').splitlines()
        i = next((i for i, (p, q) in enumerate(zip(a, b)) if p != q), min(len(a), len(b)))
        v.add('run', 'FAIL status %s vs %s; line %d: %r vs %r' % (rx[0], rf[0], i + 1, (a + [''])[i][:80],
                                                                    (b + [''])[i][:80]))
    elif rf[0] != 0:
        v.add('run', 'FAIL both programs exit %s' % rf[0])
    else:
        v.add('run', 'pass')
    # oracle (IN-63a)
    if base is None or new is None or not x.report or not os.path.exists(x.report):
        v.add('oracle', 'n/a no previous full link')
        return v
    try:
        R = oracle.Report(x.report)
    except Exception as e:  # noqa: BLE001 - a report the oracle cannot read is the subject's failure
        v.add('oracle', 'FAIL the report cannot be read: %s' % e)
        return v
    if not R.ended and R.reason in oracle.DRY_RUN_REASONS[1:]:
        v.add('oracle', 'FAIL the report has no `delta end`')
        return v
    if not R.ended:
        v.add('oracle', 'n/a no prediction in the report (%s)' % (R.reason or v.reason or 'none'))
        return v
    try:
        F = oracle.Facts(base.dir, new.dir)
        ok, _npred, lines = oracle.judge(F, R, set())
    except Exception as e:  # noqa: BLE001 - an oracle crash is a lane failure, reported as such
        v.add('oracle', 'FAIL the oracle raised %s: %s' % (type(e).__name__, e))
        return v
    v.add('oracle', 'pass' if ok else 'FAIL %s' % (lines[0] if lines else '?'))
    return v
