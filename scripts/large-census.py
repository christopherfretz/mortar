#!/usr/bin/env python3
"""The shape census of a captured bench link (WP-081; the record is
spec/landed/harness/WP-081.md).

    large-census.py --cwd DIR --response FILE [--json OUT]

DIR is the link's working directory and FILE its response file (one
argument per line, as `cargo xtask bench` writes it). GNU ld replays the
link once with `--trace --trace` to learn which archive members it loads; the
script then reads every object and every member of every archive on the
line (ELF64 little-endian only) and counts, over everything on the line
and over what the link loads:

    files, archive members, sections, SHF_ALLOC bytes, symbols (and
    global definitions), relocations (SHT_RELA/SHT_REL entries), COMDAT
    groups (and distinct signatures), .debug_* bytes, .eh_frame bytes,
    .debug_info bytes, the largest member

Objects the line names outside DIR (the crt files) are counted
separately as "system-loaded"; libraries found by `-l` (libstdc++.a,
libc.a) are not read. Nothing is written but OUT.
"""

import argparse
import json
import os
import re
import struct
import subprocess
import sys
import tempfile


def parse_response(text):
    """GNU ld's reading of a response file (enough for the bench's form)."""
    args, cur, quote, i = [], None, None, 0
    while i < len(text):
        c = text[i]
        if c == "\\":
            i += 1
            cur = (cur or "") + text[i]
        elif quote and c == quote:
            quote = None
        elif quote:
            cur = (cur or "") + c
        elif c in "\"'":
            quote = c
            cur = cur or ""
        elif c.isspace():
            if cur is not None:
                args.append(cur)
            cur = None
        else:
            cur = (cur or "") + c
        i += 1
    if cur is not None:
        args.append(cur)
    return args


def members(data):
    """(name, bytes) of each member of a regular archive."""
    assert data[:8] == b"!<arch>\n", "not a regular archive"
    out, pos, longnames = [], 8, b""
    while pos + 60 <= len(data):
        hdr = data[pos:pos + 60]
        name = hdr[:16].decode().rstrip()
        size = int(hdr[48:58].decode().strip())
        body = data[pos + 60:pos + 60 + size]
        pos += 60 + size + (size & 1)
        if name == "/" or name == "/SYM64/":
            continue
        if name == "//":
            longnames = body
            continue
        if name.startswith("/") and name[1:].isdigit():
            off = int(name[1:])
            end = longnames.index(b"/\n", off)
            name = longnames[off:end].decode()
        else:
            name = name.rstrip("/")
        out.append((name, body))
    return out


SHT_SYMTAB, SHT_RELA, SHT_REL, SHT_GROUP = 2, 4, 9, 17
SHF_ALLOC = 2


def elf_stats(data):
    if data[:4] != b"\x7fELF" or data[4] != 2 or data[5] != 1:
        return None
    if struct.unpack_from("<H", data, 16)[0] != 1:
        return None
    shoff, = struct.unpack_from("<Q", data, 0x28)
    shentsize, shnum, shstrndx = struct.unpack_from("<HHH", data, 0x3A)
    if shnum == 0 and shoff:
        shnum, = struct.unpack_from("<Q", data, shoff + 0x20)
    if shstrndx == 0xFFFF:
        shstrndx, = struct.unpack_from("<I", data, shoff + 0x28)
    secs = []
    for i in range(shnum):
        o = shoff + i * shentsize
        name, typ, flags, _addr, off, size, link, info, _al, entsize = struct.unpack_from(
            "<IIQQQQIIQQ", data, o)
        secs.append((name, typ, flags, off, size, link, info, entsize))
    stro = secs[shstrndx][3]

    def nm(n):
        e = data.index(b"\0", stro + n)
        return data[stro + n:e].decode("latin-1")

    s = dict(sections=shnum, alloc=0, symbols=0, globals_defined=0, relocs=0,
             groups=0, debug=0, debug_info=0, eh_frame=0, signatures=[])
    for (name, typ, flags, off, size, link, info, entsize) in secs:
        n = nm(name)
        if flags & SHF_ALLOC and typ != 8:
            s["alloc"] += size
        if n.startswith(".debug_") or n.startswith(".zdebug_"):
            s["debug"] += size
            if n == ".debug_info":
                s["debug_info"] += size
        if n == ".eh_frame":
            s["eh_frame"] += size
        if typ in (SHT_RELA, SHT_REL) and entsize:
            s["relocs"] += size // entsize
        if typ == SHT_SYMTAB and entsize:
            count = size // entsize
            s["symbols"] += count
            for k in range(info, count):
                st_name, st_info, _o, st_shndx = struct.unpack_from("<IBBH", data, off + k * entsize)
                if st_shndx != 0 and (st_info >> 4) in (1, 2, 10):
                    s["globals_defined"] += 1
        if typ == SHT_GROUP:
            flag, = struct.unpack_from("<I", data, off)
            if flag & 1:
                s["groups"] += 1
                # The signature: symbol `info` of the symbol table `link`.
                _sn, _st, _sf, soff, _ss, slink, _si, sent = secs[link]
                st_name, = struct.unpack_from("<I", data, soff + info * sent)
                stro2 = secs[slink][3]
                e = data.index(b"\0", stro2 + st_name)
                s["signatures"].append(data[stro2 + st_name:e])
    return s


