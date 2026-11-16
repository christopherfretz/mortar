/* correct -fPIC code: an exported byte then a local static byte, tested with an imm8 */
char gflag = 1;
static char inited = 0;
int f(void) { if (inited != 7) { inited = 7; return 1; } return 0; }
