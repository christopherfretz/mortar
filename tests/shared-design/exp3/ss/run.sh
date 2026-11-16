#!/bin/sh
# ss/run.sh: -shared -static (m-4). The gcc driver's line; a trivial -fPIC object (s.c) and one
# calling printf (p.c) linked by gcc -shared -static with GNU ld, lld and mold (fresh outputs r-*).
ulimit -c 0
cd /Volumes/AgentWork/mortar-helper/.build/target-wp-110/exp3/ss || exit 1
gcc -fPIC -c s.c -o s.o; gcc -fPIC -c p.c -o p.o
echo "DRIVER: $(gcc -### -shared -static s.o -o r-x.so 2>&1 | grep collect2 | tr ' ' '\n' | grep -E '^"?-(Bstatic|shared|static|lgcc|lgcc_eh|lc)"?$|crtbeginT|start-group|end-group' | tr '\n' ' ')"
for l in bfd lld mold; do
  for o in s p; do
    gcc -fuse-ld=$l -shared -static $o.o -o r-$l-$o.so > r-$l-$o.err 2>&1; rc=$?
    echo "$l $o rc=$rc $(grep -v collect2 r-$l-$o.err | head -1)"
  done
done
echo "LD-STATIC-EQ-BSTATIC: $(ld --help | grep -E '^ +-Bstatic' | head -1)"
echo SS-DONE
