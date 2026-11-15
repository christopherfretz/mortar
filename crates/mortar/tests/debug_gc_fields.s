# spec/image.md I-47, I-48, I-112 on a REL (i386) or RELA (x86-64) input:
# every debug section holds references to a collected section and to a
# collected merge piece, through each data field width the arch has, each
# followed by a sentinel the tombstone must not reach.
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
    .section .text._start,"ax",@progbits
_start:
.ifdef W64
    movabs $live_s, %rcx
    mov $60, %eax
    xor %edi, %edi
    syscall
.else
    mov $live_s, %ecx
    mov $1, %eax
    xor %ebx, %ebx
    int $0x80
.endif
    .section .text.dead,"ax",@progbits
dead:
    nop
    nop
    ret
    .section .rodata.str,"aMS",@progbits,1
live_s: .asciz "live"
dead_s: .asciz "dead piece"
    .macro fields piece=1
    addr 0xa5a5a5a5
    addr dead + 1
    addr 0xa5a5a5a5
    .long dead
    .long 0xa5a5a5a5
    .word dead
    .word 0xa5a5
    .byte dead
    .byte 0xa5
    .long dead - .
    .long 0xa5a5a5a5
  .if \piece
    addr dead_s
    addr 0xa5a5a5a5
  .endif
    addr live_s
    addr 0xa5a5a5a5
    .endm
    .section .debug_info,"",@progbits
    fields
    .section .debug_line,"",@progbits
    fields
    .section .debug_str_offsets,"",@progbits
    fields
    .section .debug_ranges,"",@progbits
    fields
    .section .debug_loc,"",@progbits
    fields
    .section .debug_rnglists,"",@progbits
    fields
    .section .debug_loclists,"",@progbits
    fields
    .section .debug_addr,"",@progbits
    fields
    .section .debug_frame,"",@progbits
    fields
    .section .debug_aranges,"",@progbits
    fields 0
