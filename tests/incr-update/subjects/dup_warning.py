#!/usr/bin/env python3
"""A deliberately WRONG subject (WP-223): links as mortar does and, at the
hop INCR_WRONG_AT, prints the first warning (with its `>>>` lines) twice
(the `stderr` check, IN-75a: an update that repeats a stored warning the
front also raised)."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import finish, mortar, wrong_here  # noqa: E402

rc, err = mortar(sys.argv[1:], capture=True)
lines = err.splitlines()
i = next((i for i, l in enumerate(lines) if 'warning:' in l), None)
if wrong_here() and i is not None:
    j = i + 1
    while j < len(lines) and lines[j].startswith('>>>'):
        j += 1
    lines = lines[:j] + lines[i:j] + lines[j:]
sys.stderr.write(''.join(l + '\n' for l in lines))
finish(rc)
