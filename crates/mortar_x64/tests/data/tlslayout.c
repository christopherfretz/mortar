#include <stdio.h>
#include <stdint.h>
#ifdef TDATA
__thread int d1 = 7;
#endif
__thread char b1;
__thread char big[100] __attribute__((aligned(ALIGN)));
__thread int b2;
static inline uintptr_t tp(void){uintptr_t t; __asm__("mov %%fs:0,%0":"=r"(t)); return t;}
int main(void){
  uintptr_t t=tp();
  printf("align=%d big=%ld b1=%ld b2=%ld mod=%lu", ALIGN, (long)((uintptr_t)big-t),(long)((uintptr_t)&b1-t),(long)((uintptr_t)&b2-t),(unsigned long)((uintptr_t)big%ALIGN));
#ifdef TDATA
  printf(" d1=%ld v=%d", (long)((uintptr_t)&d1-t), d1);
#endif
  printf("\n"); return 0; }
