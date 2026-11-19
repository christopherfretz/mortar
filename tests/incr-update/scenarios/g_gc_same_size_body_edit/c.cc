// The changed file. EDIT: another immediate on the same line, the same
// call: u_helper stays live, u_dead stays dead.
#ifndef EDIT
#define C_K 5
#else
#define C_K 7
#endif
int u_helper(int);
int c_entry(int x) {
  int got = u_helper(x);
  return got + C_K;
}
