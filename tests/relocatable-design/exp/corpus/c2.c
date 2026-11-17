/* c2.c: second object of the C component. */
#include "rec.h"
int hid(void);
int wk(void) { return 2; }
long common_x;                           /* common, larger: merged size 8 */
__thread int tv = 3;                     /* TLS */
const char *msg2(void) { return "hello, component"; } /* same string as c1 */
int use_hid(void) { return hid() + tv; }
__attribute__((constructor(101))) static void ctor101(void) { rec(101); }
__attribute__((constructor)) static void ctor_default2(void) { rec(2); }
