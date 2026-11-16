static int compute() { return 41; }
static int s_init = compute();
int lf(int x){ return x + s_init; }
