__thread long tu_var = 4;
int lf(int);
int uf(int x){ tu_var += x; return lf(x) + (int)tu_var; }
