#include "h.h"
#define WP250_A_ONLY 1 /* a macro of a.c only */
int main(void) {
  struct wp250_point p = { 1, 2 };
  return b_f(p.x) + c_f((int)p.y) + g_b + WP250_TWICE(WP250_ANSWER) + WP250_A_ONLY;
}
