#!/bin/bash
# escape/run2.sh: glibc 2.41's run-time answer: a -no-pie host linked (GNU ld)
# against a default-visibility build of the library, run against the
# protected build (same soname) -- with and without the W41 property (x86-64).
ulimit -c 0; cd "$(dirname "$0")"; W=work.run2; mkdir -p $W; cd $W
for arch in a64 x64; do
 if [ $arch = a64 ]; then CC=gcc; RUN=""; else CC=x86_64-linux-gnu-gcc; RUN="qemu-x86_64 -L /usr/x86_64-linux-gnu"; fi
 for prop in none iea; do
  [ $prop = iea ] && [ $arch = a64 ] && continue
  PF=""; [ $prop = iea ] && PF="-mno-direct-extern-access"
  mkdir -p def.$arch.$prop prot.$arch.$prop
  $CC -O2 -fPIC -c ../libdef.c -o libdef.$arch.o && $CC -shared -Wl,-soname,libe.so libdef.$arch.o -o def.$arch.$prop/libe.so
  $CC -O2 -fPIE $PF -c ../lib.c -o lib.$arch.$prop.o && $CC -shared -Wl,-soname,libe.so lib.$arch.$prop.o -o prot.$arch.$prop/libe.so
  $CC -O2 -fno-pie -c ../host.c -o host.$arch.o && $CC -no-pie host.$arch.o def.$arch.$prop/libe.so -o host.$arch.$prop
  echo "$arch $prop property-in-lib=$(readelf -nW prot.$arch.$prop/libe.so | grep -c -i 'indirect')"
  echo "  against default build: $(LD_LIBRARY_PATH=def.$arch.$prop $RUN ./host.$arch.$prop 2>&1 | head -1)"
  if [ -n "$RUN" ]; then out=$($RUN -E LD_LIBRARY_PATH=prot.$arch.$prop ./host.$arch.$prop 2>&1 | head -2 | tr '\n' ' '); else out=$(LD_LIBRARY_PATH=prot.$arch.$prop ./host.$arch.$prop 2>&1 | head -2 | tr '\n' ' '); fi
  echo "  against protected build: $out"
 done
done
