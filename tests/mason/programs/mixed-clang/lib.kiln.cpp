#include <cstdio>
#include <stdexcept>
#include <string>
inline std::string &tag() { static std::string t = "init"; return t; }
std::string kiln_tag(const char *s) { tag() += s; return tag(); }
void kiln_throw(int n) { if (n) throw std::invalid_argument("kiln invalid " + std::to_string(n)); }
