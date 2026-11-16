#include <stdio.h>
#include <string.h>
extern const int ro; extern const char *const rr; extern int rw, bz;
int get_ro(void); const char *get_rr(void); int get_rw(void); void set_rw(int); int get_bz(void); void set_bz(int);
int main(void) {
  printf("ro %s ", (ro == 1 && get_ro() == 1) ? "OK" : "MISLINK");
  printf("rr %s ", (strcmp(rr, "abc") == 0 && rr == get_rr()) ? "OK" : "MISLINK");
  set_rw(9); printf("rw %s ", (rw == 9 && get_rw() == 9) ? "OK" : "MISLINK");
  bz = 4; printf("bz %s\n", get_bz() == 4 ? "OK" : "MISLINK");
  return 0; }
