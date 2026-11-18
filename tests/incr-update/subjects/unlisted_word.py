#!/usr/bin/env python3
"""A deliberately WRONG subject (WP-223): at the hop INCR_WRONG_AT, links as
mortar does, claims an update with a plausible ranges file (exactly the
bytes that differ from the previous output) and then changes one more word
outside every range: a word of `.comment`, a section no other check reads
(the `dirty` check, IN-75: a write the update did not list)."""
import os
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
    ranges = diff_ranges(prev, new)
    _c64, _m, S, _P = oracle.elf(new)
    com = next(s for s in S if s['name'] == '.comment')
    off = com['off'] + 4
    assert not any(o <= off < o + n for o, n in ranges)
    write_at(out, off, bytes([new[off] ^ 0x20]))
    claim_update(args, ranges, 'one word written outside its ranges')
finish(rc)
