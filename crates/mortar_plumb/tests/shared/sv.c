__asm__(".symver foo_v1, foo@V1");
__asm__(".symver foo_v2, foo@@V2");
int foo_v1(void) { return 1; }
int foo_v2(void) { return 2; }
