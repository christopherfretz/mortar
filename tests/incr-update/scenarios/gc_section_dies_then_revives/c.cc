// The changed file. EDIT: c_entry stops calling u_helper, the only caller
// of an unchanged file's function, so u_helper's section dies.
int u_helper(int);
int c_entry(int x) {
#ifdef EDIT
  return x + 100;
#else
  return u_helper(x);
#endif
}
