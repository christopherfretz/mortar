#include <cstdio>
#include "t.h"
int main() {
    int a = tu_a();
    int b = tu_b();
    Box<int> m(1);
    int s = sum(std::vector<int>{4, 5});
    std::printf("a %d b %d s %d instances %d counter %d\n", a, b, s, Box<int>::instances,
                *shared_counter());
    std::printf("sum<int> one address %d\n", a_sum_int() == b_sum_int() && a_sum_int() == addr_of_sum<int>());
    std::printf("instances one address %d\n", a_instances() == b_instances() && a_instances() == &Box<int>::instances);
    for (auto &kv : registry()) std::printf("%s=%d\n", kv.first.c_str(), kv.second);
    return 0;
}
