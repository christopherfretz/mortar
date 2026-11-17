struct T { int a; };
inline int tget(T* t) { return t->a; }
int m3(int x) { T t{x}; return tget(&t); }
