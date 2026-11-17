/* A library whose object references names outside the seven: GNU ld's
   `__etext` (and on AArch64 its own `__end__`, `__bss_start__`). The
   functions' names hold the strings the mutations point an export at. */
extern char __etext[];
int one = 1;
void *api__etext(void) { return __etext; }
void *api__executable_start(void) { return 0; }
#ifdef __aarch64__
extern char __end__[], __bss_start__[];
void *api_end2(void) { return __end__; }
void *api_bss2(void) { return __bss_start__; }
#endif
