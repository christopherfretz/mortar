int g = 7;
static int s1 = 3;
int h[4] = {1,2,3,4};
int get(void){ return g + s1 + h[2]; }
int *addr(void){ return &g; }
void set(int v){ g = v; h[1] = v; s1 = v; }
