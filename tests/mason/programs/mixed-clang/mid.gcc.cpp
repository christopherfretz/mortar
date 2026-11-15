#include <cstdio>
#include <string>
inline std::string &tag() { static std::string t = "init"; return t; }
void kiln_throw(int n);
std::string kiln_tag(const char *s);
int gcc_relay(int n) { struct G { ~G() { std::puts("gcc relay unwound"); } } g; kiln_throw(n); return n; }
std::string gcc_tag() { tag() += "-gcc"; return kiln_tag("-kiln"); }
