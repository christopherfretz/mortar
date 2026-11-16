#include <cstddef>
struct S { static int count; static const int table[4]; static inline int inl = 3; };
int S::count = 1;
const int S::table[4] = {1,2,3,4};
inline int g_inline = 7;
static int local_arr[8];
int exported_after_local = 5;
constexpr int kTab[] = {3,1,4,1,5,9,2,6};
extern const int kExported[3] = {1,2,3};
static const int kLocal[5] = {1,2,3,4,5};
const int* end_of_local() { return kLocal + 5; }
int sum() { int s = 0; for (int* p = local_arr; p != local_arr + 8; ++p) s += *p; return s + S::count + S::inl + g_inline; }
int lookup(int i) { return kTab[i & 7] + kExported[i % 3] + S::table[i & 3]; }
int* begin_end[2] = { local_arr, local_arr + 8 };
const int* tbl_end = kLocal + 5;
static char buf[16]; char exported_buf[16];
char* bufend() { return buf + sizeof buf; }
int touch(){ buf[3]=1; exported_buf[2]=2; return exported_after_local; }
