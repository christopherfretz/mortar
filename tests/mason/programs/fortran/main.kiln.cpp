#include <cstdio>
#include <vector>
extern "C" double f_dot(int n, const double *a, const double *b);
extern "C" int f_sum_squares(int n);
extern "C" void f_print(int n);
extern "C" int cxx_square(int x) { return x * x; }
int main() {
    std::vector<double> a{1, 2, 3}, b{4, 5, 6};
    std::printf("dot %.1f\n", f_dot(3, a.data(), b.data()));
    std::fflush(stdout);
    std::printf("sum_squares %d\n", f_sum_squares(4));
    std::fflush(stdout);
    f_print(7);
    return 0;
}
