int shared_c[4];
static const char b_name[] = "b-unit";
static int b_helper(int v) { return v ^ b_name[1]; }
int bfun(int v) { return b_helper(v) + shared_c[2]; }
