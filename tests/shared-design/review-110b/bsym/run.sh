#!/bin/bash
# D-112 table on x86-64 under qemu (attack 4): bits FUNC OBJECT NOTYPE TLS IFUNC, P = interposed.
ulimit -c 0; cd "$(dirname "$0")"; rm -rf work.run; mkdir work.run; cd work.run
L22="-fuse-ld=lld -B/Volumes/AgentWork/mortar-helper/.build/review-110-adv/lld22"
CC=x86_64-linux-gnu-gcc; X="qemu-x86_64 -L /usr/x86_64-linux-gnu"
cat > lib.c <<'E'
int f(void) { return 1; }
int d = 1;
__thread int t = 1;
static int iimpl(void) { return 1; }
static void *ires(void) { return (void *)iimpl; }
int i(void) __attribute__((ifunc("ires")));
extern int n;
__asm__(".globl n\n.data\n.p2align 2\nn: .long 1\n.text\n");
int probe(void) { return (f() == 100) | (d == 100) << 1 | (n == 100) << 2 | (t == 100) << 3 | (i() == 100) << 4; }
E
cat > pre.c <<'E'
int f(void) { return 100; }
int d = 100;
int n = 100;
__thread int t = 100;
int i(void) { return 100; }
E
cat > host.c <<'E'
#include <stdio.h>
int probe(void);
int main(void) { int m = probe(); const char *k[] = {"FUNC","OBJECT","NOTYPE","TLS","IFUNC"};
  for (int b = 0; b < 5; b++) printf("%s=%c ", k[b], (m >> b) & 1 ? 'P' : 'B'); printf("\n"); return 0; }
E
echo "{ f; };" > dl.list
$CC -O1 -fPIC -c lib.c -o lib.o; $CC -O1 -fPIC -shared pre.c -o pre.so
lines=("none|" "Bsymbolic|-Wl,-Bsymbolic" "Bsymbolic-functions|-Wl,-Bsymbolic-functions" "dynlist{f}|-Wl,--dynamic-list=dl.list" "dynlist+Bsymbolic|-Wl,--dynamic-list=dl.list -Wl,-Bsymbolic" "Bsymbolic+dynlist|-Wl,-Bsymbolic -Wl,--dynamic-list=dl.list" "dynlist+Bsymbolic-functions|-Wl,--dynamic-list=dl.list -Wl,-Bsymbolic-functions" "Bsymbolic-functions+dynlist|-Wl,-Bsymbolic-functions -Wl,--dynamic-list=dl.list" "dynlist+Bno-symbolic|-Wl,--dynamic-list=dl.list -Wl,-Bno-symbolic" "Bsymbolic+Bno-symbolic|-Wl,-Bsymbolic -Wl,-Bno-symbolic" "Bno-symbolic+Bsymbolic|-Wl,-Bno-symbolic -Wl,-Bsymbolic" "Bsymbolic-functions+Bsymbolic|-Wl,-Bsymbolic-functions -Wl,-Bsymbolic" "Bsymbolic+Bsymbolic-functions|-Wl,-Bsymbolic -Wl,-Bsymbolic-functions" "dynamic-list-data|-Wl,--dynamic-list-data")
for ln in "${lines[@]}"; do name=${ln%%|*}; fl=${ln#*|}
  for l in bfd lld22 mold; do case $l in bfd) U="";; lld22) U="$L22";; mold) U="-fuse-ld=mold";; esac
    d=$l.$name; mkdir -p "$d"
    if ! $CC $U -shared lib.o $fl -o "$d/lib.so" 2>"$d/err"; then echo "$name $l: REFUSED $(head -c 80 "$d/err")"; continue; fi
    $CC host.c "$d/lib.so" -o "$d/host" -Wl,-rpath,"$PWD/$d"
    echo "$name $l: $($X -E LD_PRELOAD=$PWD/pre.so "$d/host" 2>&1) $(readelf -dW "$d/lib.so" | grep -oE 'SYMBOLIC' | head -n1)"
  done
done
