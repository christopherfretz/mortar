"""The judge's units, tested without a link (the lane's --self-test and
--quick run them; H-1, WP-223): the dirty-range rule, the stderr
comparison, elflint's five allow patterns, the (d) machinery around the
stub of `incr_state canon`, and the duty table against the spec."""

import os
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
# (d)'s units save nothing (FakeCanon writes no file): no scratch
SCRATCH = None
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, '..', 'l10'))
sys.dont_write_bytecode = True
import duty  # noqa: E402
import judge  # noqa: E402


def tiny_elf(mach, plt_types, c64=True, note=None, note_name=b'.note.stapsdt'):
    """An ELF file with a `.rela.plt` (`.rel.plt` for i386) of the given
    relocation types and a `.shstrtab`, and with `note` given a third
    section of those bytes named `note_name`: enough for the elflint
    check."""
    rela = mach != 3
    secs = [(b'.rela.plt' if rela else b'.rel.plt', 4 if rela else 9, 2,
             b''.join(struct.pack('<QQq', 0, t, 0) for t in plt_types) if c64 else
             b''.join(struct.pack('<II', 0, t) for t in plt_types), 8 if c64 else 4, (24 if c64 else 8))]
    if note is not None:
        secs.append((note_name, 7, 0, note, 4, 0))
    shstr = b'\0' + b''.join(n + b'\0' for n, *_ in secs) + b'.shstrtab\0'
    ehsize, shent = (64, 64) if c64 else (52, 40)
    off, body, nm = ehsize, b'', 1
    rows = [(0, 0, 0, 0, 0, 0, 0)]
    for n, ty, fl, data, al, es in secs:
        pad = (-(off + len(body))) % al
        body += bytes(pad)
        rows.append((nm, ty, fl, ehsize + len(body), len(data), al, es))
        body += data
        nm += len(n) + 1
    rows.append((nm, 3, 0, ehsize + len(body), len(shstr), 1, 0))
    body += shstr
    shoff = (ehsize + len(body) + 7) & ~7
    body += bytes(shoff - ehsize - len(body))
    n_sh, strndx = len(rows), len(rows) - 1
    if c64:
        eh = b'\x7fELF' + bytes([2, 1, 1]) + bytes(9) + struct.pack('<HHIQQQIHHHHHH', 2, mach, 1, 0, 0, shoff, 0,
                                                                     ehsize, 56, 0, shent, n_sh, strndx)
        sh = [struct.pack('<IIQQQQIIQQ', a, t, f, 0, o, z, 0, 0, al, es) for a, t, f, o, z, al, es in rows]
    else:
        eh = b'\x7fELF' + bytes([1, 1, 1]) + bytes(9) + struct.pack('<HHIIIIIHHHHHH', 2, mach, 1, 0, 0, shoff, 0,
                                                                     ehsize, 32, 0, shent, n_sh, strndx)
        sh = [struct.pack('<IIIIIIIIII', a, t, f, 0, o, z, 0, 0, al, es) for a, t, f, o, z, al, es in rows]
    return eh + body + b''.join(sh)


def note(owner, ntype, desc):
    """One ELF note."""
    name = owner + b'\0'
    return (struct.pack('<III', len(name), len(desc), ntype) + name + bytes(-len(name) % 4) + desc
            + bytes(-len(desc) % 4))


class FakeCanon:
    """A live `incr_state canon` for the test: the differences it is told."""
    live = True

    def __init__(self, diffs):
        self.diffs = diffs

    def compare(self, a, b, p_u, save=None):
        return list(self.diffs)


def debug_state(rows):
    """A format-5 state's header and directory as duty.state_debug_strs
    reads them (no hashes): one DEBUG_STRS table of `rows`."""
    body = b''.join(bytes([o]) + h.to_bytes(8, 'little') + off.to_bytes(4, 'little') for o, h, off in rows)
    head = b'MORTARSR' + (5).to_bytes(4, 'little') + bytes(4) + (1).to_bytes(4, 'little') + bytes(80 - 20)
    entry = ((57).to_bytes(4, 'little') + (13).to_bytes(4, 'little') + len(rows).to_bytes(8, 'little')
             + bytes(24) + (152).to_bytes(8, 'little') + bytes(24))
    return head + entry + body


