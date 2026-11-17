int m1(int); int m2(int); int m3(int);
extern "C" int printf(const char*, ...);
int main() { printf("%d %d %d\n", m1(2), m2(3), m3(4)); return 0; }
