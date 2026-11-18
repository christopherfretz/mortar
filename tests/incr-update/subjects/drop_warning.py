#!/usr/bin/env python3
"""A deliberately WRONG subject (WP-223): links as mortar does and, at the
hop INCR_WRONG_AT, drops the first warning from standard error with its
`>>>` lines (the `stderr` check, IN-75a; the exit status is mortar's)."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import finish, mortar, wrong_here  # noqa: E402

rc, err = mortar(sys.argv[1:], capture=True)
lines, out, dropping, dropped = err.splitlines(), [], False, False
for l in lines:
    if wrong_here() and not dropped and 'warning:' in l:
        dropping = dropped = True
        continue
    if dropping and l.startswith('>>>'):
        continue
    dropping = False
    out.append(l)
sys.stderr.write(''.join(l + '\n' for l in out))
finish(rc)
