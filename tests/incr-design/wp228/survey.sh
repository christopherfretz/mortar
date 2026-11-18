#!/bin/bash
# survey.sh OUTDIR: item 1's measurement. Compiles src/{a,b,c}.cc with GCC
# 14, Clang 19, Clang 22 and kiln++ (hod back end) for AArch64 and x86-64,
# DWARF 4 and 5, under five flag sets, and lists every debug section of
# each object with its relocations by target class (elfdbg.py survey).
# One TSV line per (config, object, section, target class) in
# OUTDIR/survey.tsv; compile failures in OUTDIR/survey.err.
set -u
ulimit -c 0
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="$1"
mkdir -p "$OUT/survey"
cd "$OUT/survey" || exit 1
cp "$HERE"/src/*.cc "$HERE"/src/h.h .
: > ../survey.tsv
: > ../survey.err
KILN="${MASON_KILN:-}"
HOD="${MASON_HOD:-}"
cc_of() { # compiler-target -> command prefix
  case $1 in
    gcc14-a64) echo "g++";;
    gcc14-x64) echo "x86_64-linux-gnu-g++";;
    clang19-a64) echo "clang++-19 --target=aarch64-linux-gnu";;
    clang19-x64) echo "clang++-19 --target=x86_64-linux-gnu";;
    clang22-a64) echo "clang++-22 --target=aarch64-linux-gnu";;
    clang22-x64) echo "clang++-22 --target=x86_64-linux-gnu";;
    kiln-a64) echo "env KILN_HOD=$HOD $KILN -fkiln-backend=hod --target=aarch64-linux-gnu";;
    kiln-x64) echo "env KILN_HOD=$HOD $KILN -fkiln-backend=hod --target=x86_64-linux-gnu";;
  esac
}
for c in gcc14-a64 gcc14-x64 clang19-a64 clang19-x64 clang22-a64 clang22-x64 kiln-a64 kiln-x64; do
  cc=$(cc_of $c)
  for dw in 4 5; do
    for fs in base O2 types macro pubnames frame; do
      case $fs in
        base) F="-O0 -g -gdwarf-$dw -ffunction-sections -fdata-sections";;
        O2) F="-O2 -g -gdwarf-$dw";;
        types) F="-O0 -g -gdwarf-$dw -fdebug-types-section";;
        macro) F="-O0 -g3 -gdwarf-$dw"; case $c in clang*|kiln*) F="$F -fdebug-macro";; esac;;
        pubnames) F="-O0 -g -gdwarf-$dw -gpubnames";;
        frame) F="-O2 -g -gdwarf-$dw -fno-exceptions -fno-asynchronous-unwind-tables -fno-unwind-tables";;
      esac
      tag="$c-dw$dw-$fs"
      for s in a b c; do
        if ! $cc $F -c $s.cc -o $tag-$s.o 2>> ../survey.err; then
          echo "FAIL $tag $s.cc" >> ../survey.err
          continue
        fi
        python3 -I "$HERE/elfdbg.py" survey $tag-$s.o | sed "s/^/$tag\t/" >> ../survey.tsv
      done
    done
  done
done
echo "SURVEY-DONE $(wc -l < ../survey.tsv) lines, $(grep -c '^FAIL' ../survey.err) failures"
