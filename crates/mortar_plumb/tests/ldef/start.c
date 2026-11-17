/* A program that references none of the seven; it needs the library. */
void *api_end(void);
void *lib_end(void);
void _start(void) {
#ifdef DEF
  lib_end();
#else
  api_end();
#endif
  for (;;) {}
}
