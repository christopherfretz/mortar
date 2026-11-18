"""IN-78's derived table as data (spec/incr.md IN-78, "The list, derived";
H-1, WP-223): cause -> the state's tables whose rows that cause changes.
`duty_list_covers_differences` (tests/l10/judge.py, check `duty`) holds the
comparison of canon(S) and canon(C(U)) to it: every differing row's table
must be a table of a cause. `check_spec()` is the test that this table and
the spec's agree (the lane's --self-test runs it).

Each row: (id, the spec's cause cell verbatim, the names the spec's cell
puts in backticks, the tables the row changes). The last column is the
second resolved into tables: a FILES flag (`POSITIONAL`, `LAZY`) is a
change of FILES, and a cell's words that name a class of tables instead of
a table ("its ranges gone", "its key-keyed rows deleted") are spelled out
from IN-18e's class table (WORDS below says which).
"""

import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))
SPEC = os.path.join(HERE, '..', '..', 'spec', 'incr.md')

# IN-18e's tables by kind (the class table of spec/incr.md §5.3).
KINDS = {
    'META': 1, 'STAT': 2, 'FILES': 3, 'PROBES': 4, 'OUTSECS': 5, 'SYMBOLS': 6, 'SECTIONS': 7, 'CHUNKS': 8,
    'ORDER': 9, 'SEGMENTS': 10, 'FRAGS': 11, 'SLOTS': 12, 'EH_RECORDS': 13, 'SIZES': 15, 'SYM_PLACE': 16,
    'DEFINED': 17, 'PATCHES': 18, 'REDIRECTS': 19, 'SIZED': 20, 'LAYOUT': 21, 'LEAVES': 22, 'STRINGS': 23,
    'MENTIONS': 32, 'RESOLVED': 33, 'INDEX': 34, 'HOLES': 39, 'FRAG_AT': 40, 'FILE_INDEX': 41, 'EDGES': 42,
    'SYMTAB': 43, 'GROUPS': 44, 'KEY_REFS': 45, 'COMMONS': 46, 'ODR_CLASSES': 47, 'ODR_REFS': 48, 'DIAGS': 49,
    'SYMTAB_KEYS': 50, 'REGIONS': 51, 'NAMES': 52,
}
NAMES = {k: n for n, k in KINDS.items()}
FLAGS = {'POSITIONAL': 'FILES', 'LAZY': 'FILES'}      # FILES flags the cells name
FILE_KEYED = ('SECTIONS', 'MENTIONS', 'INDEX', 'EDGES', 'FRAG_AT', 'EH_RECORDS', 'SYMTAB', 'REGIONS')
# the key-keyed tables with a file in their key or row (a gone file's rows)
FILE_KEY_ROWS = ('KEY_REFS', 'COMMONS', 'ODR_REFS', 'DIAGS')

WORDS = {
    'gone': 'its row marked gone (FILES), its ranges gone (every file-keyed table), its key-keyed rows '
            'deleted (the key-keyed tables whose rows name a file)',
    'device': 'the device rows of STAT',
}

DUTY = (
    ('changed', 'a changed, appeared, drawn-in or newly extracted file',
     ('FILES', 'POSITIONAL', 'STAT', 'SECTIONS', 'MENTIONS', 'INDEX', 'EDGES', 'FRAG_AT', 'EH_RECORDS', 'SYMTAB',
      'KEY_REFS', 'COMMONS', 'ODR_REFS', 'DIAGS'),
     ('FILES', 'STAT', 'SECTIONS', 'MENTIONS', 'INDEX', 'EDGES', 'FRAG_AT', 'EH_RECORDS', 'SYMTAB', 'KEY_REFS',
      'COMMONS', 'ODR_REFS', 'DIAGS')),
    ('gone', 'a gone or no longer extracted file',
     ('LAZY',),
     ('FILES',) + FILE_KEYED + FILE_KEY_ROWS),
    ('touched', 'an input `stat`ed again but hashed equal (touched)',
     ('STAT',),
     ('STAT',)),
    ('device', 'the device stamps (IN-22a)',
     ('STAT',),
     ('STAT',)),
    ('key', 'an affected key',
     ('SYMBOLS', 'RESOLVED', 'KEY_REFS', 'NAMES'),
     ('SYMBOLS', 'RESOLVED', 'KEY_REFS', 'NAMES')),
    ('group', 'a re-elected group',
     ('GROUPS',),
     ('GROUPS',)),
    ('moved', 'a section moved, killed or revived (IN-72 items 1-5, 11), in any file',
     ('SECTIONS', 'REGIONS'),
     ('SECTIONS', 'REGIONS')),
    ('defined', "a linker-defined key's value or a placed symbol's place",
     ('DEFINED', 'SYM_PLACE'),
     ('DEFINED', 'SYM_PLACE')),
    ('growth', "a growth step, a packed rewrite, a TLS template's size, `-e` in a changed file",
     ('CHUNKS', 'SIZES', 'SIZED', 'SEGMENTS', 'ORDER', 'LAYOUT', 'OUTSECS', 'REGIONS'),
     ('CHUNKS', 'SIZES', 'SIZED', 'SEGMENTS', 'ORDER', 'LAYOUT', 'OUTSECS', 'REGIONS')),
    ('slack', 'holes, slots, thunks, fragments',
     ('HOLES', 'SLOTS', 'PATCHES', 'REDIRECTS', 'FRAGS'),
     ('HOLES', 'SLOTS', 'PATCHES', 'REDIRECTS', 'FRAGS')),
    ('symtab', "a `.symtab` entry moved or a `.strtab` name moved (IN-49a's last global taking a removed one's "
               "place, names appended)",
     ('SYMTAB_KEYS', 'SYMTAB'),
     ('SYMTAB_KEYS', 'SYMTAB')),
    ('output', 'the output',
     ('LEAVES', 'META'),
     ('LEAVES', 'META')),
)


