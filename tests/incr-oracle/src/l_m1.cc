#include <stdexcept>
#include <cstdio>
int g(int x){ if (x>3) throw std::runtime_error("x"); return x; }
int keep(int); int dropme(int);
int main(){ std::printf("%d %d %d\n", keep(1), keep(9), dropme(9)); }
