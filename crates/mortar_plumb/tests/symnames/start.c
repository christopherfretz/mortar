/* WP-246's fixtures (tests/symnames.rs): a program with locals in each
   file, a hidden global (demoted, synth.md S-77) and plain globals. */
int afun(int);
int bfun(int);
extern int hidden_b;
static volatile int start_state;

__attribute__((noinline, used)) static int start_helper(int x) { return x + start_state; }

void _start(void) {
  start_state = afun(1) + bfun(2) + hidden_b + start_helper(3);
  for (;;) {
  }
}
