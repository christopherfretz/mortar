#!/bin/bash
# q10b/run.sh (M-6): a -no-pie host copies each kind of exported data of a
# -Bsymbolic library; the run says which copies are harmless; classify.py
# gives the rule's answer from program headers + st_value.  a64 + x86-64.
ulimit -c 0; cd "$(dirname "$0")"; W=work.run; [ -d $W ] && find $W -mindepth 1 -delete; mkdir -p $W; cd $W  # fresh outputs (mold writes in place)
R=/Volumes/AgentWork/mortar-helper/.build/review-110-adv
for arch in a64 x64; do
 if [ $arch = a64 ]; then CC=gcc; Q=""; else CC=x86_64-linux-gnu-gcc; Q="qemu-x86_64 -L /usr/x86_64-linux-gnu"; fi
 $CC -O1 -fPIC -c ../lib.c -o lib.$arch.o; $CC -O1 -fno-pie -c ../host.c -o host.$arch.o
 for ld in bfd lld mold; do case $ld in bfd) L="-fuse-ld=bfd";; lld) L="-B$R/lld22 -fuse-ld=lld";; mold) L="-fuse-ld=mold";; esac
  so=$PWD/lib.$arch.$ld.so
  $CC $L -shared -Wl,-Bsymbolic,-z,relro lib.$arch.o -o $so
  $CC -fuse-ld=bfd -no-pie host.$arch.o $so -o host.$arch.$ld 2>host.$arch.$ld.err
  echo "== $arch lib by $ld; host copies: $(readelf -rW host.$arch.$ld | awk '/COPY/{print $5}' | tr '\n' ' ')"
  echo "   run: $($Q ./host.$arch.$ld 2>&1)"
  python3 -I ../classify.py $so | sed 's/^/   /'
 done
done
for L in /usr/lib/aarch64-linux-gnu/libgdk_pixbuf-2.0.so.0.4200.12; do echo "== $L"; python3 -I ../classify.py $L | sed 's/^/   /'; done
