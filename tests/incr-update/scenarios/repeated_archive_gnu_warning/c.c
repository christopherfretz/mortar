/* liby.a(c.o): refers to the warned foo. MEMBER: its body changes, the
   reference stays. */
int foo(int);
int bar(int);
#ifdef MEMBER
int cfun(void) { return foo(3) + bar(4) + 1; }
#else
int cfun(void) { return foo(1) + bar(2); }
#endif
