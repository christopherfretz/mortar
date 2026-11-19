// WP-258: two COMDAT functions shared by a.cc and b.cc (b.o's copies are dropped).
namespace wp258 {
template <class T> struct Box { T v; T get() const { return v; } };
#ifdef WP258_NE
inline int twice(int v) { return v * 2 + 3; } // NE: sizes differ on AArch64 too
#elif defined(WP258_O2)
// O2: kept out of line (noinline), with a location list for v and r
__attribute__((noinline)) inline int twice(int v) { int r = v * 2; asm volatile("" : "+r"(r)); return r + v; }
#else
inline int twice(int v) { return v * 2; }
#endif
}
