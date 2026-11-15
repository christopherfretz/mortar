// GCC main calling kiln-built code: virtual calls into kiln classes,
// dynamic_cast/typeid across the boundary, exceptions thrown by kiln code
// caught by GCC code, and a GCC callback throwing through kiln frames.
#include "shared.h"
#include <cstdio>
#include <memory>
#include <typeinfo>
static int gcc_cb(int n) { if (n < 0) throw std::runtime_error("gcc callback"); return n + 100; }
int main() {
    for (int k = 0; k < 2; ++k) {
        std::unique_ptr<Shape> s(kiln_make(k));
        std::printf("%s %.1f %s\n", s->name(), s->area(), typeid(*s).name());
    }
    std::printf("kiln_work %d\n", kiln_work(4));
    try { kiln_work(0); } catch (const std::logic_error &e) { std::printf("gcc caught kiln %s\n", e.what()); }
    std::printf("cb %d\n", kiln_call_back(gcc_cb, 1));
    try { kiln_call_back(gcc_cb, -1); } catch (const std::runtime_error &e) { std::printf("gcc caught own throw through kiln %s\n", e.what()); }
    std::printf("twice %d hits %d one address %d\n", twice(20), hits(), kiln_hits_addr() == &hits());
    return 0;
}
