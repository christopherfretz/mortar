"""How one incremental link is judged: one L10 step (spec/incr.md IN-1,
IN-62, IN-63, IN-63a; WP-067) and, with `hop`, one update of IN-79's four
ways (IN-75, IN-75a, IN-78, IN-79; H-1, WP-223). ONE function for L10, for
`cargo xtask incr-update` (tests/incr-update) and for the crash series
(H-2): the file stays here, where L10 put it, so L10's import is unchanged.

    judge_step(tools, x, f, base, new, hop=None) -> Verdict

  tools  Tools: the xtask binary (`xtask plumb`, `xtask plumb --equiv`)
         and the program runner (an argv prefix: [] or qemu)
  x      Link: the SUBJECT's link (the update under test; today `mortar
         --incremental`, or any command put in its place)
  f      Link: the plain full link F of the same inputs and options
  base   Link or None: the incremental-mode full link of the PREVIOUS
         inputs (IN-63a's first link), in a directory holding its inputs
  new    Link or None: the incremental-mode full link of these inputs
         (IN-63a's second link), likewise
  hop    Hop or None: what IN-79 needs beyond L10's five checks (the
         previous output, the dirty ranges' file, the state, elflint).
         None: L10's five checks, exactly as WP-067 wrote them.

A Link is the link's directory, output and map file names, its exit
status, its diagnostics (L10: normalised by the lane; IN-79: `stderr`, the
raw standard error) and, for the subject, its `--incremental-report` file.

L10's checks (hop=None), in order (each `pass`, `FAIL <why>` or `n/a <why>`):
  exit     x exits as f does, with the same diagnostics (IN-4)
  verify   `xtask plumb` accepts x's output with its map (IN-1 (A))
  equiv    `xtask plumb --equiv x.map f.map x f` has no finding (IN-1 (C),
           IN-62); for a subject that linked plainly (no `incremental` in
           its map: an option IN-12 refuses) x and f must be byte-equal
  run      x's program's transcript (status and output) equals f's (IN-79 (a))
  oracle   IN-63a between base and new judged against x's report (IN-79 (c)):
           n/a without a previous full link or without a dry run's
           prediction in the report

IN-79's checks (hop given), in order (IN79_CHECKS; spec/harness.md
"incr-update" is the text):
  exit     the exit status equals f's (IN-4; `--fatal-warnings`: the
           update fails as the full link fails); a failed subject link
           leaves the state it found (IN-48: a link with an error writes
           no state)
  stderr   (d) x's standard error equals f's, line for line, in order,
           modulo the `incremental:` report line (IN-75a)
  dirty    IN-75: every byte in which x's output differs from the previous
           output lies inside a range of the subject's MORTAR_INCR_DIRTY
           file; the ranges are sorted, disjoint and inside the file
  verify   as L10's
  elflint  (b) `eu-elflint --gnu-ld` on x's output: every complaint is one
           of harness.md's four allow patterns (ELFLINT_ALLOW)
  equiv    (b) as L10's, with the update's own map
  run      (a) as L10's
  oracle   (c) as L10's
  state    (d) state_equals_capture_of_its_placement (IN-78): canon(S')
           against canon(C(U)); today through the stub of `incr_state
           canon`: "n/a (d) not run (canon: U-2)"
  duty     (d) duty_list_covers_differences (IN-78): every row in which
           canon(S) and canon(C(U)) differ is a table of a cause of
           IN-78's derived table (tests/incr-update/duty.py); same stub
and the report's first line is the step's REASON (`update`, or the
fall-back's name).
"""

import bisect
import hashlib
import os
import re
import subprocess
import time
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, '..', 'incr-oracle'))
sys.path.insert(0, os.path.join(HERE, '..', 'incr-update'))
sys.dont_write_bytecode = True
import oracle  # noqa: E402  (tests/incr-oracle/oracle.py, the IN-63a oracle, as a library)

CHECKS = ('exit', 'verify', 'equiv', 'run', 'oracle')
IN79_CHECKS = ('exit', 'stderr', 'dirty', 'verify', 'elflint', 'equiv', 'run', 'oracle', 'state', 'duty')
PLUMB_TIMEOUT = 300
ELFLINT_TIMEOUT = 120


