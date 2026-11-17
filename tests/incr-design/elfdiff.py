#!/usr/bin/env python3
"""WP-202: classify an object edit by WP-064a §3's refused-edit list.

    elfdiff.py diff BASE.o EDIT.o     one line per class that the edit hits
    elfdiff.py census OBJ...          per-object facts that IN-28b and s3 need

Objects only (ELF64 little-endian, AArch64 or x86-64), read with a small
parser here (no pyelftools on the host). Nothing is linked.

Classes printed by `diff` (spec/incr.md IN-27a's rows, WP-064a §3's names):
  size        an allocated section's size changed (the delta layout)
  sections    the section count or the section names changed
  moved       a definition moved inside its section (n1): (section, value)
  keys+/keys- a global key appears / disappears (the dependency index)
  attr        a global's binding, type or visibility changed (W1, n2, n3)
  common      a common became a definition or the reverse (n4)
  l2g/g2l     a name local in one and global in the other (W10)
  locals      the count of local symbols changed (.symtab's local block)
  merged      the strings of an SHF_MERGE section changed (s4, W6)
  eh          .eh_frame's size changed
  debug       a .debug_* section's size changed (IN-50's size class)
"""
import struct
import sys
from collections import Counter

GOT_TYPES = {183: {311, 312, 313, 309, 310}, 62: {9, 41, 42}}  # AArch64, x86-64


def members(path):
    """An object, or each ELF member of a regular archive (read in memory)."""
    b = open(path, "rb").read()
    if not b.startswith(b"!<arch>\n"):
        yield path, b
        return
    i, names = 8, b""
    while i + 60 <= len(b):
        h = b[i:i + 60]
        name, size = h[:16].decode().strip(), int(h[48:58].decode().strip())
        data = b[i + 60:i + 60 + size]
        if name == "//":
            names = data
        elif name.startswith("/") and name[1:].isdigit():
            o = int(name[1:])
            name = names[o:names.index(b"\n", o)].decode()
        if data[:4] == b"\x7fELF":
            yield f"{path}({name})", data
        i += 60 + size + (size & 1)


