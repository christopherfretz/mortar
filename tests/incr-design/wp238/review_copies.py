#!/usr/bin/env python3
"""review_copies.py OUT -- WP-238 step 2, Part 0.

Writes into OUT copies of the reviewer's scenario scripts
(tests/incr-review/wp238: roots.sh, fdeglobal.sh, skip.sh, shapes.sh and
lib.sh) that also run oracle_design.py (single links and the update,
both ways), and that remove nothing: each probe link's previous dump
directory is renamed aside instead of emptied, and the `rm -f` of the
linked program is dropped (the step's rule: no rm of a variable path).
newscen.sh (F6's inputs and the review's open questions) sources OUT's
lib.sh. Usage: `bash OUT/<script> PROBE W` with W a fresh directory.
"""
import os, re, sys

here = os.path.dirname(os.path.abspath(__file__))
src = os.path.join(here, '..', '..', 'incr-review', 'wp238')
dst = os.path.abspath(sys.argv[1])
os.makedirs(dst, exist_ok=True)
lib = open(os.path.join(src, 'lib.sh')).read()
lib = lib.replace('here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)', 'here=%s' % os.path.abspath(src))
lib = lib.replace('design=$here/../../incr-design/wp238', 'design=%s' % here)
lib = lib.replace('  mkdir -p "$d/dump"\n  find "$d/dump" -mindepth 1 -delete\n',
                  '  [ -d "$d/dump" ] && mv "$d/dump" "$d/dump.$(date +%s%N)"\n  mkdir -p "$d/dump"\n')
lib = re.sub(r'emu\(\) \{.*?\n\}\n', '''emu() {
  local o=$1 n=$2; shift 2
  echo "  emulate_design.py: $(python3 -I "$here/emulate_design.py" "$o" "$n" "$@" | tail -1 | cut -c1-300)"
  echo "  ORACLE single old: $(python3 -I "$design/oracle_design.py" "$o" | tail -1 | cut -c1-500)"
  echo "  ORACLE single new: $(python3 -I "$design/oracle_design.py" "$n" | tail -1 | cut -c1-500)"
  echo "  ORACLE update:     $(python3 -I "$design/oracle_design.py" "$o" "$n" | tail -1 | cut -c1-700)"
}
''', lib, flags=re.S)
assert 'ORACLE' in lib and 'mv "$d/dump"' in lib
open(os.path.join(dst, 'lib.sh'), 'w').write(lib)
for f in ['roots.sh', 'fdeglobal.sh', 'skip.sh', 'shapes.sh']:
    s = open(os.path.join(src, f)).read()
    s = s.replace('source "$(cd "$(dirname "$0")" && pwd)/lib.sh"', 'source %s/lib.sh' % dst)
    s = re.sub(r'^(\s*)rm -f [^\n]*$', r'\1:', s, flags=re.M)
    assert 'rm ' not in s, f
    open(os.path.join(dst, f), 'w').write(s)
n = open(os.path.join(here, 'newscen.sh')).read()
open(os.path.join(dst, 'newscen.sh'), 'w').write(n.replace('@LIB@', os.path.join(dst, 'lib.sh')))
print('wrote', dst)
