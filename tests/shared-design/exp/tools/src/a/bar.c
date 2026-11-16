/* libbar.so: the second library of the exe -> libfoo -> libbar backtrace */
int bar_fn(int x)
{
    int y = x + 1;          /* BAR_LINE */
    return y;
}
