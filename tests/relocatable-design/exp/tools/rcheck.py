"""rcheck.py OUT IN1 IN2 ...: check a relocatable output against its inputs (the
prototype of the structural rules WP-154 asks of the verifier).

It finds where each input section went by its bytes, with the bytes under
relocations masked. Sections are expected to be concatenated: for each output
section it takes the first matching place at or after the end of the previous
piece. Then for every input relocation of a placed section it expects one output
record at the moved offset, with the same type and the same target:
  - a global or weak symbol: the same name, the same addend;
  - a section symbol or a local symbol: the same (output section, offset),
    that is the piece's offset folded into the addend (RELA: in the record;
    REL: in the field);
  - a mergeable-string target that was merged: the same string and the same
    position inside it.
A COMDAT group whose signature an earlier input already gave is taken as
discarded when the output has fewer groups of that signature than the inputs.
Relocations naming a discarded member are listed with what the output wrote.
It prints counts and the first few mismatches of each kind. Exit status 0 when
every expected record is there and no output record is unexplained."""
import sys
import collections
sys.path.insert(0, __import__("os").path.dirname(__file__))
from relf import *  # noqa

SKIP = (SHT_SYMTAB, SHT_STRTAB, SHT_RELA, SHT_REL, SHT_GROUP, SHT_SYMTAB_SHNDX)


def maskpos(e, rels, size):
    w = 8 if e.c64 else 4
    m = set()
    for r in rels:
        m.update(range(r.off, min(r.off + w, size)))
    return m


def fits(ib, im, ob, om, off):
    """input bytes ib (mask positions im) at output offset off of ob (mask positions om)"""
    n = len(ib)
    if off + n > len(ob):
        return False
    if ob[off:off + n] == ib:
        return True
    for k in range(n):
        if ib[k] != ob[off + k] and k not in im and (off + k) not in om:
            return False
    return True


