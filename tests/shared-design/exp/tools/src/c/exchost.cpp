// C++ across the boundary. argv[1] = a test number runs only that test (so
// one std::terminate does not hide the others); no argument runs all.
#include "exc.h"
#include <cstdio>
#include <cstdlib>
#include <typeinfo>
struct HostDerived : libns::Base { int id() const override { return 7; } };
static void cb_throw_host() { throw HostDerived(); }
static void cb_throw_libtype() { throw libns::Derived(); }
static int fails;
static void check(bool c, const char *what) { std::printf("%s %s\n", c ? "OK" : "FAIL", what); fails += !c; }
static void t0() { check(libns::self_catch() == 42, "t0 throw+catch inside the library"); }
static void t1()
{
    int got = 0;
    try { libns::throw_lib(5); } catch (const libns::LibError &e) { got = e.code; }
    check(got == 5, "t1 catch libns::LibError (exact type) thrown in library");
}
static void t2()
{
    int got = 0;
    try { libns::throw_lib(6); } catch (const std::runtime_error &) { got = 6; }
    check(got == 6, "t2 catch std::runtime_error (base) thrown in library");
}
static void t3()
{
    int got = 0;
    try { libns::throw_lib(8); } catch (const std::exception &e) { got = std::string(e.what()) == "from library"; }
    check(got == 1, "t3 catch std::exception, what()");
}
static void t4() { check(libns::call_cb_catch(cb_throw_host) == 107, "t4 library catches host HostDerived as libns::Base&"); }
static void t5() { check(libns::call_cb_catch(cb_throw_libtype) == 102, "t5 library catches libns::Derived thrown by host"); }
static void t6()
{
    libns::Base *b = libns::make_derived();
    check(dynamic_cast<libns::Derived *>(b) != nullptr, "t6 host dynamic_cast<Derived*> of library object");
    check(typeid(*b) == typeid(libns::Derived), "t6 typeid equality across boundary");
    check(&typeid(libns::Derived) == &libns::derived_ti(), "t6 typeinfo object address unique");
    HostDerived hd;
    check(libns::is_derived(&hd) == 0 && libns::is_derived(b) == 1, "t6 library dynamic_cast");
    delete b;
}
static void t7() { check(libns::greet("world") == "hello world 42", "t7 std::string/ostringstream/cout in library"); }
int main(int argc, char **argv)
{
    std::setvbuf(stdout, nullptr, _IONBF, 0);
    void (*t[])() = { t0, t1, t2, t3, t4, t5, t6, t7 };
    if (argc > 1) { t[std::atoi(argv[1])](); return fails; }
    for (auto f : t) f();
    std::printf("%s exc-all\n", fails ? "FAIL" : "OK");
    return fails;
}
