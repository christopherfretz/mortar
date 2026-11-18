/* WP-250: copies of k.s's groups, dropped (k.o is first), and the
   sections that refer into them; each referrer starts with WP250REF. */
  .section .debug_wp250m,"MSG",@progbits,1,wp250.m,comdat
m_d:
  .asciz "Mtag..."
  .section .debug_wp250k,"G",@progbits,wp250.mk,comdat
mk_d:
  .asciz "MKtag.."
  .section .debug_wp250d,"MSG",@progbits,1,wp250.md,comdat
md_d:
  .asciz "MDtag.."
  .section .debug_info,"G",@progbits,wp250.i,comdat
.Ld_i:
  .ascii "Itag...."
  .8byte 0
  .section .debug_macro,"G",@progbits,wp250.x,comdat
.Ld_x:
  .ascii "Xtag...."
  .8byte 0
  .section .debug_macro,"",@progbits
  .ascii "WP250REF"
  .4byte m_d         /* +8  M: tombstone (both SHF_MERGE) */
  .4byte mk_d        /* +12 MK: tombstone (the keeper's SHF_MERGE) */
  .4byte md_d        /* +16 MD: tombstone (the dropped one's SHF_MERGE) */
  .4byte .Ld_x       /* +20 X: K + 0 (the debug referrer of X) */
  .section .debug_gnu_pubnames,"",@progbits
  .ascii "WP250REF"
  .4byte .Ld_i       /* +8 I: K + 0, a .debug_info member */
