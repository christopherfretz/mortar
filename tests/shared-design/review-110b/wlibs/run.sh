#!/bin/bash
# W-mutations built by the references (attack 2) and W43 (attack 4). Fresh outputs each run.
ulimit -c 0; cd "$(dirname "$0")"; rm -rf work.run; mkdir work.run; cd work.run
L22="-fuse-ld=lld -B/Volumes/AgentWork/mortar-helper/.build/review-110-adv/lld22"
MOLD="-fuse-ld=mold"
X="qemu-x86_64 -L /usr/x86_64-linux-gnu"
cat > tls.c <<'E'
__thread int tv = 4;
int f(void) { return 1; }
int gettv(void) { return tv; }
E
cat > pre.c <<'E'
__thread int other = 7;
__thread int tv = 104;
int f(void) { return 100; }
E
cat > host.c <<'E'
#include <stdio.h>
int gettv(void);
int main(void) { printf("gettv=%d\n", gettv()); return 0; }
E
echo "f;" | sed 's/^/{ /; s/$/ };/' > dl.list
echo "== W43: --dynamic-list {f}, tv unlisted (bound by D-112), interposer defines tv at another offset"
for a in x64 a64; do
  if [ $a = x64 ]; then CC=x86_64-linux-gnu-gcc; R="$X -E LD_PRELOAD=$PWD/pre.$a.so"; else CC=gcc; R="env LD_PRELOAD=$PWD/pre.$a.so"; fi
  $CC -O1 -fPIC -c tls.c -o tls.$a.o; $CC -O1 -fPIC -shared pre.c -o pre.$a.so
  for l in bfd lld22 mold; do
    case $l in bfd) U="";; lld22) U="$L22";; mold) U="$MOLD";; esac
    d=$a.$l; mkdir -p $d
    $CC $U -shared tls.$a.o -Wl,--dynamic-list=dl.list -o $d/libt.so 2>&1 | head -n 2
    $CC host.c $d/libt.so -o $d/host -Wl,-rpath,$PWD/$d
    echo "$d: $($R $d/host 2>&1) relocs: $(readelf -rW $d/libt.so | grep -E 'TLS|DTP' | awk '{print $3, $5}' | tr '\n' ' ')"
  done
done
echo "== W31: lld22 -Bsymbolic --dynamic-list {f}: f is listed (preemptible by D-112)"
cat > w31.c <<'E'
int f(void) { return 1; }
int callf(void) { return f(); }
E
cat > h31.c <<'E'
#include <stdio.h>
int callf(void);
int main(void) { printf("callf=%d\n", callf()); return 0; }
E
gcc -O1 -fPIC -c w31.c -o w31.o
for l in bfd lld22 mold; do case $l in bfd) U="";; lld22) U="$L22";; mold) U="$MOLD";; esac
  gcc $U -shared w31.o -Wl,-Bsymbolic -Wl,--dynamic-list=dl.list -o libw31.$l.so
  gcc h31.c ./libw31.$l.so -o h31.$l -Wl,-rpath,$PWD
  echo "$l: $(LD_PRELOAD=$PWD/pre.a64.so ./h31.$l) FLAGS: $(readelf -dW libw31.$l.so | grep -E 'FLAGS|SYMBOLIC' | awk '{print $2,$3,$4,$5}' | tr '\n' ' ') JUMP_SLOT/f: $(readelf -rW libw31.$l.so | grep -c 'JUMP_SLOT.* f')"
done
echo "== W24: .ctors input"
cat > w24.c <<'E'
#include <stdio.h>
static void c(void) { puts("ctor ran"); }
__attribute__((section(".ctors"), used)) static void (*p)(void) = c;
int w24(void) { return 0; }
E
gcc -O1 -fPIC -c w24.c -o w24.o
for l in bfd lld22 mold; do case $l in bfd) U="";; lld22) U="$L22";; mold) U="$MOLD";; esac
  gcc $U -shared w24.o -o libw24.$l.so
  echo "$l: sections: $(readelf -SW libw24.$l.so | grep -oE '\.(ctors|init_array)' | tr '\n' ' ') run: $(python3 -c "import ctypes; ctypes.CDLL('./libw24.$l.so')" 2>&1)"
done
echo "== W22: hidden -init function under --gc-sections"
cat > w22.c <<'E'
#include <stdio.h>
__attribute__((visibility("hidden"))) void myinit(void) { puts("init ran"); }
int w22(void) { return 0; }
E
gcc -O1 -fPIC -ffunction-sections -c w22.c -o w22.o
for l in bfd lld22 mold; do case $l in bfd) U="";; lld22) U="$L22";; mold) U="$MOLD";; esac
  gcc $U -shared w22.o -Wl,--gc-sections -Wl,-init=myinit -o libw22.$l.so
  echo "$l: $(readelf -dW libw22.$l.so | grep -E '\(INIT\)' | awk '{print $2,$3}') run: $(python3 -c "import ctypes; ctypes.CDLL('./libw22.$l.so')" 2>&1)"
done
echo "== W20: exported IFUNC, no option"
cat > w20.c <<'E'
static int impl(void) { return 1; }
static void *res(void) { return (void *)impl; }
int fi(void) __attribute__((ifunc("res")));
int callfi(void) { return fi(); }
E
cat > prefi.c <<'E'
int fi(void) { return 100; }
E
cat > h20.c <<'E'
#include <stdio.h>
int callfi(void);
int main(void) { printf("callfi=%d\n", callfi()); return 0; }
E
gcc -O1 -fPIC -c w20.c -o w20.o; gcc -fPIC -shared prefi.c -o prefi.so
for l in bfd lld22 mold; do case $l in bfd) U="";; lld22) U="$L22";; mold) U="$MOLD";; esac
  gcc $U -shared w20.o -o libw20.$l.so; gcc h20.c ./libw20.$l.so -o h20.$l -Wl,-rpath,$PWD
  echo "$l: $(LD_PRELOAD=$PWD/prefi.so ./h20.$l) relocs: $(readelf -rW libw20.$l.so | grep -E 'IRELATIV|JUMP_SLOT|GLOB_DAT' | awk '{print $3,$5}' | tr '\n' ' ')"
done
echo "DONE"
