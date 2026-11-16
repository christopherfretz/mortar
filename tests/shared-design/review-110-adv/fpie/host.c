#include <stdio.h>
extern int g; int getg(void);
int main(void){ g = 99; int v = getg(); printf("%s exe g=%d library reads g=%d\n", v==99?"OK":"FAIL", g, v); return v!=99; }
