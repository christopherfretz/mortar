// The changed file. EDIT: refers to mktemp (glibc's .gnu.warning.mktemp:
// the link warns, naming this file as the referrer).
extern "C" char *mktemp(char *);
int c_fn() {
#ifdef EDIT
  char t[] = "/nonexistent-dir/incrXXXXXX";
  return mktemp(t)[0];
#else
  return '/';
#endif
}
