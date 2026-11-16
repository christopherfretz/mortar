/* q10b: exported data of a library that binds it locally (-Bsymbolic), one per place kind. */
const int ro = 1;                       /* .rodata: read-only PT_LOAD */
const char *const rr = "abc";           /* .data.rel.ro: writable PT_LOAD inside PT_GNU_RELRO */
int rw = 3;                             /* .data: writable, outside RELRO */
int bz;                                 /* .bss: writable, outside RELRO */
int get_ro(void) { return ro; } const char *get_rr(void) { return rr; }
int get_rw(void) { return rw; } void set_rw(int v) { rw = v; } int get_bz(void) { return bz; } void set_bz(int v) { bz = v; }
