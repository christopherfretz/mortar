#include <stdio.h>
#include "h.h"
#define WP249_A_ONLY 1 /* a macro of a.c only: not defined in b */
int main(void) {
  struct wp249_point p = { 1, 2 };
  int r = b_f(p.x) + c_f((int)p.y);
  printf("%d %d %d\n", r, g_b, WP249_TWICE(WP249_ANSWER));
  return 0;
}
