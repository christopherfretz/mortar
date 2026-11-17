// mainx.cc: the final link's object for the C++ component.
#include <cstdio>
int x1_value(); int x2_value(); int x2_catch(int);
int main() {
  std::printf("x1=%d x2=%d\n", x1_value(), x2_value());
  int c = x2_catch(7);
  try { x2_catch(0); throw 5; } catch (int v) { std::printf("main caught %d\n", v); }
  std::printf("c=%d\n", c);
  return 0;
}
