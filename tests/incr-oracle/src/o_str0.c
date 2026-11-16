const char *ls(void){ return "hello"; }
int lf(int x){ return ls()[x & 3]; }
