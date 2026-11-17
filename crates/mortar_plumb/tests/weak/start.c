/* -nostdlib entry points of WP-165's links: bo (a branch alone), split
   (a branch and an unfillable use), w12 (a GOT use and an unfillable use). */
void *taddr(void);
void *gaddr(void);
int ccall(void);
int m2fn(void);
volatile long sink;
#if defined(BO)
void _start(void) { sink = ccall() + m2fn(); for (;;) {} }
#elif defined(SPLIT)
void _start(void) { sink = (long)taddr() + ccall() + m2fn(); for (;;) {} }
#else
void _start(void) { sink = (long)taddr() + (long)gaddr() + m2fn(); for (;;) {} }
#endif
