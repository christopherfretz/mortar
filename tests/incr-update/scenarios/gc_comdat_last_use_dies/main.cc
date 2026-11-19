// The changed file. EDIT: fb's only live call removed.
#include <cstdio>
int fb(int);
int main(int c, char **) {
#ifdef EDIT
  std::printf("%d\n", c);
#else
  std::printf("%d\n", fb(c));
#endif
  return 0;
}
