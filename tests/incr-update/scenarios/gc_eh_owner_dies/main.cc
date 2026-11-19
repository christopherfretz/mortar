// The changed file. EDIT: thrower's only call removed.
#include <cstdio>
int thrower(int);
int main(int c, char **) {
#ifdef EDIT
  std::printf("%d\n", c);
#else
  std::printf("%d\n", thrower(c));
#endif
  return 0;
}
