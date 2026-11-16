/* bss variants (-fno-common default): exported then static */
int gz;
static int sz;
void z(void) { sz = 99999; }
int zz(void) { return sz; }
char gb;
static char sb;
int b(void) { return sb == 3; }
