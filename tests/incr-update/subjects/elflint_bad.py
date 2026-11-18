#!/usr/bin/env python3
"""A deliberately WRONG subject (WP-223): at the hop INCR_WRONG_AT, links as
mortar does, then sets e_ident[EI_PAD] (byte 9) to 1, an output eu-elflint
rejects ("e_ident[9] is not zero"), and claims an update listing exactly
that byte: `elflint` must refuse it. (verify refuses it too; elflint runs
first, see tests/l10/judge.py, and no corruption was found that only
elflint refuses: WP-223's record.)"""
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
    WHY = 'e_ident[9] set'
    write_at(out, 9, b'\x01')
    claim_update(args, diff_ranges(prev, read(out)), WHY)
finish(rc)
