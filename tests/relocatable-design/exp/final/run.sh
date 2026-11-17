#!/bin/bash
# final/run.sh WORK [MORTAR]: item D. The C and C++ components of ../corpus,
# compiled per target and compiler, partially linked by GNU ld, lld 22 and mold
# (-r), then linked finally (static and default PIE) by GNU ld and by mortar
# through the driver, and RUN; each run's output is compared with the direct
# link's (GNU ld, the same objects without -r).
set -u
ulimit -c 0
WORK=$(realpath -m "$1"); MORTAR=${2:-/Volumes/AgentWork/mortar-helper/.build/target-wp-154/mortar-3ad9867a}
HERE=$(cd "$(dirname "$0")" && pwd); C=$HERE/../corpus
mkdir -p "$WORK/tmp" "$WORK/mbin"; cd "$WORK" || exit 1; export TMPDIR=$WORK/tmp
for n in ld ld.bfd x86_64-linux-gnu-ld i686-linux-gnu-ld aarch64-linux-gnu-ld; do ln -sf "$MORTAR" "mbin/$n"; done
runq() { case $1 in aarch64) "${@:2}";; x86_64) qemu-x86_64 -L /usr/x86_64-linux-gnu "${@:2}";; i686) qemu-i386 -L /usr/i686-linux-gnu "${@:2}";; esac; }
rlink() { # rlink ARCH LINKER OUT INPUTS...
  local a=$1 k=$2 o=$3; shift 3
  case $k in
    bfd) case $a in aarch64) ld.bfd -r -o "$o" "$@";; *) $a-linux-gnu-ld.bfd -r -o "$o" "$@";; esac;;
    lld) ld.lld-22 -r -o "$o" "$@";;
    mold) mold -r -o "$o" "$@";;
  esac
}
for a in aarch64 x86_64 i686; do
 for comp in gcc clang; do
  if [ $comp = gcc ]; then case $a in aarch64) CC=gcc; CXX=g++;; *) CC=$a-linux-gnu-gcc; CXX=$a-linux-gnu-g++;; esac; T=""
  else CC=clang-22; CXX=clang++-22; T="--target=$a-linux-gnu"; fi
  d=$a.$comp; mkdir -p $d
  F="-O1 -g -fcommon -ffunction-sections -fdata-sections"
  for s in c1 c2 c3 c4 mainc; do $CC $T $F -c "$C/$s.c" -o $d/$s.o; done
  for s in x1 x2 mainx; do $CXX $T $F -c "$C/$s.cc" -o $d/$s.o; done
  for lang in c cifunc cctors cxx; do
    case $lang in
      c) parts="$d/c1.o $d/c2.o"; m=$d/mainc.o; DRV=$CC;;
      cifunc) parts="$d/c1.o $d/c2.o $d/c4.o"; m=$d/mainc.o; DRV=$CC;;
      cctors) parts="$d/c1.o $d/c2.o $d/c3.o"; m=$d/mainc.o; DRV=$CC;;
      cxx) parts="$d/x1.o $d/x2.o"; m=$d/mainx.o; DRV=$CXX;;
    esac
    for mode in static pie; do
      mf=""; [ $mode = static ] && mf=-static
      $DRV $T $mf $m $parts -o $d/direct.$lang.$mode 2> $d/direct.$lang.$mode.err
      want=$(runq $a ./$d/direct.$lang.$mode 2>&1; echo "rc=$?")
      for k in bfd lld mold; do
        r=$d/comp.$lang.$k.o
        [ -f $r ] || rlink $a $k $r $parts 2> $r.err || { echo "$a $comp $lang -r:$k: REFUSED $(head -2 $r.err | tr '\n' ' ')"; continue; }
        for fin in bfd mortar; do
          ff=""; [ $fin = mortar ] && ff="-B$WORK/mbin/"
          ex=$d/fin.$lang.$k.$fin.$mode
          if ! $DRV $T $mf $ff $m $r -o $ex 2> $ex.err; then d0=""; [ $fin = mortar ] && { $DRV $T $mf $ff $m $parts -o $ex.d 2>/dev/null && d0="(direct mortar links)" || d0="(direct mortar link refused too)"; }; echo "$a $comp $lang $mode -r:$k final:$fin: LINK FAILED $d0: $(grep -v '^collect2\|^clang' $ex.err | head -3 | tr '\n' ' ' | cut -c1-300)"; continue; fi
          got=$(runq $a ./$ex 2>&1; echo "rc=$?")
          if [ "$got" = "$want" ]; then v=same; else v="DIFFERS: got [$(echo "$got" | tr '\n' '|')] want [$(echo "$want" | tr '\n' '|')]"; fi
          w=$(grep -ci warning $ex.err)
          echo "$a $comp $lang $mode -r:$k final:$fin: $v (warnings $w)"
        done
      done
      echo "$a $comp $lang $mode direct: $(echo "$want" | tr '\n' '|')"
    done
  done
 done
done
