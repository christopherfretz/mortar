// Code every compiler instantiates the same way (ODR-identical): the link
// keeps one copy of each COMDAT group, whichever compiler made it.
#pragma once
#include <stdexcept>
#include <string>
#include <vector>
inline int &hits() { static int h = 0; return h; }
template <class T> struct Acc {
    std::vector<T> items;
    void add(const T &t) { items.push_back(t); ++hits(); }
    T total() const { T s{}; for (const T &x : items) s += x; return s; }
};
struct LibError : std::runtime_error {
    int code;
    LibError(const char *w, int c) : std::runtime_error(w), code(c) {}
};
// Defined by the library compilers.
int gcc_work(int n);                     // throws LibError for n < 0
int gcc_call_back(int (*cb)(int), int n); // calls cb through GCC frames
std::string gcc_join(const std::vector<std::string> &v);
int clang_work(int n);                   // throws std::out_of_range for n > 100
const void *gcc_hits_addr();
const void *clang_hits_addr();
