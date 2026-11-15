#include "t.h"
int tu_a() {
    Box<int> b(3);
    Box<std::string> s("ab");
    registry()["a"] = sum(std::vector<int>{1, 2, 3});
    return b.twice() + int(s.twice().size()) + Box<int>::instances;
}
const void *a_sum_int() { return addr_of_sum<int>(); }
const void *a_instances() { return &Box<int>::instances; }
