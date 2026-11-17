#include <stdexcept>
#include <string>
inline std::string greet(int n) { return "p" + std::to_string(n); }
__attribute__((visibility("hidden"))) int hidden_helper(int x) { if (x < 0) throw std::runtime_error("neg"); return x * 2; }
int api1(int x) { try { return hidden_helper(x) + (int)greet(x).size(); } catch (const std::exception &) { return -1; } }
