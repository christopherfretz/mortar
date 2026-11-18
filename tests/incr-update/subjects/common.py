"""Shared bits of incr-update's subjects (tests/incr-update/subjects; H-1,
WP-223). A subject takes the linker's words as its arguments, as mortar
does; INCR_MORTAR is the mortar it wraps (the testhooks build), INCR_HOP
the hop, INCR_WRONG_AT the hop a wrong subject is wrong at."""
import os
import subprocess
import sys


def opt(args, prefix):
    """The value of `--name=value` (the last one), or None."""
    v = None
    for a in args:
        if a.startswith(prefix + '='):
            v = a[len(prefix) + 1:]
    return v


def output(args):
    out = 'a.out'
    for i, a in enumerate(args):
        if a == '-o' and i + 1 < len(args):
            out = args[i + 1]
    return out


def mortar(args, capture=False):
    """Runs the real mortar on `args`: its status (and, with `capture`, its
    standard error, else passed through)."""
    argv = [os.environ['INCR_MORTAR']] + args
    if not capture:
        return subprocess.run(argv).returncode, ''
    p = subprocess.run(argv, stderr=subprocess.PIPE, text=True, errors='replace')
    return p.returncode, p.stderr


def wrong_here():
    """Whether this is the hop a wrong subject is wrong at."""
    return os.environ.get('INCR_HOP') is not None and os.environ.get('INCR_HOP') == os.environ.get('INCR_WRONG_AT')


def read(path):
    try:
        with open(path, 'rb') as f:
            return f.read()
    except OSError:
        return None


def diff_ranges(prev, new):
    """The exact runs of bytes in which `new` differs from `prev` (bytes past
    `prev`'s end included): [(offset, len)], sorted and disjoint."""
    out, start = [], None
    n = len(new)
    for p in range(n):
        d = p >= len(prev) or prev[p] != new[p]
        if d and start is None:
            start = p
        elif not d and start is not None:
            out.append((start, p - start))
            start = None
    if start is not None:
        out.append((start, n - start))
    return out


def claim_update(args, ranges, why):
    """Reports an update (the report's one line) and writes `ranges` to
    MORTAR_INCR_DIRTY's path (None: no file)."""
    rep = opt(args, '--incremental-report')
    if rep:
        with open(rep, 'w') as f:
            f.write('incremental: update (incr-update wrong subject: %s)\n' % why)
    path = os.environ.get('MORTAR_INCR_DIRTY')
    if path and ranges is not None:
        with open(path, 'w') as f:
            for o, n in ranges:
                f.write('0x%x 0x%x\n' % (o, n))


def write_at(path, off, data):
    with open(path, 'r+b') as f:
        f.seek(off)
        f.write(data)


def finish(rc):
    sys.stdout.flush()
    sys.stderr.flush()
    sys.exit(rc)
