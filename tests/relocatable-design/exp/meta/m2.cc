struct S { long a; int b; };
inline int get(S* s) { return (int)s->a * s->b; }
int m2(int x) { S s{x, 2}; return get(&s); }
