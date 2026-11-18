__attribute__((used)) static int counter_b = 11;
__attribute__((visibility("hidden"))) int hidden_b = 5;

__attribute__((noinline, used)) static int helper_b(int x) { return x + counter_b; }

int bfun(int x) { return helper_b(x) * 2; }
