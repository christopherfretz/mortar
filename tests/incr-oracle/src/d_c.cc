#include <cstdio>
thread_local long c_var = 0;
long ga(); long gb(); long ge(); long gd();
int main(){ c_var += 3; std::printf("%ld\n", c_var + ga() + gb() + ge() + gd()); return 0; }
