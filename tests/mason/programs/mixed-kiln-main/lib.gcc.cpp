#include "shared.h"
#include <cstdio>
struct Guard { const char *n; ~Guard() { std::printf("gcc guard %s\n", n); } };
int gcc_work(int n) {
    Guard g{"work"};
    if (n < 0) throw LibError("negative", n);
    Acc<int> a; for (int i = 0; i < n; ++i) a.add(i);
    return a.total();
}
int gcc_call_back(int (*cb)(int), int n) { Guard g{"callback"}; return cb(n) + 1; }
std::string gcc_join(const std::vector<std::string> &v) {
    Acc<std::string> a; for (auto &s : v) a.add(s + ",");
    return a.total();
}
const void *gcc_hits_addr() { return &hits(); }
