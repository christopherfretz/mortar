#!/usr/bin/env python3
"""tally.py LOG...: counts emulate.sh's checks (readelf, verify, gdb,
lldb, plumb; raw-verify and place separately) per log, and sorts every
DIFF into a cause:
  addr-unnamed     readelf "Corrupt .debug_addr" (a dead table no unit
                   names: the variants without --addr-base)
  loc-zeros        readelf "hole"/"unused bytes" in .debug_loc (the
                   proposed zero form)
  loclists-tail    readelf "unused bytes at the end of .debug_loclists"
                   (a dead table last in the section)
  no-spec-form     plumb P-I2/P-I6: a section IN-51b gives no neutral
                   form (.debug_loc, .debug_macro, .debug_types,
                   .debug_macinfo)
  comdat-keeper    plumb M10: a group whose keeper was the old file
  uninit-local     lldb/gdb: only an uninitialized local's value differs
  other            anything else (listed)
"""
import re
import sys


def cause(check, detail):
    if check == 'readelf':
        ws = re.findall(r'[<>] +\d+ (readelf: Warning: [^|]*)', detail)
        kinds = set()
        for w in ws:
            if 'Corrupt .debug_addr' in w:
                kinds.add('addr-unnamed')
            elif '.debug_loclists' in w:
                kinds.add('loclists-tail')
            elif '.debug_loc' in w:
                kinds.add('loc-zeros')
            else:
                kinds.add('other')
        return '+'.join(sorted(kinds)) or 'other'
    if check == 'plumb':
        errs = [e for e in detail.split('|') if 'verifier rule' in e]
        kinds = set()
        for e in errs:
            if re.search(r"rule P-I[26]: .*\.debug_(loc|macro|types|macinfo)\b", e):
                kinds.add('no-spec-form')
            elif 'rule M10' in e and 'discarded' in e:
                kinds.add('comdat-keeper')
            else:
                kinds.add('other')
        return '+'.join(sorted(kinds)) or 'other'
    if check in ('gdb', 'lldb'):
        lines = [x for x in detail.split('|') if x.startswith(('<', '>'))]
        if lines and all(re.match(r'[<>] (\(\w+\) )?(s|r|i) = ', x) for x in lines):
            return 'uninit-local'
        return 'other'
    return 'other'


def main():
    for log in sys.argv[1:]:
        tot = {}
        causes = {}
        others = []
        for ln in open(log):
            w = ln.rstrip('\n').split(' ', 4)
            if len(w) < 4 or w[3] not in ('SAME', 'DIFF'):
                continue
            check, res = w[2], w[3]
            t = tot.setdefault(check, [0, 0])
            t[0 if res == 'SAME' else 1] += 1
            if res == 'DIFF' and check not in ('raw-verify',):
                c = cause(check, w[4] if len(w) > 4 else '')
                causes[(check, c)] = causes.get((check, c), 0) + 1
                if 'other' in c:
                    others.append(ln.strip()[:300])
        print('== %s' % log)
        print('  ' + '  '.join('%s %d/%d' % (k, v[0], v[0] + v[1]) for k, v in sorted(tot.items())))
        for (k, c), n in sorted(causes.items()):
            print('  DIFF %s %s %d' % (k, c, n))
        for o in others[:6]:
            print('  OTHER ' + o)


if __name__ == '__main__':
    main()
