// libcxx.so: C++ library with -g
#include <string>
namespace ns {
struct Widget {
    int v;
    explicit Widget(int x) : v(x) {}
    int get() const;
};
int Widget::get() const { return v * 10; }   // CXX_GET_LINE
thread_local int cxx_tls = 3;
std::string cxx_name = "widget";
int cxxfn(int x)
{
    Widget w(x);
    cxx_tls += 1;
    return w.get();          // CXX_CALL_LINE
}
}
