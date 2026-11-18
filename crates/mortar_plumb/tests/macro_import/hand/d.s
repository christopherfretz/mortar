/* WP-250: every group here is a copy of one of k.s's, dropped (k.o is
   first). The non-group .debug_macro and .debug_info refer into the
   copies; each starts with the marker WP250REF, then the words. */
  .section .debug_macro,"G",@progbits,wp250.r,comdat
.Ld_r:
  .ascii "Rtag...."
  .8byte 0
r_local:
  .8byte 0
  .section .debug_macro,"G",@progbits,wp250.n,comdat
.Ld_n:
  .ascii "Ntag...."
  .8byte 0
  .section .debug_macro,"G",@progbits,wp250.z,comdat
.Ld_z:
  .ascii "Ztag...."
  .8byte 0
  .section .debug_macro,"G",@progbits,wp250.t,comdat
.Ld_t:
  .ascii "T1tag..."
  .8byte 0
  .section .wp250a,"G",@progbits,wp250.a,comdat
.Ld_a:
  .ascii "Atag...."
  .8byte 0
  .section .text.wp250l,"axG",@progbits,wp250.l,comdat
  .4byte 0
  .section .debug_macro,"G",@progbits,wp250.l,comdat
.Ld_l:
  .ascii "Ltag...."
  .8byte 0
  .section .debug_macro,"",@progbits
  .ascii "WP250REF"
  .4byte .Ld_r       /* +8  R: K + 0 */
  .4byte .Ld_r+4     /* +12 R: K + 4 */
  .4byte r_local     /* +16 R: K + 16 (a local named symbol) */
  .8byte .Ld_r       /* +20 R: K + 0, 64-bit */
  .4byte .Ld_n       /* +28 N: tombstone */
  .4byte .Ld_z       /* +32 Z: tombstone */
  .4byte .Ld_t       /* +36 T: tombstone */
  .4byte .Ld_a       /* +40 A: tombstone */
  .4byte .Ld_l       /* +44 L: tombstone */
  .section .debug_info,"",@progbits
  .ascii "WP250REF"
  .4byte .Ld_r       /* +8  R from another debug name: K + 0 */
