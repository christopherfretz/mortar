// A static C++ program with exceptions, iostream and thread-local data.
#include <iostream>
#include <stdexcept>
#include <string>
thread_local std::string tl = "tls";
static int thrower(int v) { if (v > 2) throw std::runtime_error("big"); return v; }
int main(int argc, char **) {
  try { thrower(argc + 5); } catch (const std::exception &e) { std::cout << e.what() << tl << '\n'; }
  return 0;
}
