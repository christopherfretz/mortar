/* WP-250 (the supervisor's amendment to spec/image.md I-47a, and the
   referring section): the keeper (first on the line) of the groups d.s
   refers into. M: both members SHF_MERGE; MK: only the keeper's; MD:
   only the dropped one's; I: a .debug_info member (the shape of
   split/gdb-index-refused.test's x-group2); X: an ordinary member,
   referred to from d.s's .debug_macro and from r.s's sections without a
   debug name or with SHF_ALLOC. */
  .text
  .globl _start
_start:
  .4byte 0
  .section .debug_info,"",@progbits
  .ascii "PADPADPAD......."
  .section .debug_macro,"",@progbits
  .ascii "PADPADPAD......."
  .section .debug_wp250m,"MSG",@progbits,1,wp250.m,comdat
  .asciz "Mtag..."
  .section .debug_wp250k,"MSG",@progbits,1,wp250.mk,comdat
  .asciz "MKtag.."
  .section .debug_wp250d,"G",@progbits,wp250.md,comdat
  .asciz "MDtag.."
  .section .debug_info,"G",@progbits,wp250.i,comdat
  .ascii "Itag...."
  .8byte 0
  .section .debug_macro,"G",@progbits,wp250.x,comdat
  .ascii "Xtag...."
  .8byte 0
