#!/usr/bin/env python3
"""neutral.py ELF MATCH [--addr-base=keep|drop]: WP-229 ruling 8 (a).

Emulates what an update leaves of a replaced split-DWARF file's
skeleton-side contributions, in place in a linked ELF64 little-endian
output (non-allocated sections only):

- .debug_info: the FIRST unit whose DW_AT_name, DW_AT_dwo_name or
  DW_AT_GNU_dwo_name ends with MATCH (the dead version: link it before
  the new one) becomes IN-51's block form: same offset and unit_length, a
  compile unit of the same version and address size (DWARF 5:
  DW_UT_compile, the 8-byte dwo_id of a skeleton header given to the
  block), one DIE whose single attribute is DW_AT_lo_user/DW_FORM_block
  filling the unit. With --addr-base=keep (default) and an addr_base in
  the unit (DW_AT_addr_base 0x73, or DW_AT_GNU_addr_base 0x2133 for
  DWARF 4), the DIE keeps it first (DW_FORM_sec_offset), as IN-51 says
  for Clang DWARF 5; --addr-base=drop leaves the dead .debug_addr table
  unnamed (the question of ruling 8).
- .debug_abbrev: the dead unit's table becomes the block abbreviation
  then zero bytes to the next table (IN-51).
- .debug_aranges: rewritten whole without the dead unit's sets (IN-54);
  sh_size shrinks.
- Every other contribution of the dead file (.debug_line, .debug_addr,
  .debug_rnglists, .debug_ranges, .debug_str_offsets, strings) is left as
  its compiler wrote it, tombstoned by the link (IN-53).
Prints one line saying what it did.
"""
import re
import struct
import subprocess
import sys


def shdrs(b):
    shoff, = struct.unpack_from("<Q", b, 0x28)
    shentsize, shnum, shstrndx = struct.unpack_from("<HHH", b, 0x3A)
    rows = []
    for i in range(shnum):
        at = shoff + i * shentsize
        nm, ty, fl, ad, off, sz = struct.unpack_from("<IIQQQQ", b, at)
        rows.append({"at": at, "nm": nm, "off": off, "size": sz})
    so = rows[shstrndx]["off"]
    out = {}
    for r in rows:
        e = b.index(b"\0", so + r["nm"])
        out[b[so + r["nm"]:e].decode()] = r
    return out


def uleb(v):
    out = bytearray()
    while True:
        c = v & 0x7F
        v >>= 7
        if v:
            out.append(c | 0x80)
        else:
            out.append(c)
            return bytes(out)


def block_fill(total):
    """DW_FORM_block: ULEB(len) + len zero bytes, exactly `total` bytes."""
    for s in range(1, 6):
        ln = total - s
        if ln >= 0 and len(uleb(ln)) == s:
            return uleb(ln) + bytes(ln)
    for s in range(1, 6):
        ln = total - s
        if ln >= 0 and len(uleb(ln)) <= s:
            u = bytearray(uleb(ln))
            while len(u) < s:
                u[-1] |= 0x80
                u.append(0)
            return bytes(u) + bytes(ln)
    sys.exit("cannot fill %d bytes" % total)


def main():
    keep = "--addr-base=drop" not in sys.argv
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    path, match = args
    b = bytearray(open(path, "rb").read())
    S = shdrs(b)
    io = S[".debug_info"]["off"]
    ab = S[".debug_abbrev"]
    dump = subprocess.run(["llvm-dwarfdump-22", "--debug-info", path],
                          capture_output=True, text=True).stdout
    units, cur = [], None
    for line in dump.splitlines():
        m = re.match(r"0x([0-9a-f]+): (Compile|Skeleton) Unit:", line)
        if m:
            cur = {"off": int(m.group(1), 16), "names": [], "addr_base": None}
            units.append(cur)
            continue
        if cur is None:
            continue
        m = re.search(r"DW_AT_(?:name|dwo_name|GNU_dwo_name)\s+\(\"([^\"]*)\"\)", line)
        if m:
            cur["names"].append(m.group(1))
        m = re.search(r"DW_AT_(GNU_)?addr_base\s+\((0x[0-9a-f]+)\)", line)
        if m and cur["addr_base"] is None:
            cur["addr_base"] = (0x2133 if m.group(1) else 0x73, int(m.group(2), 16))
    dead = next(u for u in units if any(n.endswith(match) for n in u["names"]))
    t = dead["off"]
    ulen, = struct.unpack_from("<I", b, io + t)
    ver, = struct.unpack_from("<H", b, io + t + 4)
    if ver >= 5:
        ut, asz = b[io + t + 6], b[io + t + 7]
        aoff, = struct.unpack_from("<I", b, io + t + 8)
        hdr = 12
        b[io + t + 6] = 1  # DW_UT_compile
    else:
        aoff, = struct.unpack_from("<I", b, io + t + 6)
        asz = b[io + t + 10]
        hdr = 11
    for u in units:
        if u is dead:
            continue
        v, = struct.unpack_from("<H", b, io + u["off"] + 4)
        a, = struct.unpack_from("<I", b, io + u["off"] + (8 if v >= 5 else 6))
        if a == aoff:
            sys.exit("abbrev table shared")
    nxt = []
    for u in units:
        v, = struct.unpack_from("<H", b, io + u["off"] + 4)
        a, = struct.unpack_from("<I", b, io + u["off"] + (8 if v >= 5 else 6))
        if a > aoff:
            nxt.append(a)
    tend = min(nxt) if nxt else ab["size"]
    base = dead["addr_base"] if keep else None
    if base:
        attr, val = base
        abbrev = bytes([1, 0x11, 0]) + uleb(attr) + bytes([0x17, 0x80, 0x40, 0x09, 0, 0])
        die_head = bytes([1]) + struct.pack("<I", val)
    else:
        abbrev = bytes([1, 0x11, 0, 0x80, 0x40, 0x09, 0, 0])
        die_head = bytes([1])
    b[ab["off"] + aoff:ab["off"] + tend] = abbrev + bytes(tend - aoff - len(abbrev))
    start, end = io + t + hdr, io + t + 4 + ulen
    b[start:end] = die_head + block_fill(end - start - len(die_head))
    # IN-54: .debug_aranges without the dead unit's sets.
    ar = S.get(".debug_aranges")
    removed = 0
    if ar:
        p, e, live = ar["off"], ar["off"] + ar["size"], bytearray()
        while p < e:
            ln, = struct.unpack_from("<I", b, p)
            info_off, = struct.unpack_from("<I", b, p + 6)
            if info_off != t:
                live += b[p:p + 4 + ln]
            else:
                removed += 1
            p += 4 + ln
        b[ar["off"]:ar["off"] + len(live)] = live
        b[ar["off"] + len(live):e] = bytes(e - ar["off"] - len(live))
        struct.pack_into("<Q", b, ar["at"] + 32, len(live))
    open(path, "wb").write(b)
    print("neutralized unit 0x%x (DWARF %d, %d bytes, addr_base %s), %d aranges set(s) removed"
          % (t, ver, ulen + 4, ("%#x kept" % base[0]) if base else "none", removed))


if __name__ == "__main__":
    main()
