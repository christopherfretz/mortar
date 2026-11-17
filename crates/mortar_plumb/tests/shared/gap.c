/* W44 (spec/exp2/gapdemo/gap2.c): GCC AArch64 -fPIE reaches its own
   exported g through the section anchor and an instruction immediate. */
static int h1 = 1, h2 = 2;
int g = 3;
static int h3 = 4;
int all(void) { return h1 + h2 + g + h3; }
void set(int v) { h1 = v; g = v; h3 = v; }