def table_name(t):
    """A table given by kind (CanonDiff.kind, an int) or by name."""
    return NAMES.get(t, str(t)) if isinstance(t, int) else t


def uncovered(diffs, causes=None):
    """duty_list_covers_differences over the comparison `diffs` [(table,
    row, what)]: None when every differing row's table is a table of a
    cause (of `causes`, the update's, when given); else why not, naming the
    first row that is not."""
    for table, row, what in diffs:
        t = table_name(table)
        rows = [d for d in DUTY if t in d[3] and (causes is None or d[0] in causes)]
        if not rows:
            known = [d[0] for d in DUTY if t in d[3]]
            if known:
                return '%s row %s differs (%s): its causes (%s) are not this update\'s (%s)' % (
                    t, row, what, ','.join(known), ','.join(sorted(causes)))
            return '%s row %s differs (%s) and no cause of IN-78\'s derived table names %s' % (t, row, what, t)
    return None


def spec_rows(path=SPEC):
    """The derived table of IN-78 as the spec has it: [(cause, [names])]."""
    rows, on = [], False
    for line in open(path):
        s = line.strip()
        if s.startswith('| cause | rows that change |'):
            on = True
            continue
        if not on:
            continue
        if not s.startswith('|'):
            break
        cells = [c.strip() for c in s.strip('|').split('|')]
        if set(cells[0]) <= {'-'}:
            continue
        rows.append((cells[0], sorted(set(re.findall(r'`([A-Z][A-Z_]+)`', cells[1])))))
    return rows


def spec_kinds(path=SPEC):
    """IN-18e's class table: {table: kind}."""
    out = {}
    on = False
    for line in open(path):
        s = line.strip()
        if s.startswith('| class | tables (kind) |'):
            on = True
            continue
        if on and not s.startswith('|'):
            break
        if on:
            for name, kind in re.findall(r'`([A-Z][A-Z_]+)` \((\d+)', s):
                out[name] = int(kind)
    return out


def check_spec(path=SPEC):
    """The test that this table and the spec's agree: [problems] (empty: they
    agree). Also lists, as notes, the tables of IN-18e no cause names."""
    problems = []
    spec = spec_rows(path)
    if [c for c, _n in spec] != [d[1] for d in DUTY]:
        problems.append('the causes differ: spec %r, duty.py %r' % ([c for c, _n in spec], [d[1] for d in DUTY]))
    for (cause, names), d in zip(spec, DUTY):
        if sorted(names) != sorted(d[2]):
            problems.append('%s: the spec names %s, duty.py %s' % (d[0], names, sorted(d[2])))
        for n in names:
            t = FLAGS.get(n, n)
            if t not in d[3]:
                problems.append('%s: %s named by the spec is not among its tables' % (d[0], n))
        for t in d[3]:
            if t not in KINDS:
                problems.append('%s: %s is not a table of IN-18e' % (d[0], t))
    kinds = spec_kinds(path)
    if kinds != KINDS:
        problems.append('IN-18e\'s kinds differ from duty.py\'s: spec-only %s, duty-only %s' % (
            sorted(set(kinds.items()) - set(KINDS.items())), sorted(set(KINDS.items()) - set(kinds.items()))))
    return problems


def tables_in_no_row():
    """IN-18e's tables that no cause changes (a note for the record)."""
    used = {t for d in DUTY for t in d[3]}
    return sorted(t for t in KINDS if t not in used)


if __name__ == '__main__':
    p = check_spec()
    for line in p:
        print('duty: ' + line)
    print('duty: tables in no row: %s' % ', '.join(tables_in_no_row()))
    print('duty: %s' % ('the spec and duty.py agree' if not p else '%d problems' % len(p)))
