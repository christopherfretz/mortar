#!/usr/bin/python3 -I
# Q10 / D-126 item 2: which system shared libraries are DT_SYMBOLIC/DF_SYMBOLIC and export
# data (what a COPY in a -no-pie or x86-64 GCC PIE program would hit), and which export
# STV_PROTECTED data. Usage: scan.py DIR...
import os, subprocess, sys, re
def libs(d):
    for root, _, fs in os.walk(d):
        for f in fs:
            p = os.path.join(root, f)
            if '.so' in f and os.path.isfile(p) and not os.path.islink(p):
                with open(p, 'rb') as fh:
                    if fh.read(4) == b'\x7fELF': yield p
tot = sym = 0
for d in sys.argv[1:]:
    for p in sorted(libs(d)):
        dyn = subprocess.run(['readelf', '-dW', p], capture_output=True, text=True).stdout
        if 'SHARED' not in subprocess.run(['readelf', '-hW', p], capture_output=True, text=True).stdout and 'DYN' not in subprocess.run(['readelf', '-hW', p], capture_output=True, text=True).stdout: continue
        tot += 1
        s = bool(re.search(r'\(SYMBOLIC\)', dyn)) or bool(re.search(r'\(FLAGS\).*SYMBOLIC', dyn))
        ds = subprocess.run(['readelf', '--dyn-syms', '-W', p], capture_output=True, text=True).stdout
        objs = []; prot = []
        for l in ds.splitlines():
            f = l.split()
            if len(f) >= 8 and f[3] in ('OBJECT', 'TLS') and f[6] != 'UND' and f[4] in ('GLOBAL', 'WEAK', 'UNIQUE'):
                if f[3] == 'OBJECT': objs.append(f[7])
                if f[5] == 'PROTECTED': prot.append(f[7])
        if s:
            sym += 1
            print(f'SYMBOLIC {p}: {len(objs)} exported objects: {" ".join(objs[:8])}{" ..." if len(objs)>8 else ""}')
        if prot:
            print(f'PROTECTED-DATA {p}: {" ".join(prot[:8])}')
print(f'# {tot} shared objects scanned, {sym} symbolic')
