 .text
 .balign 4096
 .globl uf
 .type uf, %function
uf:
 .rept 1022
 nop
 .endr
 // erratum 843419: ADRP at page offset 0xff8, a load, a load through the ADRP's register
 adrp x1, u_data
 ldr x2, [x3]
 ldr x0, [x1, :lo12:u_data]
 ret
 .size uf, . - uf
 // u_data beyond ADR's reach (+-1 MiB), so the site needs a patch
 // (spec/a64.md 11.2)
 .bss
 .balign 8
 .space 0x200000
u_data: .quad 0
