#include <stdio.h>
int ext_data = 40; int ext_arr[4] = {0, 2, 0, 0};
__thread int ext_tls = 100;
__thread int ie_tls = 5;
void ext_fn(int v){ printf("ext_fn %d\n", v); }
