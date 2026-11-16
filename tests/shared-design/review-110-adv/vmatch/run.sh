#!/bin/bash
# Version-script matching precedence: the three references vs a literal model of resolve 9.7 rules 1-5.
ulimit -c 0; cd "$(dirname "$0")"; W=work.$$; mkdir $W; cd $W
R=/Volumes/AgentWork/mortar-helper/.build/review-110-adv
gcc -fPIC -c ../lib.c -o lib.o
python3 -I ../model.py ../scripts.txt > model.out
printf '%-5s %-8s %-8s %-8s %-8s  %s\n' id model bfd lld22 mold script
while IFS='|' read -r k s; do
 echo "$s" > $k.map; row=""
 for ld in bfd lld mold; do
  case $ld in bfd) L="-fuse-ld=bfd";; lld) L="-B$R/lld22 -fuse-ld=lld";; mold) L="-fuse-ld=mold";; esac
  if gcc $L -shared -nostdlib lib.o -Wl,--version-script=$k.map -o $k.$ld.so 2>$k.$ld.err; then
   v=$(readelf --dyn-syms -W $k.$ld.so | awk '$8 ~ /^foo(@|$)/ {print $8}'); [ -z "$v" ] && v=LOCAL; v=${v#foo}; v=${v#@}; v=${v#@}; [ -z "$v" ] && v=BASE
  else v=ERR; fi; row="$row $(printf '%-8s' $v)"
 done
 printf '%-5s %-8s%s  %s\n' $k "$(awk -v k=$k '$1==k{print $2}' model.out)" "$row" "$s"
done < ../scripts.txt
