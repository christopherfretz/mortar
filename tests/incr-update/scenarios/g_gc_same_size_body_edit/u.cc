// Unchanged. u_dead has no caller (--gc-sections drops it on every link).
int u_helper(int x) { return x * 3 + 1; }
int u_dead(int x) { return x - 1; }
