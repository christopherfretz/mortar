int other(int x){ return x - 1; }
int helper(int x){ return other(x) * 11; }
int gtab[4] = {1, 2, 3, 4};
int uf(int x){ return other(x); }
