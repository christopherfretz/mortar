// Unchanged: the other copy of `shared`.
inline int shared(int x) { return x * 7 + 1; }
int u_use(int x) { return shared(x) + 1; }
