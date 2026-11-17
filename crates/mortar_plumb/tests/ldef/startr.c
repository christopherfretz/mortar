/* A program whose object references `_end` (a GOT slot in a PIE). */
extern char _end[], _edata[], __bss_start[];
void *p[3];
int one = 1;
void _start(void) {
  p[0] = _end; p[1] = _edata; p[2] = __bss_start;
  for (;;) {}
}
