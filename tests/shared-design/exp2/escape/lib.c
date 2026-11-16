__attribute__((visibility("protected"))) int g = 3;
int all(void) { return g; }
void set(int v) { g = v; }
