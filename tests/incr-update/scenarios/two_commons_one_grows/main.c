#include <stdio.h>
extern char buf[];
extern int counter;
int a_size(void);
void bump(void);
int main(void) {
  buf[3] = 7;
  bump();
  printf("size=%d buf3=%d counter=%d\n", a_size(), buf[3], counter);
  return 0;
}
