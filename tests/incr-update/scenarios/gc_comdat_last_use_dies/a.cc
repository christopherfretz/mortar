// Unchanged. twice's first copy (the elected keeper); fa has no caller.
inline int twice(int x) { return x * 2; }
int fa(int x) { return twice(x); }
