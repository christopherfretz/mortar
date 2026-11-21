// The site: `site` is a 4096-aligned section whose words at 0xff8, 0xffc
// and 0x1000 are ADRP x1, a load, and a load through x1 with an unsigned
// immediate (a64.md §11.1, the 3-word form). `far` lies 2 MiB further, in
// `.bss`, so Page(far) - Q is outside ADR's s(21) and the site gets a
// patch in `.text`'s island (a64.md §11.2, layout.md L-117).
// SITE: the same section with another return value, at the same size.
#if defined(__aarch64__)
 .section .text.site,"ax",@progbits
 .balign 4096
 .globl site
 .type site,%function
site:
#ifndef SITE
 mov w0, #1
#else
 mov w0, #2
#endif
 .rept 1021
 nop
 .endr
 adrp x1, far
 ldr x2, [sp]
 ldr x3, [x1, :lo12:far]
 add w0, w0, w3
 ret
 .size site, . - site

// a writable section with bytes: a RW segment of .bss alone is refused by
// eu-elflint ("writable but contains no writable sections"; GNU ld 2.44's
// output of these objects too)
 .section .data.word,"aw",@progbits
 .balign 8
 .globl word
word:
 .quad 1

 .section .bss.far,"aw",@nobits
 .balign 16
 .skip 0x200000
 .globl far
 .type far,%object
far:
 .quad 0
 .size far, 8
#endif
