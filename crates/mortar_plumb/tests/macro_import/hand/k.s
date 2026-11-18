/* WP-250: the keeper (first on the line) of the groups d.s refers into:
   one group per branch of spec/image.md I-47a's condition. Each member
   starts with a tag the test finds in the output. */
  .text
  .globl _start
_start:
  .4byte 0
  .8byte .Lk_a  /* keeps A's group (SHF_ALLOC) under --gc-sections */
  /* pads: no member lies at offset 0 of its output section */
  .section .debug_macro,"",@progbits
  .ascii "PADPADPAD......."
  .section .debug_types,"",@progbits
  .ascii "PADPADPAD......."
/* R: one member of D's name and size: the redirect */
  .section .debug_macro,"G",@progbits,wp250.r,comdat
  .ascii "Rtag...."
  .8byte 0
r_local:
  .8byte 0
/* N: no member of D's name (a .debug_types of its size) */
  .section .debug_types,"G",@progbits,wp250.n,comdat
  .ascii "Ntag...."
  .8byte 0
/* Z: the member of D's name has another size */
  .section .debug_macro,"G",@progbits,wp250.z,comdat
  .ascii "Ztag...."
  .8byte 0
  .8byte 0
/* T: two members of D's name and size */
  .section .debug_macro,"G",@progbits,wp250.t,comdat,unique,1
  .ascii "T1tag..."
  .8byte 0
  .section .debug_macro,"G",@progbits,wp250.t,comdat,unique,2
  .ascii "T2tag..."
  .8byte 0
/* A: the member of D's name and size is SHF_ALLOC */
  .section .wp250a,"aG",@progbits,wp250.a,comdat
.Lk_a:
  .ascii "Atag...."
  .8byte 0
/* L: the member is not in the output: its group has an allocated member
   nothing refers to, collected under --gc-sections with the group */
  .section .text.wp250l,"axG",@progbits,wp250.l,comdat
  .4byte 0
  .section .debug_macro,"G",@progbits,wp250.l,comdat
  .ascii "Ltag...."
  .8byte 0
