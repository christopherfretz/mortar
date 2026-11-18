"""corpus40: R2's 40-unit corpus (spec/incr.md §1.1, Table 1) as an L10
project (WP-067).

Sources: `corpus_gen.py` (the review's generator, unchanged): common.h,
t00-t37.cc, big.cc (540 functions), main.cc, which prints a checksum of
every unit. Plus `tx.cc` (R2's added unit). The link:

  main.o big.o [tx.o] libt.a (t00-t18, tx; regular) libu.a (t19-t37, thin)

Knobs (each edit of Table 1 is a knob, so an edit and its inverse are a
knob set and reset):

  e1..e10   bool  R2's edits (lib.py's E table): e4, e5, e6 edit common.h
                  and recompile every unit
  same      bool  t07: a same-size constant edit (Table 1's archive row)
  call      bool  t07 calls tx_fn: tx is extracted from libt.a (Table 1's
                  extraction row)
  tx_add    bool  tx.o on the line directly (add and remove a unit)
  ar_order.A 0..2 libt.a / libu.a's members rotated by a third
  gc        bool  --gc-sections on the link line
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import corpus_gen as gen  # noqa: E402

NAME = 'corpus40'

E = {
    'e1': [('big.cc', '7919; /*E1*/', '7927; /*E1*/')],
    'e2': [('big.cc', '  /*E2*/', '  s += x * 3;\n  s ^= (s >> 3);')],
    'e3': [('big.cc', '/*E3*/', '/*E3*/\n// a comment line at the top')],
    'e4': [('common.h', '/*E4*/', '/*E4*/\n// a comment line in the header')],
    'e5': [('common.h', '= 3 /*E5*/', '= 4 /*E5*/')],
    'e6': [('common.h', '/*E6*/', 'int extra = 0; /*E6*/')],
    'e7': [(f, 'shared_fn', 'shared_fn_v2') for f in ('t03.cc', 't10.cc', 't20.cc', 't30.cc')],
    'e8': [('t07.cc', '/*E8*/', '/*E8*/\n// one line above checked_07')],
    'e9': [('big.cc', '/*E9*/', '/*E9*/\nint big_new_fn(int x) {\n  int s = x * 5;\n'
            '  for (int k = 0; k < x; k++) s += k;\n  return s;\n}')],
    'e10': [('big.cc', '"middle-literal-alpha"', '"middle-literal-alpha-beta-gamma-delta"')],
    'same': [('t07.cc', '  return x + 7;\n', '  return x + 9;\n')],
    'call': [('t07.cc', '#include <cstdio>\n', '#include <cstdio>\nint tx_fn(int x);\n'),
             ('t07.cc', '  h = mix(h, bag_07());\n', '  h = mix(h, bag_07());\n  h = mix(h, (std::uint64_t)tx_fn(3));\n')],
}
TX = '''#include <string>
int tx_fn(int x) { return x * 41 + 2; }
static const char *tx_lit = "tx-literal-string-for-the-added-unit";
std::string tx_global = std::string(tx_lit) + std::to_string(tx_fn(1));
int tx_len() { return (int)tx_global.size(); }
'''
LIBT = ['t%02d' % n for n in range(19)] + ['tx']
LIBU = ['t%02d' % n for n in range(19, 38)]
_BASE = None


def knobs():
    k = {e: False for e in E}
    k.update({'tx_add': False, 'ar_order.0': 0, 'ar_order.1': 0, 'gc': False})
    return k


def files(k):
    global _BASE
    if _BASE is None:
        _BASE = gen.files()
    f = dict(_BASE)
    for e, reps in E.items():
        if k[e]:
            for fn, old, new in reps:
                assert old in f[fn], (e, fn, old)
                f[fn] = f[fn].replace(old, new)
    f['tx.cc'] = TX
    return f


def sources(k, cfg):
    f = files(k)
    units = {n[:-3]: (n, 'c++', t) for n, t in f.items() if n.endswith('.cc')}
    return units, {'common.h': f['common.h']}


def unit_flags(name):
    return []


def plan(k, cfg):
    inputs = [('o', 'main'), ('o', 'big')]
    if k['tx_add']:
        inputs.append(('o', 'tx'))
    for a, (name, members, thin) in enumerate((('libt.a', LIBT, False), ('libu.a', LIBU, True))):
        r = k['ar_order.%d' % a] * len(members) // 3
        inputs.append(('a', name, members[r:] + members[:r], thin))
    return inputs, (['--gc-sections'] if k['gc'] else [])


def units_needed(k, cfg):
    return ['main', 'big'] + LIBT + LIBU
