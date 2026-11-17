#include <stdexcept>
int thrower(int x) {
  if (x > 0) throw std::runtime_error("boom");
  return x;
}
