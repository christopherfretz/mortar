/* a is replaced by --defsym=a=b; self's call to a reaches b. a is the
   first function of .text. */
int a(void) { return 1; }
int self(void) { return a(); }
