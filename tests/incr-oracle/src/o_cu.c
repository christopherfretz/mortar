int cbuf[4];
int lf(int);
int uf(int x){ cbuf[0] = x; return lf(x) + cbuf[1]; }
int after_common[2] = {7, 8};
