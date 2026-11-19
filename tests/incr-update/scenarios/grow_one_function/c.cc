// The changed file: c_value's body grows (EDIT); c_other follows it in
// the same .text, so it moves inside the section.
#ifndef EDIT
int c_value(int x) { return x * 7 + 1; }
#else
int c_value(int x) {
  int y = x * 7 + 1;
  if (y > 100)
    y -= 3;
  for (int i = 0; i < x; ++i)
    y += i & 1;
  return y - x / 2;
}
#endif
int c_other(int x) { return x - 2; }