class Tools:
    def __init__(self, xtask, runner, run_timeout=60, args=(), elflint=None):
        self.xtask, self.runner, self.run_timeout, self.args = xtask, runner, run_timeout, tuple(args)
        self.elflint = elflint


class Link:
    def __init__(self, d, out, map_, status, diags, report=None, stderr=None):
        self.dir, self.out, self.map, self.status, self.diags, self.report = d, out, map_, status, diags, report
        self.stderr = stderr

    def path(self, name):
        return os.path.join(self.dir, name)


class Hop:
    """What IN-79 needs of one update beyond L10's checks.

    prev          the previous output's bytes as they were just before the
                  subject's link (a file), or None (a first link)
    ranges        the path MORTAR_INCR_DIRTY named for the subject's link
    state         the state the subject's link left (S'), or None
    state_before  sha256 of the state before the link (None: no state)
    state_prev    a copy of the state before the link (S), for the duty
                  check, or None (made only when canon is live)
    scratch       the directory where C(U), the replay's capture, goes
    canon         the (d) comparator (CANON_STUB until U-2: not run)
    causes        the causes of IN-78's derived table this update has (a
                  set of duty.py ids), or None (any cause)
    """

    def __init__(self, prev=None, ranges=None, state=None, state_before=None, state_prev=None, scratch=None,
                 canon=None, causes=None):
        self.prev, self.ranges, self.state, self.state_before = prev, ranges, state, state_before
        self.state_prev, self.scratch, self.canon, self.causes = state_prev, scratch, canon or CANON_STUB, causes


class Verdict:
    def __init__(self):
        self.checks = []          # (name, 'pass' | 'FAIL ...' | 'n/a ...')
        self.reason = None        # 'update' | fall-back name | None
        self.times = {}           # check -> seconds (the lane's cost table)
        self._t = time.time()

    def add(self, name, result):
        self.checks.append((name, result))
        now = time.time()
        self.times[name] = now - self._t
        self._t = now

    @property
    def ok(self):
        return not any(r.startswith('FAIL') for _n, r in self.checks)

    def first_failure(self):
        return next(('%s: %s' % (n, r) for n, r in self.checks if r.startswith('FAIL')), None)

    def line(self):
        return ' '.join('%s=%s' % (n, r.split()[0]) for n, r in self.checks)


def report_line(path):
    """The report's `incremental:` line split in words, or None."""
    try:
        lines = open(path).read().splitlines()
    except OSError:
        return None
    for line in lines:
        t = line.split()
        if len(t) >= 2 and t[0] == 'incremental:':
            return t
    return None


def report_reason(path):
    """The report's reason: `update` or the fall-back's name; None without
    a report."""
    t = report_line(path)
    if t is None:
        return None
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
    argv = tools.runner + ['./' + link.out] + list(tools.args)
    try:
        p = subprocess.run(argv, cwd=link.dir, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                           timeout=tools.run_timeout)
        return p.returncode, p.stdout
    except subprocess.TimeoutExpired:
        return 'timeout', b''
    except OSError as e:
        # the program cannot be started (no execute permission, ...): the
        # run's verdict, not the lane's error
        return 'cannot run: %s' % e.strerror, b''


# --------------------------------------------------------------- the checks --
def _exit_l10(v, x, f):
    if x.status != f.status:
        v.add('exit', 'FAIL status %s, the full link %s' % (x.status, f.status))
    elif x.diags != f.diags:
        missing = [d for d in f.diags if d not in x.diags]
        extra = [d for d in x.diags if d not in f.diags]
        v.add('exit', 'FAIL diagnostics differ: missing %r extra %r' % (missing[:2], extra[:2]))
    else:
        v.add('exit', 'pass')


def _sha(path):
    try:
        with open(path, 'rb') as fh:
            return hashlib.sha256(fh.read()).hexdigest()
    except OSError:
        return None


