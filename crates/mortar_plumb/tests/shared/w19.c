/* W19 (spec/dyn.md §17.7): local-exec TLS against the library's own
   static TLS, compiled -fPIC -ftls-model=local-exec (AArch64). The
   setter keeps the access a TLS one. */
static __thread int t = 5;
int w19_get(void) { return t; }
void w19_set(int v) { t = v; }
