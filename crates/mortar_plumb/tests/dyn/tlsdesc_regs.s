# TLSDESC relaxed to initial-exec into every register (WP-082 V2):
# f_R reads tlsx through a descriptor sequence whose lea writes %R.
	.text
	.type f_rax, @function
f_rax:
	leaq tlsx@TLSDESC(%rip), %rax
	call *tlsx@TLSCALL(%rax)
	movl %fs:(%rax), %eax
	ret
	.type f_rbx, @function
f_rbx:
	pushq %rbx
	leaq tlsx@TLSDESC(%rip), %rbx
	movq %rbx, %rax
	call *tlsx@TLSCALL(%rax)
	movl %fs:(%rax), %eax
	popq %rbx
	ret
	.type f_rcx, @function
f_rcx:
	pushq %rcx
	leaq tlsx@TLSDESC(%rip), %rcx
	movq %rcx, %rax
	call *tlsx@TLSCALL(%rax)
	movl %fs:(%rax), %eax
	popq %rcx
	ret
	.type f_rdx, @function
f_rdx:
	pushq %rdx
	leaq tlsx@TLSDESC(%rip), %rdx
	movq %rdx, %rax
	call *tlsx@TLSCALL(%rax)
	movl %fs:(%rax), %eax
	popq %rdx
	ret
	.type f_rsi, @function
f_rsi:
	pushq %rsi
	leaq tlsx@TLSDESC(%rip), %rsi
	movq %rsi, %rax
	call *tlsx@TLSCALL(%rax)
	movl %fs:(%rax), %eax
	popq %rsi
	ret
	.type f_rdi, @function
f_rdi:
	pushq %rdi
	leaq tlsx@TLSDESC(%rip), %rdi
	movq %rdi, %rax
	call *tlsx@TLSCALL(%rax)
	movl %fs:(%rax), %eax
	popq %rdi
	ret
	.type f_rbp, @function
f_rbp:
	pushq %rbp
	leaq tlsx@TLSDESC(%rip), %rbp
	movq %rbp, %rax
	call *tlsx@TLSCALL(%rax)
	movl %fs:(%rax), %eax
	popq %rbp
	ret
	.type f_rsp, @function
f_rsp:
	leaq tlsx@TLSDESC(%rip), %rsp
	movq %rsp, %rax
	call *tlsx@TLSCALL(%rax)
	movl %fs:(%rax), %eax
	ret
	.type f_r8, @function
f_r8:
	pushq %r8
	leaq tlsx@TLSDESC(%rip), %r8
	movq %r8, %rax
	call *tlsx@TLSCALL(%rax)
	movl %fs:(%rax), %eax
	popq %r8
	ret
	.type f_r9, @function
f_r9:
	pushq %r9
	leaq tlsx@TLSDESC(%rip), %r9
	movq %r9, %rax
	call *tlsx@TLSCALL(%rax)
	movl %fs:(%rax), %eax
	popq %r9
	ret
	.type f_r10, @function
f_r10:
	pushq %r10
	leaq tlsx@TLSDESC(%rip), %r10
	movq %r10, %rax
	call *tlsx@TLSCALL(%rax)
	movl %fs:(%rax), %eax
	popq %r10
	ret
	.type f_r11, @function
f_r11:
	pushq %r11
	leaq tlsx@TLSDESC(%rip), %r11
	movq %r11, %rax
	call *tlsx@TLSCALL(%rax)
	movl %fs:(%rax), %eax
	popq %r11
	ret
	.type f_r12, @function
f_r12:
	pushq %r12
	leaq tlsx@TLSDESC(%rip), %r12
	movq %r12, %rax
	call *tlsx@TLSCALL(%rax)
	movl %fs:(%rax), %eax
	popq %r12
	ret
	.type f_r13, @function
f_r13:
	pushq %r13
	leaq tlsx@TLSDESC(%rip), %r13
	movq %r13, %rax
	call *tlsx@TLSCALL(%rax)
	movl %fs:(%rax), %eax
	popq %r13
	ret
	.type f_r14, @function
f_r14:
	pushq %r14
	leaq tlsx@TLSDESC(%rip), %r14
	movq %r14, %rax
	call *tlsx@TLSCALL(%rax)
	movl %fs:(%rax), %eax
	popq %r14
	ret
	.type f_r15, @function
f_r15:
	pushq %r15
	leaq tlsx@TLSDESC(%rip), %r15
	movq %r15, %rax
	call *tlsx@TLSCALL(%rax)
	movl %fs:(%rax), %eax
	popq %r15
	ret
	.globl main
	.type main, @function
main:
	pushq %rbx
	xorl %ebx, %ebx
	call f_rax
	addl %eax, %ebx
	call f_rbx
	addl %eax, %ebx
	call f_rcx
	addl %eax, %ebx
	call f_rdx
	addl %eax, %ebx
	call f_rsi
	addl %eax, %ebx
	call f_rdi
	addl %eax, %ebx
	call f_rbp
	addl %eax, %ebx
	call f_r8
	addl %eax, %ebx
	call f_r9
	addl %eax, %ebx
	call f_r10
	addl %eax, %ebx
	call f_r11
	addl %eax, %ebx
	call f_r12
	addl %eax, %ebx
	call f_r13
	addl %eax, %ebx
	call f_r14
	addl %eax, %ebx
	call f_r15
	addl %eax, %ebx
	leal -75(%rbx), %eax
	popq %rbx
	ret
	.section .note.GNU-stack,"",@progbits
