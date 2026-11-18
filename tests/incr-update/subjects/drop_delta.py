#!/usr/bin/env python3
"""A deliberately WRONG subject (WP-223): links as mortar does and, at the
hop INCR_WRONG_AT, removes from the report's dry run the `delta unit`
lines of units of unchanged files (those without the reason `file`): a
prediction that misses what moved (the `oracle` check, IN-63a)."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import finish, mortar, opt, wrong_here  # noqa: E402

args = sys.argv[1:]
rc = mortar(args)[0]
rep = opt(args, '--incremental-report')
if rc == 0 and wrong_here() and rep and os.path.exists(rep):
    lines = open(rep).read().splitlines()
    keep = [l for l in lines if not (l.startswith('delta unit ') and 'file' not in l.split()[2].split(','))]
    if len(keep) == len(lines):
        sys.exit('drop_delta: no unit of an unchanged file in the report')
    with open(rep, 'w') as f:
        f.write(''.join(l + '\n' for l in keep))
finish(rc)
