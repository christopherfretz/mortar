/* exported short then a local short compared with imm16 */
short gs = 1;
static short ls = 2;
int f(void) { return ls == 0x1234; }
void g(void) { ls = 0x4321; }
