"""meta.py FILE...: the .mason.meta containers of each file: section attributes and units (offset, size, producer)."""
import sys, struct
sys.path.insert(0, "/Volumes/AgentWork/mortar-helper/.build/target-wp-154/exp/tools")
from relf import *
for p in sys.argv[1:]:
    e = load(p)
    for s in e.secs:
        if s.name != ".mason.meta":
            continue
        rel = any(r.info == s.idx and r.type in (SHT_RELA, SHT_REL) for r in e.secs)
        d = e.bytes_of(s); o = 0; units = []
        while o + 8 <= len(d):
            if d[o:o+4] == b"MASN":
                sz = struct.unpack_from("<I", d, o + 8)[0]; prod = struct.unpack_from("<H", d, o + 28)[0]
                units.append("%#x:%d/p%d" % (o, sz, prod)); o += max(8, sz)
            elif d[o:o+8] == b"\0" * 8:
                o += 8
            else:
                units.append("BAD@%#x" % o); break
        print("%s: [%d] type %d flags %#x align %d size %d group %s rel %s units %s" % (
            p, s.idx, s.type, s.flags, s.align, s.size, bool(s.flags & SHF_GROUP), rel, " ".join(units)))
