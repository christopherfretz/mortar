#include "h.hh"
int c_f(int x) { Box<long> b{x}; return (int)b.get() + wp250_twice(CHAR_BIT); }
