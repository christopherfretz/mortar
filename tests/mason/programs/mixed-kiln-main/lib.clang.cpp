#include "shared.h"
int clang_work(int n) {
    if (n > 100) throw std::out_of_range("clang: too big");
    Acc<long> a; a.add(n); a.add(n);
    return int(a.total());
}
const void *clang_hits_addr() { return &hits(); }
