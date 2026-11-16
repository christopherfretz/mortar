int old_impl(void){return 1;} int cur(void){return 2;} int bar(void){return 3;}
__asm__(".symver old_impl,old@V1");
