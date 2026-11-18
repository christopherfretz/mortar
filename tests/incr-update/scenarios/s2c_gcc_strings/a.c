/* A: the two calls swapped (the strings' order in .rodata.str1.1 too). */
extern void put(const char *);
#ifndef A
void pa(void) { put("hello from A\n"); put("shared line!\n"); }
#else
void pa(void) { put("shared line!\n"); put("hello from A\n"); }
#endif
