#include <string>
inline std::string greet(int n) { return "p" + std::to_string(n); }
int hidden_helper(int);
int api2(int x) { return hidden_helper(x + 1) + (int)greet(x).size(); }
