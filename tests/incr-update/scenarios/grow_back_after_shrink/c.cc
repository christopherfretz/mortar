// The changed file. The first link has the grown body (BIG); hop 1
// shrinks it, hop 2 grows it back into the region it left, hop 3 grows
// it past the first link's size (HUGE). c_other follows it in the same
// .text, so it moves inside the section at every hop. c_helper is an
// inline function: its COMDAT section (.text._Z8c_helperi) follows .text
// in c.o's region, so the bytes .text frees at hop 1 are a hole INSIDE
// the region, not at its end (protoc's main.cc.o has the same shape:
// .text, then its inline functions' sections).
#if defined(HUGE)
int c_value(int x) {
  volatile int k = x * 7 + 1;
  k += 3;
  k += 5;
  k += 11;
  k += 13;
  return k;
}
#elif defined(BIG)
int c_value(int x) {
  volatile int k = x * 7 + 1;
  k += 3;
  k += 5;
  return k;
}
#else
int c_value(int x) {
  volatile int k = x * 7 + 1;
  return k;
}
#endif
inline int c_helper(int x) { return x * 5 - 4; }
int c_other(int x) { return c_helper(x) - 2; }
