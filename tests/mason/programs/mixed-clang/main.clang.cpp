// Clang main; kiln code throws through GCC frames into Clang code; one
// inline function's local static shared by all three compilers' objects.
#include <cstdio>
#include <stdexcept>
#include <string>
inline std::string &tag() { static std::string t = "init"; return t; }
int gcc_relay(int n);
std::string gcc_tag();
int main() {
    std::printf("relay %d\n", gcc_relay(0));
    try { gcc_relay(9); } catch (const std::invalid_argument &e) { std::printf("clang caught %s\n", e.what()); }
    tag() += "-clang";
    std::printf("tag %s\n", gcc_tag().c_str());
    return 0;
}
