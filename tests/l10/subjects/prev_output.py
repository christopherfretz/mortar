#!/usr/bin/env python3
"""A deliberately WRONG subject (L10's own test, WP-067): links as mortar
does, except at the step L10_WRONG_AT, where it hands back the PREVIOUS
step's output and map untouched and reports an update."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import finish, mortar, opt, output, wrong_here  # noqa: E402

args = sys.argv[1:]
if wrong_here() and os.path.exists(output(args)):
    rep = opt(args, '--incremental-report')
    if rep:
        with open(rep, 'w') as f:
            f.write('incremental: update (l10 wrong subject: the previous output)\n')
    finish(0)
finish(mortar(args)[0])