def load(path, b=None):
    if b is None:
        b = open(path, "rb").read()
    if b[:4] != b"\x7fELF" or b[4] != 2 or b[5] != 1:
        raise SystemExit(f"{path}: not ELF64 LE")
    machine = struct.unpack_from("<H", b, 18)[0]
    shoff, = struct.unpack_from("<Q", b, 40)
    shentsize, shnum, shstrndx = struct.unpack_from("<HHH", b, 58)
    secs = []
    for i in range(shnum):
        name, typ, flags, addr, off, size, link, info, align, ent = struct.unpack_from(
            "<IIQQQQIIQQ", b, shoff + i * shentsize)
        secs.append(dict(name=name, type=typ, flags=flags, off=off, size=size,
                         link=link, info=info, ent=ent, index=i))
    shs = secs[shstrndx]
    for s in secs:
        s["n"] = b[shs["off"] + s["name"]: b.index(b"\0", shs["off"] + s["name"])].decode()
    syms = []
    for s in secs:
        if s["type"] != 2:
            continue
        st = secs[s["link"]]
        for k in range(s["size"] // 24):
            nm, info, other, shndx, val, sz = struct.unpack_from("<IBBHQQ", b, s["off"] + 24 * k)
            n = b[st["off"] + nm: b.index(b"\0", st["off"] + nm)].decode(errors="replace")
            syms.append(dict(n=n, bind=info >> 4, type=info & 15, vis=other & 3,
                             other=other, shndx=shndx, val=val, size=sz, idx=k))
    relocs = []
    for s in secs:
        if s["type"] != 4:
            continue
        for k in range(s["size"] // 24):
            off, info, add = struct.unpack_from("<QQq", b, s["off"] + 24 * k)
            relocs.append((s["info"], info >> 32, info & 0xffffffff, add))
    return dict(b=b, machine=machine, secs=secs, syms=syms, relocs=relocs)


def secname(o, i):
    if i == 0:
        return "UND"
    if i >= 0xff00:
        return {0xfff1: "ABS", 0xfff2: "COM"}.get(i, hex(i))
    return o["secs"][i]["n"] if i < len(o["secs"]) else "?"


def sec_key(o):
    """Sections by (name, ordinal among same-named)."""
    seen = Counter()
    out = {}
    for s in o["secs"][1:]:
        seen[s["n"]] += 1
        out[(s["n"], seen[s["n"]])] = s
    return out


def merged_strings(o, s):
    if not (s["flags"] & 0x10) or s["type"] == 8:
        return None
    d = o["b"][s["off"]: s["off"] + s["size"]]
    if s["flags"] & 0x20:
        return Counter(d.split(b"\0"))
    e = max(s["ent"], 1)
    return Counter(d[i:i + e] for i in range(0, len(d), e))


def globals_of(o):
    return {x["n"]: x for x in o["syms"] if x["bind"] != 0 and x["n"]}


def diff(a, b):
    out = []
    sa, sb = sec_key(a), sec_key(b)
    if set(sa) != set(sb):
        out.append(f"sections {len(sa)}->{len(sb)} +{len(set(sb)-set(sa))} -{len(set(sa)-set(sb))}")
    grew = [(k[0], sa[k]["size"], sb[k]["size"]) for k in sa if k in sb and sa[k]["size"] != sb[k]["size"]]
    alloc = [g for g in grew if not g[0].startswith(".debug") and g[0] not in (".symtab", ".strtab", ".shstrtab") and not g[0].startswith(".rela") and g[0] != ".eh_frame" and not g[0].startswith(".note")]
    if alloc:
        out.append("size " + " ".join(f"{n}:{x}->{y}" for n, x, y in alloc[:6]) + (f" (+{len(alloc)-6})" if len(alloc) > 6 else ""))
    if any(g[0] == ".eh_frame" for g in grew):
        out.append("eh " + " ".join(f"{x}->{y}" for n, x, y in grew if n == ".eh_frame"))
    dbg = [g for g in grew if g[0].startswith(".debug")]
    if dbg:
        out.append("debug " + " ".join(f"{n}:{x}->{y}" for n, x, y in dbg))
    ga, gb = globals_of(a), globals_of(b)
    plus, minus = set(gb) - set(ga), set(ga) - set(gb)
    if plus:
        out.append(f"keys+ {len(plus)} " + " ".join(sorted(plus)[:3]))
    if minus:
        out.append(f"keys- {len(minus)} " + " ".join(sorted(minus)[:3]))
    attr, common, moved = [], [], 0
    for n in set(ga) & set(gb):
        x, y = ga[n], gb[n]
        if (x["bind"], x["type"], x["vis"]) != (y["bind"], y["type"], y["vis"]):
            attr.append(f"{n}:{x['bind']}/{x['type']}/{x['vis']}->{y['bind']}/{y['type']}/{y['vis']}")
        if (x["shndx"] == 0xfff2) != (y["shndx"] == 0xfff2):
            common.append(n)
        if x["shndx"] not in (0, 0xfff2) and (secname(a, x["shndx"]), x["val"]) != (secname(b, y["shndx"]), y["val"]):
            moved += 1
    la = {x["n"] for x in a["syms"] if x["bind"] == 0 and x["n"] and x["type"] != 4}
    lb = {x["n"] for x in b["syms"] if x["bind"] == 0 and x["n"] and x["type"] != 4}
    l2g = la & set(gb) - set(ga)
    g2l = lb & set(ga) - set(gb)
    if attr:
        out.append(f"attr {len(attr)} " + " ".join(attr[:3]))
    if common:
        out.append(f"common {len(common)}")
    if l2g:
        out.append(f"l2g {len(l2g)} " + " ".join(sorted(l2g)[:3]))
    if g2l:
        out.append(f"g2l {len(g2l)}")
    lmoved = 0
    da = Counter((x["n"]) for x in a["syms"] if x["bind"] == 0 and x["n"] and x["type"] in (1, 2))
    named_a = {x["n"]: (secname(a, x["shndx"]), x["val"]) for x in a["syms"] if x["bind"] == 0 and x["n"] and x["type"] in (1, 2) and da[x["n"]] == 1}
    for x in b["syms"]:
        if x["bind"] == 0 and x["n"] in named_a and x["type"] in (1, 2) and named_a[x["n"]] != (secname(b, x["shndx"]), x["val"]):
            lmoved += 1
    if moved or lmoved:
        out.append(f"moved globals={moved} locals={lmoved}")
    nla = sum(1 for x in a["syms"] if x["bind"] == 0)
    nlb = sum(1 for x in b["syms"] if x["bind"] == 0)
    if nla != nlb:
        out.append(f"locals {nla}->{nlb}")
    mchg = []
    for k in set(sa) & set(sb):
        ma, mb = merged_strings(a, sa[k]), merged_strings(b, sb[k])
        if ma is not None and ma != mb:
            mchg.append(f"{k[0]}:+{sum((mb-ma).values())}-{sum((ma-mb).values())}")
    for k in set(sb) - set(sa):
        if merged_strings(b, sb[k]):
            mchg.append(f"{k[0]}:new")
    if mchg:
        out.append("merged " + " ".join(mchg[:4]))
    return out or ["none"]


def census(o):
    syms = o["syms"]
    got = GOT_TYPES.get(o["machine"], set())
    secsym = {x["idx"] for x in syms if x["type"] == 3}
    got_sec = sum(1 for (_, s, t, _) in o["relocs"] if t in got and s in secsym)
    got_sec_targets = len({s for (_, s, t, _) in o["relocs"] if t in got and s in secsym})
    names = Counter(x["n"] for x in syms if x["bind"] == 0 and x["n"] and x["type"] not in (3, 4))
    mapping = sum(v for k, v in names.items() if k.startswith("$"))
    dup = {k: v for k, v in names.items() if v > 1 and not k.startswith("$")}
    # relocations (any type) against a local symbol that is neither a section
    # nor a mapping symbol and whose name is duplicated: IN-28b's positional case
    dup_idx = {x["idx"] for x in syms if x["bind"] == 0 and x["n"] in dup}
    dup_refs = sum(1 for (_, s, t, _) in o["relocs"] if s in dup_idx)
    secsyms_per_sec = Counter(x["shndx"] for x in syms if x["type"] == 3)
    multi_secsym = sum(1 for v in secsyms_per_sec.values() if v > 1)
    return dict(locals=sum(1 for x in syms if x["bind"] == 0), secsyms=len(secsym),
                got_secsym_relocs=got_sec, got_secsym_targets=got_sec_targets,
                mapping=mapping, dup_names=len(dup), dup_entries=sum(dup.values()),
                dup_refs=dup_refs, multi_secsym=multi_secsym,
                dup_sample=" ".join(sorted(dup)[:3]))


def main():
    if len(sys.argv) >= 4 and sys.argv[1] == "diff":
        print(" | ".join(diff(load(sys.argv[2]), load(sys.argv[3]))))
    elif len(sys.argv) >= 3 and sys.argv[1] == "census":
        tot = Counter()
        n = 0
        for p in (m for a in sys.argv[2:] for m in members(a)):
            p, data = p
            c = census(load(p, data))
            n += 1
            for k, v in c.items():
                if isinstance(v, int):
                    tot[k] += v
            tot["objs_with_got_secsym"] += c["got_secsym_relocs"] > 0
            tot["objs_with_dup_names"] += c["dup_names"] > 0
            tot["objs_with_dup_refs"] += c["dup_refs"] > 0
            if c["dup_refs"]:
                print(f"dup-ref {p}: {c['dup_refs']} refs, {c['dup_sample']}")
        print(f"objects {n} " + " ".join(f"{k}={v}" for k, v in sorted(tot.items())))
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
