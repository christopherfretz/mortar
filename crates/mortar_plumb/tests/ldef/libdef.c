/* A library that DEFINES the names as ordinary data (WP-150 §3a, item 2). */
char _end[1], _edata[1], __bss_start[1];
void *lib_end(void) { return _end; }
