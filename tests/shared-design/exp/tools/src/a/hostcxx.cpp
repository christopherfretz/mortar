#include <cstdio>
namespace ns { int cxxfn(int); }
int main()
{
    int r = ns::cxxfn(4);    // HOSTCXX_CALL_LINE
    std::printf("%s cxxfn(4)=%d\n", r == 40 ? "OK" : "FAIL", r);
    return 0;
}
