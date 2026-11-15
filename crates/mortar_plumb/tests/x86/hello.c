#include <stdio.h>
#include <string.h>
int main(int c, char **v) { printf("%zu\n", strlen(v[0]) + c); return 0; }
