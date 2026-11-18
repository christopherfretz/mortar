#!/usr/bin/env python3
"""place.py IN MAP OUT OUTMAP --dead N[,N...] [options]

WP-228 step 1, item 6: hand-made debug placements. IN is an
incremental-mode full link by mortar (every debug contribution in its size
class, IN-50) whose map MAP (`--mortar-map`) places the contributions of
the DEAD files (map file numbers) somewhere in each debug section: the
layout the update would leave (IN-50's three placements are made by the
link order the caller chose, the old version of a changed file linked as a
renamed, collected copy, E6's emulation). This script makes those
contributions dead ranges as G-1 would write them, and writes OUT and the
map OUTMAP that claims them (`dead <sec> <off> <size> neutral`, the dead
files' `nplace` lines of debug sections removed):

  .debug_info      IN-51's block units (DWARF 5 header, `DW_UT_compile`,
                   abbreviation code 1), naming the neutral abbreviation
                   table mortar wrote (the `01 11 00 80 40 09 00 00` hole);
                   with --addr-base and a dead file that has a .debug_addr
                   contribution, the first unit carries `DW_AT_addr_base`
                   aimed at that dead table, naming a second table
                   `01 11 00 73 17 80 40 09 00 00` written at the start of
                   the dead file's .debug_abbrev range (needs 11 bytes)
  .debug_abbrev    zero bytes
  .debug_line      IN-51b's v5 table (mortar's neutral.rs, version 5 always)
  .debug_rnglists, .debug_loclists   a v5 header, offset count 0, zeros
  .debug_str_offsets                 a v5 header, zeros (granule 4)
  .debug_addr      a v5 header (address size 8), zeros
  .debug_ranges, .debug_loc          zero bytes
  .debug_frame     a version-1 CIE, `DW_CFA_nop`
  .debug_macro     PROPOSED, untested before this script: a version-5 unit,
                   flags 0, one `DW_MACRO_undef` (0x02) entry whose inline
                   string fills the range, then the terminating 0
  .debug_aranges   rewritten whole (IN-54): the dead files' sets dropped,
                   the live sets in their order, the section shortened;
                   the live files' `nplace` lines moved accordingly
  .debug_str, .debug_line_str   untouched (IN-56: dead strings stay)

Options:
  --per-unit     .debug_info: one neutral unit per unit of the old
                 contribution (IN-51's literal "each unit keeps its offset
                 and its unit_length"); default: one unit per dead range
  --pieces       every dead range of every section written as a chain of
                 neutral pieces of lengths min, min+1, ... (each form's
                 minimum), the rest last: a dead range reused (IN-50 step
                 2) leaves such remainders
  --addr-base    see .debug_info above
  --keep-aranges .debug_aranges left as linked (its dead sets name dead
                 units: the negative case of IN-54)
Prints one line per section: ranges and bytes neutralized.
"""
import struct
import sys

sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from elfdbg import Elf  # noqa: E402

ABBREV = bytes([0x01, 0x11, 0x00, 0x80, 0x40, 0x09, 0x00, 0x00])
ABBREV_ADDR = bytes([0x01, 0x11, 0x00, 0x73, 0x17, 0x80, 0x40, 0x09, 0x00, 0x00])

u16 = lambda v: struct.pack('<H', v)
u32 = lambda v: struct.pack('<I', v)


def uleb_len(v):
    n = 1
    while v >= 0x80:
        v >>= 7
        n += 1
    return n


def uleb_padded(v, n):
    out = bytearray()
    for i in range(n):
        b = v & 0x7f
        v >>= 7
        out.append(b | 0x80 if i + 1 < n else b)
    return bytes(out)


def block_tail(rest):
    """A DW_FORM_block's ULEB length and body filling exactly `rest`."""
    for n in range(1, 11):
        if rest >= n and uleb_len(rest - n) <= n:
            return uleb_padded(rest - n, n) + bytes(rest - n)
    raise ValueError(rest)


# name: (minimum, granule) -- neutral.rs's and IN-51b's table, plus the
# forms this package proposes (loclists, loc, frame, addr, macro)
MINS = {'.debug_info': (15, 1), '.debug_abbrev': (1, 1), '.debug_line': (32, 1),
        '.debug_rnglists': (12, 1), '.debug_loclists': (12, 1), '.debug_str_offsets': (8, 4),
        '.debug_addr': (8, 8), '.debug_ranges': (16, 1), '.debug_loc': (16, 1),
        '.debug_frame': (13, 1), '.debug_macro': (7, 1)}


