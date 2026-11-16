#include <cstdio>
#include <stdexcept>
extern "C" int cl_value(void);
int cxl_value(int);
int main() {
  int ok = cxl_value(2) == 42 && cl_value() == 40;
  try { cxl_value(-1); ok = 0; } catch (const std::invalid_argument &) {}
  std::printf("%s cxl_value(2)=%d\n", ok ? "OK" : "FAIL", cxl_value(2));
  return !ok;
}
