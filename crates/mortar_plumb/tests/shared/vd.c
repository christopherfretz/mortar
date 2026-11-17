/* WP-149 (P-D27, D-115): version definitions of a shared object. `bar`
   gets V1, `baz` and `call_ext` V2, `q` V3 under vd3.map; `foo@V1`
   (hidden) and `foo@@V2` by .symver; `hid` is made local; `ext` is an
   import of libvdx.so's `ext@@X1` (a Vernaux after the Verdefs). */
int bar(void) { return 1; }
int baz(void) { return 2; }
int hid(void) { return 3; }
int q(void) { return 4; }
int foo_v1(void) { return 10; }
int foo_v2(void) { return 20; }
__asm__(".symver foo_v1, foo@V1");
__asm__(".symver foo_v2, foo@@V2");
extern int ext(void);
int call_ext(void) { return ext() + hid(); }
