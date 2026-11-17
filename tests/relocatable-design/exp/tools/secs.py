"""secs.py FILE [REGEX]: sections as name(type,flags,align,size), in table order."""
import sys, re, os
sys.path.insert(0, os.path.dirname(__file__))
from relf import load
T = {0: "NULL", 1: "PROGBITS", 2: "SYMTAB", 3: "STRTAB", 4: "RELA", 7: "NOTE", 8: "NOBITS", 9: "REL", 14: "INIT_ARRAY", 15: "FINI_ARRAY", 17: "GROUP", 0x6fff4c00: "LLVM_ADDRSIG", 0x40000014: "CREL", 0x70000003: "AARCH64_ATTRIBUTES"}
F = [(1, "W"), (2, "A"), (4, "X"), (0x10, "M"), (0x20, "S"), (0x40, "I"), (0x80, "L"), (0x200, "G"), (0x400, "T"), (0x800, "C"), (0x200000, "R"), (0x80000000, "E")]
e = load(sys.argv[1]); pat = re.compile(sys.argv[2] if len(sys.argv) > 2 else ".")
out = []
for s in e.secs[1:]:
    if pat.search(s.name):
        out.append("%s(%s,%s,%d,%d)" % (s.name, T.get(s.type, hex(s.type)), "".join(c for b, c in F if s.flags & b), s.align, s.size))
print(" ".join(out), end="")
