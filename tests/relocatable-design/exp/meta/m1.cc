struct S { int a; int b; };
inline int get(S* s) { return s->a + s->b; }
int m1(int x) { S s{x, 1}; return get(&s); }