def _exit_in79(v, x, f, hop):
    if x.status != f.status:
        v.add('exit', 'FAIL status %s, the full link %s' % (x.status, f.status))
    elif x.status != 0 and hop.state is not None and _sha(hop.state) != hop.state_before:
        v.add('exit', 'FAIL the link failed (status %s) and its state changed (IN-48: a link with an error '
                      'writes no state)' % x.status)
    else:
        v.add('exit', 'pass')


def stderr_lines(text):
    """IN-75a's comparison: standard error's lines, in order, without the
    `incremental:` report line (a line that begins `incremental:`; every
    diagnostic of mortar begins `mortar: ` or `>>> `, spec/diag.md, so no
    diagnostic is dropped by it). Nothing else is normalised: the lane
    links the subject and the full link under the same output name with
    the same relative input names, so no path needs rewriting, and blank
    lines, order and repeats all count."""
    return [l for l in text.splitlines() if not l.startswith('incremental:')]


def _stderr(v, x, f):
    if x.stderr is None or f.stderr is None:
        v.add('stderr', 'n/a the lane did not capture standard error')
        return
    a, b = stderr_lines(x.stderr), stderr_lines(f.stderr)
    if a == b:
        v.add('stderr', 'pass')
        return
    i = next((i for i, (p, q) in enumerate(zip(a, b)) if p != q), min(len(a), len(b)))
    what = ''
    k = abs(len(a) - len(b))
    if len(a) < len(b) and a == b[:i] + b[i + k:]:
        what = '%d line(s) dropped: ' % k
    elif len(a) > len(b) and b == a[:i] + a[i + k:]:
        what = '%d line(s) repeated: ' % k if a[i:i + k] in (a[i - k:i], a[i + k:i + 2 * k]) else \
            '%d line(s) added: ' % k
    v.add('stderr', 'FAIL %sline %d of %d/%d: %r, the full link %r' % (
        what, i + 1, len(a), len(b), (a + ['<none>'])[i][:120], (b + ['<none>'])[i][:120]))


def read_ranges(path):
    """The MORTAR_INCR_DIRTY file: [(offset, len)] (mortar_incr::testhook,
    `0x<offset> 0x<len>` lines). Raises ValueError on a malformed line."""
    out = []
    with open(path) as fh:
        for n, line in enumerate(fh, 1):
            t = line.split()
            if not t:
                continue
            if len(t) != 2 or not t[0].startswith('0x') or not t[1].startswith('0x'):
                raise ValueError('line %d: %r is not `0x<offset> 0x<len>`' % (n, line.strip()[:60]))
            out.append((int(t[0], 16), int(t[1], 16)))
    return out


def section_at(data, off):
    """The name of the output section whose file bytes hold `off`, or the
    header it lies in."""
    try:
        c64, _m, S, P = oracle.elf(data)
    except Exception:  # noqa: BLE001 - a name for a message only
        return '?'
    ehsize = 64 if c64 else 52
    if off < ehsize:
        return 'the ELF header'
    for s in S:
        if s['type'] != 8 and s['size'] and s['off'] <= off < s['off'] + s['size']:
            return s['name']
    shoff, shent = ((int.from_bytes(data[0x28:0x30], 'little'), 64) if c64 else
                    (int.from_bytes(data[0x20:0x24], 'little'), 40))
    if shoff <= off < shoff + shent * len(S):
        return 'the section header table'
    phoff = int.from_bytes(data[0x20:0x28], 'little') if c64 else int.from_bytes(data[0x1c:0x20], 'little')
    if phoff <= off < phoff + (56 if c64 else 32) * len(P):
        return 'the program header table'
    return 'no section (padding)'


