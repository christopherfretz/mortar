#!/usr/bin/env python3
"""forms_tally.py LOG...: WP-228 part A. Per neutral form (a section that
place.py wrote in neutral form in an output, the address-base unit by
DWARF version, the v4 forms), per target: the outputs holding it and
how many of them each tool judged equal to the full link (gdb, lldb,
llvm-dwarfdump --verify, readelf), with the causes of the differences
(tally.py's classes). The form of each output is read from place.py's
report next to the log (`<tag>/<output>.place`)."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from tally import cause  # noqa: E402

CHECKS = ('gdb', 'lldb', 'verify', 'readelf')


def forms_of(d, tag, x):
    p = os.path.join(d, tag, x + '.place')
    if not os.path.exists(p):
        return None
    fs = set()
    for ln in open(p):
        w = ln.split()
        if w and w[0].startswith('.debug_') and w[0] != '.debug_aranges':
            fs.add(w[0])
    dw = '5' if '-dw5' in tag else '4'
    if x.endswith('addrbase') and '.debug_addr' in fs:
        fs.add('.debug_info+addr_base v' + dw)
    if x.endswith('v4'):
        fs = {f + ' (v4 form)' if f in ('.debug_info', '.debug_line', '.debug_macro') else f for f in fs}
    if '-split-' in tag and '.debug_info' in ' '.join(fs):
        fs.add('skeleton unit v' + dw)
    if '-split-' in tag and dw == '4' and '.debug_addr' in fs:
        fs.discard('.debug_addr')
        fs.add('.debug_addr (GNU, no header)')
    return fs


def main():
    res = {}  # (form, arch) -> {check: [same, total, {cause: n}]}
    for log in sys.argv[1:]:
        d = os.path.dirname(log)
        per = {}
        for ln in open(log):
            w = ln.split(None, 4)
            if len(w) < 4 or w[2] not in CHECKS or w[3] not in ('SAME', 'DIFF'):
                continue
            per.setdefault((w[0], w[1]), []).append((w[2], w[3], w[4] if len(w) > 4 else ''))
        for (tag, x), checks in per.items():
            fs = forms_of(d, tag, x)
            if fs is None:
                continue
            arch = 'x86-64' if '-x64' in tag else 'AArch64'
            for f in fs:
                r = res.setdefault((f, arch), {})
                for c, v, det in checks:
                    e = r.setdefault(c, [0, 0, {}])
                    e[1] += 1
                    if v == 'SAME':
                        e[0] += 1
                    else:
                        k = cause(c, det)
                        e[2][k] = e[2].get(k, 0) + 1
    print('| form | target | outputs | gdb | lldb | verify | readelf | differences (cause: checks) |')
    print('|---|---|---|---|---|---|---|---|')
    for (f, arch) in sorted(res):
        r = res[(f, arch)]
        n = max(e[1] for e in r.values())
        cells = []
        why = {}
        for c in CHECKS:
            e = r.get(c)
            cells.append('%d/%d' % (e[0], e[1]) if e else '-')
            if e:
                for k, v in e[2].items():
                    why['%s %s' % (c, k)] = v
        print('| `%s` | %s | %d | %s | %s |' % (f, arch, n, ' | '.join(cells),
                                               ', '.join('%s: %d' % kv for kv in sorted(why.items())) or '-'))


if __name__ == '__main__':
    main()
