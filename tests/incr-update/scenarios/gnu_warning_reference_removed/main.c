#include <stdio.h>
int c_fn(void);
int u_fn(void);
int main(void) {
  printf("%d %d\n", c_fn(), u_fn());
  return 0;
}
