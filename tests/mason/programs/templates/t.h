// One template, one inline function with a local static, one class
// template's static member: instantiated in three translation units, so
// every object has the same COMDAT groups; the linker keeps one copy, and
// every TU must see the same address.
#pragma once
#include <map>
#include <string>
#include <vector>
template <class T> struct Box {
    static int instances;
    T value;
    explicit Box(T v) : value(v) { ++instances; }
    T twice() const { return value + value; }
    virtual ~Box() {}
};
template <class T> int Box<T>::instances = 0;
inline int *shared_counter() { static int c = 0; return &c; }
template <class T> T sum(const std::vector<T> &v) {
    T s{};
    for (const T &x : v) s = s + x;
    ++*shared_counter();
    return s;
}
template <class T> const void *addr_of_sum() { return reinterpret_cast<const void *>(&sum<T>); }
using Registry = std::map<std::string, int>;
inline Registry &registry() { static Registry r; return r; }
int tu_a();
int tu_b();
const void *a_sum_int();
const void *b_sum_int();
const void *a_instances();
const void *b_instances();
