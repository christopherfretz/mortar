int old_impl(void){return 1;} int cur_impl(void){return 2;} int bar(void){return 3;} int foo(void){return 4;} int fop(void){return 5;}
__asm__(".symver old_impl,old@V1");
__asm__(".symver cur_impl,cur@@V1");
