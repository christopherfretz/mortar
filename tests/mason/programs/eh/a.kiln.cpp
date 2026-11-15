// Exceptions thrown in one translation unit, caught in another; cleanups
// run during unwinding; rethrow; catch by base; nested try; std::exception
// types from the library.
#include <cstdio>
#include <stdexcept>
#include <string>
struct MyError : std::runtime_error {
    int code;
    MyError(const std::string &w, int c) : std::runtime_error(w), code(c) {}
};
struct Noisy {
    const char *n;
    explicit Noisy(const char *s) : n(s) {}
    ~Noisy() { std::printf("~Noisy(%s)\n", n); }
};
void thrower(int kind) {
    Noisy a("thrower");
    switch (kind) {
    case 0: throw MyError("mine", 7);
    case 1: throw 42;
    case 2: throw std::out_of_range("range");
    case 3: { std::string s; (void)s.at(5); }
    default: break;
    }
}
void rethrower(int kind) {
    Noisy b("rethrower");
    try {
        thrower(kind);
    } catch (...) {
        std::printf("rethrowing %d\n", kind);
        throw;
    }
}
