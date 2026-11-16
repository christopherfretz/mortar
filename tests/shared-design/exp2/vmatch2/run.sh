#!/bin/bash
# vmatch2/run.sh: version-script matching (resolve 9.7, WP-110 second pass):
# the three references against model.py (written from the restated rule) for
# foo (C), _Z3fooi (foo(int)), _ZN2ns3fooEv (ns::foo()).  AArch64, GCC 14.
ulimit -c 0; cd "$(dirname "$0")"; W=work.run; [ -d $W ] && find $W -mindepth 1 -delete; mkdir -p $W; cd $W  # fresh outputs (mold writes in place)
R=/Volumes/AgentWork/mortar-helper/.build/review-110-adv
g++ -fPIC -c ../lib.cc -o lib.o
python3 -I ../model.py ../scripts.txt > model.out
printf '%-5s %-14s %-9s %-9s %-9s %-9s %-9s\n' id sym mortar gnu-model bfd lld22 mold
while IFS='|' read -r k s; do
 echo "$s" > $k.map
 for sym in foo _Z3fooi _ZN2ns3fooEv; do
  row=""
  for ld in bfd lld mold; do
   case $ld in bfd) L="-fuse-ld=bfd";; lld) L="-B$R/lld22 -fuse-ld=lld";; mold) L="-fuse-ld=mold";; esac
   out=$k.$ld.so
   if [ ! -f $out.done ]; then
     if g++ $L -shared -nostdlib lib.o -Wl,--version-script=$k.map -o $out 2>$k.$ld.err; then touch $out.done; else touch $out.done $out.fail; fi
   fi
   if [ -f $out.fail ]; then v=ERR; else
    v=$(readelf --dyn-syms -W $out | awk -v s=$sym '{n=$8; sub(/@.*/,"",n)} n==s {print $8}')
    [ -z "$v" ] && v=LOCAL; v=${v#$sym}; v=${v#@}; v=${v#@}; [ -z "$v" ] && v=BASE
   fi
   row="$row $(printf '%-9s' $v)"
  done
  m=$(awk -v k=$k -v s=$sym '$1==k && $2==s {print $3, $4}' model.out)
  printf '%-5s %-14s %-9s %-9s%s\n' $k $sym $m "$row"
 done
done < ../scripts.txt
