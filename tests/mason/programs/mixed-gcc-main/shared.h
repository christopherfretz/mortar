#pragma once
#include <stdexcept>
#include <string>
inline int &hits() { static int h = 0; return h; }
template <class T> T twice(T t) { ++hits(); return t + t; }
struct KilnError : std::logic_error {
    explicit KilnError(const std::string &s) : std::logic_error(s) {}
};
struct Shape { virtual ~Shape() {} virtual double area() const = 0; virtual const char *name() const = 0; };
Shape *kiln_make(int k);                 // kiln-built classes, used by GCC code
int kiln_work(int n);                    // throws KilnError for n == 0
int kiln_call_back(int (*cb)(int), int n);
const void *kiln_hits_addr();
