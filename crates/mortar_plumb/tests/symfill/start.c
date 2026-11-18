/* WP-222's program (tests/symfill.rs): -nostdlib, static. */
extern int afun(int);
extern int bfun(int);
extern int shared_c[4];
static int sticky;
static int twice(int v) { return v * 2 + sticky; }
void _start(void) {
  sticky = afun(3) + bfun(4) + shared_c[1];
  for (;;)
    sticky = twice(sticky);
}
