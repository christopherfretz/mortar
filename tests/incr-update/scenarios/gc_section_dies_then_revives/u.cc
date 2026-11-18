// Unchanged. u_helper lives only while c.o calls it (--gc-sections).
int u_helper(int x) { return x * 3 + 1; }
int u_keep(int x) { return x - 1; }
