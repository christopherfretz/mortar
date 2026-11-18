#include <stdio.h>
int c_fn(void);
int u_fn(void);
int main(void) {
  printf("c_fn=%d u_fn=%d\n", c_fn(), u_fn());
  return 0;
}
