#include <stdexcept>
#include <string>
extern "C" int thrower(int x){ if (x) throw std::runtime_error(std::to_string(x)); return 0; }
