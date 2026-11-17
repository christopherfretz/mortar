#!/usr/bin/env python3
"""A deliberately WRONG subject (L10's own test, WP-067): links as mortar
does, and at the step L10_WRONG_AT flips the low bit of one relocated
field of main.o (an input the step did not change): the first relocation
against a global symbol in main.o's text, found through the map's `file`
and `place` lines and main.o's relocations."""
import os
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, '..', '..', 'incr-oracle'))
sys.dont_write_bytecode = True
from common import finish, mortar, opt, output, wrong_here  # noqa: E402
import oracle  # noqa: E402


def flip(out, map_):
    fid, places = None, {}
    for line in open(map_):
        t = line.split()
        if t[:1] == ['file'] and len(t) == 3 and t[2] == 'main.o':
            fid = t[1]
        elif t[:1] == ['place'] and fid is not None and t[1] == fid:
            places[int(t[2])] = int(t[3], 16)
    if fid is None:
        sys.exit('flip_word: main.o is not in the map')
    obj = oracle.Obj(open('main.o', 'rb').read())
    for sec in sorted(places):
        if not obj.S[sec]['name'].startswith('.text'):
            continue
        for roff, sym, _ty, _add in sorted(obj.rela.get(sec, [])):
            if sym < len(obj.syms) and obj.syms[sym]['bind'] != 0:
                addr = places[sec] + roff
                b = bytearray(open(out, 'rb').read())
                _c64, _m, _S, P = oracle.elf(bytes(b))
                seg = next(p for p in P if p['type'] == 1 and p['vaddr'] <= addr < p['vaddr'] + p['filesz'])
                off = seg['off'] + addr - seg['vaddr']
                b[off] ^= 1
                with open(out, 'r+b') as f:
                    f.seek(off)
                    f.write(bytes(b[off:off + 1]))
                return
    sys.exit('flip_word: no relocated field in main.o\'s text')


args = sys.argv[1:]
rc = mortar(args)[0]
if rc == 0 and wrong_here():
    flip(output(args), opt(args, '--mortar-map'))
finish(rc)
