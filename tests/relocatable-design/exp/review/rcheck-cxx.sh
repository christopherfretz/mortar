#!/bin/bash
# review/rcheck-cxx.sh FINALWORK: produces work/rcheck-cxx.log (record B.4) from final/run.sh's
# work directory: rcheck.py over each reference's -r output of the C++ component, per target.
set -u; H=$(cd "$(dirname "$0")/../tools" && pwd); cd "$1" || exit 1
for d in aarch64.gcc x86_64.gcc i686.gcc; do for r in bfd lld mold; do echo "== $d $r"
  python3 -I "$H/rcheck.py" $d/comp.cxx.$r.o $d/x1.o $d/x2.o 2>&1 | grep -vE '^\s+\['; done; done
echo "rcheck-cxx done"
