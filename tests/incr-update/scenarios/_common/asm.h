/* Small portable pieces for the assembly scenarios (a64, x86-64, i386):
   fn NAME, BIND, VAL  a function in its own section .text.NAME returning VAL
   callf SYM           a call;  addv V  adds V to the result;  retv V  sets it
   start               the entry point's label (.text, global _start)
   write_str SYM, LEN  write(1, SYM, LEN)
   ptr SYM             an address-sized word
   and print_exit (print_exit.h). */
#include "print_exit.h"
#if defined(__aarch64__)
.macro fn name, bind, val
 .section .text.\name,"ax",@progbits
 .\bind \name
 .type \name,%function
\name: mov w0,#\val
 ret
.endm
.macro callf f
 bl \f
.endm
.macro addv v
 add w0,w0,#\v
.endm
.macro retv v
 mov w0,#\v
.endm
.macro write_str s, n
 mov x0,#1
 adrp x1, \s
 add x1, x1, :lo12:\s
 mov x2,#\n
 mov x8,#64
 svc #0
.endm
.macro ptr s
 .quad \s
.endm
#elif defined(__x86_64__)
.macro fn name, bind, val
 .section .text.\name,"ax",@progbits
 .\bind \name
 .type \name,%function
\name: mov $\val,%eax
 ret
.endm
.macro callf f
 call \f
.endm
.macro addv v
 add $\v,%eax
.endm
.macro retv v
 mov $\v,%eax
.endm
.macro write_str s, n
 mov $1,%edi
 lea \s(%rip),%rsi
 mov $\n,%edx
 mov $1,%eax
 syscall
.endm
.macro ptr s
 .quad \s
.endm
#else
.macro fn name, bind, val
 .section .text.\name,"ax",@progbits
 .\bind \name
 .type \name,%function
\name: mov $\val,%eax
 ret
.endm
.macro callf f
 call \f
.endm
.macro addv v
 add $\v,%eax
.endm
.macro retv v
 mov $\v,%eax
.endm
.macro write_str s, n
 mov $1,%ebx
 mov $\s,%ecx
 mov $\n,%edx
 mov $4,%eax
 int $0x80
.endm
.macro ptr s
 .long \s
.endm
#endif
.macro start
 .text
 .globl _start
 .type _start,%function
_start:
.endm