def dirty_check(prev, new, ranges):
    """IN-75's invariant over bytes: None when every byte of `new` that
    differs from `prev` lies in a range and the ranges are sorted,
    disjoint and inside `new`; else why not."""
    last = 0
    for i, (o, n) in enumerate(ranges):
        if o < last:
            return 'range %d (0x%x 0x%x) is not sorted and disjoint (the previous one ends at 0x%x)' % (i + 1, o, n,
                                                                                                    last)
        if o + n > len(new):
            return 'range %d (0x%x 0x%x) ends past the file (0x%x bytes)' % (i + 1, o, n, len(new))
        last = o + n
    starts = [o for o, _n in ranges]

    def covered(p):
        j = bisect.bisect_right(starts, p) - 1
        return j >= 0 and p < ranges[j][0] + ranges[j][1]

    block = 4096
    common = min(len(prev), len(new))
    for at in range(0, common, block):
        if prev[at:at + block] == new[at:at + block]:
            continue
        for p in range(at, min(at + block, common)):
            if prev[p] != new[p] and not covered(p):
                return 'byte 0x%x differs (0x%02x -> 0x%02x) outside every dirty range, in %s' % (
                    p, prev[p], new[p], section_at(new, p))
    for p in range(common, len(new)):
        if not covered(p):
            return 'byte 0x%x (past the previous output\'s 0x%x bytes) outside every dirty range, in %s' % (
                p, len(prev), section_at(new, p))
    return None


def _dirty(v, x, hop):
    t = report_line(x.report) if x.report else None
    if v.reason != 'update':
        v.add('dirty', 'n/a (full link%s)' % (': ' + v.reason if v.reason else ''))
        return
    if len(t) >= 3 and t[2] == '(floor)':
        v.add('dirty', 'n/a (full link: the floor, WP-061\'s test path, writes no ranges)')
        return
    if hop.prev is None or not os.path.exists(hop.prev):
        v.add('dirty', 'FAIL `incremental: update` without a previous output')
        return
    if not hop.ranges or not os.path.exists(hop.ranges):
        v.add('dirty', 'FAIL no ranges file after `incremental: update` (MORTAR_INCR_DIRTY, a testhooks build)')
        return
    try:
        ranges = read_ranges(hop.ranges)
    except (OSError, ValueError) as e:
        v.add('dirty', 'FAIL the ranges file: %s' % e)
        return
    with open(hop.prev, 'rb') as fh:
        prev = fh.read()
    with open(x.path(x.out), 'rb') as fh:
        new = fh.read()
    why = dirty_check(prev, new, ranges)
    v.add('dirty', 'pass (%d ranges, %d bytes)' % (len(ranges), sum(n for _o, n in ranges)) if why is None
          else 'FAIL ' + why)


def _verify(v, tools, x):
    rc, out = _run([tools.xtask, 'plumb', x.out, '--map', x.map, '--map-format', 'mortar'], x.dir, PLUMB_TIMEOUT)
    v.add('verify', 'pass' if rc == 0 else 'FAIL plumb exit %s: %s' % (rc, _tail(out)))


# harness.md "Checks", `check elflint`: the four complaints every static
# glibc output gets from `eu-elflint --gnu-ld`, for mortar, GNU ld and lld
# alike (review-in72 T-1). Each is held to what the text says it is: an
# IRELATIVE record of the PLT's relocation section, the symbol
# `__ehdr_start`, and the variant PCS bit of an AArch64 output.
ELFLINT_ALLOW = (
    ('irelative-type', re.compile(r"^section \[ *\d+\] '(\.rela?\.plt)': relocation (\d+): invalid type$")),
    ('irelative-symbol', re.compile(r"^section \[ *\d+\] '(\.rela?\.plt)': relocation (\d+): invalid symbol index$")),
    ('ehdr-start', re.compile(r"^section \[ *\d+\] '\.symtab': symbol \d+ \(__ehdr_start\): st_value out of bounds$")),
    ('variant-pcs', re.compile(r"^section \[ *\d+\] '\.symtab': symbol \d+ \([^)]*\): unknown bit set in st_other$")),
)
IRELATIVE = {183: 1032, 62: 37, 3: 42}   # e_machine -> R_*_IRELATIVE


