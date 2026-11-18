#!/usr/bin/env python3
"""WP-239 (S4-4 and the size table): a format-4 state's tables (rows,
bytes), `SLOTS` by kind, `INDEX` by kind (SLOT, LOCAL_SLOT), and the
counts S4-4's refs need: per SLOTS row, how many files' INDEX rows name
its key through a slot (kind SLOT).

    statecount.py STATE"""
import struct
import sys
from collections import Counter, defaultdict

sys.path.insert(0, __import__('os').path.dirname(__file__))
import elfobj  # noqa: E402

NAMES = {1: 'META', 2: 'STAT', 3: 'FILES', 4: 'PROBES', 5: 'OUTSECS', 6: 'SYMBOLS', 7: 'SECTIONS',
         8: 'CHUNKS', 9: 'ORDER', 10: 'SEGMENTS', 11: 'FRAGS', 12: 'SLOTS', 13: 'EH_RECORDS',
         14: 'SYMTAB_LOCALS', 15: 'SIZES', 16: 'SYM_PLACE', 17: 'DEFINED', 18: 'PATCHES',
         19: 'REDIRECTS', 20: 'SIZED', 21: 'LAYOUT', 22: 'LEAVES', 23: 'STRINGS', 32: 'MENTIONS',
         33: 'RESOLVED', 34: 'INDEX', 39: 'HOLES', 40: 'FRAG_AT'}
SLOT_KINDS = ['got', 'plt', 'iplt', 'tlsdesc', 'tlsgd', 'tlsie']
IDX = {0: 'GLOBAL', 1: 'SECTION', 2: 'SLOT', 3: 'EH_FRAME', 4: 'LINK_ORDER', 5: 'LOCAL_SLOT'}


def main():
    d = elfobj.state_bytes(sys.argv[1])
    t = elfobj.state_tables(sys.argv[1])
    total = len(d)
    print(f'state bytes {total}')
    for k in sorted(t):
        rows, nb, _ = t[k]
        print(f'table {NAMES.get(k, k)} rows {rows} bytes {nb}')
    rows, nb, at = t[12]
    w = nb // rows if rows else 5
    slots = []
    for i in range(rows):
        kind, sym = struct.unpack_from('<B3xI', d, at + i * w)
        slots.append((kind, sym))
    c = Counter(SLOT_KINDS[k] for k, _ in slots)
    print('slots ' + ' '.join(f'{k}={c[k]}' for k in SLOT_KINDS) + f' total={len(slots)}')
    # SECTIONS: file at offset 0 of each row.
    srows, snb, sat = t[7]
    sw = snb // srows
    sec_file = [struct.unpack_from('<I', d, sat + i * sw)[0] for i in range(srows)]
    rows, nb, at = t[34]
    w = nb // rows if rows else 9
    kinds = Counter()
    users = defaultdict(set)  # global key -> files using it through a slot
    unit_users = defaultdict(set)
    for i in range(rows):
        unit, kind, target = struct.unpack_from('<IB3xI', d, at + i * w)
        kinds[kind] += 1
        if kind == 2:
            users[target].add(sec_file[unit])
            unit_users[target].add(unit)
    print('index ' + ' '.join(f'{IDX[k]}={kinds[k]}' for k in sorted(kinds)))
    global_slots = {s for k, s in slots if s != 0xffffffff}
    nf = [len(users[s]) for s in global_slots]
    nu = [len(unit_users[s]) for s in global_slots]
    nouse = Counter(SLOT_KINDS[k] for k, s in slots if not users[s])
    print('slots_without_SLOT_index_user ' + ' '.join(f'{k}={v}' for k, v in sorted(nouse.items())))
    print(f'slot keys {len(global_slots)} with_no_SLOT_index_user {sum(1 for x in nf if x == 0)} '
          f'file_users_sum {sum(nf)} unit_users_sum {sum(nu)} max_file_users {max(nf) if nf else 0}')
    return 0


sys.exit(main())
