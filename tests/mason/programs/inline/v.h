// Inline variables (C++17) with dynamic initialisation, defined in a header
// included by several TUs: one object, initialised once.
#pragma once
#include <string>
int make_seed();
inline int seed = make_seed();
inline std::string greeting = "hi " + std::to_string(seed);
struct S { static inline int count = 0; static constexpr int k = 3; };
template <class T> inline T zero = T(0);
template <class T> inline int tagged = sizeof(T) * 10 + S::k;
int *seed_a();
int *seed_b();
const std::string *greet_a();
int count_a();
