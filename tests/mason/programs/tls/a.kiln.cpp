// thread_local with constant and dynamic initialisation, defined in one TU
// and used from another (the TLS wrapper and init functions), destructors of
// thread_local objects, and several threads.
#include <cstdio>
#include <string>
struct Tracked {
    std::string name;
    explicit Tracked(const char *n) : name(n) {}
    ~Tracked() { std::printf("~Tracked %s\n", name.c_str()); }
};
thread_local int counter = 100;
int next_id() { static int id = 0; return ++id; }
thread_local int dyn = next_id() * 1000;
thread_local Tracked tracked("t");
int bump() { return ++counter; }
const char *tracked_name() { return tracked.name.c_str(); }
