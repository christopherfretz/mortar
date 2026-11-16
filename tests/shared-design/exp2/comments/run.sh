#!/bin/bash
# comments/run.sh (M-2): '#', '/* */' and '//' comments in a version script
# and in a dynamic list, under the three references.  AArch64.
ulimit -c 0; cd "$(dirname "$0")"; W=work.run; [ -d $W ] && find $W -mindepth 1 -delete; mkdir -p $W; cd $W  # fresh outputs (mold writes in place)
R=/Volumes/AgentWork/mortar-helper/.build/review-110-adv
gcc -fPIC -c ../lib.c -o lib.o
printf '# hash line\nV1 {\n global: foo; # trailing hash\n bar;\n local: *;\n};\n' > v_hash.map
printf '/* block */\nV1 {\n global: foo; /* inline\n multi-line */ bar;\n local: *;\n};\n' > v_block.map
printf '// slash\nV1 {\n global: foo; // trailing\n bar;\n local: *;\n};\n' > v_slash.map
printf 'V1 { global: foo; #bar;\n local: *; };\n' > v_hashmid.map
printf '# hash line\n{\n foo; # trailing\n bar;\n};\n' > d_hash.list
printf '/* block */\n{\n foo; /* x */ bar;\n};\n' > d_block.list
printf '// slash\n{\n foo; // trailing\n bar;\n};\n' > d_slash.list
for ld in bfd lld mold; do
 case $ld in bfd) L="-fuse-ld=bfd";; lld) L="-B$R/lld22 -fuse-ld=lld";; mold) L="-fuse-ld=mold";; esac
 for f in v_hash.map v_block.map v_slash.map v_hashmid.map d_hash.list d_block.list d_slash.list; do
  case $f in v_*) opt=--version-script=$f; show='$7!="UND"{print $8}';; d_*) opt="--dynamic-list=$f -Bsymbolic"; show='$7!="UND"{print $8}';; esac
  o=$f.$ld.so
  if gcc $L -shared -nostdlib lib.o -Wl,${opt// /,} -o $o 2>$o.err; then
    if [[ $f == d_* ]]; then
      # which functions stay preemptible: the listed ones are not bound by -Bsymbolic (GNU: list governs)
      echo "$ld $f: accepted; dynsym=[$(readelf --dyn-syms -W $o | awk 'NR>3 && $7!="UND"{print $8}' | sort | tr '\n' ' ')] flags=[$(readelf -dW $o | grep -oE '\(SYMBOLIC\)|FLAGS.*' | tr '\n' ' ')] $(head -1 $o.err | cut -c1-80)"
    else
      echo "$ld $f: accepted; exported=[$(readelf --dyn-syms -W $o | awk 'NR>3 && $7!="UND"{print $8}' | sort | tr '\n' ' ')] $(head -1 $o.err | cut -c1-80)"
    fi
  else echo "$ld $f: ERROR $(head -1 $o.err | cut -c1-120)"; fi
 done
done
