#!/usr/bin/env python3
"""A deliberately WRONG subject (WP-223): at the hop INCR_WRONG_AT, links as
mortar does and claims an update whose ranges file lists every run of
bytes that differs from the previous output but the first (the `dirty`
check, IN-75: a ranges file missing a range)."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import claim_update, diff_ranges, finish, mortar, output, read, wrong_here  # noqa: E402

args = sys.argv[1:]
out = output(args)
prev = read(out) if wrong_here() else None
rc = mortar(args)[0]
if rc == 0 and prev is not None:
    ranges = diff_ranges(prev, read(out))
    claim_update(args, ranges[1:], 'the first range left out')
finish(rc)
