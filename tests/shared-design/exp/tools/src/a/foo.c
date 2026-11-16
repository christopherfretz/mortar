/* libfoo.so: -g, -soname libfoo.so, DT_NEEDED libbar.so */
extern int bar_fn(int);
int foo_global = 42;
__thread int foo_tls = 7;
int libfn(int x)
{
    int r = bar_fn(x * 2);  /* FOO_LINE */
    foo_tls += r;
    return r;
}
