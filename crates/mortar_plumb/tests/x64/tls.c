/* Every TLS model from one unit: built -fPIC (GD, LD), -fPIC
   -mtls-dialect=gnu2 (TLSDESC) and not PIC (IE, LE). */
__thread int t_def = 7;
static __thread long t_loc[4] = {1, 2, 3, 4};
extern __thread int t_ext;
int get_def(void) { return t_def; }
long get_loc(int i) { return t_loc[i]; }
int get_ext(void) { return t_ext; }
