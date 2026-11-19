// Unchanged. A function with a try/catch: its .gcc_except_table lives
// while it does.
int thrower(int x) {
  try {
    if (x > 100) throw x;
    return x;
  } catch (int e) {
    return -e;
  }
}
