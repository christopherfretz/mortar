#!/usr/bin/env python3
"""classify.py OBJ... : the model of WP-229's update-side rule
(`spec/incr.md` IN-55, IN-55a as narrowed by option (A)), written from the
rule's text, applied to relocatable objects: section names, COMDAT group
membership and `.debug_info` unit headers alone (no relocation read).

Prints one line per object:
  <obj> <answer> | sections=<debug sections> | units=<kind:count ...>
where <answer> is `handled(...)`, `debug-name-index(<why>)` or
`debug-unit-kind(<why>)`. First match wins: name index, then unit kind.
"""
import struct
import sys

NAME_INDEX = (b".debug_pubnames", b".debug_pubtypes", b".debug_names")
LEFT_OUT = (b".debug_gnu_pubnames", b".debug_gnu_pubtypes")
UT = {1: "compile", 2: "type", 3: "partial", 4: "skeleton", 5: "split_compile", 6: "split_type"}


def sections(data):
    if data[:4] != b"\x7fELF":
        raise ValueError("not ELF")
    cls, enc = data[4], data[5]
    e = "<" if enc == 1 else ">"
    if cls == 2:
        shoff, = struct.unpack_from(e + "Q", data, 0x28)
        shentsize, shnum, shstrndx = struct.unpack_from(e + "HHH", data, 0x3A)
        fmt = e + "IIQQQQIIQQ"
    else:
        shoff, = struct.unpack_from(e + "I", data, 0x20)
        shentsize, shnum, shstrndx = struct.unpack_from(e + "HHH", data, 0x2E)
        fmt = e + "IIIIIIIIII"
    rows = []
    for i in range(shnum):
        f = struct.unpack_from(fmt, data, shoff + i * shentsize)
        rows.append({"name_off": f[0], "type": f[1], "flags": f[2], "offset": f[4], "size": f[5]})
    st = rows[shstrndx]
    for r in rows:
        o = st["offset"] + r["name_off"]
        r["name"] = data[o:data.index(b"\0", o)]
    in_group = set()
    for r in rows:
        if r["type"] == 17:  # SHT_GROUP
            words = struct.unpack_from(e + "%dI" % (r["size"] // 4), data, r["offset"])
            if words and words[0] & 1:  # GRP_COMDAT
                in_group.update(words[1:])
    return rows, in_group, e


def units(data, off, size, e):
    """Yield (version, unit_type or None) per unit header; raise on a bad one."""
    end, p = off + size, off
    while p < end:
        if end - p < 4:
            raise ValueError("truncated unit_length")
        ln, = struct.unpack_from(e + "I", data, p)
        p += 4
        if ln == 0xFFFFFFFF:
            ln, = struct.unpack_from(e + "Q", data, p)
            p += 8
        elif ln >= 0xFFFFFFF0:
            raise ValueError("reserved unit_length")
        nxt = p + ln
        if nxt > end or ln < 2:
            raise ValueError("unit past its section")
        ver, = struct.unpack_from(e + "H", data, p)
        if ver == 5:
            yield ver, data[p + 2]
        elif 2 <= ver <= 4:
            yield ver, None
        else:
            raise ValueError("version %d" % ver)
        p = nxt


def classify(path):
    data = open(path, "rb").read()
    rows, in_group, e = sections(data)
    dbg = [r for r in rows if r["name"].startswith(b".debug_") or r["name"] == b".gdb_index"]
    names = sorted({r["name"].decode() for r in dbg})
    kinds = {}
    answer = None
    for r in dbg:
        if r["name"] in NAME_INDEX and r["size"] > 0:
            answer = answer or "debug-name-index(%s)" % r["name"].decode()
    for i, r in enumerate(rows):
        if answer:
            break
        n = r["name"]
        if not n.startswith(b".debug_"):
            continue
        if n == b".debug_types":
            answer = "debug-unit-kind(.debug_types)"
        elif n.endswith(b".dwo"):
            answer = "debug-unit-kind(%s)" % n.decode()
        elif n == b".debug_info" and i in in_group:
            answer = "debug-unit-kind(.debug_info in a COMDAT group)"
    has_addr = any(r["name"] == b".debug_addr" and r["size"] > 0 for r in rows)
    split4 = False
    for r in rows:
        if r["name"] != b".debug_info" or r["type"] == 8:
            continue
        try:
            for ver, ut in units(data, r["offset"], r["size"], e):
                k = "v%d" % ver if ut is None else UT.get(ut, "ut%#x" % ut)
                kinds[k] = kinds.get(k, 0) + 1
                if ut is None and has_addr:
                    split4 = True
                if ut is not None and ut not in (1, 4) and not answer:
                    answer = "debug-unit-kind(DW_UT_%s)" % UT.get(ut, "%#x" % ut)
        except ValueError as x:
            answer = answer or "debug-unit-kind(unreadable header: %s)" % x
    if not answer:
        gnu = any(r["name"] in LEFT_OUT and r["size"] > 0 for r in rows)
        answer = "handled(cu=%d skeleton=%d dwarf4_split=%d gnu_index=%d)" % (
            sum(v for k, v in kinds.items() if k in ("compile", "v2", "v3", "v4")),
            kinds.get("skeleton", 0), split4, gnu)
    return "%s %s | sections=%s | units=%s" % (
        path, answer, ",".join(names), " ".join("%s:%d" % kv for kv in sorted(kinds.items())))


if __name__ == "__main__":
    for p in sys.argv[1:]:
        try:
            print(classify(p))
        except Exception as x:  # noqa: BLE001 - a model's report
            print("%s ERROR %s" % (p, x))
