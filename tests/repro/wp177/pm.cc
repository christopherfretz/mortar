#include <cstdio>
#include <string>
#include <stdexcept>
inline std::string greet(int n) { return "p" + std::to_string(n); }
int api1(int); int api2(int);
int main() { try { throw std::logic_error("x"); } catch (const std::exception &e) { std::printf("main caught %s\n", e.what()); }
  std::printf("%d %d %d %s\n", api1(3), api1(-1), api2(4), greet(9).c_str()); return 0; }
