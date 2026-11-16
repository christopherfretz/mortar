inline int vi() { return 1; }
__attribute__((visibility("hidden"))) inline int hv() { return 10; }
__attribute__((weak)) int wd() { return 7; }
__attribute__((visibility("protected"))) int pr() { return 5; }
int strong() { return 6; }
int use_a() { return vi() + hv() + wd() + pr() + strong(); }
// A dynamic initializer: an .init_array entry from this file (P-M3).
static int init_me = strong();
int get_init() { return init_me; }