def _plt_types(data):
    """{relocation section name: [type of record i]} of the output."""
    c64, mach, S, _P = oracle.elf(data)
    out = {}
    for s in S:
        if s['name'] in ('.rela.plt', '.rel.plt'):
            rela = s['name'] == '.rela.plt'
            ent = (24 if rela else 16) if c64 else (12 if rela else 8)
            types = []
            for i in range(s['size'] // ent):
                info_at = s['off'] + i * ent + (8 if c64 else 4)
                info = int.from_bytes(data[info_at:info_at + (8 if c64 else 4)], 'little')
                types.append(info & 0xffffffff if c64 else info & 0xff)
            out[s['name']] = types
    return mach, out


def elflint_complaints(text, data):
    """The complaints of `eu-elflint --gnu-ld` that no allow pattern
    allows, each as text."""
    mach, plt = None, None
    bad = []
    for line in text.splitlines():
        line = line.strip()
        if not line or line == 'No errors':
            continue
        ok = False
        for name, rx in ELFLINT_ALLOW:
            m = rx.match(line)
            if not m:
                continue
            if mach is None:
                mach, plt = _plt_types(data)
            if name.startswith('irelative'):
                types = plt.get(m.group(1), [])
                i = int(m.group(2))
                ok = i < len(types) and types[i] == IRELATIVE.get(mach)
            elif name == 'variant-pcs':
                ok = mach == 183
            else:
                ok = True
            break
        if not ok:
            bad.append(line)
    return bad


def _elflint(v, tools, x):
    if not tools.elflint:
        v.add('elflint', 'n/a eu-elflint not found')
        return
    rc, out = _run([tools.elflint, '--gnu-ld', x.out], x.dir, ELFLINT_TIMEOUT)
    if rc is None:
        v.add('elflint', 'FAIL eu-elflint %s' % out)
        return
    with open(x.path(x.out), 'rb') as fh:
        data = fh.read()
    bad = elflint_complaints(out, data)
    v.add('elflint', 'pass' if not bad else 'FAIL %s%s' % (bad[0][:200], (' (and %d more)' % (len(bad) - 1))
                                                         if len(bad) > 1 else ''))


def _equiv(v, tools, x, f):
    incremental = any(l.startswith('incremental') for l in open(x.path(x.map)))
    if incremental:
        rc, out = _run([tools.xtask, 'plumb', '--equiv', x.map, f.map, x.out, f.out], x.dir, PLUMB_TIMEOUT)
        v.add('equiv', 'pass' if rc == 0 else 'FAIL equiv exit %s: %s' % (rc, _tail(out)))
    else:
        same = open(x.path(x.out), 'rb').read() == open(f.path(f.out), 'rb').read()
        v.add('equiv', 'pass' if same else 'FAIL a plain link (no `incremental` in its map) differs from F in bytes')


def _run_check(v, tools, x, f):
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


def _oracle(v, x, base, new):
    if base is None or new is None or not x.report or not os.path.exists(x.report):
        v.add('oracle', 'n/a no previous full link')
        return
    try:
        R = oracle.Report(x.report)
    except Exception as e:  # noqa: BLE001 - a report the oracle cannot read is the subject's failure
        v.add('oracle', 'FAIL the report cannot be read: %s' % e)
        return
    if not R.ended and R.reason in oracle.DRY_RUN_REASONS[1:]:
        v.add('oracle', 'FAIL the report has no `delta end`')
        return
    if not R.ended:
        v.add('oracle', 'n/a no prediction in the report (%s)' % (R.reason or v.reason or 'none'))
        return
    if R.elided:
        # IN-10b: a report past the bound is one the oracle cannot judge (protoc's are)
        v.add('oracle', 'n/a the report elided lines (IN-10b): %s' % '; '.join(R.elided))
        return
    note = ''
    for attempt in (1, 2):
        try:
            F = oracle.Facts(base.dir, new.dir)
            ok, _npred, lines = oracle.judge(F, R, set())
            break
        except FileNotFoundError as e:
            # seen once on the workspace's virtiofs under load: ENOENT for a
            # file that exists when looked at again. One retry, named.
            if attempt == 1 and e.filename and os.path.exists(e.filename):
                note = ' (after a retry: %s)' % e
                continue
            v.add('oracle', 'FAIL the oracle raised %s: %s' % (type(e).__name__, e))
            return
        except Exception as e:  # noqa: BLE001 - an oracle crash is a lane failure, reported as such
            v.add('oracle', 'FAIL the oracle raised %s: %s' % (type(e).__name__, e))
            return
    v.add('oracle', ('pass' + note) if ok else 'FAIL %s' % (lines[0] if lines else '?'))


# ------------------------------------------------- (d): the state invariant --
class NotRun:
    """A (d) half that cannot run yet, and why."""

    def __init__(self, why):
        self.why = why


class CanonStub:
    """Stands for `incr_state canon` (U-2: `mortar_incr::state5::canon`
    and `compare`, IN-78) until U-2 lands. `compare(a, b, p_u)` returns
    NotRun, or the differences of canon(a) against canon(b) as a list of
    (table, row, what): the table's name (spec/incr.md's table names, as
    duty.py has them), the row's canonical key, what differs; [] = equal."""

    live = False

    def compare(self, a, b, p_u):
        return NotRun('(d) not run (canon: U-2)')


CANON_STUB = CanonStub()


def placement_of_map(map_path):
    """P_U, the update's Placement, read from the decision records of its
    map (IN-78, IN-79 "What (d) needs from the map"; image.md I-78a,
    plumb.md §3.2): V-1's reader of the records (WP-222) and U-6's writer
    of them. Until both exist: (None, why)."""
    return None, 'P_U: decision records not read'


def first_difference(diffs):
    """How the comparison's first differing row is printed."""
    table, row, what = diffs[0]
    more = ' (and %d more rows)' % (len(diffs) - 1) if len(diffs) > 1 else ''
    return '%s row %s: %s%s' % (table, row, what, more)


def _state(v, x, hop):
    p_u, why = placement_of_map(x.path(x.map))
    capture = os.path.join(hop.scratch or x.dir, 'capture.mortar-state')   # where C(U) goes
    res = hop.canon.compare(hop.state, capture, p_u) if hop.canon.live else hop.canon.compare(None, None, None)
    if isinstance(res, NotRun):
        v.add('state', 'n/a %s; %s' % (res.why, why))
        return
    if v.reason != 'update':
        v.add('state', 'n/a (full link%s)' % (': ' + v.reason if v.reason else ''))
        return
    if res:
        v.add('state', 'FAIL canon(S\') differs from canon(C(U)): %s' % first_difference(res))
        return
    v.add('state', 'pass%s' % ('' if p_u is not None else ' (decision rows not compared: %s)' % why))


def _duty(v, x, hop):
    import duty  # tests/incr-update/duty.py: IN-78's derived table as data
    capture = os.path.join(hop.scratch or x.dir, 'capture.mortar-state')
    res = hop.canon.compare(hop.state_prev, capture, None) if hop.canon.live else hop.canon.compare(None, None,
                                                                                                    None)
    if isinstance(res, NotRun):
        v.add('duty', 'n/a %s' % res.why)
        return
    if v.reason != 'update':
        v.add('duty', 'n/a (full link%s)' % (': ' + v.reason if v.reason else ''))
        return
    why = duty.uncovered(res, hop.causes)
    v.add('duty', 'pass (%d rows differ)' % len(res) if why is None else 'FAIL ' + why)


# ------------------------------------------------------------------ judge --
def judge_step(tools, x, f, base, new, hop=None):
    v = Verdict()
    v.reason = report_reason(x.report) if x.report else None
    if hop is None:
        _exit_l10(v, x, f)
    else:
        _exit_in79(v, x, f, hop)
        _stderr(v, x, f)
    if x.status != 0 or f.status != 0:
        for c in (CHECKS[1:] if hop is None else IN79_CHECKS[2:]):
            v.add(c, 'n/a the link failed')
        return v
    if hop is not None:
        _dirty(v, x, hop)
    _verify(v, tools, x)
    if hop is not None:
        _elflint(v, tools, x)
    _equiv(v, tools, x, f)
    _run_check(v, tools, x, f)
    _oracle(v, x, base, new)
    if hop is not None:
        _state(v, x, hop)
        _duty(v, x, hop)
    return v
