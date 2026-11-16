int foo(void){ return 1; }
int bar(void){ return foo() + 1; }
void *addr(void){ return (void*)foo; }
int dat = 3;
int getdat(void){ return dat; }