def neutral(name, size, abbrev, addr_size=8, addr_base=None, abbrev_addr=None):
    """The neutral unit of `size` bytes of section `name` (neutral.rs's
    `unit` with version 5, ported; and the proposed forms)."""
    L = size
    if name == '.debug_info':
        if addr_base is not None:
            out = u32(L - 4) + u16(5) + bytes([1, addr_size]) + u32(abbrev_addr) + bytes([1]) + u32(addr_base)
        else:
            out = u32(L - 4) + u16(5) + bytes([1, addr_size]) + u32(abbrev) + bytes([1])
        return out + block_tail(L - len(out))
    if name in ('.debug_abbrev', '.debug_ranges', '.debug_loc'):
        return bytes(L)
    if name == '.debug_line':
        common = bytes([1, 1, 1, 0xfb, 14, 7, 0, 1, 1, 1, 1, 0])
        out = u32(L - 4) + u16(5) + bytes([addr_size, 0]) + u32(20) + common + bytes([1, 1, 0x08, 0, 1, 1, 0x08, 0])
        assert len(out) <= L, L
        return out + bytes([6]) * (L - len(out))
    if name in ('.debug_rnglists', '.debug_loclists'):
        return u32(L - 4) + u16(5) + bytes([addr_size, 0]) + u32(0) + bytes(L - 12)
    if name == '.debug_str_offsets':
        assert L % 4 == 0
        return u32(L - 4) + u16(5) + u16(0) + bytes(L - 8)
    if name == '.debug_addr':
        return u32(L - 4) + u16(5) + bytes([addr_size, 0]) + bytes(L - 8)
    if name == '.debug_frame':
        # CIE: length, id all ones (32-bit), version 1, aug "", caf 1,
        # daf -4 (0x7c), ra 30/16, then DW_CFA_nop
        return u32(L - 4) + b'\xff\xff\xff\xff' + bytes([1, 0, 1, 0x7c, 30]) + bytes(L - 13)
    if name == '.debug_macro':
        # version 5, flags 0, DW_MACRO_undef line 0 "xxx...", end 0
        body = L - 3 - 1  # after the header, before the final 0
        s = body - 1 - 1 - 1  # opcode, line ULEB 0, string's NUL
        assert s >= 0, L
        return u16(5) + bytes([0]) + bytes([2, 0]) + b'x' * s + b'\0' + b'\0'
    raise ValueError(name)


def pieces(name, D):
    M, G = MINS[name]
    out = []
    r = D
    L = M
    while r - L >= M and len(out) < 40:
        if L % G == 0:
            out.append(L)
            r -= L
        L += 1
    return out + [r]


# record kinds of mortar's map (plumb.md 3.2) and the token positions that
# hold a file id
FILE_FIELDS = {'file': [1], 'place': [1], 'piece': [1], 'nplace': [1], 'merged': [1],
               'discard': [1], 'fold': [1, 3], 'region': [1], 'redirect': [1],
               'symblock': [1], 'rplace': [1], 'rframe': [1], 'rgroup': [2]}


def drop_files(lines, dead):
    """The map of the update: the emulation's old files are not inputs of
    the update, so every record of theirs goes (and the `arg` naming
    them), and later ids close up."""
    paths = set()
    for ln in lines:
        w = ln.split()
        if w and w[0] == 'file' and int(w[1]) in dead and len(w) == 3:
            paths.add(w[2])
    ren = lambda f: f - sum(1 for d in dead if d < f)
    out = []
    for ln in lines:
        w = ln.split()
        if not w:
            out.append(ln)
            continue
        if w[0] == 'arg' and len(w) == 2 and w[1] in paths:
            continue
        pos = list(FILE_FIELDS.get(w[0], []))
        if w[0] == 'slot' and len(w) == 5:
            pos = [2]
        if w[0] == 'common' and len(w) == 6:
            pos = [5]
        skip = False
        for p_ in pos:
            if w[p_] == '-':
                continue
            f = int(w[p_])
            if f in dead:
                skip = True
                break
            w[p_] = str(ren(f))
        if skip:
            continue
        out.append(' '.join(w))
    return out


