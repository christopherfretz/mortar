#include <stdexcept>
extern "C" int thrower(int x) { try { if (x) throw std::runtime_error("x"); return 0; } catch (const std::exception&) { return 42; } }
