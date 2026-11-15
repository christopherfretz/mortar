/* address of the same ifunc taken from another TU (extern: GOT in PIC/PIE code) */
#include <stdio.h>
extern int ifq(void); extern void *ifq_addr_a(void);
int main(void) { void *b = (void *)&ifq, *a = ifq_addr_a();
  printf("ifunc-ptr-eq a=%p b=%p call_a=%d call_b=%d %s\n", a, b, ((int (*)(void))a)(), ((int (*)(void))b)(), a == b ? "PASS" : "FAIL"); return a != b; }
