extern "C" int foo(void) { return 1; }
extern "C" int fop(void) { return 2; }
extern "C" int bar(void) { return 3; }
int foo(int x) { return x; }
namespace ns { int foo() { return 4; } }
