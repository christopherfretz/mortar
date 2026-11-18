/* WP-249 step 1 rule.sh's k.s (GNU ld's kept-section condition; WP-250
   judges GNU ld's, lld's and mold's links of it). Assembled with
   --defsym KREF=1 for the KREF variant. */
  .text
  .globl _start
_start:
  .4byte 0
  /* pads: no tagged copy lies at offset 0 of its output section */
  .section .debug_macro,"",@progbits
  .ascii "PADPADPAD......."
  .section .debug_types,"",@progbits
  .ascii "PADPADPAD......."
  .section .debug_line,"",@progbits
  .ascii "PADPADPAD......."
  .section .debug_macro,"G",@progbits,c1,comdat
.Lk_c1:
  .ascii "C1tag..."
  .8byte 0
  .section .debug_macro,"G",@progbits,c2,comdat
.Lk_c2:
  .ascii "C2tag..."
  .8byte 0
  .section .debug_macro,"G",@progbits,c3,comdat
.Lk_c3:
  .ascii "C3keep.."
  .8byte 0
  .section .debug_types,"G",@progbits,c4,comdat
.Lk_c4:
  .ascii "C4types."
  .8byte 0
  .section .debug_macro,"G",@progbits,c5
.Lk_c5:
  .ascii "C5keep.."
  .8byte 0
  .section .debug_macro,"G",@progbits,c7,comdat
.Lk_c7:
  .ascii "C7tag..."
  .8byte 0
  .globl c7_global
c7_global:
  .8byte 0
  .section .debug_macro,"G",@progbits,c8,comdat
.Lk_c8:
  .ascii "C8tag..."
  .8byte 0
c8_local:
  .8byte 0
  .section .debug_line,"G",@progbits,c10,comdat
.Lk_c10:
  .ascii "C10tag.."
  .8byte 0
  .section .debug_macro,"G",@progbits,c11,comdat
.Lk_c11:
  .ascii "C11tag.."
  .4byte .Lks
  .4byte 0
  .section .debug_macro,"G",@progbits,wm4.c14,comdat
.Lk_wm4_c14:
  .ascii "C14tag.."
  .8byte 0
  .section .debug_macro,"G",@progbits,c13,comdat
.Lk_c13:
  .ascii "C13tag.."
c13_k:
  .8byte 0
.ifdef KREF
  /* k.o also refers to its own copies, as a GCC unit does: each copy
     then has a section symbol */
  .section .debug_info,"",@progbits
  .4byte .Lk_c1
  .4byte .Lk_c2
  .4byte .Lk_c3
  .4byte .Lk_c4
  .4byte .Lk_c5
  .4byte .Lk_c7
  .4byte .Lk_c8
  .4byte .Lk_c10
  .4byte .Lk_c11
  .4byte .Lk_wm4_c14
  .4byte .Lk_c13
.endif
  .section .debug_str,"",@progbits
.Lks:
  .asciz "wp249"
