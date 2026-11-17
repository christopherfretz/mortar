/* WP-169: an executable under -E whose archive member --exclude-libs
   excludes (D-110 item 1). */
long xfoo(void);
int dep(void);
extern long xdata;
void _start(void) { xdata = xfoo() + dep(); for (;;) { } }
