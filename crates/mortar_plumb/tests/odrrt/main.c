/* The executable of tests/odrrt.rs (spec/plumb.md §13, WP-197): a user
   .preinit_array entry and a priority-0 constructor, which run before the
   runtime's entry, and an ordinary constructor, which runs after it. */
volatile int seen;
static void pre(void) { seen = 1; }
static void first(void) { seen = 2; }
__attribute__((section(".preinit_array"), used)) static void (*const pre_p)(void) = pre;
__attribute__((section(".init_array.00000"), used)) static void (*const first_p)(void) = first;
__attribute__((constructor)) static void later(void) { seen = 3; }
int user_fn(void) { return 1; }
void _start(void) { for (;;) user_fn(); }
