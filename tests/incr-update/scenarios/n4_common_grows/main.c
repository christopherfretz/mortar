#include <stdio.h>
extern char buf[];
int a_size(void);
int main(void) {
  buf[3] = 7;
  printf("size=%d buf3=%d\n", a_size(), buf[3]);
  return 0;
}
