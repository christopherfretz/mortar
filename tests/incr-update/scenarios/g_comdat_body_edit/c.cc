// The changed file: one immediate (EDIT: 9 in place of 7) on the same line.
#include "t.h"
#ifndef EDIT
#define C_K 7
#else
#define C_K 9
#endif
int c_value(int x) {
  int local = twice(x) + scale(x);
  return local + C_K;
}
