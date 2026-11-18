#include <stdio.h>
void put(const char *s) { fputs(s, stdout); }
void pa(void);
int k(void);
int main(void) {
  pa();
  printf("k=%d\n", k());
  return 0;
}
