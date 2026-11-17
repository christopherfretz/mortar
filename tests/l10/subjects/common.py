"""Shared bits of L10's subjects (tests/l10/subjects): each subject takes
the linker's words as its arguments, as `mortar` does."""
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
    """Runs the real mortar (L10_MORTAR) on `args`."""
    argv = [os.environ['L10_MORTAR']] + args
    if not capture:
        return subprocess.run(argv).returncode, ''
    p = subprocess.run(argv, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, errors='replace')
    return p.returncode, p.stdout


def wrong_here():
    """Whether this is the step a wrong subject is wrong at."""
    return os.environ.get('L10_STEP') is not None and os.environ.get('L10_STEP') == os.environ.get('L10_WRONG_AT')


def finish(rc):
    sys.stdout.flush()
    sys.exit(rc)
