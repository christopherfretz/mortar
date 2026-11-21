// The changed file: one immediate of c_value's body (EDIT: 9 in place of
// 7), on the same line, so only .text's bytes change. The 900 types and
// functions before it give the object, and its .debug_info, more than
// 64 KiB, so the staged store (the default) keeps .debug_info on disk
// (spec/input.md §11.1 (b), mortar_input::staged::SMALL): the update reads
// its unit headers from the file (WP-278). The lane's other objects are
// small enough to stay resident whole.
#ifndef EDIT
#define C_K 7
#else
#define C_K 9
#endif
#define C_DEF(n)                                                     \
  struct S##n {                                                      \
    int a;                                                           \
    long b;                                                          \
    double c;                                                        \
    unsigned e;                                                      \
  };                                                                 \
  int f##n(struct S##n *s, int x) {                                  \
    int loc = s->a + x;                                              \
    return (int)(s->b * loc + s->e);                                 \
  }
#define C_D10(n)                                                     \
  C_DEF(n##0) C_DEF(n##1) C_DEF(n##2) C_DEF(n##3) C_DEF(n##4)        \
  C_DEF(n##5) C_DEF(n##6) C_DEF(n##7) C_DEF(n##8) C_DEF(n##9)
#define C_D100(n)                                                    \
  C_D10(n##0) C_D10(n##1) C_D10(n##2) C_D10(n##3) C_D10(n##4)        \
  C_D10(n##5) C_D10(n##6) C_D10(n##7) C_D10(n##8) C_D10(n##9)
C_D100(1) C_D100(2) C_D100(3) C_D100(4) C_D100(5)
C_D100(6) C_D100(7) C_D100(8) C_D100(9)
int c_value(int x) {
  int scaled = x * C_K;
  return scaled + 1;
}
