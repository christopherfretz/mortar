#include <stdio.h>
int g_tv(void), g_wtv(void), g_ptv(void);
int main(void) { printf("tv=%d wtv=%d ptv=%d\n", g_tv(), g_wtv(), g_ptv()); return 0; }
