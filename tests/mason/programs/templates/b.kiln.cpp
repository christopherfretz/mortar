#include "t.h"
int tu_b() {
    Box<int> b(5);
    Box<double> d(1.5);
    registry()["b"] = int(sum(std::vector<double>{1.5, 2.5}));
    return b.twice() + int(d.twice()) + Box<int>::instances;
}
const void *b_sum_int() { return addr_of_sum<int>(); }
const void *b_instances() { return &Box<int>::instances; }
