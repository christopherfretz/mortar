/* libplug.so: dlopen'ed by the host */
int plug_global = 99;
int plugfn(int x)
{
    int r = x * 3;          /* PLUG_LINE */
    return r;
}
