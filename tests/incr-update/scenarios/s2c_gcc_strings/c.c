extern void put(const char *);
#ifndef C
int k(void) { put("shared line!\n"); return 10; }
#else
int k(void) { put("shared line!\n"); return 20; }
#endif
