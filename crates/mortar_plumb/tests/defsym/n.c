/* --defsym=k=0x1234 alone (mold links it with its -Map). */
extern char k[];
long pk = (long)k;
void _start(void) {
  for (;;) {
  }
}
