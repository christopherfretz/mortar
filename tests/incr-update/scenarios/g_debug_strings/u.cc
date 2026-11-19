// Unchanged. u_dead is called only by c_fill_call (itself uncalled), so
// --gc-sections drops both on every link.
int u_helper(int x) {
  int tripled = x * 3;
  return tripled + 1;
}
int u_dead(int x) { return x - 1; }
