"""The judge's units, tested without a link (the lane's --self-test and
--quick run them; H-1, WP-223): the dirty-range rule, the stderr
comparison, elflint's five allow patterns, the (d) machinery around the
stub of `incr_state canon`, and the duty table against the spec."""

import os
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
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

    def compare(self, a, b, p_u):
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

    def state(canon, reason='update'):
        v = _verdict(reason)
        judge._state(v, x, judge.Hop(canon=canon))
        return v.checks[0][1]

    def dut(canon, causes=None, reason='update'):
        v = _verdict(reason)
        judge._duty(v, x, judge.Hop(canon=canon, causes=causes))
        return v.checks[0][1]
    case('state-stub', state(None), 'n/a (d) not run (canon: U-2); P_U: decision records not read')
    case('duty-stub', dut(None), 'n/a (d) not run (canon: U-2)')
    stale = FakeCanon([('SECTIONS', 'u.o:7', 'address 0x4100, the capture 0x4000'), ('SYMTAB', 'u.o:3', 'x')])
    case('state-first-differing-row', state(stale),
         "FAIL canon(S') differs from canon(C(U)): SECTIONS row u.o:7: address 0x4100, the capture 0x4000 "
         "(and 1 more rows)")
    case('state-equal', state(FakeCanon([])), 'pass (decision rows not compared: P_U: decision records not read)')
    case('state-full-link', state(FakeCanon([]), 'small-output'), 'n/a (full link: small-output)')
    case('duty-covered', dut(stale), 'pass (2 rows differ)')
    case('duty-by-kind', dut(FakeCanon([(7, 'u.o:7', 'x')])), 'pass (1 rows differ)')
    case('duty-no-cause', dut(FakeCanon([('ODR_CLASSES', 'K', 'x')])),
         lambda r: r.startswith('FAIL ODR_CLASSES row K differs') and 'no cause' in r)
    case('duty-not-this-update', dut(FakeCanon([('DEFINED', 'k', 'x')]), causes={'changed', 'output'}),
         lambda r: r.startswith('FAIL DEFINED row k differs') and 'not this update' in r)
    # IN-78 after WP-215 seam 3 and WP-220: GROUPS, COMMONS and COPIES rows are
    # a changed file's; a re-elected group alone changes no row; a key's
    # COPIES rows change only with their files
    case('duty-groups-copies-are-a-changed-files', dut(FakeCanon([('GROUPS', 'g', 'x'), (55, 'k', 'x'),
                                                                  ('COMMONS', 'c', 'x')]), causes={'changed'}),
         'pass (3 rows differ)')
    case('duty-group-alone-changes-no-row', dut(FakeCanon([('GROUPS', 'g', 'x')]), causes={'group', 'key'}),
         lambda r: r.startswith('FAIL GROUPS row g differs') and 'not this update' in r)
    case('duty-copies-not-a-keys', dut(FakeCanon([('COPIES', 'k', 'x')]), causes={'key'}),
         lambda r: r.startswith('FAIL COPIES row k differs') and 'not this update' in r)
    case('duty-odr-refs-provisional', dut(FakeCanon([('ODR_REFS', 'k', 'x')])),
         lambda r: r.startswith('FAIL ODR_REFS row k differs') and 'no cause' in r)

    # DEBUG_STRS is judged against the output (WP-247; WP-228 R-2.3)
    case('duty-debug-strs-held-out', dut(FakeCanon([(57, 'k', 'x')]), causes={'touched'}), 'pass (1 rows differ)')
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
    case('debug-strs-string-missing', duty.debug_strs_check(debug_state(good[1:]), elf),
         ['.debug_str: the string at 0 is not found'])
    case('debug-strs-past-section', duty.debug_strs_check(debug_state(good + [(0, 7, 99)]), elf),
         ['.debug_str: row (0x7, 99) does not name its string'])
    case('debug-strs-bad-class', duty.debug_strs_check(debug_state(good + [(2, 7, 0)]), elf),
         ['row (2, 0x7, 0): class 2'])
    case('debug-strs-no-section', duty.debug_strs_check(debug_state(good), debug_elf({b'.debug_str': strs})),
         ['.debug_line_str: 1 rows, no section'])

    # the report's detail word and a scenario's `expect-full:` (an update
    # under --gc-sections is `update-not-implemented (live)` by rule)
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

    def expect(reason, det):
        v = _verdict(reason)
        judge.expect_full(v, ('update-not-implemented', 'live'), det)
        return v.checks[0][1]
    case('expect-full-named', expect('update-not-implemented', 'live'), 'pass (update-not-implemented (live))')
    case('expect-full-update-fails', expect('update', None), lambda r: r.startswith('FAIL an update'))
    case('expect-full-other-detail', expect('update-not-implemented', 'versioned'),
         lambda r: r.startswith('FAIL') and '(versioned)' in r)
    case('expect-full-earlier-fallback', expect('small-output', None), lambda r: r.startswith('n/a'))
    return out


if __name__ == '__main__':
    lines = units()
    for l in lines:
        print(l)
    sys.exit(1 if any('FAILS' in l for l in lines) else 0)
