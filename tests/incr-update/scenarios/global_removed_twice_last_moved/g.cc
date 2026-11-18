// The changed file. DROP1: no g_first; DROPLAST: no g_last.
#ifndef DROP1
int g_first(int x) { return x + 1; }
#endif
int g_mid(int x) { return x + 2; }
int g_keep(int x) { return x * 2; }
#ifndef DROPLAST
int g_last(int x) { return x + 3; }
#endif
