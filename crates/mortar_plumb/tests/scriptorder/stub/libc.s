	.text
	.globl puts, printf, abort, __cxa_finalize, __libc_start_main
	.globl __cxa_atexit, __cxa_at_quick_exit, __register_atfork, __stack_chk_fail
	.type puts, %function
	.type printf, %function
	.type abort, %function
	.type __cxa_finalize, %function
	.type __libc_start_main, %function
	.type __cxa_atexit, %function
	.type __cxa_at_quick_exit, %function
	.type __register_atfork, %function
	.type __stack_chk_fail, %function
puts:
printf:
abort:
__cxa_finalize:
__libc_start_main:
__cxa_atexit:
__cxa_at_quick_exit:
__register_atfork:
__stack_chk_fail:
	ret
