#!/bin/bash
# Run from an empty scratch directory (it writes there). spec/dyn.md D-12 has the table (WP-043).
# Q1 variant: cyclic support. libu (as-needed) references g; g.o (in libg.a) references u2 defined by libu.
set -u; ulimit -c 0; W=$(pwd)
for a in aarch64 x86_64; do
  if [ $a = aarch64 ]; then CC=gcc; R="env LD_LIBRARY_PATH=$W/c$a"; else CC=x86_64-linux-gnu-gcc; R="qemu-x86_64 -L /usr/x86_64-linux-gnu -E LD_LIBRARY_PATH=$W/c$a"; fi
  mkdir -p c$a; cd c$a
  echo 'int g(void); int u_fn(void){return g()+40;} int u2(void){return 5;}' > u.c
  echo '#include <stdio.h>
int u2(void); int g(void){return u2();} __attribute__((constructor)) static void c(void){printf("g.o in %d\n", u2());}' > g.c
  echo '#include <stdio.h>
int main(void){puts("main");return 0;}' > m0.c
  echo '#include <stdio.h>
int g(void) __attribute__((weak)); int main(void){printf("main %d\n", g ? g() : -1);return 0;}' > mw.c
  $CC -fPIC -shared -Wl,-soname,libu.so -o libu.so u.c
  $CC -O1 -c g.c -o g.o; rm -f libg.a; ar rcs libg.a g.o
  $CC -O1 -c m0.c -o m0.o; $CC -O1 -c mw.c -o mw.o
  for m in m0 mw; do for ord in "-lu -lg" "-lg -lu"; do
   for l in bfd lld mold; do o=o.$m.${ord// /_}.$l.$RANDOM
    line="$m.o -Wl,--as-needed -L. $ord"
    if $CC -fuse-ld=$l $line -o $o > $o.err 2>&1; then
      ext=$(nm $o | grep -c ' T g$')
      echo "$a [$line] $l: NEEDED=$(readelf -dW $o | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' | paste -sd,) g_extracted=$ext run=$($R ./$o 2>&1 | paste -sd'|')"
    else echo "$a [$line] $l: REFUSED $(head -2 $o.err | paste -sd'|')"; fi
   done; done; done
  cd ..
done
