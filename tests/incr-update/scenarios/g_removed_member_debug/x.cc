// The changed file: under EDIT it calls h_extra (lib.a's h.o is extracted).
int h_extra(int);
int x_value(int x) {
  int base = x + 1;
#ifdef EDIT
  base += h_extra(x);
#endif
  return base;
}
