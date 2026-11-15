/* references a TLS symbol defined by the executable */
extern __thread int exe_tls;
int lib_get_exe_tls(void) { return exe_tls; }
void *libt_exe_tls_addr(void) { return &exe_tls; }
