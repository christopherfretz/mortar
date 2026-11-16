/* exported int then a local static int stored with an imm32 */
int gcount = 1;
static int state = 2;
void setstate(void) { state = 123456; }
int getstate(void) { return state; }
