__thread int tls_exported = 4;
static __thread int tls_static = 5;
int get_tls(void) { return tls_exported + tls_static; }
void bump(void) { tls_static++; }
