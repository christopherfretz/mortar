/* WP-161: an executable whose `foo_fn` and `foo_data` come from an archive
   member (`xfoo.c`) that `--exclude-libs` may exclude, while `libxdd.so`
   defines `foo_fn` too (D-21's export of a definition a shared object
   also has); `keep_fn` is what `x.list` lists. */
int foo_fn(void);
extern int foo_data;
int other_fn(void);
int keep_fn(void) { return 7; }
int main(void) { return foo_fn() + foo_data + other_fn() == 9 ? 0 : 1; }
