#!/usr/bin/env python3
"""A deliberately WRONG subject (WP-223): at the hop INCR_WRONG_AT, links as
mortar does and claims an update but writes no ranges file (the `dirty`
check: a missing ranges file after `incremental: update` is a FAIL)."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import claim_update, finish, mortar, wrong_here  # noqa: E402

args = sys.argv[1:]
rc = mortar(args)[0]
if rc == 0 and wrong_here():
    claim_update(args, None, 'no ranges file')
finish(rc)
