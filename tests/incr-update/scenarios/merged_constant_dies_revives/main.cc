#include <cstdio>
const char *c_str();
double c_num(double);
int main() {
  std::printf("%s %.1f\n", c_str(), c_num(2.0));
  return 0;
}
