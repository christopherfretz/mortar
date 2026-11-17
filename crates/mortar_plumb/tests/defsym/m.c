/* --defsym=k=0x1234 --defsym=a=b --defsym=c=b+4: words holding k, c and
   a, a call to a, b and g. */
extern char k[], c[];
int a(void);
int b(void);
int g(void) { return 3; }
long pk = (long)k;
long pc = (long)c;
long pa = (long)a;
long rk(void) { return (long)k; }
void _start(void) {
  a();
  b();
  g();
  rk();
  for (;;) {
  }
}
