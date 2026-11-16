// libtool-shaped C++ library: throws across the boundary, has a static constructor
#include <stdexcept>
#include <string>
static std::string ctor_ran = "no";
struct Init { Init() { ctor_ran = "yes"; } } init_object;
const char *bar_ctor() { return ctor_ran.c_str(); }
int bar(int x) { if (x < 0) throw std::runtime_error("bar: negative " + std::to_string(x)); return x * 2; }
