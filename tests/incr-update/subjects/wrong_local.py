#!/usr/bin/env python3
"""A deliberately WRONG subject (WP-223): at the hop INCR_WRONG_AT, links as
mortar does, then gives the first local data or function symbol of
`.symtab` that does not start its section the address of its section's
start (it still fits its section, so elflint is content), and claims an
update listing exactly the changed bytes: a wrong `.symtab` local, which
check (b) must refuse; `verify` is the check that does."""
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
    WHY = 'a .symtab local moved'
    c64 = _c64
    st = next(s for s in S if s['name'] == '.symtab')
    ent = 24 if c64 else 16
    syms = oracle.syms_of(new, c64, S)
    done = False
    for j, y in enumerate(syms):
        if y['bind'] != 0 or y['type'] not in (1, 2) or not (0 < y['shn'] < len(S)):
            continue
        sec = S[y['shn']]
        if y['value'] != sec['addr'] and y['size'] <= sec['size']:
            at = st['off'] + j * ent + (8 if c64 else 4)
            write_at(out, at, struct.pack('<Q' if c64 else '<I', sec['addr']))
            done = True
            break
    assert done, 'no local to move'
    claim_update(args, diff_ranges(prev, read(out)), WHY)
finish(rc)
