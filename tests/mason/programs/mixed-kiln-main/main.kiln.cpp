// kiln main calling GCC- and Clang-built code; exceptions thrown by GCC and
// Clang code caught by kiln code; a kiln callback that throws through GCC
// frames back into kiln.
#include "shared.h"
#include <cstdio>
static int kiln_cb(int n) {
    if (n == 13) throw LibError("from kiln callback", 13);
    return n * 2;
}
int main() {
    std::printf("gcc_work %d\n", gcc_work(5));
    try { gcc_work(-3); } catch (const LibError &e) { std::printf("kiln caught gcc %s %d\n", e.what(), e.code); }
    try { clang_work(1000); } catch (const std::out_of_range &e) { std::printf("kiln caught clang %s\n", e.what()); }
    std::printf("clang_work %d\n", clang_work(21));
    std::printf("callback %d\n", gcc_call_back(kiln_cb, 4));
    try { gcc_call_back(kiln_cb, 13); } catch (const LibError &e) { std::printf("kiln caught own throw through gcc %d\n", e.code); }
    std::printf("join %s\n", gcc_join({"x", "y", "z"}).c_str());
    Acc<int> a; a.add(1);
    std::printf("hits %d one address %d\n", hits(), gcc_hits_addr() == &hits() && clang_hits_addr() == &hits());
    return 0;
}
