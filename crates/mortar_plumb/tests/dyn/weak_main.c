#include <stdio.h>
extern void hook(void) __attribute__((weak));
extern int wv __attribute__((weak));
void (*hp)(void) = hook;            /* absolute word in writable data (AbsWord) */
int main(void) {
  if (hook) { hook(); puts("main: hook non-null, called"); } else puts("main: hook absent");
  printf("hp==hook %d; wv %d\n", hp == hook, &wv ? wv : -1);
  return 0;
}
