__attribute__((used)) static int counter_a = 7;
__attribute__((used)) static const char *greeting_a = "hello from a";

__attribute__((noinline, used)) static int helper_a(int x) { return x * counter_a + greeting_a[0]; }
__attribute__((noinline, used)) static int other_helper_a(int x) { return helper_a(x) - 1; }

int afun(int x) { return other_helper_a(x) + helper_a(x); }
