/* A library whose object references three of D-21 item 2's seven names
   (initialized data too, so `_edata` ends a `.data`). */
extern char _end[], _edata[], __bss_start[];
int counter;
int one = 1;
void *api_end(void) { return _end; }
void *api_edata(void) { return _edata; }
void *api_bss(void) { return __bss_start; }
