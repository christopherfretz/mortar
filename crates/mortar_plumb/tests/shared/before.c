/* D-117 "before the start" (tests/shared-design/sup/c.c): correct
   -fPIC code forming tab - 1; GCC -O0 names .data + 8, inside gvar. */
int gvar[4] = {1,2,3,4};
static double tab[8] = {1,2,3,4,5,6,7,8};
int gcount = 7;
static int w[5] = {1,2,3,4,5};
double f(int n){ double r=0; double *p = tab - 1; for(int i=1;i<=n;i++){ p[i] = p[i]*2+gvar[0]; r+=p[i]; } return r; }
int g(int i){ return w[i-1] + w[i-2] + gcount; }
int h(int i,int j){ return w[i-j-3]; }
