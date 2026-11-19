// The changed file: one immediate of the body (EDIT: 9 in place of 7),
// on the same line, so only .text's bytes change.
#ifndef EDIT
#define C_K 7
#else
#define C_K 9
#endif
int c_value(int x) {
  int scaled = x * C_K;
  return scaled + 1;
}
