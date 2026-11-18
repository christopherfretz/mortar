#!/usr/bin/env python3
"""A deliberately WRONG subject (incr-update's self test, WP-223): links as
mortar does and, at the hop INCR_WRONG_AT, exits 1 after a link that
succeeded (the `exit` check)."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import finish, mortar, wrong_here  # noqa: E402

rc = mortar(sys.argv[1:])[0]
finish(1 if rc == 0 and wrong_here() else rc)
