const char *ls(void){ return "hello, a much wider world: the string grows past any alignment slack, the string grows past any alignment slack, the string grows past any alignment slack, the string grows past any alignment slack, the string grows past any alignment slack, the string grows past any alignment slack, end"; }
int lf(int x){ return ls()[x & 3]; }
