#!/bin/bash
# S1 (s1-section-permute.sh) for x86-64 or i386 objects, run under qemu.
# usage: s1x-section-permute.sh EMPTY-DIR x86_64|i386
set -u
here=$(cd "$(dirname "$0")" && pwd)
arch=$2
if [ "$arch" = i386 ]; then AS="i686-linux-gnu-as --32"; Q=qemu-i386; EM="-m elf_i386"
else AS="x86_64-linux-gnu-as"; Q=qemu-x86_64; EM="-m elf_x86_64"; fi
export AS
. "$here/lib.sh"
runx() { local out; out=$($Q ./"$1" 2>&1); echo "exit=$? out=[$out]"; }
cd "$1" || exit 1
fg() {
cat <<A
 .section .text.$1,"ax",@progbits
 .globl $1
 .type $1,@function
$1: mov \$$3,%eax
 ret
 .section .text.$2,"ax",@progbits
 .globl $2
 .type $2,@function
$2: mov \$$4,%eax
 ret
A
}
cs() { if [ "$arch" = i386 ]; then
printf ' .text\n .globl _start\n_start:\n call f\n add $%s,%%eax\n mov %%eax,%%ebx\n mov $1,%%eax\n int $0x80\n' "$1"
else
printf ' .text\n .globl _start\n_start:\n call f\n add $%s,%%eax\n mov %%eax,%%edi\n mov $60,%%eax\n syscall\n' "$1"
fi; }
fg f g 1 2 > a1.s; fg g f 2 1 > a2.s; cs 10 > c1.s; cs 20 > c2.s
asm a1.s a.o; asm c1.s c.o
L="$EM a.o c.o -static"
full out $L || exit 1
echo "link 0: $(runx out)   (want 11)"
s0=$(stat -c %s a.o); asm a2.s a.o; echo "a.o size $s0 -> $(stat -c %s a.o)"
echo "update 1 (a.o changed): $(upd out $L | sed 's/ windows=.*//')"
echo "after update 1: $(runx out)   (want 11)"
asm c2.s c.o
echo "update 2 (c.o changed): $(upd out $L | sed 's/ windows=.*//')"
echo "after update 2: $(runx out)   (want 21; 22 = g was called)"
full ref2 $L; echo "ref2: $(runx ref2)"
