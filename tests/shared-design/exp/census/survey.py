#!/usr/bin/env python3
"""Classify the real-builds survey's -shared lines (kind == shared).
Run as: python3 -I -B survey.py RESULTS_DIR   (prints a TSV and a summary)"""
import collections, glob, os, re, sys

R = sys.argv[1]
DRIVER_OPTS = {'-plugin', '--build-id', '--eh-frame-hdr', '--hash-style=gnu', '--as-needed', '-shared',
               '-X', '-EL', '-maarch64linux', '--fix-cortex-a53-843419', '-m', '--push-state',
               '--pop-state', '--no-as-needed', '-o'}
rows = []
for f in sorted(glob.glob(os.path.join(R, '*', 'lines'))):
    setname = os.path.basename(os.path.dirname(f))
    proj, comp, mode = setname.split('.')
    for line in open(f):
        p = line.rstrip('\n').split('\t')
        if len(p) < 5 or p[1] != 'shared':
            continue
        lid, kind, outcome, d, argv = p[0], p[1], p[2], p[3], p[4:]
        if len(argv) == 1:
            argv = argv[0].split(' ')
        rows.append((proj, comp, mode, lid, outcome, argv))

print('project\tcompiler\tline\tprobe\tcontrol_outcome\tmortar_outcome\tlang\tinputs\textra_options\tl_tail\tsoname\tversion_script')
summary = collections.Counter()
ctl = {(r[0], r[1], re.sub(r'\d{5,}', 'N', r[3])): r for r in rows if r[2] == 'control'}
mor = {(r[0], r[1], re.sub(r'\d{5,}', 'N', r[3])): r for r in rows if r[2] == 'mortar'}
for key in sorted(set(ctl) | set(mor)):
    r = ctl.get(key) or mor[key]
    argv = r[5]
    i, extra, ltail, inputs = 0, [], [], []
    soname = vs = '-'
    while i < len(argv):
        a = argv[i]
        if a in ('-plugin', '-m', '-o', '-soname', '--version-script', '-z', '-rpath'):
            v = argv[i + 1] if i + 1 < len(argv) else ''
            if a == '-soname':
                soname = v; extra.append('-soname')
            elif a == '--version-script':
                vs = v; extra.append('--version-script')
            elif a in ('-z', '-rpath'):
                extra.append(a + ' ' + v)
            i += 2; continue
        if a.startswith('-plugin-opt') or a.startswith('-L'):
            i += 1; continue
        if a.startswith('-l'):
            ltail.append(a)
        elif a.startswith('-'):
            if a not in DRIVER_OPTS:
                extra.append(a.split('=')[0] + ('=…' if '=' in a else ''))
        else:
            b = os.path.basename(a)
            if b not in ('crti.o', 'crtn.o', 'crtbeginS.o', 'crtendS.o'):
                inputs.append(b)
        i += 1
    lang = 'C++' if '-lstdc++' in ltail else 'C'
    probe = 'probe' if r[3].startswith('configure/') else 'build'
    m = mor.get(key)
    c = ctl.get(key)
    print('\t'.join([r[0], r[1], r[3], probe, c[4] if c else '-', m[4] if m else '-', lang,
                     f'{len(inputs)} ({" ".join(inputs[:4])}{" …" if len(inputs) > 4 else ""})',
                     ' '.join(extra) or '-', ' '.join(ltail), soname, vs]))
    summary[(probe, lang, ' '.join(sorted(set(extra))) or '-')] += 1
print()
for (probe, lang, ex), n in sorted(summary.items()):
    print(f'{n}\t{probe}\t{lang}\textra options: {ex}')
