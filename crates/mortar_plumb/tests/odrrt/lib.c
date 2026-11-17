/* The shared object of tests/odrrt.rs (spec/plumb.md §13, WP-197): a
   priority-0 constructor before the runtime's entry, one of priority 101
   and an ordinary one after it. */
volatile int seen;
static void first(void) { seen = 2; }
__attribute__((section(".init_array.00000"), used)) static void (*const first_p)(void) = first;
__attribute__((constructor(101))) static void early(void) { seen = 4; }
__attribute__((constructor)) static void later(void) { seen = 3; }
int user_fn(void) { return 1; }
