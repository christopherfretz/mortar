// census library, C++: throws and catches inside the library
#include <cstdio>
#include <stdexcept>
#include <string>
int lib_data = 41;
extern "C" int lib_inited;
int lib_inited = 0;
static int helper(int x) { if (x > 0) throw std::runtime_error(std::to_string(x)); return x; }
extern "C" int lib_func(void) { try { return helper(lib_data); } catch (const std::exception &e) { return std::stoi(e.what()) + 1; } }
extern "C" void my_init(void) { lib_inited = 1; }
extern "C" void my_fini(void) { }
extern "C" int lib_check(void) { std::printf("lib C++ data=%d\n", lib_data); return lib_func(); }
