#!/bin/sh
# detect.sh WORK [MORTAR]: WP-229 item 3. R10's configurations (spec/incr.md
# §12, `dwarf_detect.sh`) rebuilt from source, extended: g++ 14, clang 19
# and 22, both 64-bit targets, plus `-gno-pubnames` and
# `-gsplit-dwarf=single`. Per object: the rule's answer (classify.py, the
# model; the Rust function's answer is printed by the update_split test on
# the same objects), the debug sections, the unit kinds, whether the CU
# names a .dwo (DW_AT_[GNU_]dwo_name), and, with MORTAR, a static link
# (AArch64 only: the native host) with --incremental=full: verify errors.
# Writes WORK/detect.tsv; the last line is `detect: done`.
set -u; ulimit -c 0
W="$1"; M="${2:-}"
D=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$W"; cd "$W" || exit 1
cat > t.h <<'E'
#pragma once
template <class T> struct Box { T v; T get() const { return v; } };
struct Pt { int x, y; int sum() const { return x + y; } };
E
cat > m.cc <<'E'
#include "t.h"
int u(int);
int main(int c, char **) { Box<int> b{c}; Pt p{c, 2}; return b.get() + p.sum() + u(c); }
E
cat > u.cc <<'E'
#include "t.h"
int u(int c) { Box<long> b{c}; Pt p{1, c}; return (int)b.get() + p.sum(); }
E
: > detect.tsv
dwo() { llvm-dwarfdump-22 --debug-info "$1" 2>/dev/null | grep -o 'DW_AT_\(GNU_\)\?dwo_name\|DW_TAG_skeleton_unit' | sort -u | tr '\n' ' '; }
run() { # tag target comp flags...
  tag=$1; tgt=$2; cc=$3; shift 3
  tf=""
  if [ "$tgt" = x64 ]; then
    case $cc in g++) cc=x86_64-linux-gnu-g++;; *) tf="--target=x86_64-linux-gnu";; esac
  fi
  d="$W/$tgt-$(echo "$tag" | tr ' =' '__')"; mkdir -p "$d"
  if ! (cd "$d" && $cc $tf "$@" -I"$W" -c "$W/m.cc" -o m.o 2>cc.err && $cc $tf "$@" -I"$W" -c "$W/u.cc" -o u.o 2>>cc.err); then
    printf '%s\t%s\tCOMPILE FAILED: %s\n' "$tgt" "$tag" "$(head -1 "$d/cc.err")" >> detect.tsv; return
  fi
  ans=$(python3 -I "$D/classify.py" "$d/m.o" | sed "s|^$d/||")
  printf '%s\t%s\t%s\tdwo-attrs: %s\tdwo-file: %s\n' "$tgt" "$tag" "$ans" "$(dwo "$d/m.o")" "$(ls "$d" | grep -c '\.dwo$')" >> detect.tsv
  if [ -n "$M" ] && [ "$tgt" = a64 ]; then
    mkdir -p "$W/bin"; ln -sf "$M" "$W/bin/ld"
    if (cd "$d" && $cc $tf -static -B"$W/bin" -Wl,--incremental=full -Wl,--gc-sections m.o u.o -o out 2>ld.err); then
      v=$(llvm-dwarfdump-22 --verify "$d/out" 2>&1 | grep -c 'error:')
      s=$(readelf -SW "$d/out" | grep -c 'debug_gnu_pub')
      printf '%s\t%s\tlink(incremental=full): ok verify-errors=%s gnu-index-sections=%s\n' "$tgt" "$tag" "$v" "$s" >> detect.tsv
    else
      printf '%s\t%s\tlink: FAILED %s\n' "$tgt" "$tag" "$(head -2 "$d/ld.err" | tr '\n' ' ')" >> detect.tsv
    fi
  fi
}
for tgt in a64 x64; do
  for comp in g++ clang++-19 clang++-22 ${MASON_KILN:-}; do
    b="-O0 -g"
    for dw in 4 5; do run "${comp##*/} dwarf$dw plain" $tgt $comp $b -gdwarf-$dw; done
    for dw in 4 5; do run "${comp##*/} dwarf$dw types" $tgt $comp $b -gdwarf-$dw -fdebug-types-section; done
    for dw in 4 5; do run "${comp##*/} dwarf$dw split" $tgt $comp $b -gdwarf-$dw -gsplit-dwarf; done
    run "${comp##*/} dwarf5 pubnames" $tgt $comp $b -gdwarf-5 -gpubnames
    run "${comp##*/} dwarf5 gnu-pubnames" $tgt $comp $b -gdwarf-5 -ggnu-pubnames
    run "${comp##*/} dwarf4 gnu-pubnames" $tgt $comp $b -gdwarf-4 -ggnu-pubnames
    # extensions (WP-229)
    for dw in 4 5; do run "${comp##*/} dwarf$dw split no-pubnames" $tgt $comp $b -gdwarf-$dw -gsplit-dwarf -gno-pubnames; done
    run "${comp##*/} dwarf5 split types" $tgt $comp $b -gdwarf-5 -gsplit-dwarf -fdebug-types-section
    case $comp in clang*|*kiln*) run "${comp##*/} dwarf5 split=single" $tgt $comp $b -gdwarf-5 -gsplit-dwarf=single;; esac
  done
done
echo "detect: done"