def main():
    a = [x for x in sys.argv[1:] if not x.startswith('--')]
    opts = [x for x in sys.argv[1:] if x.startswith('--')]
    src, mapf, out, outmap = a[:4]
    dead = set()
    for o in opts:
        if o.startswith('--dead='):
            dead |= {int(x) for x in o.split('=', 1)[1].split(',') if x}
    per_unit = '--per-unit' in opts
    as_pieces = '--pieces' in opts
    addr_base_on = '--addr-base' in opts
    keep_ar = '--keep-aranges' in opts
    elf = Elf(open(src, 'rb').read())
    b = bytearray(elf.b)
    lines = open(mapf).read().splitlines()
    ranges = {}  # (file, sec) -> [(off, size)]
    for ln in lines:
        w = ln.split()
        if w and w[0] == 'nplace' and int(w[1]) in dead and w[3].startswith('.debug_'):
            ranges.setdefault((int(w[1]), w[3]), []).append((int(w[4], 16), int(w[5], 16)))
    # merge each file's adjacent ranges (group sections follow its main one)
    for k, v in ranges.items():
        v.sort()
        m = []
        for o, s in v:
            if m and m[-1][0] + m[-1][1] == o:
                m[-1] = (m[-1][0], m[-1][1] + s)
            else:
                m.append((o, s))
        ranges[k] = m
    sec = {s.name: s for s in elf.secs}
    # the neutral abbreviation table mortar wrote
    ab = sec.get('.debug_abbrev')
    abbrev_at = None
    if ab is not None:
        abd = bytes(b[ab.off:ab.off + ab.size])
        abbrev_at = abd.rfind(ABBREV)
    new_dead = []  # (sec, off, size)
    report = {}
    dead_files = sorted(dead)
    # .debug_info first needs the addr tables and abbrev slots
    for f in dead_files:
        addr = ranges.get((f, '.debug_addr'), [])
        abr = ranges.get((f, '.debug_abbrev'), [])
        for name in sorted({n for (ff, n) in ranges if ff == f}):
            if name == '.debug_aranges':
                continue
            if name in ('.debug_str', '.debug_line_str'):
                continue
            s = sec[name]
            for (o, size) in ranges[(f, name)]:
                if name == '.debug_info':
                    chunks = []
                    if per_unit:
                        p = o
                        while p < o + size:
                            ln, = struct.unpack_from('<I', b, s.off + p)
                            chunks.append((p, ln + 4))
                            p += ln + 4
                    elif as_pieces:
                        q = o
                        for L in pieces(name, size):
                            chunks.append((q, L))
                            q += L
                    else:
                        chunks = [(o, size)]
                    for k, (p, L) in enumerate(chunks):
                        if k == 0 and addr_base_on and addr and abr and abr[0][1] >= 11:
                            abo = abr[0][0]
                            data = neutral(name, L, abbrev_at, addr_base=addr[0][0] + 8, abbrev_addr=abo)
                        else:
                            data = neutral(name, L, abbrev_at)
                        assert len(data) == L, (name, L, len(data))
                        b[s.off + p:s.off + p + L] = data
                else:
                    if as_pieces and name != '.debug_addr':
                        q = o
                        for L in pieces(name, size):
                            data = neutral(name, L, abbrev_at)
                            assert len(data) == L
                            b[s.off + q:s.off + q + L] = data
                            q += L
                    else:
                        data = neutral(name, size, abbrev_at)
                        assert len(data) == size, (name, size, len(data))
                        b[s.off + o:s.off + o + size] = data
                    if name == '.debug_abbrev' and addr_base_on and addr and (o, size) == abr[0] and size >= 11:
                        b[s.off + o:s.off + o + len(ABBREV_ADDR)] = ABBREV_ADDR
                new_dead.append((name, o, size))
                r = report.setdefault(name, [0, 0])
                r[0] += 1
                r[1] += size
    # .debug_aranges, rewritten whole (IN-54)
    shift_map = {}
    if '.debug_aranges' in sec and not keep_ar:
        s = sec['.debug_aranges']
        sets = []
        for ln in lines:
            w = ln.split()
            if w and w[0] == 'nplace' and w[3] == '.debug_aranges':
                sets.append((int(w[4], 16), int(w[5], 16), int(w[1]), int(w[2])))
        sets.sort()
        old = bytes(b[s.off:s.off + s.size])
        newb = bytearray()
        for (o, size, f, si) in sets:
            if f in dead:
                continue
            shift_map[(f, si)] = len(newb)
            newb += old[o:o + size]
        dropped = s.size - len(newb)
        b[s.off:s.off + s.size] = bytes(newb) + bytes(dropped)
        # the section header's size
        shdr = elf.shoff + s.i * elf.shentsize
        if elf.c64:
            struct.pack_into('<Q', b, shdr + 32, len(newb))
        else:
            struct.pack_into('<I', b, shdr + 20, len(newb))
        report['.debug_aranges'] = ['rewritten', dropped]
    open(out, 'wb').write(b)
    import os
    os.chmod(out, 0o755)
    # the map
    mo = []
    for ln in lines:
        w = ln.split()
        if w and w[0] == 'nplace' and w[3].startswith('.debug_'):
            f, si = int(w[1]), int(w[2])
            if f in dead and not (w[3] in ('.debug_aranges',) and keep_ar):
                continue
            if w[3] == '.debug_aranges' and not keep_ar:
                if (f, si) in shift_map:
                    w[4] = '%x' % shift_map[(f, si)]
                    ln = ' '.join(w)
        if w and w[0] == 'end':
            for (name, o, size) in sorted(new_dead):
                mo.append('dead %s %x %x neutral' % (name, o, size))
        mo.append(ln)
    if '--keep-file' not in opts:
        mo = drop_files(mo, dead)
    open(outmap, 'w').write('\n'.join(mo) + '\n')
    for k in sorted(report):
        print('%s %s' % (k, report[k]))


if __name__ == '__main__':
    main()
