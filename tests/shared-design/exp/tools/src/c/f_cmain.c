#include <stdio.h>
extern int f_add(int);
extern struct { int ca, cb; } cblk;
int main(void)
{
    cblk.ca = 3; cblk.cb = 4;
    int r = f_add(5);
    printf("%s C main f_add(5)=%d (expect 39)\n", r == 39 ? "OK" : "FAIL", r);
    return 0;
}
