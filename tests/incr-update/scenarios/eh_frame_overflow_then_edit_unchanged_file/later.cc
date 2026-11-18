// Unchanged until hop 2. LATER: adds another value to what it caught.
#ifdef LATER
#define LATER_ADD 1000
#else
#define LATER_ADD 100
#endif
int may_throw(int x) {
  if (x % 3 == 0) throw x;
  if (x % 3 == 1) throw (long)x;
  return x;
}
int later(int x) {
  try { return may_throw(x); } catch (int e) { return e + LATER_ADD; } catch (long e) { return (int)e; }
}
