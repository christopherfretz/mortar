int foo(void){return 1;} int bar(void){return 2;}
int foo_old(void){return 0;}
__asm__(".symver foo_old, foo@V1");
