#include <cstdio>
#include "v.h"
int main() {
    S::count += 10;
    int c = count_a();
    std::printf("seed %d same %d greeting %s same %d count %d tagged<char> %d\n", seed,
                seed_a() == &seed && seed_b() == &seed, greeting.c_str(), greet_a() == &greeting, c,
                tagged<char>);
    return 0;
}
