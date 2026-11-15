#!/bin/bash
# Run from an empty scratch directory (it writes there). spec/dyn.md D-12 has the table (WP-043).
# Q1: does an undefined reference of a loaded command-line shared object extract an archive
# member, and is it the same when the object is --as-needed and ends up not needed?
set -u; ulimit -c 0
W=$(pwd)
for a in aarch64 x86_64; do
  if [ $a = aarch64 ]; then CC=gcc; R="env LD_LIBRARY_PATH=$W/$a"; else CC=x86_64-linux-gnu-gcc; R="qemu-x86_64 -L /usr/x86_64-linux-gnu -E LD_LIBRARY_PATH=$W/$a"; fi
  mkdir -p $a; cd $a
  echo 'int g(void); int u_fn(void){return g()+40;}' > u.c
  echo '#include <stdio.h>
int g(void){return 2;} __attribute__((constructor)) static void c(void){puts("g.o in");}' > g.c
  echo '#include <stdio.h>
int main(void){puts("main");return 0;}' > m0.c
  echo '#include <stdio.h>
int u_fn(void); int main(void){printf("%d\n",u_fn());return 0;}' > m1.c
  $CC -fPIC -shared -Wl,-soname,libu.so -o libu.so u.c
  $CC -O1 -c g.c -o g.o; rm -f libg.a; ar rcs libg.a g.o
  $CC -O1 -c m0.c -o m0.o; $CC -O1 -c m1.c -o m1.o
  for m in m0 m1; do for an in as-needed no-as-needed; do for ord in "libu.so libg.a" "libg.a libu.so"; do
   for l in bfd lld mold; do o=o.$m.$an.${ord// /_}.$l.$RANDOM
    line="$m.o -Wl,--$an -L. $ord"
    line=${line//libu.so/-lu}; line=${line//libg.a/-lg}
    if $CC -fuse-ld=$l $line -o $o > $o.err 2>&1; then
      ext=$(nm $o | grep -c ' T g$'); dyn=$(readelf -W --dyn-syms $o | awk '$8=="g"' | wc -l)
      echo "$a [$line] $l: NEEDED=$(readelf -dW $o | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' | paste -sd,) g_extracted=$ext g_in_dynsym=$dyn run=$($R ./$o 2>&1 | paste -sd'|')"
    else echo "$a [$line] $l: REFUSED $(head -2 $o.err | paste -sd'|')"; fi
   done; done; done; done
  cd ..
done
