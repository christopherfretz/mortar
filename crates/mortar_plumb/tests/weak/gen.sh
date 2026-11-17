#!/bin/sh
# Regenerates WP-165's fixtures of tests/weak.rs: dyn.md D-25's branch
# cells, -nostdlib, AArch64 and x86-64:
#   bo-exec, bo-pie  start(BO) call: `vfn` reached by a call only (WeakDyn)
#   split            start(SPLIT) test call, -no-pie: the split key
#   w12              start(W12) test got, -no-pie: Weak0 (GOT + unfillable)
# mortar's outputs (with maps), and lld's and GNU ld's of bo-exec and
# split (with maps and the words each linker was given, `.line`).
# The outputs are checked in.
#   sh gen.sh <mortar>
# Run from this directory. First made with the mortar of WP-165's base
# (int/batch-9, 0b04350c).
set -e
mortar=$1
for a in a64 x64; do
  case $a in
    a64) cc=aarch64-linux-gnu-gcc; bfd=aarch64-linux-gnu-ld.bfd; ld=/lib/ld-linux-aarch64.so.1;;
    x64) cc=x86_64-linux-gnu-gcc; bfd=x86_64-linux-gnu-ld.bfd; ld=/lib64/ld-linux-x86-64.so.2;;
  esac
  f="-O2 -fno-asynchronous-unwind-tables -fno-stack-protector"
  $cc $f -fPIE -c call.c -o call-$a.o
  $cc $f -fPIE -c got.c -o got-$a.o
  $cc $f -mcmodel=large -fno-pic -c test.c -o test-$a.o
  for m in BO SPLIT W12; do
    $cc $f -fPIE -D$m -c start.c -o start-$(echo $m | tr A-Z a-z)-$a.o
  done
  $cc $f -fPIC -shared -nostdlib -Wl,-soname,libm2.so -Wl,-z,max-page-size=4096 m2.c -o libm2-$a.so
  "$mortar" -o bo-exec-$a -dynamic-linker $ld start-bo-$a.o call-$a.o libm2-$a.so \
    --mortar-map=bo-exec-$a.map
  "$mortar" -pie -o bo-pie-$a -dynamic-linker $ld start-bo-$a.o call-$a.o libm2-$a.so \
    --mortar-map=bo-pie-$a.map
  "$mortar" -o split-$a -dynamic-linker $ld start-split-$a.o test-$a.o call-$a.o libm2-$a.so \
    --mortar-map=split-$a.map
  "$mortar" -o w12-$a -dynamic-linker $ld start-w12-$a.o test-$a.o got-$a.o libm2-$a.so \
    --mortar-map=w12-$a.map
  for c in bo-exec split; do
    case $c in
      bo-exec) o="start-bo-$a.o call-$a.o";;
      split) o="start-split-$a.o test-$a.o call-$a.o";;
    esac
    for l in lld bfd; do
      case $l in lld) prog=ld.lld-19;; bfd) prog=$bfd;; esac
      # shellcheck disable=SC2086
      set -- -z max-page-size=4096 -o $c-$a.$l -dynamic-linker $ld $o libm2-$a.so -Map=$c-$a.$l.map
      printf '%s\n' "$@" > $c-$a.$l.line
      $prog "$@"
    done
  done
done
