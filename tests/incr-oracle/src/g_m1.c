#include <stdio.h>
extern const int __start_ureg[], __stop_ureg[];
int keepf(int); int dropf(int); int gfun(void);
int main(void){ int s = 0; for (const int *p = __start_ureg; p < __stop_ureg; p++) s += *p; printf("%d %d %d %d\n", s, keepf(2), gfun(), dropf(2)); return 0; }
