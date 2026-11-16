int lf(int);
const char *us(void){ return "unchanged string"; }
const char *us2(void){ return "another one"; }
int uf(int x){ return lf(x) + us()[x & 7] + us2()[1]; }
