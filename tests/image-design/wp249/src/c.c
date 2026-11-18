#include <string.h>
#include "h.h"
int c_f(int x) {
  struct wp249_point q = { WP249_TWICE(x), 3 };
  return q.x + (int)strlen("c");
}
