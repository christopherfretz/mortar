__attribute__((weak)) int gfn(int);
extern int gw __attribute__((weak));
int uf(int x){ return (gfn ? gfn(x) : -1) + (&gw ? gw : -100); }
