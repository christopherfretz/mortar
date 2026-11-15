#include <cstdio>
struct Rec {
    const char *n;
    explicit Rec(const char *s) : n(s) { std::printf("ctor %s\n", n); }
    ~Rec() { std::printf("dtor %s\n", n); }
};
Rec m1("m1");
Rec &lazy();
int register_atexit();
int main() {
    std::printf("main\n");
    register_atexit();
    lazy();
    lazy();
    std::printf("main returns\n");
    return 0;
}
