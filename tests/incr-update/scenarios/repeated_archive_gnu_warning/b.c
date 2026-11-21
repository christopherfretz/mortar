/* libb.a(b.o): extracted for c.o's bar; calls back into c.o (the reason
   for the second mention of liby.a). */
int cfun(void);
int bar(int x) { return x > 0 ? x : cfun(); }
