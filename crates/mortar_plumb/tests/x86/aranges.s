# spec/image.md I-112 on an output the verifier reads as mortar's: one
# source assembled twice (a.o, b.o; the COMDAT group `grp` of b.o is
# dropped), linked with `--gc-sections` (`.text.dead` is collected).
.ifdef W64
  .macro addr v
    .quad \v
  .endm
.else
  .macro addr v
    .long \v
  .endm
.endif
    .globl _start
.ifdef FIRST
    .section .text._start,"ax",@progbits
_start:
.ifdef W64
    mov $60, %eax
    xor %edi, %edi
    syscall
.else
    mov $1, %eax
    xor %ebx, %ebx
    int $0x80
.endif
.endif
    .section .text.dead,"ax",@progbits
dead:
    ret
    .section .debug_grp,"G",@progbits,grp,comdat
gnote:
    .long 0
    .section .debug_aranges,"",@progbits
    .long end - ver
ver:
    .short 2
    .long 0
.ifdef W64
    .byte 8, 0
.else
    .byte 4, 0
.endif
    .balign 16
    # 1: a collected `SHF_ALLOC` target: the tombstone, and the length 0.
    addr dead
    addr 9
    # 2: a target that is not `SHF_ALLOC` (b.o's dropped group): the
    # tombstone, the length as the input has it.
    addr gnote
    addr 5
    # 3: a record's field right after the field: left as relocated.
    addr dead
    addr _start
    addr 0
    addr 0
end:
