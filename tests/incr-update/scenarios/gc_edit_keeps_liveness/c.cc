// The changed file. EDIT: another immediate, the same call: u_helper stays
// live, u_dead stays dead.
int u_helper(int);
int c_entry(int x) {
#ifdef EDIT
  return u_helper(x) + 7;
#else
  return u_helper(x) + 5;
#endif
}
