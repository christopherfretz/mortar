// lib.a's member: calls m1 (the cycle).
int m1_fn(int);
int m2_fn(int x) { return x > 0 ? m1_fn(x - 1) + 10 : 0; }
