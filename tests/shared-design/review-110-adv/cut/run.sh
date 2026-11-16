#!/bin/bash
# Cut check: S4 lists -static-libstdc++ in -shared but not S2 (TLS) as a dependency.
ulimit -c 0; cd "$(dirname "$0")"; W=work.$$; mkdir $W; cd $W
g++ -O1 -fPIC -c ../x.cc -o x.o
g++ -shared -static-libstdc++ x.o -o libx.so -Wl,-t 2>/dev/null | grep -E 'libstdc\+\+\.a|libgcc' | head -3
echo "TLS relocs in the inputs pulled from libstdc++.a / output:"; readelf -rW libx.so | awk '/TLS/{print $3}' | sort | uniq -c
readelf -SW libx.so | grep -E '\.tdata|\.tbss'; readelf -dW libx.so | grep -E 'TLSDESC|FLAGS'
