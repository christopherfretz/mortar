static int s1[4] = {1,2,3,4};
int g = 5;
int h = 6;
static int s2 = 7;
int geth(void){ return h; }
int getg(void){ return g; }
int sum(void){ return g + h + s1[2] + s2; }
int *endp(void){ return &s1[4]; }
static int *const pend = &s1[4];
int *getpend(void){ return pend; }
