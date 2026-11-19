// Unchanged. twice's second copy; fb is main's callee.
inline int twice(int x) { return x * 2; }
int fb(int x) { return twice(x) + 1; }
