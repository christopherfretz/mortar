# m1_extract.py DIR...: the fixture of WP-216's edit_moves_only_changed_sections
# (spec/incr.md IN-72 item 2's model series, tests/incr-design/model_region.py)
# from review-in72's protobuf 3.21.12 builds: for each DIR (o0g, o2fs) the
# files before strutil.o in the first link that have groups or alloc sections
# (main.o) and the five strutil versions. Per object: every SHF_ALLOC section
# (index, name, type, flags, size, alignment, and its anchor of model.md
# §7.1 as model_region.py's identity_sections takes it: comdat:SIG,
# global:NAME, local:NAME or none:) and
# every defined GLOBAL/WEAK FUNC/OBJECT symbol (name, section index, value).
# Run: python3 m1_extract.py .build/review-in72/m/pb/o0g .build/review-in72/m/pb/o2fs > m1.txt
import sys, subprocess, re, os
def sh(*a): return subprocess.run(a, capture_output=True, text=True, check=True).stdout
FL = {'W': 0x1, 'A': 0x2, 'X': 0x4, 'M': 0x10, 'S': 0x20, 'I': 0x40, 'L': 0x80, 'G': 0x200, 'T': 0x400}
TY = {'PROGBITS': 1, 'NOBITS': 8, 'NOTE': 7, 'INIT_ARRAY': 14, 'FINI_ARRAY': 15, 'PREINIT_ARRAY': 16}
for d in sys.argv[1:]:
    print(f'dir {os.path.basename(d)}')
    for obj in ['main', 'strutil_base', 'strutil_grow1', 'strutil_grow2', 'strutil_rmfn', 'strutil_addfn']:
        path = f'{d}/{obj}.o'
        grp = {}; cur = None
        for l in sh('readelf', '-gW', path).splitlines():
            m = re.match(r"COMDAT group section \[\s*\d+\] `\.group' \[(\S+)\]", l)
            if m: cur = m[1]; continue
            m = re.match(r'\s+\[\s*(\d+)\]', l)
            if m and cur: grp[int(m[1])] = cur
        syms = []
        for l in sh('readelf', '-sW', path).splitlines():
            p = l.split()
            if len(p) >= 8 and p[0].endswith(':') and p[6].isdigit():
                syms.append(dict(value=int(p[1], 16), type=p[3], bind=p[4], shndx=int(p[6]), name=p[7]))
        def anchor(i):
            if i in grp: return f'comdat:{grp[i]}'
            at0 = [y for y in syms if y['shndx'] == i and y['value'] == 0 and y['type'] in ('FUNC', 'OBJECT', 'NOTYPE') and y['name'] and not y['name'].startswith('$')]
            g = [y['name'] for y in at0 if y['bind'] in ('GLOBAL', 'WEAK')]
            lo = [y['name'] for y in at0 if y['bind'] == 'LOCAL']
            return f'global:{g[0]}' if g else f'local:{lo[0]}' if lo else 'none:'
        print(f'file {obj}')
        for l in sh('readelf', '-SW', path).splitlines():
            m = re.match(r'\s*\[\s*(\d+)\]\s+(\S+)\s+(\S+)\s+([0-9a-f]+)\s+([0-9a-f]+)\s+([0-9a-f]+)\s+([0-9a-f]+)\s+(\S*)\s+(\d+)\s+(\d+)\s+(\d+)', l)
            if not m or 'A' not in m[8]: continue
            flags = sum(FL.get(ch, 0) for ch in m[8])
            ty = TY.get(m[3])
            if ty is None: continue
            print(f'sec {m[1]} {m[2]} {ty} {flags:#x} {int(m[6], 16)} {max(1, int(m[11]))} {anchor(int(m[1]))}')
        for y in syms:
            if y['bind'] in ('GLOBAL', 'WEAK') and y['type'] in ('FUNC', 'OBJECT'):
                print(f"sym {y['name']} {y['shndx']} {y['value']}")
