// The changed file. EDIT: neither the literal nor the constant.
#ifndef EDIT
const char *c_str() { return "the merged constant C"; }
double c_num(double x) { return x * 3.25e7; }
#else
const char *c_str() { return "x"; }
double c_num(double x) { return x; }
#endif
