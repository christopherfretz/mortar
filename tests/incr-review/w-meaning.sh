#!/bin/bash
# One-hop shapes from a FULL link's state: the unchanged files stay
# byte-equal while their meaning changes through the one changed file.
# Each case: link 0, the edit, the test path's update (by position, then
# by identity on a second copy), the full link of the edited inputs; the
# three compared by run and bytes.
# usage: w-meaning.sh ROOT CASE     (AArch64 host)
set -u
here=$(cd "$(dirname "$0")" && pwd); . "$here/lib.sh"
root=$1; case=$2
fn() { printf ' .section .text.%s,"ax",@progbits\n .%s %s\n .type %s,%%function\n%s: mov w0,#%s\n ret\n' "$1" "$2" "$1" "$1" "$1" "$3"; }
start() { printf ' .text\n .globl _start\n_start:\n bl %s\n add w0,w0,#%s\n mov x8,#93\n svc #0\n' "$1" "$2"; }
setup() { # writes the sources of case $1 into cwd: v1 and v2 of the changed file x.s
  case $1 in
  W1-weak-overridden) # a.o weak f=1; x.o weak f=2 -> global f=2 (st_info only)
    fn f weak 1 > a.s; fn f weak 2 > x1.s; fn f globl 2 > x2.s; start f 10 > c.s
    ORDER="a.o x.o c.o"; WANT=12 ;;
  W1b-weak-no-longer-overridden) # x.o global f=2 -> weak f=2: a.o's weak f=1 wins
    fn f weak 1 > a.s; fn f globl 2 > x1.s; fn f weak 2 > x2.s; start f 10 > c.s
    ORDER="a.o x.o c.o"; WANT=11 ;;
  W8-gc-liveness-flip) # x.o calls f -> calls g (same size), --gc-sections
    { fn f globl 1; fn g globl 2; } > a.s; start f 10 > x1.s; start g 10 > x2.s; : > c.s
    ORDER="a.o x.o"; EXTRA="--gc-sections"; WANT=12 ;;
  W9-undef-to-other) # x.o calls f -> calls g (same size), no gc
    { fn f globl 1; fn g globl 2; } > a.s; start f 10 > x1.s; start g 10 > x2.s; : > c.s
    ORDER="a.o x.o"; WANT=12 ;;
  W10-local-to-global) # x.o: local f=2 -> global f=2; a.o weak f=1, c.o calls f
    fn f weak 1 > a.s; fn f local 2 | sed 's/ .local f/ .local f/' > x1.s; fn f globl 2 > x2.s; start f 10 > c.s
    ORDER="a.o x.o c.o"; WANT=12 ;;
  W11-comdat-keeper-moves) # x.o (first) leaves group G1 (renamed G2, same length): b.o's copy keeps
    grp() { printf ' .section .text.h,"axG",@progbits,%s,comdat\n .globl h\n .type h,%%function\nh: mov w0,#%s\n ret\n' "$1" "$2"; }
    grp G1 2 > x1.s; grp G2 2 > x2.s; grp G1 2 > a.s; start h 10 > c.s
    ORDER="x.o a.o c.o"; WANT=12 ;;
  W6-string-dedup-into-changed) # x.o (first) gains a.o's string at the same size
    printf ' .section .rodata.str1.1,"aMS",@progbits,1\n .asciz "zzz"\n .data\n .xword .rodata.str1.1\n' > x1.s
    printf ' .section .rodata.str1.1,"aMS",@progbits,1\n .asciz "foo"\n .data\n .xword .rodata.str1.1\n' > x2.s
    printf ' .section .rodata.str1.1,"aMS",@progbits,1\nLf: .asciz "foo"\n .text\n .globl _start\n_start:\n mov x0,#1\n adrp x1, Lf\n add x1,x1,:lo12:Lf\n mov x2,#3\n mov x8,#64\n svc #0\n mov x0,#12\n mov x8,#93\n svc #0\n' > a.s
    : > c.s; ORDER="x.o a.o"; WANT=12 ;;
  esac
}
for m in pos idn; do
  d="$root/$case-$m"; mkdir -p "$d"; find "$d" -mindepth 1 -delete; cd "$d"
  EXTRA=""; setup "$case"
  asm a.s a.o; asm x1.s x.o; [ -s c.s ] && asm c.s c.o
  L="$ORDER -static $EXTRA"
  full out $L > link0.txt 2>&1 || { echo "$case: link 0 failed: $(head -2 link0.txt)"; break; }
  s0=$(stat -c %s x.o); asm x2.s x.o; s1=$(stat -c %s x.o)
  if [ "$m" = idn ]; then export MORTAR_FLOOR_IDENTITY=1; else unset MORTAR_FLOOR_IDENTITY; fi
  u=$(upd out $L)
  full ref $L > ref.txt 2>&1
  echo "$case ($m): x.o $s0->$s1; update: $u"
  echo "    out: $(runx out)  ref: $(runx ref)  (want exit $WANT); $(same out ref)"
done
unset MORTAR_FLOOR_IDENTITY
cmp -s "$root/$case-pos/out" "$root/$case-idn/out" && echo "    position == identity (bytes)" || echo "    POSITION != IDENTITY (bytes)"
