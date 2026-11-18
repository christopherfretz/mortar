int shared_c[4];
static int a_local = 5;
static int a_helper(int v) { return v + a_local; }
int afun(int v) { return a_helper(v) + shared_c[0]; }
