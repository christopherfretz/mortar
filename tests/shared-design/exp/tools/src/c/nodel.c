/* -z nodelete library: state must survive dlclose + dlopen */
#include <stdio.h>
static int calls;
__attribute__((constructor)) static void c(void) { printf("EV nodelete ctor\n"); fflush(stdout); }
__attribute__((destructor)) static void d(void) { printf("EV nodelete dtor\n"); fflush(stdout); }
int cyc_call(void) { printf("EV cyc_call\n"); fflush(stdout); return ++calls; }
