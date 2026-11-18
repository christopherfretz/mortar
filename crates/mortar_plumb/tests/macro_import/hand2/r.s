/* WP-250: a dropped copy of k.s's group X referred to from sections
   I-47a's referrer rule excludes: .wp250note (no SHF_ALLOC, no debug
   name: I-46) and .wp250data (SHF_ALLOC); each starts with WP250REF.
   mortar refuses both links by name; the test links k.o r.o with lld
   and reads its map as mortar's. */
  .section .debug_macro,"G",@progbits,wp250.x,comdat
.Lr_x:
  .ascii "Xtag...."
  .8byte 0
  .section .wp250note,"",@progbits
  .ascii "WP250REF"
  .4byte .Lr_x
  .section .wp250data,"aR",@progbits
  .ascii "WP250REF"
  .4byte .Lr_x
