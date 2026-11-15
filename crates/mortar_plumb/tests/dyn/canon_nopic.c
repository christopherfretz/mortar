#include <stdio.h>
#include <stdlib.h>
void *addr_puts_nopic(void) { return (void *)&puts; }
void *addr_qsort_nopic(void) { return (void *)&qsort; }
