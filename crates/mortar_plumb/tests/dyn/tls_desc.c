/* compiled -fPIC with the descriptor dialect: GD to a .so symbol, GD to an exe symbol */
extern __thread int tlsx, exe_tls;
int tls_desc_read(void) { return tlsx; }
int *tls_desc_addr(void) { return &tlsx; }
int tls_desc_exe(void) { return exe_tls; }
