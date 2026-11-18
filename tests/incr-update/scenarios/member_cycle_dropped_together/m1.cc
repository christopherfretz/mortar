// lib.a's member: calls m2, has a constructor.
extern int g_ctor;
int m2_fn(int);
int m1_fn(int x) { return x > 0 ? m2_fn(x - 1) + 1 : 0; }
static struct M1 { M1() { g_ctor += 100; } } m1_ctor;