KEYS = ["files", "members", "sections", "alloc", "symbols", "globals_defined", "relocs",
        "groups", "debug", "debug_info", "eh_frame", "bytes"]


def empty():
    d = {k: 0 for k in KEYS}
    d["signatures"] = set()
    d["largest"] = (0, "")
    return d


def add(total, s, size, label):
    for k in KEYS:
        if k in s:
            total[k] += s[k]
    total["bytes"] += size
    total["signatures"].update(s["signatures"])
    if size > total["largest"][0]:
        total["largest"] = (size, label)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cwd", required=True)
    ap.add_argument("--response", required=True)
    ap.add_argument("--json")
    a = ap.parse_args()
    cwd = os.path.realpath(a.cwd)
    args = parse_response(open(a.response).read())
    with tempfile.TemporaryDirectory() as t:
        p = subprocess.run(["/usr/bin/ld.bfd", "@" + os.path.realpath(a.response), "-o",
                            os.path.join(t, "out"), "--trace", "--trace"], cwd=cwd,
                           capture_output=True, text=True)
        if p.returncode:
            sys.exit("ld.bfd: " + p.stderr[:2000])
        trace = p.stdout
    loaded = set()
    for line in trace.splitlines():
        m = re.match(r"^\((.*)\)(.*)$", line.strip()) or re.match(r"^(.*\.a)\((.*)\)$", line.strip())
        if m:
            loaded.add((os.path.realpath(os.path.join(cwd, m.group(1))), m.group(2)))
    files = []
    after_o = False
    for arg in args:
        path = os.path.join(cwd, arg)
        skip, after_o = after_o, arg == "-o"
        if not skip and not arg.startswith("-") and os.path.isfile(path):
            rp = os.path.realpath(path)
            if rp not in files:
                files.append(rp)
    groups = {}
    for scope in ["line", "loaded", "system-loaded"]:
        groups[scope] = empty()
    sizes = []
    archives = 0
    for rp in files:
        data = open(rp, "rb").read()
        system = not rp.startswith(cwd + "/")
        if data[:8] == b"!<arch>\n":
            archives += 0 if system else 1
            for name, body in members(data):
                s = elf_stats(body)
                if s is None:
                    continue
                s["files"] = 0
                s["members"] = 1
                hit = (rp, name) in loaded
                if not system:
                    add(groups["line"], s, len(body), name)
                    if hit:
                        add(groups["loaded"], s, len(body), name)
                        sizes.append(len(body))
                elif hit:
                    add(groups["system-loaded"], s, len(body), name)
        else:
            s = elf_stats(data)
            if s is None:
                continue
            s["files"] = 1
            s["members"] = 0
            key = "system-loaded" if system else "line"
            add(groups[key], s, len(data), rp)
            if not system:
                add(groups["loaded"], s, len(data), rp)
                sizes.append(len(data))
    out = {"archives": archives}
    for scope, d in groups.items():
        d["distinct_signatures"] = len(d.pop("signatures"))
        d["largest"] = list(d["largest"])
        out[scope] = d
    sizes.sort()
    out["loaded_median_bytes"] = sizes[len(sizes) // 2] if sizes else 0
    text = json.dumps(out, indent=1)
    print(text)
    if a.json:
        open(a.json, "w").write(text)


if __name__ == "__main__":
    main()
