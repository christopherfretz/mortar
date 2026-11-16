#!/bin/bash
# D-121: an exported PROTECTED IFUNC (rule: one IPLT entry, exported STT_FUNC at it) -- unmeasured by the design.
ulimit -c 0; cd "$(dirname "$0")"; W=work.$$; mkdir $W; cd $W
R=/Volumes/AgentWork/mortar-helper/.build/review-110-adv
gcc -O1 -fPIC -c ../lib.c -o lib.o
for ld in bfd lld mold; do
 case $ld in bfd) L="-fuse-ld=bfd";; lld) L="-B$R/lld22 -fuse-ld=lld";; mold) L="-fuse-ld=mold";; esac
 mkdir $ld
 if gcc $L -shared lib.o -o $ld/libp.so 2>$ld/err; then
  echo "$ld: dynsym f: $(readelf --dyn-syms -W $ld/libp.so | awk '$8=="f"{print $4,$5,$6,$2}') rela: $(readelf -rW $ld/libp.so | awk '/R_AARCH64/{print $3}' | sort | uniq -c | tr -s ' ' | tr '\n' ',')"
  for h in pie nopie; do
   if [ $h = pie ]; then F=""; else F="-fno-pic -no-pie"; fi
   if gcc $F ../host.c $ld/libp.so -Wl,-rpath,$PWD/$ld -o $ld/host-$h 2>$ld/herr-$h; then echo "   host $h: $($ld/host-$h 2>&1)"; else echo "   host $h: LINK $(head -1 $ld/herr-$h)"; fi
  done
 else echo "$ld: LINK FAILED $(head -2 $ld/err | tr '\n' ' ')"; fi
done
