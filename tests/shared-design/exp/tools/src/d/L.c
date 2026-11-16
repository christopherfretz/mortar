/* L: default-visibility f and v that L itself calls/reads (PLT/GOT, preemptible) */
int f(void) { return 1; }
int v = 10;
int L_calls_f(void) { return f(); }
int L_reads_v(void) { return v; }
void *L_addr_f(void) { return (void *)&f; }
void *L_addr_v(void) { return (void *)&v; }
