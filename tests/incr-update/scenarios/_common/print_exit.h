/* print_exit (assembly scenarios without libc): writes the value in w0 /
   eax plus 64 as one letter and a newline, then exits 0 (so the run check
   compares the letter, and both programs exit 0). */
#if defined(__aarch64__)
.macro print_exit
 add w0, w0, #64
 mov w1, #10
 strb w0, [sp, #-16]!
 strb w1, [sp, #1]
 mov x1, sp
 mov x0, #1
 mov x2, #2
 mov x8, #64
 svc #0
 mov x0, #0
 mov x8, #93
 svc #0
.endm
#elif defined(__x86_64__)
.macro print_exit
 add $64, %eax
 sub $16, %rsp
 movb %al, (%rsp)
 movb $10, 1(%rsp)
 mov $1, %edi
 mov %rsp, %rsi
 mov $2, %edx
 mov $1, %eax
 syscall
 xor %edi, %edi
 mov $60, %eax
 syscall
.endm
#else
.macro print_exit
 add $64, %eax
 sub $16, %esp
 movb %al, (%esp)
 movb $10, 1(%esp)
 mov $1, %ebx
 mov %esp, %ecx
 mov $2, %edx
 mov $4, %eax
 int $0x80
 xor %ebx, %ebx
 mov $1, %eax
 int $0x80
.endm
#endif
