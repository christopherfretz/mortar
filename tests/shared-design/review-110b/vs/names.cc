extern "C" { int foo(void){return 1;} int foo2(void){return 2;} int fop(void){return 3;} int bar(void){return 4;} int baz(void){return 5;} int fa(void){return 6;} }
int foo(int x){return x;}
namespace ns { int foo(){return 7;} template<class T> int bar(){return 8;} template int bar<int>(); }
