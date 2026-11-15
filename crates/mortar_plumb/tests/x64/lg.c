/* W9: GCC -mcmodel=medium -fcommon: lc is SHN_X86_64_LCOMMON. */
#include <stdio.h>
#ifndef BIG
#define BIG (1<<20)
#endif
static char lb[BIG];                 /* .lbss */
char ld_[BIG] = {1};                 /* .ldata */
const char lr[1<<20] = {2};          /* .lrodata */
char lc[BIG];                        /* LCOMMON with -fcommon */
int small_d = 3; int small_b; const int small_r = 4;
int main(void){ lb[BIG-1]=5; lc[BIG-1]=6; printf("%d %d %d %d %d %d %d\n", lb[BIG-1], ld_[0], lr[0], lc[BIG-1], small_d, small_b, small_r); return 0; }
