/* W41: an object compiled -mno-direct-extern-access (x86-64), which
   marks it GNU_PROPERTY_1_NEEDED_INDIRECT_EXTERN_ACCESS. */
extern int imp(void);
int w41_call(void) { return imp(); }
