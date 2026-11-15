#include <iostream>
#include <cmath>
#include <cstring>
#include <thread>
int main(int c, char **v) { std::thread t([]{}); t.join(); char b[8]; std::memcpy(b, v[0], 2); std::cout << std::sqrt((double)c) << std::cosh(c * 1.0) << b[0] << std::endl; }
