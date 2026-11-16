static int h1 = 1, h2 = 2;
int g = 3;
static int h3 = 4;
int all(void) { return h1 + h2 + g + h3; }
void set(int v) { h1 = v; g = v; h3 = v; }
