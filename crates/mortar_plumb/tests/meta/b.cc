inline int vi() { return 1; }
__attribute__((visibility("hidden"))) inline int hv() { return 10; }
__attribute__((weak)) int wd() { return 8; }
int use_b() { return vi() + hv() + wd(); }
extern "C" void _start() { for (;;) { use_b(); } }
