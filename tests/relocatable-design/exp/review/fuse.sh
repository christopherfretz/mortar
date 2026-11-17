#!/bin/bash
# review/fuse.sh WORK: the linker line of `-r -fuse-ld=lld|mold` per driver, from the
# driver's -### output (census/run.sh's ld wrapper did not see these: m9), compared with
# the plain -r line after the linker's name.
set -u; ulimit -c 0
W=$(realpath -m "$1"); mkdir -p "$W"; cd "$W" || exit 1
echo 'int f(void){return 1;}' > x.c
for D in gcc g++ x86_64-linux-gnu-gcc i686-linux-gnu-gcc clang-22 "clang-22 --target=x86_64-linux-gnu" "clang-22 --target=i686-linux-gnu"; do
  line() { $D "$@" -### -o out.o x.c 2>&1 | grep -E '(collect2|ld|lld|mold)"? ' | grep -v '"-cc1"' | tail -1 | sed -E 's/^ *"?[^ "]*(collect2|ld\.lld|ld\.mold|ld\.bfd|ld|mold|lld)"? /LINKER /; s/"//g; s|/tmp/[^ ]*|TMP|g'; }
  base=$(line -r); for u in lld mold; do l=$(line -r -fuse-ld=$u); [ "$l" = "$base" ] && s=same || s="DIFFERS: $l"; echo "$D -r -fuse-ld=$u: $s"; done; echo "$D -r: $base" | cut -c1-260; done
echo "-- gcc: the words of the -fuse-ld line that the plain -r line lacks (collect2 consumes -fuse-ld to pick the linker)"
for u in lld mold; do gcc -r -fuse-ld=$u -### -o out.o x.c 2>&1 | grep collect2 | tr ' ' '\n' | grep -vx -F -f <(gcc -r -### -o out.o x.c 2>&1 | grep collect2 | tr ' ' '\n') | grep -v -- -fresolution | tr '\n' ' '; echo; done
echo "fuse done"
