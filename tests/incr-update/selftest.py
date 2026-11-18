"""The judge's units, tested without a link (the lane's --self-test and
--quick run them; H-1, WP-223): the dirty-range rule, the stderr
comparison, elflint's four allow patterns, the (d) machinery around the
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


def tiny_elf(mach, plt_types, c64=True):
    """An ELF file with a `.rela.plt` (`.rel.plt` for i386) of the given
    relocation types and a `.shstrtab`: enough for the elflint check."""
    rela = mach != 3
    name = b'.rela.plt' if rela else b'.rel.plt'
    shstr = b'\0' + name + b'\0.shstrtab\0'
    if c64:
        recs = b''.join(struct.pack('<QQq', 0, t, 0) for t in plt_types)
    else:
        recs = b''.join(struct.pack('<II', 0, t) for t in plt_types)
    ehsize, shent = (64, 64) if c64 else (52, 40)
    rel_off = ehsize
    str_off = rel_off + len(recs)
    shoff = (str_off + len(shstr) + 7) & ~7
    if c64:
        eh = b'\x7fELF' + bytes([2, 1, 1]) + bytes(9) + struct.pack('<HHIQQQIHHHHHH', 2, mach, 1, 0, 0, shoff, 0,
                                                                     ehsize, 56, 0, shent, 3, 2)
        sh = [struct.pack('<IIQQQQIIQQ', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0),
              struct.pack('<IIQQQQIIQQ', 1, 4, 2, 0, rel_off, len(recs), 0, 0, 8, 24),
              struct.pack('<IIQQQQIIQQ', 1 + len(name) + 1, 3, 0, 0, str_off, len(shstr), 0, 0, 1, 0)]
    else:
        eh = b'\x7fELF' + bytes([1, 1, 1]) + bytes(9) + struct.pack('<HHIIIIIHHHHHH', 2, mach, 1, 0, 0, shoff, 0,
                                                                     ehsize, 32, 0, shent, 3, 2)
        sh = [struct.pack('<IIIIIIIIII', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0),
              struct.pack('<IIIIIIIIII', 1, 9, 2, 0, rel_off, len(recs), 0, 0, 4, 8),
              struct.pack('<IIIIIIIIII', 1 + len(name) + 1, 3, 0, 0, str_off, len(shstr), 0, 0, 1, 0)]
    body = eh + recs + shstr
    body += bytes(shoff - len(body))
    return body + b''.join(sh)


class FakeCanon:
    """A live `incr_state canon` for the test: the differences it is told."""
    live = True

    def __init__(self, diffs):
        self.diffs = diffs

    def compare(self, a, b, p_u):
        return list(self.diffs)


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

    # elflint's four allow patterns (harness.md "Checks")
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
    case('elflint-fifth-pattern', judge.elflint_complaints(
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
    case('duty-not-this-update', dut(FakeCanon([('GROUPS', 'g', 'x')]), causes={'changed', 'output'}),
         lambda r: r.startswith('FAIL GROUPS row g differs') and 'not this update' in r)
    return out


if __name__ == '__main__':
    lines = units()
    for l in lines:
        print(l)
    sys.exit(1 if any('FAILS' in l for l in lines) else 0)
