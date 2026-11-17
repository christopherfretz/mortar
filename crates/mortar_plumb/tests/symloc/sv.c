/* WP-169: two .symver definitions of foo. */
__asm__(".symver foo_v1, foo@V1");
__asm__(".symver foo_v2, foo@@V2");
int foo_v1(int x) { return x + 1; }
int foo_v2(int x) { return x + 2; }
