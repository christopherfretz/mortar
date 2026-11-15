/* The driver of the relaxation sites (tests/sites/mod.rs): each check is a
   pair (value computed through the relocation under test, value computed
   without it) in `res`. */
#include <stdio.h>
extern int run_checks(void);
extern unsigned res[];
extern const char *names[];
int main(void) {
  int n = run_checks(), f = 0;
  for (int i = 0; i < n; i++)
    if (res[2 * i] != res[2 * i + 1]) {
      f++;
      printf("FAIL %s: got %#x want %#x\n", names[i], res[2 * i], res[2 * i + 1]);
    }
  printf("%d checks, %d failures\n", n, f);
  return f != 0;
}
