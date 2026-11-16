static int impl(void){return 42;}
static void *res(void){return (void*)impl;}
__attribute__((visibility("protected"))) int f(void) __attribute__((ifunc("res")));
void *lib_addr(void){return (void*)f;}
void *lib_data = (void*)f;
int lib_call(void){return f();}
