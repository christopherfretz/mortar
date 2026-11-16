__thread long tl_new = 9;
int lf(int x){ tl_new += x; return x + 1 + (int)tl_new; }
