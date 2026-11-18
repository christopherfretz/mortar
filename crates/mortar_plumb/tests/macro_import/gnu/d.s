/* WP-249 step 1 rule.sh's d.s (GNU ld's kept-section condition; WP-250
   judges GNU ld's, lld's and mold's links of it). Assembled with
   --defsym KREF=1 for the KREF variant. */
  .section .debug_macro,"G",@progbits,c1,comdat
.Lc1:
  .ascii "C1tag..."
  .8byte 0
  .section .debug_macro,"G",@progbits,c2,comdat
.Lc2:
  .ascii "C2tag..."
  .8byte 0
  .8byte 0
  .section .debug_macro,"G",@progbits,c3,comdat
.Lc3:
  .ascii "C3drop.."
  .8byte 0
  .section .debug_macro,"G",@progbits,c4,comdat
.Lc4:
  .ascii "C4macro."
  .8byte 0
  .section .debug_macro,"G",@progbits,c5
.Lc5:
  .ascii "C5drop.."
  .8byte 0
  .section .debug_macro,"G",@progbits,c7,comdat
  .ascii "C7tag..."
  .8byte 0
  .globl c7_global
c7_global:
  .8byte 0
  .section .debug_macro,"G",@progbits,c8,comdat
  .ascii "C8tag..."
  .8byte 0
c8_local:
  .8byte 0
  .section .debug_line,"G",@progbits,c10,comdat
.Lc10:
  .ascii "C10tag.."
  .8byte 0
  .section .debug_macro,"G",@progbits,c11,comdat
.Lc11:
  .ascii "C11tag.."
  .4byte .Lds
  .4byte 0
  .section .debug_macro,"G",@progbits,wm4.c14,comdat
.Lc14:
  .ascii "C14drop."
  .8byte 0
  .section .debug_macro,"G",@progbits,c13,comdat
.Lc13:
  .ascii "C13tag.."
c13_d:
  .8byte 0
  .section .debug_str,"",@progbits
.Lds:
  .asciz "wp249"
  .section .debug_macro,"",@progbits
  .ascii "WP249REF"
  .4byte .Lc1            /* C1 same name, same size, same bytes */
  .4byte .Lc1+4          /* C1+4 the same with an addend */
  .4byte .Lc2            /* C2 same name, other size */
  .4byte .Lc3            /* C3 same name and size, other bytes */
  .4byte .Lc4            /* C4 the kept group has no member of this name */
  .4byte .Lc5            /* C5 a group without GRP_COMDAT */
  .4byte c7_global+0     /* C7 a global symbol in the member (resolves to the kept copy's) */
  .4byte c8_local        /* C8 a local named symbol in the member */
  .8byte .Lc1            /* C9 a 64-bit word, same as C1 */
  .4byte .Lc10           /* C10 a .debug_line group member */
  .4byte .Lc11           /* C11 a member with its own relocation (a .rela member in the group) */
  .4byte .Lc13           /* C13 a member whose local named symbols differ (c13_k, c13_d) */
  .4byte .Lc14           /* C14 C3 (other bytes, same size) under a GCC-style signature wm4.* */
  .section .debug_info,"",@progbits
  .ascii "WP249REF"
  .4byte .Lc1            /* I1 another referring name, same as C1 */
