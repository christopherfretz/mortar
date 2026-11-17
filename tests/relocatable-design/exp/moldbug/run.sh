#!/bin/bash
# moldbug/run.sh WORK: mold 2.37.1 -r turns every common symbol into SHN_ABS
# (value = its alignment), and crashes (SIGSEGV) on an IFUNC definition.
# The final program then reads address 4; GNU ld and mortar alike link it.
set -u; ulimit -c 0
WORK=$(realpath -m "$1"); M=${2:-/Volumes/AgentWork/mortar-helper/.build/target-wp-154/mortar-3ad9867a}
mkdir -p "$WORK/mbin"; cd "$WORK" || exit 1; ln -sf "$M" mbin/ld
printf 'int cx;\nint get(void) { return cx; }\n' > c.c
printf 'extern int cx; int get(void);\nint main(void) { cx = 5; return get() == 5 ? 0 : 1; }\n' > m.c
printf 'static int a(void){return 1;}\nstatic int (*r(void))(void){return a;}\nint f(void) __attribute__((ifunc("r")));\n' > i.c
gcc -fcommon -c c.c -o c.o; gcc -c m.c -o m.o; gcc -c i.c -o i.o
for k in bfd mold; do
  case $k in bfd) ld.bfd -r -o r.$k.o c.o;; mold) mold -r -o r.$k.o c.o;; esac
  echo "-r by $k: $(readelf -sW r.$k.o | awk '$NF=="cx"{print $7, "value", $2, "size", $3}')"
  for fin in bfd mortar; do
    ff=""; [ $fin = mortar ] && ff=-B$WORK/mbin/
    gcc -static $ff m.o r.$k.o -o p.$k.$fin && { ./p.$k.$fin; echo "   final $fin -static: rc=$?"; }
  done
done
mold -r -o ri.o i.o; echo "mold -r of an IFUNC definition: rc=$?"
