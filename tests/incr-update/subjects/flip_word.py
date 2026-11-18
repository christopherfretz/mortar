#!/usr/bin/env python3
"""A deliberately WRONG subject (WP-223): at the hop INCR_WRONG_AT, links as
mortar does, then flips one bit of the first word of `.text` and claims an
update whose ranges list exactly the bytes that differ (so `dirty` passes;
elflint does not read code): `verify` must refuse it (IN-1 (A))."""
import os
import struct  # noqa: F401
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, '..', '..', 'incr-oracle'))
sys.dont_write_bytecode = True
from common import claim_update, diff_ranges, finish, mortar, output, read, wrong_here, write_at  # noqa: E402
import oracle  # noqa: E402

args = sys.argv[1:]
out = output(args)
prev = read(out) if wrong_here() else None
rc = mortar(args)[0]
if rc == 0 and prev is not None:
    new = read(out)
    _c64, _m, S, _P = oracle.elf(new)
    WHY = 'a bit of .text flipped'
    t = next(s for s in S if s['name'] == '.text')
    write_at(out, t['off'], bytes([new[t['off']] ^ 0x08]))
    claim_update(args, diff_ranges(prev, read(out)), WHY)
finish(rc)
