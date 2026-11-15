#include <string>
#include <stdexcept>
thread_local std::string tl_s = "abc";
static thread_local int tl_i = 4;
extern thread_local int tl_ext;
int f2(int x) { if (x > 10) throw std::runtime_error("x"); tl_i += x; return tl_s.size() + tl_i + tl_ext; }
