/* a libtool library depending on another libtool library */
extern int foo(void);
int dep(void) { return foo() + 1; }
