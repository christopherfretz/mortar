#include <stdexcept>
__attribute__((noinline)) int thrower(int x) { if (x == 1) throw 42; if (x == 2) throw std::runtime_error("e"); return x; }
int api1(int x) { try { return thrower(x); } catch (const std::exception &) { return -1; } }
