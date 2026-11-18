// The changed file. EDIT: a static constructor (one more .init_array entry).
extern int g_count;
#ifdef EDIT
static struct A { A() { g_count += 10; } } a_ctor;
#endif
int a_fn() { return 1; }
