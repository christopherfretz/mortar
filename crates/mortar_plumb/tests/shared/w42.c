/* W42: a versioned reference vf@VA. */
extern int vf(void);
__asm__(".symver vf,vf@VA");
int w42_call(void) { return vf(); }
