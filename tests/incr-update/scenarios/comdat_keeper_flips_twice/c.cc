// The changed file (first in link order: the keeper). EDIT: no `shared`.
#ifndef EDIT
inline int shared(int x) { return x * 7 + 1; }
int c_use(int x) { return shared(x); }
#else
int c_use(int x) { return x * 7 + 1; }
#endif
