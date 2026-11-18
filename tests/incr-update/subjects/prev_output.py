#!/usr/bin/env python3
"""A deliberately WRONG subject (WP-223): at the hop INCR_WRONG_AT, hands
back the PREVIOUS hop's output and map untouched, claiming an update with
an empty ranges file (which is true: nothing changed). Found by `equiv`:
the old map against the full link of the new inputs."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import claim_update, finish, mortar, output, wrong_here  # noqa: E402

args = sys.argv[1:]
if wrong_here() and os.path.exists(output(args)):
    claim_update(args, [], 'the previous output')
    finish(0)
finish(mortar(args)[0])
