#include <cstdio>
#include <exception>
#include <stdexcept>
#include <string>
struct MyError : std::runtime_error {
    int code;
    MyError(const std::string &w, int c) : std::runtime_error(w), code(c) {}
};
void thrower(int kind);
void rethrower(int kind);
static int nested() {
    try {
        try {
            rethrower(0);
        } catch (const MyError &e) {
            std::printf("inner caught %s %d\n", e.what(), e.code);
            throw std::logic_error("translated");
        }
    } catch (const std::logic_error &e) {
        std::printf("outer caught %s\n", e.what());
        return 1;
    }
    return 0;
}
int main() {
    for (int k = 0; k < 5; ++k) {
        try {
            rethrower(k);
            std::printf("kind %d: no throw\n", k);
        } catch (const MyError &e) {
            std::printf("kind %d: MyError %s %d\n", k, e.what(), e.code);
        } catch (int i) {
            std::printf("kind %d: int %d\n", k, i);
        } catch (const std::exception &e) {
            std::printf("kind %d: std::exception %s\n", k, e.what());
        }
    }
    std::exception_ptr p;
    try { thrower(1); } catch (...) { p = std::current_exception(); }
    try { std::rethrow_exception(p); } catch (int i) { std::printf("exception_ptr %d\n", i); }
    return nested() + 10;
}
