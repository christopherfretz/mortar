#!/usr/bin/env python3
"""A deliberately WRONG subject (L10's own test, WP-067): links as mortar
does but drops the first diagnostic that names an error or a warning."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import finish, mortar  # noqa: E402

rc, text = mortar(sys.argv[1:], capture=True)
dropped = False
for line in text.splitlines():
    if not dropped and ('error' in line or 'warning' in line):
        dropped = True
        continue
    print(line)
finish(rc)
