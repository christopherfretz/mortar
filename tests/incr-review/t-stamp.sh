#!/bin/bash
# IN-22 shapes on this directory's file system: a.o rewritten IN PLACE
# with the same size and its mtime (ns) restored, right after the link;
# a.o replaced by a hard link of an equal-size file; a.o deleted and made
# again (inode reuse possible) with the old mtime. Each time the update
# must see a.o changed (hashed) and give f()=3.
# usage: t-stamp.sh EMPTY-DIR
set -u
here=$(cd "$(dirname "$0")" && pwd); . "$here/lib.sh"
cd "$1" || exit 1
fn() { printf ' .section .text.f,"ax",@progbits\n .globl f\n .type f,%%function\nf: mov w0,#%s\n ret\n' "$1"; }
fn 1 > a1.s; fn 3 > a3.s
printf ' .text\n .globl _start\n_start:\n bl f\n add w0,w0,#10\n mov x8,#93\n svc #0\n' > c.s
asm c.s c.o; $AS a3.s -o a3.o
L="a.o c.o -static"
for shape in inplace hardlink recreate; do
  asm a1.s a.o; full out $L || exit 1
  case $shape in
  inplace) python3 -I -c 'import os,sys; st=os.stat("a.o"); b=open("a3.o","rb").read(); assert len(b)==st.st_size; f=open("a.o","r+b"); f.write(b); f.close(); os.utime("a.o", ns=(st.st_atime_ns, st.st_mtime_ns))' ;;
  hardlink) cp a3.o a3h.o; touch -r a.o a3h.o; ln -f a3h.o a.o ;;
  recreate) m=$(stat -c %y a.o); ino=$(stat -c %i a.o); rm -f a.o; cp a3.o a.o; touch -d "$m" a.o; echo "   inode $ino -> $(stat -c %i a.o)" ;;
  esac
  echo "$shape: update: $(upd out $L | sed 's/ windows=.*//')"
  echo "   after: $(runx out)  (want 13)"
done