def debug_elf(sections):
    """An ELF64 file of section headers only, with `sections` {name: bytes}."""
    names = b'\0' + b''.join(n + b'\0' for n in sections) + b'.shstrtab\0'
    data = b''.join(sections.values()) + names
    shoff = 64 + len(data)
    hdrs, at, nat = [bytes(64)], 64, 1
    for n, b in list(sections.items()) + [(b'.shstrtab', names)]:
        hdrs.append(nat.to_bytes(4, 'little') + (1).to_bytes(4, 'little') + bytes(16)
                    + at.to_bytes(8, 'little') + len(b).to_bytes(8, 'little') + bytes(24))
        at, nat = at + len(b), nat + len(n) + 1
    eh = bytearray(64)
    eh[:4] = b'\x7fELF'
    eh[0x28:0x30] = shoff.to_bytes(8, 'little')
    eh[0x3a:0x3c] = (64).to_bytes(2, 'little')
    eh[0x3c:0x3e] = len(hdrs).to_bytes(2, 'little')
    eh[0x3e:0x40] = (len(hdrs) - 1).to_bytes(2, 'little')
    return bytes(eh) + data + b''.join(hdrs)


def exec_elf(addr, code):
    """An AArch64 ELF64 file with one `.text` (PROGBITS, AX) at `addr`
    holding `code`: enough for the islands check."""
    shstr = b'\0.text\0.shstrtab\0'
    off = 64
    body = code + shstr
    shoff = (off + len(body) + 7) & ~7
    body += bytes(shoff - off - len(body))
    eh = b'\x7fELF' + bytes([2, 1, 1]) + bytes(9) + struct.pack('<HHIQQQIHHHHHH', 2, 183, 1, addr, 0, shoff, 0,
                                                                 64, 56, 0, 64, 3, 2)
    sh = [bytes(64), struct.pack('<IIQQQQIIQQ', 1, 1, 6, addr, off, len(code), 0, 0, 4, 0),
          struct.pack('<IIQQQQIIQQ', 7, 3, 0, 0, off + len(code), len(shstr), 0, 0, 1, 0)]
    return eh + body + b''.join(sh)


def thunk_rows(key):
    """The rows of one 12-byte island at 0x400000 holding one thunk (kind
    0) whose key is the SYMBOLS row `key`."""
    return {'CHUNKS': [{'i': 0, 'tag': judge.TAG_PATCHES, 'size': 12, 'addr': 0x400000, 'out': None, 'part': 0,
                        'name': '.text'}],
            'PATCHES': [{'i': 0, 'chunk': 0, 'kind': 0, 'offset': 0, 'symbol': 0, 'section': None, 'value': 0}],
            'REDIRECTS': [], 'SYMBOLS': [dict(key, i=0)]}


def _verdict(reason='update'):
    v = judge.Verdict()
    v.reason = reason
    return v


