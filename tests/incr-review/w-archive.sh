#!/bin/bash
# Archive shapes, one hop from a full link's state, by position and by
# identity: A1 the changed file now extracts a member it did not (bl f ->
# bl h); A2 two members named m.o (regular archive), x.o edited; A3 the
# same as a thin archive.  usage: w-archive.sh ROOT     (AArch64 host)
set -u
here=$(cd "$(dirname "$0")" && pwd); . "$here/lib.sh"
root=$1
fn() { printf ' .section .text.%s,"ax",@progbits\n .globl %s\n .type %s,%%function\n%s: mov w0,#%s\n ret\n' "$1" "$1" "$1" "$1" "$2"; }
start() { printf ' .text\n .globl _start\n_start:\n bl %s\n add w0,w0,#%s\n bl g\n mov x8,#93\n svc #0\n' "$1" "$2"; }
for c in A1 A2 A3; do for m in pos idn; do
  d="$root/$c-$m"; mkdir -p "$d/d1" "$d/d2"; find "$d" -mindepth 1 -name '*.o' -delete; cd "$d"
  fn f 1 > d1/m.s; fn g 2 > d2/m.s; fn h 3 > h.s
  asm d1/m.s d1/m.o; asm d2/m.s d2/m.o; asm h.s h.o
  rm -f lib.a
  case $c in
  A1) ar rcs lib.a d1/m.o h.o 2>/dev/null; ar q lib.a d2/m.o; start f 10 > x1.s; start h 10 > x2.s ;;
  A2) ar rcs lib.a d1/m.o; ar q lib.a d2/m.o; start f 10 > x1.s; start f 20 > x2.s ;;
  A3) ar rcsT lib.a d1/m.o d2/m.o; start f 10 > x1.s; start f 20 > x2.s ;;
  esac
  asm x1.s x.o; L="x.o lib.a -static"
  full out $L > l0.txt 2>&1 || { echo "$c: link 0 failed $(head -1 l0.txt)"; continue; }
  asm x2.s x.o
  if [ $m = idn ]; then export MORTAR_FLOOR_IDENTITY=1; else unset MORTAR_FLOOR_IDENTITY; fi
  echo "$c ($m): $(ar t lib.a | tr '\n' ' ') update: $(upd out $L | sed 's/ windows=.*//')"
  full ref $L > /dev/null 2>&1; echo "    out: $(runx out) ref: $(runx ref); $(same out ref)"
done; unset MORTAR_FLOOR_IDENTITY; cmp -s "$root/$c-pos/out" "$root/$c-idn/out" && echo "    position == identity" || echo "    POSITION != IDENTITY"; done
