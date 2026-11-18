#include <stdio.h>
#include "h.h"
int g_b = 7;
int b_f(int x) {
  struct wp249_point q = { x, WP249_ANSWER };
  puts("b");
  return q.x + (int)q.y;
}
