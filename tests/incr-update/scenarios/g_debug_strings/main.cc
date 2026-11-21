#include <cstdio>
int c_value(int);
int u_helper(int);
int main(int argc, char **) {
  std::printf("c=%d u=%d\n", c_value(argc + 1), u_helper(argc));
  return 0;
}
