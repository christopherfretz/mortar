#!/bin/bash
# WP-245 follow-up A: what mortar's FULL link gives for `.got` when no
# slot is demanded, on both 64-bit targets (static, -nostdlib, one
# `_start`). Variants: `got` loads g through the GOT; `no` takes g's
# address directly; `gotsym`/`nogotsym` the same plus a reference to
# `_GLOBAL_OFFSET_TABLE_`. Prints each `.got*` output section's address
# and size. Usage: got-empty.sh <mortar binary> <scratch dir>.
set -eu
mortar=$1 out=$2
mkdir -p "$out/bin"
ln -sf "$mortar" "$out/bin/ld"
for t in x86_64 aarch64; do
  if [ $t = x86_64 ]; then
    GOT='movq g@GOTPCREL(%rip), %rax'; NO='leaq g(%rip), %rax'
    GG='leaq _GLOBAL_OFFSET_TABLE_(%rip), %rax'
    X='movl $60, %eax
  xorl %edi, %edi
  syscall'
  else
    GOT='adrp x0, :got:g
  ldr x0, [x0, :got_lo12:g]'
    NO='adrp x0, g
  add x0, x0, :lo12:g'
    GG='adrp x0, _GLOBAL_OFFSET_TABLE_
  add x0, x0, :lo12:_GLOBAL_OFFSET_TABLE_'
    X='mov x0, #0
  mov x8, #93
  svc #0'
  fi
  for v in got no gotsym nogotsym; do
    case $v in
      got) I=$GOT ;; no) I=$NO ;;
      gotsym) I="$GOT
  $GG" ;;
      nogotsym) I="$NO
  $GG" ;;
    esac
    s=$out/$t-$v
    printf '  .text\n  .globl _start\n_start:\n  %s\n  %s\n  .data\n  .globl g\ng:\n  .long 5\n  .section .note.GNU-stack,"",%%progbits\n' "$I" "$X" > "$s.s"
    $t-linux-gnu-gcc -c -Wa,-mrelax-relocations=no "$s.s" -o "$s.o" 2>/dev/null ||
      $t-linux-gnu-gcc -c "$s.s" -o "$s.o"
    $t-linux-gnu-gcc -nostdlib -static -B"$out/bin/" -Wl,--incremental=full "$s.o" -o "$s"
    secs=$(readelf -SW "$s" | sed 's/\[ */[/' |
      awk '$2 ~ /^\.got/ {printf "%s addr=%s size=%s; ", $2, $4, $6}')
    echo "$t $v: ${secs:-no .got section}"
  done
done
echo "got-empty: DONE"
