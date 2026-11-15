#include "v.h"
int make_seed() { static int calls = 0; return 40 + ++calls; }
int *seed_a() { return &seed; }
const std::string *greet_a() { return &greeting; }
int count_a() { S::count += 1; return S::count + tagged<long> + int(zero<double>); }
