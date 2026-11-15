/* compiled -fPIC with the traditional dialect: GD to a .so symbol, GD to an exe symbol */
extern __thread int tlsx, exe_tls;
int tls_gd_read(void) { return tlsx; }
int *tls_gd_addr(void) { return &tlsx; }
int tls_gd_exe(void) { return exe_tls; }
