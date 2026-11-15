extern int f(void) __attribute__((visibility("hidden")));
extern int (*pdef)(void);
int (*get_hid(void))(void) { return f; }
int main(void) { return pdef() + get_hid()() + f() - 0x66; }
