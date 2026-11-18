#include "h.h"
Shape::~Shape() {}
int Shape::area() const { return w * w; }
int c_f(int x) {
  Box<long> bl{x};
  int k = x * 3 + (int)bl.get();
  return k + 1;
}
