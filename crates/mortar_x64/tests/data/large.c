/* WP-084: the medium and large code models' sections (.lbss, .ldata,
   .lrodata; .ltext from Clang -mcmodel=large) beside small ones. */
#include <stdio.h>
#include <string.h>
#define BIG (1 << 20)
static char lb[BIG];
char ld_[BIG] = {1};
const char lr[BIG] = {2};
int small_d = 3;
int small_b;
const int small_r = 4;
static const char *strs[] = {"a", "b"};
static int (*fp)(const char *) = puts;
__attribute__((noinline)) int large_fn(int x) { return x + lr[0]; }
int main(void) {
  lb[BIG - 1] = 5;
  fp("large");
  printf("%d %d %d %d %d %d %s %zu %d\n", lb[BIG - 1], ld_[0], lr[0], small_d, small_b, small_r,
         strs[1], strlen(strs[0]), large_fn(40));
  return 0;
}
