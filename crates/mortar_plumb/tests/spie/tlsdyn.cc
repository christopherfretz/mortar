// thread_local objects with constructors and destructors.
#include <cstdio>
#include <string>
struct G { int v; G() : v(5) { std::puts("init"); } ~G() { std::puts("fini"); } };
thread_local G g;
thread_local int plain = 3;
int main(){ return g.v + plain == 8 ? 0 : 1; }
