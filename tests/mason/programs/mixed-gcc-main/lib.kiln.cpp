#include "shared.h"
#include <cstdio>
struct Square : Shape { double s; explicit Square(double x) : s(x) {} double area() const override { return s * s; } const char *name() const override { return "square"; } };
struct Circle : Shape { double r; explicit Circle(double x) : r(x) {} double area() const override { return 3.0 * r * r; } const char *name() const override { return "circle"; } };
struct Guard { ~Guard() { std::printf("kiln guard\n"); } };
Shape *kiln_make(int k) { if (k == 0) return new Square(2); return new Circle(k); }
int kiln_work(int n) {
    Guard g;
    if (n == 0) throw KilnError("kiln: zero");
    return twice(n);
}
int kiln_call_back(int (*cb)(int), int n) { Guard g; return cb(n) + twice(1); }
const void *kiln_hits_addr() { return &hits(); }
