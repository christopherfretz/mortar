// Unchanged: its constructor runs after a.o's (link order).
int g_count = 1;
static struct B { B() { g_count *= 2; } } b_ctor;
