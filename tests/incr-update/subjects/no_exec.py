#!/usr/bin/env python3
"""A deliberately WRONG subject (WP-223): at the hop INCR_WRONG_AT, links as
mortar does and leaves the output without execute permission (an update
that recreates the file and loses its mode): its bytes are right, so only
`run` sees it."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import finish, mortar, output, wrong_here  # noqa: E402

args = sys.argv[1:]
rc = mortar(args)[0]
if rc == 0 and wrong_here():
    os.chmod(output(args), 0o644)
finish(rc)
