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
  elflint  (b) `eu-elflint --gnu-ld` on x's output: every complaint is one
           of harness.md's five allow patterns (ELFLINT_ALLOW). Before
           verify: the outside tool speaks first (every corruption elflint
           was seen to reject, verify rejects too; WP-223's record)
  verify   as L10's
  equiv    (b) as L10's, with the update's own map
  run      (a) as L10's
  oracle   (c) as L10's
  state    (d) until (d) proper: (d0) (WP-267 P-4), `incr_state compare
           S' G`, G the state of the hop's `--incremental=full` link;
           exact on an identity hop whose base was a full link, else equal
           outside IN-78's placement rows (d0_exact, d0_outside_placement;
           what it does not judge: spec/harness.md "incr-update")
  duty     (d) duty_list_covers_differences (IN-78) under (d0): every row
           in which canon(S) and canon(S') differ is a table of a cause of
           IN-78's derived table (tests/incr-update/duty.py)
and the report's first line is the step's REASON (`update`, or the
fall-back's name). The lane adds `expect` (expect_full) after a hop of a
scenario that says `expect-full:`.
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
IN79_CHECKS = ('exit', 'stderr', 'dirty', 'elflint', 'verify', 'equiv', 'run', 'oracle', 'state', 'duty')
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
    canon         the (d) comparator: IncrState ((d0), WP-267 P-4), or
                  CANON_STUB without the tool (not run, `n/a` by name)
    causes        the causes of IN-78's derived table this update has (a
                  set of duty.py ids), or None (any cause)
    """

    def __init__(self, prev=None, ranges=None, state=None, state_before=None, state_prev=None, scratch=None,
                 canon=None, causes=None, base_was_update=False, g_status=None, g_state=None, identity=None,
                 dirs=None):
        self.prev, self.ranges, self.state, self.state_before = prev, ranges, state, state_before
        self.state_prev, self.scratch, self.canon, self.causes = state_prev, scratch, canon or CANON_STUB, causes
        # WP-225 ruling M2a2-1: the previous successful hop's link was an
        # update (it writes format 5 only, no format-4 side state, so no dry
        # run can predict this hop until M6): (c) is `n/a base-was-update`.
        self.base_was_update = base_was_update
        # WP-225 M2a3-3 (the (c) flag): the exit status of the hop's G link
        # (`--incremental=full` of the new inputs, the oracle's `new`), or
        # None when not run. A failed G link does not end the run; (c) names it.
        self.g_status = g_status
        # WP-267 P-4 ((d0)): G's state (the hop's `--incremental=full` link
        # of the same inputs), and whether the hop is an identity hop
        # (True, False, or None: not known, judged as not one)
        self.g_state, self.identity = g_state, identity
        # (the subject's directory, G's): the one string in which the two
        # states may differ by construction (d0_normalise)
        self.dirs = dirs


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


def report_detail(path):
    """The fall-back's detail word (`layout` of `incremental: full
    (update-not-implemented (layout))`; spec/incr.md §7), or None."""
    t = report_line(path)
    if t is None or t[1] != 'full' or len(t) < 4 or not t[2].startswith('(') or t[2].endswith(')'):
        return None
    return t[3].strip('():') if t[3].startswith('(') else None


def expect_full(v, want, detail):
    """A scenario's `expect-full:` (tests/incr-update): a hop that may not
    be an update by rule (an update under `--gc-sections` whose liveness
    flips is `update-not-implemented (layout)`, WP-225 gc-lift, until
    M2b-3) is that named full link or another fall-back that comes first
    (`no-state`, `small-output`, ...); `update` fails, and so does `want`
    with another detail word. `want`'s detail may name several words
    (`layout|symtab`: refusals of one reason that come before the rule's,
    e.g. the refill's, WP-225 M2b-2d), any of which passes."""
    reason, wr, wd = v.reason, want[0], want[1]
    if reason == 'update':
        v.add('expect', 'FAIL an update where the rule says `full (%s (%s))`' % (wr, wd))
    elif reason == wr and detail not in wd.split('|'):
        v.add('expect', 'FAIL `full (%s (%s))` where the rule says `(%s)`' % (wr, detail, wd))
    elif reason == wr:
        v.add('expect', 'pass (%s (%s))' % (wr, wd))
    else:
        v.add('expect', 'n/a (full link: %s, before the rule)' % reason)


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
# alike (review-in72 T-1), and a fifth for an output that keeps a
# SystemTap probe note (the AArch64 host's static libstdc++.a carries them,
# on its exception paths; WP-223's record). Each is held to what the text says it is: an IRELATIVE record of
# the PLT's relocation section, the symbol `__ehdr_start`, the variant PCS
# bit of an AArch64 output, and a note of owner `stapsdt`, type 3, in a
# section named `.note.stapsdt`.
ELFLINT_ALLOW = (
    ('irelative-type', re.compile(r"^section \[ *\d+\] '(\.rela?\.plt)': relocation (\d+): invalid type$")),
    ('irelative-symbol', re.compile(r"^section \[ *\d+\] '(\.rela?\.plt)': relocation (\d+): invalid symbol index$")),
    ('ehdr-start', re.compile(r"^section \[ *\d+\] '\.symtab': symbol \d+ \(__ehdr_start\): st_value out of bounds$")),
    ('variant-pcs', re.compile(r"^section \[ *\d+\] '\.symtab': symbol \d+ \([^)]*\): unknown bit set in st_other$")),
    ('stapsdt', re.compile(r"^section \[ *(\d+)\] '\.note\.stapsdt': unknown object file note type 3 with owner name "
                           r"'stapsdt' at offset (\d+)$")),
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


def stapsdt_note_ends_at(data, index, end):
    """True when section `index` of the output is named `.note.stapsdt`
    and holds a note of owner `stapsdt` and type 3 that ends at byte `end`
    of the section (elflint names the offset just past the note it read)."""
    _c64, _mach, S, _P = oracle.elf(data)
    if index >= len(S) or S[index]['name'] != '.note.stapsdt':
        return False
    s = S[index]
    sec = data[s['off']:s['off'] + s['size']]
    o = 0
    while o + 12 <= len(sec):
        namesz, descsz, ntype = (int.from_bytes(sec[o + 4 * k:o + 4 * k + 4], 'little') for k in range(3))
        name = sec[o + 12:o + 12 + namesz]
        nxt = o + 12 + ((namesz + 3) & ~3) + ((descsz + 3) & ~3)
        if nxt > len(sec):
            return False
        if nxt == end:
            return ntype == 3 and name == b'stapsdt\0'
        o = nxt
    return False


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
            elif name == 'stapsdt':
                ok = stapsdt_note_ends_at(data, int(m.group(1)), int(m.group(2)))
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


def _oracle(v, x, base, new, hop=None):
    if hop is not None and hop.g_status not in (None, 0):
        # M2a3-3: the hop's own G link failed: (c) has no `new`, said by name
        v.add('oracle', 'n/a the G link failed (status %s)' % hop.g_status)
        return
    if base is None or new is None or not x.report or not os.path.exists(x.report):
        v.add('oracle', 'n/a no previous full link')
        return
    if hop is not None and hop.base_was_update:
        # ruling M2a2-1 (WP-225): the ONLY other way (c) is n/a by itself
        v.add('oracle', 'n/a base-was-update')
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
    """Stands for the comparator when the lane has no `incr_state` (and in
    the self-test): `n/a` by name, never a pass (WP-267 P-4). `compare(a, b, p_u)` returns
    NotRun, or the differences of canon(a) against canon(b) as a list of
    (table, row, what): the table's name (spec/incr.md's table names, as
    duty.py has them), the row's canonical key, what differs; [] = equal."""

    live = False

    def compare(self, a, b, p_u):
        return NotRun('(d) not run (no incr_state: (d0)\'s comparator absent)')


CANON_STUB = CanonStub()
STATE_TIMEOUT = 120


class CompareFailed:
    """The comparator ran and could not give an answer (a timeout, an
    exit status other than 0 and 1, output it does not parse): a FAIL of
    the check, never a pass."""

    def __init__(self, why):
        self.why = why


def parse_compare(text):
    """`incr_state compare a b`'s standard output as [(table, row, what)],
    or None when it does not parse. Its lines (examples/incr_state.rs's
    `compare` arm, state5::compare): `<TABLE> <row>: <what>` a difference,
    the row `row <i>` (equal row counts: `<what>` is `<hex> != <hex>`),
    the row's hex (unequal counts, a set difference: `only in the first`
    or `only in the second`), or empty (`<n> rows against <m>
    (duplicates)`); then `<n> differences`."""
    lines = text.splitlines()
    if not lines or not lines[-1].endswith(' differences'):
        return None
    try:
        n = int(lines[-1].split()[0])
    except ValueError:
        return None
    out = []
    for line in lines[:-1]:
        table, sp, rest = line.partition(' ')
        row, sep, what = rest.partition(': ')
        if not sp or not sep or not table:
            return None
        out.append((table, row, what))
    return out if len(out) == n else None


class IncrState:
    """(d0)'s live comparator (WP-267 P-4): `incr_state compare a b`
    (crates/mortar_incr/examples/incr_state.rs; the canonical forms of
    state5::canon and their differences, state5::compare). `p_u` is not
    read (the tool's `compare` takes none; `placement_of_map` gives none).
    The output is saved to `save` when it names a difference."""

    live = True

    def __init__(self, tool):
        self.tool = tool

    def compare(self, a, b, p_u, save=None):
        rc, out = _run([self.tool, 'compare', a, b], os.path.dirname(os.path.abspath(a)), STATE_TIMEOUT)
        if rc is None:
            return CompareFailed('TIMEOUT incr_state compare %s' % out)
        if rc not in (0, 1):
            return CompareFailed('incr_state compare exit %s: %s' % (rc, _tail(out)))
        diffs = parse_compare(out)
        if diffs is None or (rc == 0) != (not diffs):
            return CompareFailed('incr_state compare exit %s, output not understood: %s' % (rc, _tail(out)))
        if diffs and save:
            with open(save, 'w') as fh:
                fh.write(out)
        return diffs


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


# IN-78's placement rows (its decision rows and its derived layout rows):
# on a hop (d0) does not judge exactly, a difference in them is excluded,
# since the update's placement may differ from G's fresh one (WP-267
# review §6). Whole tables:
D0_PLACEMENT_TABLES = ('CHUNKS', 'ORDER', 'HOLES', 'PATCHES', 'REDIRECTS', 'REGIONS', 'SYMTAB',
                       'DEFINED', 'SYM_PLACE', 'SIZES', 'SIZED', 'SEGMENTS', 'LAYOUT', 'OUTSECS')


# the map's lines that are not the placement: the command line, and the
# update's own `.symtab` block records (a full link's map has none)
MAP_NOT_PLACEMENT = ('arg', 'symblock')


def identity_hop(x, g_map):
    """Whether the hop is an identity hop for (d0): the subject's map and
    G's (the same inputs) give the same records, line for line, outside
    MAP_NOT_PLACEMENT: every `place`, `piece`, `chunk`, `capacity`,
    `order`, `region`, `hole`, `frag`, `slot`, `define` (image.md I-78,
    I-78a). Then the update's placement IS the full link's and nothing
    explains a difference of the states. None when a map is missing."""
    try:
        with open(x.path(x.map)) as fx, open(g_map) as fg:
            keep = lambda fh: [l for l in fh if l.split(' ', 1)[0].strip() not in MAP_NOT_PLACEMENT]
            return keep(fx) == keep(fg)
    except OSError:
        return None


# (d0) never judges these, on any hop (WP-267 P-4): `STAT` (G's inputs
# are copies in its own directory: other inodes and times; the rows' paths
# and hashes are FILES' too) and, in `STRINGS`, the link's directory (G
# links in h<i>, the subject in its directory: the one string that differs
# by construction; d0_normalise checks that it is the only one)
D0_NEVER = ('STAT',)


def d0_normalise(diffs, dirs):
    """(diffs, notes): the differences (d0) judges, and what it set aside
    by construction (D0_NEVER's tables; a STRINGS row whose only
    difference is the subject's directory against G's)."""
    out, notes = [], {}
    a, b = (os.path.abspath(d).encode() for d in dirs) if dirs else (None, None)
    for d in diffs:
        table, row, what = d
        if table in D0_NEVER:
            notes[table] = notes.get(table, 0) + 1
            continue
        if table == 'STRINGS' and a and len(a) == len(b) and ' != ' in what:
            try:
                x, y = (bytes.fromhex(h) for h in what.split(' != '))
            except ValueError:
                x = y = None
            if x is not None and x != y and x == y.replace(b, a):
                notes['STRINGS (the directory)'] = notes.get('STRINGS (the directory)', 0) + 1
                continue
        out.append(d)
    return out, notes


D0_KINDS = ('exact', 'outside-placement', 'n/a', 'FAIL')


def d0_kind(result):
    """The `state` check's verdict by the rule that gave it (the lane's
    summary)."""
    if result.startswith('pass (d0 exact'):
        return 'exact'
    if result.startswith('pass (d0 outside placement'):
        return 'outside-placement'
    return 'FAIL' if result.startswith('FAIL') else 'n/a'


def d0_exact(hop):
    """(exact, why): the rule (d0) judges this hop by."""
    if hop.base_was_update:
        return False, 'the base was an update'
    if hop.identity is None:
        return False, 'identity not known'
    if not hop.identity:
        return False, 'not an identity hop'
    return True, 'an identity hop on a full link'


# IN-78's placement COLUMNS of the other tables, as byte ranges of the
# canonical row (state5::canon encodes a row as the state does; the
# layouts are spec/incr.md §5.3's and the format-5 columns of IN-18e,
# each range checked against the lane's own differences, WP-267 P-4's
# record): a row whose bytes differ only inside them is excluded.
D0_PLACEMENT_COLUMNS = {
    'SECTIONS': ((64, 76),),          # place chunk `u32`, place offset `u64` (76-byte row)
    'FILES': ((72, 80),),             # eh_offset, eh_capacity: the `.eh_frame` range (97-byte row)
    'FRAGS': ((24, 32),),             # offset `u64` (after out, ordinal, content hash)
    'META': ((16, 24),),              # the output's size (the output's layout)
}
D0_ROW_LEN = {'SECTIONS': 76, 'FILES': 97, 'META': 196}
# tables whose rows' ORDER is placement (SLOTS: the slot lists' positions):
# equal row counts whose differing rows are a permutation are excluded
D0_PLACEMENT_ORDER = ('SLOTS',)


def _masked_equal(table, what):
    try:
        x, y = (bytes.fromhex(h) for h in what.split(' != '))
    except ValueError:
        return False
    if len(x) != len(y) or (table in D0_ROW_LEN and len(x) != D0_ROW_LEN[table]):
        return False
    m = bytearray(len(x))
    for lo, hi in D0_PLACEMENT_COLUMNS[table]:
        m[lo:hi] = b'\1' * len(m[lo:hi])
    return all(p == q or k for p, q, k in zip(x, y, m))


def d0_outside_placement(diffs):
    """(kept, excluded): the differences outside IN-78's placement rows,
    and those in them."""
    kept, excluded = [], []
    perm = {}
    for d in diffs:
        table, row, what = d
        if table in D0_PLACEMENT_TABLES:
            excluded.append(d)
        elif table in D0_PLACEMENT_COLUMNS and row.startswith('row ') and _masked_equal(table, what):
            excluded.append(d)
        elif table in D0_PLACEMENT_ORDER and row.startswith('row ') and ' != ' in what:
            perm.setdefault(table, []).append(d)
        else:
            kept.append(d)
    for table, ds in perm.items():
        a = sorted(w.split(' != ')[0] for _t, _r, w in ds)
        b = sorted(w.split(' != ')[1] for _t, _r, w in ds)
        (excluded if a == b else kept).extend(ds)
    return kept, excluded


def _state(v, x, hop):
    """(d0) (WP-267 P-4) until (d) proper: canon(S') against canon(G), G
    the state of the hop's own `--incremental=full` link of the same
    inputs; EXACT on an identity hop whose base was a full link, else
    equal outside IN-78's placement rows (d0_rule)."""
    p_u, why = placement_of_map(x.path(x.map))
    if not hop.canon.live:
        res = hop.canon.compare(None, None, None)
        v.add('state', 'n/a %s; %s' % (res.why, why))
        return
    if v.reason != 'update':
        v.add('state', 'n/a (full link%s)' % (': ' + v.reason if v.reason else ''))
        return
    if not hop.g_state or not os.path.exists(hop.g_state):
        v.add('state', 'n/a (d0) has no G state (the G link %s)' % (
            'failed (status %s)' % hop.g_status if hop.g_status not in (None, 0) else 'wrote none'))
        return
    save = os.path.join(hop.scratch or x.dir, 'd0-compare.txt')
    res = hop.canon.compare(hop.state, hop.g_state, p_u, save=save)
    if isinstance(res, NotRun):
        v.add('state', 'n/a %s; %s' % (res.why, why))
        return
    if isinstance(res, CompareFailed):
        v.add('state', 'FAIL (d0) %s' % res.why)
        return
    res, _never = d0_normalise(res, hop.dirs)
    exact, rule_why = d0_exact(hop)
    if exact:
        if res:
            v.add('state', 'FAIL (d0 exact: %s) canon(S\') differs from canon(G): %s' % (rule_why,
                                                                                    first_difference(res)))
        else:
            v.add('state', 'pass (d0 exact)')
        return
    kept, excluded = d0_outside_placement(res)
    if kept:
        v.add('state', 'FAIL (d0 outside placement: %s) canon(S\') differs from canon(G) outside the placement '
                       'rows: %s' % (rule_why, first_difference(kept)))
        return
    v.add('state', 'pass (d0 outside placement: %d rows excluded)' % len(excluded))


# duty under (d0) holds out STRINGS: canon's pool holds the strings the
# rows name (IN-78's canon names a string by its bytes, in its row)
DUTY_NOT_A_ROW = ('STRINGS',)


def _duty(v, x, hop):
    """duty_list_covers_differences (IN-78) under (d0): canon(S), the state
    before the update, against canon(S'), the state it wrote (C(U) is not
    captured: (d) proper; where (d0) holds S' stands for it). Every
    differing row's table must be a table of a cause (of `hop.causes`; the
    lane gives None: any cause)."""
    import duty  # tests/incr-update/duty.py: IN-78's derived table as data
    if not hop.canon.live:
        v.add('duty', 'n/a %s' % hop.canon.compare(None, None, None).why)
        return
    if v.reason != 'update':
        v.add('duty', 'n/a (full link%s)' % (': ' + v.reason if v.reason else ''))
        return
    if not hop.state_prev or not os.path.exists(hop.state_prev):
        v.add('duty', 'n/a (d0) has no state before the update')
        return
    res = hop.canon.compare(hop.state_prev, hop.state, None,
                            save=os.path.join(hop.scratch or x.dir, 'd0-duty-compare.txt'))
    if isinstance(res, NotRun):
        v.add('duty', 'n/a %s' % res.why)
        return
    if isinstance(res, CompareFailed):
        v.add('duty', 'FAIL (d0) %s' % res.why)
        return
    held = [d for d in res if d[0] in DUTY_NOT_A_ROW]
    why = duty.uncovered([d for d in res if d[0] not in DUTY_NOT_A_ROW], hop.causes)
    v.add('duty', 'pass (S against S\': %d rows differ; not judged: %s)' % (len(res) - len(held), ', '.join(
        DUTY_NOT_A_ROW)) if why is None else 'FAIL ' + why)


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
        _elflint(v, tools, x)
    _verify(v, tools, x)
    _equiv(v, tools, x, f)
    _run_check(v, tools, x, f)
    _oracle(v, x, base, new, hop)
    if hop is not None:
        _state(v, x, hop)
        _duty(v, x, hop)
    return v