def main(argv):
    out = load(argv[1])
    ins = [load(p) for p in argv[2:]]
    orel_at = collections.defaultdict(list)   # (out sec idx, offset) -> [Rel]
    orels_by_sec = {}
    for rs, tgt, recs in out.relsecs():
        orels_by_sec[tgt.idx] = recs
        for r in recs:
            orel_at[(tgt.idx, r.off)].append(r)
    omask = {}
    for s in out.secs:
        if s.type in SKIP or s.idx == 0:
            continue
        ob = out.bytes_of(s)
        omask[s.idx] = (ob, maskpos(out, orels_by_sec.get(s.idx, []), len(ob)))
    byname = collections.defaultdict(list)
    for s in out.secs:
        if s.idx in omask:
            byname[s.name].append(s.idx)
    # groups: discarded signatures
    ogroups = collections.Counter(g[2] for g in out.groups())
    seen = collections.Counter()
    cursor = collections.defaultdict(int)
    claimed = collections.defaultdict(list)
    place = {}  # (file, sec idx) -> (out sec idx, offset) or "discarded" / None
    stats = collections.Counter()
    notes = collections.defaultdict(list)
    for fi, e in enumerate(ins):
        disc = set()
        for g, flag, sig, mem in e.groups():
            seen[sig] += 1
            if flag & 1 and seen[sig] > 1 and ogroups[sig] < seen[sig]:
                disc.update(mem)
        rels_of = {}
        for rs, tgt, recs in e.relsecs():
            rels_of[tgt.idx] = recs
        for s in e.secs:
            if s.idx == 0 or s.type in SKIP:
                continue
            if s.idx in disc:
                place[(fi, s.idx)] = "discarded"
                stats["sections discarded (group)"] += 1
                continue
            ib = e.bytes_of(s)
            im = maskpos(e, rels_of.get(s.idx, []), len(ib))
            probe = None
            for k in range(0, max(0, len(ib) - 8)):
                if not any(p in im for p in range(k, k + 8)):
                    probe = k
                    break
            cands = byname.get(s.name, []) or [i for i in omask]
            got = None
            for oi in cands:
                ob, om = omask[oi]
                al = max(1, s.align)
                if s.size == 0:
                    got = (oi, cursor[oi]) if oi in byname.get(s.name, []) else None
                    if got:
                        break
                    continue
                start = cursor[oi]
                start = (start + al - 1) // al * al
                tries = [start]
                if probe is not None:
                    pat = ib[probe:probe + 8]
                    k = ob.find(pat)
                    while k >= 0:
                        o2 = k - probe
                        if o2 >= 0 and o2 % al == 0 and o2 != start:
                            tries.append(o2)
                        k = ob.find(pat, k + 1)
                    tries = [start] + sorted(t for t in tries[1:] if t >= start) + sorted(t for t in tries[1:] if t < start)
                else:
                    tries = list(range(start, len(ob) - s.size + 1, al)) + list(range(0, min(start, len(ob) - s.size + 1), al))
                for off in tries:
                    if fits(ib, im, ob, om, off) and not any(a < off + s.size and off < b for a, b in claimed[oi]):
                        got = (oi, off)
                        break
                if got:
                    break
            place[(fi, s.idx)] = got
            if got:
                oi, off = got
                claimed[oi].append((off, off + s.size))
                cursor[oi] = max(cursor[oi], off + s.size)
                stats["sections placed"] += 1
                if out.secs[oi].name != s.name:
                    notes["renamed"].append("%s:%s -> %s" % (e.name, s.name, out.secs[oi].name))
            else:
                stats["sections not found (merged or rewritten)"] += 1
                notes["not found"].append("%s:%s (%d bytes, flags %#x)" % (e.name, s.name, s.size, s.flags))
    used = set()

    def ocanon(r):
        y = out.syms[r.sym] if r.sym < len(out.syms) else None
        if y is None or r.sym == 0:
            return ("none", r.addend)
        if y.type == 3 or (y.bind == 0 and y.shndx not in (SHN_UNDEF, SHN_ABS, SHN_COMMON)):
            return ("sec", y.shndx, y.value + r.addend)
        return ("name", y.name, r.addend)

    def strat(e, sidx, off):
        s = e.secs[sidx]
        d = e.bytes_of(s)
        a = off
        while a > 0 and d[a - 1] != 0:
            a -= 1
        b = d.find(b"\0", off)
        return (d[a:b], off - a)

    for fi, e in enumerate(ins):
        for rs, tgt, recs in e.relsecs():
            p = place.get((fi, tgt.idx))
            if p in (None, "discarded"):
                continue
            oi, base = p
            for r in recs:
                stats["input records"] += 1
                y = e.syms[r.sym]
                if r.sym == 0:
                    want = ("none", r.addend)
                elif y.type == 3 or (y.bind == 0 and y.shndx not in (SHN_UNDEF, SHN_ABS, SHN_COMMON)):
                    tp = place.get((fi, y.shndx))
                    if tp == "discarded":
                        want = ("discarded", e.secs[y.shndx].name)
                    elif tp is None:
                        ts = e.secs[y.shndx] if y.shndx < len(e.secs) else None
                        if ts is not None and ts.flags & SHF_MERGE and ts.flags & SHF_STRINGS:
                            want = ("str",) + strat(e, y.shndx, y.value + r.addend)
                        else:
                            want = ("unplaced", ts.name if ts else y.shndx)
                    else:
                        want = ("sec", tp[0], tp[1] + y.value + r.addend)
                else:
                    want = ("name", y.name, r.addend)
                cand = orel_at.get((oi, base + r.off), [])
                cand = [c for c in cand if c.type == r.type and id(c) not in used]
                if not cand:
                    stats["MISSING output record"] += 1
                    notes["missing"].append("%s:%s+%#x type %d want %s" % (e.name, tgt.name, r.off, r.type, want))
                    continue
                c = cand[0]
                used.add(id(c))
                have = ocanon(c)
                if want[0] == "discarded":
                    stats["records naming a discarded copy"] += 1
                    ys = out.syms[c.sym] if c.sym < len(out.syms) else None
                    notes["to discarded"].append("%s:%s+%#x -> out sym %r (%s) addend %d" % (
                        e.name, tgt.name, r.off, ys.name if ys else None,
                        {0: "UNDEF"}.get(ys.shndx, out.secs[ys.shndx].name if ys and ys.shndx < len(out.secs) else ys.shndx) if ys else "-", c.addend))
                    continue
                if want[0] == "str":
                    ok = have[0] == "sec" and strat(out, have[1], have[2]) == want[1:]
                    stats["records into merged strings: %s" % ("same string" if ok else "WRONG")] += 1
                    continue
                if want[0] == "unplaced":
                    stats["records to an unplaced section (not judged)"] += 1
                    continue
                if have == want:
                    stats["records equal"] += 1
                    if want[0] == "sec" and c.sym != 0:
                        k = "section symbol" if out.syms[c.sym].type == 3 else "local symbol"
                        stats["  local targets written against a %s" % k] += 1
                else:
                    stats["RECORD DIFFERS"] += 1
                    notes["differs"].append("%s:%s+%#x type %d want %s have %s" % (e.name, tgt.name, r.off, r.type, want, have))
    extra = sum(1 for k, v in orel_at.items() for c in v if id(c) not in used)
    stats["output records unexplained"] = extra
    for k in sorted(stats):
        print("  %-55s %d" % (k, stats[k]))
    for k in notes:
        for n in notes[k][:6]:
            print("  [%s] %s" % (k, n))
        if len(notes[k]) > 6:
            print("  [%s] ... %d more" % (k, len(notes[k]) - 6))
    bad = stats["MISSING output record"] + stats["RECORD DIFFERS"] + extra + stats.get("records into merged strings: WRONG", 0)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
