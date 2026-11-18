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
from IN-18e's class table (WORDS below says which). A cell's
`TABLE.field` names its table (`FILE_INDEX.first_global` is a change of
FILE_INDEX). A cell may name a table only to say the cause does NOT change
it ("its `COPIES` rows change only with their files", "the keeper is the
copy whose file has the least `FILES.rank`, not stored"): NOT_CHANGED lists
those, with the spec's words, and the test holds them out of the row.

The table as it reads after WP-215 seam 3 and WP-220's seam: GROUPS
(per (signature, file), no keeper stored) and COMMONS (per (key, file), no
winner stored) and the new COPIES (55, per (key, file, symbol index)) are
a changed file's rows and a gone file's key-keyed rows; a re-elected group
has no row of its own (its drawn-in files are `changed`); an ODR-affected
key's DIAGS rows are the key's; ODR_REFS and ODR_CLASSES are PROVISIONAL
(not written until WP-236) and no cause names them. Seam 6 (WP-214):
a changed file whose symbol table is replaced and a gone file mark their
locals' old `SYMBOLS` run `GONE` (a SYMBOLS row's mark, FLAGS), so both
rows change SYMBOLS.

WP-247 (WP-228 R-2.3): `DEBUG_STRS` (57) is judged against THIS output,
not by equality with a full link's capture (an update appends its strings,
a full link packs them: both right). JUDGED_BY_OUTPUT holds it out of
`uncovered` and `debug_strs_check(state, output)` is its check: every row
names its string's bytes in the output section, and every string of the
output section is found. IN-18e's class table may list the kind or not yet
(the spec text is proposed in WP-247's record).
"""

import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))
SPEC = os.path.join(HERE, '..', '..', 'spec', 'incr.md')

# IN-18e's tables by kind (the class table of spec/incr.md §5.3).
KINDS = {
    'META': 1, 'STAT': 2, 'FILES': 3, 'PROBES': 4, 'OUTSECS': 5, 'SYMBOLS': 6, 'SECTIONS': 7, 'CHUNKS': 8,
    'ORDER': 9, 'SEGMENTS': 10, 'FRAGS': 11, 'SLOTS': 12, 'SIZES': 15, 'SYM_PLACE': 16,
    'DEFINED': 17, 'PATCHES': 18, 'REDIRECTS': 19, 'SIZED': 20, 'LAYOUT': 21, 'LEAVES': 22, 'STRINGS': 23,
    'MENTIONS': 32, 'RESOLVED': 33, 'INDEX': 34, 'HOLES': 39, 'FILE_INDEX': 41, 'EDGES': 42,
    'SYMTAB': 43, 'GROUPS': 44, 'KEY_REFS': 45, 'COMMONS': 46, 'ODR_CLASSES': 47, 'ODR_REFS': 48, 'DIAGS': 49,
    'SYMTAB_KEYS': 50, 'REGIONS': 51, 'NAMES': 52, 'COPIES': 55, 'FRAG_USE': 56,
}
# tables judged against the output, not by the duty list (WP-228 R-2.3)
JUDGED_BY_OUTPUT = {'DEBUG_STRS': 57}
NAMES = {k: n for n, k in list(KINDS.items()) + list(JUDGED_BY_OUTPUT.items())}
FLAGS = {'POSITIONAL': 'FILES', 'LAZY': 'FILES',     # FILES flags the cells name
         'GONE': 'SYMBOLS'}                           # a SYMBOLS row's mark (seam 6)
FILE_KEYED = ('SECTIONS', 'MENTIONS', 'INDEX', 'EDGES', 'FRAG_USE', 'SYMTAB', 'REGIONS')
# the key-keyed tables with a file in their key or row (a gone file's rows;
# ODR_REFS is PROVISIONAL, not written until WP-236)
FILE_KEY_ROWS = ('KEY_REFS', 'GROUPS', 'COMMONS', 'COPIES', 'DIAGS')
# a cell's names that say what the cause does NOT change
NOT_CHANGED = {
    'key': {'COPIES': 'its `COPIES` rows change only with their files, WP-220'},
    'group': {'FILES': 'the keeper is the copy whose file has the least `FILES.rank`, not stored'},
}

WORDS = {
    'gone': 'its row marked gone (FILES; FILE_INDEX.gone), its ranges gone (every file-keyed table), its '
            'key-keyed rows deleted (the key-keyed tables whose rows name a file)',
    'device': 'the device rows of STAT',
}

DUTY = (
    ('changed', 'a changed, appeared, drawn-in or newly extracted file',
     ('FILES', 'POSITIONAL', 'STAT', 'SECTIONS', 'MENTIONS', 'INDEX', 'EDGES', 'FRAG_USE', 'SYMTAB',
      'FILE_INDEX', 'KEY_REFS', 'GROUPS', 'COMMONS', 'COPIES', 'DIAGS', 'SYMBOLS', 'GONE'),
     ('FILES', 'STAT', 'SECTIONS', 'MENTIONS', 'INDEX', 'EDGES', 'FRAG_USE', 'SYMTAB', 'FILE_INDEX',
      'KEY_REFS', 'GROUPS', 'COMMONS', 'COPIES', 'DIAGS', 'SYMBOLS')),
    ('gone', 'a gone or no longer extracted file',
     ('LAZY', 'SYMBOLS', 'GONE'),
     ('FILES', 'FILE_INDEX', 'SYMBOLS') + FILE_KEYED + FILE_KEY_ROWS),
    ('touched', 'an input `stat`ed again but hashed equal (touched)',
     ('STAT',),
     ('STAT',)),
    ('device', 'the device stamps (IN-22a)',
     ('STAT',),
     ('STAT',)),
    ('key', 'an affected key',
     ('SYMBOLS', 'RESOLVED', 'KEY_REFS', 'NAMES', 'DIAGS', 'COPIES'),
     ('SYMBOLS', 'RESOLVED', 'KEY_REFS', 'NAMES', 'DIAGS')),
    ('group', 'a re-elected group',
     ('FILES',),
     ()),
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
    # seam 5 (WP-239): the kept records are FILES columns; format 5 has no
    # EH_RECORDS table and FRAG_USE (kind 56) in FRAG_AT's place.
    ('eh', 'an `.eh_frame` range moved or the section rewritten whole (IN-34a)',
     ('FILES',),
     ('FILES',)),
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
        if t in JUDGED_BY_OUTPUT:
            continue    # debug_strs_check judges it
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
        rows.append((cells[0], sorted(set(re.findall(r'`([A-Z][A-Z_]+)(?:\.[a-z_]+)?`', cells[1])))))
    return rows


def cause_cell(path, cause):
    """The spec's cell of rows for `cause`, as text."""
    for line in open(path):
        cells = [c.strip() for c in line.strip().strip('|').split('|')]
        if line.strip().startswith('|') and cells[0] == cause and len(cells) >= 2:
            return cells[1]
    return ''


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
        held_out = NOT_CHANGED.get(d[0], {})
        for n, words in held_out.items():
            if n not in names:
                problems.append('%s: %s is held out as not changed, but the spec does not name it' % (d[0], n))
            elif words not in cause_cell(path, cause):
                problems.append('%s: the spec no longer says %r' % (d[0], words))
            if n in d[3]:
                problems.append('%s: %s is held out as not changed and also among its tables' % (d[0], n))
        for n in names:
            t = FLAGS.get(n, n)
            if t not in d[3] and n not in held_out:
                problems.append('%s: %s named by the spec is not among its tables' % (d[0], n))
        for t in d[3]:
            if t not in KINDS:
                problems.append('%s: %s is not a table of IN-18e' % (d[0], t))
    kinds = spec_kinds(path)
    for t, k in JUDGED_BY_OUTPUT.items():
        if kinds.get(t, k) != k:
            problems.append('IN-18e gives %s kind %d, duty.py %d' % (t, kinds[t], k))
        kinds.pop(t, None)
    if kinds != KINDS:
        problems.append('IN-18e\'s kinds differ from duty.py\'s: spec-only %s, duty-only %s' % (
            sorted(set(kinds.items()) - set(KINDS.items())), sorted(set(KINDS.items()) - set(kinds.items()))))
    return problems


def tables_in_no_row():
    """IN-18e's tables that no cause changes (a note for the record)."""
    used = {t for d in DUTY for t in d[3]}
    return sorted(t for t in KINDS if t not in used)


# ---- DEBUG_STRS against the output (WP-247) -----------------------------------

M64 = (1 << 64) - 1
P0, P1, P2, P3 = 0x2d358dccaa6c78a5, 0x8bb84b93962eacc9, 0x4b33a62ed433d4a3, 0x4d5a2da51de1aa47


def _mum(a, b):
    r = a * b
    return r & M64, r >> 64


def _mix(a, b):
    lo, hi = _mum(a, b)
    return lo ^ hi


SEED = _mix(P0, P1)


def hash_bytes(s):
    """mortar_core::hash::hash_bytes (wyhash v4 with a fixed seed; the
    vectors of mortar_core/tests/hash.rs are its self-test)."""
    n = len(s)
    r8 = lambda at: int.from_bytes(s[at:at + 8], 'little')
    r4 = lambda at: int.from_bytes(s[at:at + 4], 'little')
    seed = SEED
    if n <= 16:
        if n >= 4:
            q = (n >> 3) << 2
            a = (r4(0) << 32) | r4(q)
            b = (r4(n - 4) << 32) | r4(n - 4 - q)
        elif n > 0:
            a, b = (s[0] << 16) | (s[n >> 1] << 8) | s[n - 1], 0
        else:
            a, b = 0, 0
    else:
        p, i = 0, n
        if i >= 48:
            l1 = l2 = seed
            while i >= 48:
                seed = _mix(r8(p) ^ P1, r8(p + 8) ^ seed)
                l1 = _mix(r8(p + 16) ^ P2, r8(p + 24) ^ l1)
                l2 = _mix(r8(p + 32) ^ P3, r8(p + 40) ^ l2)
                p, i = p + 48, i - 48
            seed ^= l1 ^ l2
        while i > 16:
            seed = _mix(r8(p) ^ P1, r8(p + 8) ^ seed)
            p, i = p + 16, i - 16
        a, b = r8(n - 16), r8(n - 8)
    lo, hi = _mum(a ^ P1, b ^ seed)
    return _mix(lo ^ P0 ^ n, hi ^ P1)


def name_hash(name, version=b''):
    """mortar_incr::update::rows::name_hash: the name, a NUL, the version."""
    return hash_bytes(bytes(name) + b'\0' + bytes(version))


DEBUG_STR_SECTIONS = {0: b'.debug_str', 1: b'.debug_line_str'}


def state_debug_strs(state):
    """The DEBUG_STRS rows [(out, hash, offset)] of the format-5 state
    `state` (bytes; IN-18c's directory, rows in place; not verified: the
    reader's tests do that)."""
    if state[:8] != b'MORTARSR':
        raise ValueError('not a format-5 state')
    count = int.from_bytes(state[16:20], 'little')
    for i in range(count):
        e = state[80 + 72 * i:80 + 72 * (i + 1)]
        kind, width = int.from_bytes(e[0:4], 'little'), int.from_bytes(e[4:8], 'little')
        if kind != JUDGED_BY_OUTPUT['DEBUG_STRS']:
            continue
        if width != 13:
            raise ValueError('DEBUG_STRS of width %d' % width)
        rows, body = int.from_bytes(e[8:16], 'little'), int.from_bytes(e[40:48], 'little')
        out = []
        for r in range(rows):
            b = state[body + 13 * r:body + 13 * (r + 1)]
            out.append((b[0], int.from_bytes(b[1:9], 'little'), int.from_bytes(b[9:13], 'little')))
        return out
    return None


def elf_sections(elf):
    """{name: bytes, or None for SHF_COMPRESSED} of an ELF64 little-endian
    file's sections."""
    u = lambda at, n: int.from_bytes(elf[at:at + n], 'little')
    shoff, shentsize, shnum, shstrndx = u(0x28, 8), u(0x3a, 2), u(0x3c, 2), u(0x3e, 2)
    hdr = lambda i: shoff + i * shentsize
    names = u(hdr(shstrndx) + 0x18, 8)
    out = {}
    for i in range(1, shnum):
        h = hdr(i)
        at = names + u(h, 4)
        name = elf[at:elf.index(b'\0', at)]
        flags, off, size = u(h + 8, 8), u(h + 0x18, 8), u(h + 0x20, 8)
        out[name] = None if flags & 0x800 else elf[off:off + size]
    return out


def debug_strs_check(state, elf):
    """WP-228 R-2.3 over a state's DEBUG_STRS and its output (both bytes):
    [problems] (empty: the table is right for this output). Every row
    names its string's bytes (the bytes at its offset, to a NUL inside the
    section, hash to its key); every string of each section is found (its
    key's row names those bytes). A compressed section is not judged."""
    rows = state_debug_strs(state)
    if rows is None:
        return ['no DEBUG_STRS table']
    secs = elf_sections(elf)
    problems = []
    by_key = {}
    for out, h, off in rows:
        if out not in DEBUG_STR_SECTIONS:
            problems.append('row (%d, %#x, %d): class %d' % (out, h, off, out))
            continue
        by_key.setdefault((out, h), []).append(off)
    for out, name in sorted(DEBUG_STR_SECTIONS.items()):
        sec = secs.get(name)
        mine = [(k, v) for k, v in by_key.items() if k[0] == out]
        if sec is None:
            if name not in secs and mine:
                problems.append('%s: %d rows, no section' % (name.decode(), len(mine)))
            continue

        def string_at(at):
            end = sec.find(b'\0', at)
            return None if at >= len(sec) or end < 0 else sec[at:end]

        for (o, h), offs in mine:
            for off in offs:
                s = string_at(off)
                if s is None or name_hash(s) != h:
                    problems.append('%s: row (%#x, %d) does not name its string' % (name.decode(), h, off))
        at = 0
        while at < len(sec):
            s = string_at(at)
            if s is None:
                problems.append('%s: no NUL after %d' % (name.decode(), at))
                break
            if not any(string_at(o) == s for o in by_key.get((out, name_hash(s)), ())):
                problems.append('%s: the string at %d is not found' % (name.decode(), at))
            at += len(s) + 1
    return problems


if __name__ == '__main__':
    p = check_spec()
    for line in p:
        print('duty: ' + line)
    print('duty: tables in no row: %s' % ', '.join(tables_in_no_row()))
    print('duty: %s' % ('the spec and duty.py agree' if not p else '%d problems' % len(p)))
