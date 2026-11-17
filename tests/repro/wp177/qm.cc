#include <cstdio>
#include <stdexcept>
int api1(int);
int main() {
  try { throw std::logic_error("x"); } catch (const std::exception &) { std::puts("main caught logic_error"); }
  int r;
  try { r = api1(1); std::printf("api1(1) returned %d (wrong: int caught as std::exception)\n", r); }
  catch (int i) { std::printf("main caught int %d (right)\n", i); }
  return 0;
}