def units(log=print):
    out = []

    def case(name, got, want):
        ok = got == want if not callable(want) else want(got)
        out.append('INCR-UPDATE SELF-TEST unit %s: %s' % (name, 'ok' if ok else 'FAILS: got %r' % (got,)))

    # the duty table: the spec and duty.py agree
    p = duty.check_spec()
    out.append('INCR-UPDATE SELF-TEST unit duty-table-agrees-with-spec: %s' % (
        'ok (%d causes; tables in no row: %s)' % (len(duty.DUTY), ', '.join(duty.tables_in_no_row()))
        if not p else 'FAILS: %s' % '; '.join(p)))

    # the dirty-range rule (IN-75)
    prev = bytes(64)
    new = bytearray(prev)
    new[10] = 1
    new[11] = 2
    case('dirty-covered', judge.dirty_check(prev, bytes(new), [(8, 4)]), None)
    case('dirty-stray-byte', judge.dirty_check(prev, bytes(new), [(8, 3)]),
         lambda r: r is not None and r.startswith('byte 0xb differs'))
    case('dirty-unsorted', judge.dirty_check(prev, bytes(new), [(20, 2), (8, 4)]),
         lambda r: r is not None and 'not sorted and disjoint' in r)
    case('dirty-overlap', judge.dirty_check(prev, bytes(new), [(8, 4), (10, 4)]),
         lambda r: r is not None and 'not sorted and disjoint' in r)
    case('dirty-past-file', judge.dirty_check(prev, bytes(new), [(8, 4), (60, 8)]),
         lambda r: r is not None and 'ends past the file' in r)
    case('dirty-grown-uncovered', judge.dirty_check(prev, bytes(new) + b'xy', [(8, 4), (64, 1)]),
         lambda r: r is not None and 'past the previous output' in r)
    case('dirty-identical-no-ranges', judge.dirty_check(prev, prev, []), None)

    # the stderr comparison (IN-75a)
    def stderr(xs, fs):
        v = _verdict()
        judge._stderr(v, judge.Link('.', 'x', 'x.map', 0, [], stderr=xs), judge.Link('.', 'f', 'f.map', 0, [],
                                                                                     stderr=fs))
        return v.checks[0][1]
    w = 'mortar: warning: the use of `mktemp\' is dangerous\n>>> referenced by c.o:(.text+0x28)\n'
    case('stderr-equal-modulo-report-line', stderr('incremental: update changed=1\n' + w, w), 'pass')
    case('stderr-dropped', stderr('', w), lambda r: r.startswith('FAIL 2 line(s) dropped'))
    case('stderr-duplicated', stderr(w + w, w), lambda r: r.startswith('FAIL 2 line(s) repeated: line 3'))
    case('stderr-order', stderr('b\na\n', 'a\nb\n'), lambda r: r.startswith('FAIL') and 'line 1' in r)
    case('stderr-not-normalised', stderr(w.replace('c.o', 'd.o'), w), lambda r: r.startswith('FAIL'))

    # elflint's five allow patterns (harness.md "Checks")
    a64 = tiny_elf(183, [1032, 1026])
    x64 = tiny_elf(62, [37])
    i386 = tiny_elf(3, [42], c64=False)
    rt = "section [ 3] '.rela.plt': relocation 0: invalid type"
    rs = "section [ 3] '.rela.plt': relocation 0: invalid symbol index"
    eh = "section [22] '.symtab': symbol 9 (__ehdr_start): st_value out of bounds"
    pcs = "section [22] '.symtab': symbol 227 (__arm_tpidr2_save): unknown bit set in st_other"
    case('elflint-four-allowed-a64', judge.elflint_complaints('\n'.join([rt, rs, eh, pcs, 'No errors']), a64), [])
    case('elflint-irelative-only', judge.elflint_complaints(rt.replace('relocation 0', 'relocation 1'), a64),
         lambda r: len(r) == 1)
    case('elflint-irelative-x64', judge.elflint_complaints(rt + '\n' + rs, x64), [])
    case('elflint-irelative-i386', judge.elflint_complaints(rt.replace('.rela.plt', '.rel.plt'), i386), [])
    case('elflint-variant-pcs-a64-only', judge.elflint_complaints(pcs, x64), lambda r: len(r) == 1)
    # the fifth: a probe note held to its section, owner and type (elflint
    # names the offset just past the note it read)
    sdt = note(b'stapsdt', 3, bytes(52)) + note(b'stapsdt', 3, bytes(50))
    st = "section [ 2] '.note.stapsdt': unknown object file note type 3 with owner name 'stapsdt' at offset %d"
    case('elflint-stapsdt-allowed', judge.elflint_complaints('%s\n%s' % (st % 72, st % 144),
                                                             tiny_elf(183, [], note=sdt)), [])
    case('elflint-stapsdt-x64', judge.elflint_complaints(st % 72, tiny_elf(62, [], note=sdt)), [])
    case('elflint-stapsdt-not-a-note-end', judge.elflint_complaints(st % 70, tiny_elf(183, [], note=sdt)),
         lambda r: len(r) == 1)
    case('elflint-stapsdt-other-owner', judge.elflint_complaints(
        st % 72, tiny_elf(183, [], note=note(b'stapsdx', 3, bytes(52)))), lambda r: len(r) == 1)
    case('elflint-stapsdt-other-type', judge.elflint_complaints(
        st % 72, tiny_elf(183, [], note=note(b'stapsdt', 4, bytes(52)))), lambda r: len(r) == 1)
    case('elflint-stapsdt-other-section', judge.elflint_complaints(
        st % 72, tiny_elf(183, [], note=sdt, note_name=b'.note.stapsdx')), lambda r: len(r) == 1)
    case('elflint-stapsdt-wrong-index', judge.elflint_complaints(
        st.replace('[ 2]', '[ 1]') % 72, tiny_elf(183, [], note=sdt)), lambda r: len(r) == 1)
    case('elflint-sixth-pattern', judge.elflint_complaints(
        "section [ 5] '.comment': has wrong flags: expected none, is MERGE|STRINGS", a64), lambda r: len(r) == 1)

    # (d): the stub, and the machinery around it with a live comparator
    x = judge.Link(HERE, 'x', 'x.map', 0, [])

    # the files exist: FakeCanon does not read them, the judge asks they exist
    any_file = os.path.join(HERE, 'selftest.py')

    def state(canon, reason='update', identity=True, base_was_update=False, g_state=any_file):
        v = _verdict(reason)
        judge._state(v, x, judge.Hop(canon=canon, state=any_file, g_state=g_state, identity=identity,
                                     base_was_update=base_was_update, scratch=SCRATCH))
        return v.checks[0][1]

    def dut(canon, causes=None, reason='update'):
        v = _verdict(reason)
        judge._duty(v, x, judge.Hop(canon=canon, causes=causes, state=any_file, state_prev=any_file,
                                    scratch=SCRATCH))
        return v.checks[0][1]

    def row(table, i, a, b):
        return (table, 'row %d' % i, '%s != %s' % (a.hex(), b.hex()))
    case('state-stub', state(None), "n/a (d) not run (no incr_state: (d0)'s comparator absent); "
                                    "P_U: decision records not read")
    case('duty-stub', dut(None), "n/a (d) not run (no incr_state: (d0)'s comparator absent)")
    # (d0) (WP-267 P-4): the rules, on the comparator's own lines
    case('d0-parse', judge.parse_compare('SECTIONS row 3: 00 != 01\nHOLES 0a0b: only in the first\n'
                                         'HOLES : 2 rows against 1 (duplicates)\n3 differences\n'),
         [('SECTIONS', 'row 3', '00 != 01'), ('HOLES', '0a0b', 'only in the first'),
          ('HOLES', '', '2 rows against 1 (duplicates)')])
    case('d0-parse-count-wrong', judge.parse_compare('SECTIONS row 3: 00 != 01\n2 differences\n'), None)
    case('d0-parse-no-count', judge.parse_compare('SECTIONS row 3: 00 != 01\n'), None)
    sec = bytes(range(76))
    sec_moved = sec[:64] + bytes(12)              # only the place (chunk, offset) differs
    sec_stale = sec[:40] + b'\xff' + sec[41:]     # sh_size differs: not placement
    stale = FakeCanon([row('SYMBOLS', 7, b'\x01' * 36, b'\x02' * 36), row('SECTIONS', 2, sec, sec_moved)])
    case('d0-identity-one-stale-row-fails', state(FakeCanon([row('SYMBOLS', 7, b'\x01' * 36, b'\x02' * 36)])),
         lambda r: r.startswith("FAIL (d0 exact: an identity hop on a full link) canon(S') differs from canon(G): "
                                "SYMBOLS row row 7"))
    case('d0-identity-placement-row-fails-too', state(FakeCanon([row('SECTIONS', 2, sec, sec_moved)])),
         lambda r: r.startswith('FAIL (d0 exact'))
    case('d0-identity-equal', state(FakeCanon([])), 'pass (d0 exact)')
    case('d0-moved-stale-non-placement-row-fails', state(stale, identity=False),
         lambda r: r.startswith("FAIL (d0 outside placement: not an identity hop) canon(S') differs from canon(G) "
                                "outside the placement rows: SYMBOLS row row 7"))
    case('d0-moved-stale-column-fails', state(FakeCanon([row('SECTIONS', 2, sec, sec_stale)]), identity=False),
         lambda r: r.startswith('FAIL (d0 outside placement') and 'SECTIONS row row 2' in r)
    case('d0-moved-placement-only-passes', state(FakeCanon([
        row('SECTIONS', 2, sec, sec_moved), ('HOLES', 'aa', 'only in the first'), row('CHUNKS', 0, b'a', b'b'),
        row('FILES', 1, bytes(97), bytes(72) + b'\x10' + bytes(24)),
        row('SLOTS', 0, b'\x01' * 12, b'\x02' * 12), row('SLOTS', 1, b'\x02' * 12, b'\x01' * 12)]),
        identity=False), 'pass (d0 outside placement: 6 rows excluded)')
    case('d0-moved-slots-not-a-permutation-fails', state(FakeCanon([
        row('SLOTS', 0, b'\x01' * 12, b'\x02' * 12), row('SLOTS', 1, b'\x02' * 12, b'\x03' * 12)]),
        identity=False), lambda r: r.startswith('FAIL (d0 outside placement') and 'SLOTS' in r)
    case('d0-files-eh-records-not-placement', state(FakeCanon([
        row('FILES', 1, bytes(97), bytes(80) + b'\x01' + bytes(16))]), identity=False),
        lambda r: r.startswith('FAIL (d0 outside placement') and 'FILES' in r)
    case('d0-base-was-update-identity-not-exact', state(FakeCanon([row('SECTIONS', 2, sec, sec_moved)]),
                                                         base_was_update=True),
         'pass (d0 outside placement: 1 rows excluded)')
    case('d0-base-was-update-stale-row-fails', state(stale, base_was_update=True),
         lambda r: r.startswith('FAIL (d0 outside placement: the base was an update)'))
    case('d0-identity-unknown-not-exact', state(FakeCanon([row('SECTIONS', 2, sec, sec_moved)]), identity=None),
         'pass (d0 outside placement: 1 rows excluded)')
    case('d0-never-stat-and-directory', judge.d0_normalise(
        [('STAT', 'row 0', 'x'), row('STRINGS', 0, b'/r/ws/b', b'/r/h1/b'), row('STRINGS', 1, b'/r/ws/b', b'/r/h1/c')],
        ('/r/ws', '/r/h1')),
        ([row('STRINGS', 1, b'/r/ws/b', b'/r/h1/c')], {'STAT': 1, 'STRINGS (the directory)': 1}))
    # supervisor's ruling (P-4 hand-back): a DEBUG_STRS offset is placement
    # off an exact hop (judged against the output instead); its hash is not
    ds, ds_moved, ds_hash = b'\x00' + b'\xaa' * 8 + b'\x67\x05\x00\x00', b'\x00' + b'\xaa' * 8 + b'\x7a\x05\x00\x00', \
        b'\x00' + b'\xab' * 8 + b'\x67\x05\x00\x00'
    case('d0-moved-debug-strs-offset-excluded', state(FakeCanon([row('DEBUG_STRS', 0, ds, ds_moved)]),
                                                      identity=False), 'pass (d0 outside placement: 1 rows excluded)')
    case('d0-moved-debug-strs-hash-fails', state(FakeCanon([row('DEBUG_STRS', 0, ds, ds_hash)]), identity=False),
         lambda r: r.startswith('FAIL (d0 outside placement') and 'DEBUG_STRS' in r)
    case('d0-identity-debug-strs-offset-fails', state(FakeCanon([row('DEBUG_STRS', 0, ds, ds_moved)])),
         lambda r: r.startswith('FAIL (d0 exact'))
    case('d0-no-g-state', state(FakeCanon([]), g_state=os.path.join(HERE, 'no-such-state')),
         'n/a (d0) has no G state (the G link wrote none)')
    case('d0-full-link', state(FakeCanon([]), 'small-output'), 'n/a (full link: small-output)')
    case('d0-tool-absent-is-na', state(judge.CANON_STUB),
         lambda r: r.startswith('n/a ') and 'not run' in r)
    case('d0-tool-failed-fails', state(type('T', (), {'live': True, 'compare': lambda self, *a, **k:
                                                       judge.CompareFailed('TIMEOUT incr_state compare')})()),
         'FAIL (d0) TIMEOUT incr_state compare')
    case('d0-kinds', [judge.d0_kind(r) for r in ('pass (d0 exact)', 'pass (d0 outside placement: 2 rows excluded)',
                                                 'n/a (full link: x)', 'FAIL (d0) x')],
         ['exact', 'outside-placement', 'n/a', 'FAIL'])
    covered = 'pass (S against S\': %d rows differ; not judged: STRINGS)'
    case('duty-covered', dut(stale), covered % 2)
    case('duty-by-kind', dut(FakeCanon([(7, 'u.o:7', 'x')])), covered % 1)
    case('duty-strings-held-out', dut(FakeCanon([('STRINGS', 'row 0', 'x')])), covered % 0)
    case('duty-no-cause', dut(FakeCanon([('ODR_CLASSES', 'K', 'x')])),
         lambda r: r.startswith('FAIL ODR_CLASSES row K differs') and 'no cause' in r)
    case('duty-not-this-update', dut(FakeCanon([('DEFINED', 'k', 'x')]), causes={'changed', 'output'}),
         lambda r: r.startswith('FAIL DEFINED row k differs') and 'not this update' in r)
    # IN-78 after WP-215 seam 3 and WP-220: GROUPS, COMMONS and COPIES rows are
    # a changed file's; a re-elected group alone changes no row; a key's
    # COPIES rows change only with their files
    case('duty-groups-copies-are-a-changed-files', dut(FakeCanon([('GROUPS', 'g', 'x'), (55, 'k', 'x'),
                                                                  ('COMMONS', 'c', 'x')]), causes={'changed'}),
         covered % 3)
    case('duty-group-alone-changes-no-row', dut(FakeCanon([('GROUPS', 'g', 'x')]), causes={'group', 'key'}),
         lambda r: r.startswith('FAIL GROUPS row g differs') and 'not this update' in r)
    case('duty-copies-not-a-keys', dut(FakeCanon([('COPIES', 'k', 'x')]), causes={'key'}),
         lambda r: r.startswith('FAIL COPIES row k differs') and 'not this update' in r)
    case('duty-odr-refs-provisional', dut(FakeCanon([('ODR_REFS', 'k', 'x')])),
         lambda r: r.startswith('FAIL ODR_REFS row k differs') and 'no cause' in r)

    # DEBUG_STRS is judged against the output (WP-247; WP-228 R-2.3)
    case('duty-debug-strs-held-out', dut(FakeCanon([(57, 'k', 'x')]), causes={'touched'}), covered % 1)
    vectors = [(b'', 0x93228a4de0eec5a2), (b'a', 0xaced12527fe5bff8), (b'abc', 0x989b4a209c1011c9),
               (b'_start', 0xa9058601da6bbdae), (b'0123456789abcdef', 0x88de385a856cfb95),
               (b'0123456789abcdefg', 0x14f37288a5f8073a),
               (b'_ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6appendEPKcm'
                b'XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX', 0xd5923a012db1fc8a), (b'\x00\xff\x80\x7f', 0x07d2f360dee87cf0)]
    case('debug-strs-hash-port', [k for k, v in vectors if duty.hash_bytes(k) != v], [])
    strs = b'int\0main\0char\0'
    good = [(0, duty.name_hash(b'int'), 0), (0, duty.name_hash(b'main'), 4), (0, duty.name_hash(b'char'), 9),
            (0, duty.name_hash(b'ain'), 5), (1, duty.name_hash(b'a.c'), 0)]
    elf = debug_elf({b'.debug_str': strs, b'.debug_line_str': b'a.c\0'})
    case('debug-strs-right', duty.debug_strs_check(debug_state(good), elf), [])
    case('debug-strs-row-wrong-bytes', duty.debug_strs_check(debug_state(good[:2] + [(0, good[2][1], 4)]), elf),
         lambda r: any('does not name its string' in x for x in r) and any('at 9 is not found' in x for x in r))
    # a string in the map's dead range (an update keeps it as written: L-147,
    # G-5) need not be found; outside one it must
    case('debug-strs-dead-string-excused', duty.debug_strs_check(debug_state(good[1:]), elf,
                                                                 {b'.debug_str': [(0, 4)]}), [])
    case('debug-strs-dead-elsewhere-not-excused', duty.debug_strs_check(debug_state(good[1:]), elf,
                                                                        {b'.debug_line_str': [(0, 4)]}),
         lambda r: any('at 0 is not found' in x for x in r))
    case('debug-strs-string-missing', duty.debug_strs_check(debug_state(good[1:]), elf),
         ['.debug_str: the string at 0 is not found'])
    case('debug-strs-past-section', duty.debug_strs_check(debug_state(good + [(0, 7, 99)]), elf),
         ['.debug_str: row (0x7, 99) does not name its string'])
    case('debug-strs-bad-class', duty.debug_strs_check(debug_state(good + [(2, 7, 0)]), elf),
         ['row (2, 0x7, 0): class 2'])
    case('debug-strs-no-section', duty.debug_strs_check(debug_state(good), debug_elf({b'.debug_str': strs})),
         ['.debug_line_str: 1 rows, no section'])

    # the report's detail word and a scenario's `expect-full:` (a detail
    # word such as `live` of `update-not-implemented (live)`)
    import tempfile
    with tempfile.TemporaryDirectory(prefix='incr-update-selftest-') as d:
        rp = os.path.join(d, 'rep.txt')

        def detail(line):
            with open(rp, 'w') as fh:
                fh.write(line + '\nchanged=0 refilled=0 bytes=0 time=0\n')
            return judge.report_reason(rp), judge.report_detail(rp)
        case('report-detail-live', detail('incremental: full (update-not-implemented (live))'),
             ('update-not-implemented', 'live'))
        case('report-detail-none', detail('incremental: full (small-output)'), ('small-output', None))
        case('report-detail-update', detail('incremental: update'), ('update', None))

        # (c) after an update (ruling M2a2-1): `n/a base-was-update` exactly
        # when the previous successful hop's link was an update; a fall-back
        # whose base was a FULL link and whose report has no `delta end`
        # still FAILS.
        def oracle_c(base_was_update):
            with open(rp, 'w') as fh:
                fh.write('incremental: full (update-not-implemented (layout))\n')
            v = _verdict('update-not-implemented')
            xr = judge.Link(d, 'x', 'x.map', 0, [], rp)
            base = judge.Link(d, 'prog', 'prog.map', 0, [])
            judge._oracle(v, xr, base, base, judge.Hop(base_was_update=base_was_update))
            return v.checks[0][1]
        case('oracle-base-was-update', oracle_c(True), 'n/a base-was-update')
        case('oracle-base-full-no-delta-end-fails', oracle_c(False), 'FAIL the report has no `delta end`')

        # (c) when the hop's G link fails (ruling M2a3-3): the run goes on
        # and the oracle names the failed link, never `no previous full link`.
        def oracle_g_failed(status):
            v = _verdict('update')
            xr = judge.Link(d, 'x', 'x.map', 0, [], rp)
            base = judge.Link(d, 'prog', 'prog.map', 0, [])
            judge._oracle(v, xr, base, None, judge.Hop(g_status=status))
            return v.checks[0][1]
        case('oracle-g-link-failed', oracle_g_failed(1), 'n/a the G link failed (status 1)')
        case('oracle-g-link-ok-no-new', oracle_g_failed(0), 'n/a no previous full link')

    def expect(reason, det):
        v = _verdict(reason)
        judge.expect_full(v, ('update-not-implemented', 'live'), det)
        return v.checks[0][1]
    case('expect-full-named', expect('update-not-implemented', 'live'), 'pass (update-not-implemented (live))')
    case('expect-full-update-fails', expect('update', None), lambda r: r.startswith('FAIL an update'))
    case('expect-full-other-detail', expect('update-not-implemented', 'versioned'),
         lambda r: r.startswith('FAIL') and '(versioned)' in r)
    case('expect-full-earlier-fallback', expect('small-output', None), lambda r: r.startswith('n/a'))

    def expect_any(det):
        v = _verdict('update-not-implemented')
        judge.expect_full(v, ('update-not-implemented', 'layout|symtab'), det)
        return v.checks[0][1]
    case('expect-full-one-of-two', expect_any('symtab'), lambda r: r.startswith('pass'))
    case('expect-full-none-of-two', expect_any('eh'), lambda r: r.startswith('FAIL') and '(eh)' in r)

    # WP-272's `islands` (the units over a real link are lane.py's
    # island_units): n/a by name off AArch64 and without an island
    case('islands-x86-64-na', judge.islands_na({}, tiny_elf(62, [])), 'n/a (x86-64: no patch islands)')
    case('islands-i386-na', judge.islands_na({}, tiny_elf(3, [], c64=False)), 'n/a (i386: no patch islands)')
    a64 = tiny_elf(183, [])
    case('islands-no-island-chunk-na', judge.islands_na({'CHUNKS': []}, a64), 'n/a (no island chunk)')
    empty = {'CHUNKS': [{'i': 0, 'tag': judge.TAG_PATCHES, 'size': 0}]}
    case('islands-empty-island-na', judge.islands_na(empty, a64), 'n/a (no island: 1 Patches chunks, all empty)')
    case('islands-rows-parse', judge.parse_rows('PATCHES 0 chunk 8 kind 2 offset 10 symbol - section 4 value ff\n'),
         {'PATCHES': [{'i': 0, 'chunk': 8, 'kind': 2, 'offset': 16, 'symbol': None, 'section': 4, 'value': 255}]})
    # a thunk whose key is a local or a versioned symbol: its target is not
    # resolved here, so `n/a` by name, never a pass or a FAIL (WP-276
    # review, ruling 6 (b)); a global key the output does not define FAILs
    thunk = exec_elf(0x400000, struct.pack('<III', 0x90000010, 0x91000210 | (0x123 << 10), 0xD61F0200))
    local = {'kind': 2, 'file': 1, 'ordinal': 3, 'name': 'helper', 'version': '%'}
    case('islands-thunk-local-key-na', judge.islands_check(thunk_rows(local), thunk, ''),
         'n/a (local key: thunk PATCHES row 0 to helper; the other rows judged)')
    versioned = {'kind': 1, 'file': None, 'ordinal': 0, 'name': 'f', 'version': 'V1'}
    case('islands-thunk-versioned-key-na', judge.islands_check(thunk_rows(versioned), thunk, ''),
         'n/a (versioned key: thunk PATCHES row 0 to f; the other rows judged)')
    glob = {'kind': 0, 'file': None, 'ordinal': 0, 'name': 'g', 'version': '%'}
    case('islands-thunk-undefined-global-fails', judge.islands_check(thunk_rows(glob), thunk, ''),
         lambda r: r.startswith('FAIL PATCHES row 0 (thunk kind 0 to g) at 0x400000: the thunk branches to 0x400123'))
    case('islands-thunk-local-not-a-thunk-fails', judge.islands_check(thunk_rows(local), exec_elf(
        0x400000, bytes(12)), ''), lambda r: r.startswith('FAIL PATCHES row 0 (thunk kind 0) at 0x400000: words'))
    case('islands-thunk-decode', judge.a64_thunk_target(0x90000010 | (1 << 29), 0x91000210 | (0x123 << 10),
                                                        0xD61F0200, 0x400ffc), 0x401123)
    return out


if __name__ == '__main__':
    lines = units()
    for l in lines:
        print(l)
    sys.exit(1 if any('FAILS' in l for l in lines) else 0)
